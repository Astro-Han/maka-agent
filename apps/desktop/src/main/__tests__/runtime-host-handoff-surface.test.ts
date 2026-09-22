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
import type { HostHandoffAction, HostHandoffView } from '@maka/runtime-host/client';
import type { DesktopHostHandoffPayload } from '../../preload/bridge-contract.js';
import { createDesktopHostHandoffSurface } from '../runtime-host-handoff-surface.js';

function attention(revision: string): HostHandoffView {
  return { revision, state: 'attention', reason: 'retry_required', target: { name: 'Local', location: 'local' },
    mayExitNaturally: false, actions: ['cancel', 'retry'], defaultAction: 'cancel' };
}
const flush = () => new Promise<void>((resolve) => setImmediate(resolve));

function harness(deferLocale = false) {
  const handlers = new Map<string, (event: unknown, payload?: unknown) => unknown>();
  const sent: (DesktopHostHandoffPayload | null)[] = [];
  const locales: (() => void)[] = [];
  const authorized = {};
  let focuses = 0;
  const surface = createDesktopHostHandoffSurface({
    ipcMain: { handle(channel: string, handler: (event: unknown, payload?: unknown) => unknown) { handlers.set(channel, handler); } } as never,
    authorize: (event) => event === authorized,
    send: (payload) => sent.push(payload),
    focus: () => { focuses++; },
    resolveLocale: () => deferLocale ? new Promise((resolve) => locales.push(() => resolve('en'))) : Promise.resolve('en'),
  });
  return { surface, sent, locales, handlers, authorized, focuses: () => focuses };
}

test('only an authorized live decision can submit; retired surfaces cannot return', async () => {
  const h = harness();
  const decisions: [string, HostHandoffAction][] = [];
  const first = h.surface((revision, action) => decisions.push([revision, action]));
  const second = h.surface(() => {});
  first.update(attention('first'));
  await flush();
  second.update({ ...attention('second'), state: 'progress', phase: 'staging' } as HostHandoffView);
  await flush();
  assert.equal(h.sent.at(-1)?.view.revision, 'first');
  assert.equal(h.focuses(), 1);
  const decide = h.handlers.get('runtime-host-handoff:decide')!;
  assert.throws(() => decide({}, { revision: 'first', action: 'retry' }), /main renderer/);
  assert.throws(() => decide(h.authorized, { revision: 'first', action: 'replace' }), /no longer/);
  assert.throws(() => decide(h.authorized, { revision: 'old', action: 'cancel' }), /no longer/);
  decide(h.authorized, { revision: 'first', action: 'retry' });
  assert.deepEqual(decisions, [['first', 'retry']]);
  second.update(attention('second'));
  await flush();
  assert.equal(h.sent.at(-1)?.view.revision, 'second');
  second.close();
  await flush();
  assert.equal(h.sent.at(-1)?.view.revision, 'first');
  first.close();
  first.update(attention('resurrected'));
  await flush();
  assert.equal(h.sent.at(-1), null);
  assert.throws(() => decide(h.authorized, { revision: 'first', action: 'retry' }), /no longer/);
});

test('a late localized publication cannot resurrect a closed decision', async () => {
  const h = harness(true);
  const surface = h.surface(() => {});
  surface.update(attention('old'));
  surface.close();
  await flush();
  h.locales[0]!();
  await flush();
  assert.deepEqual(h.sent, [null]);
  assert.equal(h.focuses(), 0);
});
