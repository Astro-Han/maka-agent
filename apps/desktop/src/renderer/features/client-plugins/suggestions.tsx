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

import { createContext, useContext, useMemo, useSyncExternalStore, type ReactNode } from 'react';
import type { ComposerSuggestion } from '@maka-agent/plugin-sdk/client';

class Suggestions {
  #sources = new Map<number, readonly ComposerSuggestion[]>();
  #next = 0;
  #snapshot: readonly ComposerSuggestion[] = [];
  #listeners = new Set<() => void>();
  snapshot = () => this.#snapshot;
  subscribe = (listener: () => void) => {
    this.#listeners.add(listener);
    return () => { this.#listeners.delete(listener); };
  };
  publish = (items: readonly ComposerSuggestion[]) => {
    if (items.length > 4096 || this.#sources.size >= 128) throw new Error('Too many composer suggestions');
    const owner = ++this.#next;
    this.#sources.set(owner, []);
    const update = (items: readonly ComposerSuggestion[]) => {
      if (!this.#sources.has(owner)) return;
      if (items.length > 4096) throw new Error('Too many composer suggestions');
      this.#sources.set(owner, items.map((item) => Object.freeze({
        ...item, id: 'plugin:' + owner + ':' + item.id,
      })));
      this.#changed();
    };
    update(items);
    return { update, dispose: () => {
      if (this.#sources.delete(owner)) this.#changed();
    } };
  };
  #changed() {
    const next = [...this.#sources.values()].flat();
    if (next.length === this.#snapshot.length && next.every((item, index) => {
      const previous = this.#snapshot[index]!;
      return item.id === previous.id && item.name === previous.name
        && item.description === previous.description && item.insertText === previous.insertText;
    })) return;
    this.#snapshot = next;
    for (const listener of this.#listeners) listener();
  }
}
const Context = createContext<Suggestions | undefined>(undefined);
const EMPTY: readonly ComposerSuggestion[] = [];
const empty = () => EMPTY;
const noop = () => () => {};

export function ComposerSuggestionsProvider({ scope, children }: { scope: string; children: ReactNode }) {
  const store = useMemo(() => new Suggestions(), [scope]);
  return <Context.Provider value={store}>{children}</Context.Provider>;
}
export function usePublishComposerSuggestions() {
  return useContext(Context)?.publish;
}
export function useComposerSuggestions() {
  const store = useContext(Context);
  return useSyncExternalStore(store?.subscribe ?? noop, store?.snapshot ?? empty, empty);
}
