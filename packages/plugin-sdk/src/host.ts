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

import type { Executions, Invocation, MessageContent } from './execution.js';
import type { Processes } from './process.js';
import type { Terminals } from './terminal.js';
import type { Credentials } from './credentials.js';
import type { Http } from './http.js';
import type { Files } from './filesystem.js';

export type * from './execution.js';
export type * from './process.js';
export type * from './terminal.js';
export type * from './credentials.js';
export type * from './http.js';
export type * from './filesystem.js';
export type * from './llm.js';
export type * from './clients.js';

/** Independent API version, used by runtime.sdkVersion in maka.extension.json. */
export const HOST_SDK_VERSION = 1;
export type Json =
  | null
  | boolean
  | number
  | string
  | readonly Json[]
  | { readonly [key: string]: Json };
export type Awaitable<T> = T | PromiseLike<T>;
export interface Registration {
  /** Revokes this exact registration, never a newer replacement. Idempotent. */
  close(): Promise<void>;
}
export interface Cancellation {
  readonly aborted: boolean;
  wait(): Promise<void>;
  throwIfAborted(): void;
}
export interface Identity {
  readonly packageId: string;
  readonly entryId: string;
  readonly scope: 'profile' | 'desktop-ui' | `session:${string}`;
  readonly activation: string;
  /** Inspection only; not a durable identifier. */
  readonly generation: number;
}

/** User-facing Client calls, never an Agent tool invocation or implicit process grant. */
export interface RemoteCaller {
  readonly clientInstanceId: string;
  readonly documentId: string;
  readonly sessionId: string | null;
  readonly signal: Cancellation;
  readonly views: {
    authorize<T>(
      request: import('./authorization.js').AuthorizationRequest,
      use: (call: ResourceContext) => Awaitable<T>,
    ): Promise<T>;
    session(): Promise<SessionView>;
    workspace(input: WorkspaceViewInput): Promise<SessionView>;
  };
}
export interface WorkspaceViewInput {
  workspace: { kind: 'project'; projectId: string } | { kind: 'host_path'; path: string };
  permissionMode: 'explore' | 'ask' | 'bypass';
  collaborationMode: 'agent' | 'plan';
}
export interface SessionView {
  workspace: { target: WorkspaceViewInput['workspace']; hostCwd: string };
  tools: readonly string[];
}
export interface RemoteOptions {
  /** Host rejects the endpoint for callers without path access. */
  access?: 'granted' | 'host_paths';
}
/** Throw an Error carrying this code to preserve its meaning across Remote.
 * Unclassified exceptions become unavailable. An unknown outcome requires
 * domain recovery; it does not imply that the plugin failed to clean up.
 */
export interface RemoteFailure extends Error {
  readonly code: 'invalid' | 'revoked' | 'cancelled' | 'outcome_unknown' | 'unavailable';
}
export interface RemoteStream<T extends Json> {
  next(): Awaitable<IteratorResult<T, void>>;
  /** Signal synchronously; unblock any pending next(). */
  cancel(): void;
  close(): Awaitable<void>;
}
export interface Services {
  get<Input = Json, Output = Json>(name: string): Promise<Service<Input, Output> | undefined>;
}
export interface Service<Input, Output> extends Registration {
  call(input: Input): Promise<Output>;
}
export type CallSource =
  | { readonly kind: 'agent'; readonly invocation: Invocation; readonly operationId: string | null }
  | { readonly kind: 'remote'; readonly requestId: string }
  | { readonly kind: 'background'; readonly grant: string };
