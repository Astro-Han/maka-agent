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

import type {
  AttachmentRef,
  DirectoryReference,
  InlineReference,
  QuoteRef,
} from '@maka/core/events';
import type { DesktopTranscriptBatchPayload } from '../preload/transcript-contract.js';
import type { InputSelections } from '@maka/runtime-host/protocol';
import type { TurnOrchestration } from '@maka/core/orchestration';

export type DesktopDraftAttachment =
  | { readonly id: string; readonly kind: 'retained'; readonly attachment: AttachmentRef }
  | { readonly id: string; readonly kind: 'file'; readonly name: string; readonly mimeType: string; readonly bytes: number };

/** Unsubmitted editor state, not a Host message or a second outbox. */
export interface DesktopComposerDraft {
  readonly text: string;
  readonly attachments: readonly DesktopDraftAttachment[];
  readonly directoryReferences?: readonly DirectoryReference[];
  readonly quotes?: readonly QuoteRef[];
  readonly workspaceFileReferences?: readonly Pick<InlineReference, 'value' | 'start'>[];
  readonly inputSelections?: InputSelections;
  readonly turnOrchestration?: TurnOrchestration;
  readonly revision?: {
    readonly sourceSessionId: string;
    readonly sourceTurnId: string;
    readonly copyId: string;
    readonly phase: 'preparing' | 'ready' | 'abandoning';
  };
}

export interface DesktopComposerDraftRecord {
  readonly authority: string;
  readonly version: number;
  readonly snapshot: DesktopComposerDraft | null;
  /** Retained after the corresponding outbox message has been observed and retired. */
  readonly submittedMessageId?: string;
}

export type DesktopLocalMessageState = 'saved' | 'sending' | 'accepted' | 'unknown' | 'failed';

/** Presentation only; the durable command and attachment bytes stay in Main. */
export interface DesktopLocalMessage {
  readonly sessionId: string;
  readonly messageId: string;
  readonly createdAt: number;
  readonly state: DesktopLocalMessageState;
  readonly canCancel: boolean;
  readonly placement: 'current_turn' | 'next_turn';
  readonly text: string;
  readonly attachments: readonly AttachmentRef[];
  readonly directoryReferences?: readonly DirectoryReference[];
  readonly quotes?: readonly QuoteRef[];
  readonly inlineReferences: readonly InlineReference[];
  readonly turnId?: string;
  readonly error?: string;
}

export interface DesktopCachedTranscript {
  readonly cachedAt: number;
  readonly batches: readonly DesktopTranscriptBatchPayload[];
}

export interface DesktopSessionLocalBridge {
  flushDrafts(): Promise<void>;
  onFlushDrafts(handler: () => Promise<void>): () => void;
  readDraft(sessionId: string): Promise<DesktopComposerDraftRecord>;
  readDraftFile(sessionId: string, id: string, authority: string): Promise<{ name: string; mimeType: string; base64: string }>;
  saveDraft(sessionId: string, expectedVersion: number, snapshot: DesktopComposerDraft | null,
    uploads: readonly { id: string; item: import('../preload/attachment-ingest-payload.js').IngestInput }[], authority: string): Promise<DesktopComposerDraftRecord>;
  listMessages(sessionId: string): Promise<readonly DesktopLocalMessage[]>;
  /** Only an intent that has never been dispatched can be cancelled locally. */
  cancelMessage(sessionId: string, messageId: string): Promise<void>;
  /** Reconcile the same immutable command; never turn an unknown outcome into a new execution. */
  reconcileMessage(sessionId: string, messageId: string): Promise<void>;
  readTranscript(sessionId: string): Promise<DesktopCachedTranscript | null>;
  subscribeChanges(handler: (sessionId: string) => void): () => void;
}
