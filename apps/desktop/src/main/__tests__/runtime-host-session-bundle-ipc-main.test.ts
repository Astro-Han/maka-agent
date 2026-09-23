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
import type { IpcMain } from 'electron';
import { RuntimeHostOperationError } from '@maka/runtime-host/client';
import {
  __bundleFileNameForTests as bundleFileName,
  registerRuntimeHostSessionBundleIpc,
  type RuntimeHostSessionBundleIpcDeps,
} from '../runtime-host-session-bundle-ipc-main.js';

type IpcHandler = Parameters<IpcMain['handle']>[1];

test('writes to the picked destination and reports what travelled', async () => {
  const asked: unknown[] = [];
  const ipc = ipcHarness();
  registerRuntimeHostSessionBundleIpc(
    deps({
      client: {
        exportSessionBundle: async (input) => {
          asked.push(input);
          return { sessionCount: 2, compressedBytes: 2048 };
        },
      },
      dialog: { save: { canceled: false, filePath: '/picked/Hello.maka-session' } },
    }),
    ipc,
  );

  const result = await ipc.invoke('session-bundle:export', 'session-1', 'Hello');

  assert.deepEqual(asked, [{ sessionId: 'session-1', destination: '/picked/Hello.maka-session', expectedSubtreeDigest: undefined }]);
  // The count is the subtree, not one: a bundle rooted at a Session carries the
  // subagent conversations under it, and the page says so.
  assert.deepEqual(result, {
    ok: true,
    sessionCount: 2,
    path: '/picked/Hello.maka-session',
  });
});

test('closing the save dialog asks the Host for nothing', async () => {
  let called = false;
  const ipc = ipcHarness();
  registerRuntimeHostSessionBundleIpc(
    deps({
      client: {
        exportSessionBundle: async () => {
          called = true;
          return { sessionCount: 0, compressedBytes: 0 };
        },
      },
      dialog: { save: { canceled: true } },
    }),
    ipc,
  );

  const result = await ipc.invoke('session-bundle:export', 'session-1', 'Hello');

  assert.deepEqual(result, { ok: false, reason: 'canceled' });
  assert.equal(called, false, 'a cancelled dialog is a decision, not a request');
});

test('binds imported sessions to the selected workspace and invalidates the catalog once', async () => {
  const events: Array<{ reason: string; sessionId?: string }> = [];
  const calls: unknown[] = [];
  const ipc = ipcHarness();
  registerRuntimeHostSessionBundleIpc(
    deps({
      client: {
        importSessionBundle: async (input) => {
          calls.push(input);
          return { sessionCount: 2, artifactFiles: 3 };
        },
      },
      dialog: { open: { canceled: false, filePaths: ['/picked/bundle.maka-session'] } },
      onSessionsChanged: (reason, sessionId) => events.push({ reason, ...(sessionId ? { sessionId } : {}) }),
    }),
    ipc,
  );

  const result = await ipc.invoke('session-bundle:import');

  assert.deepEqual(result, { ok: true, sessionCount: 2 });
  assert.deepEqual(calls, [{
    source: '/picked/bundle.maka-session',
    workspace: { kind: 'host_path', path: '/selected/workspace' },
  }]);
  // The Host republishes its own catalog; the desktop keeps a separate list and
  // only re-reads when told, so an imported task is invisible without this. No
  // id: a list that grows with the subtree can outgrow a frame after the
  // Sessions are committed, so the result is a count and this says "re-read".
  assert.deepEqual(events, [{ reason: 'created' }]);
});

test('keeps the reason a reader can act on', async () => {
  const ipc = ipcHarness();
  registerRuntimeHostSessionBundleIpc(
    deps({
      client: {
        exportSessionBundle: async () => {
          throw new RuntimeHostOperationError(
            'session-bundle.export',
            'session_busy',
            'Session is running',
          );
        },
      },
      dialog: { save: { canceled: false, filePath: '/picked/Hello.maka-session' } },
    }),
    ipc,
  );

  assert.deepEqual(await ipc.invoke('session-bundle:export', 'session-1', 'Hello'), {
    ok: false,
    reason: 'session_busy',
  });
});

