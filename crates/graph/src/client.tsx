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

import {
  copy,
  states,
  type Cursor,
  type Epoch,
  type Graph,
  type Query,
  type Reply,
} from './client/model.js';
import { Presets } from './client/presets.js';
import { WorkDetails } from './client/work.js';
type Props = ClientSlots['session.composer.before'];

function Panel({ context, ...props }: Props & { context: ClientContext }) {
  const [epochs, setEpochs] = useState<Epoch[]>([]);
  const [before, setBefore] = useState<number | null>();
  const [currentEpoch, setCurrentEpoch] = useState<number | null>(null);
  const [selected, setSelected] = useState<string>();
  const [after, setAfter] = useState<Cursor | null>(null);
  const [graph, setGraph] = useState<Graph | null>(null);
  const [failure, setFailure] = useState(false);
  const [authorization, setAuthorization] = useState({ authorized: false, selected: false });
  const [busy, setBusy] = useState(false);
  const [retry, setRetry] = useState(0);
  const request = useRef(0);
  const t = copy[props.locale];

  useEffect(() => {
    setBusy(false);
    const lifetime = new AbortController();
    const query = context.remote.method<Query, Reply>('query', props.sessionId);
    const changes = context.remote.stream<null, null>('changes', props.sessionId);
    let running = false;
    let again = false;
    const refresh = async () => {
      again = true;
      if (running) return;
      running = true;
      try {
        do {
          again = false;
          const status = await context.remote.method<
            { kind: 'status' },
            { authorized: boolean; selected: boolean }
          >(
            'authorize',
            props.sessionId,
          )({ kind: 'status' });
          if (lifetime.signal.aborted) return;
          setAuthorization(status);
          const directory = await query({ kind: 'epochs', before: null });
          if (lifetime.signal.aborted) return;
          if (directory.kind !== 'epochs') throw new Error('Unexpected Graph directory');
          setEpochs((previous) => {
            const ids = new Set(directory.epochs.map((epoch) => epoch.graphId));
            return [...directory.epochs, ...previous.filter((epoch) => !ids.has(epoch.graphId))];
          });
          setBefore((previous) => (previous === undefined ? directory.nextBefore : previous));
          setCurrentEpoch(directory.currentEpoch);
          const target = selected ?? directory.epochs[0]?.graphId;
          if (!target) {
            setGraph(null);
            setFailure(false);
            return;
          }
          const result = await query({
            kind: 'snapshot',
            graphId: target,
            after,
          });
          if (lifetime.signal.aborted) return;
          if (result.kind !== 'snapshot') throw new Error('Unexpected Graph snapshot');
          setGraph(result.graph);
          setFailure(false);
        } while (again && !lifetime.signal.aborted);
      } catch (error) {
        if (!lifetime.signal.aborted) {
          console.error('Agent Graph read failed', error);
          setFailure(true);
        }
      } finally {
        running = false;
      }
    };
    void refresh();
    void (async () => {
      try {
        for await (const _ of changes(null, lifetime.signal)) await refresh();
      } catch (error) {
        if (!lifetime.signal.aborted) {
          console.error('Agent Graph stream failed', error);
          setFailure(true);
        }
      }
    })();
    return () => {
      lifetime.abort();
      request.current++;
    };
  }, [context, props.sessionId, props.contextRevision, selected, after, retry]);

  const act = async (action: () => Promise<() => void>) => {
    if (busy) return;
    const version = ++request.current;
    setBusy(true);
    try {
      const commit = await action();
      if (request.current === version) commit();
    } catch (error) {
      if (request.current === version) {
        console.error('Agent Graph action failed', error);
        setFailure(true);
      }
    } finally {
      if (request.current === version) setBusy(false);
    }
  };
  const panel = (
    <section data-maka-graph-plugin aria-label={t.title}>
      <header>
        <strong>{t.title}</strong>
        {!authorization.authorized ? (
          <button
            type="button"
            disabled={busy}
            onClick={() =>
              void act(async () => {
                const grant = await context.authorization.approve('profile', {
                  operationId: crypto.randomUUID(),
                  title:
                    props.locale === 'en'
                      ? 'Allow Agent Graph background work'
                      : '允许 Agent Graph 后台执行',
                  target: { kind: 'session', sessionId: props.sessionId },
                  capabilities: ['executions'],
                });
                if (!grant || grant.revoked)
                  throw new Error('Agent Graph authorization was not granted');
                await context.remote.method(
                  'authorize',
                  props.sessionId,
                )({ kind: 'remember', id: grant.id });
                return () => setRetry((value) => value + 1);
              })
            }
          >
            {props.locale === 'en' ? 'Authorize background execution' : '授权后台执行'}
          </button>
        ) : null}
        {epochs.length ? (
          <select
            aria-label={t.history}
            value={selected ?? epochs[0]?.graphId}
            onChange={(event) => {
              setSelected(event.target.value);
              setAfter(null);
            }}
          >
            {epochs.map((epoch) => (
              <option key={epoch.graphId} value={epoch.graphId}>
                {epoch.mode} #{epoch.epoch}
              </option>
            ))}
          </select>
        ) : null}
        {typeof before === 'number' ? (
          <button
            type="button"
            disabled={busy}
            onClick={() =>
              void act(async () => {
                const result = await context.remote.method<Query, Reply>(
                  'query',
                  props.sessionId,
                )({ kind: 'epochs', before });
                if (result.kind !== 'epochs') throw new Error('Unexpected Graph directory');
                return () => {
                  setEpochs((current) => [
                    ...current,
                    ...result.epochs.filter(
                      (entry) => !current.some((epoch) => epoch.graphId === entry.graphId),
                    ),
                  ]);
                  setBefore(result.nextBefore);
                };
              })
            }
          >
            {t.history}
          </button>
        ) : null}
        {graph?.epoch.epoch === currentEpoch && !graph.finished && !graph.stopRequested ? (
          <button
            type="button"
            disabled={busy}
            onClick={() =>
              void act(async () => {
                await context.remote.method<{ graphId: string }, null>(
                  'stop',
                  props.sessionId,
                )({ graphId: graph.epoch.graphId });
                return () => setRetry((value) => value + 1);
              })
            }
          >
            {t.stop}
          </button>
        ) : null}
      </header>
      {failure ? (
        <div role="status">
          {t.failed}{' '}
          <button
            type="button"
            onClick={() => {
              setAfter(null);
              setRetry((value) => value + 1);
            }}
          >
            {t.retry}
          </button>
        </div>
      ) : null}
      {graph ? (
        <>
          <p>
            {graph.finished ? t.finished : graph.stopRequested ? t.stopping : graph.epoch.mode}
            {' · '}
            {graph.work.length}/{graph.totalWork}
          </p>
          <ol>
            {graph.work.map((work) => (
              <li key={work.workId}>
                <strong>{states[props.locale][work.execution?.state ?? work.status]}</strong>
                <WorkDetails
                  key={graph.epoch.graphId + '/' + work.workId}
                  context={context}
                  sessionId={props.sessionId}
                  graphId={graph.epoch.graphId}
                  work={work}
                  locale={props.locale}
                />
                {work.execution ? (
                  <button
                    type="button"
                    onClick={() => props.onOpenSession(work.execution!.sessionId)}
                  >
                    {t.session}
                  </button>
                ) : null}
              </li>
            ))}
          </ol>
          {!graph.totalWork ? <p>{t.empty}</p> : null}
          {after ? (
            <button type="button" onClick={() => setAfter(null)}>
              {t.first}
            </button>
          ) : null}
          {graph.nextAfter ? (
            <button type="button" onClick={() => setAfter(graph.nextAfter)}>
              {t.next}
            </button>
          ) : null}
        </>
      ) : (
        <p>{epochs.length ? t.loading : t.empty}</p>
      )}
    </section>
  );
  return !epochs.length && !authorization.selected ? (
    <details>
      <summary>{t.title}</summary>
      {panel}
    </details>
  ) : (
    panel
  );
}

const plugin: ClientPlugin = {
  activate(context) {
    context.slots.register('application.manage', 'presets', (props) =>
      props.section === 'subagents' ? <Presets {...props} context={context} /> : null,
    );
    context.style(
      '[data-maka-graph-presets] fieldset{display:grid;gap:12px}[data-maka-graph-presets] label{display:flex;gap:8px;align-items:center}[data-maka-graph-presets] li{display:flex;gap:12px;align-items:center;margin-block:12px}',
    );
    context.style(
      '[data-maka-graph-plugin]{margin:8px 0;padding:12px;border:1px solid var(--border-color,#8884);border-radius:8px;max-height:40vh;overflow:auto}[data-maka-graph-plugin] header{display:flex;align-items:center;gap:8px;flex-wrap:wrap}[data-maka-graph-plugin] ol{padding-inline-start:24px}[data-maka-graph-plugin] li{margin-block:8px}[data-maka-graph-plugin] p{white-space:pre-wrap;overflow-wrap:anywhere;margin-block:6px}[data-maka-graph-plugin] small{opacity:.7}',
    );
    context.slots.register('session.composer.before', 'graph', (props) => (
      <Panel key={props.sessionId} {...props} context={context} />
    ));
  },
};
export default plugin;
