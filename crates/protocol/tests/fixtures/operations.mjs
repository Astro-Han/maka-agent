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

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { HOST_OPERATION_SPECS } from '../../../../packages/runtime-host/src/protocol/operations.ts';
import { RUNTIME_HOST_COMPATIBILITY_EPOCH } from '../../../../packages/runtime-host/src/protocol/index.ts';

const actual = JSON.parse(readFileSync(0, 'utf8'));
const expected = Object.fromEntries(
  // The TS Host remains a reference, not a requirement to retain migrated business RPCs.
  Object.entries(HOST_OPERATION_SPECS)
    .filter(([operation]) => operation in actual.operations)
    .map(([operation, spec]) => [
      operation,
      {
        mode: spec.mode,
        availability: spec.availability,
        unavailableError: spec.errors.includes('operation_unavailable')
          ? 'operation_unavailable'
          : 'internal_failure',
      },
    ]),
);
assert.deepEqual(
  actual.operations,
  expected,
  'Rust Host operation contracts differ from current source',
);
assert.equal(actual.epoch, RUNTIME_HOST_COMPATIBILITY_EPOCH);
const providerCatalog = HOST_OPERATION_SPECS['model.provider.catalog.query'];
for (const page of actual.providerPages) {
  assert.deepEqual(providerCatalog.decodeOutput(page), page);
}
const page = actual.providerPages[0];
assert.throws(() => providerCatalog.decodeOutput({ ...page, next: 'unrelated' }));
assert.throws(() =>
  providerCatalog.decodeOutput({ ...page, entries: [page.entries[0], page.entries[0]] }),
);
for (const { operation, input, decoded } of actual.targets) {
  const decode = () => HOST_OPERATION_SPECS[operation].decodeInput(input);
  if (decoded === null) assert.throws(decode, JSON.stringify({ operation, input }));
  else assert.deepEqual(JSON.parse(JSON.stringify(decode())), decoded);
}
console.log(
  JSON.stringify({ check: 'operation-contract', operationCount: Object.keys(expected).length }),
);
