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

import type { ClientContext, ClientDescriptor, ClientIdentity, ClientPlugin, ClientRemote, ClientLocalFiles, ClientAuthorization } from '@maka-agent/plugin-sdk/client';
import type { Json } from '@maka-agent/plugin-sdk/host';
import type { ClientEvents, ClientLabel } from '@maka-agent/plugin-sdk/client';
import type { SlotEntry } from './slots.js';
import { createElement } from 'react';

type Cleanup = () => void | PromiseLike<void>;
interface Effect {
  readonly setup: () => void | Cleanup;
  state: 'staged' | 'starting' | 'active' | 'released';
  cleanup?: Cleanup;
  settlement?: Promise<void>;
}
export type ClientRemoteFactory = (identity: ClientIdentity, signal: AbortSignal) => {
  readonly api: ClientRemote;
  close(): Promise<void>;
};
export type ClientFilesFactory = (identity: ClientIdentity, signal: AbortSignal) => ClientLocalFiles | undefined;
export type ClientAuthorizationFactory = (identity: ClientIdentity, signal: AbortSignal) => ClientAuthorization;
export type ClientEventsFactory = (identity: ClientIdentity, signal: AbortSignal) => {
  subscribe(...args: Parameters<ClientEvents['subscribe']>): Cleanup;
};

export class ClientInstance {
  readonly lifetime = new AbortController();
  readonly slots: SlotEntry[] = [];
  readonly #effects = new Set<Effect>();
  readonly #onError: (error: unknown) => void;
  #phase: 'staged' | 'active' | 'retired' = 'staged';
  #shutdown?: Promise<void>;
  #initializing?: Promise<void>;
  readonly #remote?: ReturnType<ClientRemoteFactory>;
  readonly #identity: ClientIdentity;
  readonly #files?: ClientLocalFiles;
  readonly #authorization?: ClientAuthorization;
  readonly #events?: ReturnType<ClientEventsFactory>;

