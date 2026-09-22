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
import { importSource, openSource, recoverUser } from './files.js';
import { UpdatePreview } from './preview.js';
import { Locations } from './locations.js';
import type { ClientContext } from '@maka-agent/plugin-sdk/client';
import {
  type Call,
  type Catalog,
  type Copy,
  type Mutation,
  type Preview,
  type Reply,
  type Skill,
  type Target,
  type View,
} from './model.js';

export function Manage({
  context,
  call,
  target,
  t,
}: {
  context: ClientContext;
  call: Call;
  target: Target;
  t: Copy;
}) {
  const [view, setView] = useState<View>('governance');
  const [page, setPage] = useState<Catalog>();
  const [preview, setPreview] = useState<{ ref: string; value: Preview }>();
  const [deleting, setDeleting] = useState<string>();
  const [failure, setFailure] = useState('');
  const [busy, setBusy] = useState(false);
  const epoch = useRef(0);
  useEffect(
    () => () => {
      epoch.current++;
    },
    [],
  );
  const accept = (result: Reply) => {
    if (result.kind !== 'page' || !('view' in result))
      throw new Error(result.kind === 'rejected' ? result.reason : t.changed);
    setPage(result);
  };
  const run = async (work: () => Promise<() => void>) => {
    if (busy) return;
    const current = ++epoch.current;
    setBusy(true);
    setFailure('');
    try {
      const commit = await work();
      if (current === epoch.current) commit();
    } catch (error) {
      if (current === epoch.current) setFailure(error instanceof Error ? error.message : t.failed);
    } finally {
      if (current === epoch.current) setBusy(false);
    }
  };
  const load = (selected: View, cursor?: string) => {
    void run(async () => {
      const result = await call({
        kind: 'catalog',
        view: selected,
        page: cursor && page ? { revision: page.revision, cursor } : null,
      });
      return () => {
        accept(result);
        setView(selected);
        setPreview(undefined);
        setDeleting(undefined);
      };
    });
  };
  useEffect(() => {
    const current = ++epoch.current;
    setBusy(true);
    void call({ kind: 'catalog', view: 'governance', page: null })
      .then((result) => {
        if (current === epoch.current) accept(result);
      })
      .catch((error) => {
        if (current === epoch.current) setFailure(String(error));
      })
      .finally(() => {
        if (current === epoch.current) setBusy(false);
      });
    return () => {
      epoch.current++;
    };
  }, [call]);
  const mutate = (mutation: Mutation, revision = page?.revision) => {
    if (!revision) return;
    void run(async () => {
      const result = await call({ kind: 'mutate', expectedRevision: revision, mutation });
      if (result.kind !== 'committed' && result.kind !== 'unchanged')
        throw new Error(result.kind === 'rejected' ? result.reason : t.changed);
      const refreshed = await call({ kind: 'catalog', view, page: null });
      return () => {
        accept(refreshed);
        setPreview(undefined);
        setDeleting(undefined);
      };
    });
  };
  const actions = (skill: Skill) => (
    <div className="skills-actions">
      {context.localFiles
        ? (['file', 'directory'] as const).map((target) => (
            <button
              key={target}
              type="button"
              disabled={busy}
              onClick={() =>
                void run(async () => {
                  await openSource(context, call, skill.ref, target);
                  return () => {};
                })
              }
            >
              {target === 'file' ? t.openFile : t.openFolder}
            </button>
          ))
        : null}
      <button
        type="button"
        disabled={busy}
        onClick={() => mutate({ kind: 'set_enabled', ref: skill.ref, enabled: !skill.enabled })}
      >
        {skill.enabled ? t.disable : t.enable}
      </button>
      <button
        type="button"
        disabled={busy}
        onClick={() => mutate({ kind: 'set_pinned', ref: skill.ref, pinned: !skill.pinned })}
      >
        {skill.pinned ? t.unpin : t.pin}
      </button>
      {skill.manageable ? (
        <button type="button" disabled={busy} onClick={() => setDeleting(skill.ref)}>
          {t.remove}
        </button>
      ) : null}
      {skill.manageable &&
      ['update_available', 'local_modified'].includes(skill.managedUpdateStatus ?? '') ? (
        <button
          type="button"
          disabled={busy}
          onClick={() =>
            void run(async () => {
              const result = await call({
                kind: 'preview',
                expectedRevision: page!.revision,
                ref: skill.ref,
              });
              if (result.kind !== 'preview')
                throw new Error(result.kind === 'rejected' ? result.reason : t.changed);
              return () => setPreview({ ref: skill.ref, value: result });
            })
          }
        >
          {t.preview}
        </button>
      ) : null}
    </div>
  );
  return (
    <section aria-label={t.manage}>
      <Locations context={context} target={target} t={t} />
      <nav>
        {context.localFiles ? (
          <button
            type="button"
            disabled={busy}
            onClick={() =>
              void run(async () => {
                if (!(await importSource(context))) return () => {};
                const result = await call({ kind: 'catalog', view: 'managed_sources', page: null });
                return () => {
                  accept(result);
                  setView('managed_sources');
                };
              })
            }
          >
            {t.import}
          </button>
        ) : null}
        {(['governance', 'bundled', 'managed_sources'] as const).map((tab, index) => (
          <button
            type="button"
            key={tab}
            disabled={busy}
            aria-pressed={view === tab}
            onClick={() => load(tab)}
          >
            {[t.installed, t.bundled, t.sources][index]}
          </button>
        ))}
        <button type="button" disabled={busy} onClick={() => load(view)}>
          {t.refresh}
        </button>
        <button
          type="button"
          disabled={busy || !page}
          onClick={() => mutate({ kind: 'create_starter' })}
        >
          {t.add}
        </button>
      </nav>
      {failure ? <p role="alert">{failure}</p> : null}
      {page?.userRecovery ? (
        <div role="alert">
          <p>{page.userRecovery}</p>
          <button
            type="button"
            disabled={busy}
            onClick={() =>
              void run(async () => {
                await recoverUser(context);
                const result = await call({ kind: 'catalog', view, page: null });
                return () => accept(result);
              })
            }
          >
            {t.recover}
          </button>
        </div>
      ) : null}
      {page?.items.length === 0 ? <p>{t.empty}</p> : null}
      <ul>
        {page?.items.map((item) => (
          <li key={'ref' in item ? item.ref : item.id}>
            <strong>{item.name}</strong>
            <p>{item.description}</p>
            {'ref' in item ? (
              <>
                <small>
                  {item.ref} · {item.validationStatus}
                </small>
                {item.kind === 'skill' ? actions(item) : null}
              </>
            ) : (
              <button
                type="button"
                disabled={busy || item.installed}
                onClick={() =>
                  mutate({
                    kind: 'install',
                    sourceType: item.kind === 'bundled' ? 'bundled' : 'managed',
                    sourceId: item.id,
                  })
                }
              >
                {item.installed ? t.installed : t.install}
              </button>
            )}
          </li>
        ))}
      </ul>
      {deleting ? (
        <div role="alert">
          <p>
            {t.confirm} {deleting}
          </p>
          <button
            type="button"
            disabled={busy}
            onClick={() => mutate({ kind: 'delete', ref: deleting })}
          >
            {t.remove}
          </button>
          <button type="button" disabled={busy} onClick={() => setDeleting(undefined)}>
            {t.cancel}
          </button>
        </div>
      ) : null}
      <UpdatePreview
        preview={preview}
        t={t}
        busy={busy}
        apply={mutate}
        cancel={() => setPreview(undefined)}
      />
      {page?.nextCursor ? (
        <button type="button" disabled={busy} onClick={() => load(view, page.nextCursor!)}>
          {t.next}
        </button>
      ) : null}
    </section>
  );
}
