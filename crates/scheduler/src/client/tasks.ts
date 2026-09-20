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

import type { ClientContext } from '@maka-agent/plugin-sdk/client';
import type { ScheduledTask } from '@maka/core/scheduled-task';

type Page =
  | { kind: 'page'; revision: number; tasks: ScheduledTask[]; nextCursor: string | null }
  | { kind: 'revision_changed'; expected: number; actual: number };
interface Snapshot {
  tasks: readonly ScheduledTask[];
  error?: string;
}

export class Tasks {
  #context: ClientContext;
  #state: Snapshot = { tasks: [] };
  #listeners = new Set<() => void>();
  #stopped = false;
  #loading: Promise<void> | undefined;
  #again = false;
  constructor(context: ClientContext) {
    this.#context = context;
  }
  snapshot = (): Snapshot => this.#state;
  subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  };
  #publish(state: Snapshot) {
    if (this.#stopped) return;
    this.#state = state;
    for (const listener of this.#listeners) listener();
  }
  refresh = (): Promise<void> => {
    if (this.#stopped) return Promise.resolve();
    if (this.#loading) {
      this.#again = true;
      return this.#loading;
    }
    this.#loading = this.#load()
      .catch((error: unknown) => {
        this.#publish({
          ...this.#state,
          error: error instanceof Error ? error.message : String(error),
        });
      })
      .finally(() => {
        this.#loading = undefined;
        if (this.#again) {
          this.#again = false;
          void this.refresh();
        }
      });
    return this.#loading;
  };
  async #load() {
    const query = this.#context.remote.method<
      { kind: 'query'; query: { kind: 'list'; cursor?: string; expectedRevision?: number } },
      Page
    >('request');
    for (let attempt = 0; attempt < 3; attempt++) {
      const tasks: ScheduledTask[] = [];
      let cursor: string | undefined;
      let revision: number | undefined;
      for (;;) {
        const page = await query({
          kind: 'query',
          query: { kind: 'list', cursor, expectedRevision: revision },
        });
        if (this.#stopped) return;
        if (page.kind === 'revision_changed') break;
        tasks.push(...page.tasks);
        if (tasks.length > 256) throw new Error('Task catalog exceeds its bound');
        if (!page.nextCursor) {
          this.#publish({ tasks });
          return;
        }
        if (page.nextCursor === cursor) throw new Error('Task cursor did not advance');
        cursor = page.nextCursor;
        revision = page.revision;
      }
    }
    throw new Error('Task catalog changed while reading; refresh to retry');
  }
  start(): () => void {
    const lifetime = new AbortController();
    const updates = this.#context.remote.stream<
      null,
      { revision: number; ready: boolean; error: string | null }
    >('changes')(null, lifetime.signal);
    void (async () => {
      try {
        for await (const update of updates) {
          if (this.#stopped) break;
          if (update.ready) await this.refresh();
          else this.#publish({ ...this.#state, error: update.error ?? 'Scheduler is recovering' });
        }
      } catch (error) {
        this.#publish({
          ...this.#state,
          error: error instanceof Error ? error.message : String(error),
        });
      }
    })();
    return () => {
      this.#stopped = true;
      lifetime.abort();
      this.#listeners.clear();
    };
  }
}
