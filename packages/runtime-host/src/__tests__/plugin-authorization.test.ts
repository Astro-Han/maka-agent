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
import test from 'node:test';
import { PLUGIN_AUTHORIZATION_OPERATION_SPECS } from '../protocol/plugin-authorization.js';

test('authorization wire preserves HTTP and sandboxed processes without granting unrestricted clients', () => {
  const decode = PLUGIN_AUTHORIZATION_OPERATION_SPECS['plugin.authorization'].decodeInput;
  const id = '470bdd64-752b-49ca-aee3-b40b8aa3e656';
  for (const target of [
    { kind: 'plugin_workspace' },
    { kind: 'workspace', workspace: { kind: 'host_path', path: '/workspace' } },
  ]) {
    for (const [sandboxMode, capabilities, allowed] of [
      ['read-only', ['network', 'processes', 'read_files'], true],
      ['workspace-write', ['network', 'processes', 'write_files'], true],
      ['danger-full-access', ['client_capabilities', 'network'], true],
      ['read-only', ['network', 'write_files'], false],
      ['workspace-write', ['client_capabilities'], false],
    ] as const) {
      const request = {
        client: {
          entryId: 'external-ui',
          extensionId: 'external',
          activation: id,
          contentDigest: `sha256-${'a'.repeat(64)}`,
          clientDigest: `sha256-${'b'.repeat(64)}`,
        },
        scope: 'profile',
        command: {
          kind: 'approve',
          request: {
            operationId: id,
            title: 'Explicit capability consent',
            target: { ...target, sandboxMode },
            capabilities,
          },
        },
      };
      if (allowed) assert.deepEqual(decode(request), request);
      else assert.throws(() => decode(request));
    }
  }
});
