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
import { afterEach, describe, it } from 'node:test';
import { act, createElement } from 'react';
import { LocaleProvider } from '@maka/ui';
import type { StoredMessage } from '@maka/core/session';
import { cleanupFakeDom, installReactRenderer } from './fake-dom.js';
import { useAppShellSessionWorkspace } from '../../renderer/use-app-shell-session-workspace.js';

/**
 * The session workspace hands its actions to consumers that put them in
 * dependency arrays and pass them down as props. When `setActiveId` was a
 * function declaration in the hook body it changed identity every render, which
 * rebuilt the whole Session rail command chain and defeated `SessionNavRow`'s
 * `memo` on every commit — measured at 20 full re-renders of a 32-row sidebar
 * for a single session switch. Identity is a contract, not an implementation
 * detail, so it is asserted here rather than left to review.
 */
type Workspace = ReturnType<typeof useAppShellSessionWorkspace>;

/**
 * Every function the hook returns, read off the first render rather than
 * listed here. A hand-kept list covers what someone remembered on the day it
 * was written and silently stops covering whatever is added later, which is
 * the opposite of what a contract test is for.
 */
function actionKeys(workspace: Workspace): string[] {
  return Object.keys(workspace).filter(
    (key) => typeof (workspace as Record<string, unknown>)[key] === 'function',
  );
}

describe('session workspace action identity', () => {
  afterEach(cleanupFakeDom);

  it('hands over identity and rows together and rejects superseded reads', () => {
    const { root } = installReactRenderer();
    let workspace!: Workspace;
    const displays: Array<{ id: string | undefined; messages: StoredMessage[] }> = [];
    function Probe(): null {
      workspace = useAppShellSessionWorkspace({ error: () => {} });
      displays.push({ id: workspace.activeId, messages: workspace.messages });
      return null;
    }
    act(() => root.render(createElement(LocaleProvider, {
      locale: 'en', children: createElement(Probe),
    })));
    act(() => workspace.seedSessions(['a', 'b', 'c'].map((id) => ({
      id, name: id, isFlagged: false, isArchived: false, labels: [],
      hasUnread: false, status: 'active' as const, backend: 'ai-sdk' as const,
      revision: 1, runtimeHostId: 'local', profileId: 'local', profileName: 'Local',
      llmConnectionSlug: 'test', connectionLocked: false, model: 'test',
      permissionMode: 'ask' as const, profileKind: 'local' as const,
    }))));
    const row = (id: string): StoredMessage => ({ id, type: 'user', text: id, turnId: id, ts: 1 });
    const a = [row('a-message')];
    const c = [row('c-message')];
    act(() => { workspace.setActiveId('a'); workspace.commitTranscript('a', a); });
    displays.length = 0;
    act(() => workspace.setActiveId('b'));
    assert.equal(workspace.requestedSessionId, 'b');
    assert.equal(workspace.activeId, 'a');
    assert.equal(workspace.messages, a);
    act(() => workspace.setActiveId('c'));
    act(() => workspace.commitTranscript('b', [row('b-message')]));
    assert.equal(workspace.activeId, 'a');
    act(() => workspace.commitTranscript('c', c));
    assert.equal(workspace.activeId, 'c');
    assert.equal(workspace.messageLoadPending, false);
    assert.ok(displays.every((display) =>
      (display.id === 'a' && display.messages === a) ||
      (display.id === 'c' && display.messages === c)));

    act(() => workspace.setActiveId('b'));
    act(() => workspace.startNewSession());
    act(() => workspace.commitTranscript('b', [row('b-message')]));
    assert.equal(workspace.activeId, undefined);
    assert.deepEqual(workspace.messages, []);
    // A first-send task has no readable Host history yet; it must activate
    // immediately rather than waiting for its own first message to be sent.
    act(() => workspace.setActiveId('new-local-task'));
    assert.equal(workspace.activeId, 'new-local-task');
  });

  it('keeps every action identity fixed across re-renders', () => {
    const { root } = installReactRenderer();
    const reads: Workspace[] = [];

    function Probe(): null {
      reads.push(useAppShellSessionWorkspace({ error: () => {} }));
      return null;
    }

    act(() => {
      root.render(
        createElement(LocaleProvider, { locale: 'en', children: createElement(Probe) }),
      );
    });
    assert.equal(reads.length, 1);

    // Three unrelated state changes, each of which re-renders the hook.
    act(() => reads[0]!.setActiveId('session-a'));
    act(() => reads[0]!.setMessages([]));
    act(() => reads[0]!.setMessageLoadPending(true));
    assert.ok(reads.length > 1, 'the probe should have re-rendered');

    const first = reads[0]!;
    const keys = actionKeys(first);
    // A guard on the guard: if the hook's shape ever collapses, the loop below
    // would pass by having nothing to check.
    assert.ok(keys.length > 10, `expected the workspace to expose actions, saw ${keys.length}`);
    for (const key of keys) {
      for (const later of reads.slice(1)) {
        assert.equal(
          (later as Record<string, unknown>)[key],
          (first as Record<string, unknown>)[key],
          `${key} changed identity between renders`,
        );
      }
    }
  });
});
