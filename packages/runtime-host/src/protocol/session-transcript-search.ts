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

import {
  requireCount,
  requireEntityId,
  requireExactRecord,
  requireId,
  requireUtf8String,
} from './codec.js';
import { invalidProtocolFrame } from './errors.js';
import { defineOperation } from './operation-spec.js';

export interface TranscriptSearchInput {
  readonly subscriptionId: string;
  readonly throughSequence: number | null;
  readonly query: string;
  readonly includeInternal: boolean;
  readonly cursor: string | null;
  readonly maxMatches: number;
}
export interface TranscriptSearchResult {
  readonly sessionId: string;
  readonly throughSequence: number | null;
  readonly matches: readonly { readonly sequence: number; readonly preview: string }[];
  readonly nextCursor: string | null;
}
export function decodeTranscriptSearchInput(value: unknown): TranscriptSearchInput {
  const v = requireExactRecord(value, 'transcript search input', [
    'subscriptionId',
    'throughSequence',
    'query',
    'includeInternal',
    'cursor',
    'maxMatches',
  ]);
  const maxMatches = requireCount(v.maxMatches, 'maxMatches');
  if (maxMatches < 1 || maxMatches > 64 || typeof v.includeInternal !== 'boolean') {
    throw invalidProtocolFrame('Invalid transcript search limit or visibility');
  }
  return {
    subscriptionId: requireId(v.subscriptionId, 'subscriptionId'),
    throughSequence:
      v.throughSequence === null ? null : requireCount(v.throughSequence, 'throughSequence'),
    query: requireUtf8String(v.query, 'query', 512),
    includeInternal: v.includeInternal,
    cursor: v.cursor === null ? null : requireUtf8String(v.cursor, 'cursor', 1024),
    maxMatches,
  };
}
export function decodeTranscriptSearchResult(value: unknown): TranscriptSearchResult {
  const v = requireExactRecord(value, 'transcript search result', [
    'sessionId',
    'throughSequence',
    'matches',
    'nextCursor',
  ]);
  const throughSequence =
    v.throughSequence === null ? null : requireCount(v.throughSequence, 'throughSequence');
  if (!Array.isArray(v.matches) || v.matches.length > 64)
    throw invalidProtocolFrame('Invalid search matches');
  let previous = -1;
  const matches = v.matches.map((value) => {
    const row = requireExactRecord(value, 'search match', ['sequence', 'preview']);
    const sequence = requireCount(row.sequence, 'sequence');
    if (throughSequence === null || sequence > throughSequence || sequence <= previous) {
      throw invalidProtocolFrame('Invalid search match order');
    }
    previous = sequence;
    return { sequence, preview: requireUtf8String(row.preview, 'preview', 384) };
  });
  const nextCursor =
    v.nextCursor === null ? null : requireUtf8String(v.nextCursor, 'nextCursor', 1024);
  if (throughSequence === null && nextCursor !== null)
    throw invalidProtocolFrame('Empty history cannot continue');
  return {
    sessionId: requireEntityId(v.sessionId, 'sessionId'),
    throughSequence,
    matches,
    nextCursor,
  };
}
export const SESSION_TRANSCRIPT_SEARCH_OPERATION_SPECS = {
  'session.transcript.search': defineOperation({
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
    decodeInput: decodeTranscriptSearchInput,
    decodeOutput: decodeTranscriptSearchResult,
    assertOutputForInput(input: TranscriptSearchInput, output: TranscriptSearchResult): void {
      if (
        output.throughSequence !== input.throughSequence ||
        output.matches.length > input.maxMatches ||
        (output.nextCursor !== null && output.nextCursor === input.cursor)
      ) {
        throw invalidProtocolFrame('Transcript search response does not match request');
      }
    },
  }),
} as const;
