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

import type { TurnOrchestration } from '@maka/core/orchestration';
import { z } from 'zod';
import {
  requireEncodedByteLimit,
  requireEntityId,
  requireExactRecord,
  requireShapedRecord,
} from './codec.js';
import { invalidProtocolFrame } from './errors.js';
import { defineOperation } from './operation-spec.js';
import {
  decodeInputSelections,
  decodeTurnOrchestration,
  type InputSelections,
  type MessageContent,
} from './turn.js';

// Read accepted data structurally. Native submission limits are not a history
// format: plugins can legitimately submit larger quotes and reference lists.
const count = z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER);
const sourceContent: z.ZodType<MessageContent> = z.strictObject({
  text: z.string(),
  displayText: z.string().optional(),
  attachments: z
    .array(
      z.strictObject({
        kind: z.enum(['image', 'pdf', 'doc', 'code', 'other']),
        name: z.string(),
        mimeType: z.string(),
        bytes: count,
        ref: z.discriminatedUnion('kind', [
          z.strictObject({
            kind: z.literal('session_file'),
            sessionId: z.string(),
            relativePath: z.string(),
          }),
          z.strictObject({
            kind: z.literal('session_context'),
            sessionId: z.string(),
            refId: z.string(),
          }),
          z.strictObject({ kind: z.literal('workspace_file'), relativePath: z.string() }),
          z.strictObject({ kind: z.literal('external_file'), absolutePath: z.string() }),
        ]),
      }),
    )
    .optional(),
  directoryReferences: z.array(z.strictObject({ hostId: z.string(), path: z.string() })).optional(),
  quotes: z
    .array(
      z.strictObject({
        text: z.string(),
        label: z.string().optional(),
        sourceTurnId: z.string().optional(),
        source: z
          .strictObject({
            sessionId: z.string(),
            sessionName: z.string(),
            capturedAt: z.number().min(0).max(8_640_000_000_000_000),
            truncated: z.boolean(),
          })
          .optional(),
      }),
    )
    .optional(),
  inlineReferences: z
    .array(
      z.strictObject({
        kind: z.enum(['skill', 'workspace_file']),
        value: z.string(),
        label: z.string(),
        start: count,
      }),
    )
    .optional(),
});

export interface SessionSourcesQueryInput {
  readonly sessionId: string;
  readonly turnId: string;
}

export interface SessionSourceMessage {
  readonly messageId: string;
  readonly content: MessageContent;
  readonly inputSelections?: InputSelections;
  readonly turnOrchestration?: TurnOrchestration;
}

export interface SessionSourcesQueryResult {
  readonly sessionId: string;
  readonly turnId: string;
  readonly messages: readonly SessionSourceMessage[];
}

export const SESSION_SOURCES_OPERATION_SPECS = {
  'session.sources.query': defineOperation<
    SessionSourcesQueryInput,
    SessionSourcesQueryResult,
    | 'host_not_ready'
    | 'host_draining'
    | 'operation_unavailable'
    | 'invalid_request'
    | 'not_found'
    | 'operation_conflict'
    | 'persistence_failed'
    | 'internal_failure'
  >({
    mode: 'query',
    availability: 'ready',
    errors: [
      'host_not_ready',
      'host_draining',
      'operation_unavailable',
      'invalid_request',
      'not_found',
      'operation_conflict',
      'persistence_failed',
      'internal_failure',
    ],
    decodeInput: (value) => {
      const input = requireExactRecord(value, 'Session source input', ['sessionId', 'turnId']);
      return {
        sessionId: requireEntityId(input.sessionId, 'sessionId'),
        turnId: requireEntityId(input.turnId, 'turnId'),
      };
    },
    decodeOutput: (value) => {
      requireEncodedByteLimit(value, 'Turn sources', 700 * 1024);
      const output = requireExactRecord(value, 'Session sources', [
        'sessionId',
        'turnId',
        'messages',
      ]);
      if (
        !Array.isArray(output.messages) ||
        output.messages.length === 0 ||
        output.messages.length > 64
      ) {
        throw invalidProtocolFrame('Invalid Turn source count');
      }
      const messages = output.messages.map((value): SessionSourceMessage => {
        const source = requireShapedRecord(
          value,
          'Source message',
          ['messageId', 'content'],
          ['inputSelections', 'turnOrchestration'],
        );
        const inputSelections = decodeInputSelections(source.inputSelections);
        const content = sourceContent.safeParse(source.content);
        if (!content.success) throw invalidProtocolFrame('Invalid source content');
        return {
          messageId: requireEntityId(source.messageId, 'messageId'),
          content: content.data,
          ...(Object.keys(inputSelections).length ? { inputSelections } : {}),
          ...(source.turnOrchestration === undefined
            ? {}
            : { turnOrchestration: decodeTurnOrchestration(source.turnOrchestration) }),
        };
      });
      if (new Set(messages.map((message) => message.messageId)).size !== messages.length) {
        throw invalidProtocolFrame('Duplicate Turn source');
      }
      const result = {
        sessionId: requireEntityId(output.sessionId, 'sessionId'),
        turnId: requireEntityId(output.turnId, 'turnId'),
        messages,
      };
      requireEncodedByteLimit(result, 'Turn sources', 700 * 1024);
      return result;
    },
    assertOutputForInput: (input, output) => {
      if (input.sessionId !== output.sessionId || input.turnId !== output.turnId) {
        throw invalidProtocolFrame('Turn source identity changed');
      }
    },
  }),
} as const;
