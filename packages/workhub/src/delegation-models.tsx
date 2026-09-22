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

import { useEffect, useState } from 'react';
import { Button } from '@maka/ui/plugin';
import type { AuthorizationRequest, ClientContext } from '@maka-agent/plugin-sdk/client';
import type { ExecutionTarget } from '@maka-agent/plugin-sdk/host';
import { authorize } from './access.js';
import { ModelSelection } from './model-selection.js';

type Page = {
  entries: { operationId: string; title: string; retired: boolean }[];
  nextAfter: string | null;
};
type Choice = {
  revision: number | null;
  authorization: AuthorizationRequest['target'];
  name: string;
  target: ExecutionTarget | null;
};
type Status = {
  recovery: { unavailable?: string; failures: { operationId: string; message: string }[] };
};

export function DelegationModels({
  context,
  locale,
  contextRevision,
}: {
  context: ClientContext;
  locale: string;
  contextRevision?: number;
}) {
  const zh = locale !== 'en';
  const [open, setOpen] = useState(false);
  const [after, setAfter] = useState<string>();
  const [page, setPage] = useState<Page>();
  const [status, setStatus] = useState<Status>();
  const [selected, setSelected] = useState('');
  const [choice, setChoice] = useState<{ operation: string; value: Choice }>();
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [refresh, setRefresh] = useState(0);
  useEffect(() => {
    let active = true;
    if (open)
      void Promise.all([
        context.remote.method<{ after?: string }, Page>('assignments')({ after }),
        context.remote.method<null, Status>('query')(null),
      ])
        .then(([page, status]) => {
          if (active) {
            setPage(page);
            setStatus(status);
          }
        })
        .catch((error: unknown) => {
          if (active) setError(error instanceof Error ? error.message : String(error));
        });
    return () => {
      active = false;
    };
  }, [context, open, after, refresh, contextRevision]);
  const operation =
    open && page?.entries.some((entry) => entry.operationId === selected && !entry.retired)
      ? selected
      : undefined;
  useEffect(() => {
    let active = true;
    setChoice((current) => (current?.operation === operation ? current : undefined));
    if (operation)
      void context.remote
        .method<{ assignmentId: string }, Choice>('delegation-model')({ assignmentId: operation })
        .then((value) => {
          if (active) setChoice({ operation, value });
        })
        .catch((error: unknown) => {
          if (active) setError(error instanceof Error ? error.message : String(error));
        });
    return () => {
      active = false;
    };
  }, [context, operation, refresh]);
  const current = choice?.operation === operation ? choice : undefined;
  return (
    <details open={open} onToggle={(event) => setOpen(event.currentTarget.open)}>
      <summary>{zh ? '委派模型与恢复' : 'Delegated models and recovery'}</summary>
      {open ? (
        <div className="workhub-model-selection">
          {status?.recovery.unavailable ? <p role="alert">{status.recovery.unavailable}</p> : null}
          {status?.recovery.failures.map((failure) => (
            <p role="alert" key={failure.operationId}>
              {failure.message}
            </p>
          ))}
          <label>
            {zh ? '委派任务' : 'Delegated task'}
            <select
              disabled={busy}
              value={operation ?? ''}
              onChange={(event) => setSelected(event.target.value)}
            >
              <option value="">{zh ? '选择委派任务' : 'Choose a delegated task'}</option>
              {page?.entries
                .filter((entry) => !entry.retired)
                .map((entry) => (
                  <option key={entry.operationId} value={entry.operationId}>
                    {entry.title}
                  </option>
                ))}
            </select>
          </label>
          <Button
            label={zh ? '刷新' : 'Refresh'}
            isDisabled={busy}
            onClick={() => setRefresh((value) => value + 1)}
          />
          {after ? (
            <Button
              isDisabled={busy}
              label={zh ? '首页' : 'First page'}
              onClick={() => {
                setSelected('');
                setAfter(undefined);
              }}
            />
          ) : null}
          {page?.nextAfter ? (
            <Button
              isDisabled={busy}
              label={zh ? '下一页' : 'Next page'}
              onClick={() => {
                setSelected('');
                setAfter(page.nextAfter!);
              }}
            />
          ) : null}
          {error ? <p role="alert">{error}</p> : null}
          {current ? (
            <>
              <p>{current.value.name}</p>
              <ModelSelection
                key={current.operation}
                context={context}
                locale={locale}
                initialTarget={
                  current.value.target?.kind === 'model' ? current.value.target : undefined
                }
                label={zh ? '使用此模型恢复委派' : 'Use this model for the delegated task'}
                onSelect={async (target) => {
                  setBusy(true);
                  setError(undefined);
                  try {
                    await authorize(
                      context,
                      current.value.authorization,
                      zh ? '更换委派任务模型' : 'Change the delegated task model',
                    );
                    await context.remote.method('select-delegation-model')({
                      assignmentId: current.operation,
                      expectedRevision: current.value.revision,
                      target,
                    });
                  } finally {
                    setBusy(false);
                    setRefresh((value) => value + 1);
                  }
                }}
              />
            </>
          ) : null}
        </div>
      ) : null}
    </details>
  );
}
