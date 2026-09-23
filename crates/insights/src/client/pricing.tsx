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

export function Pricing({ api, signal, zh }: { api: Api; signal: AbortSignal; zh: boolean }) {
  const [page, setPage] = useState<Page>();
  const [draft, setDraft] = useState<Draft>(blank);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
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
          throw new Error(zh ? '报价已变化，请刷新。' : 'Rates changed. Refresh the catalog.');
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
    [api, signal, zh],
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
    setNotice('');
    try {
      const receipt = await api.updatePrice({ expectedRevision: page.revision, mutation });
      if (signal.aborted || ticket !== version.current) return;
      // A conflict or missing reply never authorizes a blind resubmission.
      setPage(undefined);
      if (receipt.kind === 'revision_conflict')
        throw new Error(
          zh
            ? '报价已被修改。刷新后检查，再决定是否保存。'
            : 'Rates changed. Refresh and review before deciding to save again.',
        );
      setDraft(blank);
      setNotice(
        zh ? '已保存；仅影响此后准入的调用。' : 'Saved; applies to future admissions only.',
      );
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
      if (!raw.trim() || !Number.isFinite(value) || value < 0)
        throw new Error(
          zh ? '单价必须是非负有限数字。' : 'Rates must be finite, non-negative numbers.',
        );
      return value;
    };
    try {
      if (!draft.modelKey.trim()) throw new Error(zh ? '请输入模型标识。' : 'Enter a model key.');
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
    <section aria-label={zh ? '报价' : 'Pricing'}>
      <p className="insights-note">
        {zh
          ? '美元 / 百万 token。空白缓存价表示未配置，不是免费。历史调用保留准入时的报价。'
          : 'USD per million tokens. Blank cache rates are unspecified, not free. Historical calls retain their admission quote.'}
      </p>
      <button type="button" disabled={busy} onClick={() => void load({ kind: 'start' })}>
        {zh ? '刷新报价' : 'Refresh rates'}
      </button>
      {error && <p role="alert">{error}</p>}
      {notice && <p role="status">{notice}</p>}
      <form
        onSubmit={(event) => {
          event.preventDefault();
          save();
        }}
      >
        <fieldset disabled={busy || !page} className="insights-price-form">
          <legend>{zh ? '设置模型报价' : 'Set model rates'}</legend>
          <label>
            {zh ? '模型标识' : 'Model key'}
            <input
              required
              maxLength={512}
              value={draft.modelKey}
              onChange={(event) => setDraft({ ...draft, modelKey: event.target.value })}
            />
          </label>
          {(['input', 'output', 'read', 'write'] as const).map((key, index) => (
            <label key={key}>
              {
                (zh
                  ? ['输入', '输出', '缓存读取（可选）', '缓存写入（可选）']
                  : ['Input', 'Output', 'Cache read (optional)', 'Cache write (optional)'])[index]
              }
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
          <button type="submit">{zh ? '保存报价' : 'Save rates'}</button>
          <button type="button" onClick={() => setDraft(blank)}>
            {zh ? '清空草稿' : 'Clear draft'}
          </button>
        </fieldset>
      </form>
      {page && (
        <>
          <div className="insights-table">
            <table>
              <thead>
                <tr>
                  {(zh
                    ? ['模型', '输入', '输出', '缓存读取', '缓存写入', '来源', '操作']
                    : ['Model', 'Input', 'Output', 'Cache read', 'Cache write', 'Source', 'Actions']
                  ).map((label) => (
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
                      <td key={index}>{value === undefined ? '—' : dollars(value)}</td>
                    ))}
                    <td>
                      {entry.source === 'builtin'
                        ? zh
                          ? '内置'
                          : 'Built-in'
                        : zh
                          ? '自定义'
                          : 'Custom'}
                    </td>
                    <td>
                      <button
                        type="button"
                        disabled={busy}
                        onClick={() => setDraft(edit(entry.pricing))}
                      >
                        {zh ? '编辑' : 'Edit'}
                      </button>
                      {entry.source === 'custom' && (
                        <button
                          type="button"
                          disabled={busy}
                          onClick={() =>
                            void update({ kind: 'delete', modelKey: entry.pricing.modelKey })
                          }
                        >
                          {entry.resetEffect === 'restore_builtin'
                            ? zh
                              ? '恢复内置价'
                              : 'Restore built-in'
                            : zh
                              ? '移除报价'
                              : 'Remove rates'}
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
              {zh ? '当前条目' : 'Entries'} {page.entries.length ? page.offset + 1 : 0}–
              {page.offset + page.entries.length}
            </span>
            <button
              type="button"
              disabled={busy || page.offset === 0}
              onClick={() => void load({ kind: 'start' })}
            >
              {zh ? '首页' : 'First'}
            </button>
            <button
              type="button"
              disabled={busy || page.nextOffset === null}
              onClick={() => {
                if (page.nextOffset !== null)
                  void load({ kind: 'continue', revision: page.revision, offset: page.nextOffset });
              }}
            >
              {zh ? '下一页' : 'Next'}
            </button>
          </div>
        </>
      )}
    </section>
  );
}
