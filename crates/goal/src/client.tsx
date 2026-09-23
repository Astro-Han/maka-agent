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
type Current = {
  revision: number;
  goal: {
    id: string;
    status: string;
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
  | { kind: 'control'; id: string; revision: number; action: string; grant?: string };
function GoalView({
  context,
  sessionId,
  locale,
}: ClientSlots['session.inspector.overview'] & { context: ClientContext }) {
  const zh = locale !== 'en';
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
  async function act(action: string) {
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
            throw new Error(
              zh
                ? '请填写目标、1–100轮及有效 token 阈值'
                : 'Enter an objective, 1–100 iterations and a valid token threshold',
            );
          const grant = await context.authorization.approve('profile', {
            operationId: crypto.randomUUID(),
            title: zh
              ? '允许此目标在会话中自动续跑'
              : 'Allow this Goal to continue in this Session',
            target: { kind: 'session', sessionId },
            capabilities: ['executions', 'read_usage'],
          });
          if (version !== epoch.current || context.signal.aborted) return;
          if (!grant || grant.revoked)
            throw new Error(zh ? '未获得后台授权' : 'Background access was not granted');
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
            title: zh ? '重新授权此目标续跑' : 'Renew Goal background access',
            target: { kind: 'session', sessionId },
            capabilities: ['executions', 'read_usage'],
          });
          if (version !== epoch.current || context.signal.aborted) return;
          if (!grant || grant.revoked)
            throw new Error(zh ? '未获得授权' : 'Access was not granted');
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
  const statuses: Record<string, string> = {
    armed: '待启动',
    active: '执行中',
    paused: '已暂停',
    waiting: '等待输入',
    achieved: '已完成',
    impossible: '无法完成',
    cancelled: '已取消',
    cancellation_unknown: '取消待核对',
    max_iterations: '达到轮数上限',
    budget_limited: '达到用量阈值',
    budget_unknown: '用量不完整',
    blocked: '需要处理执行问题',
  };
  return (
    <section data-maka-goal>
      <h3>Goal</h3>
      <p>
        {zh
          ? '明确目标后允许自动续跑。模型报告完成后，还需本轮执行正常结束。'
          : 'Authorize automatic continuation toward an objective. Model completion reports apply after the execution ends successfully.'}
      </p>
      {current && (
        <>
          <p>{current.goal.arm.objective}</p>
          <p>
            {zh ? (statuses[current.goal.status] ?? current.goal.status) : current.goal.status} ·{' '}
            {current.goal.iterations}/{current.goal.arm.maxIterations}
          </p>
          <p>{current.goal.note}</p>
          <small>
            {zh ? '本会话新增已观测 token' : 'Observed additional Session tokens'}:{' '}
            {current.goal.consumed.known}
            {current.goal.consumed.missing > 0 ? ' (incomplete)' : ''}
          </small>
          {current.goal.pending && (
            <p>
              {zh
                ? '当前一轮尚未结算；暂停只停止后续轮次。取消会按原操作 ID 等待结果，已进入派发的请求可能先被 Host 接受。'
                : 'The current iteration is unsettled. Pause stops later iterations; cancellation waits on the original operation, which may still be admitted if dispatch already began.'}
            </p>
          )}
          <div>
            {['cancelled', 'cancellation_unknown'].includes(current.goal.status) &&
              current.goal.pending && (
                <button disabled={busy} onClick={() => void act('retry_cancel')}>
                  {zh
                    ? '重查取消状态（必要时重新授权）'
                    : 'Check cancellation (renew access if needed)'}
                </button>
              )}
            {!terminal && (
              <>
                <button
                  disabled={busy || current.goal.status === 'paused'}
                  onClick={() => void act('pause')}
                >
                  {zh ? '暂停' : 'Pause'}
                </button>
                <button
                  disabled={
                    busy || !['armed', 'paused', 'waiting', 'blocked'].includes(current.goal.status)
                  }
                  onClick={() => void act('resume')}
                >
                  {zh ? '启动／继续' : 'Start / resume'}
                </button>
                <button disabled={busy} onClick={() => void act('cancel')}>
                  {zh ? '取消目标' : 'Cancel goal'}
                </button>
                <button
                  disabled={busy || !!current.goal.pending}
                  onClick={() => void act('complete')}
                >
                  {zh ? '标记完成' : 'Mark complete'}
                </button>
              </>
            )}
          </div>
        </>
      )}
      {canCreate && (
        <>
          <label>
            {zh ? '目标' : 'Objective'}
            <textarea
              value={objective}
              onChange={(e) => setObjective(e.target.value)}
              disabled={busy || !!retry}
            />
          </label>
          <label>
            {zh ? '最多续跑轮数' : 'Maximum iterations'}
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
            {zh ? '本会话新增 token 阈值（可选）' : 'Additional Session token threshold (optional)'}
            <input
              type="number"
              min="1"
              value={budget}
              onChange={(e) => setBudget(e.target.value)}
              disabled={busy || !!retry}
            />
          </label>
          <small>
            {zh
              ? '包含创建目标后的其他会话活动；达到已观测阈值或用量缺失时停止续跑，不是单次请求的硬上限。'
              : 'Includes other Session activity since Goal creation. Stops continuation after the observed threshold or missing usage; not a hard request limit.'}
          </small>
          <div>
            <button disabled={busy || !!retry} onClick={() => void act('arm')}>
              {zh ? '保存，稍后启动' : 'Save for later'}
            </button>
            <button disabled={busy || !!retry} onClick={() => void act('start')}>
              {zh ? '授权并启动' : 'Authorize and start'}
            </button>
          </div>
        </>
      )}
      {retry && (
        <button disabled={busy} onClick={() => void act('retry')}>
          {zh ? '查询／重试原请求' : 'Retry original request'}
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
