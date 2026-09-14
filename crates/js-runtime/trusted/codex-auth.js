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

// SDK-generated routing headers are not user request customization. They must
// be present before both native WS upgrade and the shared HTTP fallback.
export function codexHeaders({ accessToken, sessionId }) {
  const account = accountId(accessToken);
  return {
    ...(account ? { 'ChatGPT-Account-Id': account } : {}),
    'OpenAI-Beta': 'responses=experimental',
    originator: 'codex_cli_rs',
    'User-Agent': 'codex_cli_rs/0.0.0 (Maka)',
    session_id: sessionId,
    'x-client-request-id': sessionId,
  };
}

function accountId(token) {
  try {
    const parts = token.split('.');
    if (parts.length !== 3 || !parts[1]) return null;
    const payload = parts[1].replaceAll('-', '+').replaceAll('_', '/');
    const bytes = Uint8Array.from(atob(payload), (char) => char.charCodeAt(0));
    const claims = JSON.parse(new TextDecoder().decode(bytes));
    const text = (value) => typeof value === 'string' && value.length > 0;
    if (text(claims?.chatgpt_account_id)) return claims.chatgpt_account_id;
    const nested = claims?.['https://api.openai.com/auth']?.chatgpt_account_id;
    if (text(nested)) return nested;
    for (const organization of Array.isArray(claims?.organizations) ? claims.organizations : []) {
      if (text(organization?.id) && organization.id.trim()) return organization.id.trim();
    }
  } catch {}
  // A JWT subject is NOT a ChatGPT account ID. Invalid/opaque tokens simply
  // omit this optional routing hint; the server remains the auth authority.
  return null;
}
