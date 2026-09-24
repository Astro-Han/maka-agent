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

import type { Json, ProviderDescriptor, ProviderIdentity } from '@maka-agent/plugin-sdk/host';
import { decodeProviderIdentity, RuntimePolicyDomainDecodeError } from '@maka/core/runtime-policy';
import {
  requireCount,
  requireEncodedByteLimit,
  requireExactRecord,
  requireRecord,
  requireShapedRecord,
  requireUtf8String,
} from './codec.js';
import { invalidProtocolFrame } from './errors.js';
import { defineOperation } from './operation-spec.js';

export interface ModelProviderCatalogQuery {
  scope?: 'profile' | 'desktop-ui' | `session:${string}`;
  after?: string | null;
  revision?: number | null;
}

export interface ModelProviderCatalogChangedFrame {
  readonly kind: 'model.provider.catalog.changed';
  readonly revision: number;
}

export function decodeModelProviderCatalogChangedFrame(
  value: unknown,
): ModelProviderCatalogChangedFrame {
  const row = requireExactRecord(value, 'provider catalog change', ['kind', 'revision']);
  if (row.kind !== 'model.provider.catalog.changed')
    throw invalidProtocolFrame('Invalid provider catalog change');
  return {
    kind: 'model.provider.catalog.changed',
    revision: requireCount(row.revision, 'provider catalog revision'),
  };
}
export type ModelProviderCatalogResult =
  | { kind: 'revision_changed'; revision: number }
  | {
      kind: 'page';
      revision: number;
      entries: { identity: ProviderIdentity; descriptor: ProviderDescriptor }[];
      next: string | null;
    };

function identifier(value: unknown, max = 256): string {
  const text = requireUtf8String(value, 'provider identifier', max);
  if (!text || /[\p{White_Space}\p{Cc}]/u.test(text))
    throw invalidProtocolFrame('Invalid provider identifier');
  return text;
}
// Rust orders provider names by UTF-8 bytes, not JavaScript's UTF-16 code units.
function compareNames(left: string, right: string): number {
  const a = new TextEncoder().encode(left);
  const b = new TextEncoder().encode(right);
  for (let index = 0; index < Math.min(a.length, b.length); index++) {
    if (a[index] !== b[index]) return a[index]! - b[index]!;
  }
  return a.length - b.length;
}
function scope(value: unknown): NonNullable<ModelProviderCatalogQuery['scope']> {
  if (value === 'profile' || value === 'desktop-ui') return value;
  if (typeof value === 'string' && value.startsWith('session:')) {
    return `session:${identifier(value.slice(8))}`;
  }
  throw invalidProtocolFrame('Invalid provider scope');
}
function flag(value: unknown): boolean {
  if (typeof value !== 'boolean') throw invalidProtocolFrame('Invalid provider flag');
  return value;
}
function label(value: unknown, controls = false): string {
  const text = requireUtf8String(value, 'provider label', 256);
  if (!/[^\p{White_Space}]/u.test(text) || (!controls && /[\p{Cc}]/u.test(text)))
    throw invalidProtocolFrame('Invalid provider label');
  return text;
}
function jsonObject(value: unknown): { [key: string]: Json } {
  return requireRecord(value, 'provider JSON object') as { [key: string]: Json };
}
function descriptor(value: unknown): ProviderDescriptor {
  requireEncodedByteLimit(value, 'provider descriptor', 64 * 1024);
  const row = requireExactRecord(value, 'provider descriptor', [
    'label',
    'configurationSchema',
    'configurationDefaults',
    'authentication',
    'anonymous',
    'discovery',
  ]);
  if (!Array.isArray(row.authentication) || row.authentication.length > 16) {
    throw invalidProtocolFrame('Invalid provider authentication methods');
  }
  const ids = new Set<string>();
  const authentication = row.authentication.map((value) => {
    const method = requireExactRecord(value, 'provider authentication method', [
      'id',
      'label',
      'inputSchema',
      'interactive',
    ]);
    const id = identifier(method.id);
    if (ids.has(id)) throw invalidProtocolFrame('Duplicate provider authentication method');
    ids.add(id);
    return {
      id,
      label: label(method.label, true),
      inputSchema: jsonObject(method.inputSchema),
      interactive: flag(method.interactive),
    };
  });
  return {
    label: label(row.label),
    configurationSchema: jsonObject(row.configurationSchema),
    configurationDefaults: jsonObject(row.configurationDefaults),
    authentication,
    anonymous: flag(row.anonymous),
    discovery: flag(row.discovery),
  };
}
function identity(value: unknown): ProviderIdentity {
  try {
    return decodeProviderIdentity(value);
  } catch (error) {
    if (error instanceof RuntimePolicyDomainDecodeError) throw invalidProtocolFrame(error.message);
    throw error;
  }
}

