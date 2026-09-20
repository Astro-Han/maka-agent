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

import { ClientRuntime, type ClientRuntimeOptions, type ClientDiagnostic } from '@maka/ui/client-plugins';
import type { ClientPluginServices } from './ports.js';

type Transport = ReturnType<ClientPluginServices['connect']>;
type Runtime = Pick<ClientRuntime, 'slots' | 'invalidate' | 'reconcile' | 'close'>;
interface Snapshot {
  readonly runtime?: Runtime;
  readonly session?: (sessionId: string) => Promise<string>;
  readonly failure: boolean;
  readonly contextRevision: number;
}

/** One activation owner per Host/document. Slots only borrow its published view. */
export class ClientHostRuntime {
  readonly #listeners = new Set<() => void>();
  #snapshot: Snapshot = { failure: false, contextRevision: 0 };
  #lifetime?: AbortController;
  #closing: Promise<void> = Promise.resolve();
  #fenced = false;

  constructor(
    readonly connect: () => Transport,
    readonly options: Pick<ClientRuntimeOptions, 'document' | 'modules' | 'report'>,
    readonly create: (options: ClientRuntimeOptions) => Runtime = (options) => new ClientRuntime(options),
  ) {}

  snapshot = (): Snapshot => this.#snapshot;

  report = (diagnostic: ClientDiagnostic): void => {
    this.#set({ failure: true });
    this.options.report(diagnostic);
  };

  subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    if (this.#listeners.size === 1) this.#start();
    return () => {
      this.#listeners.delete(listener);
      if (this.#listeners.size === 0) {
        this.#lifetime?.abort();
        this.#lifetime = undefined;
        this.#set({ runtime: undefined, session: undefined });
      }
    };
  };

  #set(change: Partial<Snapshot>): void {
    this.#snapshot = { ...this.#snapshot, ...change };
    for (const listener of this.#listeners) listener();
  }

  #start(): void {
    const lifetime = new AbortController();
    this.#lifetime = lifetime;
    // Remounts (including StrictMode) cannot overlap the old effect cleanup.
    void this.#closing.then(() => {
      if (lifetime.signal.aborted || this.#fenced) return;
      this.#open(lifetime);
    }).catch((error: unknown) => {
      lifetime.abort();
      this.#set({ failure: true, runtime: undefined, session: undefined });
      this.options.report({ error });
    });
  }

  #open(lifetime: AbortController): void {
    const transport = this.connect();
    const session = async (sessionId: string) => {
      lifetime.signal.throwIfAborted();
      const projected = await transport.session(sessionId);
      lifetime.signal.throwIfAborted();
      return projected;
    };
    const report = (diagnostic: ClientDiagnostic) => {
      if (!lifetime.signal.aborted) this.report(diagnostic);
      else this.options.report(diagnostic);
    };
    const runtime = this.create({ ...this.options, source: transport.source,
      remote: transport.remote, localFiles: transport.localFiles, authorization: transport.authorization, report });
    let revision = 0;
    let retry = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let fetching: AbortController | undefined;
    const subscriptions: Array<() => void> = [];
    lifetime.signal.addEventListener('abort', () => {
      clearTimeout(timer);
      fetching?.abort();
      const failures: unknown[] = [];
      for (const unsubscribe of subscriptions.reverse()) {
        try { unsubscribe(); } catch (error) { failures.push(error); }
      }
      this.#closing = runtime.close().then(() => {
        if (failures.length) throw new AggregateError(failures, 'Client subscriptions could not close');
      }).catch((error: unknown) => {
        // A fresh runtime must not bypass unconfirmed cleanup.
        this.#fenced = true;
        this.#set({ failure: true, runtime: undefined, session: undefined });
        this.options.report({ error });
      });
    }, { once: true });
    const refresh = () => {
      if (lifetime.signal.aborted) return;
      clearTimeout(timer);
      runtime.invalidate();
      fetching?.abort();
      fetching = new AbortController();
      const signal = AbortSignal.any([lifetime.signal, fetching.signal, AbortSignal.timeout(30_000)]);
      const current = ++revision;
      void transport.snapshot(signal).then(async (snapshot) => {
        signal.throwIfAborted();
        await runtime.reconcile(snapshot);
        if (current !== revision || lifetime.signal.aborted) return;
        retry = 0;
        this.#set({ failure: false });
      }).catch((error: unknown) => {
        if (current !== revision || lifetime.signal.aborted) return;
        report({ error });
        if (retry < 3) timer = setTimeout(refresh, 250 * 2 ** retry++);
      });
    };
    subscriptions.push(transport.subscribe(() => { retry = 0; refresh(); }));
    subscriptions.push(transport.subscribeContext(() => {
      if (!lifetime.signal.aborted)
        this.#set({ contextRevision: this.#snapshot.contextRevision + 1 });
    }));
    this.#set({ runtime, session, failure: false });
    refresh();
  }
}
