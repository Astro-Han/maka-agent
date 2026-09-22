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

import { useEffect, useRef, useState, useSyncExternalStore } from 'react';
import type {
  ClientContext,
  ClientPlugin,
  ClientSlots,
  AuthorizationRequest,
} from '@maka-agent/plugin-sdk/client';
import type { SessionConfiguration } from '@maka-agent/plugin-sdk/host';
import type {
  ScheduledTask,
  CreateScheduledTaskInput,
  UpdateScheduledTaskInput,
  ScheduledTaskEffect,
} from '@maka/core/scheduled-task';
import { Button, ScheduledTasksPage, useToast } from '@maka/ui/plugin';
import { Tasks } from './client/tasks.js';

type Create = Omit<CreateScheduledTaskInput, 'createdBy'>;
type Mutation =
  | { kind: 'create'; input: Create }
  | { kind: 'update'; taskId: string; patch: UpdateScheduledTaskInput }
  | { kind: 'pause' | 'resume' | 'trigger_now' | 'clear_history' | 'delete'; taskId: string }
  | { kind: 'snooze'; taskId: string; delayMs: number };

async function authorize(
  context: ClientContext,
  effect: ScheduledTaskEffect,
  zh: boolean,
): Promise<string> {
  const target: AuthorizationRequest['target'] =
    effect.kind === 'notify'
      ? { kind: 'profile' }
      : effect.kind === 'session_resume'
        ? { kind: 'session', sessionId: effect.sessionId }
        : {
            kind: 'workspace',
            workspace: effect.execution.projectId
              ? { kind: 'project', projectId: effect.execution.projectId }
              : { kind: 'host_path', path: effect.execution.cwd },
            sandboxMode: effect.execution.sandboxMode,
          };
  const request: AuthorizationRequest = {
    operationId: crypto.randomUUID(),
    title: zh ? '允许定时任务在后台执行' : 'Allow scheduled background work',
    target,
    capabilities: effect.kind === 'notify' ? ['notifications'] : ['executions', 'notifications'],
  };
  const grant = await context.authorization.approve('profile', request);
  if (!grant || grant.revoked)
    throw new Error(zh ? '未获得后台授权' : 'Background authorization was not granted');
  await context.remote.method('request')({ kind: 'remember_grant', id: grant.id });
  return grant.id;
}

function Manage({
  context,
  tasks,
  locale,
  action,
}: ClientSlots['application.manage'] & { context: ClientContext; tasks: Tasks }) {
  const snapshot = useSyncExternalStore(tasks.subscribe, tasks.snapshot, tasks.snapshot);
  const toast = useToast();
  const zh = locale !== 'en';
  const [createNonce, setCreateNonce] = useState(0);
  const handled = useRef<number | undefined>(undefined);
  useEffect(() => {
    if (action?.name === 'create' && action.id !== handled.current) {
      handled.current = action.id;
      setCreateNonce(action.id);
      action.handled();
    }
  }, [action]);
  const call = context.remote.method<
    { kind: 'mutate'; mutation: Mutation; grant?: string },
    { kind: 'task'; task: ScheduledTask } | { kind: 'deleted'; taskId: string }
  >('request');
  const mutate = async (mutation: Mutation, effect?: ScheduledTaskEffect): Promise<boolean> => {
    try {
      const grant = effect ? await authorize(context, effect, zh) : undefined;
      await call({ kind: 'mutate', mutation, grant });
      await tasks.refresh();
      return true;
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
      return false;
    }
  };
  return (
    <section data-maka-scheduler-plugin>
      {snapshot.error && <p role="alert">{snapshot.error}</p>}
      <ScheduledTasksPage
        tasks={[...snapshot.tasks]}
        createRequestNonce={createNonce}
        onCreateRequestHandled={() => setCreateNonce(0)}
        onRefresh={() => tasks.refresh()}
        onCreate={(input) => mutate({ kind: 'create', input }, input.effect)}
        onUpdate={(taskId, patch) => mutate({ kind: 'update', taskId, patch }, patch.effect)}
        onToggle={async (taskId, enabled) => {
          await mutate(
            { kind: enabled ? 'resume' : 'pause', taskId },
            enabled ? snapshot.tasks.find((task) => task.id === taskId)?.effect : undefined,
          );
        }}
        onTriggerNow={async (taskId) => {
          await mutate({ kind: 'trigger_now', taskId });
        }}
        onSnooze={async (taskId) => {
          await mutate({ kind: 'snooze', taskId, delayMs: 10 * 60 * 1000 });
        }}
        onClearRunHistory={async (taskId) => {
          await mutate({ kind: 'clear_history', taskId });
        }}
        onDelete={async (taskId) => {
          if (
            await toast.confirm({
              title: zh ? '删除定时任务？' : 'Delete scheduled task?',
              description: zh
                ? '已接受的执行不会被取消。'
                : 'Already accepted executions will not be cancelled.',
            })
          )
            await mutate({ kind: 'delete', taskId });
        }}
      />
    </section>
  );
}

