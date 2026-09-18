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

import type { Json } from './host.js';

export interface Invocation {
  session_id: string;
  turn_id: string;
  run_id: string;
  invocation_id: string;
}
export type AttachmentLocation =
  | { kind: 'session_file'; sessionId: string; relativePath: string }
  | { kind: 'workspace_file'; relativePath: string }
  | { kind: 'external_file'; absolutePath: string }
  | { kind: 'session_context'; sessionId: string; refId: string };
export interface Attachment {
  kind: 'image' | 'pdf' | 'doc' | 'code' | 'other';
  name: string;
  mimeType: string;
  bytes: number;
  ref: AttachmentLocation;
}
export interface MessageContent {
  text: string;
  display_text?: string | null;
  attachments?: readonly Attachment[] | null;
  quotes?: readonly { text: string; label?: string; sourceTurnId?: string }[] | null;
  directory_references?: readonly { hostId: string; path: string }[] | null;
  inline_references?:
    | readonly {
        kind: 'skill' | 'workspace_file';
        value: string;
        label: string;
        start: number;
      }[]
    | null;
}
export interface ExecutionReceipt {
  invocation: Invocation;
  messageId: string;
  contentDigest: string;
}
export interface WorkspacePatch {
  artifactId: string;
  sessionId: string;
  turnId: string;
  bytes: number;
  baseCommit: string;
}
export type ExecutionOutcome =
  | { kind: 'completed' }
  | { kind: 'cancelled'; source: string }
  | { kind: 'failed'; class: string; message: string | null }
  | { kind: 'handoff_paused'; pause: Json }
  | { kind: 'context_compact_finished'; outcome: Json };
export interface ExecutionObservation {
  receipt: ExecutionReceipt;
  progress:
    | { state: 'pending' | 'running' | 'waiting_for_user' | 'paused' }
    | { state: 'ended'; outcome: ExecutionOutcome };
  /** Pass this exact fence to events/event for a consistent view. */
  throughSequence: number;
  /** Exact canonical outcome record; absent until execution has settled. */
  terminalEventId?: string;
  /** Stable current interaction-set or handoff identity; unrelated log writes do not change it. */
  attentionId?: string;
}
export interface LogEvent {
  sequence: number;
  event: {
    id: string;
    recorded_at: { secs_since_epoch: number; nanos_since_epoch: number };
    invocation: Invocation;
    /** Canonical, versioned by Host; validate fact kinds before interpreting payloads. */
    fact: Json;
  };
}
export interface Executions {
  /** Host grants access to the Entry's Session and children it creates. */
  submit(input: {
    operationId: string;
    sessionId: string;
    content: MessageContent;
    /** Applies to this execution without changing the Session default. */
    orchestrationMode?: 'default' | 'graph' | 'swarm';
  }): Promise<ExecutionReceipt>;
  createChild(input: {
    operationId: string;
    parentSessionId: string;
    name: string;
    /** May narrow the parent's current permission, never widen it. */
    permissionMode?: 'explore' | 'ask' | 'bypass';
    /** Intersected with the parent's ceiling, including future dynamic tools. */
    boundTools?: readonly string[];
    /** Appended to inherited instructions; the combined budget is 16 KiB UTF-8. */
    instructions?: string;
    /** Isolated Git workspaces are Host-owned and persist across child turns. */
    workspace?: 'inherit' | 'isolated_git';
    target?:
      | {
          kind: 'model';
          model: { connection_id: string; connection_slug: string; model: string };
          thinkingLevel?: 'off' | 'minimal' | 'low' | 'medium' | 'high' | 'xhigh' | 'max' | null;
        }
      | { kind: 'executor'; executorId: string };
  }): Promise<{ sessionId: string }>;
  /** Requires settled workspace writers; retries return the same immutable Artifact. */
  workspacePatch(operationId: string): Promise<WorkspacePatch | null>;
  query(operationId: string): Promise<ExecutionObservation>;
  cancel(operationId: string): Promise<ExecutionObservation>;
  events(input: {
    operationId: string;
    after: number;
    through: number;
  }): Promise<{ events: LogEvent[]; throughSequence: number; nextAfter: number | null }>;
  event(input: { operationId: string; eventId: string; through: number }): Promise<LogEvent | null>;
}
