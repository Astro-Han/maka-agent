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

const KEY = 'maka-composer-drafts-v1';
type Drafts = [string, string][];

/** Desktop-owned text only: never an outbox and never automatically submitted. */
export function composerDraftStorage(storage: Pick<Storage, 'getItem' | 'setItem'>) {
  const load = (): Drafts => {
    const value: unknown = JSON.parse(storage.getItem(KEY) ?? '[]');
    return Array.isArray(value) ? value.filter((entry): entry is [string, string] =>
      Array.isArray(entry) && entry.length === 2 && entry.every((field) => typeof field === 'string')) : [];
  };
  return {
    read(key: string | undefined) {
      if (!key) return undefined;
      try { return load().find(([id]) => id === key)?.[1]; } catch { return undefined; }
    },
    write(key: string | undefined, value: string) {
      if (!key) return;
      try {
        const drafts = load().filter(([id]) => id !== key);
        if (value) drafts.push([key, value]);
        // Storage quota failure is visible; never evict another unsent draft.
        storage.setItem(KEY, JSON.stringify(drafts));
      } catch (error) {
        throw new Error('Desktop could not persist the draft; copy it before closing this window.', { cause: error });
      }
    },
  };
}
