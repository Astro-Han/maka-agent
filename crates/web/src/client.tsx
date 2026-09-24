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

type Settings = { enabled: boolean; source: 'model' | 'tavily' };
type Check = {
  credential_revision: number;
  outcome: 'valid' | 'invalid_credentials' | 'rate_limited' | 'network_error' | 'timeout';
};
type Snapshot = {
  revision: number | null;
  settings: Settings;
  credential: { revision: number | null; configured: boolean; check: Check | null };
};
type Results = {
  query: string;
  truncated: boolean;
  omittedResults: number;
  rows: { title: string; url: string; snippet: string; truncated: boolean }[];
};
type Request =
  | { kind: 'read' }
  | { kind: 'configure'; expectedRevision: number | null; settings: Settings }
  | { kind: 'credential'; expectedRevision: number | null; secret: string | null }
  | { kind: 'test'; operationId: string }
  | { kind: 'search'; operationId: string; query: { query: string; limit: number } };
type Response =
  | { kind: 'snapshot'; snapshot: Snapshot }
  | { kind: 'saved'; revision: number }
  | {
      kind: 'credential';
      receipt: { kind: 'written'; revision: number } | { kind: 'conflict'; actual: number | null };
    }
  | { kind: 'test'; check: Check }
  | { kind: 'search'; results: Results };

