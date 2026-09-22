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

import { useEffect, useState } from 'react';
import { Button } from '@astryxdesign/core/Button';
import { THINKING_LEVELS } from '@maka/core/model-thinking';
import type { ExecutionTarget, ExecutorChoices, ExecutorSettings } from '@maka-agent/plugin-sdk/host';

type Target = Extract<ExecutionTarget, { kind: 'executor' }>;

export function ExecutorSelection({ locale, label, search, initialValue, onSelect }: {
  locale: string;
  label: string;
  search(query: { query: string }): Promise<ExecutorChoices>;
  initialValue?: Target;
  onSelect(target: Target): Promise<void>;
}) {
  const zh = locale !== 'en';
  const [query, setQuery] = useState('');
  const [page, setPage] = useState<ExecutorChoices>();
  const [selected, setSelected] = useState(initialValue?.executorId ?? '');
  const [draft, setDraft] = useState<{ id: string; settings: ExecutorSettings } | undefined>(initialValue ? { id: initialValue.executorId, settings: initialValue.settings ?? {} } : undefined);
  const [revision, setRevision] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  useEffect(() => {
    let active = true;
    setPage(undefined);
    setError(undefined);
    const timer = setTimeout(() => {
      void search({ query }).then((next) => {
        if (active) setPage(next);
      }).catch((error: unknown) => {
        if (active) setError(error instanceof Error ? error.message : String(error));
      });
    }, 150);
    return () => { active = false; clearTimeout(timer); };
  }, [search, query, revision]);
  const executor = page?.executors.find((choice) => choice.id === selected) ?? page?.executors[0];
  const settings = executor && draft?.id === executor.id ? draft.settings : {};
  const change = (patch: Partial<ExecutorSettings>) => {
    if (executor) setDraft({ id: executor.id, settings: { ...settings, ...patch } });
  };
  return (
    <div className="maka-executor-selection">
      <label>{zh ? '搜索执行器' : 'Search executors'}
        <input value={query} disabled={busy} onChange={(event) => setQuery(event.target.value)} />
      </label>
      <label>{zh ? '执行器' : 'Executor'}
        <select value={executor?.id ?? ''} disabled={busy || !executor} onChange={(event) => {
          setSelected(event.target.value);
          setDraft(undefined);
        }}>
          {!executor ? <option value="">{zh ? '没有可用执行器' : 'No executors available'}</option> : null}
          {page?.executors.map((choice) => <option key={choice.id} value={choice.id}>{choice.displayName}</option>)}
        </select>
      </label>
      {executor ? <>
        <label>{zh ? '模型 ID（可选）' : 'Model ID (optional)'}
          <input maxLength={512} value={settings.model ?? ''} disabled={busy} onChange={(event) => change({ model: event.target.value })} />
        </label>
        {executor.capabilities.thinking ? <label>{zh ? '思考程度' : 'Thinking'}
          <select value={settings.thinkingLevel ?? ''} disabled={busy} onChange={(event) => change({ thinkingLevel: THINKING_LEVELS.find((level) => level === event.target.value) })}>
            <option value="">{zh ? '默认' : 'Default'}</option>
            {THINKING_LEVELS.map((level) => <option key={level} value={level}>{level}</option>)}
          </select>
        </label> : null}
      </> : null}
      {page?.complete === false ? <p>{zh ? '请缩小搜索范围。' : 'Refine the search.'}</p> : null}
      <Button label={zh ? '刷新' : 'Refresh'} isDisabled={busy} onClick={() => setRevision((value) => value + 1)} />
      <Button label={label} isDisabled={busy || !executor} onClick={() => {
        if (!executor) return;
        setBusy(true);
        setError(undefined);
        const model = settings.model?.trim();
        const target: Target = { kind: 'executor', executorId: executor.id, settings: {
          ...(model ? { model } : {}),
          ...(executor.capabilities.thinking && settings.thinkingLevel ? { thinkingLevel: settings.thinkingLevel } : {}),
        } };
        void onSelect(target).catch((error: unknown) => {
          setError(error instanceof Error ? error.message : String(error));
        }).finally(() => setBusy(false));
      }} />
      {error ? <p role="alert">{error}</p> : null}
    </div>
  );
}
