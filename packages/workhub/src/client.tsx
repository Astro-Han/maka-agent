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

import { useEffect, useMemo, useState } from 'react';
import { Button } from '@maka/ui/plugin';
import type { ClientPlugin, ClientSlots } from '@maka-agent/plugin-sdk/client';
import { authorize, registerAccess } from './access.js';
import { registerFeedback } from './feedback.js';
import { WorkHubRoot } from './surface.js';
import { coordinationCommands } from './client-session.js';
import { bindSurface } from './client-surface.js';
import { ModelSelection, type ModelTarget } from './model-selection.js';
import styles from './styles.css';

type Resolution = { sessionId: string };

const plugin: ClientPlugin = {
  activate(context) {
    context.style(`@layer components {\n${styles}\n}`);
    registerFeedback(context);
    registerAccess(context);
    context.slots.register('workhub.surface', 'conversation', function Surface(props) {
      const bound = useMemo(
        () =>
          bindSurface(
            props,
            coordinationCommands(context, props.hostAttachments, props.hostSessionId),
            context.signal,
          ),
        [
          props.sessions,
          props.native,
          props.attachments,
          props.contextUsage,
          props.hostAttachments,
          props.hostSessionId,
        ],
      );
      return <WorkHubRoot {...props} {...bound} signal={context.signal} />;
    });
    const resolve = context.remote.method<null, Resolution>('resolve');
    context.slots.register(
      'session.resolve',
      'coordinator',
      function Coordinator(props: ClientSlots['session.resolve']) {
        const [failed, setFailed] = useState(false);
        const [retry, setRetry] = useState(0);
        useEffect(() => {
          props.onResolving();
          return props.onResolving;
        }, [props.onResolving]);
        useEffect(() => {
          const observation = new AbortController();
          setFailed(false);
          void resolve(null)
            .then((outcome) => {
              if (observation.signal.aborted) return;
              props.onResolved(outcome.sessionId, observation.signal);
            })
            .catch((error: unknown) => {
              if (!observation.signal.aborted) {
                setFailed(true);
                props.onError(error instanceof Error ? error.message : String(error));
              }
            });
          return () => {
            observation.abort();
          };
        }, [props.contextRevision, props.onResolved, props.onError, retry]);
        return failed ? (
          <div>
            <Button
              label={props.locale === 'en' ? 'Retry' : props.locale === 'zh-TW' ? '重試' : '重试'}
              onClick={() => {
                props.onResolving();
                void (async () => {
                  const zh = props.locale !== 'en';
                  await authorize(
                    context,
                    { kind: 'plugin_workspace', sandboxMode: 'workspace-write' },
                    zh ? '启用 WorkHub 协调会话' : 'Enable the WorkHub coordinator',
                  );
                  await authorize(
                    context,
                    { kind: 'profile' },
                    zh ? '允许 WorkHub 发现任务' : 'Allow WorkHub to discover tasks',
                  );
                  setRetry((current) => current + 1);
                })().catch((error: unknown) =>
                  props.onError(error instanceof Error ? error.message : String(error)),
                );
              }}
            />
            <ModelSelection
              context={context}
              locale={props.locale}
              label={props.locale === 'en' ? 'Use this model' : '使用此模型'}
              onSelect={async (target) => {
                await authorize(
                  context,
                  { kind: 'plugin_workspace', sandboxMode: 'workspace-write' },
                  props.locale === 'en'
                    ? 'Enable the WorkHub coordinator'
                    : '启用 WorkHub 协调会话',
                );
                await authorize(
                  context,
                  { kind: 'profile' },
                  props.locale === 'en'
                    ? 'Allow WorkHub to discover tasks'
                    : '允许 WorkHub 发现任务',
                );
                await context.remote.method<ModelTarget, Resolution>('select-coordinator-model')(
                  target,
                );
                setRetry((current) => current + 1);
              }}
            />
          </div>
        ) : null;
      },
    );
  },
};
export default plugin;
