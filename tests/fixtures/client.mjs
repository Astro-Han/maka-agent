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

import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import { withSourceBundle } from '../support/source.mjs';

const entries = [
  ['--skills-client-workspace', 'client-skills-entry.mjs'],
  ['--scheduler-workspace', 'client-scheduler-entry.mjs'],
  ['--plugin-remote', 'client-plugin-remote-entry.mjs'],
  ['--workhub-workspace', 'client-workhub-entry.mjs'],
  ['--workhub-answer-workspace', 'client-workhub-entry.mjs'],
  ['--workhub-delegation-workspace', 'client-workhub-entry.mjs'],
  ['--workhub-creation-workspace', 'client-workhub-entry.mjs'],
  ['--workhub-selection-workspace', 'client-workhub-entry.mjs'],
  ['--workhub-stop-workspace', 'client-workhub-entry.mjs'],
  ['--workhub-steering-workspace', 'client-workhub-entry.mjs'],
  ['--workhub-resume-workspace', 'client-workhub-entry.mjs'],
  ['--workhub-correction-workspace', 'client-workhub-entry.mjs'],
  ['--workhub-correction-creation-workspace', 'client-workhub-entry.mjs'],
  ['--onboarding-workspace', 'client-onboarding-entry.mjs'],
  ['--project-workspace', 'client-project-entry.mjs'],
  ['--native-candidate', 'client-candidate-entry.mjs'],
  ['--native-access', 'client-access-entry.mjs'],
  ['--native-managed', 'client-native-managed-entry.mjs'],
  ['--activation-frame', 'client-activation-entry.mjs'],
  ['--bridge', 'client-bridge-entry.mjs'],
  ['--system-prompt-workspace', 'client-system-prompt-entry.mjs'],
  ['--model-overrides-workspace', 'client-model-overrides-entry.mjs'],
  ['--runtime-policy-workspace', 'client-runtime-policy-entry.mjs'],
  ['--artifact-workspace', 'client-artifact-entry.mjs'],
];
const entry = entries.find(([flag]) => process.argv.includes(flag))?.[1] ?? 'client-entry.mjs';
await withSourceBundle(fileURLToPath(new URL(entry, import.meta.url)), (bundle, inputs) => {
  for (const source of ['client/connection.ts', 'protocol/index.ts', 'protocol/codec.ts']) {
    if (!inputs.some((input) => input.endsWith(`packages/runtime-host/src/${source}`))) {
      throw new Error(`Bundle omitted original source ${source}`);
    }
  }
  const child = spawnSync(process.execPath, [bundle, ...process.argv.slice(2)], {
    stdio: 'inherit',
    // The parent owns cleanup even if a failed fixture retains handles.
    timeout: process.argv.includes('--ssh')
      ? 180000
      : process.argv.includes('--live-provider-workspace')
        ? 330000
        : process.argv.includes('--oauth-execution-workspace')
          ? 150000
          : process.argv.includes('--message-queue-workspace')
            ? 55000
            : process.argv.includes('--large-output-workspace')
              ? 120000
              : process.argv.includes('--bash-workspace') ||
                  process.argv.includes('--onboarding-workspace') ||
                  (process.argv.includes('--native-managed') &&
                    process.argv.includes('--management'))
                ? 45000
                : 15000,
  });
  if (child.error) throw child.error;
  if (child.signal) throw new Error(`Original-client subprocess terminated by ${child.signal}`);
  process.exitCode = child.status ?? 1;
});
