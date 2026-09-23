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
import { it, type TestContext } from 'node:test';
import type { MessageContent } from '@maka/core/events';
import type { SessionSourceMessage } from '@maka/runtime-host/protocol';
import { createAppShellRevisionActions, type TurnRevisionDraft } from '../../renderer/app-shell-revision-actions.js';

function harness(t: TestContext, sources: SessionSourceMessage[]) {
  const sourceId: string = crypto.randomUUID();
  const activeIdRef = { current: sourceId };
  const revisionDraftRef: { current: TurnRevisionDraft | null } = { current: null };
  const drafts = new Map([[sourceId, 'my unrelated unsent work']]);
  const restored: Array<{ id: string; content: MessageContent }> = [];
  const abandoned: string[] = [];
  const errors: string[] = [];
  let gate: Promise<void> = Promise.resolve();
  const previous = Object.getOwnPropertyDescriptor(globalThis, 'window');
  Object.defineProperty(globalThis, 'window', { configurable: true, value: { maka: { sessions: {
    async reviseBeforeTurn(_id: string, { copyId }: { copyId: string }) {
      await gate;
      return { id: copyId };
    },
    async readTurnSources(id: string) {
      return sources.map((source) => ({
        ...source,
        content: { ...source.content, attachments: source.content.attachments?.map((attachment) => ({
          ...attachment, ref: { kind: 'session_file', sessionId: id, relativePath: 'image.png' },
        })) },
      }));
    },
    async abandonSessionCopy(_source: string, id: string) { abandoned.push(id); },
  } } } });
  t.after(() => {
    if (previous) Object.defineProperty(globalThis, 'window', previous);
    else Reflect.deleteProperty(globalThis, 'window');
  });
  function actions() {
    return createAppShellRevisionActions({
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
      hasPendingContext: () => false,
      restoreContext: (session, content) => { restored.push({ id: session.id, content }); },
      openSessionInChat: (id) => { activeIdRef.current = id; },
      refreshSessions: async () => [],
      commitRevisionDraft: (draft) => { revisionDraftRef.current = draft; },
      toastApi: { info() {}, error(_title, description) { errors.push(description ?? 'error'); } },
    });
  }
  return { actions, sourceId, activeIdRef, revisionDraftRef, drafts, restored, abandoned, errors,
    holdCreation() { let resolve!: () => void; gate = new Promise<void>((done) => { resolve = done; }); return resolve; },
  };
}

it('edits the complete original batch with target-owned context and keeps the source draft', async (t) => {
  const sources: SessionSourceMessage[] = [{
    messageId: 'first', content: {
      text: 'raw request', displayText: 'a different display projection',
      attachments: [{ kind: 'image', name: 'image.png', mimeType: 'image/png', bytes: 10,
        ref: { kind: 'session_file', sessionId: 'source', relativePath: 'image.png' } }],
      quotes: [{ text: 'q'.repeat(40_000), source: {
        sessionId: 'quote-origin', sessionName: 'Evidence', capturedAt: 1, truncated: false,
      } }],
      directoryReferences: [{ hostId: 'host', path: '/workspace' }],
    }, inputSelections: { 'custom-plugin': ['first'] },
  }, {
    messageId: 'second', content: { text: 'follow-up @file.ts',
      inlineReferences: [{ kind: 'workspace_file', value: '@file.ts', label: 'file.ts', start: 10 }] },
    inputSelections: { 'custom-plugin': ['second'] },
  }];
  const h = harness(t, sources);
  await h.actions().beginEditUserMessage('turn');
  assert.deepEqual(h.errors, []);
  const draft = h.revisionDraftRef.current;
  assert.ok(draft);
  assert.notEqual(draft.draftSessionId, h.sourceId);
  assert.equal(h.drafts.get(draft.draftSessionId), 'raw request\n\nfollow-up @file.ts');
  assert.deepEqual(draft.inputSelections, { 'custom-plugin': ['first', 'second'] });
  const attachment = h.restored[0]?.content.attachments?.[0];
  assert.equal(attachment?.ref.kind, 'session_file');
  assert.ok(attachment?.ref.kind === 'session_file');
  assert.equal(attachment.ref.sessionId, draft.draftSessionId);
  assert.deepEqual(h.restored[0]?.content.quotes, sources[0]?.content.quotes);
  assert.deepEqual(h.restored[0]?.content.directoryReferences, sources[0]?.content.directoryReferences);
  assert.equal(h.restored[0]?.content.inlineReferences?.length, 1);

  await h.actions().cancelRevisionDraft();
  assert.equal(h.activeIdRef.current, h.sourceId);
  assert.equal(h.drafts.get(h.sourceId), 'my unrelated unsent work');
  assert.equal(h.drafts.has(draft.draftSessionId), false);
  assert.deepEqual(h.abandoned, [draft.copyId]);
  await h.actions().beginEditUserMessage('turn');
  const newer = h.revisionDraftRef.current;
  assert.ok(newer);
  h.actions().completeRevisionSend(draft);
  assert.equal(h.revisionDraftRef.current, newer, 'Late admission cannot clear a newer edit');
  h.drafts.set(newer.draftSessionId, 'typed while awaiting admission');
  h.actions().completeRevisionSend(newer);
  assert.equal(h.revisionDraftRef.current, null);
  assert.equal(h.drafts.get(newer.draftSessionId), 'typed while awaiting admission');
  assert.equal(h.drafts.get(h.sourceId), 'my unrelated unsent work');
});

it('does not overwrite new typing or restore context after cancellation during creation', async (t) => {
  const h = harness(t, [{ messageId: 'source', content: { text: 'original' } }]);
  let release = h.holdCreation();
  const preparation = h.actions().beginEditUserMessage('turn');
  h.drafts.set(h.sourceId, 'new typing during copy');
  release();
  await preparation;
  assert.equal(h.drafts.get(h.sourceId), 'new typing during copy');
  assert.equal(h.restored.length, 0);
  assert.equal(h.revisionDraftRef.current, null);
  assert.equal(h.abandoned.length, 1);

  release = h.holdCreation();
  const cancelled = h.actions().beginEditUserMessage('turn');
  await h.actions().cancelRevisionDraft();
  release();
  await cancelled;
  assert.equal(h.drafts.get(h.sourceId), 'new typing during copy');
  assert.equal(h.restored.length, 0);
  assert.equal(h.revisionDraftRef.current, null);
  assert.equal(h.abandoned.length, 2);
  assert.deepEqual(h.errors, []);
});
