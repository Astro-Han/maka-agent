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
import { clientPluginRemote } from '../../renderer/platform/desktop/client-plugin-remote.js';
import { ClientPluginSessionSlot } from '../../renderer/features/client-plugins/index.js';
import { desktopSessionKey } from '../../shared/runtime-host-identity.js';

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

test('plugin consent is explicit and cannot outlive its document or publication', async () => {
  const owner = renderer();
  const handlers = new Map<string, IpcHandler>();
  let nonce = randomUUID();
  let published = true;
  let confirmation = deferred<boolean>();
  let showing = deferred<AbortSignal>();
  let writes = 0;
  const identity = {entryId:'view',extensionId:'example',activation:randomUUID(),contentDigest:'sha256-'+'a'.repeat(64),clientDigest:'sha256-'+'b'.repeat(64)};
  const proposal = {client:identity,scope:'profile',command:{kind:'approve',request:{operationId:randomUUID(),title:'Send reminders',target:{kind:'profile'},capabilities:['notifications']}}};
  const dispose = registerClientPluginRemoteIpc({
    ipcMain: {handle: (name, handler) => {handlers.set(name,handler);}},
    ownsRenderer: (contents) => contents === owner.emitter as unknown as WebContents,
    report: assert.ifError,
    client: {hostEpoch:'host',async request() {throw new Error('Unexpected Remote command');}},
    authorization: {
      async validate() {if (!published) throw new Error('Client publication retired');},
      confirm(_input, signal) {showing.resolve(signal); return confirmation.promise;},
      async request() {writes++; return {kind:'grant',grant:null};},
    },
  });
  try {
    const {epoch} = await handlers.get('plugins:connection')!(owner.event, nonce);
    const invoke = (requestId = randomUUID()) => handlers.get('plugins:authorization')!(owner.event, nonce, epoch, proposal, requestId);
    let pending = invoke();
    await showing.promise;
    confirmation.resolve(false);
    assert.deepEqual(await pending,{kind:'grant',grant:null});
    assert.equal(writes,0);
    confirmation = deferred(); showing = deferred();
    pending = invoke();
    await showing.promise;
    confirmation.resolve(true);
    await pending;
    assert.equal(writes,1);
    confirmation = deferred(); showing = deferred();
    const requestId = randomUUID();
    pending = invoke(requestId);
    const retiredSignal = await showing.promise;
    await handlers.get('plugins:authorization-cancel')!(owner.event, randomUUID(), epoch, requestId);
    assert.equal(retiredSignal.aborted,false,'another document cannot cancel this approval');
    await handlers.get('plugins:authorization-cancel')!(owner.event, nonce, epoch, requestId);
    assert.equal(retiredSignal.aborted,true);
    confirmation.resolve(true);
    assert.deepEqual(await pending,{kind:'grant',grant:null});
    assert.equal(writes,1);
    confirmation = deferred(); showing = deferred();
    pending = invoke();
    const signal = await showing.promise;
    owner.emitter.emit('did-start-navigation',{},'new-page',false,true);
    assert.equal(signal.aborted,true);
    confirmation.resolve(true); // A late click must never reach Host.
    assert.deepEqual(await pending,{kind:'grant',grant:null});
    assert.equal(writes,1);
    nonce = randomUUID();
    await handlers.get('plugins:connection')!(owner.event, nonce);
    confirmation = deferred(); showing = deferred();
    const rejected = assert.rejects(invoke(),/publication retired/);
    await showing.promise;
    published = false;
    confirmation.resolve(true);
    await rejected;
    assert.equal(writes,1);
  } finally {await dispose();}
  assert.deepEqual(owner.emitter.eventNames(),[]);
});

