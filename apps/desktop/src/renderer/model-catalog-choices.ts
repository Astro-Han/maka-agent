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

import { resolveConnectionModelCatalog, type ModelCatalogEntry } from '@maka/core/model-catalog';
import {
  offerableCatalogEntries,
  type ProjectedLlmConnection,
} from '@maka/core/llm-connections';
import type { ProviderType } from '@maka/core/llm-connections';
import type { UiLocale } from '@maka/core/ui-locale';
import { getShellRemainingCopy } from './locales/shell-remaining-copy.js';

const DAILY_REVIEW_MODEL_KEY_SEPARATOR = '::';

/**
 * The model to pre-fill when adding a provider. No connection exists yet, so
 * there is no Host-resolved catalog to read: this is the one place a client
 * still resolves a catalog itself, and it answers a question about the
 * provider rather than about a connection.
 */
export function buildCatalogRecommendedDefaultModel(providerType: ProviderType): string {
  const entries = resolveConnectionModelCatalog({
    slug: providerType,
    providerType,
    defaultModel: '',
  }).filter((entry) => entry.canUseAsChatDefault);
  return entries[0]?.id ?? '';
}

export function buildCatalogDailyReviewModelOptions(
  connections: readonly ProjectedLlmConnection[],
  currentModelKey: string,
  locale: UiLocale,
): Array<readonly [string, string]> {
  const current = parseDailyReviewModelKey(currentModelKey);
  const candidates: Array<{ key: string; label: string; safeSourceLabel: string }> = [];
  const seenKeys = new Set<string>();

  for (const connection of connections) {
    const safeSourceLabel = connection.name === connection.slug
      ? connection.slug
      : `${connection.name} · ${connection.slug}`;
    // The Host decides what is offerable; the caller appends a saved-but-
    // unavailable selection itself, with a label that says so.
    for (const entry of offerableCatalogEntries(connection)) {
      const key = dailyReviewModelKey(connection.slug, entry.id);
      if (seenKeys.has(key)) continue;
      seenKeys.add(key);
      candidates.push({ key, label: modelDisplayLabel(entry), safeSourceLabel });
    }
  }

  const options: Array<readonly [string, string]> = [];
  const modelCounts = new Map<string, number>();
  for (const candidate of candidates) {
    modelCounts.set(candidate.label, (modelCounts.get(candidate.label) ?? 0) + 1);
  }
  for (const candidate of candidates) {
    const label = (modelCounts.get(candidate.label) ?? 0) > 1
      ? `${candidate.label} · ${candidate.safeSourceLabel}`
      : candidate.label;
    options.push([candidate.key, label]);
  }

  const trimmedCurrent = currentModelKey.trim();
  if (trimmedCurrent && !options.some(([value]) => value === trimmedCurrent)) {
    const label = current?.model || trimmedCurrent.split(DAILY_REVIEW_MODEL_KEY_SEPARATOR).pop() || trimmedCurrent;
    const sourceLabel = current?.connectionSlug ? ` · ${current.connectionSlug}` : '';
    options.push([trimmedCurrent, `${label}${sourceLabel} · ${getShellRemainingCopy(locale).models.unavailable}`]);
  }

  return options;
}

function modelDisplayLabel(entry: Pick<ModelCatalogEntry, 'id' | 'displayName'>): string {
  return entry.displayName?.trim() || entry.id;
}

function dailyReviewModelKey(connectionSlug: string, model: string): string {
  return `${connectionSlug}${DAILY_REVIEW_MODEL_KEY_SEPARATOR}${model}`;
}

function parseDailyReviewModelKey(value: string): { connectionSlug: string; model: string } | undefined {
  const trimmed = value.trim();
  const index = trimmed.indexOf(DAILY_REVIEW_MODEL_KEY_SEPARATOR);
  if (index <= 0) return undefined;
  const connectionSlug = trimmed.slice(0, index);
  const model = trimmed.slice(index + DAILY_REVIEW_MODEL_KEY_SEPARATOR.length);
  if (!connectionSlug || !model) return undefined;
  return { connectionSlug, model };
}