function Session({
  context,
  sessionId,
  locale,
}: ClientSlots['session.composer.before'] & { context: ClientContext }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const zh = locale !== 'en';
  const lifetime = useRef(0);
  useEffect(() => {
    lifetime.current++;
    return () => {
      lifetime.current++;
    };
  }, [sessionId]);
  const approve = async (root: boolean) => {
    if (busy) return;
    const current = lifetime.current;
    setBusy(true);
    setError('');
    try {
      if (!root) {
        await authorize(context, { kind: 'session_resume', sessionId }, zh);
      } else {
        const view = await context.remote.method<
          { kind: 'session' },
          { [K in keyof SessionConfiguration]: SessionConfiguration[K] }
        >(
          'request',
          sessionId,
        )({ kind: 'session' });
        if (view.target.kind !== 'model')
          throw new Error(zh ? '此会话没有模型' : 'This Session has no model');
        await authorize(
          context,
          {
            kind: 'agent_run',
            execution: {
              cwd: view.workspace.hostCwd,
              ...(view.workspace.target.kind === 'project'
                ? { projectId: view.workspace.target.projectId }
                : {}),
              llmConnectionId: view.target.model.connection_id,
              llmConnectionSlug: view.target.model.connection_slug,
              model: view.target.model.model,
              ...(view.target.thinkingLevel ? { thinkingLevel: view.target.thinkingLevel } : {}),
              sandboxMode: view.sandboxMode,
              collaborationMode: view.collaborationMode,
              orchestrationMode: view.behavior,
            },
          },
          zh,
        );
      }
    } catch (error) {
      if (lifetime.current === current)
        setError(error instanceof Error ? error.message : String(error));
    } finally {
      if (lifetime.current === current) setBusy(false);
    }
  };
  return (
    <details data-maka-scheduler-consent>
      <summary>{zh ? '定时任务后台权限' : 'Scheduled background access'}</summary>
      <Button
        isDisabled={busy}
        onClick={() => void approve(false)}
        label={zh ? '允许继续此会话' : 'Allow resuming this Session'}
      />
      <Button
        isDisabled={busy}
        onClick={() => void approve(true)}
        label={zh ? '允许在此工作区创建会话' : 'Allow new Sessions in this workspace'}
      />
      {error && <p role="alert">{error}</p>}
    </details>
  );
}

const plugin: ClientPlugin = {
  activate(context) {
    const tasks = new Tasks(context);
    context.effect(() => tasks.start());
    context.slots.register('application.manage', 'scheduler', (props) =>
      props.section === 'scheduled-tasks' ? (
        <Manage {...props} context={context} tasks={tasks} />
      ) : null,
    );
    context.slots.register('session.composer.before', 'consent', (props) => (
      <Session {...props} context={context} />
    ));
    context.slots.register('navigation.status', 'scheduler', (props) => {
      const state = useSyncExternalStore(tasks.subscribe, tasks.snapshot, tasks.snapshot);
      if (props.section !== 'automations') return null;
      const count = state.tasks.filter((task) => task.status === 'active').length;
      return count ? (
        <span aria-label={props.locale === 'en' ? 'Active scheduled tasks' : '活动定时任务'}>
          {count}
        </span>
      ) : null;
    });
  },
};
export default plugin;
