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

import type { SessionEvent } from '@maka/core/events';
import type { SessionChangedEvent } from '@maka/core/session';
import type { ClientEventRequest, ClientProductEvent } from '@maka-agent/plugin-sdk/client';
import type { SessionObservationMessage } from '../shared/session-execution-projection.js';
import { releaseSessionObservation } from './session-observation-release.js';

interface EventConnection {
  current(): boolean;
  onRetire(listener: () => void): () => void;
  subscribe<T extends readonly unknown[]>(channel: string, listener: (...args: T) => void): () => void;
  observe(sessionId: string, observerId: string): Promise<{ kind: 'ready' | 'cancelled' }>;
  unobserve(observerId: string): Promise<unknown>;
}

/** Reuse the ordered product observation channel, without following replacement Hosts. */
export function subscribeClientEvents(
  connect: () => Promise<EventConnection>,
  request: ClientEventRequest,
  listener: (event: ClientProductEvent) => void,
  onError: (error: Error) => void,
): () => Promise<void> {
  if (request.kind !== 'session.changed' && request.kind !== 'session.event' && request.kind !== 'tool.activity')
    throw new Error('Unknown Client event subscription');
  if (request.kind !== 'session.changed' && (!request.sessionId || typeof request.sessionId !== 'string'))
    throw new Error('Client event subscription requires a canonical Session ID');
  const observerId = crypto.randomUUID();
  let stopped = false;
  let unsubscribe = () => {};
  let unsubscribeRetirement = () => {};
  let release: (() => Promise<void>) | undefined;
  let closing: Promise<void> | undefined;
  const close = (): Promise<void> => {
    stopped = true;
    unsubscribe();
    unsubscribeRetirement();
    return closing ??= release?.() ?? Promise.resolve();
  };
  const fail = (error: unknown) => {
    if (stopped) return;
    void close().catch(onError);
    onError(error instanceof Error ? error : new Error(String(error)));
  };
  void connect().then((connection) => {
    if (stopped) return;
    if (!connection.current()) throw new Error('Client event connection has retired');
    unsubscribeRetirement = connection.onRetire(() => { void close().catch(onError); });
    const deliver = (event: ClientProductEvent) => {
      if (stopped || !connection.current()) return;
      // Plugin exceptions cannot interrupt an ordered seed or other IPC listeners.
      try { listener(event); } catch (error) { onError(error instanceof Error ? error : new Error(String(error))); }
    };
    if (request.kind === 'session.changed') {
      unsubscribe = connection.subscribe<[SessionChangedEvent]>('sessions:changed', (event) => {
        deliver({ ...event, kind: 'session.changed' });
      });
      return;
    }
    const { sessionId } = request;
    const consume = (event: SessionEvent) => {
      const { id, turnId, ts, type, ...payload } = event;
      if (request.kind === 'session.event') {
        deliver({ kind: request.kind, sessionId, event: { id, turnId, ts, type, payload } });
      } else if (type === 'tool_start' || type === 'tool_output_delta' || type === 'tool_progress' ||
        type === 'tool_result_preview' || type === 'tool_result') {
        deliver({ kind: 'tool.activity', sessionId,
          event: { id, turnId, ts, type, payload: { ...payload, toolUseId: event.toolUseId } } });
      }
    };
    unsubscribe = connection.subscribe<[SessionEvent | SessionObservationMessage]>(`sessions:event:${sessionId}`, (event) => {
      if (stopped || !connection.current()) return;
      if (event.type === 'host_observation_seed') {
        if (event.observerIds.includes(observerId)) for (const item of event.events) consume(item);
      } else if (event.type === 'host_observation_error') fail(new Error(event.message));
      else if (event.type !== 'host_observation_pending' && event.type !== 'host_execution') consume(event);
    });
    const completion = Promise.resolve().then(() => stopped
      ? { kind: 'cancelled' as const }
      : connection.observe(sessionId, observerId));
    release = () => releaseSessionObservation(Promise.resolve({ completion }), () => connection.unobserve(observerId));
    void completion.then((result) => {
      if (result.kind === 'cancelled' && !stopped) fail(new Error('Client observation was cancelled'));
    }, fail);
  }).catch(fail);
  return close;
}
