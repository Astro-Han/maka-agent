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
export interface CallContext {
  readonly invocation: Invocation;
  readonly operationId?: string | null;
  readonly signal: Cancellation;
  readonly processes: Processes;
  readonly terminals: Terminals;
  readonly http: Http;
  /** Uses admitted ∩ current permissions and the invocation's tool ceiling. */
  readonly files: Files;
  readonly llm: import('./llm.js').Llm;
  readonly clients: import('./clients.js').ClientCapabilities;
  readonly services: Services;
}
export type ServiceContext = { readonly configuration: readonly Json[] } & (
  | CallContext
  | { readonly signal: Cancellation; readonly invocation?: undefined }
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
      request: { invocation: Invocation },
      call: { signal: Cancellation },
    ) => Awaitable<string | null | undefined>);
export interface PromptSection {
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
export interface HostContext {
  readonly remote: {
    method<I extends Json, O extends Json>(
      name: string,
      invoke: (input: I, caller: RemoteCaller) => Awaitable<O>,
    ): Promise<Registration>;
    stream<I extends Json, O extends Json>(
      name: string,
      open: (input: I, caller: RemoteCaller) => Awaitable<RemoteStream<O>>,
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
  readonly executions: Executions;
  readonly credentials: Credentials;
  sleep(milliseconds: number): Promise<void>;
  /** Cleanup runs in reverse registration order. */
  effect(dispose: () => Awaitable<void>): void;
  /** Stage during activation; starts only after publication becomes effective. */
  run(task: () => Awaitable<void>): void;
}
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
