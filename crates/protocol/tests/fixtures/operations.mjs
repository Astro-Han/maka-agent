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

const actual = JSON.parse(readFileSync(0, 'utf8'));
const expected = Object.fromEntries(
  Object.entries(HOST_OPERATION_SPECS).map(([operation, spec]) => [
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
assert.deepEqual(actual, expected, 'Rust operation vocabulary differs from current source');
console.log(
  JSON.stringify({ check: 'operation-contract', operationCount: Object.keys(expected).length }),
);
