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
import type { ClientContext } from '@maka-agent/plugin-sdk/client';
import type { ExecutionTarget, ModelChoices } from '@maka-agent/plugin-sdk/host';

export type ModelTarget = Extract<ExecutionTarget, { kind: 'model' }>;

export function ModelSelection({
  context,
  locale,
  label,
  onSelect,
}: {
  context: ClientContext;
  locale: string;
  label: string;
  onSelect: (target: ModelTarget) => Promise<void>;
}) {
  const zh = locale !== 'en';
  const [query, setQuery] = useState('');
  const [choices, setChoices] = useState<ModelChoices>();
  const [selected, setSelected] = useState('');
  const [thinking, setThinking] = useState<{ model: string; level: string }>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  useEffect(() => {
    let active = true;
    setChoices(undefined);
    setError(undefined);
    const timer = setTimeout(() => {
      void context.remote
        .method<{ query: string }, ModelChoices>('models')({ query })
        .then((page) => {
          if (active) setChoices(page);
        })
        .catch((error: unknown) => {
          if (active) setError(error instanceof Error ? error.message : String(error));
        });
    }, 150);
    return () => {
      active = false;
      clearTimeout(timer);
    };
  }, [context, query]);
  const key = (choice: ModelChoices['models'][number]) => JSON.stringify(choice.model);
  const model =
    choices?.models.find((choice) => key(choice) === selected) ??
    choices?.models.find((choice) => choice.isDefault) ??
    choices?.models[0];
  const level =
    model && thinking?.model === key(model)
      ? model.thinkingLevels.find((level) => level === thinking.level)
      : undefined;
  return (
    <div className="workhub-model-selection">
      <label>
        {zh ? '搜索模型' : 'Search models'}
        <input value={query} disabled={busy} onChange={(event) => setQuery(event.target.value)} />
      </label>
      <label>
        {zh ? '模型' : 'Model'}
        <select
          value={model ? key(model) : ''}
          disabled={busy || !model}
          onChange={(event) => {
            setSelected(event.target.value);
            setThinking(undefined);
          }}
        >
          {!model ? <option value="">{zh ? '没有可用模型' : 'No models available'}</option> : null}
          {choices?.models.map((choice) => (
            <option key={key(choice)} value={key(choice)}>
              {choice.connectionName} · {choice.displayName}
            </option>
          ))}
        </select>
      </label>
      {model?.thinkingLevels.length ? (
        <label>
          {zh ? '思考程度' : 'Thinking'}
          <select
            value={level ?? ''}
            disabled={busy}
            onChange={(event) => setThinking({ model: key(model), level: event.target.value })}
          >
            <option value="">{zh ? '默认' : 'Default'}</option>
            {model.thinkingLevels.map((level) => (
              <option key={level} value={level}>
                {level}
              </option>
            ))}
          </select>
        </label>
      ) : null}
      {choices?.complete === false ? (
        <p>{zh ? '请缩小搜索范围以查看其他模型。' : 'Refine the search to find more models.'}</p>
      ) : null}
      <Button
        label={label}
        isDisabled={busy || !model}
        onClick={() => {
          if (!model) return;
          setBusy(true);
          setError(undefined);
          void onSelect({ kind: 'model', model: model.model, thinkingLevel: level ?? null })
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
