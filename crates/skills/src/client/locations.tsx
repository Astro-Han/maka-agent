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
import type { ClientContext } from '@maka-agent/plugin-sdk/client';
import type { Copy, Target } from './model.js';

type Location = {
  id: 'project:maka' | 'project:agents' | 'workspace:legacy' | 'user:maka' | 'user:agents';
  path: string | null;
  status: 'available' | 'missing' | 'blocked_path' | 'read_failed' | 'unavailable';
  validCount: number;
  invalidCount: number;
};
type Action =
  | { kind: 'list' }
  | {
      kind: 'open';
      id: Location['id'];
      expectedPath: string;
      createIfMissing: boolean;
    };
type Result =
  | { kind: 'locations'; locations: Location[] }
  | { kind: 'resolved'; path: string }
  | { kind: 'rejected'; reason: string };

export function Locations({
  context,
  target,
  t,
}: {
  context: ClientContext;
  target: Target;
  t: Copy;
}) {
  const [locations, setLocations] = useState<Location[]>();
  const [failure, setFailure] = useState('');
  const [busy, setBusy] = useState(false);
  const epoch = useRef(0);
  const call = useMemo(() => {
    const workspace =
      target.kind === 'workspace'
        ? {
            workspace: target.workspace,
            sandboxMode: target.sandboxMode,
            collaborationMode: target.collaborationMode,
          }
        : null;
    const method = context.remote.method<{ workspace: typeof workspace; action: Action }, Result>(
      'locations',
      target.kind === 'session' ? target.sessionId : undefined,
    );
    return (action: Action) => method({ workspace, action });
  }, [context, target]);
  useEffect(() => {
    epoch.current++;
    setLocations(undefined);
    setFailure('');
    setBusy(false);
    return () => {
      epoch.current++;
    };
  }, [call, context.localFiles]);
  if (!context.localFiles) return null;
  const run = async (work: (current: () => boolean) => Promise<Location[] | undefined>) => {
    if (busy) return;
    const generation = ++epoch.current;
    const current = () => generation === epoch.current;
    setBusy(true);
    setFailure('');
    try {
      const result = await work(current);
      if (current() && result) setLocations(result);
    } catch (error) {
      if (current()) setFailure(error instanceof Error ? error.message : t.failed);
    } finally {
      if (current()) setBusy(false);
    }
  };
  const list = async () => {
    const result = await call({ kind: 'list' });
    if (result.kind !== 'locations') throw new Error(t.changed);
    return result.locations;
  };
  return (
    <section aria-label={t.locations}>
      <button type="button" disabled={busy} onClick={() => void run(list)}>
        {locations ? t.refreshLocations : t.locations}
      </button>
      {failure ? <p role="alert">{failure}</p> : null}
      {locations ? (
        <ul>
          {locations.map((location) => (
            <li key={location.id}>
              <strong>{t.locationNames[location.id]}</strong>
              {location.path ? <p>{location.path}</p> : null}
              <small>
                {t.locationStatuses[location.status]}
                {location.status === 'available'
                  ? ` · ${t.validSkills}: ${location.validCount} · ${t.invalidSkills}: ${location.invalidCount}`
                  : ''}
              </small>
              {location.path &&
              (location.status === 'available' || location.status === 'missing') ? (
                <button
                  type="button"
                  disabled={busy}
                  onClick={() =>
                    void run(async (current) => {
                      const result = await call({
                        kind: 'open',
                        id: location.id,
                        expectedPath: location.path!,
                        createIfMissing: location.status === 'missing',
                      });
                      if (!current()) return;
                      if (result.kind === 'rejected') {
                        const refreshed = await list();
                        if (current()) {
                          setLocations(refreshed);
                          throw new Error(t.changed);
                        }
                        return;
                      }
                      if (result.kind !== 'resolved') throw new Error(t.changed);
                      await context.localFiles?.open(result.path);
                      if (current()) return list();
                    })
                  }
                >
                  {location.status === 'missing' ? t.createFolder : t.openFolder}
                </button>
              ) : null}
            </li>
          ))}
        </ul>
      ) : null}
    </section>
  );
}
