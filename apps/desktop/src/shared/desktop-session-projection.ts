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

import type { DailyReviewSummary } from '@maka/core/daily-review';
import type { ApprovalPolicy } from '@maka/core/execution-permissions';
import type {
  AttachmentRef,
  MessageContent,
  SessionEvent,
  StorageRef,
  ToolResultContent,
} from '@maka/core/events';
import type { SessionSummary, StoredMessage, TurnRecord } from '@maka/core/session';
import type { UsageStats } from '@maka/core/settings';
import type { RuntimeHostProfileKind } from '@maka/runtime-host/profile-kind';
import { desktopSessionKey, parseDesktopSessionKey, type DesktopHostRef } from './runtime-host-identity.js';

export interface DesktopSessionSummary extends SessionSummary {
  /** Shared read-only projections do not disclose execution authority. */
  readonly approvalPolicy: ApprovalPolicy | null;
  /** Client cache is readable history, not evidence of current Host execution. */
  readonly localState?: 'pending' | 'cached';
  readonly localCreatedAt?: number;
  /** Monotonic revision of the authoritative Runtime Host Session. */
  readonly revision: number;
  /** Present on authoritative Session Catalog snapshots, absent from command responses. */
  readonly activityAt?: number;
  readonly runtimeHostId: string;
  readonly profileId: string;
  readonly profileName: string;
  readonly profileKind: RuntimeHostProfileKind;
  /** Present only for Session projections granted to a Guest principal. */
  readonly shared?: true;
}

export type DesktopSessionSummaryInput = SessionSummary & { readonly approvalPolicy: ApprovalPolicy | null; readonly revision: number; readonly localState?: 'pending' | 'cached'; readonly localCreatedAt?: number };

export function compareDesktopSessionCatalogSummaries(left: DesktopSessionSummary, right: DesktopSessionSummary): number {
  const leftActivity = left.localState === 'pending' ? left.localCreatedAt : left.activityAt;
  const rightActivity = right.localState === 'pending' ? right.localCreatedAt : right.activityAt;
  if (leftActivity === undefined || rightActivity === undefined) {
    throw new Error('Runtime Host Session Catalog activity is unavailable');
  }
  return rightActivity - leftActivity || left.id.localeCompare(right.id);
}

export type DesktopSessionUpdateFailureCode =
  | 'session_busy'
  | 'operation_conflict'
  | 'operation_unavailable'
  | 'not_found';

export type DesktopSessionUpdateResult<Session> =
  | { readonly ok: true; readonly session: Session }
  | { readonly ok: false; readonly code: DesktopSessionUpdateFailureCode };

export interface DesktopSessionHost extends DesktopHostRef {
  readonly profileId: string;
  readonly profileName: string;
  readonly profileKind: RuntimeHostProfileKind;
}

function projectSessionId(host: DesktopHostRef, sessionId: string): string {
  return desktopSessionKey({ hostId: host.hostId, sessionId });
}

export function projectDesktopStorageRef(
  host: DesktopHostRef,
  ref: StorageRef,
): StorageRef {
  return ref.kind === 'session_file'
    ? { ...ref, sessionId: projectSessionId(host, ref.sessionId) }
    : ref;
}

export function projectDesktopAttachmentRefs(
  host: DesktopHostRef,
  attachments: readonly AttachmentRef[],
): AttachmentRef[] {
  return attachments.map((attachment) => ({
    ...attachment,
    ref: projectDesktopStorageRef(host, attachment.ref),
  }));
}

/** Convert only references belonging to this exact Host and canonical Session. */
export function hostAttachmentRefs(
  session: { scope: DesktopHostRef; sessionId: string },
  attachments: readonly AttachmentRef[],
): AttachmentRef[] {
  return attachments.map((attachment) => {
    if (attachment.ref.kind !== 'session_file') return attachment;
    const owner = parseDesktopSessionKey(attachment.ref.sessionId);
    if (owner.hostId !== session.scope.hostId || owner.sessionId !== session.sessionId) {
      throw new Error('Retained attachment belongs to another Host or Session');
    }
    return { ...attachment, ref: { ...attachment.ref, sessionId: owner.sessionId } };
  });
}

function projectMessageContent<T extends MessageContent>(
  host: DesktopHostRef,
  content: T,
): T {
  if (!content.attachments?.some((attachment) => attachment.ref.kind === 'session_file')) {
    return content;
  }
  return {
    ...content,
    attachments: projectDesktopAttachmentRefs(host, content.attachments),
  };
}

