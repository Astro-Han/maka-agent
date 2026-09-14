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
import { withSourceModule } from '../../../../tests/support/source.mjs';

const cases = JSON.parse(readFileSync(0, 'utf8'));
await withSourceModule(
  'packages/runtime/src/tool-result-archive-resource.ts',
  async ({ readToolResultArchiveResource }) => {
    for (const { body, input, actual } of cases) {
      const expected = await readToolResultArchiveResource(
        {
          readArchivedToolResultResource(request) {
            assert.equal(request.sessionId, 'session');
            assert.equal(request.storage, 'event');
            assert.equal(request.runtimeEventId, 'event/中文');
            return { ok: true, serializedResult: body };
          },
        },
        'session',
        input,
      );
      assert.deepEqual(actual, expected);
    }
  },
);
