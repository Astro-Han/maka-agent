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
import { ClientRuntime, ClientSlot, type ClientDiagnostic } from '@maka/ui/client-plugins';
import { createServicesContext } from '../../application/contracts/feature-services.js';
import type { ClientHostRef, ClientPluginServices } from './ports.js';
import { usePublishComposerSuggestions } from './suggestions.js';
export { ComposerSuggestionsProvider, useComposerSuggestions } from './suggestions.js';

export type { ClientHostRef, ClientPluginServices } from './ports.js';
const { Provider, useServices } = createServicesContext<ClientPluginServices>('ClientPluginServices');
export const ClientPluginServicesProvider = Provider;
const modules = { react: React, 'react/jsx-runtime': JsxRuntime, '@maka-agent/plugin-sdk/client': ClientSdk };

/** Bound to an originating Host, never the currently selected default Host. */
export function ClientPluginSlot<K extends keyof ClientSdk.ClientSlots>(props: {
  readonly host: ClientHostRef;
  readonly name: K;
  readonly input: ClientSdk.ClientSlots[K];
}): React.ReactNode {
  const services = useServices();
  const publishSuggestions = usePublishComposerSuggestions();
  const [runtime, setRuntime] = React.useState<ClientRuntime>();
  const [failure, setFailure] = React.useState(false);
  const [contextRevision, changed] = React.useReducer((value: number) => value + 1, 0);
  const report = React.useCallback((diagnostic: ClientDiagnostic) => {
    console.error('Client plugin failed', diagnostic);
    setFailure(true);
  }, []);
  const { profileId, hostId } = props.host;
  const transport = React.useMemo(() => services.connect({ profileId, hostId }), [services, profileId, hostId]);
  React.useEffect(() => transport.subscribeContext(changed), [transport]);
  React.useEffect(() => {
    const lifetime = new AbortController();
    const instance = new ClientRuntime({ document, modules, source: transport.source, remote: transport.remote,
      localFiles: transport.localFiles,
      report: (diagnostic) => {
        if (!lifetime.signal.aborted) report(diagnostic);
        else console.error('Client plugin cleanup failed', diagnostic);
      },
    });
    setRuntime(instance);
    setFailure(false);
    let revision = 0;
    let retry = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let fetching: AbortController | undefined;
    const refresh = () => {
      if (lifetime.signal.aborted) return;
      instance.invalidate();
      fetching?.abort();
      fetching = new AbortController();
      const signal = AbortSignal.any([lifetime.signal, fetching.signal, AbortSignal.timeout(30_000)]);
      const current = ++revision;
      void transport.snapshot(signal).then(async (snapshot) => {
        signal.throwIfAborted();
        await instance.reconcile(snapshot);
        if (current !== revision || lifetime.signal.aborted) return;
        retry = 0;
        setFailure(false);
      }).catch((error: unknown) => {
        if (current !== revision || lifetime.signal.aborted) return;
        report({ error });
        if (retry < 3) timer = setTimeout(refresh, 250 * 2 ** retry++);
      });
    };
    const unsubscribe = transport.subscribe(() => {
      clearTimeout(timer);
      retry = 0;
      refresh();
    });
    refresh();
    return () => {
      lifetime.abort();
      clearTimeout(timer);
      unsubscribe();
      // No state writes into an unmounted component. The error remains observable.
      void instance.close().catch((error: unknown) => console.error('Client plugin cleanup failed', error));
    };
  }, [transport, report]);
  const composerInput = {...props.input, contextRevision,
    ...(props.name === 'workspace.manage' ? {} : {publishSuggestions})};
  const input = 'onOpenSession' in composerInput ? {
    ...composerInput,
    onOpenSession(sessionId: string) {
      const open = props.input as ClientSdk.ClientSlots['session.composer.before'];
      void transport.session(sessionId).then(open.onOpenSession).catch((error: unknown) => report({ error }));
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
