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

import { readFileSync } from 'node:fs';
import assert from 'node:assert/strict';
import { withSourceModule } from '../../../../tests/support/source.mjs';

const cases = JSON.parse(readFileSync(0, 'utf8'));
await withSourceModule('packages/runtime/src/read-page.ts', ({ readPage, readPageSchema }) => {
  const results = cases.map(({ text, input, budget, actual }) => {
    readPageSchema.parse(actual);
    assert.ok(JSON.stringify(actual).length <= budget);
    const complete = readPage(text, input, Number.MAX_SAFE_INTEGER);
    assert.equal(actual.offset, complete.offset);
    assert.equal(actual.totalLines, complete.totalLines);
    if (actual.next) {
      // TS must accept Rust cursors and reconstruct the requested line range.
      const continued = readPage(text, actual.next, Number.MAX_SAFE_INTEGER);
      const partial = actual.content + continued.content === complete.content;
      assert.equal(actual.content + (partial ? '' : '\n') + continued.content, complete.content);
      assert.equal(actual.returnedLines + continued.returnedLines, complete.returnedLines);
      assert.equal(actual.partialLine === true, partial || complete.partialLine === true);
    } else assert.deepEqual(actual, complete);
    const original = readPage(text, input, budget);
    return {
      next: original.next,
      remainder: original.next ? readPage(text, original.next, Number.MAX_SAFE_INTEGER) : null,
    };
  });
  process.stdout.write(JSON.stringify(results));
});
