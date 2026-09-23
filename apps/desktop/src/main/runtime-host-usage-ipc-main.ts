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

import type { UsageQuery } from "@maka/core/usage-stats/types";
import { handleReconnectableRead, type ReconnectableReadIpcMain, tryReconnectableReadResult } from "./ipc-reconnect-policy.js";
import type { DesktopRuntimeHostClient } from "./runtime-host-client.js";

/** Session Inspector's read; settings pages use the public plugin Usage API. */
export function registerRuntimeHostUsageIpc(deps: {
  readonly ipcMain: ReconnectableReadIpcMain;
  readonly client: DesktopRuntimeHostClient;
}): void {
  handleReconnectableRead(deps.ipcMain, "usage:summary", (_event, query: UsageQuery) =>
    tryReconnectableReadResult(async () => {
      const { toolName: _toolName, ...llmQuery } = query;
      const result = await deps.client.queryUsage({ kind: "summary", query: llmQuery });
      if (result.kind !== "summary") throw new Error("Runtime Host returned an invalid Usage projection");
      return { ...result.summary, provenance: result.provenance };
    }, "USAGE_SUMMARY_FAILED"));
}
