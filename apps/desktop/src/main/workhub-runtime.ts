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

import { type DesktopTargetScope } from '../shared/runtime-host-identity.js';
import type { DesktopRuntimeHostClient } from './runtime-host-client.js';

interface WorkHubRuntimeDeps {
  client(scope: DesktopTargetScope): Pick<DesktopRuntimeHostClient, 'queryTurn' | 'stopTurn'>;
  isCurrent(scope: DesktopTargetScope): boolean;
}

/** Validate the exact calling Session and turn; never infer a coordinator by name. */
export function createWorkHubRuntime(deps: WorkHubRuntimeDeps) {
  const requireCurrent = (scope: DesktopTargetScope) => {
    if (!deps.isCurrent(scope)) throw new Error('Runtime Host changed');
  };
  const queryTurn = async (client: ReturnType<WorkHubRuntimeDeps['client']>, sessionId: string, turnId: string) => {
    const turn = await client.queryTurn({ sessionId, turnId });
    if (turn.sessionId !== sessionId || turn.turnId !== turnId) throw new Error('WorkHub turn identity changed');
    return turn;
  };
  const isLive = (turn: Awaited<ReturnType<typeof queryTurn>>) =>
    turn.status !== 'completed' && turn.status !== 'failed' && turn.status !== 'cancelled';

  return {
    async assertTurn(scope: DesktopTargetScope, sessionId: string, turnId: string): Promise<void> {
      requireCurrent(scope);
      const turn = await queryTurn(deps.client(scope), sessionId, turnId);
      requireCurrent(scope);
      if (!isLive(turn)) throw new Error('WorkHub turn is no longer active');
    },
    async interrupt(scope: DesktopTargetScope, sessionId: string, turnId: string): Promise<void> {
      const client = deps.client(scope);
      const turn = await queryTurn(client, sessionId, turnId);
      if (isLive(turn)) await client.stopTurn({ sessionId: turn.sessionId, turnId: turn.turnId, runId: turn.runId });
    },
  };
}
