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
  requireEncodedByteLimit,
  requireExactRecord,
  requireId,
  requireRecord,
  requireString,
} from './codec.js';
import { invalidProtocolFrame } from './errors.js';
import { defineOperation } from './operation-spec.js';

export interface PluginRemoteClient {
  entryId: string;
  extensionId: string;
  activation: string;
  contentDigest: string;
  clientDigest: string;
}
export interface PluginRemoteBinding {
  client: PluginRemoteClient;
  method: string;
  sessionId: string | null;
}
export interface PluginRemoteTarget {
  entryId: string;
  activation: string;
  registration: string;
}
export type PluginRemoteInput =
  | { kind: 'open_document' }
  | { kind: 'bind'; binding: PluginRemoteBinding }
  | {
      kind: 'call' | 'open';
      binding: PluginRemoteBinding;
      target: PluginRemoteTarget;
      document: string;
      input: unknown;
    }
  | { kind: 'next' | 'close'; document: string; stream: string }
  | { kind: 'close_document'; document: string };
export type PluginRemoteResult =
  | { kind: 'document'; document: string }
  | { kind: 'bound'; target: PluginRemoteTarget; handler: 'method' | 'stream' }
  | { kind: 'value'; value: unknown }
  | { kind: 'opened'; stream: string }
  | { kind: 'item'; item: unknown }
  | { kind: 'end' }
  | { kind: 'pending' }
  | { kind: 'closed' };

export const PLUGIN_REMOTE_OPERATION_SPECS = {
  'plugin.remote': defineOperation<
    PluginRemoteInput,
    PluginRemoteResult,
    | 'host_not_ready'
    | 'host_draining'
    | 'invalid_request'
    | 'operation_unavailable'
    | 'operation_conflict'
    | 'outcome_unknown'
    | 'internal_failure'
  >({
    mode: 'command',
    availability: 'ready',
    errors: [
      'host_not_ready',
      'host_draining',
      'invalid_request',
      'operation_unavailable',
      'operation_conflict',
      'outcome_unknown',
      'internal_failure',
    ],
    decodeInput(value) {
      requireEncodedByteLimit(value, 'Remote request', 96 * 1024);
      const row = requireRecord(value, 'Remote request');
      switch (row.kind) {
        case 'open_document':
          requireExactRecord(row, 'Remote document open', ['kind']);
          return { kind: row.kind };
        case 'bind':
          requireExactRecord(row, 'Remote bind', ['kind', 'binding']);
          return { kind: row.kind, binding: binding(row.binding) };
        case 'call':
        case 'open':
          requireExactRecord(row, 'Remote invocation', [
            'kind',
            'binding',
            'target',
            'document',
            'input',
          ]);
          requireEncodedByteLimit(row.input, 'Remote payload', 64 * 1024);
          return {
            kind: row.kind,
            binding: binding(row.binding),
            target: target(row.target),
            document: uuid(row.document),
            input: row.input,
          };
        case 'next':
        case 'close':
          requireExactRecord(row, 'Remote stream', ['kind', 'document', 'stream']);
          return { kind: row.kind, document: uuid(row.document), stream: uuid(row.stream) };
        case 'close_document':
          requireExactRecord(row, 'Remote document close', ['kind', 'document']);
          return { kind: row.kind, document: uuid(row.document) };
        default:
          throw invalidProtocolFrame('Invalid Remote operation');
      }
    },
    decodeOutput(value) {
      requireEncodedByteLimit(value, 'Remote result', 96 * 1024);
      const row = requireRecord(value, 'Remote result');
      switch (row.kind) {
        case 'document':
          requireExactRecord(row, 'Remote document', ['kind', 'document']);
          return { kind: row.kind, document: uuid(row.document) };
        case 'bound':
          requireExactRecord(row, 'Remote bound', ['kind', 'target', 'handler']);
          if (row.handler !== 'method' && row.handler !== 'stream')
            throw invalidProtocolFrame('Invalid Remote handler kind');
          return { kind: row.kind, target: target(row.target), handler: row.handler };
        case 'value':
          requireExactRecord(row, 'Remote value', ['kind', 'value']);
          requireEncodedByteLimit(row.value, 'Remote value', 64 * 1024);
          return { kind: row.kind, value: row.value };
        case 'opened':
          requireExactRecord(row, 'Remote stream handle', ['kind', 'stream']);
          return { kind: row.kind, stream: uuid(row.stream) };
        case 'item':
          requireExactRecord(row, 'Remote stream item', ['kind', 'item']);
          requireEncodedByteLimit(row.item, 'Remote item', 64 * 1024);
          return { kind: row.kind, item: row.item };
        case 'end':
        case 'pending':
        case 'closed':
          requireExactRecord(row, 'Remote closed', ['kind']);
          return { kind: row.kind };
        default:
          throw invalidProtocolFrame('Invalid Remote result');
      }
    },
  }),
} as const;
function binding(value: unknown): PluginRemoteBinding {
  const row = requireExactRecord(value, 'Remote binding', ['client', 'method', 'sessionId']);
  return {
    client: decodePluginClientIdentity(row.client),
    method: identity(row.method),
    sessionId: row.sessionId === null ? null : requireId(row.sessionId, 'Remote Session'),
  };
}
export function decodePluginClientIdentity(value: unknown): PluginRemoteClient {
  const client = requireExactRecord(value, 'Remote Client', [
    'entryId',
    'extensionId',
    'activation',
    'contentDigest',
    'clientDigest',
  ]);
  return {
    entryId: identity(client.entryId),
    extensionId: identity(client.extensionId),
    activation: uuid(client.activation),
    contentDigest: digest(client.contentDigest),
    clientDigest: digest(client.clientDigest),
  };
}
function target(value: unknown): PluginRemoteTarget {
  const row = requireExactRecord(value, 'Remote target', ['entryId', 'activation', 'registration']);
  return {
    entryId: identity(row.entryId),
    activation: uuid(row.activation),
    registration: uuid(row.registration),
  };
}
function identity(value: unknown): string {
  const text = requireString(value, 'Remote identity', 128);
  if (!/^[a-z][a-z0-9]*(?:[._:-][a-z0-9]+)*$/.test(text))
    throw invalidProtocolFrame('Invalid Remote identity');
  return text;
}
function uuid(value: unknown): string {
  const text = requireString(value, 'Remote instance identity', 36);
  if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(text))
    throw invalidProtocolFrame('Invalid Remote instance identity');
  return text;
}
function digest(value: unknown): string {
  const text = requireString(value, 'Remote digest', 71);
  if (!/^sha256-[a-f0-9]{64}$/.test(text)) throw invalidProtocolFrame('Invalid Remote digest');
  return text;
}
