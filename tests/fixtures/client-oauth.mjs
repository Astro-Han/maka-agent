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

export async function verifyOAuthReceipts(connection, receipts) {
  const request = (operation, input) => connection.request(operation, input, 3000);
  assert.deepEqual(await request('oauth.enrollment.query', { provider: 'xai-oauth' }), {
    provider: 'xai-oauth',
    enabled: true,
  });
  for (const [input, identity] of receipts) {
    const expected = { attemptId: input.attemptId, connection: identity, phase: 'authenticated' };
    assert.deepEqual(await request('oauth.login.start', input), expected);
    for (const operation of ['oauth.login.query', 'oauth.login.cancel']) {
      assert.deepEqual(await request(operation, { attemptId: input.attemptId }), expected);
    }
    await assert.rejects(
      request('oauth.login.start', {
        attemptId: input.attemptId,
        target: { kind: 'existing', connectionId: identity.connectionId },
      }),
      (error) => error.code === 'invalid_request',
    );
  }
  await assert.rejects(
    request('oauth.login.query', { attemptId: 'missing' }),
    (error) => error.code === 'not_found',
  );
  console.log(JSON.stringify({ check: 'oauth-receipts', providers: receipts.length }));
}
