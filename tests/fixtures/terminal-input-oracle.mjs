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
await withSourceModule('packages/core/src/terminal-input.ts', async (core) => {
  await withSourceModule('packages/runtime/src/shell-run-contract.ts', async (contract) => {
    const results = JSON.parse(input).map(({ actions, state }) => {
      try {
        contract.validateWriteStdinInput({ actions });
        const parsed = actions.map(core.parseTerminalInputAction);
        return {
          encoded: core.encodeTerminalInputActions(parsed, state),
          bytes: core.encodedTerminalInputActionsByteLength(parsed),
        };
      } catch {
        return null;
      }
    });
    process.stdout.write(JSON.stringify(results));
  });
});
