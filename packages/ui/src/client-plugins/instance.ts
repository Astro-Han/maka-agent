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

import type { ClientContext, ClientDescriptor, ClientIdentity, ClientPlugin, ClientRemote } from '@maka-agent/plugin-sdk/client';
import type { Json } from '@maka-agent/plugin-sdk/host';
import type { SlotEntry } from './slots.js';

type Cleanup = () => void | PromiseLike<void>;
interface Effect { readonly setup: () => void | Cleanup; cancelled: boolean; cleanup?: Cleanup }
export type ClientRemoteFactory = (identity: ClientIdentity, signal: AbortSignal) => {
  readonly api: ClientRemote;
  close(): Promise<void>;
};

export class ClientInstance {
  readonly lifetime = new AbortController();
  readonly slots: SlotEntry[] = [];
  readonly #effects: Effect[] = [];
  readonly #onError: (error: unknown) => void;
  #phase: 'staged' | 'active' | 'retired' = 'staged';
  #shutdown?: Promise<void>;
  #initializing?: Promise<void>;
  readonly #remote?: ReturnType<ClientRemoteFactory>;
  readonly #identity: ClientIdentity;

  constructor(readonly descriptor: ClientDescriptor, onError: (error: unknown) => void, remote?: ClientRemoteFactory) {
    this.#onError = onError;
    this.#identity = Object.freeze({
      entryId: descriptor.entryId, extensionId: descriptor.extensionId,
      activation: descriptor.activation, contentDigest: descriptor.contentDigest,
      clientDigest: descriptor.clientDigest,
    });
    this.#remote = remote?.(this.#identity, this.lifetime.signal);
  }

  initialize(plugin: ClientPlugin, document: Document): Promise<void> {
    this.#initializing = this.#initialize(plugin, document);
    return this.#initializing;
  }

  async #initialize(plugin: ClientPlugin, document: Document): Promise<void> {
    const context: ClientContext = {
      identity: this.#identity,
      signal: this.lifetime.signal,
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
          const assertActive = () => this.#assertActive();
          return async function* (input: I, signal?: AbortSignal): AsyncGenerator<O> {
            assertActive();
            if (!open) throw new Error('Client Remote is unavailable');
            for await (const item of open(input, signal)) yield item;
          };
        },
      },
      slots: {
        register: (slot, key, component, order = 0) => {
          this.#assertStaged();
          if (!key || key.length > 128 || !Number.isFinite(order) || this.slots.length >= 128)
            throw new Error('Invalid or excessive Client slot registration');
          if (this.slots.some((entry) => entry.slot === slot && entry.key === key))
            throw new Error('Duplicate Client slot key');
          const entry: SlotEntry = { owner: context.identity, slot, key, component, order };
          this.slots.push(entry);
          return () => {
            this.#assertStaged();
            const index = this.slots.indexOf(entry);
            if (index >= 0) this.slots.splice(index, 1);
          };
        },
      },
      effect: (setup) => {
        this.#assertStaged();
        if (this.#effects.length >= 128) throw new Error('Client effect limit exceeded');
        const effect: Effect = { setup, cancelled: false };
        this.#effects.push(effect);
        return () => { this.#assertStaged(); effect.cancelled = true; };
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
      this.#effects.push({ setup: () => {}, cancelled: false, cleanup });
    }
    this.lifetime.signal.throwIfAborted();
  }

  publish(): void {
    this.#assertStaged();
    this.#phase = 'active';
    for (const effect of this.#effects) {
      if (effect.cancelled || effect.cleanup) continue;
      effect.cleanup = effect.setup() || undefined;
    }
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
      if (!effect.cleanup) continue;
      try { await effect.cleanup(); } catch (error) { errors.push(error); this.#onError(error); }
    }
    this.#effects.length = 0;
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
