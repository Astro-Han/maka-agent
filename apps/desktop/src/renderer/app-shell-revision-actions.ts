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


import type { MessageContent } from '@maka/core/events';
import type { UiLocale } from '@maka/core/ui-locale';
import type { InputSelections, SessionSourceMessage } from '@maka/runtime-host/protocol';
import type { TurnOrchestration } from '@maka/core/orchestration';
import type { ComposerHandle } from '@maka/ui';
import type { DesktopSessionSummary } from '../preload/bridge-contract.js';
import type { SessionComposerDrafts } from './session-composer-drafts.js';
import { getDesktopConversationCopy } from './locales/conversation-copy.js';
import { localizedShellErrorMessage } from './locales/shell-copy.js';
import { mergeWorkspaceReferences } from './follow-up-submit-routing.js';

type RefBox<T> = { current: T };
type ToastApi = {
  info(title: string, description?: string): void;
  error(title: string, description?: string, diagnosticDetails?: string,
    diagnosticTarget?: { sessionId: string }): void;
};

/** The source composer remains untouched until the target owns all input. */
export type TurnRevisionDraft = {
  sourceSessionId: string;
  sourceTurnId: string;
  copyId: string;
  copyPhase: 'started' | 'abandoning';
  draftSessionId: string;
  previousComposerText: string;
  inputSelections?: InputSelections;
  turnOrchestration?: TurnOrchestration;
};

export interface AppShellRevisionActions {
  beginEditUserMessage(turnId: string): Promise<void>;
  completeRevisionSend(draft: TurnRevisionDraft): void;
  cancelRevisionDraft(): Promise<void>;
}

/** Copy first; edit target-owned original input, never the prepared transcript. */
export function createAppShellRevisionActions(deps: {
  uiLocale: UiLocale;
  activeIdRef: RefBox<string | undefined>;
  captureSelection(): () => boolean;
  composerRef: RefBox<ComposerHandle | null>;
  hasPendingContext(): boolean;
  drafts: SessionComposerDrafts;
  openSessionInChat(sessionId: string): void;
  refreshSessions(): Promise<DesktopSessionSummary[]>;
  commitRevisionDraft(draft: TurnRevisionDraft | null): void;
  revisionDraftRef: RefBox<TurnRevisionDraft | null>;
  toastApi: ToastApi;
}): AppShellRevisionActions {
  const { uiLocale, activeIdRef, captureSelection, composerRef,
    hasPendingContext, drafts, openSessionInChat, refreshSessions,
    commitRevisionDraft, revisionDraftRef, toastApi } = deps;
  const copy = getDesktopConversationCopy(uiLocale).actions;

  async function beginEditUserMessage(turnId: string): Promise<void> {
    const sessionId = activeIdRef.current;
    if (!sessionId) return;
    const existing = revisionDraftRef.current;
    if (existing && (existing.sourceSessionId !== existing.draftSessionId || existing.copyPhase === 'abandoning')) {
      if (existing.draftSessionId === sessionId && existing.sourceTurnId === turnId)
        composerRef.current?.focus();
      else toastApi.info(copy.revisionUnavailableTitle, copy.revisionAlreadyActive);
      return;
    }
    if (hasPendingContext()) {
      toastApi.info(copy.revisionUnavailableTitle, copy.revisionDraftAttachmentConflict);
      return;
    }
    const selectionIsCurrent = captureSelection();
    const savedRevision = drafts.snapshot(sessionId)?.revision;
    if (savedRevision && savedRevision.sourceTurnId !== turnId) {
      toastApi.info(copy.revisionUnavailableTitle, copy.revisionAlreadyActive);
      return;
    }
    const draft: TurnRevisionDraft = {
      sourceSessionId: sessionId, sourceTurnId: turnId,
      copyId: savedRevision?.copyId ?? crypto.randomUUID(),
      copyPhase: savedRevision?.phase === 'abandoning' ? 'abandoning' : 'started', draftSessionId: sessionId,
      previousComposerText: composerRef.current?.getText() ?? '',
    };
    commitRevisionDraft(draft);
    try {
      // A lost cancellation acknowledgement can only retry cancellation.
      if (draft.copyPhase === 'abandoning') {
        if (await abandonTurnRevisionCopyAttempt(draft)) {
          if (revisionDraftRef.current === draft) commitRevisionDraft(null);
        }
        return;
      }
      drafts.update(sessionId, { revision: {
        sourceSessionId: sessionId, sourceTurnId: turnId, copyId: draft.copyId, phase: 'preparing',
      } });
      await drafts.flush(sessionId);
      if (revisionDraftRef.current !== draft) return;
      const session = await window.maka.sessions.reviseBeforeTurn(sessionId, {
        sourceTurnId: draft.sourceTurnId, copyId: draft.copyId,
      });
      const sources = await window.maka.sessions.readTurnSources(session.id, draft.sourceTurnId);
      const input = revisionInput(sources);
      await refreshSessions();
      if (revisionDraftRef.current !== draft) return;
      if (!selectionIsCurrent() || activeIdRef.current !== sessionId ||
          composerRef.current?.getText() !== draft.previousComposerText || hasPendingContext()) {
        if (await abandonTurnRevisionCopyAttempt(draft)) {
          drafts.update(sessionId, { revision: undefined });
          await drafts.flush(sessionId);
        }
        if (revisionDraftRef.current === draft) commitRevisionDraft(null);
        return;
      }
      const snapshot = await drafts.seed(session.id, {
        text: input.content.text,
        attachments: (input.content.attachments ?? []).map((attachment) => ({ id: crypto.randomUUID(), kind: 'retained', attachment })),
        quotes: input.content.quotes, directoryReferences: input.content.directoryReferences,
        workspaceFileReferences: input.content.inlineReferences?.map(({ value, start }) => ({ value, start })),
        inputSelections: input.inputSelections, turnOrchestration: input.turnOrchestration,
        revision: { sourceSessionId: sessionId, sourceTurnId: turnId, copyId: draft.copyId, phase: 'ready' },
      });
      drafts.update(sessionId, { revision: undefined });
      await drafts.flush(sessionId);
      if (revisionDraftRef.current !== draft) { drafts.forget(session.id); return; }
      commitRevisionDraft(snapshot.revision ? {
        ...draft, draftSessionId: session.id,
        inputSelections: snapshot.inputSelections, turnOrchestration: snapshot.turnOrchestration,
      } : null);
      openSessionInChat(session.id);
      composerRef.current?.focus();
      toastApi.info(copy.revisionReadyTitle, copy.revisionReadyDescription);
    } catch (error) {
      // Keep the stable request identity: retry discovers any committed copy.
      if (revisionDraftRef.current !== draft) return;
      commitRevisionDraft(null);
      if (selectionIsCurrent())
        toastApi.error(copy.operationFailedTitle,
          localizedShellErrorMessage(error, copy.operationFailedFallback, uiLocale),
          undefined, { sessionId });
    }
  }

  async function cancelRevisionDraft(): Promise<void> {
    const draft = revisionDraftRef.current;
    if (!draft) return;
    const selectionIsCurrent = captureSelection();
    const abandoning = { ...draft, copyPhase: 'abandoning' as const };
    commitRevisionDraft(abandoning);
    drafts.update(draft.draftSessionId, { revision: {
      sourceSessionId: draft.sourceSessionId, sourceTurnId: draft.sourceTurnId, copyId: draft.copyId, phase: 'abandoning',
    } });
    try { await drafts.flush(draft.draftSessionId); }
    catch (error) { toastApi.error(copy.operationFailedTitle, String(error)); return; }
    if (!(await abandonTurnRevisionCopyAttempt(abandoning))) {
      if (revisionDraftRef.current === abandoning)
        toastApi.error(copy.operationFailedTitle, copy.operationFailedFallback);
      return;
    }
    if (revisionDraftRef.current !== abandoning) return;
    commitRevisionDraft(null);
    // Preparation never changed the source composer. Do not undo newer typing.
    if (draft.draftSessionId !== draft.sourceSessionId) {
      drafts.forget(draft.draftSessionId);
      composerRef.current?.clearDraft(draft.draftSessionId);
      if (selectionIsCurrent() && activeIdRef.current === draft.draftSessionId)
        openSessionInChat(draft.sourceSessionId);
    } else {
      drafts.update(draft.sourceSessionId, { revision: undefined });
      await drafts.flush(draft.sourceSessionId);
    }
    await refreshSessions().catch(() => []);
  }

  function completeRevisionSend(draft: TurnRevisionDraft): void {
    if (revisionDraftRef.current !== draft) return;
    // Composer owns exact submitted-text cleanup; newer typing and the
    // unrelated source draft are still the user's unsent work.
    commitRevisionDraft(null);
  }

  return { beginEditUserMessage, completeRevisionSend, cancelRevisionDraft };
}

