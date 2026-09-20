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
import { test } from 'node:test';
import { createModuleHubCommandPort, type ModuleHubCommands } from '../../renderer/features/module-hub/testing.js';

test('command port keeps the newest controller through stale cleanup', async () => {
  const calls: string[] = [];
  const commands = (name: string): ModuleHubCommands => ({
    openAction: () => calls.push(`${name}:create`),
    copyTodayDailyReview: async () => {
      calls.push(`${name}:copy`);
    },
    pasteTodayDailyReview: async () => {
      calls.push(`${name}:paste`);
    },
    saveTodayDailyReview: async () => {
      calls.push(`${name}:save`);
    },
  });
  const port = createModuleHubCommandPort();
  const disconnectFirst = port.connect(commands('first'));
  const disconnectSecond = port.connect(commands('second'));

  disconnectFirst();
  port.openAction({ section: 'automations', module: 'scheduled-tasks' }, 'create');
  assert.deepEqual(calls, ['second:create']);

  disconnectSecond();
  await port.copyTodayDailyReview();
  assert.deepEqual(calls, ['second:create']);
});
