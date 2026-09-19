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
import { EventEmitter } from 'node:events';
import { randomUUID } from 'node:crypto';
import { resolve } from 'node:path';
import test from 'node:test';
import type { IpcMainInvokeEvent, WebContents } from 'electron';
import type { IpcHandler } from '../ipc-reconnect-policy.js';
import { registerClientPluginRemoteIpc } from '../client-plugin-remote-ipc.js';

function renderer() {
  const emitter = Object.assign(new EventEmitter(), {
    mainFrame: { frameToken: randomUUID() },
    isDestroyed: () => false,
  });
  const event = { sender: emitter, senderFrame: emitter.mainFrame } as unknown as IpcMainInvokeEvent;
  return { emitter, event };
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { resolve, promise };
}

test('Remote documents belong to one Renderer and drain navigation, late opens and crashes without replay', async () => {
  let handler!: IpcHandler;
  let files!: IpcHandler;
  let connection!: IpcHandler;
  const one = renderer();
  const two = renderer();
  const allowed = new Set([one.emitter, two.emitter]);
  const closed: string[] = [];
  const errors: unknown[] = [];
  let late: ReturnType<typeof deferred<string>> | undefined;
  let attempts = 0;
  const dispose = registerClientPluginRemoteIpc({
    ipcMain: { handle: (channel, listener) => { if (channel === 'plugins:remote') handler = listener; else if (channel === 'plugins:files') files = listener; else connection = listener; } },
    ownsRenderer: (contents: WebContents) => allowed.has(contents as unknown as typeof one.emitter),
    report: (error) => errors.push(error),
    client: { hostEpoch: 'host-process', async request(_operation, input) {
      if (input.kind === 'open_document') return { kind: 'document', document: late ? await late.promise : randomUUID() };
      if (input.kind === 'close_document') { closed.push(input.document); return { kind: 'closed' }; }
      attempts++;
      throw new Error('connection lost after dispatch');
    } },
  });
  const nonce = randomUUID();
  const invoke = (event: IpcMainInvokeEvent, input: unknown) => handler(event, nonce, input);
  try {
    assert.deepEqual(await connection(one.event, nonce), { hostEpoch: 'host-process' });
    await assert.rejects(async () => connection({ ...one.event, senderFrame: two.event.senderFrame }, nonce), /live Desktop/);
    await assert.rejects(files(one.event, nonce, {}, {kind:'pick'}), /unavailable for this Host/);
    const opened = await invoke(one.event, { kind: 'open_document' });
    await assert.rejects(invoke(two.event, { kind: 'next', document: opened.document, stream: randomUUID() }), /belong/);
    await assert.rejects(invoke({ ...one.event, senderFrame: two.event.senderFrame }, { kind: 'open_document' }), /live Desktop/);
    await assert.rejects(invoke(one.event, { kind: 'next', document: opened.document, stream: randomUUID() }), /connection lost/);
    assert.equal(attempts, 1);
    one.emitter.emit('did-start-navigation', {}, 'same-page#hash', true, true);
    assert.deepEqual(closed, []);
    one.emitter.emit('did-start-navigation', {}, 'new-page', false, true);
    assert.deepEqual(closed, [opened.document]);
    assert.equal(one.emitter.listenerCount('destroyed'), 0);

    late = deferred<string>();
    const pending = invoke(one.event, { kind: 'open_document' });
    const rejected = assert.rejects(pending, /retired during open/);
    one.emitter.emit('did-start-navigation', {}, 'newer-page', false, true);
    const lateId = randomUUID();
    late.resolve(lateId);
    await rejected;
    assert.deepEqual(closed, [opened.document, lateId]);

    late = undefined;
    const next = await invoke(one.event, { kind: 'open_document' });
    one.emitter.emit('render-process-gone');
    assert.deepEqual(closed, [opened.document, lateId, next.document]);
  } finally { await dispose(); }
  assert.deepEqual(errors, []);
  for (const { emitter } of [one, two]) assert.deepEqual(emitter.eventNames(), []);
  await assert.rejects(invoke(one.event, { kind: 'open_document' }), /live Desktop/);
});

test('local file actions require a published Client and reject a selection returned after navigation', {timeout: 1000}, async () => {
  let files!: IpcHandler;
  const owner = renderer();
  const selection = deferred<string | null>();
  const picking = deferred<void>();
  const opened: string[] = [];
  let published = true;
  const identity = {entryId:'view', activation:randomUUID(), clientDigest:'sha256-'+'a'.repeat(64)};
  const dispose = registerClientPluginRemoteIpc({
    ipcMain: {handle(channel, listener) { if (channel === 'plugins:files') files = listener; }},
    ownsRenderer: contents => contents === owner.emitter as unknown as WebContents,
    report: assert.ifError,
    client: {hostEpoch: 'host-process', async request() { throw new Error('Unexpected Remote command'); }},
    files: {
      async validate(input) {
        assert.equal(input.kind, 'bundle');
        if (!published) throw new Error('Client publication retired');
      },
      pick() { picking.resolve(); return selection.promise; },
      async open(path) { opened.push(path); },
    },
  });
  const nonce = randomUUID();
  const path = resolve('SKILL.md');
  try {
    await files(owner.event, nonce, identity, {kind:'open',path});
    published = false;
    await assert.rejects(files(owner.event, nonce, identity, {kind:'open',path}), /publication retired/);
    assert.deepEqual(opened,[path]);
    published = true;
    const rejected = assert.rejects(files(owner.event, nonce, identity, {kind:'pick'}), /retired during file selection/);
    await picking.promise;
    owner.emitter.emit('did-start-navigation', {}, 'new-page', false, true);
    selection.resolve(path);
    await rejected;
  } finally { await dispose(); }
  assert.deepEqual(owner.emitter.eventNames(),[]);
});
