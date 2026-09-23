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
  const zh = locale !== 'en';
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
    const version = generation.current;
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
      <h3>{zh ? '任务回顾' : 'Session recap'}</h3>
      <p>
        {zh
          ? '根据会话历史生成一句回顾，使用本会话选定的模型。'
          : 'Summarize this conversation in one sentence using its selected model.'}
      </p>
      {recap?.kind === 'ready' && (
        <>
          <p>{recap.text}</p>
          <small>{recap.modelId}</small>
        </>
      )}
      {retry && !busy && (
        <p role="status">
          {zh
            ? '结果尚未确认，请先查询原请求。生成新回顾可能产生额外模型费用。'
            : 'The result is unconfirmed. Check the original request first; generating again may incur another model charge.'}
        </p>
      )}
      {recap?.kind === 'failed' && (
        <p role="status">
          {zh
            ? '未能生成完整回顾，请检查会话模型后重试。'
            : 'A complete recap could not be generated. Check the session model and try again.'}
        </p>
      )}
      {error && <p role="alert">{error}</p>}
      <div>
        {retry && (
          <button disabled={busy} onClick={() => void generate(retry)}>
            {zh ? '查询原请求' : 'Check original request'}
          </button>
        )}
        <button disabled={busy} onClick={() => void generate(crypto.randomUUID())}>
          {busy ? (zh ? '处理中…' : 'Working…') : zh ? '生成新回顾' : 'Generate new recap'}
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
