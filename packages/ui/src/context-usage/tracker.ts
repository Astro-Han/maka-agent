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
import { createTraceRefreshCoalescer, type TraceRefreshCoalescer } from './refresh.js';

/** The gauge reads scalar facts, not the Host's complete diagnostics protocol. */
export type ContextUsageSnapshot =
  | { readonly status: 'unavailable' }
  | { readonly status: 'available'; readonly providerId: string; readonly modelId: string;
      readonly inputTokens?: number; readonly contextWindow?: number };

/** A token count describes only the model/provider route it was metered on. */
export interface LiveContextRoute {
  readonly model?: string;
  readonly providerType?: string;
}

/** The gauge's reading: the last settled request's prompt, and its ceiling. */
export interface LiveContextUsage {
  readonly usageTokens: number;
  /** The window the request was metered against, frozen at call time. */
  readonly contextWindow?: number;
}

/** Pair prompt usage with its frozen context window; reject other routes. */
export function liveContextUsageFromDiagnostics(
  diagnostics: ContextUsageSnapshot | undefined,
  route: LiveContextRoute,
): LiveContextUsage | undefined {
  if (!diagnostics || diagnostics.status !== 'available') return undefined;
  if (route.model === undefined || route.providerType === undefined) return undefined;
  if (diagnostics.modelId !== route.model || diagnostics.providerId !== route.providerType) {
    return undefined;
  }
  const inputTokens = diagnostics.inputTokens;
  if (inputTokens === undefined || !Number.isFinite(inputTokens) || inputTokens <= 0) {
    return undefined;
  }
  return {
    usageTokens: inputTokens,
    ...(diagnostics.contextWindow !== undefined
      ? { contextWindow: diagnostics.contextWindow }
      : {}),
  };
}

export interface LiveContextUsageTarget {
  readonly sessionId: string;
  readonly route: LiveContextRoute;
}

/**
 * Identity, not reference: two targets answer the same question when their
 * session and route match field by field.
 */
function sameLiveContextUsageTarget(
  left: LiveContextUsageTarget | undefined,
  right: LiveContextUsageTarget | undefined,
): boolean {
  if (left === undefined || right === undefined) return left === right;
  return (
    left.sessionId === right.sessionId &&
    left.route.model === right.route.model &&
    left.route.providerType === right.route.providerType
  );
}

export interface LiveContextUsageTracker {
  /** Aims the tracker at a session, or at nothing. Reads immediately. */
  setTarget(target: LiveContextUsageTarget | undefined): void;
  /** Records a live event; schedules a re-read when it can change the snapshot. */
  observe(event: SessionEvent): void;
  /** Drops any scheduled or in-flight read; the last reported value stands. */
  dispose(): void;
}

/** Re-read settled request facts on coalesced events. Target changes invalidate
 * old reads; transient failure retains only the current target's last value. */
export function createLiveContextUsageTracker(input: {
  query: (sessionId: string) => Promise<ContextUsageSnapshot>;
  delayMs: number;
  schedule: (callback: () => void, delayMs: number) => unknown;
  cancel: (handle: unknown) => void;
  onChange: (usage: LiveContextUsage | undefined) => void;
}): LiveContextUsageTracker {
  let target: LiveContextUsageTarget | undefined;
  let revision = 0;
  const coalescer: TraceRefreshCoalescer = createTraceRefreshCoalescer({
    refresh: () => refresh(),
    delayMs: input.delayMs,
    schedule: input.schedule,
    cancel: input.cancel,
  });

  function refresh(): void {
    const current = target;
    if (!current) return;
    const readRevision = ++revision;
    void input.query(current.sessionId).then(
      (diagnostics) => {
        if (readRevision !== revision) return;
        input.onChange(liveContextUsageFromDiagnostics(diagnostics, current.route));
      },
      () => {
        // A failed read leaves the last value standing: it is still the newest
        // answer anyone has, and blanking it would report "no usage" for a
        // read that simply failed.
      },
    );
  }

  return {
    setTarget(next) {
      // Any target change — another session, another route, or none — makes
      // the current reading unanswerable until the next read lands, and
      // invalidates every read already in flight. A changed target clears the
      // reading on screen BEFORE the first read on the new one: that reading
      // answers the previous target's question, and a rejected first read
      // would otherwise pin it there indefinitely. Re-aiming at the SAME
      // target does not clear — the standing value still answers it, and
      // blanking it would flicker.
      revision += 1;
      const changed = !sameLiveContextUsageTarget(target, next);
      target = next;
      coalescer.cancel();
      if (!next) {
        input.onChange(undefined);
        return;
      }
      if (changed) input.onChange(undefined);
      refresh();
    },
    observe(event) {
      coalescer.observe(event);
    },
    dispose() {
      revision += 1;
      target = undefined;
      coalescer.cancel();
    },
  };
}
