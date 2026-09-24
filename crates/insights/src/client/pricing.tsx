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

import type { ClientLocale } from '@maka-agent/plugin-sdk/client';
import { copy } from './pricing-copy.js';

import { useCallback, useEffect, useRef, useState } from 'react';
import type { Price, PricingPage, PricingQuery, PricingUpdate } from '@maka-agent/plugin-sdk/host';
import type { Api } from './model.js';
import { dollars } from './totals.js';

type Page = Extract<PricingPage, { kind: 'page' }>;
type Draft = { modelKey: string; input: string; output: string; read: string; write: string };
const blank: Draft = { modelKey: '', input: '', output: '', read: '', write: '' };
function edit(price: Price): Draft {
  return {
    modelKey: price.modelKey,
    input: String(price.inputUsdPer1M),
    output: String(price.outputUsdPer1M),
    read: price.cacheReadUsdPer1M?.toString() ?? '',
    write: price.cacheWriteUsdPer1M?.toString() ?? '',
  };
}

export function Pricing({
  api,
  signal,
  locale,
}: {
  api: Api;
  signal: AbortSignal;
  locale: ClientLocale;
}) {
  const t = copy[locale];
  const [page, setPage] = useState<Page>();
  const [draft, setDraft] = useState<Draft>(blank);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [saved, setSaved] = useState(false);
  const version = useRef(0);
  const active = useRef(false);
  const load = useCallback(
    async (query: PricingQuery) => {
      active.current = true;
      const ticket = ++version.current;
      setBusy(true);
      setError('');
      try {
        const result = await api.prices(query);
        if (signal.aborted || ticket !== version.current) return;
        if (result.kind === 'revision_changed') {
          setPage(undefined);
          throw new Error(t.ratesChanged);
        }
        setPage(result);
      } catch (reason) {
        if (!signal.aborted && ticket === version.current) setError(String(reason));
      } finally {
        if (ticket === version.current) {
          active.current = false;
          setBusy(false);
        }
      }
    },
    [api, signal, locale],
  );
  useEffect(() => {
    void load({ kind: 'start' });
    return () => {
      version.current++;
    };
  }, [load]);

  async function update(mutation: PricingUpdate['mutation']) {
    if (!page || active.current) return;
    active.current = true;
    const ticket = ++version.current;
    setBusy(true);
    setError('');
    setSaved(false);
    try {
      const receipt = await api.updatePrice({ expectedRevision: page.revision, mutation });
      if (signal.aborted || ticket !== version.current) return;
      // A conflict or missing reply never authorizes a blind resubmission.
      setPage(undefined);
      if (receipt.kind === 'revision_conflict') throw new Error(t.reviewChanges);
      setDraft(blank);
      setSaved(true);
      const refreshed = await api.prices({ kind: 'start' });
      if (!signal.aborted && ticket === version.current && refreshed.kind === 'page')
        setPage(refreshed);
    } catch (reason) {
      if (!signal.aborted && ticket === version.current) {
        setPage(undefined);
        setError(String(reason));
      }
    } finally {
      active.current = false;
      if (ticket === version.current) setBusy(false);
    }
  }
  function save() {
    const rate = (raw: string) => {
      const value = Number(raw);
      if (!raw.trim() || !Number.isFinite(value) || value < 0) throw new Error(t.invalidRates);
      return value;
    };
    try {
      if (!draft.modelKey.trim()) throw new Error(t.modelRequired);
      void update({
        kind: 'upsert',
        pricing: {
          modelKey: draft.modelKey.trim(),
          inputUsdPer1M: rate(draft.input),
          outputUsdPer1M: rate(draft.output),
          ...(draft.read.trim() ? { cacheReadUsdPer1M: rate(draft.read) } : {}),
          ...(draft.write.trim() ? { cacheWriteUsdPer1M: rate(draft.write) } : {}),
        },
      });
    } catch (reason) {
      setError(String(reason));
    }
  }
  return (
    <section aria-label={t.title}>
      <p className="insights-note">{t.description}</p>
      <button type="button" disabled={busy} onClick={() => void load({ kind: 'start' })}>
        {t.refresh}
      </button>
      {error && <p role="alert">{error}</p>}
      {saved && <p role="status">{t.saved}</p>}
      <form
        onSubmit={(event) => {
          event.preventDefault();
          save();
        }}
      >
        <fieldset disabled={busy || !page} className="insights-price-form">
          <legend>{t.setRates}</legend>
          <label>
            {t.modelKey}
            <input
              required
              maxLength={512}
              value={draft.modelKey}
              onChange={(event) => setDraft({ ...draft, modelKey: event.target.value })}
            />
          </label>
          {(['input', 'output', 'read', 'write'] as const).map((key, index) => (
            <label key={key}>
              {t.rateLabels[index]}
              <input
                type="number"
                min="0"
                step="any"
                required={key === 'input' || key === 'output'}
                value={draft[key]}
                onChange={(event) => setDraft({ ...draft, [key]: event.target.value })}
              />
            </label>
          ))}
          <button type="submit">{t.save}</button>
          <button type="button" onClick={() => setDraft(blank)}>
            {t.clear}
          </button>
        </fieldset>
      </form>
      {page && (
        <>
          <div className="insights-table">
            <table>
              <thead>
                <tr>
                  {t.columns.map((label) => (
                    <th key={label}>{label}</th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {page.entries.map((entry) => (
                  <tr key={entry.pricing.modelKey}>
                    <td>{entry.pricing.modelKey}</td>
                    {[
                      entry.pricing.inputUsdPer1M,
                      entry.pricing.outputUsdPer1M,
                      entry.pricing.cacheReadUsdPer1M,
                      entry.pricing.cacheWriteUsdPer1M,
                    ].map((value, index) => (
                      <td key={index}>{value === undefined ? '—' : dollars(value, locale)}</td>
                    ))}
                    <td>{entry.source === 'builtin' ? t.builtin : t.custom}</td>
                    <td>
                      <button
                        type="button"
                        disabled={busy}
                        onClick={() => setDraft(edit(entry.pricing))}
                      >
                        {t.edit}
                      </button>
                      {entry.source === 'custom' && (
                        <button
                          type="button"
                          disabled={busy}
                          onClick={() =>
                            void update({ kind: 'delete', modelKey: entry.pricing.modelKey })
                          }
                        >
                          {entry.resetEffect === 'restore_builtin' ? t.restore : t.remove}
                        </button>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <div className="insights-actions">
            <span>
              {t.entries} {page.entries.length ? page.offset + 1 : 0}–
              {page.offset + page.entries.length}
            </span>
            <button
              type="button"
              disabled={busy || page.offset === 0}
              onClick={() => void load({ kind: 'start' })}
            >
              {t.first}
            </button>
            <button
              type="button"
              disabled={busy || page.nextOffset === null}
              onClick={() => {
                if (page.nextOffset !== null)
                  void load({ kind: 'continue', revision: page.revision, offset: page.nextOffset });
              }}
            >
              {t.next}
            </button>
          </div>
        </>
      )}
    </section>
  );
}
