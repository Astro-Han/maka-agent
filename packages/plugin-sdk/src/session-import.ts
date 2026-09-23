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

import type { Executions } from './execution.js';

export interface SessionImportSource {
  adapter: string;
  sessionId: string;
}
export type SessionImportContent =
  | { kind: 'user'; text: string }
  | { kind: 'assistant'; text: string; model?: string | null; thinking?: string | null }
  | { kind: 'tool_call'; callId: string; name: string; input?: unknown }
  | { kind: 'tool_result'; callId: string; output: unknown; isError: boolean }
  | { kind: 'note'; text: string };

export interface SessionImportRecord {
  sourceMessageId: string;
  sourceTurnId: string;
  timestamp?: number | null;
  content: SessionImportContent;
}

/** Records are historical evidence, never executable calls or provider usage.
 * Import bytes must fit the runtime history budget, leaving space to continue.
 */
export type SessionImportCommand =
  | { action: 'begin'; root: Parameters<Executions['createRoot']>[0]; source: SessionImportSource }
  | {
      action: 'append';
      operationId: string;
      position: number;
      records: readonly SessionImportRecord[];
    }
  | { action: 'publish'; operationId: string; records: number }
  | { action: 'inspect' | 'abandon'; operationId: string };

export interface SessionImportReceipt {
  sessionId: string;
  state: 'collecting' | 'published' | 'abandoned';
  records: number;
  bytes: number;
}
