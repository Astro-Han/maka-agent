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

import { Component, useSyncExternalStore, type ReactNode } from 'react';
import type { ClientIdentity, ClientSlots } from '@maka-agent/plugin-sdk/client';

export interface SlotEntry {
  readonly owner: ClientIdentity;
  readonly slot: keyof ClientSlots;
  readonly key: string;
  readonly order: number;
  /** Existential input: only the matching slot may invoke this renderer. */
  readonly render: (input: never) => ReactNode;
}

/** A single immutable publication; plugins cannot mutate the live registry. */
export class ClientSlotStore {
  readonly #listeners = new Set<() => void>();
  #entries: readonly SlotEntry[] = [];
  snapshot = (): readonly SlotEntry[] => this.#entries;
  subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  };
  replace(entries: readonly SlotEntry[]): void {
    this.#entries = [...entries].sort((a, b) =>
      a.order - b.order || compare(a.owner.entryId, b.owner.entryId) || compare(a.key, b.key));
    for (const listener of this.#listeners) listener();
  }
}

export function ClientSlot<K extends keyof ClientSlots>(props: {
  readonly store: ClientSlotStore;
  readonly name: K;
  readonly entryId?: string;
  readonly className?: string;
  readonly input: ClientSlots[K];
  readonly onError: (owner: ClientIdentity, error: unknown) => void;
}): ReactNode {
  const entries = useSyncExternalStore(props.store.subscribe, props.store.snapshot, props.store.snapshot);
  const matching = entries.filter((entry) => entry.slot === props.name &&
    (!props.entryId || entry.owner.entryId === props.entryId));
  if (!matching.length) return null;
  return <div className={props.className}>{matching.map((entry) => (
    <SlotBoundary key={`${entry.owner.activation}/${entry.key}`} owner={entry.owner} onError={props.onError}>
      {entry.render(props.input as never)}
    </SlotBoundary>
  ))}</div>;
}

class SlotBoundary extends Component<{
  readonly owner: ClientIdentity;
  readonly onError: (owner: ClientIdentity, error: unknown) => void;
  readonly children: ReactNode;
}, { failed: boolean }> {
  state = { failed: false };
  static getDerivedStateFromError(): { failed: boolean } { return { failed: true }; }
  componentDidCatch(error: unknown): void { this.props.onError(this.props.owner, error); }
  render(): ReactNode { return this.state.failed ? null : this.props.children; }
}

function compare(a: string, b: string): number { return a < b ? -1 : a > b ? 1 : 0; }
