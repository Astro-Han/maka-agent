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

import type { ClientDescriptor } from '@maka-agent/plugin-sdk/client';
import type { ClientSnapshot } from '@maka/ui/client-plugins';
import type { PluginClientQueryResult } from '@maka/runtime-host/protocol';
import type { MakaBridge } from '../../../preload/bridge-contract.js';
import type { ClientPluginServices } from '../../features/client-plugins/index.js';
import { clientPluginRemote } from './client-plugin-remote.js';

/** Every request retains its original Host; a default-Host change cannot reroute it. */
export function createDesktopClientPluginServices(
  bridge: Pick<MakaBridge, 'clientPlugins' | 'runtimeHostProfiles'> = window.maka,
): ClientPluginServices {
  let defaultProfile: string | undefined;
  return {
    async defaultHost(signal) {
      const host = await bounded(bridge.runtimeHostProfiles.getDefaultHost(), signal);
      signal.throwIfAborted();
      defaultProfile = host.profileId;
      return host;
    },
    subscribeDefaultHost(listener) {
      return bridge.runtimeHostProfiles.subscribeChanges((event) => {
        if (event.isDefault || event.profileId === defaultProfile) listener();
      });
    },
    connect(host) {
      const query: typeof bridge.clientPlugins.query = async (origin, input) => bridge.clientPlugins.query(origin, input);
      let targetEpoch: string | undefined;
      let localFiles = false;
      return {
        subscribeContext: (listener) => bridge.clientPlugins.subscribeContext(host, listener),
        async session(sessionId) {
          if (!targetEpoch) throw new Error('Client catalog has no connection identity');
          return bridge.clientPlugins.session(host, targetEpoch, sessionId);
        },
        remote(identity, signal) {
          if (!targetEpoch) throw new Error('Client catalog has no connection identity');
          return clientPluginRemote(bridge.clientPlugins.remote, host, targetEpoch)(identity, signal);
        },
        localFiles(identity, signal) {
          if (!targetEpoch) throw new Error('Client catalog has no connection identity');
          if (!localFiles) return undefined;
          const epoch = targetEpoch;
          return {
            pick: () => bounded(bridge.clientPlugins.file(host, epoch, identity, {kind:'pick'}), signal),
            async open(path) {
              await bounded(bridge.clientPlugins.file(host, epoch, identity, {kind:'open',path}),
                AbortSignal.any([signal, AbortSignal.timeout(30_000)]));
            },
          };
        },
        async snapshot(signal): Promise<ClientSnapshot> {
          const connection = await bounded(bridge.clientPlugins.connection(host), signal);
          const { epoch } = connection;
          const entries: ClientDescriptor[] = [];
          let cursor: { revision: string; afterEntry: string } | null = null;
          let revision: string | undefined;
          do {
            signal.throwIfAborted();
            const page: PluginClientQueryResult = await bounded(query(host, { kind: 'snapshot', cursor }), signal);
            if (page.kind !== 'snapshot' || (revision !== undefined && page.revision !== revision))
              throw new Error('Client snapshot changed during pagination');
            revision = page.revision;
            if (entries.length && page.entries.length && entries.at(-1)!.entryId >= page.entries[0].entryId)
              throw new Error('Client snapshot cursor did not advance');
            entries.push(...page.entries);
            if (entries.length > 4096) throw new Error('Client catalog exceeds Entry limit');
            cursor = page.nextCursor;
          } while (cursor);
          if (revision === undefined) throw new Error('Client snapshot has no revision');
          const current = await bounded(bridge.clientPlugins.connection(host), signal);
          if (epoch !== current.epoch || connection.hostEpoch !== current.hostEpoch)
            throw new Error('Client connection changed during snapshot');
          targetEpoch = epoch;
          localFiles = connection.localFiles;
          return { revision, connection: epoch, hostEpoch: connection.hostEpoch, entries };
        },
        async source(descriptor, signal) {
          const parts: string[] = [];
          let offset = 0;
          do {
            signal.throwIfAborted();
            const page = await bounded(query(host, {
              kind: 'bundle', entryId: descriptor.entryId, activation: descriptor.activation,
              clientDigest: descriptor.clientDigest, offset,
            }), signal);
            if (page.kind !== 'bundle' || page.entryId !== descriptor.entryId ||
              page.activation !== descriptor.activation || page.clientDigest !== descriptor.clientDigest ||
              page.totalBytes !== descriptor.totalBytes || page.offset !== offset)
              throw new Error('Client bundle identity changed');
            parts.push(page.content);
            if (page.nextOffset === null) break;
            if (page.nextOffset <= offset) throw new Error('Client bundle cursor did not advance');
            offset = page.nextOffset;
          } while (offset < descriptor.totalBytes);
          return parts.join('');
        },
        subscribe(listener) {
          const changed = bridge.clientPlugins.subscribeChanges(host, listener);
          const connected = bridge.runtimeHostProfiles.subscribeChanges((event) => {
            if (event.profileId === host.profileId && event.hostId === host.hostId) listener();
          });
          return () => { changed(); connected(); };
        },
      };
    },
  };
}

function bounded<T>(work: Promise<T>, signal: AbortSignal): Promise<T> {
  return new Promise((resolve, reject) => {
    const abort = () => { signal.removeEventListener('abort', abort); reject(signal.reason); };
    signal.addEventListener('abort', abort, { once: true });
    work.then(
      (value) => { signal.removeEventListener('abort', abort); resolve(value); },
      (error) => { signal.removeEventListener('abort', abort); reject(error); },
    );
    if (signal.aborted) abort();
  });
}
