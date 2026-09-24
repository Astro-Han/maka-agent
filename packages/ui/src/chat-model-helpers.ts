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

import type { ChatModelChoice } from '@maka/core/chat-model-choice';
import type { ProviderIdentity } from '@maka/core/runtime-policy';
export type { ChatModelChoice } from '@maka/core/chat-model-choice';

export interface ModelMenuGroup {
  connectionSlug: string;
  provider: ProviderIdentity;
  heading: string;
  choices: ChatModelChoice[];
}

/** Connection identity owns grouping; labels never decide authority or availability. */
export function modelMenuGroups(choices: ChatModelChoice[]): ModelMenuGroup[] {
  const byConnection = new Map<string, ModelMenuGroup>();
  for (const choice of choices) {
    const group = byConnection.get(choice.connectionId);
    if (group) {
      group.choices.push(choice);
    } else {
      byConnection.set(choice.connectionId, {
        connectionSlug: choice.connectionSlug,
        provider: choice.provider,
        heading: choice.connectionName?.trim() || choice.providerLabel,
        choices: [choice],
      });
    }
  }
  const groups = [...byConnection.values()];
  const counts = new Map<string, number>();
  for (const { heading } of groups) counts.set(heading, (counts.get(heading) ?? 0) + 1);
  for (const group of groups) {
    if (counts.get(group.heading)! > 1) group.heading += ' · ' + group.connectionSlug;
  }
  return groups;
}

export function modelChoiceValue(connectionSlug: string, model: string): string {
  return `${encodeURIComponent(connectionSlug)}:${encodeURIComponent(model)}`;
}

export function exactModelChoiceValue(
  connectionId: string,
  connectionSlug: string,
  model: string,
): string {
  return `${encodeURIComponent(connectionId)}:${modelChoiceValue(connectionSlug, model)}`;
}

export function parseModelChoiceValue(value: string): { llmConnectionSlug: string; model: string } | undefined {
  const idx = value.indexOf(':');
  if (idx <= 0) return undefined;
  try {
    const llmConnectionSlug = decodeURIComponent(value.slice(0, idx));
    const model = decodeURIComponent(value.slice(idx + 1));
    if (!llmConnectionSlug || !model) return undefined;
    return { llmConnectionSlug, model };
  } catch {
    return undefined;
  }
}
