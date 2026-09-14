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
const { provider, responses, previousTokens } = JSON.parse(input);
const prefix = { 'openai-codex': 'Codex', 'xai-oauth': 'Xai', 'github-copilot': 'GitHubCopilot' }[
  provider
];
const file = { 'openai-codex': 'codex', 'xai-oauth': 'xai', 'github-copilot': 'github-copilot' }[
  provider
];
await withSourceModule(
  previousTokens
    ? 'packages/runtime/src/subscription-credentials.ts'
    : `packages/runtime/src/${file}-oauth-enrollment.ts`,
  async (source) => {
    const requests = [];
    const options = {
      signal: new AbortController().signal,
      fetchFn: async (url, init) => {
        const headers = new Headers(init.headers);
        const body =
          headers.get('content-type') === 'application/json'
            ? JSON.parse(init.body)
            : Object.fromEntries(new URLSearchParams(init.body));
        const authHeaders = Object.fromEntries(
          [...headers].filter(([name]) =>
            [
              'authorization',
              'user-agent',
              'editor-version',
              'editor-plugin-version',
              'copilot-integration-id',
              'openai-intent',
              'x-github-api-version',
            ].includes(name),
          ),
        );
        requests.push({ url, body, contentType: headers.get('content-type'), authHeaders });
        const response = responses.shift();
        return new Response(JSON.stringify(response.payload), { status: response.status });
      },
      sleep: async () => {},
    };
    if (previousTokens) {
      const tokens = await source.refreshOAuthSubscriptionTokens({
        ...options,
        providerType: provider,
        tokens: previousTokens,
      });
      delete tokens.expires_at;
      process.stdout.write(JSON.stringify({ requests, tokens }));
      return;
    }
    const authorization = await source[`start${prefix}DeviceAuthorization`](options);
    const granted = await source[`poll${prefix}DeviceAuthorization`]({ ...options, authorization });
    const tokens =
      provider === 'openai-codex'
        ? await source.exchangeCodexDeviceAuthorizationCode({ ...options, grant: granted })
        : granted;
    if (provider === 'github-copilot') {
      try {
        await source.verifyGitHubCopilotModelEntitlement({ tokens, fetchFn: options.fetchFn });
      } catch (error) {
        process.stdout.write(JSON.stringify({ requests, error: error.constructor.name }));
        return;
      }
    }
    delete tokens.expires_at;
    process.stdout.write(JSON.stringify({ requests, tokens }));
  },
);
