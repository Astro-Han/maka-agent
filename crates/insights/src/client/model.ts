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

import type { ClientContext } from '@maka-agent/plugin-sdk/client';
import type {
  Json,
  UsagePage,
  UsageRead,
  UsageSelection,
  UsageSummary,
  PricingPage,
  PricingQuery,
  PricingUpdate,
  PricingUpdated,
} from '@maka-agent/plugin-sdk/host';

export type Range = '24h' | '7d' | '30d' | 'all';
export type Tab = 'overview' | 'activity' | 'providers' | 'models' | 'tools' | 'pricing';
export type Preferences = { range: Range; tab: Tab; selection: UsageSelection };
export type Snapshot = { revision: number | null; preferences: Preferences };
type Request =
  | { kind: 'preferences' }
  | { kind: 'save_preferences'; expectedRevision: number | null; preferences: Preferences }
  | { kind: 'activity'; operationId: string; read: UsageRead }
  | { kind: 'summary'; operationId: string; cursor: string }
  | { kind: 'prices'; query: PricingQuery }
  | { kind: 'update_price'; operationId: string; update: PricingUpdate };
type Response =
  | { kind: 'preferences'; snapshot: Snapshot }
  | { kind: 'activity'; page: UsagePage }
  | { kind: 'summary'; summary: UsageSummary }
  | { kind: 'prices'; page: PricingPage }
  | { kind: 'price_updated'; receipt: PricingUpdated }
  | { kind: 'refresh_required' };

export class RefreshRequired extends Error {
  constructor() {
    super('Snapshot changed. Refresh before continuing.');
  }
}

export function api(context: ClientContext) {
  // One JSON boundary, bound to this Client's exact Rust backend generation.
  const invoke = context.remote.method<Json, Json>('request') as unknown as (
    input: Request,
  ) => Promise<Response>;
  async function ask<K extends Response['kind']>(request: Request, expected: K) {
    const response = await invoke(request);
    if (response.kind === 'refresh_required') throw new RefreshRequired();
    if (response.kind !== expected) throw new Error('Unexpected Insights response');
    return response as Extract<Response, { kind: K }>;
  }
  return {
    preferences: async () => (await ask({ kind: 'preferences' }, 'preferences')).snapshot,
    save: async (snapshot: Snapshot, preferences: Preferences) =>
      (
        await ask(
          { kind: 'save_preferences', expectedRevision: snapshot.revision, preferences },
          'preferences',
        )
      ).snapshot,
    activity: async (read: UsageRead) =>
      (await ask({ kind: 'activity', operationId: crypto.randomUUID(), read }, 'activity')).page,
    summary: async (cursor: string) =>
      (await ask({ kind: 'summary', operationId: crypto.randomUUID(), cursor }, 'summary')).summary,
    prices: async (query: PricingQuery) => (await ask({ kind: 'prices', query }, 'prices')).page,
    updatePrice: async (update: PricingUpdate) =>
      (
        await ask(
          { kind: 'update_price', operationId: crypto.randomUUID(), update },
          'price_updated',
        )
      ).receipt,
  };
}
export type Api = ReturnType<typeof api>;

export function rangeBounds(range: Range) {
  const to = Date.now();
  const days = { '24h': 1, '7d': 7, '30d': 30, all: 0 }[range];
  return { from: days ? Math.max(0, to - days * 86_400_000) : 0, to };
}
