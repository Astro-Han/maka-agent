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

import type { StoredMessage } from '@maka/core/session';
import { createAppShellRevisionActions, type TurnRevisionDraft } from '../../renderer/app-shell-revision-actions.js';

function userMessage(turnId: string, text: string, extra: Record<string, unknown> = {}): StoredMessage {
  return {
    id: `msg-${turnId}`,
    type: 'user',
    turnId,
    ts: 1,
    text,
    ...extra,
  } as StoredMessage;
}

function createActions(input: { messages: StoredMessage[] }) {
  const drafts: unknown[] = [];
  let composerText = '';
  const revisionDraftRef: { current: unknown } = { current: null };
  const actions = createAppShellRevisionActions({
    uiLocale: 'en' as never,
    activeIdRef: { current: 'session-1' },
    composerRef: {
      current: {
        getText: () => composerText,
        setText: (text: string) => {
          composerText = text;
        },
        focus: () => {},
        setDraft: (_sessionId: string, text: string) => {
          composerText = text;
        },
        clearDraft: () => {},
      } as never,
    },
    messages: input.messages,
    hasPendingAttachments: () => false,
    openSessionInChat: () => {},
    refreshMessages: async () => true,
    refreshSessions: async () => [],
    setMessages: () => {},
    commitRevisionDraft: (draft: unknown) => {
      revisionDraftRef.current = draft;
      drafts.push(draft);
    },
    revisionDraftRef,
    toastApi: {
      info: () => {},
      error: () => {},
    },
  } as never);
  return Object.assign(actions, { drafts, composerState: { get text(): string { return composerText; } } });
}

describe('app-shell revision actions with structured context (#5109)', () => {
  it('keeps editing allowed when only earlier turns carry attachments', () => {
    const h = createActions({
      messages: [
        userMessage('turn-1', 'with image', {
          attachments: [
            {
              kind: 'image',
              name: 'chart.png',
              mimeType: 'image/png',
              bytes: 10,
              ref: { kind: 'session_file', sessionId: 'session-1', relativePath: 'a.png' },
            },
          ],
        }),
        userMessage('turn-2', 'plain follow-up'),
      ],
    });

    h.beginEditUserMessage('turn-2');

    assert.ok(h.drafts.at(-1), 'a retained historical attachment must not block the edit');
    assert.equal(h.composerState.text, 'plain follow-up');
  });

  it('rejects a source message that itself carries attachments', () => {
    const h = createActions({
      messages: [
        userMessage('turn-1', 'with image', {
          attachments: [
            {
              kind: 'image',
              name: 'chart.png',
              mimeType: 'image/png',
              bytes: 10,
              ref: { kind: 'session_file', sessionId: 'session-1', relativePath: 'a.png' },
            },
          ],
        }),
      ],
    });

    h.beginEditUserMessage('turn-1');

    assert.equal(h.drafts.at(-1), undefined, 'attachment-bearing sources stay explicitly rejected');
  });
});

it('preserves prepared revision text on retry, restores it on cancel, and settles only its own draft', async (t) => {
  const previousText = 'previous unsent draft /skill:project-only ';
  const editedText = 'edited with skill /skill:workspace-only ';
  const prepared: TurnRevisionDraft = {
    sourceSessionId: 'revision-source', sourceTurnId: 'turn', copyId: 'revision-child',
    copyPhase: 'started', draftSessionId: 'revision-child', originalText: 'original',
    previousComposerText: previousText,
  };
  const revisionDraftRef: { current: TurnRevisionDraft | null } = { current: prepared };
  const activeIdRef = { current: prepared.draftSessionId };
  const drafts = new Map([[prepared.sourceSessionId, previousText], [prepared.draftSessionId, editedText]]);
  const abandoned: string[] = [];
  const originalWindow = Object.getOwnPropertyDescriptor(globalThis, 'window');
  Object.defineProperty(globalThis, 'window', {
    configurable: true,
    value: { maka: { sessions: {
      reviseBeforeTurn() { assert.fail('A prepared revision must reuse its child on retry'); },
      async abandonSessionCopy(source: string, copyId: string) {
        assert.equal(source, prepared.sourceSessionId);
        abandoned.push(copyId);
      },
    } } },
  });
  t.after(() => {
    if (originalWindow) Object.defineProperty(globalThis, 'window', originalWindow);
    else Reflect.deleteProperty(globalThis, 'window');
  });
  const actions = createAppShellRevisionActions({
    uiLocale: 'en', activeIdRef, revisionDraftRef,
    captureSelection: () => { const id = activeIdRef.current; return () => id === activeIdRef.current; },
    composerRef: { current: {
      getText: () => drafts.get(activeIdRef.current) ?? '',
      setText: (text) => { drafts.set(activeIdRef.current, text); },
      appendText: (text) => { drafts.set(activeIdRef.current, (drafts.get(activeIdRef.current) ?? '') + text); },
      getDraft: (id) => drafts.get(id) ?? '',
      setDraft: (id, text) => { drafts.set(id, text); },
      clearDraft: (id) => { drafts.delete(id); },
      focus() {}, openModelPicker() {},
    } },
    messages: [], hasPendingAttachments: () => false,
    openSessionInChat: (id) => { activeIdRef.current = id; },
    refreshSessions: async () => [], setMessages() {},
    commitRevisionDraft: (draft) => { revisionDraftRef.current = draft; },
    toastApi: { info() {}, error() { assert.fail('Unexpected revision error'); } },
  });
  assert.equal(await actions.prepareRevisionSend(editedText), true);
  assert.equal(drafts.get(prepared.draftSessionId), editedText);
  await actions.cancelRevisionDraft();
  assert.deepEqual(abandoned, [prepared.copyId]);
  assert.equal(activeIdRef.current, prepared.sourceSessionId);
  assert.equal(drafts.get(prepared.sourceSessionId), previousText);
  assert.equal(drafts.has(prepared.draftSessionId), false);
  assert.equal(revisionDraftRef.current, null);

  const newer = { ...prepared, copyId: 'next-copy', draftSessionId: 'next-child' };
  revisionDraftRef.current = newer;
  drafts.set(newer.draftSessionId, editedText);
  actions.completeRevisionSend(prepared);
  assert.equal(revisionDraftRef.current, newer, 'A late admission must not clear a newer edit');
  assert.equal(drafts.get(newer.sourceSessionId), previousText);
  actions.completeRevisionSend(newer);
  assert.equal(revisionDraftRef.current, null);
  assert.equal(drafts.size, 0, 'Confirmed send clears both source and child drafts');
});
