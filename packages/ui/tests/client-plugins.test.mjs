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
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { ClientRuntime, ClientSlot } from '../dist/client-plugins/index.js';

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

test('public events publish with their instance and immediately stop delivery on disposal or retirement', async () => {
  const fixture = { initializing: deferred(), context: undefined, release: undefined, values: [] };
  const source = bundle(`
    const f = require('fixture'); f.context = ctx;
    f.release = ctx.events.subscribe({kind:'session.changed'}, event => f.values.push(event));
    await f.initializing.promise;
  `);
  const callbacks = [];
  const errors = [];
  let cleaned = 0;
  const runtime = new ClientRuntime({
    document: documentHarness(), modules: { fixture }, source: async () => source,
    report: ({ error }) => errors.push(error),
    events: () => ({ subscribe(_request, listener) {
      callbacks.push(listener);
      return async () => { cleaned++; };
    } }),
  });
  const publishing = runtime.reconcile({ revision: 'one', entries: [descriptor(source, 'one')] });
  fixture.initializing.resolve();
  assert.equal(callbacks.length, 0);
  await publishing;
  assert.equal(callbacks.length, 1);
  const event = { kind: 'session.changed', sessionId: 's', reason: 'updated', ts: 1 };
  callbacks[0](event);
  fixture.release(); fixture.release();
  callbacks[0](event);
  assert.equal(fixture.values.length, 1, 'stop before deferred effect cleanup runs');
  fixture.context.events.subscribe({ kind: 'session.changed' }, () => { throw new Error('listener failed'); });
  callbacks[1](event);
  assert.match(errors[0].message, /listener failed/);
  const closing = runtime.close();
  callbacks[1](event);
  await closing;
  assert.equal(errors.length, 1);
  assert.equal(cleaned, 2);
});

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
  assert.equal(renderToStaticMarkup(createElement(ClientSlot, {
    store: one.slots, name: 'turn.footer', input: { sessionId: 's', turnId: 't', locale: 'en' }, onError() {},
  })), '', 'an unpopulated anchor must not create an empty layout item');
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
    fixture.release=ctx.effect(()=>()=>{throw new Error('cleanup failed')});
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
  fixture.release();
  fixture.release();
  await Promise.resolve();
  await assert.rejects(runtime.reconcile({ revision: 'two', entries: [descriptor(source, 'two')] }), /cleanup unconfirmed/);
  assert.deepEqual(runtime.slots.snapshot(), []);
  await assert.rejects(call('retired'), /not effective/);
  assert.equal(calls, 1);
  assert.equal(closed, 1);
  await assert.rejects(runtime.reconcile({ revision: 'three', entries: [descriptor(source, 'new')] }), /reload the document/);
  await runtime.close();
});

test('published effects release independently, reclaim capacity and drain late cleanup exactly once', async () => {
  const document = documentHarness();
  const fixture = { context: undefined, events: [], draining: deferred() };
  const source = bundle(`
    const fixture=require('fixture');
    fixture.context=ctx;
    const cancelled=ctx.effect(()=>{throw new Error('cancelled setup ran')});
    cancelled(); cancelled();
    const self=ctx.effect(()=>{self();return ()=>fixture.events.push('self-cleanup')});
  `);
  const runtime = new ClientRuntime({ document, modules: { fixture }, source: async () => source, report() {} });
  await runtime.reconcile({ revision: 'one', entries: [descriptor(source, 'one')] });
  const context = fixture.context;
  assert.deepEqual(fixture.events, ['self-cleanup']);
  // Repeated component mounts release their stylesheet capacity, not just DOM nodes.
  for (let i = 0; i < 130; i++) {
    const dispose = context.style('.mounted {}');
    assert.equal(document.querySelectorAll('style').length, 1);
    dispose(); dispose();
    await Promise.resolve(); await Promise.resolve();
    assert.equal(document.querySelectorAll('style').length, 0);
  }
  const dispose = context.effect(() => {
    fixture.events.push('start');
    return async () => { fixture.events.push('draining'); await fixture.draining.promise; fixture.events.push('done'); };
  });
  dispose(); dispose();
  let closed = false;
  const closing = runtime.close().then(() => { closed = true; });
  await Promise.resolve(); await Promise.resolve();
  assert.equal(closed, false);
  assert.deepEqual(fixture.events, ['self-cleanup', 'start', 'draining']);
  assert.throws(() => context.effect(() => {}), /closed/);
  fixture.draining.resolve();
  await closing;
  dispose();
  assert.deepEqual(fixture.events, ['self-cleanup', 'start', 'draining', 'done']);
});