export interface ResourceContext {
  /** Borrow this call's execution authority; close the view after use. */
  readonly executions: { open(): Promise<Executions & Registration> };
  readonly signal: Cancellation;
  readonly processes: Processes;
  readonly terminals: Terminals;
  readonly http: Http;
  /** Uses the source's admitted and current permissions. */
  readonly files: Files;
  readonly llm: import('./llm.js').Llm;
  readonly clients: import('./clients.js').ClientCapabilities;
  readonly services: Services;
}
export interface CallContext extends ResourceContext {
  readonly invocation: Invocation;
  readonly operationId?: string | null;
}
export type ServiceContext = { readonly configuration: readonly Json[] } & (
  | (ResourceContext & {
      readonly source: CallSource;
      readonly invocation?: Invocation | null;
      readonly operationId?: string | null;
    })
  | { readonly signal: Cancellation; readonly invocation?: undefined; readonly source?: undefined }
);
export interface ToolDefinition {
  name: string;
  description: string;
  inputSchema: Json;
  directOnly?: boolean;
  semantics?: 'parallel' | 'exclusive_step' | 'finish_turn';
}
export interface ExecutorDefinition {
  name: string;
  displayName: string;
  capabilities?: { thinking?: boolean; toolActivity?: boolean; attachments?: boolean };
}
export interface ExecutorRequest {
  invocation: Invocation;
  conversationKey: string;
  content: MessageContent;
  cwd: string;
  instructions: string | null;
}
export type ExecutorOutput =
  | { type: 'output_delta' | 'thinking_delta'; text: string }
  | { type: 'tool_start'; toolCallId: string; name: string; input: Json }
  | { type: 'tool_progress'; toolCallId: string; text: string }
  | { type: 'tool_result'; toolCallId: string; text: string; isError?: boolean };
export type ExecutorOutcome =
  | { status: 'completed'; text: string }
  | { status: 'cancelled'; reason?: string | null }
  | { status: 'failed'; message: string; code?: string | null; recoverable?: boolean };
export interface ExecutorContext extends CallContext {
  /** Resolves after durable recording. Tool activity is observation, not dispatch. */
  emit(output: ExecutorOutput): Promise<void>;
}
export type TextProvider =
  | string
  | ((
      request: PromptRequest,
      call: { signal: Cancellation },
    ) => Awaitable<string | null | undefined>);
export type PromptRequest =
  | { readonly kind: 'session'; readonly sessionId: string; readonly cwd: string }
  | { readonly kind: 'model_step'; readonly invocation: Invocation; readonly cwd: string };
export interface PromptSection {
  /** Use plain for resolved or user-authored text; template interpolates registered variables. */
  format?: 'plain' | 'template';
  name: string;
  order?: number;
  text: TextProvider;
}
export type StorageData = { kind: 'present'; value: Json } | { kind: 'deleted' };
export interface StorageRecord {
  revision: number;
  data: StorageData;
}
export interface StorageMutation {
  key: string;
  /** null means the key must never have existed; deletions retain a revision. */
  expectedRevision: number | null;
  data: StorageData;
}
export interface BehaviorPreparation {
  instructions?: string;
  toolMode?: 'direct' | 'code_mode';
  nativeTools?: 'workspace' | 'attachments';
  requiredClients?: {
    required: readonly string[];
    optional?: readonly string[];
    private?: readonly string[];
  } | null;
  toolCeiling?: readonly string[] | null;
}
export interface HostContext {
  /** Opens current background authority and confirms cleanup after the callback. */
  withAuthorization<T>(
    id: string,
    use: (call: ResourceContext & { readonly source: CallSource }) => Awaitable<T>,
  ): Promise<T>;
  readonly behaviors: {
    /** Preparation narrows capabilities; it never grants execution authority.
     * Close/re-register when the source changes to invalidate stale admissions.
     */
    register(
      name: string,
      prepare: (
        request: { readonly sessionId: string },
        call: { readonly signal: Cancellation },
      ) => Awaitable<BehaviorPreparation>,
    ): Promise<Registration>;
  };
  readonly input: {
    /** Pure preparation. Close/re-register when its source changes to revoke stale admissions. */
    prepare(
      name: string,
      prepare: (request: InputPreparationRequest) => Awaitable<InputPreparationOutcome>,
    ): Promise<Registration>;
  };
  readonly remote: {
    method<I extends Json, O extends Json>(
      name: string,
      invoke: (input: I, caller: RemoteCaller) => Awaitable<O>,
      options?: RemoteOptions,
    ): Promise<Registration>;
    stream<I extends Json, O extends Json>(
      name: string,
      open: (input: I, caller: RemoteCaller) => Awaitable<RemoteStream<O>>,
      options?: RemoteOptions,
    ): Promise<Registration>;
  };
  readonly identity: Identity;
  readonly signal: Cancellation;
  readonly tools: {
    register<Input = Json>(
      definition: ToolDefinition,
      invoke: (input: Input, call: CallContext) => Awaitable<Json>,
    ): Promise<Registration>;
  };
  readonly executors: {
    register(
      definition: ExecutorDefinition,
      execute: (request: ExecutorRequest, call: ExecutorContext) => Awaitable<ExecutorOutcome>,
    ): Promise<Registration>;
  };
  readonly prompt: {
    section(definition: PromptSection & { complete?: boolean }): Promise<Registration>;
    variable(name: string, text: TextProvider): Promise<Registration>;
    context(definition: PromptSection): Promise<Registration>;
  };
  readonly services: Services & {
    provide<Input = Json, Output = Json>(
      name: string,
      invoke: (input: Input, call: ServiceContext) => Awaitable<Output>,
    ): Promise<Registration>;
  };
  readonly storage: {
    read(key: string): Promise<StorageRecord | null>;
    batch(mutations: readonly StorageMutation[]): Promise<StorageRecord[]>;
  };
  /** Non-secret user preferences; no configuration or execution authority. */
  readonly preferences: {
    read(): Promise<{
      revision: number;
      personalization: { displayName: string; assistantTone: string };
      workspaceInstructions: boolean;
    }>;
  };
  readonly executions: {
    /** Restore explicit Host consent; the ID alone is not permission. Closing
     * releases this view, never cancels work already accepted by the Host. */
    restore(id: string): Promise<Executions & Registration>;
  };
  readonly data: PrivateFiles;
  readonly credentials: Credentials;
  sleep(milliseconds: number): Promise<void>;
  /** Cleanup runs in reverse registration order. */
  effect(dispose: () => Awaitable<void>): void;
  /** Stage during activation; starts only after publication becomes effective. */
  run(task: () => Awaitable<void>): void;
}
/** Package/scope-private files, not the user's workspace. Operations are
 * independent; the plugin owns its file format and multi-operation recovery.
 * Paths use portable relative components, never links or parent traversal.
 * Files are flushed, but namespace mutations do not promise power-loss durability.
 */