export const MODEL_PROVIDER_OPERATION_SPECS = {
  'model.provider.catalog.query': defineOperation<
    ModelProviderCatalogQuery,
    ModelProviderCatalogResult,
    | 'host_not_ready'
    | 'host_draining'
    | 'invalid_request'
    | 'operation_unavailable'
    | 'internal_failure'
    | 'persistence_failed'
  >({
    mode: 'query',
    availability: 'ready',
    errors: [
      'host_not_ready',
      'host_draining',
      'invalid_request',
      'operation_unavailable',
      'internal_failure',
      'persistence_failed',
    ],
    decodeInput(value) {
      const row = requireShapedRecord(
        value,
        'provider catalog query',
        [],
        ['scope', 'after', 'revision'],
      );
      if (row.after != null && row.revision == null)
        throw invalidProtocolFrame('Provider cursor requires a revision');
      return {
        ...(row.scope === undefined ? {} : { scope: scope(row.scope) }),
        ...(row.after === undefined
          ? {}
          : { after: row.after === null ? null : identifier(row.after) }),
        ...(row.revision === undefined
          ? {}
          : {
              revision:
                row.revision === null ? null : requireCount(row.revision, 'provider revision'),
            }),
      };
    },
    decodeOutput(value) {
      requireEncodedByteLimit(value, 'provider catalog page', 512 * 1024 + 4096);
      const row = requireRecord(value, 'provider catalog page');
      const revision = requireCount(row.revision, 'provider revision');
      if (row.kind === 'revision_changed') {
        requireExactRecord(row, 'provider revision change', ['kind', 'revision']);
        return { kind: 'revision_changed', revision };
      }
      requireExactRecord(row, 'provider page', ['kind', 'revision', 'entries', 'next']);
      if (row.kind !== 'page' || !Array.isArray(row.entries) || row.entries.length > 32)
        throw invalidProtocolFrame('Invalid provider page');
      let previous: string | undefined;
      const entries = row.entries.map((value) => {
        const entry = requireExactRecord(value, 'provider entry', ['identity', 'descriptor']);
        const selected = identity(entry.identity);
        if (previous !== undefined && compareNames(previous, selected.name) >= 0)
          throw invalidProtocolFrame('Provider page changed cursor order');
        previous = selected.name;
        return { identity: selected, descriptor: descriptor(entry.descriptor) };
      });
      const next = row.next === null ? null : identifier(row.next);
      if (next !== null && next !== entries.at(-1)?.identity.name)
        throw invalidProtocolFrame('Invalid provider cursor');
      return { kind: 'page', revision, entries, next };
    },
    assertOutputForInput(input, output) {
      const requestedScope = input.scope ?? 'profile';
      const valid =
        output.kind === 'revision_changed'
          ? input.revision != null && input.revision !== output.revision
          : (input.revision == null || input.revision === output.revision) &&
            output.entries.every(
              ({ identity }) =>
                (input.after == null || compareNames(identity.name, input.after) > 0) &&
                (identity.scope === requestedScope ||
                  (requestedScope.startsWith('session:') && identity.scope === 'profile')),
            );
      if (!valid) throw invalidProtocolFrame('Provider page does not match its query');
    },
  }),
} as const;
