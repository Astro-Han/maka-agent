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

import * as React from 'react';
import * as JsxRuntime from 'react/jsx-runtime';
import * as ClientSdk from '@maka-agent/plugin-sdk/client';
import * as ClientUi from '@maka/ui/plugin';
import { ClientSlotStore } from '@maka/ui/client-plugins';
import { createServicesContext } from '../../application/contracts/feature-services.js';
import type { ClientHostRef, ClientPluginServices } from './ports.js';
import { ClientHostRuntime } from './host-runtime.js';

const context = createServicesContext<{
  services: ClientPluginServices;
  hosts: Map<string, ClientHostRuntime>;
}>('ClientPluginServices');
export const useServices = context.useServices;
const modules = { react: React, 'react/jsx-runtime': JsxRuntime, '@maka-agent/plugin-sdk/client': ClientSdk, '@maka/ui/plugin': ClientUi };
const emptySlots = new ClientSlotStore();
const emptySnapshot: ReturnType<ClientHostRuntime['snapshot']> = { failure: false, contextRevision: 0 };
const inactive = { subscribe: () => () => {}, snapshot: () => emptySnapshot };
const reportFailure = (diagnostic: Parameters<ClientHostRuntime['report']>[0]) => console.error('Client plugin failed', diagnostic);

export function ClientPluginServicesProvider(props: { services: ClientPluginServices; children?: React.ReactNode }) {
  const value = React.useMemo(() => ({ services: props.services, hosts: new Map<string, ClientHostRuntime>() }), [props.services]);
  return <context.Provider services={value}>{props.children}</context.Provider>;
}

/** All consumers borrow the same Host/document activation owner. */
export function useClientHost(host: ClientHostRef | undefined) {
  const { services, hosts } = useServices();
  let owner: ClientHostRuntime | undefined;
  if (host) {
    const { profileId, hostId } = host;
    const key = JSON.stringify([profileId, hostId]);
    owner = hosts.get(key);
    if (!owner) {
      owner = new ClientHostRuntime(() => services.connect({ profileId, hostId }), {
        document, modules, report: reportFailure,
      });
      hosts.set(key, owner);
    }
  }
  const source = owner ?? inactive;
  const snapshot = React.useSyncExternalStore(source.subscribe, source.snapshot, source.snapshot);
  return { ...snapshot, report: owner?.report ?? reportFailure };
}

export function useClientSlots(runtime: ReturnType<typeof useClientHost>['runtime']) {
  const store = runtime?.slots ?? emptySlots;
  return React.useSyncExternalStore(store.subscribe, store.snapshot, store.snapshot);
}
