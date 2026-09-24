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

import { copy } from './client/copy.js';

import { useEffect, useMemo, useRef, useState } from 'react';
import type { ClientContext, ClientPlugin, ClientSlots } from '@maka-agent/plugin-sdk/client';
import type { ModelChoice, ModelChoices, SandboxMode } from '@maka-agent/plugin-sdk/host';
import {
  connect,
  type Catalog,
  type Copies,
  type Copy,
  type Entry,
  type Intent,
  type Snapshot,
} from './client/model.js';
import { Sources } from './client/sources.js';
import { History } from './client/history.js';

const modelKey = (choice: ModelChoice) => choice.model.connection_id + '/' + choice.model.model;
export function ImportPage({
  context,
  locale,
  onOpenSession,
}: ClientSlots['settings.page'] & { context: ClientContext }) {
  const t = copy[locale];
  const api = useMemo(() => connect(context), [context]);
  const epoch = useRef(0);
  const [snapshot, setSnapshot] = useState<Snapshot>();
  const [copies, setCopies] = useState<Copies>({ copies: [], next: null });
  const [models, setModels] = useState<ModelChoices>();
  const [sourceId, setSourceId] = useState('');
  const [model, setModel] = useState('');
  const [modelQuery, setModelQuery] = useState('');
  const [destination, setDestination] = useState('');
  const [sandbox, setSandbox] = useState<SandboxMode>('workspace-write');
  const [text, setText] = useState('');
  const [cwd, setCwd] = useState('');
  const [archived, setArchived] = useState(false);
  const [page, setPage] = useState<Catalog>();
  const [selected, setSelected] = useState<Entry>();
  const [pending, setPending] = useState<Intent>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [reload, setReload] = useState(0);
  const locked = busy || !!pending;
  useEffect(() => {
    setSnapshot(undefined);
    setCopies({ copies: [], next: null });
    setModels(undefined);
    setSourceId('');
    setModel('');
    setModelQuery('');
    setDestination('');
    setSandbox('workspace-write');
    setText('');
    setCwd('');
    setArchived(false);
    setPage(undefined);
    setSelected(undefined);
    setPending(undefined);
  }, [api]);
  useEffect(() => {
    const current = ++epoch.current;
    setBusy(true);
    setError('');
    setPage(undefined);
    setSelected(undefined);
    void Promise.allSettled([
      api({ kind: 'sources' }, 'sources'),
      api({ kind: 'copies', after: null }, 'copies'),
      api({ kind: 'models', query: { query: '' } }, 'models'),
    ])
      .then(([source, history, choices]) => {
        if (epoch.current !== current || context.signal.aborted) return;
        if (source.status === 'fulfilled') {
          setSnapshot(source.value.snapshot);
          setSourceId(source.value.snapshot.configuration.sources[0]?.id ?? '');
        }
        if (history.status === 'fulfilled') setCopies(history.value.page);
        if (choices.status === 'fulfilled') {
          setModels(choices.value.choices);
          const currentModel = choices.value.choices.models.find((choice) => choice.isDefault);
          setModel(currentModel ? modelKey(currentModel) : '');
        }
        setError(
          [source, history, choices]
            .filter((result) => result.status === 'rejected')
            .map((result) => String(result.reason))
            .join('\n'),
        );
      })
      .finally(() => {
        if (epoch.current === current) setBusy(false);
      });
    return () => {
      epoch.current++;
    };
  }, [api, reload, context.signal]);
  useEffect(() => {
    if (
      pending &&
      copies.copies.some((copy) => copy.operationId === pending.operationId && copy.receipt)
    ) {
      setPending(undefined);
      setSelected(undefined);
    }
  }, [copies, pending]);

  async function run(operation: () => Promise<() => void>) {
    const current = ++epoch.current;
    setBusy(true);
    setError('');
    try {
      const publish = await operation();
      if (epoch.current === current && !context.signal.aborted) publish();
    } catch (reason) {
      if (epoch.current === current && !context.signal.aborted) setError(String(reason));
    } finally {
      if (epoch.current === current && !context.signal.aborted) setBusy(false);
    }
  }
  function invalidate() {
    setPage(undefined);
    setSelected(undefined);
  }
  function catalog(cursor: string | null = null) {
    if (!snapshot?.revision || !sourceId) return;
    const revision = snapshot.revision;
    void run(async () => {
      const result = await api(
        {
          kind: 'catalog',
          sourceId,
          revision,
          query: {
            cwd: cwd.trim() || null,
            text,
            includeArchived: archived,
            limit: 20,
            cursor,
          },
        },
        'catalog',
      );
      return () => {
        setPage(result.page);
        setSelected(undefined);
      };
    });
  }
  function history(after: string | null) {
    void run(async () => {
      const result = await api({ kind: 'copies', after }, 'copies');
      return () => setCopies(result.page);
    });
  }
  function settle(copy: Copy, abandon: boolean) {
    void run(async () => {
      await api({ kind: abandon ? 'abandon' : 'deliver', operationId: copy.operationId }, 'copy');
      const result = await api({ kind: 'copies', after: null }, 'copies');
      return () => {
        setCopies(result.page);
        if (pending?.operationId === copy.operationId) {
          setPending(undefined);
          setSelected(undefined);
        }
      };
    });
  }
  function importSelection() {
    const choice = models?.models.find((choice) => modelKey(choice) === model);
    if (!pending && (!choice || !selected || !snapshot?.revision || !destination.trim())) return;
    const request: Intent = pending ?? {
      operationId: crypto.randomUUID(),
      selection: {
        sourceId,
        sourceRevision: snapshot!.revision!,
        sessionId: selected!.id,
        path: selected!.path,
      },
      workspace: { kind: 'host_path', path: destination.trim() },
      settings: {
        target: {
          kind: 'model',
          model: choice!.model,
          thinkingLevel: choice!.defaultThinkingLevel,
        },
        sandboxMode: sandbox,
        approvalPolicy: { kind: 'on-request' },
        collaborationMode: 'agent',
        behavior: 'default',
      },
    };
    setPending(request);
    void run(async () => {
      await api({ kind: 'prepare', request }, 'copy');
      await api({ kind: 'deliver', operationId: request.operationId }, 'copy');
      const result = await api({ kind: 'copies', after: null }, 'copies');
      return () => {
        setPending(undefined);
        setCopies(result.page);
        setSelected(undefined);
      };
    });
  }
  return (
    <section data-maka-import>
      <header>
        <h2>{t.title}</h2>
        <button type="button" disabled={busy} onClick={() => setReload((value) => value + 1)}>
          {t.refresh}
        </button>
      </header>
      <p>{t.description}</p>
      {error && <p role="alert">{error}</p>}
      {busy && <p role="status">{t.working}</p>}
      {snapshot && (
        <>
          <Sources
            snapshot={snapshot}
            busy={locked}
            locale={locale}
            save={(sources) => {
              void run(async () => {
                const result = await api(
                  {
                    kind: 'save_sources',
                    expectedRevision: snapshot.revision,
                    configuration: { sources },
                  },
                  'sources',
                );
                return () => {
                  setSnapshot(result.snapshot);
                  setSourceId(sources[0]?.id ?? '');
                  invalidate();
                };
              });
            }}
          />
          <form
            onSubmit={(event) => {
              event.preventDefault();
              catalog();
            }}
          >
            <label>
              {t.source}
              <select
                value={sourceId}
                disabled={locked}
                onChange={(event) => {
                  setSourceId(event.target.value);
                  invalidate();
                }}
              >
                <option value="">{t.chooseSource}</option>
                {snapshot.configuration.sources.map((source) => (
                  <option key={source.id} value={source.id}>
                    {source.name}
                  </option>
                ))}
              </select>
            </label>
            <label>
              {t.search}
              <input
                value={text}
                disabled={locked}
                onChange={(event) => {
                  setText(event.target.value);
                  invalidate();
                }}
              />
            </label>
            <label>
              {t.workspaceFilter}
              <input
                value={cwd}
                disabled={locked}
                onChange={(event) => {
                  setCwd(event.target.value);
                  invalidate();
                }}
              />
            </label>
            <label>
              <input
                type="checkbox"
                checked={archived}
                disabled={locked}
                onChange={(event) => {
                  setArchived(event.target.checked);
                  invalidate();
                }}
              />
              {t.includeArchived}
            </label>
            <button type="submit" disabled={locked || !sourceId}>
              {t.readCatalog}
            </button>
          </form>
          {page && (
            <fieldset disabled={locked}>
              <legend>{t.chooseConversation}</legend>
              {!page.entries.length && <p>{t.noConversations}</p>}
              {page.entries.map((entry) => (
                <label key={entry.id + entry.path} className="import-entry">
                  <input
                    type="radio"
                    name="import-selection"
                    checked={selected?.id === entry.id && selected.path === entry.path}
                    onChange={() => setSelected(entry)}
                  />
                  <span>
                    <strong>{entry.title}</strong>
                    <small>{entry.cwd ?? ''}</small>
                    <small>
                      {entry.updatedAt
                        ? new Date(entry.updatedAt).toLocaleString(locale)
                        : t.unknownTime}
                    </small>
                  </span>
                </label>
              ))}
              {page.next && (
                <button type="button" onClick={() => catalog(page.next)}>
                  {t.nextPage}
                </button>
              )}
            </fieldset>
          )}
          <fieldset disabled={locked}>
            <legend>{t.executionSettings}</legend>
            <label>
              {t.destinationWorkspace}
              <input
                required
                value={destination}
                onChange={(event) => setDestination(event.target.value)}
              />
            </label>
            <label>
              {t.findModel}
              <input value={modelQuery} onChange={(event) => setModelQuery(event.target.value)} />
            </label>
            <button
              type="button"
              onClick={() =>
                void run(async () => {
                  const result = await api(
                    { kind: 'models', query: { query: modelQuery } },
                    'models',
                  );
                  return () => {
                    setModels(result.choices);
                    setModel('');
                  };
                })
              }
            >
              {t.searchModels}
            </button>
            {models && !models.complete && <p>{t.incompleteModels}</p>}
            <label>
              {t.model}
              <select value={model} onChange={(event) => setModel(event.target.value)}>
                <option value="">{t.chooseModel}</option>
                {models?.models.map((choice) => (
                  <option key={modelKey(choice)} value={modelKey(choice)}>
                    {choice.connectionName} / {choice.displayName}
                  </option>
                ))}
              </select>
            </label>
            <label>
              {t.sandbox}
              <select
                value={sandbox}
                onChange={(event) => setSandbox(event.target.value as SandboxMode)}
              >
                <option value="read-only">{t.readOnly}</option>
                <option value="workspace-write">{t.workspaceWrite}</option>
                <option value="danger-full-access">{t.bypass}</option>
              </select>
            </label>
          </fieldset>
          <div className="import-actions">
            <button
              type="button"
              disabled={busy || (!pending && (!selected || !model || !destination.trim()))}
              onClick={importSelection}
            >
              {pending ? t.retry : t.create}
            </button>
            {pending && (
              <button type="button" disabled={busy} onClick={() => history(null)}>
                {t.savedImports}
              </button>
            )}
          </div>
          {pending && (
            <aside>
              <p>{t.retryDescription}</p>
              <button
                type="button"
                disabled={busy}
                onClick={() => {
                  setPending(undefined);
                  setSelected(undefined);
                }}
              >
                {t.setAside}
              </button>
            </aside>
          )}
        </>
      )}
      <History
        page={copies}
        busy={busy}
        locale={locale}
        next={history}
        settle={settle}
        open={
          onOpenSession
            ? (id) => {
                void run(async () => {
                  await onOpenSession(id);
                  return () => {};
                });
              }
            : undefined
        }
      />
    </section>
  );
}
const plugin: ClientPlugin = {
  activate(context) {
    context.style(`
      [data-maka-import]{display:grid;gap:16px;color:inherit;font:inherit}
      [data-maka-import] header,[data-maka-import] .import-actions{display:flex;align-items:center;gap:12px;flex-wrap:wrap}
      [data-maka-import] form,[data-maka-import] fieldset,[data-maka-import] details{display:grid;gap:10px;border:1px solid color-mix(in srgb,currentColor 20%,transparent);border-radius:8px;padding:14px}
      [data-maka-import] label{display:flex;align-items:center;gap:10px;flex-wrap:wrap}
      [data-maka-import] input:not([type=checkbox]):not([type=radio]),[data-maka-import] select{min-width:160px;flex:1;background:transparent;color:inherit;border:1px solid color-mix(in srgb,currentColor 25%,transparent);border-radius:5px;padding:7px}
      [data-maka-import] button{background:transparent;color:inherit;border:1px solid color-mix(in srgb,currentColor 25%,transparent);border-radius:5px;padding:6px 10px;cursor:pointer}
      [data-maka-import] button:disabled{opacity:.5;cursor:default}
      [data-maka-import] ul{padding:0;list-style:none;display:grid;gap:12px}
      [data-maka-import] li{display:flex;align-items:center;gap:10px;flex-wrap:wrap}
      [data-maka-import] code{overflow-wrap:anywhere}
      [data-maka-import] .import-entry span{display:grid;gap:3px}
      [data-maka-import] small{opacity:.7;overflow-wrap:anywhere}
      [data-maka-import] [role=alert]{color:#e56b6f}
    `);
    context.slots.register(
      'settings.page',
      'imports',
      (props) => <ImportPage {...props} context={context} />,
      {
        label: { en: 'Conversation import', 'zh-CN': '会话导入', 'zh-TW': '會話匯入' },
        order: 35,
      },
    );
  },
};
export default plugin;
