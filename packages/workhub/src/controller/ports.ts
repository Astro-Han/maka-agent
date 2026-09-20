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
  Configured,
  ExecutionTarget,
  ExecutionReceipt,
  ExecutionObservation,
} from '@maka-agent/plugin-sdk/host';
import type { ChatModelChoice } from '@maka/core/chat-model-choice';
import type { StoredMessage, SessionSummary } from '@maka/core/session';
import type {
  SessionEvent,
  AttachmentRef,
  MessageQueuePlacement,
  ActiveInteractionRequestEvent,
} from '@maka/core/events';
import type { TurnSnapshot } from '@maka/runtime-host/protocol';
import type { SessionExecutionProjection } from '@maka/ui/session-execution';
import type { InteractionFormResponse } from '@maka/core/interaction';
import type { UserQuestionResponse } from '@maka/core/user-question';

/** Retries retain the original operation identity and payload across Host restarts. */
export interface WorkHubAnswerInput {
  readonly operationId: string;
  readonly text: string;
  readonly attachments?: AttachmentRef[];
}
export type WorkHubAnswerResult =
  | {
      readonly kind: 'admitted';
      readonly receipt: ExecutionReceipt;
      readonly progress?: ExecutionObservation['progress'];
    }
  | { readonly kind: 'unknown' };

export interface WorkHubTranscriptSnapshot {
  readonly messages: readonly StoredMessage[];
  readonly hasOlder: boolean;
  readonly ready: boolean;
}
export interface WorkHubTranscript {
  observationChanged(phase: 'pending' | 'ready'): void;
  loadEarlier(): Promise<void>;
  close(): Promise<void>;
}

/** The conversation controller has no native-window, browser or local-file access. */
export type CoordinationCommands = Pick<
  CoordinationSessionServices,
  'answer' | 'cancelAnswer' | 'configureModel' | 'enqueueMessage'
>;
export type CoordinationSessionAdapter = Omit<
  CoordinationSessionServices,
  keyof CoordinationCommands
>;

export interface CoordinationSessionServices {
  subscribeAvailability(handler: () => void): () => void;
  getSession(sessionId: string): Promise<SessionSummary & { revision: number }>;
  subscribeSessions(handler: () => void): () => void;
  listSessions(): Promise<(SessionSummary & { revision: number })[]>;
  modelChoices(sessionId: string): Promise<ChatModelChoice[]>;
  listActiveInteractions(sessionId: string): Promise<ActiveInteractionRequestEvent[]>;
  subscribeActiveInteractions(
    handler: (event: { sessionId: string; interactions: ActiveInteractionRequestEvent[] }) => void,
  ): () => void;
  respondToUserForm(sessionId: string, response: InteractionFormResponse): Promise<void>;
  respondToUserQuestion(sessionId: string, response: UserQuestionResponse): Promise<void>;
  answer(sessionId: string, input: WorkHubAnswerInput): Promise<WorkHubAnswerResult>;
  cancelAnswer(sessionId: string, input: WorkHubAnswerInput): Promise<void>;
  enqueueMessage(
    sessionId: string,
    messageId: string,
    text: string,
    attachments: AttachmentRef[],
    placement: MessageQueuePlacement,
    expectedTurnId: string,
  ): Promise<'admitted' | 'unknown' | 'rejected'>;
  retractQueueEntry(sessionId: string, entryId: string): Promise<void>;
  promoteQueueEntry(sessionId: string, entryId: string): Promise<void>;
  updateQueueEntry(
    sessionId: string,
    entryId: string,
    expectedQueueRevision: number,
    text: string,
  ): Promise<void>;
  reorderQueueEntries(sessionId: string, entryIds: readonly string[]): Promise<void>;
  configureModel(
    sessionId: string,
    input: { expectedRevision: number; target: Extract<ExecutionTarget, { kind: 'model' }> },
  ): Promise<Configured>;
  observe(
    sessionId: string,
    handler: (event: SessionEvent) => void,
    onError: (error: unknown) => void,
    onPhase: (phase: 'pending' | 'ready') => void,
    onExecution?: (projection: SessionExecutionProjection<TurnSnapshot> | undefined) => void,
  ): () => void;
  openTranscript(
    sessionId: string,
    handler: (snapshot: WorkHubTranscriptSnapshot) => void,
    signal: AbortSignal,
    onError: (error: unknown) => void,
  ): Promise<WorkHubTranscript>;
  /** Only confirmed retractions; undefined means this exact Turn was no longer active. */
  stop(sessionId: string, turnId: string): Promise<readonly string[] | undefined>;
}
