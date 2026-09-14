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

import { withSourceModule } from '../support/source.mjs';
let input = '';
for await (const chunk of process.stdin) input += chunk;
const cases = JSON.parse(input);
await withSourceModule('packages/runtime/src/model-fetcher.ts', async (source) => {
  const output = [];
  for (const test of cases) {
    const requests = [];
    const result = await source.runConnectionModelDiscoveryEffect(
      { providerType: test.provider, baseUrl: 'http://fixture.invalid/v1/' },
      test.token,
      {
        fetch: async (url, init) => {
          requests.push({
            url: String(url),
            headers: Object.fromEntries(new Headers(init.headers)),
          });
          return new Response(JSON.stringify(test.payload), { status: test.status ?? 200 });
        },
      },
    );
    output.push({ result, requests });
  }
  process.stdout.write(JSON.stringify(output));
});
