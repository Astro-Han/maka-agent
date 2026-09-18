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
import { createHash } from 'node:crypto';
import { runInNewContext } from 'node:vm';
import { test } from 'node:test';
import { parseHTML } from 'linkedom';
import { ClientRuntime } from '../dist/client-plugins/index.js';

function documentHarness() {
  const { document, window } = parseHTML('<html><head></head><body></body></html>');
  const append = document.head.append.bind(document.head);
  document.head.append = (node) => {
    append(node);
    if (node.tagName !== 'SCRIPT') return;
    void fetch(node.src).then((reply) => reply.text()).then((source) => {
      if (!node.isConnected) return;
      Object.defineProperty(document, 'currentScript', { configurable: true, value: node });
      try {
        runInNewContext(source, { window });
        node.onload?.();
      } catch { node.onerror?.(); }
      finally { Object.defineProperty(document, 'currentScript', { configurable: true, value: null }); }
    });
  };
  return document;
}

function bundle(body) {
  return 'window.__MakaClientBundle__({id:"demo.client",factory(require){return {default:{async activate(ctx,config){' +
    body + '}}}}});';
}
function descriptor(source, activation) {
  return {
    entryId: 'demo.ui', extensionId: 'demo.client', activation,
    contentDigest: 'package-' + activation,
    clientDigest: 'sha256-' + createHash('sha256').update(source).digest('hex'),
    sdkVersion: 1, dependencies: [], config: {}, totalBytes: Buffer.byteLength(source),
  };
}
function deferred() {
  let resolve;
  const promise = new Promise((complete) => { resolve = complete; });
  return { promise, resolve };
}

test('independent Hosts load exact bytes, revoke stale UI, retry the same revision and release scripts/effects', async () => {
  const document = documentHarness();
  const events = [];
  const source = bundle(`
    ctx.slots.register('session.composer.before','panel',()=>null);
    ctx.style('.demo { color: red; }');
    ctx.effect(()=>{require('fixture').events.push('start:'+config.host);return ()=>require('fixture').events.push('stop:'+config.host)});
  `);
  const first = descriptor(source, 'a');
  first.config = { host: 'one' };
  const second = descriptor(source, 'b');
  second.config = { host: 'two' };
  let transient = false;
  let tamper = false;
  const options = {
    document, modules: { fixture: { events } }, report() {},
    source: async () => {
      if (transient) { transient = false; throw new Error('temporary transport failure'); }
      return tamper ? source + ' ' : source;
    },
  };
  const one = new ClientRuntime(options);
  const two = new ClientRuntime(options);
  try {
    await Promise.all([
      one.reconcile({ revision: 'one', entries: [first] }),
      two.reconcile({ revision: 'two', entries: [second] }),
    ]);
    assert.equal(one.slots.snapshot()[0].owner.activation, 'a');
    assert.equal(two.slots.snapshot()[0].owner.activation, 'b');
    assert.equal(document.querySelectorAll('script').length, 0);
    assert.equal(document.querySelectorAll('style').length, 2);
    // Disable first: the other Host's identically named Entry remains active.
    await one.reconcile({ revision: 'empty', entries: [] });
    assert.equal(two.slots.snapshot().length, 1);
    assert.equal(document.querySelectorAll('style').length, 1);
    tamper = true;
    await assert.rejects(one.reconcile({ revision: 'retry', entries: [first] }), /length mismatch/);
    tamper = false;
    transient = true;
    await assert.rejects(one.reconcile({ revision: 'retry', entries: [first] }), /temporary/);
    await one.reconcile({ revision: 'retry', entries: [first] });
    assert.equal(one.slots.snapshot().length, 1);
    // A failed replacement must not leave a retired generation visible.
    const incompatible = { ...first, activation: 'c', sdkVersion: 999 };
    await assert.rejects(one.reconcile({ revision: 'incompatible', entries: [incompatible] }), /Incompatible/);
    assert.deepEqual(one.slots.snapshot(), []);
    assert.equal(two.slots.snapshot().length, 1);
  } finally {
    await Promise.all([one.close(), two.close()]);
  }
  assert.equal(document.querySelectorAll('style,script').length, 0);
  assert.equal(events.filter((event) => event.startsWith('start')).length, 3);
  assert.equal(events.filter((event) => event.startsWith('stop')).length, 3);
});

