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
  SkillCatalogRepository,
  SkillCatalogRepositoryError,
  type CanonicalSkillInventorySnapshot,
  type SkillCatalogLocalContext,
} from './skill-catalog-repository.js';

/** Owns the model inventory's recovery and shutdown lane. */
export class HostSkillCatalogCoordinator {
  readonly #repository: SkillCatalogRepository;
  #accepting = true;
  #tail: Promise<void> = Promise.resolve();
  #closePromise: Promise<void> | undefined;

  constructor(repository: SkillCatalogRepository) {
    this.#repository = repository;
  }

  recover(): Promise<void> {
    return this.#enqueue(() => this.#repository.recover());
  }

  readCanonicalModelInventory(
    context: SkillCatalogLocalContext,
  ): Promise<CanonicalSkillInventorySnapshot> {
    return this.#admitModelInventoryRead(() =>
      this.#repository.readCanonicalModelInventory(context),
    );
  }

  beginDrain(): void {
    this.#accepting = false;
  }

  close(): Promise<void> {
    if (this.#closePromise) return this.#closePromise;
    this.beginDrain();
    this.#closePromise = this.#tail;
    return this.#closePromise;
  }

  #admitModelInventoryRead<T>(run: () => Promise<T>): Promise<T> {
    if (!this.#accepting) {
      return Promise.reject(
        new SkillCatalogRepositoryError('persistence_failed', 'Runtime Host is draining'),
      );
    }
    return this.#enqueue(run);
  }

  #enqueue<T>(run: () => Promise<T>): Promise<T> {
    const pending = this.#tail.then(run, run);
    this.#tail = pending.then(
      () => undefined,
      () => undefined,
    );
    return pending;
  }
}
