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
import type { ClientContext, ClientSlots } from '@maka-agent/plugin-sdk/client';

type Preset = {
  id: string;
  name: string;
  description: string;
  profile: 'local_read' | 'web_research' | 'implementation';
  connectionSlug: string;
  model: string;
  thinkingLevel: 'off' | 'minimal' | 'low' | 'medium' | 'high' | 'xhigh' | 'max' | null;
  enabled: boolean;
};
type Snapshot = { revision: number | null; presets: Preset[] };
type Request = { kind: 'read' } | { kind: 'replace'; snapshot: Snapshot };
type Props = ClientSlots['application.manage'] & { context: ClientContext };

const copy = {
  en: {
    add: 'Add agent preset',
    reload: 'Reload',
    save: 'Save',
    remove: 'Remove',
    cancel: 'Cancel',
    id: 'ID',
    name: 'Name',
    description: 'Instructions for choosing this preset',
    connection: 'Connection slug',
    model: 'Model',
    profile: 'Capabilities',
    thinking: 'Reasoning effort',
    enabled: 'Enabled',
    inherited: 'Default',
    empty: 'No presets. General-purpose agents remain available.',
    loading: 'Loading…',
    local_read: 'Read-only repository',
    web_research: 'Web research',
    implementation: 'Isolated implementation',
  },
  zh: {
    add: '添加 Agent 预设',
    reload: '重新加载',
    save: '保存',
    remove: '删除',
    cancel: '取消',
    id: '标识',
    name: '名称',
    description: '选择此预设的说明',
    connection: '连接标识',
    model: '模型',
    profile: '能力',
    thinking: '思考程度',
    enabled: '启用',
    inherited: '默认',
    empty: '尚无预设，仍可使用通用 Agent。',
    loading: '加载中…',
    local_read: '只读代码库',
    web_research: '网络调研',
    implementation: '独立工作区实现',
  },
};

export function Presets({ context, locale }: Props) {
  const t = locale === 'en' ? copy.en : copy.zh;
  const [snapshot, setSnapshot] = useState<Snapshot>();
  const [draft, setDraft] = useState<Preset>();
  const [editing, setEditing] = useState<string>();
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const [reload, setReload] = useState(0);
  useEffect(() => {
    let active = true;
    setSnapshot(undefined);
    context.remote
      .method<Request, Snapshot>('settings')({ kind: 'read' })
      .then((value) => {
        if (active) {
          setSnapshot(value);
          setError('');
        }
      })
      .catch((reason) => {
        if (active) setError(String(reason));
      });
    return () => {
      active = false;
    };
  }, [context, reload]);
  const save = async (presets: Preset[]) => {
    if (!snapshot || busy) return;
    setBusy(true);
    try {
      const value = await context.remote.method<Request, Snapshot>('settings')({
        kind: 'replace',
        snapshot: { revision: snapshot.revision, presets },
      });
      if (context.signal.aborted) return;
      setSnapshot(value);
      setDraft(undefined);
      setEditing(undefined);
      setError('');
    } catch (reason) {
      if (!context.signal.aborted) setError(String(reason));
    } finally {
      if (!context.signal.aborted) setBusy(false);
    }
  };
  return (
    <section data-maka-graph-presets>
      {error ? <p role="alert">{error}</p> : null}
      <button
        type="button"
        disabled={busy}
        onClick={() => {
          setDraft(undefined);
          setReload((value) => value + 1);
        }}
      >
        {t.reload}
      </button>
      {!snapshot ? (
        <p role="status">{t.loading}</p>
      ) : draft ? (
        <form
          onSubmit={(event) => {
            event.preventDefault();
            void save(
              editing
                ? snapshot.presets.map((value) => (value.id === editing ? draft : value))
                : [...snapshot.presets, draft],
            );
          }}
        >
          <fieldset disabled={busy}>
            {(['id', 'name', 'description', 'connectionSlug', 'model'] as const).map((field) => (
              <label key={field}>
                {t[field === 'connectionSlug' ? 'connection' : field]}
                <input
                  required={field !== 'description'}
                  disabled={field === 'id' && !!editing}
                  maxLength={field === 'description' ? 1000 : field === 'model' ? 512 : 128}
                  value={draft[field]}
                  onChange={(event) => setDraft({ ...draft, [field]: event.target.value })}
                />
              </label>
            ))}
            <label>
              {t.profile}
              <select
                value={draft.profile}
                onChange={(event) =>
                  setDraft({ ...draft, profile: event.target.value as Preset['profile'] })
                }
              >
                {(['local_read', 'web_research', 'implementation'] as const).map((value) => (
                  <option key={value} value={value}>
                    {t[value]}
                  </option>
                ))}
              </select>
            </label>
            <label>
              {t.thinking}
              <select
                value={draft.thinkingLevel ?? ''}
                onChange={(event) =>
                  setDraft({
                    ...draft,
                    thinkingLevel: (event.target.value as Preset['thinkingLevel']) || null,
                  })
                }
              >
                <option value="">{t.inherited}</option>
                {['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'].map((value) => (
                  <option key={value}>{value}</option>
                ))}
              </select>
            </label>
            <label>
              <input
                type="checkbox"
                checked={draft.enabled}
                onChange={(event) => setDraft({ ...draft, enabled: event.target.checked })}
              />
              {t.enabled}
            </label>
            <button type="submit">{t.save}</button>
            <button type="button" onClick={() => setDraft(undefined)}>
              {t.cancel}
            </button>
          </fieldset>
        </form>
      ) : (
        <>
          {!snapshot.presets.length ? (
            <p>{t.empty}</p>
          ) : (
            <ul>
              {snapshot.presets.map((preset) => (
                <li key={preset.id}>
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => {
                      setEditing(preset.id);
                      setDraft({ ...preset });
                    }}
                  >
                    {preset.name}
                  </button>
                  <span>
                    {preset.connectionSlug} / {preset.model} · {t[preset.profile]}
                    {preset.enabled ? '' : ' (off)'}
                  </span>
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() =>
                      void save(snapshot.presets.filter((value) => value.id !== preset.id))
                    }
                  >
                    {t.remove}
                  </button>
                </li>
              ))}
            </ul>
          )}
          <button
            type="button"
            disabled={busy || snapshot.presets.length >= 64}
            onClick={() => {
              setEditing(undefined);
              setDraft({
                id: '',
                name: '',
                description: '',
                profile: 'local_read',
                connectionSlug: '',
                model: '',
                thinkingLevel: null,
                enabled: true,
              });
            }}
          >
            {t.add}
          </button>
        </>
      )}
    </section>
  );
}
