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

import type { QuoteRef, SessionQuoteSource } from './events.js';
import type { StoredMessage } from './session.js';
import { userFacingText } from './session.js';

const encoder = new TextEncoder();
export type SessionQuote = QuoteRef & { source: SessionQuoteSource };

/** Bound both UTF-16 and UTF-8 without splitting a code point. */
export function boundReferenceText(text: string, maxChars: number, maxBytes = 32_000): string {
  let chars = 0;
  let bytes = 0;
  for (const point of text) {
    const size = encoder.encode(point).length;
    if (chars + point.length > maxChars || bytes + size > maxBytes) break;
    chars += point.length;
    bytes += size;
  }
  return text.slice(0, chars);
}

/** A committed text-only tail, not an execution history or a live Session link. */
export function createSessionQuote(
  messages: readonly StoredMessage[],
  source: { sessionId: string; sessionName: string; capturedAt: number },
  hasEarlier: boolean,
): SessionQuote {
  const selected: string[] = [];
  let remainingChars = 12_000;
  let remainingBytes = 32_000;
  let truncated = hasEarlier;
  for (let index = messages.length - 1; index >= 0; index--) {
    const message = messages[index]!;
    if (message.type !== 'user' && message.type !== 'assistant') continue;
    const text = (message.type === 'user' ? userFacingText(message) : message.text).trim();
    if (!text) continue;
    if (selected.length === 24) {
      truncated = true;
      break;
    }
    const prefix = message.type === 'user' ? 'User: ' : 'Assistant: ';
    const separator = selected.length ? 2 : 0;
    const content = boundReferenceText(
      text,
      Math.max(0, remainingChars - prefix.length - separator),
      Math.max(0, remainingBytes - prefix.length - separator),
    );
    if (content.length !== text.length) {
      truncated = true;
      // Keep one partial newest message, never a fragment of an older message.
      if (!selected.length && content) selected.push(prefix + content);
      break;
    }
    const line = prefix + text;
    selected.push(line);
    remainingChars -= line.length + separator;
    remainingBytes -= encoder.encode(line).length + separator;
  }
  const text = selected.reverse().join('\n\n');
  if (!text) throw new Error('Session has no committed user or assistant text');
  return {
    text,
    source: {
      ...source,
      sessionName: boundReferenceText(source.sessionName || 'Untitled', 200),
      truncated,
    },
  };
}
