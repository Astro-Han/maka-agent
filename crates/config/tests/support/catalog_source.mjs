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

let input = '';
for await (const chunk of process.stdin) input += chunk;
const { snapshot, pages } = JSON.parse(input);
const decoded = await withSourceModule(
  'packages/runtime-host/src/protocol/runtime-policy.ts',
  async (source) =>
    pages.map((page) => {
      const decoded =
        source.RUNTIME_POLICY_OPERATION_SPECS['connection.catalog.query'].decodeOutput(page);
      assert.deepEqual(decoded, page);
      return decoded;
    }),
);
await withSourceModule('packages/runtime-host/src/client/catalog-reader.ts', async (source) => {
  let index = 0;
  const actual = await source.readRuntimeHostConnectionCatalog({
    async request(operation, input) {
      assert.equal(operation, 'connection.catalog.query');
      assert.deepEqual(
        input,
        index === 0
          ? { kind: 'start' }
          : {
              kind: 'continue',
              revision: snapshot.revision,
              cursor: decoded[index - 1].nextCursor,
            },
      );
      assert(index < decoded.length);
      return decoded[index++];
    },
  });
  assert.equal(index, pages.length);
  const expected = {
    ...snapshot,
    connections: snapshot.connections.map(({ modelsFetchedAt: _at, ...row }, connectionIndex) => ({
      ...row,
      catalogEntries: pages
        .flatMap((page) => page.items)
        .filter((item) => item.kind === 'catalog_entry' && item.connectionIndex === connectionIndex)
        .map((item) => item.entry),
    })),
  };
  assert.deepEqual(actual, expected);
});
