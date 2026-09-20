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
import { copy, type Detail, type Query, type Reply, type Work } from './model.js';
import { WorkResult } from './result.js';

export function WorkDetails(props: {
  context: ClientContext;
  sessionId: string;
  graphId: string;
  work: Work;
  locale: ClientSlots['session.composer.before']['locale'];
}) {
  const [detail, setDetail] = useState<Detail>();
  const [opened, setOpened] = useState(false);
  const [failure, setFailure] = useState(false);
  const [busy, setBusy] = useState(false);
  const lifetime = useRef({ active: true, busy: false });
  useEffect(() => {
    const state = lifetime.current;
    state.active = true;
    return () => {
      state.active = false;
    };
  }, []);
  const t = copy[props.locale];
  const load = async () => {
    const state = lifetime.current;
    if (state.busy || !state.active) return;
    state.busy = true;
    setBusy(true);
    const offset = detail?.nextOffset ?? 0;
    try {
      const result = await props.context.remote.method<Query, Reply>(
        'query',
        props.sessionId,
      )({
        kind: 'work',
        graphId: props.graphId,
        workId: props.work.workId,
        offset,
      });
      if (!state.active) return;
      if (
        result.kind !== 'work' ||
        !result.work ||
        result.work.workId !== props.work.workId ||
        result.work.offset !== offset
      )
        throw new Error('Graph work is unavailable');
      const page = result.work;
      setDetail((previous) => ({
        ...page,
        instruction: (offset ? (previous?.instruction ?? '') : '') + page.instruction,
      }));
      setFailure(false);
    } catch (error) {
      if (state.active) {
        console.error('Graph detail read failed', error);
        setFailure(true);
      }
    } finally {
      state.busy = false;
      if (state.active) setBusy(false);
    }
  };
  return (
    <>
      {!opened ? (
        <p>
          {props.work.instruction}
          {props.work.instructionTruncated ? <small> — {t.omitted}</small> : null}
        </p>
      ) : null}
      <details
        onToggle={(event) => {
          const open = event.currentTarget.open;
          setOpened(open);
          if (open && !detail) void load();
        }}
      >
        <summary>{t.details}</summary>
        {detail ? (
          <>
            <p>
              {t.target}:{' '}
              {detail.target.kind === 'agent'
                ? detail.target.agentId
                : detail.target.kind === 'preset'
                  ? detail.target.presetId
                  : detail.target.operatorId}
            </p>
            <p>{detail.instruction}</p>
            {detail.nextOffset !== null ? (
              <button type="button" disabled={busy} onClick={() => void load()}>
                {t.more}
              </button>
            ) : null}
            {detail.inputIds.length || detail.selectedResultInputs.length ? (
              <div>
                {t.inputs}:
                <ul>
                  {detail.inputIds.map((id) => (
                    <li key={id}>{id}</li>
                  ))}
                  {detail.selectedResultInputs.map((input) => (
                    <li key={input.sourceGraphId + '/' + input.resultId}>
                      {input.sourceGraphId} / {input.resultId}
                    </li>
                  ))}
                </ul>
              </div>
            ) : null}
            {detail.replaces ? (
              <p>
                {t.replaces}: {detail.replaces}
              </p>
            ) : null}
          </>
        ) : !failure ? (
          <p>{t.loading}</p>
        ) : null}
        {failure ? (
          <p role="status">
            {t.failed}{' '}
            <button type="button" disabled={busy} onClick={() => void load()}>
              {t.retry}
            </button>
          </p>
        ) : null}
      </details>
      {props.work.execution?.resultRecordId ? (
        <WorkResult
          key={props.work.execution.resultRecordId}
          context={props.context}
          sessionId={props.sessionId}
          graphId={props.graphId}
          workId={props.work.workId}
          recordId={props.work.execution.resultRecordId}
          locale={props.locale}
        />
      ) : null}
    </>
  );
}