test('superseded initialization drains its late handle without publishing effects or accepting stale registrations', async () => {
  const document = documentHarness();
  const entered = deferred();
  const release = deferred();
  const events = [];
  const slow = bundle(`
    const fixture=require('fixture');
    fixture.entered.resolve(ctx);
    ctx.effect(()=>{fixture.events.push('unexpected-start')});
    await fixture.release.promise;
    return ()=>{fixture.events.push('late-cleanup')};
  `);
  const fast = bundle(`
    ctx.slots.register('session.composer.before','panel',()=>null);
    ctx.effect(()=>{require('fixture').events.push('new-start');return ()=>require('fixture').events.push('new-stop')});
  `);
  const old = descriptor(slow, 'old');
  const next = descriptor(fast, 'new');
  const runtime = new ClientRuntime({
    document, modules: { fixture: { entered, release, events } }, report() {},
    source: async (entry) => entry.activation === 'old' ? slow : fast,
  });
  const initial = runtime.reconcile({ revision: 'old', entries: [old] });
  const rejected = assert.rejects(initial, /superseded/);
  const context = await entered.promise;
  const retired = new Promise((resolve) => context.signal.addEventListener('abort', resolve, { once: true }));
  const replacement = runtime.reconcile({ revision: 'new', entries: [next] });
  await retired;
  assert.throws(() => context.effect(() => {}), /closed/);
  assert.deepEqual(runtime.slots.snapshot(), []);
  release.resolve();
  await rejected;
  await replacement;
  assert.deepEqual(events, ['late-cleanup', 'new-start']);
  assert.equal(runtime.slots.snapshot()[0].owner.activation, 'new');
  await runtime.close();
  assert.deepEqual(events, ['late-cleanup', 'new-start', 'new-stop']);
});

test('Remote calls require publication and failed cleanup withdraws UI before fencing replacement', async () => {
  const fixture = { context: undefined, rejected: false };
  let calls = 0;
  let closed = 0;
  const source = bundle(`
    const fixture=require('fixture');
    fixture.context=ctx;
    try { await ctx.remote.method('echo')(null); } catch { fixture.rejected=true; }
    ctx.slots.register('session.composer.before','panel',()=>null);
    ctx.effect(()=>()=>{throw new Error('cleanup failed')});
  `);
  const entry = descriptor(source, 'one');
  const runtime = new ClientRuntime({
    document: documentHarness(), modules: { fixture }, source: async () => source, report() {},
    remote: (identity) => {
      assert.deepEqual(Object.keys(identity).sort(), ['activation', 'clientDigest', 'contentDigest', 'entryId', 'extensionId']);
      return {
      api: { method: () => async (input) => { calls++; return input; }, stream() { throw new Error('unused'); } },
      close: async () => { closed++; },
      };
    },
  });
  await runtime.reconcile({ revision: 'one', entries: [entry] });
  assert.equal(fixture.rejected, true);
  assert.equal(calls, 0);
  const call = fixture.context.remote.method('echo');
  assert.equal(await call('active'), 'active');
  await assert.rejects(runtime.reconcile({ revision: 'two', entries: [entry] }), /cleanup unconfirmed/);
  assert.deepEqual(runtime.slots.snapshot(), []);
  await assert.rejects(call('retired'), /not effective/);
  assert.equal(calls, 1);
  assert.equal(closed, 2);
  await assert.rejects(runtime.reconcile({ revision: 'three', entries: [descriptor(source, 'new')] }), /reload the document/);
  await runtime.close();
});
