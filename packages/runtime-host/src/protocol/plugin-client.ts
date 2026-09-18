/*
 * Licensed to the Apache Software Foundation (ASF) under one
 * or more contributor license agreements.  See the NOTICE file
 * distributed with this work for additional information
 * regarding copyright ownership.  The ASF licenses this file
 * to you under the Apache License, Version 2.0 (the
 * "License"); you may not use this file except in compliance
 * with the License.  You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing,
 * software distributed under the License is distributed on an
 * "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
 * KIND, either express or implied.  See the License for the
 * specific language governing permissions and limitations
 * under the License.
 */

import {
  requireCount,
  requireEncodedByteLimit,
  requireExactRecord,
  requireRecord,
  requireShapedRecord,
  requireString,
} from './codec.js';
import { invalidProtocolFrame } from './errors.js';
import { defineOperation } from './operation-spec.js';

export interface PluginClientCursor {
  revision: string;
  afterEntry: string;
}

export interface PluginClientChangedFrame {
  readonly kind: 'plugin.client.changed';
  readonly revision: string;
}
export function decodePluginClientChangedFrame(value: unknown): PluginClientChangedFrame {
  const row = requireExactRecord(value, 'Client change', ['kind', 'revision']);
  if (row.kind !== 'plugin.client.changed')
    throw invalidProtocolFrame('Invalid Client change kind');
  return { kind: 'plugin.client.changed', revision: digest(row.revision) };
}
export interface PluginClientDescriptor {
  entryId: string;
  extensionId: string;
  activation: string;
  contentDigest: string;
  clientDigest: string;
  sdkVersion: number;
  totalBytes: number;
  dependencies: string[];
  config: unknown;
}
export type PluginClientQueryInput =
  | { kind: 'snapshot'; cursor?: PluginClientCursor | null }
  | { kind: 'bundle'; entryId: string; activation: string; clientDigest: string; offset: number };
export type PluginClientQueryResult =
  | {
      kind: 'snapshot';
      revision: string;
      entries: PluginClientDescriptor[];
      nextCursor: PluginClientCursor | null;
    }
  | {
      kind: 'bundle';
      entryId: string;
      activation: string;
      clientDigest: string;
      offset: number;
      totalBytes: number;
      content: string;
      nextOffset: number | null;
    };

export const PLUGIN_CLIENT_OPERATION_SPECS = {
  'plugin.client.query': defineOperation<
    PluginClientQueryInput,
    PluginClientQueryResult,
    | 'host_not_ready'
    | 'host_draining'
    | 'invalid_request'
    | 'operation_unavailable'
    | 'operation_conflict'
    | 'stale_cursor'
    | 'internal_failure'
  >({
    mode: 'query',
    availability: 'ready',
    errors: [
      'host_not_ready',
      'host_draining',
      'invalid_request',
      'operation_unavailable',
      'operation_conflict',
      'stale_cursor',
      'internal_failure',
    ],
    decodeInput(value) {
      const row = requireRecord(value, 'Client query');
      if (row.kind === 'snapshot') {
        requireShapedRecord(row, 'Client snapshot query', ['kind'], ['cursor']);
        return { kind: 'snapshot', ...(row.cursor == null ? {} : { cursor: cursor(row.cursor) }) };
      }
      requireExactRecord(row, 'Client bundle query', [
        'kind',
        'entryId',
        'activation',
        'clientDigest',
        'offset',
      ]);
      if (row.kind !== 'bundle') throw invalidProtocolFrame('Invalid Client query kind');
      return {
        kind: 'bundle',
        entryId: identifier(row.entryId),
        activation: activation(row.activation),
        clientDigest: digest(row.clientDigest),
        offset: bytes(row.offset),
      };
    },
    decodeOutput(value) {
      requireEncodedByteLimit(value, 'Client query output', 128 * 1024);
      const row = requireRecord(value, 'Client query output');
      if (row.kind === 'snapshot') {
        requireExactRecord(row, 'Client snapshot page', [
          'kind',
          'revision',
          'entries',
          'nextCursor',
        ]);
        const revision = digest(row.revision);
        if (!Array.isArray(row.entries) || row.entries.length > 32)
          throw invalidProtocolFrame('Invalid Client page');
        const entries = row.entries.map(descriptor);
        for (let index = 1; index < entries.length; index++) {
          if (entries[index - 1].entryId >= entries[index].entryId)
            throw invalidProtocolFrame('Invalid Client entry order');
        }
        const nextCursor = row.nextCursor === null ? null : cursor(row.nextCursor);
        if (
          nextCursor &&
          (nextCursor.revision !== revision || nextCursor.afterEntry !== entries.at(-1)?.entryId)
        ) {
          throw invalidProtocolFrame('Client cursor does not match page');
        }
        return { kind: 'snapshot', revision, entries, nextCursor };
      }
      requireExactRecord(row, 'Client bundle chunk', [
        'kind',
        'entryId',
        'activation',
        'clientDigest',
        'offset',
        'totalBytes',
        'content',
        'nextOffset',
      ]);
      if (row.kind !== 'bundle' || typeof row.content !== 'string')
        throw invalidProtocolFrame('Invalid Client bundle chunk');
      const offset = bytes(row.offset);
      const totalBytes = bytes(row.totalBytes);
      const length = Buffer.byteLength(row.content, 'utf8');
      const end = offset + length;
      const nextOffset = row.nextOffset === null ? null : bytes(row.nextOffset);
      if (
        length > 16 * 1024 ||
        end > totalBytes ||
        (length === 0 && offset !== totalBytes) ||
        nextOffset !== (end < totalBytes ? end : null)
      )
        throw invalidProtocolFrame('Invalid Client chunk range');
      return {
        kind: 'bundle',
        entryId: identifier(row.entryId),
        activation: activation(row.activation),
        clientDigest: digest(row.clientDigest),
        offset,
        totalBytes,
        content: row.content,
        nextOffset,
      };
    },
  }),
} as const;

