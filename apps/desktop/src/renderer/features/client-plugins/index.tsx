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
import { ClientSlot } from '@maka/ui/client-plugins';
import { createServicesContext } from '../../application/contracts/feature-services.js';
import type { ClientHostRef, ClientPluginServices } from './ports.js';
import { usePublishComposerSuggestions } from './suggestions.js';
import { ClientHostRuntime } from './host-runtime.js';
export { ComposerSuggestionsProvider, useComposerSuggestions } from './suggestions.js';

export type { ClientHostRef, ClientPluginServices } from './ports.js';
const { Provider, useServices } = createServicesContext<{
  services: ClientPluginServices;
  hosts: Map<string, ClientHostRuntime>;
}>('ClientPluginServices');

export function ClientPluginServicesProvider(props: { services: ClientPluginServices; children?: React.ReactNode }) {
  const value = React.useMemo(() => ({ services: props.services, hosts: new Map<string, ClientHostRuntime>() }), [props.services]);
  return <Provider services={value}>{props.children}</Provider>;
}
const modules = { react: React, 'react/jsx-runtime': JsxRuntime, '@maka-agent/plugin-sdk/client': ClientSdk };

/** Bound to an originating Host, never the currently selected default Host. */
export function ClientPluginSlot<K extends keyof ClientSdk.ClientSlots>(props: {
  readonly host: ClientHostRef;
  readonly name: K;
  readonly input: ClientSdk.ClientSlots[K];
}): React.ReactNode {
  const { services, hosts } = useServices();
  const publishSuggestions = usePublishComposerSuggestions();
  const { profileId, hostId } = props.host;
  const key = JSON.stringify([profileId, hostId]);
  let owner = hosts.get(key);
  if (!owner) {
    owner = new ClientHostRuntime(() => services.connect({ profileId, hostId }), {
      document, modules, report: (diagnostic) => console.error('Client plugin failed', diagnostic),
    });
    hosts.set(key, owner);
  }
  const { runtime, failure, contextRevision, session } = React.useSyncExternalStore(owner.subscribe, owner.snapshot, owner.snapshot);
  const report = owner.report;
  const composerInput = {...props.input, contextRevision,
    ...(props.name === 'workspace.manage' ? {} : {publishSuggestions})};
  const input = 'onOpenSession' in composerInput ? {
    ...composerInput,
    onOpenSession(sessionId: string) {
      const open = props.input as ClientSdk.ClientSlots['session.composer.before'];
      if (!session) return;
      void session(sessionId).then(open.onOpenSession).catch((error: unknown) => report({ error }));
    },
  } : composerInput;
  return <div className={props.name === 'workspace.manage' ? undefined : 'maka-composer-plugin-slot'}>
    {failure ? <div role="status" className="clientPluginFailure">
      {props.input.locale === 'zh-CN' ? '部分扩展未能加载。' :
        props.input.locale === 'zh-TW' ? '部分擴充功能未能載入。' : 'Some extensions could not load.'}
    </div> : null}
    {runtime ? <ClientSlot store={runtime.slots} name={props.name} input={input}
      onError={(identity, error) => report({ identity, error })} /> : null}
  </div>;
}

export function ClientPluginComposerSlot(props: {
  readonly host: ClientHostRef;
  readonly input: ClientSdk.ClientSlots['session.composer.before'];
}) {
  return <ClientPluginSlot {...props} name="session.composer.before" />;
}
