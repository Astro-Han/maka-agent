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

await withSourceModule('packages/runtime-host/src/protocol/model-provider.ts', async (source) => {
  let input = '';
  process.stdin.setEncoding('utf8');
  for await (const chunk of process.stdin) input += chunk;
  const spec = source.MODEL_PROVIDER_OPERATION_SPECS['model.provider.catalog.query'];
  for (const test of JSON.parse(input)) {
    let actual = false;
    try {
      spec.assertOutputForInput(spec.decodeInput(test.input), spec.decodeOutput(test.output));
      actual = true;
    } catch {}
    assert.equal(actual, test.expected, JSON.stringify(test));
  }
});