test('carries the message of a failure no reason code describes', async () => {
  const ipc = ipcHarness();
  registerRuntimeHostSessionBundleIpc(
    deps({
      client: {
        importSessionBundle: async () => {
          throw new Error('Invalid Session id');
        },
      },
      dialog: { open: { canceled: false, filePaths: ['/picked/bundle.maka-session'] } },
    }),
    ipc,
  );

  // `failed` is the code nothing downstream can act on, so the message is all
  // the user has -- and losing it is how a wrong id reads as "that did not
  // work" with no way to tell what was wrong.
  assert.deepEqual(await ipc.invoke('session-bundle:import'), {
    ok: false,
    reason: 'failed',
    detail: 'Invalid Session id',
  });
});

function deps(input: {
  client?: Partial<RuntimeHostSessionBundleIpcDeps['client']>;
  dialog?: {
    save?: { canceled: boolean; filePath?: string };
    open?: { canceled: boolean; filePaths: string[] };
    workspace?: { canceled: boolean; filePaths: string[] };
  };
  onSessionsChanged?: (reason: string, sessionId?: string) => void;
}): RuntimeHostSessionBundleIpcDeps {
  return {
    client: {
      previewSessionBundle: async () => ({ sessionCount: 1, subtreeDigest: '0'.repeat(64) }),
      exportSessionBundle: async () => ({ sessionCount: 0, compressedBytes: 0 }),
      importSessionBundle: async () => ({ sessionCount: 0, artifactFiles: 0 }),
      ...input.client,
    },
    mainWindowController: {
      showSaveDialog: async () => input.dialog?.save ?? { canceled: true },
      showOpenDialog: async (options) => options.properties?.includes('openDirectory')
        ? input.dialog?.workspace ?? { canceled: false, filePaths: ['/selected/workspace'] }
        : input.dialog?.open ?? { canceled: true, filePaths: [] },
    },
    emitSessionsChanged: (reason, sessionId) => input.onSessionsChanged?.(reason, sessionId),
  };
}

function ipcHarness() {
  const handlers = new Map<string, IpcHandler>();
  return {
    handle(channel: string, handler: IpcHandler): void {
      handlers.set(channel, handler);
    },
    async invoke(channel: string, ...args: unknown[]): Promise<unknown> {
      const handler = handlers.get(channel);
      assert.ok(handler, `missing handler: ${channel}`);
      return handler({ sender: { id: 1 } } as never, ...args);
    },
  };
}

test('proposes a filename a task name cannot steer', () => {
  // The dialog still decides the path. This decides what it opens holding, and
  // a task name is written by a person.
  assert.equal(bundleFileName('Refactor compaction'), 'Refactor compaction');
  assert.equal(bundleFileName('../../etc/passwd'), 'etc passwd', 'no separators survive');
  assert.equal(bundleFileName('.hidden'), 'hidden', 'not a dotfile');
  assert.equal(bundleFileName('   '), 'maka-session', 'a blank name still names something');
  assert.equal(bundleFileName(undefined), 'maka-session');
  assert.ok(bundleFileName('x'.repeat(500)).length <= 80, 'bounded for the filesystem');
});

test('passes the Host preview unchanged through export and stops when workspace selection is canceled', async () => {
  let sent: unknown;
  const ipc = ipcHarness();
  registerRuntimeHostSessionBundleIpc(
    deps({
      client: {
        previewSessionBundle: async (id) => {
          assert.equal(id, 'root');
          return { sessionCount: 3, subtreeDigest: 'a'.repeat(64) };
        },
        importSessionBundle: async () => { throw new Error('canceled import must not reach Host'); },
        exportSessionBundle: async (input) => {
          sent = input.expectedSubtreeDigest;
          return { sessionCount: 2, compressedBytes: 1 };
        },
      },
      dialog: {
        save: { canceled: false, filePath: '/picked/Hello.maka-session' },
        open: { canceled: false, filePaths: ['/picked/source.maka-session'] },
        workspace: { canceled: true, filePaths: [] },
      },
    }),
    ipc,
  );

  const preview = await ipc.invoke('session-bundle:preview', 'root');
  assert.deepEqual(preview, { ok: true, sessionCount: 3, subtreeDigest: 'a'.repeat(64) });
  await ipc.invoke('session-bundle:export', 'root', 'Hello', 'a'.repeat(64));
  assert.equal(sent, 'a'.repeat(64));
  assert.deepEqual(await ipc.invoke('session-bundle:import'), { ok: false, reason: 'canceled' });
});
