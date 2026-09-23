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

import type { Invocation } from './execution.js';
import type { ModelGeneration, ModelChoice } from './llm.js';

export interface ModelAttempt {
  requestId: string;
  origin:
    | { kind: 'agent'; invocation: Invocation; purpose: 'main' | 'compaction' }
    | {
        kind: 'auxiliary';
        source:
          | { kind: 'agent'; invocation: Invocation; operation_id: string }
          | { kind: 'host_effect'; id: string };
      };
  binding: ModelChoice['model'] | null;
  sessionId: string | null;
  modelId: string;
  startedAt: number;
  completedAt: number;
  outcome: 'success' | 'error' | 'aborted' | 'unknown';
  usage: ModelGeneration['usage'];
  quote: {
    providerId: string;
    revision: number;
    pricing: {
      modelKey: string;
      inputUsdPer1M: number;
      outputUsdPer1M: number;
      cacheReadUsdPer1M?: number;
      cacheWriteUsdPer1M?: number;
    } | null;
  } | null;
  costUsd: number | null;
}

export interface UsagePage {
  /** Re-read the same page. Cursors expire on Host restart and confer no access. */
  cursor: string;
  nextCursor: string | null;
  attempts: readonly ModelAttempt[];
  total: number;
}

export type UsageRead =
  | { kind: 'start'; filter: { from: number; to: number; sessionId?: string | null } }
  | { kind: 'continue'; cursor: string };

export interface Usage {
  /** Settled physical calls, including failed retries and auxiliary SDK calls.
   * At most 100 rows / 48 KiB. Missing counters or rates remain unknown.
   * Agent calls see their Session; independent calls need read_usage consent
   * with a profile or Session target. No conversation bodies are exposed.
   */
  models(input: UsageRead): Promise<UsagePage>;
}
