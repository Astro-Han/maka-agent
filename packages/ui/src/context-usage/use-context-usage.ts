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

import { useEffect, useState } from 'react';
import type { Result } from '@maka/core/result';
import type { SessionEvent } from '@maka/core/events';
import {
  createLiveContextUsageTracker,
  type LiveContextUsage,
  type ContextUsageSnapshot,
} from './tracker.js';
import { TRACE_REFRESH_DEBOUNCE_MS } from './refresh.js';

export interface ContextUsageService {
  context(sessionId: string): Promise<Result<ContextUsageSnapshot>>;
  subscribeSessionEvents(sessionId: string, handler: (event: SessionEvent) => void): () => void;
}

/** Read settled provider-request usage immediately on target changes and
 * refresh on coalesced execution events, not on every streamed token. */
export function useLiveContextUsage(input: {
  readonly inspector: ContextUsageService;
  readonly sessionId: string | undefined;
  readonly model: string | undefined;
  readonly providerType: string | undefined;
}): LiveContextUsage | undefined {
  const { inspector } = input;
  const [usage, setUsage] = useState<LiveContextUsage | undefined>(undefined);
  const { sessionId, model, providerType } = input;
  useEffect(() => {
    const tracker = createLiveContextUsageTracker({
      query: async (targetSessionId) => {
        const result = await inspector.context(targetSessionId);
        if (!result.ok) throw new Error(result.error.message);
        return result.data;
      },
      delayMs: TRACE_REFRESH_DEBOUNCE_MS,
      schedule: (callback, delayMs) => setTimeout(callback, delayMs),
      cancel: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>),
      onChange: setUsage,
    });
    tracker.setTarget(
      sessionId === undefined
        ? undefined
        : { sessionId, route: { model, providerType } },
    );
    const unsubscribe =
      sessionId === undefined
        ? undefined
        : inspector.subscribeSessionEvents(sessionId, (event) => tracker.observe(event));
    return () => {
      unsubscribe?.();
      tracker.dispose();
    };
  }, [inspector, sessionId, model, providerType]);
  return usage;
}
