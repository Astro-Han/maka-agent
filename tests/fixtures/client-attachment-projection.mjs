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
import { withSourceModule } from '../support/source.mjs';

let input = '';
for await (const chunk of process.stdin) input += chunk;
const cases = JSON.parse(input);
await withSourceModule('packages/runtime/src/model-history.ts', async (source) => {
  for (const test of cases) {
    assert.equal(source.formatTextWithInlineRefs(test.content), test.text);
    assert.equal(
      source.buildSteeringEnvelope(source.formatTextWithInlineRefs(test.content)),
      test.steering,
    );
  }
});
const wire = ({ display_text, directory_references, inline_references, ...rest }) => ({
  ...rest,
  ...(display_text !== null ? { displayText: display_text } : {}),
  ...(directory_references !== undefined ? { directoryReferences: directory_references } : {}),
  ...(inline_references !== undefined ? { inlineReferences: inline_references } : {}),
});
await withSourceModule('packages/core/src/events.ts', async (source) => {
  for (const test of cases) {
    assert.deepEqual(
      source.aggregateMessageContents(test.aggregation.sources.map(wire)),
      wire(test.aggregation.expected),
    );
  }
});
console.log('original-attachment-reference-projection');
