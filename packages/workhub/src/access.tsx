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

import { useState } from 'react';
import { Button } from '@maka/ui/plugin';
import type {
  AuthorizationRequest,
  ClientContext,
  ClientSlots,
} from '@maka-agent/plugin-sdk/client';
import type { Executions } from '@maka-agent/plugin-sdk/host';

type Target = AuthorizationRequest['target'];
type Creation = {
  authorization: Target;
  settings: Parameters<Executions['createRoot']>[0]['settings'];
};

export async function authorize(
  context: ClientContext,
  target: Target,
  title: string,
): Promise<void> {
  const remembered = await context.remote.method<Target, string | null>('consent')(target);
  if (remembered) {
    const grant = await context.authorization.query('profile', remembered);
    if (grant && !grant.revoked) return;
  }
  const grant = await context.authorization.approve('profile', {
    operationId: crypto.randomUUID(),
    title,
    target,
    capabilities: target.kind === 'profile' ? ['read_sessions'] : ['executions'],
  });
  if (!grant || grant.revoked) throw new Error('WorkHub authorization was not granted');
  await context.remote.method('authorize')({ id: grant.id });
}

export function registerAccess(context: ClientContext): void {
  context.slots.register('workspace.manage', 'new-work', function Workspace(props) {
    const zh = props.locale !== 'en';
    return (
      <ConsentButton
        label={zh ? '使用此工作区创建 WorkHub 任务' : 'Create WorkHub tasks in this workspace'}
        action={async () => {
          const target: Target = {
            kind: 'workspace',
            workspace: props.workspace,
            sandboxMode: props.sandboxMode,
          };
          await authorize(
            context,
            target,
            zh ? '允许 WorkHub 在此工作区执行任务' : 'Allow WorkHub work in this workspace',
          );
          const creation = await context.remote.method<
            {
              authorization: Target;
              collaborationMode: ClientSlots['workspace.manage']['collaborationMode'];
            },
            Creation
          >('creation-template')({
            authorization: target,
            collaborationMode: props.collaborationMode,
          });
          await context.remote.method<Creation, null>('configure-creation')(creation);
        }}
      />
    );
  });
  context.slots.register('session.composer.before', 'target-access', function Session(props) {
    const zh = props.locale !== 'en';
    return (
      <ConsentButton
        label={zh ? '允许 WorkHub 协调此任务' : 'Allow WorkHub to coordinate this task'}
        action={() =>
          authorize(
            context,
            { kind: 'session', sessionId: props.sessionId },
            zh
              ? '允许 WorkHub 在此会话提交和管理任务'
              : 'Allow WorkHub to submit and manage work in this Session',
          )
        }
      />
    );
  });
}

function ConsentButton({ label, action }: { label: string; action: () => Promise<void> }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  return (
    <div>
      <Button
        label={label}
        isDisabled={busy}
        onClick={() => {
          setBusy(true);
          setError(undefined);
          void action()
            .catch((error: unknown) =>
              setError(error instanceof Error ? error.message : String(error)),
            )
            .finally(() => setBusy(false));
        }}
      />
      {error ? <p role="alert">{error}</p> : null}
    </div>
  );
}
