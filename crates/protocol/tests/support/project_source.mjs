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
import { withSourceModule } from '../../../../tests/support/source.mjs';

await withSourceModule('packages/runtime-host/src/protocol/project-catalog.ts', async (source) => {
  let input = '';
  for await (const chunk of process.stdin) input += chunk;
  for (const [index, test] of JSON.parse(input).entries()) {
    const spec =
      source.PROJECT_CATALOG_OPERATION_SPECS[
        test.kind.startsWith('query') ? 'project.catalog.query' : 'project.catalog.mutate'
      ];
    let actual;
    try {
      const isInput = test.kind.endsWith('_in');
      const value = isInput ? spec.decodeInput(test.value) : spec.decodeOutput(test.value);
      if (test.input) spec.assertOutputForInput(spec.decodeInput(test.input), value);
      actual = {
        ok: true,
        decoded: { value, ...(isInput ? { hostPaths: spec.usesHostPaths(value) } : {}) },
      };
    } catch {
      actual = { ok: false };
    }
    assert.deepEqual(actual, test.expected, `case ${index}: ${JSON.stringify(test.value)}`);
  }
});
