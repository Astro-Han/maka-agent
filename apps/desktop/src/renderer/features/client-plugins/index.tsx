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
import { useUiLocale } from '@maka/ui';
import * as JsxRuntime from 'react/jsx-runtime';
import * as ClientSdk from '@maka-agent/plugin-sdk/client';
import * as ClientUi from '@maka/ui/plugin';
import { ClientSlot } from '@maka/ui/client-plugins';
import { createServicesContext } from '../../application/contracts/feature-services.js';
import type { ClientHostRef, ClientPluginServices } from './ports.js';
import { usePublishComposerSuggestions } from './suggestions.js';
import { ClientHostRuntime } from './host-runtime.js';
import { parseDesktopSessionKey } from '../../../shared/runtime-host-identity.js';
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
const modules = { react: React, 'react/jsx-runtime': JsxRuntime, '@maka-agent/plugin-sdk/client': ClientSdk, '@maka/ui/plugin': ClientUi };

/** Bound to an originating Host, never the currently selected default Host. */
export function ClientPluginSlot<K extends keyof ClientSdk.ClientSlots>(props: {
  readonly host: ClientHostRef;
  readonly name: K;
  readonly entryId?: string;
  readonly className?: string;
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
  const resolving = 'onResolved' in props.input ? props.input.onResolved : undefined;
  const resolutionError = 'onError' in props.input ? props.input.onError : undefined;
  const onResolved = React.useCallback((id: string, signal: AbortSignal) => {
    if (!session || signal.aborted) return;
    void session(id).then((projected) => {
      if (!signal.aborted) resolving?.(projected, signal);
    }).catch((error: unknown) => {
      if (!signal.aborted) resolutionError?.(error instanceof Error ? error.message : String(error));
    });
  }, [session, resolving, resolutionError]);
  const composerInput = {...props.input, contextRevision,
    ...(props.name === 'session.resolve' ? {onResolved} :
      props.name.endsWith('.composer.before') ? {publishSuggestions} : {})};
  const input = 'onOpenSession' in composerInput ? {
    ...composerInput,
    onOpenSession(sessionId: string) {
      const open = props.input as ClientSdk.ClientSlots['session.composer.before'];
      if (!session) return;
      void session(sessionId).then(open.onOpenSession).catch((error: unknown) => report({ error }));
    },
  } : composerInput;
  return <>
    {failure ? <div role="status" className="clientPluginFailure">
      {props.input.locale === 'zh-CN' ? '部分扩展未能加载。' :
        props.input.locale === 'zh-TW' ? '部分擴充功能未能載入。' : 'Some extensions could not load.'}
    </div> : null}
    {runtime ? <ClientSlot store={runtime.slots} name={props.name} entryId={props.entryId} input={input}
      className={props.className ?? (props.name.endsWith('.composer.before') ? 'maka-composer-plugin-slot' : undefined)}
      onError={(identity, error) => report({ identity, error })} /> : null}
  </>;
}

export function ClientPluginSessionSlot<K extends 'session.composer.before' | 'session.header.actions' | 'turn.footer'>(props: {
  readonly host: ClientHostRef;
  readonly name: K;
  readonly className?: string;
  readonly input: ClientSdk.ClientSlots[K];
}) {
  const session = parseDesktopSessionKey(props.input.sessionId);
  if (session.hostId !== props.host.hostId) throw new Error('Plugin Session belongs to another Host');
  return <ClientPluginSlot {...props}
    input={{ ...props.input, sessionId: session.sessionId }} />;
}

/** Keep product anchors together and preserve each Turn component across streaming renders. */
export function ClientPluginSurfaces(input: {
  session?: { readonly profileId: string; readonly runtimeHostId: string };
  sessionId?: string;
  locale: ClientSdk.ClientSlots['turn.footer']['locale'];
  onOpenSession(sessionId: string): void;
  composer: React.RefObject<{ appendText(text: string): void; focus(): void } | null>;
  readOnly: boolean;
  children(slots: { header: React.ReactNode; composer: React.ReactNode; TurnFooter?: React.ComponentType<{ turnId: string }> }): React.ReactNode;
}) {
  const { sessionId, locale } = input;
  const profileId = input.session?.profileId;
  const hostId = input.session?.runtimeHostId;
  const origin = profileId && hostId ? { profileId, hostId } : undefined;
  const TurnFooter = React.useMemo(() => {
    if (!sessionId || !profileId || !hostId) return undefined;
    const host = { profileId, hostId };
    return function PluginTurnFooter({ turnId }: { turnId: string }) {
      return <ClientPluginSessionSlot host={host} name="turn.footer" input={{ sessionId, turnId, locale }} />;
    };
  }, [sessionId, locale, profileId, hostId]);
  return <>{input.children({
    TurnFooter,
    header: origin && sessionId ? <ClientPluginSessionSlot host={origin}
      name="session.header.actions" className="clientPluginHeaderActions" input={{ sessionId, locale }} /> : null,
    composer: origin && sessionId ? <ClientPluginSessionSlot host={origin}
      name="session.composer.before" input={{ sessionId, locale,
        onOpenSession: input.onOpenSession, appendText: input.readOnly ? undefined : (text) => {
          input.composer.current?.appendText(text);
          input.composer.current?.focus();
        } }} /> : null,
  })}<ClientApplicationOverlay locale={locale} /></>;
}

function ClientApplicationOverlay({ locale }: { locale: ClientSdk.ClientSlots['application.overlay']['locale'] }) {
  const origin = useDefaultPluginHost(true);
  return origin?.host ? <ClientPluginSlot host={origin.host} name="application.overlay" input={{ locale }} /> : null;
}

function useDefaultPluginHost(enabled: boolean) {
  const { services } = useServices();
  const [origin, setOrigin] = React.useState<{ host?: ClientHostRef; error?: string }>();
  React.useEffect(() => {
    if (!enabled) { setOrigin(undefined); return; }
    const lifetime = new AbortController();
    let request: AbortController | undefined;
    const refresh = () => {
      request?.abort();
      const pending = new AbortController();
      request = pending;
      const signal = AbortSignal.any([lifetime.signal, pending.signal, AbortSignal.timeout(30_000)]);
      setOrigin(undefined);
      void services.defaultHost(signal).then((host) => {
        if (!signal.aborted) setOrigin({ host });
      }).catch((error: unknown) => {
        if (!lifetime.signal.aborted && !pending.signal.aborted)
          setOrigin({ error: error instanceof Error ? error.message : String(error) });
      });
    };
    const unsubscribe = services.subscribeDefaultHost(refresh);
    refresh();
    return () => { lifetime.abort(); request?.abort(); unsubscribe(); };
  }, [services, enabled]);
  return origin;
}

/** A workspace consumes one configured Client Entry, not a private feature RPC. */
export function usePluginSession(entryId: string, enabled: boolean, locale: 'en' | 'zh-CN' | 'zh-TW') {
  const origin = useDefaultPluginHost(enabled);
  const [resolved, setResolved] = React.useState<{ host: ClientHostRef; sessionId?: string; error?: string }>();
  const host = origin?.host;
  const onResolving = React.useCallback(() => { setResolved(undefined); }, []);
  const onResolved = React.useCallback((sessionId: string) => {
    if (host) setResolved({ host, sessionId });
  }, [host]);
  const onError = React.useCallback((error: string) => {
    if (host) setResolved({ host, error });
  }, [host]);
  const current = enabled && resolved?.host === host ? resolved : undefined;
  const error = origin?.error ?? current?.error;
  return {
    host: current?.sessionId ? host : undefined,
    sessionId: current?.sessionId,
    resolver: enabled ? <>
      {host ? <ClientPluginSlot host={host} entryId={entryId} name="session.resolve"
        input={{ locale, onResolving, onResolved, onError }} /> : null}
      {error ? <div role="status" className="clientPluginFailure">{error}</div> : null}
    </> : null,
  };
}

/** Owns resolution lifetime; composition only supplies the selected surface. */
export function ClientPluginSession(props: {
  entryId: string;
  children: (binding: { host: ClientHostRef; sessionId: string }) => React.ReactNode;
}) {
  const binding = usePluginSession(props.entryId, true, useUiLocale());
  return <>{binding.resolver}{binding.host && binding.sessionId
    ? props.children({ host: binding.host, sessionId: binding.sessionId }) : null}</>;
}
