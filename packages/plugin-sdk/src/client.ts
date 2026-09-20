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
import type { ClientAuthorization } from './authorization.js';
export type {
  ClientAuthorization,
  AuthorizationCapability,
  AuthorizationScope,
  AuthorizationRequest,
  AuthorizationGrant,
} from './authorization.js';

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
  /** Resolve a plugin-owned Session for an explicitly selected workspace provider. */
  'session.resolve': {
    readonly contextRevision?: number;
    readonly locale: 'en' | 'zh-CN' | 'zh-TW';
    readonly onResolving: () => void;
    readonly onResolved: (sessionId: string, signal: AbortSignal) => void;
    readonly onError: (message: string) => void;
  };
  'workspace.composer.before': ClientWorkspace & {
    readonly contextRevision?: number;
    readonly locale: 'en' | 'zh-CN' | 'zh-TW';
    readonly appendText?: (text: string) => void;
    readonly publishSuggestions?: (items: readonly ComposerSuggestion[]) => ComposerPublication;
  };
  'workspace.manage': ClientWorkspace & {
    readonly contextRevision?: number;
    readonly section: string;
    readonly locale: 'en' | 'zh-CN' | 'zh-TW';
  };
  'session.composer.before': {
    /** Invalidation hint only; Host still resolves the authoritative Session. */
    readonly contextRevision?: number;
    readonly sessionId: string;
    readonly locale: 'en' | 'zh-CN' | 'zh-TW';
    /** Canonical Session ID from this plugin's Host, not a Desktop projection key. */
    readonly onOpenSession: (sessionId: string) => void;
    /** Edit the current draft only; never submits a message or changes Session. */
    readonly appendText?: (text: string) => void;
    readonly publishSuggestions?: (items: readonly ComposerSuggestion[]) => ComposerPublication;
  };
}

/** A proposed workspace, not a resolved path or execution permission. */
export interface ClientWorkspace {
  readonly workspace: { kind: 'project'; projectId: string } | { kind: 'host_path'; path: string };
  readonly permissionMode: 'explore' | 'ask' | 'bypass';
  readonly collaborationMode: 'agent' | 'plan';
}

/** Draft-only completion. Choosing one inserts text; it cannot execute a command. */
export interface ComposerSuggestion {
  readonly id: string;
  readonly name: string;
  readonly description?: string;
  readonly insertText: string;
}

/** One publisher's draft suggestions. Updating keeps item identities stable. */
export interface ComposerPublication {
  update(items: readonly ComposerSuggestion[]): void;
  dispose(): void;
}

export interface ClientContext {
  readonly identity: ClientIdentity;
  /** Originating Host epoch. Changes retire this instance; retries never follow a new Host. */
  readonly hostEpoch?: string;
  readonly signal: AbortSignal;
  readonly remote: ClientRemote;
  readonly authorization: ClientAuthorization;
  /** Optional desktop-local paths; never interpreted as paths on a remote Host. */
  readonly localFiles?: ClientLocalFiles;
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

export interface ClientLocalFiles {
  pick(): Promise<string | null>;
  open(path: string): Promise<void>;
}
export type ClientFileRequest = { kind: 'pick' } | { kind: 'open'; path: string };

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
