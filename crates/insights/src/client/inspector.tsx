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

import { useEffect, useMemo, useState } from 'react';
import type { ClientContext, ClientSlots } from '@maka-agent/plugin-sdk/client';
import type { UsageSummary } from '@maka-agent/plugin-sdk/host';
import { api as connect } from './model.js';
import { Totals } from './totals.js';

const relevant = new Set([
  'tool_start',
  'tool_result',
  'token_usage',
  'provider_retry',
  'error',
  'complete',
  'abort',
]);

export function Inspector({
  context,
  sessionId,
  locale,
}: ClientSlots['session.inspector.overview'] & {
  context: ClientContext;
}) {
  const api = useMemo(() => connect(context, sessionId), [context, sessionId]);
  const [summary, setSummary] = useState<UsageSummary>();
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const [reload, setReload] = useState(0);
  const zh = locale !== 'en';
  useEffect(() => {
    let live = true;
    let reading = false;
    let queued = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    setSummary(undefined);
    setError('');
    async function refresh() {
      if (!live || context.signal.aborted) return;
      if (reading) {
        queued = true;
        return;
      }
      reading = true;
      setBusy(true);
      try {
        const page = await api.activity({ kind: 'start', filter: { from: 0, to: Date.now() } });
        if (!live || context.signal.aborted) return;
        const next = await api.summary(page.cursor);
        if (live && !context.signal.aborted) {
          setSummary(next);
          setError('');
        }
      } catch (reason) {
        if (live && !context.signal.aborted) setError(String(reason));
      } finally {
        reading = false;
        if (live && !context.signal.aborted) {
          setBusy(false);
          if (queued) schedule();
        }
      }
    }
    function schedule() {
      queued = true;
      if (!live || reading || timer !== undefined) return;
      timer = setTimeout(() => {
        timer = undefined;
        queued = false;
        void refresh();
      }, 400);
    }
    const unsubscribe = context.events.subscribe(
      { kind: 'session.event', sessionId },
      (event) => {
        if (event.kind === 'session.event' && relevant.has(event.event.type)) schedule();
      },
      (reason) => {
        if (live) setError(String(reason));
      },
    );
    void refresh();
    return () => {
      live = false;
      clearTimeout(timer);
      unsubscribe();
    };
  }, [api, context, sessionId, reload]);
  return (
    <section data-maka-insights aria-label={zh ? '会话用量' : 'Session usage'}>
      <header>
        <h2>{zh ? '会话用量' : 'Session usage'}</h2>
        <button type="button" disabled={busy} onClick={() => setReload((value) => value + 1)}>
          {zh ? '刷新' : 'Refresh'}
        </button>
      </header>
      {error && (
        <p role="alert">
          {summary
            ? zh
              ? '刷新失败；以下是上次成功读取的快照。'
              : 'Refresh failed; showing the last successful snapshot.'
            : ''}
          {error}
        </p>
      )}
      {busy && !summary && <p role="status">{zh ? '正在读取用量…' : 'Loading usage…'}</p>}
      {summary && <Totals summary={summary} tab="overview" zh={zh} />}
    </section>
  );
}
