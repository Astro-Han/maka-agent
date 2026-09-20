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

import { workspaceNameFromCwd } from './workspace-name.js';

import type { StoredMessage } from '@maka/core/session';

import type { DelegationFeedback as WorkHubDelegationFeedback } from '../slots.js';
export type { DelegationFeedback as WorkHubDelegationFeedback } from '../slots.js';
export type WorkHubDelegationState = WorkHubDelegationFeedback['state'];

export interface WorkHubLinkedWork {
  readonly id: string;
  readonly coordinationTurnId: string;
  readonly targetSessionId: string;
  readonly targetSessionName: string;
  readonly workspaceName?: string;
  readonly targetMessageId?: string;
  readonly targetTurnId?: string;
  readonly state?: WorkHubDelegationState;
  readonly resultPreview?: string;
}

/** Only this plugin's successful, durable tool receipts create work links. */
export function workHubLinkedWork(
  messages: readonly StoredMessage[],
  sessions: readonly { id: string; name: string; cwd?: string }[],
  fallbackName: string,
  projectSession: (hostSessionId: string) => string,
): WorkHubLinkedWork[] {
  const sessionById = new Map(sessions.map((session) => [session.id, session]));
  const taskCalls = new Set(
    messages.flatMap((message) =>
      message.type === 'tool_call' && message.toolName === 'workhub_tasks' ? [message.id] : [],
    ),
  );
  return messages.flatMap((message): WorkHubLinkedWork[] => {
    if (message.type !== 'tool_result' || message.isError || !taskCalls.has(message.toolUseId))
      return [];
    let value: unknown;
    if (message.content.kind === 'json') value = message.content.value;
    else if (message.content.kind === 'text') {
      try {
        value = JSON.parse(message.content.text);
      } catch {
        return [];
      }
    }
    const envelope = record(value);
    let id = envelope?.operationId;
    let result = record(envelope?.result);
    if (result?.kind === 'corrected') {
      id = result.replacementId;
      result = record(result.replacement);
    }
    if (typeof id !== 'string' || (result?.kind !== 'submitted' && result?.kind !== 'queued'))
      return [];
    const receipt = record(result.receipt);
    const invocation = record(receipt?.invocation);
    if (
      typeof invocation?.session_id !== 'string' ||
      typeof invocation.turn_id !== 'string' ||
      typeof receipt?.messageId !== 'string'
    )
      return [];
    const target = projectSession(invocation.session_id);
    const session = sessionById.get(target);
    return [
      {
        id,
        coordinationTurnId: message.turnId,
        targetSessionId: target,
        targetSessionName: session?.name ?? fallbackName,
        workspaceName: workspaceNameFromCwd(session?.cwd),
        targetMessageId: receipt.messageId,
        targetTurnId: invocation.turn_id,
        state: 'accepted',
      },
    ];
  });
}
function record(value: unknown): Record<string, unknown> | undefined {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined;
}

export function applyWorkHubDelegationFeedback(
  assignments: readonly WorkHubLinkedWork[],
  feedback: readonly WorkHubDelegationFeedback[],
): WorkHubLinkedWork[] {
  const byId = new Map(feedback.map((item) => [item.id, item]));
  return assignments.map((assignment) => {
    const item = byId.get(assignment.id);
    return item
      ? { ...assignment, state: item.state, resultPreview: item.resultPreview }
      : assignment;
  });
}