export function projectDesktopToolResultContent(
  host: DesktopHostRef,
  content: ToolResultContent,
): ToolResultContent {
  switch (content.kind) {
    case 'image':
      return { ...content, ref: projectDesktopStorageRef(host, content.ref) };
    case 'subagent':
      return content.childSessionId
        ? { ...content, childSessionId: projectSessionId(host, content.childSessionId) }
        : content;
    case 'agent_swarm':
      return {
        ...content,
        items: content.items.map((item) =>
          item.childSessionId
            ? { ...item, childSessionId: projectSessionId(host, item.childSessionId) }
            : item,
        ),
      };
    default:
      return content;
  }
}

export function projectDesktopStoredMessage(
  host: DesktopHostRef,
  message: StoredMessage,
): StoredMessage {
  switch (message.type) {
    case 'user':
      return message.attachments?.some((attachment) => attachment.ref.kind === 'session_file')
        ? { ...message, attachments: projectDesktopAttachmentRefs(host, message.attachments) }
        : message;
    case 'tool_result':
      return { ...message, content: projectDesktopToolResultContent(host, message.content) };
    case 'turn_state':
      return message.parentSessionId
        ? { ...message, parentSessionId: projectSessionId(host, message.parentSessionId) }
        : message;
    default:
      return message;
  }
}

export function projectDesktopSessionEvent(
  host: DesktopHostRef,
  event: SessionEvent,
): SessionEvent {
  switch (event.type) {
    case 'tool_output_delta':
      return { ...event, sessionId: projectSessionId(host, event.sessionId) };
    case 'tool_result_preview':
      return {
        ...event,
        content: {
          ...event.content,
          childSessionId: projectSessionId(host, event.content.childSessionId),
        },
      };
    case 'steering_message':
      return { ...event, content: projectMessageContent(host, event.content) };
    case 'queue_update':
      return {
        ...event,
        ...(event.steeringEntries
          ? {
              steeringEntries: event.steeringEntries.map((entry) => ({
                ...entry,
                content: projectMessageContent(host, entry.content),
              })),
            }
          : {}),
        ...(event.followupEntries
          ? {
              followupEntries: event.followupEntries.map((entry) => ({
                ...entry,
                content: projectMessageContent(host, entry.content),
              })),
            }
          : {}),
      };
    default:
      return event;
  }
}

export function projectDesktopTurnRecord(
  host: DesktopHostRef,
  turn: TurnRecord,
): TurnRecord {
  return turn.parentSessionId
    ? { ...turn, parentSessionId: projectSessionId(host, turn.parentSessionId) }
    : turn;
}

export function projectDesktopSessionSummary(
  host: DesktopSessionHost,
  session: DesktopSessionSummaryInput,
): DesktopSessionSummary {
  return {
    ...session,
    id: projectSessionId(host, session.id),
    ...(session.parentSessionId === undefined
      ? {}
      : { parentSessionId: projectSessionId(host, session.parentSessionId) }),
    ...(session.revisionRootSessionId === undefined
      ? {}
      : { revisionRootSessionId: projectSessionId(host, session.revisionRootSessionId) }),
    ...(session.revisionParentSessionId === undefined
      ? {}
      : { revisionParentSessionId: projectSessionId(host, session.revisionParentSessionId) }),
    ...(session.subagent === undefined
      ? {}
      : {
          subagent: {
            ...session.subagent,
            parentSessionId: projectSessionId(host, session.subagent.parentSessionId),
          },
        }),
    ...(session.subagentParent === undefined
      ? {}
      : {
          subagentParent: {
            ...session.subagentParent,
            parentSessionId: projectSessionId(host, session.subagentParent.parentSessionId),
          },
        }),
    runtimeHostId: host.hostId,
    profileId: host.profileId,
    profileName: host.profileName,
    profileKind: host.profileKind,
  };
}

export function projectDesktopDailyReviewSummary(
  host: DesktopHostRef,
  summary: DailyReviewSummary,
): DailyReviewSummary {
  return {
    ...summary,
    sessions: summary.sessions.map((session) => ({
      ...session,
      id: projectSessionId(host, session.id),
    })),
  };
}

export function projectDesktopUsageActivity(
  host: DesktopHostRef,
  logs: UsageStats['logs'],
): UsageStats['logs'] {
  return logs.map((log) => ({
    ...log,
    ...(log.sessionId === undefined
      ? {}
      : { sessionId: projectSessionId(host, log.sessionId) }),
  }));
}

export function projectDesktopUsageStats(host: DesktopHostRef, stats: UsageStats): UsageStats {
  return { ...stats, logs: projectDesktopUsageActivity(host, stats.logs) };
}