export interface PrivateFiles {
  /** Reads at most 1 MiB (default 64 KiB). A non-null cursor means more bytes exist. */
  read(input: { path: string; offset?: number; limit?: number }): Promise<{
    bytes: Uint8Array;
    nextOffset: number | null;
  }>;
  /** Flush a bounded write, not an atomic replacement. outcome_unknown requires recovery. */
  write(input: {
    path: string;
    offset?: number;
    bytes: Uint8Array | readonly number[];
    truncate?: boolean;
  }): Promise<void>;
  /** Lexical pagination, not a snapshot across concurrent directory mutations. */
  list(input?: { path?: string; after?: string | null; limit?: number }): Promise<{
    entries: { name: string; kind: 'file' | 'directory' | 'other' }[];
    nextAfter: string | null;
  }>;
  createDirectory(path: string): Promise<void>;
  /** Removes only a file/link or an empty directory. */
  remove(path: string): Promise<void>;
  /** May replace an existing destination file. */
  rename(from: string, to: string): Promise<void>;
}
/** No invocation exists yet; preparation does not grant tool, file or process authority. */
export interface InputReceipt {
  readonly source: {
    readonly kind: 'input';
    readonly name: string;
    readonly packageId: string;
    readonly entryId: string;
    readonly activation: string;
    readonly revision: string;
  };
  readonly receipt: Json;
}
export interface InputPreparationRequest {
  readonly sessionId: string;
  readonly cwd: string;
  readonly content: MessageContent;
  /** Evidence from preceding providers; a provider cannot replace it. */
  readonly preparation: readonly InputReceipt[];
  readonly selections: Readonly<Record<string, readonly string[]>>;
  readonly tools: readonly string[];
  readonly signal: Cancellation;
}
export type InputPreparationOutcome =
  | { readonly kind: 'unchanged' }
  | {
      readonly kind: 'ready';
      readonly content: MessageContent;
      readonly receipt: Json;
      readonly requiredTools?: readonly string[];
    }
  | { readonly kind: 'blocked'; readonly message: string; readonly receipt: Json };
export type HostPlugin<Config = Json> = (
  context: HostContext,
  config: Config,
) => Awaitable<void | (() => Awaitable<void>)>;
export interface HostError extends Error {
  code:
    | 'invalid'
    | 'revoked'
    | 'conflict'
    | 'outcome_unknown'
    | 'unavailable'
    | 'busy'
    | 'not_found';
}
