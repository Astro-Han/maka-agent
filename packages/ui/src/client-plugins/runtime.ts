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

import {
  CLIENT_SDK_VERSION,
  type ClientBundle,
  type ClientDescriptor,
  type ClientIdentity,
} from '@maka-agent/plugin-sdk/client';
import { loadClientBundle } from './bundle.js';
import { ClientInstance, type ClientRemoteFactory, type ClientFilesFactory, type ClientAuthorizationFactory, type ClientEventsFactory } from './instance.js';
import { ClientSlotStore } from './slots.js';

export interface ClientSnapshot {
  readonly revision: string;
  /** New transport identity revokes handles even when package bytes did not change. */
  readonly connection?: string;
  /** Host process epoch, distinct from the Client's transport/target identity. */
  readonly hostEpoch?: string;
  readonly entries: readonly ClientDescriptor[];
}
export interface ClientDiagnostic { readonly identity?: ClientIdentity; readonly error: unknown }
export interface ClientRuntimeOptions {
  readonly document: Document;
  readonly modules: Readonly<Record<string, unknown>>;
  readonly source: (descriptor: ClientDescriptor, signal: AbortSignal) => Promise<string>;
  readonly report: (diagnostic: ClientDiagnostic) => void;
  readonly remote?: ClientRemoteFactory;
  readonly localFiles?: ClientFilesFactory;
  readonly authorization?: ClientAuthorizationFactory;
  readonly events?: ClientEventsFactory;
}

export class ClientRuntime {
  readonly slots = new ClientSlotStore();
  readonly #options: ClientRuntimeOptions;
  readonly #fenced = new Set<string>();
  readonly #factories = new Map<string, ClientBundle['factory']>();
  readonly #modules = new Map<string, ReturnType<ClientBundle['factory']>>();
  #active: ClientInstance[] = [];
  #revision?: string;
  #connection?: string;
  #hostEpoch?: string;
  #pending?: AbortController;
  #work: Promise<void> = Promise.resolve();
  #closed = false;

