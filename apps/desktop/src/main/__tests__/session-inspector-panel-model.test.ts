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

import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  SESSION_TRACE_SCHEMA_VERSION,
  type SessionTrace,
  type TraceModelAttempt,
  type TraceModelCallStep,
  type TraceStep,
} from '@maka/core/session-trace';
import {
  deriveInspectorPanelModel,
  deriveInspectorOverviewModel,
} from '../../renderer/features/workbar/testing.js';

test('derives per-turn cost only from priced model-call step totals', () => {
  const cases: readonly {
    name: string;
    steps: TraceStep[];
    expected: number | undefined;
  }[] = [
    { name: 'empty', steps: [], expected: undefined },
    {
      name: 'tool-only',
      steps: [
        {
          kind: 'tool',
          id: 'tool-1',
          turnId: 'turn-1',
          runId: 'run-1',
          startedAt: 1,
          endedAt: 2,
          durationMs: 1,
          toolName: 'Read',
          status: 'completed',
        },
      ],
      expected: undefined,
    },
    { name: 'unpriced', steps: [modelCallStep('unpriced')], expected: undefined },
    { name: 'priced', steps: [modelCallStep('priced', 0.01)], expected: 0.01 },
    {
      name: 'mixed',
      steps: [modelCallStep('priced', 0.01), modelCallStep('unpriced')],
      expected: 0.01,
    },
    {
      name: 'multiple calls',
      steps: [modelCallStep('first', 0.01), modelCallStep('second', 0.02)],
      expected: 0.03,
    },
    { name: 'zero-priced', steps: [modelCallStep('free', 0)], expected: 0 },
    {
      name: 'retried logical call',
      // Deliberately disagree with the nested attempts: the display boundary
      // must trust the logical call's already-aggregated price.
      steps: [modelCallStep('retry', 0.04, [modelAttempt(0, 0.01), modelAttempt(1, 0.02)])],
      expected: 0.04,
    },
  ];

  for (const { name, steps, expected } of cases) {
    const turn = deriveInspectorPanelModel(traceWithSteps(steps)).turns[0];
    assert.equal(turn?.costUsd, expected, name);
    assert.equal(turn?.durationMs, 9, `${name} duration`);
  }
});

test('shows one compact diagnostic line for a failed history-compaction call', () => {
  const trace: SessionTrace = {
    schemaVersion: SESSION_TRACE_SCHEMA_VERSION,
    sessionId: 'session-1',
    turns: [
      {
        turnId: 'turn-1',
        runId: 'run-1',
        startedAt: 1,
        endedAt: 10,
        durationMs: 9,
        steps: [
          {
            kind: 'model_call',
            id: 'call-compact-1',
            turnId: 'turn-1',
            runId: 'run-1',
            startedAt: 1,
            endedAt: 10,
            durationMs: 9,
            callKind: 'history_compact',
            historyCompactRoute: 'provider_native',
            providerId: 'openai-codex',
            modelId: 'gpt-5.6-luna',
            step: 0,
            status: 'failed',
            attempts: [
              {
                attemptId: 'attempt-compact-1',
                attempt: 0,
                status: 'failed',
                startedAt: 1,
                completedAt: 10,
                latencyMs: 9,
                errorClass: 'RequestRejected',
                httpStatus: 400,
                providerCode: 'invalid_request_error',
                providerRequestId: 'req-compact-1',
                retryable: false,
                costBasis: 'unpriced',
                usageBasis: 'missing',
              },
            ],
          },
        ],
      },
    ],
    coverage: {
      modelCalls: 'no_known_gap',
      turnsMissingModelCalls: [],
      turnsWithFewerModelCallsThanSteps: [],
      unreadableRecords: 0,
      oversizedRuns: 0,
    },
  };

  const row = deriveInspectorPanelModel(trace).turns[0]?.steps[0];
  assert.equal(row?.callKind, 'history_compact');
  assert.equal(
    row?.detail,
    'route=provider_native · error=RequestRejected · HTTP 400 · code=invalid_request_error · request=req-compact-1 · retryable=false',
  );
  const turn = deriveInspectorPanelModel(trace).turns[0];
  assert.equal((turn as { startedAt?: number } | undefined)?.startedAt, 1);
});

test('reports runs omitted only by the bounded online view separately', () => {
  const model = deriveInspectorPanelModel({
    schemaVersion: SESSION_TRACE_SCHEMA_VERSION,
    sessionId: 'session-1',
    turns: [],
    coverage: {
      modelCalls: 'partial',
      turnsMissingModelCalls: [],
      turnsWithFewerModelCallsThanSteps: [],
      unreadableRecords: 0,
      oversizedRuns: 1,
    },
  });

  assert.deepEqual(model.coverage, {
    kind: 'partial',
    turnsMissing: 0,
    turnsShort: 0,
    unreadableRecords: 0,
    oversizedRuns: 1,
  });
});

function traceWithSteps(steps: TraceStep[]): SessionTrace {
  return {
    schemaVersion: SESSION_TRACE_SCHEMA_VERSION,
    sessionId: 'session-1',
    turns: [
      {
        turnId: 'turn-1',
        runId: 'run-1',
        startedAt: 1,
        endedAt: 10,
        durationMs: 9,
        steps,
      },
    ],
    coverage: {
      modelCalls: 'no_known_gap',
      turnsMissingModelCalls: [],
      turnsWithFewerModelCallsThanSteps: [],
      unreadableRecords: 0,
      oversizedRuns: 0,
    },
  };
}

function modelCallStep(
  id: string,
  costUsd?: number,
  attempts: TraceModelAttempt[] = [modelAttempt(0, costUsd)],
): TraceModelCallStep {
  return {
    kind: 'model_call',
    id,
    turnId: 'turn-1',
    runId: 'run-1',
    startedAt: 1,
    endedAt: 10,
    durationMs: 9,
    callKind: 'main',
    providerId: 'provider-1',
    modelId: 'model-1',
    step: 0,
    attempts,
    status: 'completed',
    ...(costUsd !== undefined ? { costUsd } : {}),
  };
}

function modelAttempt(attempt: number, costUsd?: number): TraceModelAttempt {
  return {
    attemptId: `attempt-${attempt}`,
    attempt,
    status: 'completed',
    startedAt: 1,
    completedAt: 10,
    latencyMs: 9,
    ...(costUsd !== undefined ? { costUsd } : {}),
    costBasis: costUsd === undefined ? 'unpriced' : 'priced',
    usageBasis: 'reported',
  };
}
