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

import { useEffect, useRef, useState } from 'react';
import type { ClientContext, ClientPlugin, ClientSlots } from '@maka-agent/plugin-sdk/client';
type Settings = { enabled: boolean; url: string; model: string; timeoutMs: number };
type Snapshot = {
  revision: number | null;
  settings: Settings;
  credentialRevision: number | null;
  configured: boolean;
  headerNames: string[];
};
type Request =
  | { kind: 'read' }
  | { kind: 'configure'; expectedRevision: number | null; settings: Settings }
  | {
      kind: 'credential';
      url: string;
      expectedRevision: number | null;
      secret: { apiKey: string | null; headers: Record<string, string> } | null;
    }
  | { kind: 'test'; operationId: string };
type Response =
  | { kind: 'snapshot'; snapshot: Snapshot }
  | { kind: 'tested'; answer: import('@maka-agent/plugin-sdk/host').Json };
function SettingsPage({
  context,
  locale,
}: ClientSlots['settings.page'] & { context: ClientContext }) {
  const t = copy[locale];
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [draft, setDraft] = useState<Settings | null>(null);
  const [secret, setSecret] = useState('');
  const [headers, setHeaders] = useState('{}');
  const [busy, setBusy] = useState(false);
  const [tested, setTested] = useState(false);
  const [error, setError] = useState('');
  const version = useRef(0);
  const active = useRef(false);
  const call = context.remote.method<Request, Response>('request');
  useEffect(() => {
    const generation = ++version.current;
    active.current = false;
    setBusy(false);
    setSnapshot(null);
    setDraft(null);
    setSecret('');
    setHeaders('{}');
    setTested(false);
    setError('');
    void call({ kind: 'read' })
      .then((r) => {
        if (generation === version.current && !context.signal.aborted && r.kind === 'snapshot') {
          setSnapshot(r.snapshot);
          setDraft(r.snapshot.settings);
        }
      })
      .catch((e) => {
        if (generation === version.current) setError(String(e));
      });
    return () => {
      version.current++;
    };
  }, [context]);
  async function run(request: Request) {
    if (active.current) return;
    active.current = true;
    setBusy(true);
    setError('');
    setTested(false);
    const generation = ++version.current;
    try {
      const result = await call(request);
      if (generation !== version.current || context.signal.aborted) return;
      if (result.kind === 'snapshot') {
        setSnapshot(result.snapshot);
        setDraft(result.snapshot.settings);
        setSecret('');
        setHeaders('{}');
      } else setTested(true);
    } catch (e) {
      if (generation === version.current && !context.signal.aborted) setError(String(e));
    } finally {
      if (generation === version.current) {
        active.current = false;
        setBusy(false);
      }
    }
  }
  const changed = draft && snapshot && JSON.stringify(draft) !== JSON.stringify(snapshot.settings);
  return (
    <section data-maka-jev>
      <p>{t.description}</p>
      {error && <p role="alert">{error}</p>}
      {tested && <p role="status">{t.testSucceeded}</p>}
      <button disabled={busy} onClick={() => void run({ kind: 'read' })}>
        {t.refresh}
      </button>
      {snapshot && draft && (
        <fieldset disabled={busy}>
          <label>
            <input
              type="checkbox"
              checked={draft.enabled}
              onChange={(e) => setDraft({ ...draft, enabled: e.target.checked })}
            />
            {t.enable}
          </label>
          <label>
            URL
            <input
              type="url"
              value={draft.url}
              maxLength={8192}
              onChange={(e) => setDraft({ ...draft, url: e.target.value })}
            />
          </label>
          <label>
            {t.model}
            <input
              value={draft.model}
              maxLength={256}
              onChange={(e) => setDraft({ ...draft, model: e.target.value })}
            />
          </label>
          <label>
            {t.timeout}
            <input
              type="number"
              min={100}
              max={60000}
              value={draft.timeoutMs}
              onChange={(e) => setDraft({ ...draft, timeoutMs: Number(e.target.value) })}
            />
          </label>
          <button
            disabled={!changed}
            onClick={() =>
              void run({ kind: 'configure', expectedRevision: snapshot.revision, settings: draft })
            }
          >
            {t.save}
          </button>
          <label>
            {t.apiKey}
            <input
              type="password"
              autoComplete="new-password"
              maxLength={4096}
              value={secret}
              placeholder={snapshot.configured ? t.saved : ''}
              onChange={(e) => setSecret(e.target.value)}
            />
          </label>
          <label>
            {t.headers}
            <textarea
              value={headers}
              maxLength={16000}
              onChange={(e) => setHeaders(e.target.value)}
            />
          </label>
          <p>
            {t.replaceWarning}
            {snapshot.headerNames.join(', ') || '—'}
          </p>
          <button
            disabled={!!changed}
            onClick={() => {
              try {
                const parsed: unknown = JSON.parse(headers);
                if (
                  !parsed ||
                  typeof parsed !== 'object' ||
                  Array.isArray(parsed) ||
                  Object.values(parsed).some((v) => typeof v !== 'string')
                )
                  throw new Error(t.invalidHeaders);
                void run({
                  kind: 'credential',
                  url: snapshot.settings.url,
                  expectedRevision: snapshot.credentialRevision,
                  secret: {
                    apiKey: secret.trim() || null,
                    headers: parsed as Record<string, string>,
                  },
                });
              } catch (e) {
                setError(String(e));
              }
            }}
          >
            {t.replaceCredentials}
          </button>
          <button
            disabled={!!changed || !snapshot.configured}
            onClick={() =>
              void run({
                kind: 'credential',
                url: snapshot.settings.url,
                expectedRevision: snapshot.credentialRevision,
                secret: null,
              })
            }
          >
            {t.removeKey}
          </button>
          <button
            disabled={!!changed || !snapshot.configured || !snapshot.settings.enabled}
            onClick={() => void run({ kind: 'test', operationId: crypto.randomUUID() })}
          >
            {t.test}
          </button>
        </fieldset>
      )}
    </section>
  );
}
const plugin: ClientPlugin = {
  activate(context) {
    context.style(
      `[data-maka-jev] { display:grid; gap:14px; max-width:760px; font:inherit; color:inherit; } [data-maka-jev] fieldset { display:grid; gap:12px; border:0; padding:0; } [data-maka-jev] label { display:flex; gap:10px; align-items:center; } [data-maka-jev] input:not([type=checkbox]) { flex:1; min-width:0; } [data-maka-jev] button,[data-maka-jev] input { font:inherit; color:inherit; background:transparent; border:1px solid #8886; border-radius:6px; padding:8px; } [data-maka-jev] :disabled { opacity:.5; } [data-maka-jev] [role=alert] { color:var(--destructive,#c44); }`,
    );
    context.slots.register(
      'settings.page',
      'jev',
      (props) => <SettingsPage {...props} context={context} />,
      { label: 'Jev', order: 36 },
    );
  },
};
export default plugin;