  constructor(readonly descriptor: ClientDescriptor, onError: (error: unknown) => void, remote?: ClientRemoteFactory, files?: ClientFilesFactory, readonly hostEpoch?: string, authorization?: ClientAuthorizationFactory, events?: ClientEventsFactory) {
    this.#onError = onError;
    this.#identity = Object.freeze({
      entryId: descriptor.entryId, extensionId: descriptor.extensionId,
      activation: descriptor.activation, contentDigest: descriptor.contentDigest,
      clientDigest: descriptor.clientDigest,
    });
    this.#remote = remote?.(this.#identity, this.lifetime.signal);
    this.#files = files?.(this.#identity, this.lifetime.signal);
    this.#authorization = authorization?.(this.#identity, this.lifetime.signal);
    this.#events = events?.(this.#identity, this.lifetime.signal);
  }

  initialize(plugin: ClientPlugin, document: Document): Promise<void> {
    this.#initializing = this.#initialize(plugin, document);
    return this.#initializing;
  }

  async #initialize(plugin: ClientPlugin, document: Document): Promise<void> {
    const context: ClientContext = {
      identity: this.#identity,
      hostEpoch: this.hostEpoch,
      signal: this.lifetime.signal,
      events: {
        subscribe: (request, listener, onError) => {
          let listening = true;
          const fail = (error: Error) => {
            if (!listening || this.lifetime.signal.aborted) return;
            try { if (onError) onError(error); else this.#onError(error); }
            catch (failure) { this.#onError(failure); }
          };
          const release = context.effect(() => {
            if (!this.#events) throw new Error('Client events are unavailable');
            return this.#events.subscribe(request, (event) => {
              if (!listening || this.lifetime.signal.aborted) return;
              try { listener(event); } catch (error) { this.#onError(error); }
            }, fail);
          });
          return () => { listening = false; release(); };
        },
      },
      authorization: {
        approve: async (scope, request) => {
          this.#assertActive();
          if (!this.#authorization) throw new Error('Plugin authorization is unavailable');
          return this.#authorization.approve(scope, request);
        },
        query: async (scope, id) => {
          this.#assertActive();
          if (!this.#authorization) throw new Error('Plugin authorization is unavailable');
          return this.#authorization.query(scope, id);
        },
        revoke: async (scope, id) => {
          this.#assertActive();
          if (!this.#authorization) throw new Error('Plugin authorization is unavailable');
          await this.#authorization.revoke(scope, id);
        },
      },
      localFiles: this.#files ? {
        pick: async () => {
          this.#assertActive();
          const path = await this.#files!.pick();
          this.#assertActive();
          return path;
        },
        open: async (path) => {
          this.#assertActive();
          await this.#files!.open(path);
        },
      } : undefined,
      remote: {
        method: <I extends Json, O extends Json>(name: string, session?: string) => {
          const call = this.#remote?.api.method<I, O>(name, session);
          return async (input: I): Promise<O> => {
            this.#assertActive();
            if (!call) throw new Error('Client Remote is unavailable');
            return await call(input);
          };
        },
        stream: <I extends Json, O extends Json>(name: string, session?: string) => {
          const open = this.#remote?.api.stream<I, O>(name, session);
          return (input: I, signal?: AbortSignal): AsyncIterable<O> => {
            this.#assertActive();
            if (!open) throw new Error('Client Remote is unavailable');
            return open(input, signal);
          };
        },
      },
      slots: {
        register: (slot, key, component, ...options) => {
          this.#assertStaged();
          const settings = options[0];
          if (settings !== undefined && (!settings || typeof settings !== 'object' || Array.isArray(settings)))
            throw new Error('Client slot options must be an object');
          if (settings && Object.keys(settings).some((key) => key !== 'order' && !(slot === 'settings.page' && key === 'label')))
            throw new Error('Unknown Client slot option');
          const order = settings?.order ?? 0;
          const label = slot === 'settings.page'
            ? pageLabel(settings && 'label' in settings ? settings.label : undefined)
            : undefined;
          if (!key || key.length > 128 || !Number.isFinite(order) || this.slots.length >= 128)
            throw new Error('Invalid or excessive Client slot registration');
          if (this.slots.some((entry) => entry.slot === slot && entry.key === key))
            throw new Error('Duplicate Client slot key');
          const entry: SlotEntry = { owner: context.identity, slot, key, order, label,
            render: (input) => createElement(component, input) };
          this.slots.push(entry);
          return () => {
            this.#assertStaged();
            const index = this.slots.indexOf(entry);
            if (index >= 0) this.slots.splice(index, 1);
          };
        },
      },
      effect: (setup) => {
        if (this.#phase === 'retired') throw new Error('Client registration is closed');
        if (typeof setup !== 'function') throw new Error('Client effect must be a function');
        if (this.#effects.size >= 128) throw new Error('Client effect limit exceeded');
        const effect: Effect = { setup, state: 'staged' };
        this.#effects.add(effect);
        if (this.#phase === 'active') this.#start(effect);
        return () => { this.#release(effect); };
      },
      style: (css) => {
        if (new TextEncoder().encode(css).length > 256 * 1024)
          throw new Error('Client stylesheet exceeds limit');
        return context.effect(() => {
          const style = document.createElement('style');
          style.textContent = css;
          document.head.append(style);
          return () => style.remove();
        });
      },
    };
    const cleanup = await plugin.activate(Object.freeze(context), this.descriptor.config);
    if (typeof cleanup === 'function') {
      this.#effects.add({ setup: () => {}, state: 'active', cleanup });
    }
    this.lifetime.signal.throwIfAborted();
  }

  publish(): void {
    this.#assertStaged();
    this.#phase = 'active';
    for (const effect of this.#effects) this.#start(effect);
  }

  #start(effect: Effect): void {
    if (effect.state !== 'staged') return;
    effect.state = 'starting';
    try { effect.cleanup = effect.setup() || undefined; }
    catch (error) { this.#effects.delete(effect); throw error; }
    // Setup can synchronously release itself or retire its owner.
    if (this.#released(effect)) this.#settle(effect);
    else effect.state = 'active';
  }

  #released(effect: Effect): boolean { return effect.state === 'released'; }

  #release(effect: Effect): Promise<void> | undefined {
    if (this.#released(effect)) return effect.settlement;
    const starting = effect.state === 'starting';
    effect.state = 'released';
    if (!starting) this.#settle(effect);
    return effect.settlement;
  }

  #settle(effect: Effect): void {
    const cleanup = effect.cleanup;
    effect.cleanup = undefined;
    if (!cleanup) { this.#effects.delete(effect); return; }
    effect.settlement = Promise.resolve().then(cleanup);
    // A released async resource still counts toward the limit and belongs to
    // shutdown. Keep failed settlements so replacement cannot lose the fence.
    void effect.settlement.then(
      () => { this.#effects.delete(effect); },
      (error) => { this.#onError(error); },
    );
  }

  retire(): void {
    if (this.#phase === 'retired') return;
    this.#phase = 'retired';
    this.lifetime.abort(new Error('Client plugin retired'));
  }

  shutdown(): Promise<void> {
    this.retire();
    this.#shutdown ??= this.#cleanup();
    return this.#shutdown;
  }

  async #cleanup(): Promise<void> {
    // A late activation may return a cleanup handle. Do not declare it released
    // while initialization is still running; the caller owns the drain deadline.
    await this.#initializing?.catch(() => {});
    const errors: unknown[] = [];
    try { await this.#remote?.close(); } catch (error) { errors.push(error); this.#onError(error); }
    for (const effect of [...this.#effects].reverse()) {
      try { await this.#release(effect); } catch (error) { errors.push(error); }
    }
    this.#effects.clear();
    this.slots.length = 0;
    if (errors.length) throw new AggregateError(errors, 'Client plugin cleanup failed');
  }

  #assertStaged(): void {
    if (this.#phase !== 'staged') throw new Error('Client registration is closed');
  }
  #assertActive(): void {
    if (this.#phase !== 'active') throw new Error('Client plugin is not effective');
  }
}

function pageLabel(value: unknown): ClientLabel {
  const valid = (text: unknown): text is string => typeof text === 'string' && text.trim().length > 0 && text.length <= 256;
  if (valid(value)) return value;
  if (value && typeof value === 'object' && 'en' in value && 'zh-CN' in value && 'zh-TW' in value &&
    valid(value.en) && valid(value['zh-CN']) && valid(value['zh-TW']))
    return Object.freeze({ en: value.en, 'zh-CN': value['zh-CN'], 'zh-TW': value['zh-TW'] });
  throw new Error('Settings pages require a nonempty title or localized titles');
}
