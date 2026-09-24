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
import type { ClientPlugin, ClientContext, ClientSlots } from '@maka-agent/plugin-sdk/client';
type Recap =
  | { kind: 'pending'; operationId: string; through: number }
  | { kind: 'ready'; operationId: string; through: number; text: string; modelId: string }
  | { kind: 'failed'; operationId: string; through: number; reason: string };
type Request = { kind: 'read' } | { kind: 'generate'; operationId: string };
function RecapView({
  context,
  sessionId,
  locale,
}: ClientSlots['session.inspector.overview'] & { context: ClientContext }) {
  const t = copy[locale];
  const call = useMemo(
    () => context.remote.method<Request, { recap: Recap | null }>('request', sessionId),
    [context, sessionId],
  );
  const [recap, setRecap] = useState<Recap | null>(null);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const [retry, setRetry] = useState<string | null>(null);
  const generation = useRef(0);
  const active = useRef(false);
  useEffect(() => {
    const version = ++generation.current;
    active.current = false;
    setBusy(false);
    setRecap(null);
    setError('');
    setRetry(null);
    void call({ kind: 'read' })
      .then((r) => {
        if (version === generation.current && !context.signal.aborted) {
          setRecap(r.recap);
          setRetry(r.recap?.kind === 'pending' ? r.recap.operationId : null);
        }
      })
      .catch((e) => {
        if (version === generation.current && !context.signal.aborted) setError(String(e));
      });
    return () => {
      generation.current++;
    };
  }, [call, context]);
  async function generate(operationId: string) {
    if (active.current) return;
    active.current = true;
    setBusy(true);
    setError('');
    setRetry(operationId);
    const version = ++generation.current;
    try {
      const r = await call({ kind: 'generate', operationId });
      if (version !== generation.current || context.signal.aborted) return;
      setRecap(r.recap);
      if (r.recap?.kind !== 'pending') setRetry(null);
    } catch (e) {
      if (version === generation.current && !context.signal.aborted) setError(String(e));
    } finally {
      if (version === generation.current) {
        active.current = false;
        setBusy(false);
      }
    }
  }
  return (
    <section data-maka-recap>
      <h3>{t.title}</h3>
      <p>{t.description}</p>
      {recap?.kind === 'ready' && (
        <>
          <p>{recap.text}</p>
          <small>{recap.modelId}</small>
        </>
      )}
      {retry && !busy && <p role="status">{t.unconfirmed}</p>}
      {recap?.kind === 'failed' && <p role="status">{t.failed}</p>}
      {error && <p role="alert">{error}</p>}
      <div>
        {retry && (
          <button disabled={busy} onClick={() => void generate(retry)}>
            {t.checkOriginal}
          </button>
        )}
        <button disabled={busy} onClick={() => void generate(crypto.randomUUID())}>
          {busy ? t.working : t.generate}
        </button>
      </div>
    </section>
  );
}
const plugin: ClientPlugin = {
  activate(context) {
    context.style(
      '[data-maka-recap]{display:grid;gap:10px;font:inherit;color:inherit}[data-maka-recap] p{white-space:pre-wrap;margin:0}[data-maka-recap] div{display:flex;gap:8px}[data-maka-recap] button{font:inherit;color:inherit;border:1px solid #8886;background:transparent;border-radius:6px;padding:7px}[data-maka-recap] [role=alert]{color:var(--destructive,#c44)}',
    );
    context.slots.register('session.inspector.overview', 'recap', (props) => (
      <RecapView {...props} context={context} />
    ));
  },
};
export default plugin;
