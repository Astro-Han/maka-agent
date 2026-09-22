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
import type { ClientContext, ClientPlugin, ClientSlots } from '@maka-agent/plugin-sdk/client';
import { copy, request, type Invocable, type Target } from './client/model.js';
import { Manage } from './client/manage.js';
import { useSuggestions } from './client/suggestions.js';
import { useChanges } from './client/changes.js';
import { styles } from './client/styles.js';

function Panel({
  context,
  target,
  manageOnly = false,
  ...props
}: {
  locale: ClientSlots['session.composer.before']['locale'];
  appendText?: (text: string) => void;
  publishSuggestions?: ClientSlots['session.composer.before']['publishSuggestions'];
  context: ClientContext;
  target: Target;
  manageOnly?: boolean;
  contextRevision?: number;
}) {
  const t = copy[props.locale === 'en' ? 'en' : 'zh-CN'];
  const call = useMemo(() => request(context, target), [context, target]);
  const [open, setOpen] = useState(false);
  const [manage, setManage] = useState(false);
  const [page, setPage] = useState<Invocable>();
  const [filter, setFilter] = useState('');
  const [failure, setFailure] = useState('');
  const [busy, setBusy] = useState(false);
  const changes = useChanges(context);
  const revision = changes.revision + '/' + (props.contextRevision ?? 0);
  const suggestionFailure = useSuggestions(call, props.publishSuggestions, revision);
  const epoch = useRef(0);
  useEffect(
    () => () => {
      epoch.current++;
    },
    [],
  );
  const load = async (cursor?: string, refresh = false) => {
    if (busy && !refresh) return;
    const current = ++epoch.current;
    setBusy(true);
    setFailure('');
    setOpen(true);
    try {
      const result = await call({
        kind: 'invocable',
        page: cursor && page ? { revision: page.revision, cursor } : null,
      });
      if (current !== epoch.current) return;
      if (result.kind !== 'page' || 'view' in result) throw new Error(t.changed);
      setPage(result);
    } catch (error) {
      if (current === epoch.current) setFailure(error instanceof Error ? error.message : t.failed);
    } finally {
      if (current === epoch.current) setBusy(false);
    }
  };
  useEffect(() => {
    if (open) void load(undefined, true);
    else {
      epoch.current++;
      setPage(undefined);
      setBusy(false);
    }
  }, [call, revision]);
  if (manageOnly)
    return (
      <section data-maka-skills-plugin data-mode="manage" aria-label={t.title}>
        <Manage context={context} call={call} target={target} t={t} />
      </section>
    );
  return (
    <section data-maka-skills-plugin data-mode="composer" aria-label={t.title}>
      <header>
        <strong>{t.title}</strong>
        <button
          type="button"
          disabled={busy}
          aria-expanded={open}
          onClick={() => (open ? setOpen(false) : void load())}
        >
          {t.available}
        </button>
        <button type="button" aria-expanded={manage} onClick={() => setManage(!manage)}>
          {t.manage}
        </button>
      </header>
      {suggestionFailure ? <p role="alert">{suggestionFailure}</p> : null}
      {changes.failure ? <p role="alert">{changes.failure}</p> : null}
      {open ? (
        <div className="skills-picker">
          <div className="skills-filter">
            <input
              aria-label={t.filter}
              placeholder={t.filter}
              value={filter}
              onChange={(event) => setFilter(event.target.value)}
            />
            <button type="button" disabled={busy} onClick={() => void load()}>
              {t.refresh}
            </button>
          </div>
          {failure ? <p role="alert">{failure}</p> : null}
          <ul>
            {page?.items
              .filter((item) =>
                (item.name + ' ' + item.id + ' ' + item.description)
                  .toLocaleLowerCase()
                  .includes(filter.toLocaleLowerCase()),
              )
              .map((item) => (
                <li key={item.ref}>
                  <button
                    type="button"
                    disabled={!props.appendText}
                    onClick={() => {
                      props.appendText?.('/skill:' + item.id + ' ');
                      setOpen(false);
                    }}
                  >
                    {item.name}
                  </button>
                  <p>{item.description}</p>
                </li>
              ))}
          </ul>
          {page?.items.length === 0 ? <p>{t.empty}</p> : null}
          {page?.nextCursor ? (
            <button type="button" disabled={busy} onClick={() => void load(page.nextCursor!)}>
              {t.next}
            </button>
          ) : null}
        </div>
      ) : null}
      {manage ? <Manage context={context} call={call} target={target} t={t} /> : null}
    </section>
  );
}
const plugin: ClientPlugin = {
  activate(context) {
    context.style(styles);
    context.slots.register(
      'session.composer.before',
      'skills',
      (props) => (
        <Panel
          key={props.sessionId}
          {...props}
          context={context}
          target={{ kind: 'session', sessionId: props.sessionId }}
        />
      ),
      { order: 10 },
    );
    context.slots.register(
      'workspace.composer.before',
      'skills',
      (props) => (
        <Panel
          key={JSON.stringify([props.workspace, props.sandboxMode, props.collaborationMode])}
          {...props}
          context={context}
          target={{ kind: 'workspace', ...props }}
        />
      ),
      { order: 10 },
    );
    context.slots.register(
      'workspace.manage',
      'skills',
      (props) =>
        props.section === 'skills' ? (
          <Panel
            key={JSON.stringify(props.workspace)}
            {...props}
            context={context}
            target={{ kind: 'workspace', ...props }}
            manageOnly
          />
        ) : null,
      { order: 10 },
    );
  },
};
export default plugin;
