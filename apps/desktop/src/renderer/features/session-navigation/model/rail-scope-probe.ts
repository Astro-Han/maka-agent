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

import type { SessionRailData } from '@maka/ui';

/** The flag `scripts/perf/*` sets to select the configuration to measure. */
const FLAG = '__makaRailScopeDefeated';

/**
 * The rail's data as a rail scoped to the whole shell delivered it (#4109).
 *
 * This exists so the rail's scope can be measured, and it is here because there
 * is no other honest way to do it. Renderer timings shift by orders of
 * magnitude between app launches while the spread inside one launch is small,
 * so "before" and "after" have to alternate inside a SINGLE running instance —
 * which means both configurations must be reachable from one build.
 *
 * It reproduces the defect rather than restoring the old code. The defect was
 * never "one object changed identity" — it was that everything a row is drawn
 * from was rebuilt by the shell's render, so the rows' `memo` compared unequal
 * and the whole rail redrew. Copying the callbacks is what reproduces that; a
 * fresh wrapper alone would leave every row bailing out and would measure
 * nothing.
 *
 * Returns the data untouched unless a probe sets the flag. No product path
 * sets it.
 */
export function applyRailScopeProbe(data: SessionRailData): SessionRailData {
  if ((globalThis as unknown as Record<string, unknown>)[FLAG] !== true) return data;
  const rowActions = data.rowActions;
  const projectActions = data.projectActions;
  return {
    ...data,
    onSelectSession: (sessionId) => data.onSelectSession?.(sessionId),
    sessionMeta: (session) => data.sessionMeta?.(session),
    rowActions: rowActions ? { ...rowActions } : rowActions,
    projectActions: projectActions ? { ...projectActions } : projectActions,
    groups: data.groups ? [...data.groups] : data.groups,
  };
}