function Manage({
  context,
  locale,
}: ClientSlots['application.manage'] & { context: ClientContext }) {
  const t = copy[locale];
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [secret, setSecret] = useState('');
  const [query, setQuery] = useState('');
  const [results, setResults] = useState<Results | null>(null);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const lifetime = useRef(0);
  const active = useRef(false);
  const call = context.remote.method<Request, Response>('request');

  useEffect(() => {
    const version = ++lifetime.current;
    void call({ kind: 'read' })
      .then((response) => {
        if (version === lifetime.current && response.kind === 'snapshot')
          setSnapshot(response.snapshot);
      })
      .catch((reason) => {
        if (version === lifetime.current) setError(String(reason));
      });
    return () => {
      lifetime.current++;
    };
  }, [context]);

  async function run(request: Request) {
    if (active.current) return;
    active.current = true;
    setBusy(true);
    setError('');
    const version = lifetime.current;
    try {
      const result = await call(request);
      if (version !== lifetime.current || context.signal.aborted) return;
      if (result.kind === 'snapshot') setSnapshot(result.snapshot);
      if (result.kind === 'saved' && request.kind === 'configure') {
        setSnapshot(
          (current) =>
            current && { ...current, revision: result.revision, settings: request.settings },
        );
      }
      if (result.kind === 'credential' && request.kind === 'credential') {
        if (result.receipt.kind === 'conflict') throw new Error(t.credentialChanged);
        const revision = result.receipt.revision;
        setSnapshot(
          (current) =>
            current && {
              ...current,
              credential: { revision, configured: !!request.secret?.trim(), check: null },
            },
        );
        setSecret('');
      }
      if (result.kind === 'test')
        setSnapshot(
          (current) =>
            current && {
              ...current,
              credential: {
                ...current.credential,
                check:
                  current.credential.revision === result.check.credential_revision
                    ? result.check
                    : null,
              },
            },
        );
      if (result.kind === 'search') setResults(result.results);
    } catch (reason) {
      if (version === lifetime.current)
        setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      active.current = false;
      if (version === lifetime.current) setBusy(false);
    }
  }
  const status = snapshot?.credential.check?.outcome;
  const labels = {
    valid: t.valid,
    invalid_credentials: t.invalidCredentials,
    rate_limited: t.rateLimited,
    network_error: t.networkError,
    timeout: t.timedOut,
  };
  return (
    <section data-maka-web-plugin>
      <header>
        <button type="button" disabled={busy} onClick={() => void run({ kind: 'read' })}>
          {t.refresh}
        </button>
      </header>
      <p>{t.description}</p>
      {error && <p role="alert">{error}</p>}
      {snapshot ? (
        <fieldset disabled={busy}>
          <label>
            <input
              type="checkbox"
              checked={snapshot.settings.enabled}
              onChange={(event) =>
                void run({
                  kind: 'configure',
                  expectedRevision: snapshot.revision,
                  settings: { ...snapshot.settings, enabled: event.target.checked },
                })
              }
            />
            {t.enable}
          </label>
          <label>
            {t.source}
            <select
              value={snapshot.settings.source}
              onChange={(event) =>
                void run({
                  kind: 'configure',
                  expectedRevision: snapshot.revision,
                  settings: {
                    ...snapshot.settings,
                    source: event.target.value === 'model' ? 'model' : 'tavily',
                  },
                })
              }
            >
              <option value="model">{t.nativeSearch}</option>
              <option value="tavily">Tavily</option>
            </select>
          </label>
          <p>{snapshot.settings.source === 'model' ? t.nativeDescription : t.tavilyDescription}</p>
          <label>
            {t.apiKey}
            <input
              type="password"
              autoComplete="new-password"
              value={secret}
              maxLength={4096}
              placeholder={snapshot.credential.configured ? t.savedKey : ''}
              onChange={(event) => setSecret(event.target.value)}
            />
          </label>
          <div className="web-actions">
            <button
              type="button"
              disabled={!secret.trim()}
              onClick={() =>
                void run({
                  kind: 'credential',
                  expectedRevision: snapshot.credential.revision,
                  secret,
                })
              }
            >
              {t.saveKey}
            </button>
            <button
              type="button"
              disabled={!snapshot.credential.configured}
              onClick={() =>
                void run({
                  kind: 'credential',
                  expectedRevision: snapshot.credential.revision,
                  secret: null,
                })
              }
            >
              {t.removeKey}
            </button>
            <button
              type="button"
              disabled={!snapshot.credential.configured}
              onClick={() => void run({ kind: 'test', operationId: crypto.randomUUID() })}
            >
              {t.testKey}
            </button>
          </div>
          <p role="status">
            {status
              ? labels[status]
              : snapshot.credential.configured
                ? t.notTested
                : t.noCredential}
          </p>
          <label>
            {t.trySearch}
            <input
              value={query}
              maxLength={200}
              onChange={(event) => setQuery(event.target.value)}
            />
          </label>
          <button
            type="button"
            disabled={
              !query.trim() ||
              !snapshot.settings.enabled ||
              snapshot.settings.source !== 'tavily' ||
              !snapshot.credential.configured
            }
            onClick={() =>
              void run({
                kind: 'search',
                operationId: crypto.randomUUID(),
                query: { query, limit: 5 },
              })
            }
          >
            {t.search}
          </button>
        </fieldset>
      ) : (
        <p>{t.loading}</p>
      )}
      {results && (
        <section>
          <h3>{results.query}</h3>
          {results.truncated && (
            <p role="status">
              {t.truncated}
              {results.omittedResults}
            </p>
          )}
          <ol>
            {results.rows.map((row, index) => (
              <li key={row.url + index}>
                <a href={row.url} target="_blank" rel="noopener noreferrer">
                  {row.title || row.url}
                </a>
                <p>
                  {row.snippet}
                  {row.truncated ? ' …' : ''}
                </p>
              </li>
            ))}
          </ol>
        </section>
      )}
    </section>
  );
}
const plugin: ClientPlugin = {
  activate(context) {
    context.style(`
      [data-maka-web-plugin] { max-width:760px; padding:24px; font:inherit; color:var(--foreground,inherit); }
      [data-maka-web-plugin] header, [data-maka-web-plugin] .web-actions { display:flex; gap:10px; align-items:center; flex-wrap:wrap; }
      [data-maka-web-plugin] h2 { flex:1; }
      [data-maka-web-plugin] fieldset { border:0; padding:0; display:grid; gap:16px; }
      [data-maka-web-plugin] label { display:flex; gap:10px; align-items:center; }
      [data-maka-web-plugin] input:not([type=checkbox]), [data-maka-web-plugin] select { flex:1; min-width:0; }
      [data-maka-web-plugin] input, [data-maka-web-plugin] select, [data-maka-web-plugin] button { font:inherit; color:inherit; background:var(--background-elevated,transparent); border:1px solid var(--border,#8884); border-radius:8px; padding:8px 12px; }
      [data-maka-web-plugin] button { cursor:pointer; }
      [data-maka-web-plugin] :disabled { opacity:.5; cursor:default; }
      [data-maka-web-plugin] :focus-visible { outline:2px solid var(--accent,#6aa5ff); outline-offset:2px; }
      [data-maka-web-plugin] p { white-space:pre-wrap; overflow-wrap:anywhere; line-height:1.5; }
      [data-maka-web-plugin] [role=alert] { color:var(--destructive,#c44); }
      [data-maka-web-plugin] li { margin:16px 0; }
    `);
    context.slots.register('application.manage', 'web', (props) =>
      props.section === 'search' ? <Manage {...props} context={context} /> : null,
    );
  },
};
export default plugin;