/** Preserve the whole submitted batch without applying transcript truncation. */
function revisionInput(sources: readonly SessionSourceMessage[]): {
  content: MessageContent;
  inputSelections: InputSelections;
  turnOrchestration?: TurnOrchestration;
} {
  if (!sources.length) throw new Error('The original Turn input is unavailable');
  const selections = new Map<string, string[]>();
  const inlineReferences: NonNullable<MessageContent['inlineReferences']> = [];
  let textOffset = 0;
  let orchestration: TurnOrchestration | undefined;
  for (const source of sources) {
    for (const [name, values] of Object.entries(source.inputSelections ?? {})) {
      selections.set(name, [...new Set([...(selections.get(name) ?? []), ...values])]);
    }
    if (source.turnOrchestration) {
      if (orchestration && (orchestration.mode !== source.turnOrchestration.mode ||
          orchestration.source !== source.turnOrchestration.source))
        throw new Error('This Turn contains conflicting orchestration choices');
      orchestration = source.turnOrchestration;
    }
    for (const reference of mergeWorkspaceReferences(source.content.text, undefined,
      source.content.inlineReferences)) {
      inlineReferences.push({ ...reference, kind: 'workspace_file', label: reference.value,
        start: textOffset + reference.start });
    }
    textOffset += source.content.text.length + 2;
  }
  return {
    content: {
      text: sources.map(({ content }) => content.text).join('\n\n'),
      attachments: sources.flatMap(({ content }) => content.attachments ?? []),
      directoryReferences: sources.flatMap(({ content }) => content.directoryReferences ?? []),
      quotes: sources.flatMap(({ content }) => content.quotes ?? []),
      inlineReferences,
    },
    inputSelections: Object.fromEntries(selections),
    turnOrchestration: orchestration,
  };
}

export async function abandonTurnRevisionCopyAttempt(draft: TurnRevisionDraft): Promise<boolean> {
  try {
    await window.maka.sessions.abandonSessionCopy(draft.sourceSessionId, draft.copyId);
    return true;
  } catch {
    return false;
  }
}
