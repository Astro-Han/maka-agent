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

import type { ProviderIdentity } from '@maka-agent/plugin-sdk/host';
import type { JsonObject } from '../request-customization.js';
import { serializedByteLength } from '../serialized-byte-length.js';
import { domainError, exactRecord } from './domain-codec.js';

export type { ProviderIdentity };

function name(value: unknown, max: number): string {
  if (
    typeof value !== 'string' ||
    value.length === 0 ||
    /[\p{White_Space}\p{Cc}]/u.test(value) ||
    new TextEncoder().encode(value).length > max
  ) {
    throw domainError('Invalid provider identity');
  }
  return value;
}

export function decodeProviderIdentity(value: unknown): ProviderIdentity {
  const row = exactRecord(value, 'provider identity', ['packageId', 'entryId', 'scope', 'name']);
  const owner = (value: unknown) => {
    const id = name(value, 128);
    if (!/^[a-z][a-z0-9]*(?:[._:-][a-z0-9]+)*$/.test(id))
      throw domainError('Invalid provider owner');
    return id;
  };
  let scope: ProviderIdentity['scope'];
  if (row.scope === 'profile') scope = row.scope;
  else if (typeof row.scope === 'string' && row.scope.startsWith('session:')) {
    scope = `session:${name(row.scope.slice(8), 256)}`;
  } else throw domainError('Invalid provider scope');
  return {
    packageId: owner(row.packageId),
    entryId: owner(row.entryId),
    scope,
    name: name(row.name, 256),
  };
}

export function decodeProviderConfiguration(value: unknown): JsonObject {
  if (
    value === null ||
    typeof value !== 'object' ||
    Array.isArray(value) ||
    serializedByteLength(value, 64 * 1024) > 64 * 1024
  ) {
    throw domainError('Provider configuration must be a JSON object within 64 KiB');
  }
  return value as JsonObject;
}