test('reconnecting to the same Host revokes old Remote leases without fencing the replacement Client', async () => {
  const owner = renderer();
  const nonce = randomUUID();
  const handlers = new Map<string, IpcHandler>();
  const calls: unknown[] = [];
  const closes: string[] = [];
  const sessionId = randomUUID();
  const host = { profileId: 'origin', hostId: 'host' };
  const input = { sessionId: desktopSessionKey({ hostId: host.hostId, sessionId }), locale: 'en' as const, onOpenSession() {} };
  const composer = ClientPluginSessionSlot({ host, input, name: 'session.composer.before' });
  assert.equal(composer.props.input.sessionId, sessionId);
  assert.throws(() => ClientPluginSessionSlot({ host: { ...host, hostId: 'other' }, input, name: 'session.composer.before' }), /another Host/);
  const footer = ClientPluginSessionSlot({ host, name: 'turn.footer', input: { sessionId: input.sessionId, turnId: 'turn-1', locale: 'en' } });
  assert.equal(footer.props.input.sessionId, composer.props.input.sessionId);
  assert.equal(footer.props.input.turnId, 'turn-1');
  const register = () => registerClientPluginRemoteIpc({
    ipcMain: { handle: (name, handler) => { handlers.set(name, handler); } },
    ownsRenderer: contents => contents === owner.emitter as unknown as WebContents,
    report: assert.ifError,
    client: { hostEpoch: 'same-host', async request(_operation, input) {
      switch (input.kind) {
        case 'open_document': return { kind: 'document', document: randomUUID() };
        case 'bind':
          assert.equal(input.binding.sessionId, sessionId);
          return { kind: 'bound', handler: 'method', target: {
          entryId: 'backend', activation: randomUUID(), registration: randomUUID(),
        } };
        case 'call': calls.push(input.input); return { kind: 'value', value: input.input };
        case 'close_document': closes.push(input.document); return { kind: 'closed' };
        default: assert.fail('unexpected request');
      }
    } },
  });
  const identity = { entryId: 'view', extensionId: 'demo', activation: randomUUID(), contentDigest: 'sha256-' + 'a'.repeat(64), clientDigest: 'sha256-' + 'b'.repeat(64) };
  const remote = (epoch: string) => clientPluginRemote(
    (_host, expected, input) => handlers.get('plugins:remote')!(owner.event, nonce, expected, input),
    { profileId: 'origin', hostId: 'host' }, epoch,
  )(identity, new AbortController().signal);
  let dispose = register();
  try {
    const before = await handlers.get('plugins:connection')!(owner.event, nonce);
    const old = remote(before.epoch);
    const call = old.api.method<string, string>('echo', composer.props.input.sessionId);
    assert.equal(await call('before reconnect'), 'before reconnect');
    await assert.rejects(old.api.method('echo', input.sessionId)(null), /Invalid Remote Session/);
    assert.deepEqual(calls, ['before reconnect'], 'a Desktop projection never reaches the Host');
    await dispose();
    dispose = register();
    const after = await handlers.get('plugins:connection')!(owner.event, nonce);
    assert.equal(before.hostEpoch, after.hostEpoch);
    assert.notEqual(before.epoch, after.epoch);
    await assert.rejects(call('never replay'));
    await old.close();
    const next = remote(after.epoch);
    assert.equal(await next.api.method<string, string>('echo', sessionId)('after reconnect'), 'after reconnect');
    await next.close();
    assert.deepEqual(calls, ['before reconnect', 'after reconnect']);
    assert.equal(closes.length, 2);
  } finally { await dispose(); }
});

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
    ipcMain: { handle: (channel, listener) => { if (channel === 'plugins:remote') handler = listener; else if (channel === 'plugins:files') files = listener; else if (channel === 'plugins:connection') connection = listener; } },
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
  const { epoch, hostEpoch } = await connection(one.event, nonce);
  const invoke = (event: IpcMainInvokeEvent, input: unknown) => handler(event, nonce, epoch, input);
  try {
    assert.equal(hostEpoch, 'host-process');
    await assert.rejects(async () => connection({ ...one.event, senderFrame: two.event.senderFrame }, nonce), /live Desktop/);
    await assert.rejects(files(one.event, nonce, epoch, {}, {kind:'pick'}), /unavailable for this Host/);
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
  let connection!: IpcHandler;
  const owner = renderer();
  const selection = deferred<string | null>();
  const picking = deferred<void>();
  const opened: string[] = [];
  let published = true;
  const identity = {entryId:'view', activation:randomUUID(), clientDigest:'sha256-'+'a'.repeat(64)};
  const dispose = registerClientPluginRemoteIpc({
    ipcMain: {handle(channel, listener) { if (channel === 'plugins:files') files = listener; else if (channel === 'plugins:connection') connection = listener; }},
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
  const { epoch } = await connection(owner.event, nonce);
  const path = resolve('SKILL.md');
  try {
    await files(owner.event, nonce, epoch, identity, {kind:'open',path});
    published = false;
    await assert.rejects(files(owner.event, nonce, epoch, identity, {kind:'open',path}), /publication retired/);
    assert.deepEqual(opened,[path]);
    published = true;
    const rejected = assert.rejects(files(owner.event, nonce, epoch, identity, {kind:'pick'}), /retired during file selection/);
    await picking.promise;
    owner.emitter.emit('did-start-navigation', {}, 'new-page', false, true);
    selection.resolve(path);
    await rejected;
  } finally { await dispose(); }
  assert.deepEqual(owner.emitter.eventNames(),[]);
});
