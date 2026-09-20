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
import type { ClientContext, ClientSlots } from '@maka-agent/plugin-sdk/client';
import { copy, type Query, type Reply, type ResultPage } from './model.js';

export function WorkResult(props: {
  context: ClientContext;
  sessionId: string;
  graphId: string;
  workId: string;
  recordId: string;
  part?: 'answer' | 'patch';
  locale: ClientSlots['session.composer.before']['locale'];
}) {
  const [page, setPage] = useState<ResultPage>();
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState(false);
  const lifetime = useRef({ active: true, busy: false });
  useEffect(() => {
    const state = lifetime.current;
    state.active = true;
    return () => {
      state.active = false;
    };
  }, []);
  const t = copy[props.locale];
  const part = props.part ?? 'answer';
  const load = async () => {
    const state = lifetime.current;
    if (state.busy || !state.active) return;
    state.busy = true;
    setBusy(true);
    const offset = page?.nextOffset ?? 0;
    try {
      const response = await props.context.remote.method<Query, Reply>(
        'query',
        props.sessionId,
      )({
        kind: 'result',
        graphId: props.graphId,
        workId: props.workId,
        recordId: props.recordId,
        offset,
        part,
      });
      if (!state.active) return;
      if (
        response.kind !== 'result' ||
        !response.result ||
        response.result.recordId !== props.recordId ||
        response.result.graphId !== props.graphId ||
        response.result.workId !== props.workId ||
        response.result.part !== part ||
        response.result.offset !== offset
      )
        throw new Error('Graph result is unavailable');
      const next = response.result;
      setPage((previous) => ({
        ...next,
        text: (offset ? (previous?.text ?? '') : '') + next.text,
      }));
      setFailed(false);
    } catch (error) {
      if (state.active) {
        console.error('Graph result read failed', error);
        setFailed(true);
      }
    } finally {
      state.busy = false;
      if (state.active) setBusy(false);
    }
  };
  return (
    <details
      onToggle={(event) => {
        if (event.currentTarget.open && !page) void load();
      }}
    >
      <summary>{part === 'patch' ? t.patch : t.result}</summary>
      {page ? (
        part === 'patch' ? (
          <pre>{page.text}</pre>
        ) : (
          <p>{page.text}</p>
        )
      ) : !failed ? (
        <p>{t.loading}</p>
      ) : null}
      {failed ? <p role="status">{part === 'patch' ? t.patchUnavailable : t.failed}</p> : null}
      {failed || (page && page.nextOffset !== null) ? (
        <button type="button" disabled={busy} onClick={() => void load()}>
          {failed ? t.retry : t.more}
        </button>
      ) : null}
      {part === 'answer' && page?.isolatedWorkspace ? <WorkResult {...props} part="patch" /> : null}
    </details>
  );
}