test('unrelated catalog changes preserve UI state; dependency replacement and connection changes revoke exact modules', async () => {
  const document = documentHarness();
  const generations = {};
  const observed = {};
  const source = (id, dependency) => `window.__MakaClientBundle__({id:${JSON.stringify(id)},factory(require){
    const fixture=require('fixture');
    const generation=fixture.generations[${JSON.stringify(id)}]=(fixture.generations[${JSON.stringify(id)}]||0)+1;
    const dependency=${dependency ? `require(${JSON.stringify(dependency)}).generation` : 'null'};
    return {generation,default:{activate(ctx){
      fixture.observed[${JSON.stringify(id)}]={generation,dependency,signal:ctx.signal,hostEpoch:ctx.hostEpoch};
      ctx.slots.register('session.composer.before','panel',()=>null);
      ctx.style('.owned-by-${id} {}');
    }}};
  }});`;
  const sources = new Map([
    ['base', source('base')],
    ['dependent', source('dependent', 'base')],
    ['independent', source('independent')],
  ]);
  const entries = [...sources].map(([id, source]) => ({
    ...descriptor(source, id + '-one'), entryId: id + '.ui', extensionId: id,
    dependencies: id === 'dependent' ? ['base'] : [],
  }));
  const runtime = new ClientRuntime({
    document, modules: { fixture: { generations, observed } }, report() {},
    source: async (descriptor) => sources.get(descriptor.extensionId),
  });
  const snapshot = { revision: 'initial', connection: 'connection-one', hostEpoch: 'host-one', entries };
  try {
    await runtime.reconcile(snapshot);
    assert.equal(observed.base.hostEpoch, 'host-one');
    const independent = runtime.slots.snapshot().find((entry) => entry.owner.extensionId === 'independent');
    const originalBase = observed.base;
    await runtime.reconcile({ ...snapshot, revision: 'unrelated-control-change' });
    assert.deepEqual(generations, { base: 1, dependent: 1, independent: 1 });
    assert.equal(runtime.slots.snapshot().find((entry) => entry.owner.extensionId === 'independent'), independent);
    assert.equal(originalBase.signal.aborted, false);

    const invalid = [{ ...entries[0], activation: 'replacement', sdkVersion: 999 }, ...entries.slice(1)];
    await assert.rejects(runtime.reconcile({ ...snapshot, revision: 'broken', entries: invalid }), /Incompatible/);
    assert.equal(originalBase.signal.aborted, true);
    assert.equal(observed.dependent.signal.aborted, true);
    assert.equal(observed.independent.signal.aborted, false);
    assert.deepEqual(runtime.slots.snapshot(), [independent]);
    // A failed reconcile cannot make the last successful revision a false no-op.
    await runtime.reconcile(snapshot);
    assert.deepEqual(generations, { base: 2, dependent: 2, independent: 1 });
    assert.equal(observed.dependent.dependency, 2);

    const replaced = [{ ...entries[0], activation: 'replacement' }, ...entries.slice(1)];
    await runtime.reconcile({ ...snapshot, revision: 'updated', entries: replaced });
    assert.deepEqual(generations, { base: 3, dependent: 3, independent: 1 });
    assert.equal(observed.dependent.dependency, 3, 'dependent must import the replacement module');
    assert.equal(runtime.slots.snapshot().find((entry) => entry.owner.extensionId === 'independent'), independent);
    const beforeReconnect = observed.independent;
    const reconnected = { revision: 'updated', connection: 'connection-two', hostEpoch: 'host-one', entries: replaced };
    await runtime.reconcile(reconnected);
    assert.equal(beforeReconnect.signal.aborted, true);
    assert.deepEqual(generations, { base: 4, dependent: 4, independent: 2 });
    const beforeRestart = observed.independent;
    await runtime.reconcile({ ...reconnected, hostEpoch: 'host-two' });
    assert.equal(beforeRestart.signal.aborted, true);
    assert.equal(observed.independent.hostEpoch, 'host-two');
    assert.deepEqual(generations, { base: 5, dependent: 5, independent: 3 });
    assert.equal(document.querySelectorAll('style').length, 3);
  } finally { await runtime.close(); }
  assert.equal(document.querySelectorAll('style,script').length, 0);
});
