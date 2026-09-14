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
import { mcpProxyToolName } from '../../../../packages/runtime/src/mcp-tools.ts';
import { clientCapabilityProviderId } from '../../../../packages/runtime-host/src/server/client-capability-provider-id.ts';
import {
  decodeClientCapabilityReplaceInput,
  decodeClientCapabilityReplaceResult,
  decodeClientCapabilityResult,
  decodeClientCapabilityClientFrame,
  decodeClientCapabilityHostFrame,
  decodeClientCapabilityUnregisterInput,
  validateToolInputSchema,
} from '../../../../packages/runtime-host/src/protocol/client-capability.ts';

const decoders = {
  manifest: decodeClientCapabilityReplaceInput,
  result: decodeClientCapabilityReplaceResult,
  unregister: decodeClientCapabilityUnregisterInput,
  'call-result': decodeClientCapabilityResult,
  'client-frame': decodeClientCapabilityClientFrame,
  'host-frame': decodeClientCapabilityHostFrame,
  'proxy-name': ({ serverId, toolName }) => mcpProxyToolName(serverId, toolName),
  'provider-id': clientCapabilityProviderId,
  schema: (input) => {
    validateToolInputSchema(input);
    return input;
  },
};

function verifyCapabilityContract() {
  const cases = JSON.parse(readFileSync(0, 'utf8'));
  for (const { kind, input, expected } of cases) {
    let actual;
    try {
      assert(Object.hasOwn(decoders, kind));
      const value = decoders[kind](input);
      actual = { ok: true, value };
    } catch {
      actual = { ok: false };
    }
    assert.deepEqual(
      actual,
      expected,
      `Capability contract drift: ${kind} ${JSON.stringify(input)}`,
    );
  }
  console.log('original-client-capability-contract');
}

verifyCapabilityContract();
