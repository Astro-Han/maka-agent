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
type Status = keyof (typeof copy)['en']['statuses'];
type Control = 'pause' | 'resume' | 'cancel' | 'complete';
type Current = {
  revision: number;
  goal: {
    id: string;
    status: Status;
    iterations: number;
    pending: { dispatched: boolean } | null;
    note: string;
    authorityBlocked: boolean;
    consumed: { known: number; missing: number };
    arm: { objective: string; maxIterations: number; tokenBudget: number | null };
  };
};
type Arm = {
  operationId: string;
  objective: string;
  grant: string;
  maxIterations: number;
  tokenBudget: number | null;
  start: boolean;
};
type Request =
  | { kind: 'read' }
  | { kind: 'arm'; arm: Arm }
  | { kind: 'control'; id: string; revision: number; action: Control; grant?: string };
function GoalView({
  context,
  sessionId,
  locale,
}: ClientSlots['session.inspector.overview'] & { context: ClientContext }) {
  const t = copy[locale];
  const call = useMemo(
    () => context.remote.method<Request, { current: Current | null }>('request', sessionId),
    [context, sessionId],
  );
  const [current, setCurrent] = useState<Current | null>(null);
  const [objective, setObjective] = useState('');
  const [iterations, setIterations] = useState('10');
  const [budget, setBudget] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const [retry, setRetry] = useState<Arm | null>(null);
  const epoch = useRef(0);
  const locked = useRef(false);
  useEffect(() => {
    const version = ++epoch.current;
    setCurrent(null);
    setError('');
    setRetry(null);
    setBusy(false);
    locked.current = false;
    setObjective('');
    let stopped = false;
    let timer: ReturnType<typeof setTimeout>;
    async function refresh() {
      try {
        const r = await call({ kind: 'read' });
        if (!stopped && version === epoch.current && !context.signal.aborted)
          setCurrent((previous) =>
            previous && (!r.current || previous.revision > r.current.revision)
              ? previous
              : r.current,
          );
      } catch (e) {
        if (!stopped && version === epoch.current) setError(String(e));
      } finally {
        if (!stopped) timer = setTimeout(() => void refresh(), 2000);
      }
    }
    void refresh();
    return () => {
      stopped = true;
      epoch.current++;
      clearTimeout(timer);
    };
  }, [call, context]);
  async function act(action: Control | 'start' | 'arm' | 'retry' | 'retry_cancel') {
    if (locked.current) return;
    locked.current = true;
    setBusy(true);
    setError('');
    const version = epoch.current;
    try {
      let request: Request;
      if (action === 'start' || action === 'arm' || action === 'retry') {
        let arm = retry;
        if (action !== 'retry') {
          const count = Number(iterations),
            tokens = budget.trim() ? Number(budget) : null;
          if (
            !objective.trim() ||
            !Number.isInteger(count) ||
            count < 1 ||
            count > 100 ||
            (tokens !== null && (!Number.isSafeInteger(tokens) || tokens < 1 || tokens > 1e9))
          )
            throw new Error(t.invalidGoal);
          const grant = await context.authorization.approve('profile', {
            operationId: crypto.randomUUID(),
            title: t.authorizeContinuation,
            target: { kind: 'session', sessionId },
            capabilities: ['executions', 'read_usage'],
          });
          if (version !== epoch.current || context.signal.aborted) return;
          if (!grant || grant.revoked) throw new Error(t.backgroundDenied);
          arm = {
            operationId: crypto.randomUUID(),
            objective: objective.trim(),
            grant: grant.id,
            maxIterations: count,
            tokenBudget: tokens,
            start: action === 'start',
          };
          setRetry(arm);
        }
        if (!arm) return;
        request = { kind: 'arm', arm };
      } else {
        if (!current) return;
        let grantId: string | undefined;
        if (
          (action === 'resume' &&
            (current.goal.status === 'blocked' || current.goal.authorityBlocked)) ||
          (action === 'retry_cancel' && current.goal.authorityBlocked)
        ) {
          const grant = await context.authorization.approve('profile', {
            operationId: crypto.randomUUID(),
            title: t.renewAccess,
            target: { kind: 'session', sessionId },
            capabilities: ['executions', 'read_usage'],
          });
          if (version !== epoch.current || context.signal.aborted) return;
          if (!grant || grant.revoked) throw new Error(t.accessDenied);
          grantId = grant.id;
        }
        request = {
          kind: 'control',
          id: current.goal.id,
          revision: current.revision,
          action: action === 'retry_cancel' ? 'cancel' : action,
          ...(grantId ? { grant: grantId } : {}),
        };
      }
      const r = await call(request);
      if (version !== epoch.current || context.signal.aborted) return;
      setCurrent((previous) =>
        previous && (!r.current || previous.revision > r.current.revision) ? previous : r.current,
      );
      setRetry(null);
    } catch (e) {
      if (version === epoch.current) setError(String(e));
    } finally {
      if (version === epoch.current) {
        locked.current = false;
        setBusy(false);
      }
    }
  }
  const terminal =
    current &&
    [
      'achieved',
      'impossible',
      'cancelled',
      'cancellation_unknown',
      'max_iterations',
      'budget_limited',
      'budget_unknown',
    ].includes(current.goal.status);
  const canCreate = !current || (terminal && !current.goal.pending);
  return (
    <section data-maka-goal>
      <h3>{t.title}</h3>
      <p>{t.description}</p>
      {current && (
        <>
          <p>{current.goal.arm.objective}</p>
          <p>
            {t.statuses[current.goal.status]} · {current.goal.iterations}/
            {current.goal.arm.maxIterations}
          </p>
          <p>{current.goal.note}</p>
          <small>
            {t.observedTokens}: {current.goal.consumed.known}
            {current.goal.consumed.missing > 0 ? ` (${t.incomplete})` : ''}
          </small>
          {current.goal.pending && <p>{t.unsettled}</p>}
          <div>
            {['cancelled', 'cancellation_unknown'].includes(current.goal.status) &&
              current.goal.pending && (
                <button disabled={busy} onClick={() => void act('retry_cancel')}>
                  {t.checkCancellation}
                </button>
              )}
            {!terminal && (
              <>
                <button
                  disabled={busy || current.goal.status === 'paused'}
                  onClick={() => void act('pause')}
                >
                  {t.pause}
                </button>
                <button
                  disabled={
                    busy || !['armed', 'paused', 'waiting', 'blocked'].includes(current.goal.status)
                  }
                  onClick={() => void act('resume')}
                >
                  {t.resume}
                </button>
                <button disabled={busy} onClick={() => void act('cancel')}>
                  {t.cancel}
                </button>
                <button
                  disabled={busy || !!current.goal.pending}
                  onClick={() => void act('complete')}
                >
                  {t.complete}
                </button>
              </>
            )}
          </div>
        </>
      )}
      {canCreate && (
        <>
          <label>
            {t.objective}
            <textarea
              value={objective}
              onChange={(e) => setObjective(e.target.value)}
              disabled={busy || !!retry}
            />
          </label>
          <label>
            {t.iterations}
            <input
              type="number"
              min="1"
              max="100"
              value={iterations}
              onChange={(e) => setIterations(e.target.value)}
              disabled={busy || !!retry}
            />
          </label>
          <label>
            {t.tokenThreshold}
            <input
              type="number"
              min="1"
              value={budget}
              onChange={(e) => setBudget(e.target.value)}
              disabled={busy || !!retry}
            />
          </label>
          <small>{t.budgetDescription}</small>
          <div>
            <button disabled={busy || !!retry} onClick={() => void act('arm')}>
              {t.save}
            </button>
            <button disabled={busy || !!retry} onClick={() => void act('start')}>
              {t.start}
            </button>
          </div>
        </>
      )}
      {retry && (
        <button disabled={busy} onClick={() => void act('retry')}>
          {t.retry}
        </button>
      )}
      {error && <p role="alert">{error}</p>}
    </section>
  );
}
const plugin: ClientPlugin = {
  activate(context) {
    context.style(
      '[data-maka-goal]{display:grid;gap:10px;font:inherit;color:inherit}[data-maka-goal] p{margin:0;white-space:pre-wrap}[data-maka-goal] label{display:grid;gap:4px}[data-maka-goal] textarea{min-height:80px}[data-maka-goal] input,[data-maka-goal] textarea,[data-maka-goal] button{font:inherit;color:inherit;background:transparent;border:1px solid #8886;border-radius:6px;padding:7px}[data-maka-goal] div{display:flex;flex-wrap:wrap;gap:8px}[data-maka-goal] button:disabled{opacity:.5;cursor:default}[data-maka-goal] [role=alert]{color:var(--destructive,#c44)}',
    );
    context.slots.register('session.inspector.overview', 'goal', (props) => (
      <GoalView {...props} context={context} />
    ));
  },
};
export default plugin;