  constructor(options: ClientRuntimeOptions) { this.#options = options; }

  invalidate(): void {
    this.#pending?.abort(new Error('Client snapshot superseded'));
  }

  reconcile(snapshot: ClientSnapshot): Promise<void> {
    if (this.#closed) return Promise.reject(new Error('Client runtime is closed'));
    this.invalidate();
    const request = new AbortController();
    this.#pending = request;
    this.#work = this.#work.catch(() => {}).then(async () => {
      request.signal.throwIfAborted();
      if (this.#revision === snapshot.revision && this.#connection === snapshot.connection && this.#hostEpoch === snapshot.hostEpoch) return;
      this.#revision = undefined;
      const timeout = setTimeout(() => request.abort(new Error('Client initialization timed out')), 30_000);
      try { await this.#replace(snapshot, request.signal); }
      finally { clearTimeout(timeout); }
    });
    return this.#work;
  }

  async close(): Promise<void> {
    this.#closed = true;
    this.#pending?.abort(new Error('Client runtime closed'));
    this.slots.replace([]);
    for (const instance of this.#active) instance.retire();
    await this.#work.catch(() => {});
    const retired = this.#active;
    this.#active = [];
    this.#factories.clear();
    this.#modules.clear();
    await this.#retire(retired);
  }

  async #replace(snapshot: ClientSnapshot, signal: AbortSignal): Promise<void> {
    const descriptors = index(snapshot.entries);
    const staged: ClientInstance[] = [];
    // Withdraw stale UI before any asynchronous loading, including failed candidates.
    const changed = changedPackages(this.#active.map((instance) => instance.descriptor), snapshot.entries);
    if (snapshot.connection !== this.#connection || snapshot.hostEpoch !== this.#hostEpoch) {
      for (const instance of this.#active) changed.add(instance.descriptor.extensionId);
    }
    this.#connection = snapshot.connection;
    this.#hostEpoch = snapshot.hostEpoch;
    const stale = this.#active.filter((instance) => changed.has(instance.descriptor.extensionId));
    this.#active = this.#active.filter((instance) => !changed.has(instance.descriptor.extensionId));
    for (const id of changed) this.#modules.delete(id);
    this.slots.replace(this.#active.flatMap((instance) => instance.slots));
    try {
      await this.#retire(stale);
      signal.throwIfAborted();
      let totalBytes = 0;
      for (const descriptor of descriptors.values()) {
        if (descriptor.sdkVersion !== CLIENT_SDK_VERSION) throw new Error('Incompatible Client SDK');
        totalBytes += descriptor.totalBytes;
        if (totalBytes > 32 * 1024 * 1024) throw new Error('Client catalog byte budget exceeded');
        if (!this.#factories.has(moduleKey(descriptor))) {
          const source = await interruptible(this.#options.source(descriptor, signal), signal);
          const factory = await interruptible(loadClientBundle(descriptor, source, this.#options.document, signal), signal);
          this.#factories.set(moduleKey(descriptor), factory);
        }
      }
      const modules = this.#modules;
      const visiting = new Set<string>();
      const materialize = (id: string): ReturnType<ClientBundle['factory']> => {
        const cached = modules.get(id);
        if (cached) return cached;
        if (visiting.has(id)) throw new Error('Client module dependency cycle');
        const descriptor = descriptors.get(id);
        if (!descriptor) throw new Error('Client dependency is not composed: ' + id);
        visiting.add(id);
        for (const dependency of descriptor.dependencies) materialize(dependency);
        const factory = this.#factories.get(moduleKey(descriptor));
        if (!factory) throw new Error('Client factory unavailable');
        const exports = factory((specifier) => {
          if (Object.hasOwn(this.#options.modules, specifier)) return this.#options.modules[specifier];
          if (!descriptor.dependencies.includes(specifier)) throw new Error('Undeclared Client dependency: ' + specifier);
          return materialize(specifier);
        });
        if (!exports || typeof exports.default?.activate !== 'function')
          throw new Error('Client bundle must export a default plugin');
        modules.set(id, exports);
        visiting.delete(id);
        return exports;
      };
      for (const descriptor of snapshot.entries) {
        signal.throwIfAborted();
        if (this.#active.some((instance) => instance.descriptor.entryId === descriptor.entryId)) continue;
        if (this.#fenced.has(descriptor.entryId)) throw new Error('Client cleanup unconfirmed; reload the document');
        const instance = new ClientInstance(descriptor, (error) => this.#options.report({ identity: descriptor, error }), this.#options.remote, this.#options.localFiles, snapshot.hostEpoch, this.#options.authorization, this.#options.events);
        staged.push(instance);
        await interruptible(instance.initialize(materialize(descriptor.extensionId).default, this.#options.document), signal);
      }
      signal.throwIfAborted();
      for (const instance of staged) instance.publish();
      this.#active.push(...staged);
      this.slots.replace(this.#active.flatMap((instance) => instance.slots));
      this.#revision = snapshot.revision;
    } catch (error) {
      await this.#retire(staged).catch(() => {});
      this.#options.report({ error });
      throw error;
    } finally {
      const used = new Set(this.#active.map((instance) => moduleKey(instance.descriptor)));
      for (const key of this.#factories.keys()) if (!used.has(key)) this.#factories.delete(key);
      const packages = new Set(this.#active.map((instance) => instance.descriptor.extensionId));
      for (const id of this.#modules.keys()) if (!packages.has(id)) this.#modules.delete(id);
    }
  }

  async #retire(instances: readonly ClientInstance[]): Promise<void> {
    for (const instance of instances) instance.retire();
    const results = await Promise.allSettled([...instances].reverse().map(async (instance) => {
      const deadline = new AbortController();
      const timer = setTimeout(() => deadline.abort(new Error('Client cleanup timed out')), 5_000);
      try { await interruptible(instance.shutdown(), deadline.signal); }
      catch (error) { this.#fenced.add(instance.descriptor.entryId); throw error; }
      finally { clearTimeout(timer); }
    }));
    const errors = results.filter((result) => result.status === 'rejected');
    if (errors.length) throw new AggregateError(errors.map((result) => result.reason), 'Client cleanup unconfirmed');
  }
}

function binding(descriptor: ClientDescriptor): string {
  return JSON.stringify([descriptor.entryId, descriptor.extensionId, descriptor.activation, descriptor.contentDigest,
    descriptor.clientDigest, descriptor.totalBytes, descriptor.sdkVersion, descriptor.dependencies, descriptor.config]);
}

/** Module exports share a package lifetime; imported exports share its invalidation. */
function changedPackages(previous: readonly ClientDescriptor[], next: readonly ClientDescriptor[]): Set<string> {
  const before = new Set(previous.map(binding));
  const after = new Set(next.map(binding));
  const changed = new Set([
    ...previous.filter((entry) => !after.has(binding(entry))),
    ...next.filter((entry) => !before.has(binding(entry))),
  ].map((entry) => entry.extensionId));
  let expanded: boolean;
  do {
    expanded = false;
    for (const entry of next) {
      if (!changed.has(entry.extensionId) && entry.dependencies.some((id) => changed.has(id))) {
        changed.add(entry.extensionId);
        expanded = true;
      }
    }
  } while (expanded);
  return changed;
}

function moduleKey(descriptor: ClientDescriptor): string {
  return descriptor.extensionId + '/' + descriptor.clientDigest;
}

function index(entries: readonly ClientDescriptor[]): Map<string, ClientDescriptor> {
  if (entries.length > 4096) throw new Error('Client Entry limit exceeded');
  const byPackage = new Map<string, ClientDescriptor>();
  const ids = new Set<string>();
  for (const entry of entries) {
    if (ids.has(entry.entryId)) throw new Error('Duplicate Client Entry');
    ids.add(entry.entryId);
    const previous = byPackage.get(entry.extensionId);
    if (previous && (previous.contentDigest !== entry.contentDigest || previous.clientDigest !== entry.clientDigest))
      throw new Error('Mixed Client package generations');
    byPackage.set(entry.extensionId, entry);
  }
  return byPackage;
}

export function interruptible<T>(work: Promise<T>, signal: AbortSignal): Promise<T> {
  return new Promise((resolve, reject) => {
    const abort = () => {
      signal.removeEventListener('abort', abort);
      reject(signal.reason);
    };
    signal.addEventListener('abort', abort, { once: true });
    work.then(
      (value) => { signal.removeEventListener('abort', abort); resolve(value); },
      (error) => { signal.removeEventListener('abort', abort); reject(error); },
    );
    if (signal.aborted) abort();
  });
}
