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

export type Price = {
  modelKey: string;
  inputUsdPer1M: number;
  outputUsdPer1M: number;
  cacheReadUsdPer1M?: number;
  cacheWriteUsdPer1M?: number;
};
export type PriceQuote = { providerId: string; revision: number; pricing: Price | null };

export type PricingQuery =
  | { kind: 'start' }
  | { kind: 'continue'; revision: number; offset: number };

export type PricingPage =
  | {
      kind: 'page';
      revision: number;
      offset: number;
      entries: readonly (
        | { source: 'builtin'; pricing: Price }
        | {
            source: 'custom';
            pricing: Price;
            resetEffect: 'restore_builtin' | 'become_unpriced';
          }
      )[];
      nextOffset: number | null;
    }
  | { kind: 'revision_changed'; expectedRevision: number; actualRevision: number };

export type PricingUpdate = {
  expectedRevision: number;
  mutation: { kind: 'upsert'; pricing: Price } | { kind: 'delete'; modelKey: string };
};
export type PricingUpdated =
  | { kind: 'committed' | 'unchanged'; revision: number }
  | { kind: 'revision_conflict'; expectedRevision: number; actualRevision: number };

export interface PricingCatalog {
  /** At most 128 entries / 48 KiB. Revision changes require restarting the scan. */
  query(input: PricingQuery): Promise<PricingPage>;
}
export interface Prices extends PricingCatalog {
  /** Requires explicit profile manage_pricing consent. Changes affect future admissions only.
   * A lost reply can produce a revision conflict on retry; query before deciding the next edit.
   */
  update(input: PricingUpdate): Promise<PricingUpdated>;
}
