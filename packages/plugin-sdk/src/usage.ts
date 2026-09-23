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
  quote: import('./pricing.js').PriceQuote | null;
  costUsd: number | null;
}

export interface UsagePage {
  /** Re-read the same page. Cursors expire on Host restart and confer no access. */
  cursor: string;
  nextCursor: string | null;
  attempts: readonly Activity[];
  total: number;
}

export interface ToolAttempt {
  requestId: string;
  invocation: Invocation;
  call: {
    tool_call_id: string;
    origin:
      | { kind: 'provider'; step_id: string }
      | {
          kind: 'code_mode' | 'code_cell';
          parent_operation_id: string;
          parent_tool_call_id: string;
        }
      | { kind: 'standalone' }
      | {
          kind: 'host_sdk';
          package_id: string;
          entry_id: string;
          activation: string;
          parent_operation_id: string | null;
        };
  };
  name: string;
  binding: ModelChoice['model'] | null;
  completedAt: number;
  result:
    | {
        kind: 'rejected';
        reason:
          | 'unavailable'
          | 'invalid_input'
          | 'policy_denied'
          | 'preparation_failed'
          | 'exclusive_conflict'
          | 'cancelled';
      }
    | { kind: 'settled'; startedAt: number; outcome: 'success' | 'error' | 'unknown' };
}

export type Activity =
  | { kind: 'model'; attempt: ModelAttempt }
  | { kind: 'tool'; attempt: ToolAttempt };

export interface UsageSelection {
  kind?: 'model' | 'tool' | null;
  status?: 'success' | 'error' | 'aborted' | 'unknown' | 'rejected' | null;
  /** Literal ASCII-case-insensitive substring, at most 1 KiB UTF-8; no control characters. */
  search?: string;
}

export type UsageRead =
  | {
      kind: 'start';
      filter: { from: number; to: number; sessionId?: string | null; activity?: UsageSelection };
    }
  | { kind: 'continue'; cursor: string }
  | { kind: 'refine'; cursor: string; selection: UsageSelection };

export interface Usage {
  /** Physical calls, including failed retries, auxiliary SDK calls and tool refusals.
   * At most 100 rows / 48 KiB. Missing counters or rates remain unknown.
   * Agent calls see their Session; independent calls need read_usage consent
   * with a profile or Session target. No conversation bodies are exposed.
   */
  activity(input: UsageRead): Promise<UsagePage>;
  /** Uses the activity cursor's scope, time range and fence, ignoring list filters.
   * Known subtotals retain missing-call coverage; a zero estimate is not missing.
   */
  summary(cursor: string): Promise<UsageSummary>;
}

export interface UsageTokens {
  known: number;
  missing: number;
}
export interface UsageSummary {
  /** Unsettled admissions within the time range at this fence, excluded from completed totals. */
  pending: { models: number; tools: number };
  /** Complete breakdowns, at most 128 groups each / 48 KiB overall; overflow fails explicitly. */
  byProvider: readonly { providerId: string | null; totals: UsageSummary['models'] }[];
  byModel: readonly { modelId: string; totals: UsageSummary['models'] }[];
  byTool: readonly { name: string; totals: UsageSummary['tools'] }[];
  models: {
    calls: number;
    success: number;
    error: number;
    aborted: number;
    unknown: number;
    /** Sum of observed durations, excluding unknown outcomes; not wall-clock time. */
    durationMs: number;
    input: UsageTokens;
    output: UsageTokens;
    cacheRead: UsageTokens;
    cacheWrite: UsageTokens;
    reasoning: UsageTokens;
    cost: { knownUsd: number; unvalued: number; unpriced: number };
  };
  tools: {
    calls: number;
    success: number;
    error: number;
    unknown: number;
    rejected: number;
    /** Known success/error durations only; not wall-clock time. */
    durationMs: number;
    /** Only known success/error settlements contribute to latency. */
    meanLatencyMs: number | null;
  };
}
