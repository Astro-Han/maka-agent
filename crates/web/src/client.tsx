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
  const zh = locale !== 'en';
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
        if (result.receipt.kind === 'conflict')
          throw new Error(
            zh ? '密钥已被修改，请刷新后重试。' : 'Credential changed. Refresh before retrying.',
          );
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
    valid: zh ? '验证成功' : 'Valid',
    invalid_credentials: zh ? '密钥无效' : 'Invalid credentials',
    rate_limited: zh ? '请求限流' : 'Rate limited',
    network_error: zh ? '网络或服务响应错误' : 'Network or provider response error',
    timeout: zh ? '请求超时' : 'Timed out',
  };
  return (
    <section data-maka-web-plugin>
      <header>
        <button type="button" disabled={busy} onClick={() => void run({ kind: 'read' })}>
          {zh ? '刷新' : 'Refresh'}
        </button>
      </header>
      <p>
        {zh
          ? '搜索来源由 Web 插件管理。隐私模式下不提供搜索和网页抓取。'
          : 'The Web plugin manages search sources. Search and fetching are disabled in incognito mode.'}
      </p>
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
            {zh ? '启用联网搜索' : 'Enable web search'}
          </label>
          <label>
            {zh ? '搜索来源' : 'Search source'}
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
              <option value="model">{zh ? '模型内置搜索' : 'Model-native search'}</option>
              <option value="tavily">Tavily</option>
            </select>
          </label>
          <p>
            {snapshot.settings.source === 'model'
              ? zh
                ? '使用当前模型协议支持的原生搜索；不支持时不会偷偷改用其他来源。'
                : 'Uses native search supported by the selected model protocol; never silently switches sources.'
              : zh
                ? '通过 Tavily 获取标题、来源链接和摘要。'
                : 'Fetches titles, source links and snippets through Tavily.'}
          </p>
          <label>
            {zh ? 'Tavily 密钥' : 'Tavily API key'}
            <input
              type="password"
              autoComplete="new-password"
              value={secret}
              maxLength={4096}
              placeholder={
                snapshot.credential.configured
                  ? zh
                    ? '已保存；输入新密钥以替换'
                    : 'Saved; enter a new key to replace'
                  : ''
              }
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
              {zh ? '保存密钥' : 'Save key'}
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
              {zh ? '删除密钥' : 'Remove key'}
            </button>
            <button
              type="button"
              disabled={!snapshot.credential.configured}
              onClick={() => void run({ kind: 'test', operationId: crypto.randomUUID() })}
            >
              {zh ? '测试已保存密钥' : 'Test saved key'}
            </button>
          </div>
          <p role="status">
            {status
              ? labels[status]
              : snapshot.credential.configured
                ? zh
                  ? '尚未验证'
                  : 'Not tested'
                : zh
                  ? '未配置密钥'
                  : 'No credential configured'}
          </p>
          <label>
            {zh ? '测试搜索' : 'Try a search'}
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
            {zh ? '搜索' : 'Search'}
          </button>
        </fieldset>
      ) : (
        <p>{zh ? '正在读取 Web 插件配置…' : 'Loading Web configuration…'}</p>
      )}
      {results && (
        <section>
          <h3>{results.query}</h3>
          {results.truncated && (
            <p role="status">
              {zh
                ? '部分结果或摘要已截断。省略结果数：'
                : 'Some results or snippets were truncated. Omitted results: '}
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
