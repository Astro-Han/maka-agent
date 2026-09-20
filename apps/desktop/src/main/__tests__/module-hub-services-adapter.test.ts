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

import { strict as assert } from 'node:assert';
import { describe, it } from 'node:test';
import type { ModuleHubRuntimeHostRef } from '../../renderer/features/module-hub/testing.js';
import {
  createDesktopModuleHubServices,
  type DesktopModuleHubBridge,
} from '../../renderer/platform/desktop/create-module-hub-services.js';

type Call = { name: string; args: unknown[] };

function methodRecorder(calls: Call[], prefix: string) {
  return new Proxy(
    {} as Record<PropertyKey, unknown>,
    {
      get: (target, property) =>
        Reflect.has(target, property)
          ? Reflect.get(target, property)
          : (...args: unknown[]) => {
              calls.push({ name: `${prefix}.${String(property)}`, args });
              return Promise.resolve(undefined);
            },
    },
  );
}

describe('createDesktopModuleHubServices', () => {
  it('maps host-scoped Daily Review, and clipboard operations', async () => {
    const calls: Call[] = [];
    const host: ModuleHubRuntimeHostRef = {
      profileId: 'remote-a',
      hostId: 'host-a',
    };
    const bridge = {
      runtimeHostProfiles: {
        getDefaultHost: async () => host,
        subscribeChanges: () => () => undefined,
      },
      dailyReview: methodRecorder(calls, 'dailyReview'),
    } as unknown as DesktopModuleHubBridge;
    const clipboard = {
      async writeText(text: string) {
        calls.push({ name: 'clipboard.writeText', args: [text] });
      },
    };
    const services = createDesktopModuleHubServices(bridge, { clipboard });

    assert.deepEqual(await services.runtimeHosts.getDefault(), host);

    await services.dailyReview.day(0, 7, host);
    await services.dailyReview.runOnce({ range: 7, offsetDays: -1 });
    await services.dailyReview.listArchives();
    await services.dailyReview.getArchive('archive');
    await services.dailyReview.saveMarkdownToFile({
      markdown: '# Review',
      defaultName: 'review.md',
    });
    await services.clipboard.writeText('review');

    assert.deepEqual(calls, [
      { name: 'dailyReview.day', args: [0, 7, host] },
      { name: 'dailyReview.runOnce', args: [{ range: 7, offsetDays: -1 }] },
      { name: 'dailyReview.listArchives', args: [] },
      { name: 'dailyReview.getArchive', args: ['archive'] },
      {
        name: 'dailyReview.saveMarkdownToFile',
        args: [{ markdown: '# Review', defaultName: 'review.md' }],
      },
      { name: 'clipboard.writeText', args: ['review'] },
    ]);
  });

  it('persists keep-awake policy and releases its change subscription', async () => {
    let changed: (() => void) | undefined;
    let disposed = 0;
    const updates: unknown[] = [];
    const base = {
      runtimeHostProfiles: {
        getDefaultHost: async () => ({ profileId: 'local', hostId: 'local' }),
        subscribeChanges: () => () => undefined,
      },
      dailyReview: methodRecorder([], 'dailyReview'),
    };
    const services = createDesktopModuleHubServices(
      {
        ...base,
        settings: {
          getClient: async () => ({
            system: { keepSystemAwake: true },
          }),
          updateClient: async (patch: unknown) => {
            updates.push(patch);
            return { settings: { system: { keepSystemAwake: false } } };
          },
          subscribeClientChanged(handler: () => void) {
            changed = handler;
            return () => {
              disposed += 1;
            };
          },
        },
      } as unknown as DesktopModuleHubBridge,
      { clipboard: { writeText: async () => undefined } },
    );
    assert.equal(services.clientSettings.supported, true);
    assert.equal(await services.clientSettings.getKeepSystemAwake(), true);
    assert.equal(await services.clientSettings.setKeepSystemAwake(false), false);
    let notifications = 0;
    const unsubscribe = services.clientSettings.subscribeChanges(() => {
      notifications += 1;
    });
    changed?.();
    unsubscribe();
    assert.deepEqual(updates, [{ system: { keepSystemAwake: false } }]);
    assert.equal(notifications, 1);
    assert.equal(disposed, 1);


  });
});
