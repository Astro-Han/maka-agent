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

/** Read the latest provider-counted request in a tail transcript. Skip records
 * without anchors; reject the newest anchor if its model or connection differs. */
export interface LatestRequestUsageAnchor {
  inputTokens: number;
  outputTokens?: number;
  modelId?: string;
  connectionId?: string;
}

export function selectLatestRequestUsage(
  messages: readonly { type: string; lastRequestAnchor?: LatestRequestUsageAnchor }[],
  model: string | undefined,
  route: { llmConnectionId?: string } | undefined,
): number | undefined {
  const connectionId = route?.llmConnectionId;
  if (model === undefined || connectionId === undefined) return undefined;
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const message = messages[index];
    if (message?.type !== 'token_usage') continue;
    const anchor = message.lastRequestAnchor;
    if (!anchor) continue;
    if (anchor.modelId !== model || anchor.connectionId !== connectionId) return undefined;
    if (!Number.isFinite(anchor.inputTokens) || anchor.inputTokens <= 0) return undefined;
    const output = Number.isFinite(anchor.outputTokens ?? 0) ? Math.max(0, anchor.outputTokens ?? 0) : 0;
    return anchor.inputTokens + output;
  }
  return undefined;
}
