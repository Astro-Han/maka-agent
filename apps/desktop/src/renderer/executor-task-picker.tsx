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

import { useCallback, useState } from 'react';
import { Button, ExecutorSelection } from '@maka/ui/plugin';
import type { ExecutionTarget } from '@maka-agent/plugin-sdk/host';
import type { DesktopNewTaskTarget } from '../preload/bridge-contract.js';

export type ExecutorTarget = Extract<ExecutionTarget, { kind: 'executor' }>;

export function ExecutorTaskPicker({ target, locale, value, disabled, onChange }: {
  target: { kind: 'new'; host: DesktopNewTaskTarget } | { kind: 'session'; sessionId: string };
  locale: string;
  value?: ExecutorTarget;
  disabled: boolean;
  onChange(value?: ExecutorTarget): void | Promise<void>;
}) {
  const zh = locale !== 'en';
  const [open, setOpen] = useState(false);
  const host = target.kind === 'new' ? target.host : undefined;
  const sessionId = target.kind === 'session' ? target.sessionId : undefined;
  const search = useCallback((query: { query: string }) => sessionId
    ? window.maka.sessions.searchExecutors(sessionId, query)
    : window.maka.newTasks.searchExecutors(host!, query), [host?.profileId, host?.hostId, sessionId]);
  return (
    <details open={open} onToggle={(event) => setOpen(event.currentTarget.open)}>
      <summary>{value?.executorId ?? (zh ? '插件执行器' : 'Plugin executor')}</summary>
      {open ? <fieldset disabled={disabled}>
        {value && target.kind === 'new' ? <Button label={zh ? '使用内置模型运行时' : 'Use built-in model runtime'} onClick={() => { void onChange(); setOpen(false); }} /> : null}
        <ExecutorSelection locale={locale} label={zh ? '用于此任务' : 'Use for this task'} search={search} initialValue={value} onSelect={async (target) => {
          await onChange(target);
          setOpen(false);
        }} />
      </fieldset> : null}
    </details>
  );
}
