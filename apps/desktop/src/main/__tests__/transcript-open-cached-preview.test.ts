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
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { runInNewContext } from 'node:vm';
import test from 'node:test';
import { build } from 'esbuild';
import type { StoredMessage } from '@maka/core/session';
import type { MakaBridge } from '../../preload/bridge-contract.js';
import type { DesktopTranscriptBatch } from '../../preload/transcript-contract.js';
import { encodeDesktopTranscriptSnapshot } from '../desktop-transcript-ipc.js';
import { DesktopTranscriptRangeStore } from '../../renderer/platform/desktop/desktop-transcript-range-store.js';
import { desktopSessionKey } from '../../shared/runtime-host-identity.js';
import { deferred, waitFor } from '@maka/core/test-only/async-primitives';

const OWNER = {
  hostId: 'host-1', targetEpoch: 'epoch-1', profileId: 'local',
  profileName: 'Local', profileKind: 'local', profileAccess: 'owner', readiness: 'ready',
};
const SESSION_ID = desktopSessionKey({ hostId: OWNER.hostId, sessionId: 'session-1' });

const message = (id: string, turnId: string): StoredMessage => ({
  type: 'user', id, turnId, ts: 1, text: id,
});
const CACHED_TAIL = [
  { sequence: 19, message: message('m19', 't19') },
  { sequence: 20, message: message('m20', 't20') },
];
const FULL = Array.from({ length: 20 }, (_, index) => ({
  sequence: index + 1, message: message(`m${index + 1}`, `t${index + 1}`),
}));

interface Harness {
  readonly bridge: MakaBridge;
  readonly state: { cacheReads: number };
  liveListeners(): number;
}

async function preloadHarness(options: {
  liveOpenFails?: boolean;
  partialLive?: boolean;
  beforeCacheRead?(): Promise<void>;
}): Promise<Harness> {
  let bridge: MakaBridge | undefined;
  let consumerId = '';
  let deliverySequence = 0;
  const state = { cacheReads: 0 };
  const listeners = new Map<string, (...args: unknown[]) => void>();
  const deliver = (batch: Omit<DesktopTranscriptBatch, 'deliverySequence'>) => {
    listeners.get(`sessions:transcript:${consumerId}`)?.({}, OWNER, {
      ...batch, deliverySequence: ++deliverySequence,
    });
  };
  const ipcRenderer = {
    on(channel: string, listener: (...args: unknown[]) => void) { listeners.set(channel, listener); },
    off(channel: string) { listeners.delete(channel); },
    send() {},
    async invoke(channel: string, ...args: unknown[]): Promise<unknown> {
      if (channel === 'app:bootstrapReady') return;
      if (channel === 'runtime-host:activeIdentity') return OWNER;
      if (channel === 'runtime-host:identities') return [OWNER];
      if (channel === 'session-local:transcript') {
        state.cacheReads += 1;
        await options.beforeCacheRead?.();
        return {
          cachedAt: 1,
          batches: [...encodeDesktopTranscriptSnapshot({
            beginsAtTurnBoundary: true,
            sessionId: 'session-1', generation: 'cached:g1', hostEpoch: 'epoch-1',
            durableThrough: 20, durable: CACHED_TAIL, hasOlder: true,
          })],
        };
      }
      if (channel === 'sessions:transcript:open') {
        consumerId = args[2] as string;
        if (options.partialLive) {
          const batch = [...encodeDesktopTranscriptSnapshot({
            beginsAtTurnBoundary: true,
            sessionId: 'session-1', generation: 'live-1', hostEpoch: 'epoch-1',
            durableThrough: 20, durable: CACHED_TAIL, hasOlder: true,
          })][0]!;
          deliver({ ...batch, ready: false });
        }
        if (options.liveOpenFails) throw new Error('live transcript unavailable');
        setImmediate(() => {
          for (const batch of encodeDesktopTranscriptSnapshot({
            beginsAtTurnBoundary: true,
            sessionId: 'session-1', generation: 'live-1', hostEpoch: 'epoch-1',
            durableThrough: 20, durable: FULL, hasOlder: false,
          })) deliver(batch);
        });
        return { kind: 'ready', value: {
          sessionId: 'session-1', generation: 'live-1', hostEpoch: 'epoch-1',
          readThroughMessageId: null,
        } };
      }
      if (
        channel === 'sessions:transcript:ack' ||
        channel === 'sessions:transcript:acknowledge-tail' ||
        channel === 'sessions:transcript:close'
      ) return;
      throw new Error(`Unexpected channel: ${channel}`);
    },
  };
  const bundle = await build({
    entryPoints: [fileURLToPath(new URL('../../../src/preload/preload.ts', import.meta.url))],
    bundle: true, write: false, platform: 'node', format: 'cjs', external: ['electron'],
  });
  const require = createRequire(import.meta.url);
  runInNewContext(bundle.outputFiles[0]!.text, {
    require: (id: string) => id === 'electron' ? {
      ipcRenderer,
      contextBridge: { exposeInMainWorld(name: string, value: MakaBridge) {
        if (name === 'maka') bridge = value;
      } },
    } : require(id),
    process: { env: {} }, Buffer, console, setTimeout, clearTimeout, TextEncoder, TextDecoder,
    Uint8Array, crypto: globalThis.crypto,
  });
  assert.ok(bridge);
  return { bridge, state, liveListeners: () => [...listeners.keys()].filter((key) => key.startsWith('sessions:transcript:')).length };
}