function cursor(value: unknown): PluginClientCursor {
  const row = requireExactRecord(value, 'Client cursor', ['revision', 'afterEntry']);
  return { revision: digest(row.revision), afterEntry: identifier(row.afterEntry) };
}
function descriptor(value: unknown): PluginClientDescriptor {
  const row = requireExactRecord(value, 'Client descriptor', [
    'entryId',
    'extensionId',
    'activation',
    'contentDigest',
    'clientDigest',
    'sdkVersion',
    'totalBytes',
    'dependencies',
    'config',
  ]);
  const sdkVersion = requireCount(row.sdkVersion, 'Client SDK version');
  if (
    sdkVersion === 0 ||
    sdkVersion > 0xffff_ffff ||
    !Array.isArray(row.dependencies) ||
    row.dependencies.length > 128
  ) {
    throw invalidProtocolFrame('Invalid Client SDK version or dependencies');
  }
  requireEncodedByteLimit(row.config, 'Client config', 64 * 1024);
  return {
    entryId: identifier(row.entryId),
    extensionId: identifier(row.extensionId),
    activation: activation(row.activation),
    contentDigest: digest(row.contentDigest),
    clientDigest: digest(row.clientDigest),
    sdkVersion,
    totalBytes: bytes(row.totalBytes),
    dependencies: row.dependencies.map(identifier),
    config: row.config,
  };
}
function identifier(value: unknown): string {
  const text = requireString(value, 'Plugin identity', 128);
  if (!/^[a-z][a-z0-9]*(?:[._:-][a-z0-9]+)*$/.test(text))
    throw invalidProtocolFrame('Invalid Plugin identity');
  return text;
}
function activation(value: unknown): string {
  const text = requireString(value, 'Client activation', 36);
  if (!/^[0-9a-f-]{36}$/i.test(text)) throw invalidProtocolFrame('Invalid Client activation');
  return text;
}
function digest(value: unknown): string {
  const text = requireString(value, 'Client digest', 71);
  if (!/^sha256-[a-f0-9]{64}$/.test(text)) throw invalidProtocolFrame('Invalid Client digest');
  return text;
}
function bytes(value: unknown): number {
  const count = requireCount(value, 'Client byte offset/size');
  if (count > 8 * 1024 * 1024) throw invalidProtocolFrame('Client bytes exceed limit');
  return count;
}
