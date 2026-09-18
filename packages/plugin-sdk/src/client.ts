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

import type { ComponentType } from 'react';
import type { Awaitable, Json } from './host.js';

/** Independent of the application release and Host protocol epoch. */
export const CLIENT_SDK_VERSION = 1;

export interface ClientIdentity {
  readonly entryId: string;
  readonly extensionId: string;
  readonly activation: string;
  readonly contentDigest: string;
  readonly clientDigest: string;
}

export interface ClientDescriptor extends ClientIdentity {
  readonly sdkVersion: number;
  readonly totalBytes: number;
  readonly dependencies: readonly string[];
  readonly config: unknown;
}

/** Augment this interface for slots agreed upon by a product and its plugins. */
export interface ClientSlots {
  'session.composer.before': {
    readonly sessionId: string;
    readonly locale: 'en' | 'zh-CN' | 'zh-TW';
    /** Canonical Session ID from this plugin's Host, not a Desktop projection key. */
    readonly onOpenSession: (sessionId: string) => void;
  };
}

export interface ClientContext {
  readonly identity: ClientIdentity;
  readonly signal: AbortSignal;
  readonly remote: ClientRemote;
  readonly slots: {
    /** Registrations are staged until initialization succeeds. Keys are local to this Entry. */
    register<K extends keyof ClientSlots>(
      slot: K,
      key: string,
      component: ComponentType<ClientSlots[K]>,
      order?: number,
    ): () => void;
  };
  /** Setup starts only after publication; cleanup runs in reverse order on retirement. */
  effect(setup: () => void | (() => Awaitable<void>)): () => void;
  style(css: string): () => void;
}

/** Handles bind once. Retirement never redirects a call to a new implementation. */
export interface ClientRemote {
  method<I extends Json, O extends Json>(
    name: string,
    sessionId?: string,
  ): (input: I) => Promise<O>;
  stream<I extends Json, O extends Json>(
    name: string,
    sessionId?: string,
  ): (input: I, signal?: AbortSignal) => AsyncIterable<O>;
}

export interface ClientPlugin {
  activate(context: ClientContext, config: unknown): Awaitable<void | (() => Awaitable<void>)>;
}

/** Prebuilt bundles register synchronously; factories must not start business work. */
export interface ClientBundle {
  readonly id: string;
  readonly factory: (require: (specifier: string) => unknown) => {
    readonly default: ClientPlugin;
    readonly [exportName: string]: unknown;
  };
}