function publications(store: DesktopTranscriptRangeStore) {
  const seen: Array<{ ids: string[]; hasOlder: boolean }> = [];
  store.subscribe(() => {
    const snapshot = store.snapshot();
    if (snapshot.ready) {
      seen.push({ ids: snapshot.messages.map((entry) => entry.id), hasOlder: snapshot.hasOlder });
    }
  });
  return seen;
}

// A healthy open publishes the live answer as the first history; the cached
// tail is not a preview the reader ever sees.
test('a healthy transcript open publishes only the live answer and never reads the cache', async () => {
  const { bridge, state } = await preloadHarness({ liveOpenFails: false });
  const store = new DesktopTranscriptRangeStore(SESSION_ID);
  const seen = publications(store);
  const handle = await bridge.transcripts.open(SESSION_ID, (batch) => store.accept(batch), () => {}, 'history');
  await waitFor(() => seen.length === 1, { timeoutMs: 5_000 });
  await handle.close();
  assert.deepEqual(seen, [{ ids: FULL.map((entry) => entry.message.id), hasOlder: false }]);
  assert.equal(state.cacheReads, 0);
});

// The cache stands in only when the live read never answered; even then it
// advertises no earlier history because nothing can serve the read.
test('a failed live open falls back to the cached transcript without earlier history', async () => {
  const { bridge, state } = await preloadHarness({ liveOpenFails: true });
  const store = new DesktopTranscriptRangeStore(SESSION_ID);
  const seen = publications(store);
  const handle = await bridge.transcripts.open(SESSION_ID, (batch) => store.accept(batch), () => {}, 'history');
  await waitFor(() => seen.length === 1, { timeoutMs: 5_000 });
  assert.deepEqual(seen, [{ ids: ['m19', 'm20'], hasOlder: false }]);
  assert.equal(state.cacheReads, 1);
  assert.equal(handle.generation, 'cached:g1');
  await assert.rejects(handle.loadEarlier(), /Reconnect the Host/);
  await handle.close();
});

test('cancellation during cache fallback cannot publish or retain the abandoned consumer', async () => {
  const started = deferred<void>();
  const release = deferred<void>();
  const { bridge, liveListeners } = await preloadHarness({
    liveOpenFails: true,
    beforeCacheRead: () => { started.resolve(); return release.promise; },
  });
  const delivered: DesktopTranscriptBatch[] = [];
  let cancel = () => {};
  const opened = bridge.transcripts.open(SESSION_ID, (batch) => delivered.push(batch), (close) => { cancel = close; });
  const rejected = assert.rejects(opened, /cancelled/);
  await started.promise;
  cancel();
  release.resolve();
  await rejected;
  assert.deepEqual(delivered, []);
  assert.equal(liveListeners(), 0);
});

test('a partially delivered live generation never falls back to older cached history', async () => {
  const { bridge, state, liveListeners } = await preloadHarness({ liveOpenFails: true, partialLive: true });
  const delivered: DesktopTranscriptBatch[] = [];
  await assert.rejects(bridge.transcripts.open(SESSION_ID, (batch) => delivered.push(batch)), /live transcript unavailable/);
  assert.equal(delivered.length, 1);
  assert.equal(delivered[0]!.generation, 'live-1');
  assert.equal(state.cacheReads, 0);
  assert.equal(liveListeners(), 0);
});
