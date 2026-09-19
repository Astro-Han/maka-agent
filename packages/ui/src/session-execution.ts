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

/** Read-only execution activity. It is never an execution command or ownership claim. */
export interface SessionTurnActivity {
  readonly turnId: string;
  readonly status: 'admitted' | 'created' | 'running' | 'waiting_for_user' | 'completed' | 'failed' | 'cancelled';
  readonly rootExecutionKind?: 'context_compact';
}

export interface SessionExecutionProjection<Turn extends SessionTurnActivity = SessionTurnActivity> {
  readonly type: 'host_execution';
  readonly available: boolean;
  readonly rootTurn: Turn | null;
}

/** Retain the last nonterminal identity for Stop even while observation is unavailable. */
export function activeHostTurn<Turn extends SessionTurnActivity>(
  projection: SessionExecutionProjection<Turn> | undefined,
) {
  const turn = projection?.rootTurn;
  return turn && turn.status !== 'completed' && turn.status !== 'failed' && turn.status !== 'cancelled'
    ? turn : undefined;
}

/** Unavailable observation must never be displayed as current execution. */
export function chatTurnActivity(projection: SessionExecutionProjection | undefined) {
  if (!projection?.available) return undefined;
  const turn = activeHostTurn(projection);
  return turn ? {
    turnId: turn.turnId,
    awaitingInput: turn.status === 'waiting_for_user',
    compacting: turn.rootExecutionKind === 'context_compact',
  } : undefined;
}
