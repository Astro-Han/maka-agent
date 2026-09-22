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

import type { SearchError, SearchResult } from '@maka/core/search';
import { normalizeSearchQuery, normalizeSearchLimit } from '@maka/core/search';
import { createPluginRemote } from '@maka/runtime-host/client';
import type { DesktopRuntimeHostClient } from './runtime-host-client.js';
import type { WebContents } from 'electron';
import { desktopSessionKey } from '../shared/runtime-host-identity.js';
import {
  type ReconnectableReadIpcMain,
} from './ipc-reconnect-policy.js';

interface RuntimeHostSearchIpcDeps {
  readonly ipcMain: ReconnectableReadIpcMain;
  readonly client: Pick<
    DesktopRuntimeHostClient,
    'request' | 'hostId' | 'queryRuntimePolicy'
  >;
}

export function registerRuntimeHostSearchIpc(
  deps: RuntimeHostSearchIpcDeps,
): void {
  const pending = new WeakMap<WebContents, Map<string, AbortController>>();
  // A search belongs to this candidate and its cancellation registry. Replaying
  // it on a replacement would revive work the renderer has already abandoned.
  deps.ipcMain.handle('search:thread', async (event, request: unknown, requestId?: unknown) => {
    if (requestId !== undefined && (typeof requestId !== 'string' || !requestId || requestId.length > 128)) {
      return { ok: false, reason: 'invalid_query', message: 'Invalid search request identity.' };
    }
    const controller = new AbortController();
    const remote = createPluginRemote(
      (input) => deps.client.request('plugin.remote', input, 40_000),
      { packageId: 'maka.recall' },
      controller.signal,
    );
    const release = () => {
      event.sender?.removeListener('destroyed', abort);
      event.sender?.removeListener('render-process-gone', abort);
      if (typeof requestId === 'string') {
        const requests = pending.get(event.sender);
        if (requests?.get(requestId) === controller) requests.delete(requestId);
      }
    };
    const abort = () => controller.abort();
    if (typeof requestId === 'string') {
      let requests = pending.get(event.sender);
      if (!requests) {
        requests = new Map();
        pending.set(event.sender, requests);
      }
      requests.get(requestId)?.abort();
      requests.set(requestId, controller);
    }
    controller.signal.addEventListener('abort', release, { once: true });
    event.sender?.once('destroyed', abort);
    // Crash recovery reloads the same WebContents without destroying it.
    event.sender?.once('render-process-gone', abort);
    try {
      if (!request || typeof request !== 'object' || Array.isArray(request) || !('source' in request) || request.source !== 'thread')
        return { ok: false, reason: 'invalid_query', message: 'Expected a conversation search.' };
      const input = request as Record<string, unknown>;
      const query = normalizeSearchQuery(input.query);
      const limit = normalizeSearchLimit(input.limit);
      if (!query.ok) return query;
      if (!limit.ok) return limit;
      if ((await deps.client.queryRuntimePolicy()).policy.privacy.incognitoActive)
        return { ok: false, reason: 'incognito_active', message: 'History search is disabled in incognito mode.' };
      const page = await remote.api.method('search')({ terms: [query.value], limit: limit.value });
      return projectResults(page, deps.client.hostId);
    } catch (error) {
      if (!controller.signal.aborted) throw error;
      return { ok: false, reason: 'aborted', message: 'History search was aborted.' };
    } finally {
      controller.signal.removeEventListener('abort', release);
      release();
      await remote.close();
    }
  });
  // Register after search so requests waiting for a candidate start before
  // their queued cancellations are delivered.
  deps.ipcMain.handle('search:thread:cancel', (event, requestId: unknown) => {
    if (typeof requestId === 'string') pending.get(event.sender)?.get(requestId)?.abort();
  });
}

function projectResults(value: unknown, hostId: string): SearchResult[] | SearchError {
  if (!value || typeof value !== 'object' || !('matches' in value) || !Array.isArray(value.matches) || !('complete' in value) || typeof value.complete !== 'boolean')
    throw new Error('Invalid Recall search page');
  const complete = value.complete;
  if (!complete && value.matches.length === 0)
    return { ok: false, reason: 'provider_error', message: 'History search could not read all eligible conversations.' };
  return value.matches.map((match: unknown): SearchResult => {
    if (!match || typeof match !== 'object' || !('kind' in match) || !('sessionId' in match) || typeof match.sessionId !== 'string' || !('title' in match) || typeof match.title !== 'string')
      throw new Error('Invalid Recall match');
    const sessionId = desktopSessionKey({hostId, sessionId: match.sessionId});
    if (match.kind === 'title') return {source: 'thread', title: match.title, target: {kind: 'thread', sessionId}, truncated: !complete};
    if (match.kind !== 'passage' || !('turnId' in match) || typeof match.turnId !== 'string' || !('messageId' in match) || typeof match.messageId !== 'string' || !('sequence' in match) || typeof match.sequence !== 'number' || !Number.isSafeInteger(match.sequence) || match.sequence < 0 || !('text' in match) || typeof match.text !== 'string' || !('truncated' in match) || typeof match.truncated !== 'boolean')
      throw new Error('Invalid Recall passage');
    return {source: 'thread', title: match.title, snippet: match.text,
      target: {kind: 'thread', sessionId, turnId: match.turnId, sequence: match.sequence, messageId: match.messageId},
      truncated: match.truncated || !complete};
  });
}
