<!--
  Licensed to the Apache Software Foundation (ASF) under one
  or more contributor license agreements.  See the NOTICE file
  distributed with this work for additional information
  regarding copyright ownership.  The ASF licenses this file
  to you under the Apache License, Version 2.0 (the
  "License"); you may not use this file except in compliance
  with the License.  You may obtain a copy of the License at

      http://www.apache.org/licenses/LICENSE-2.0

  Unless required by applicable law or agreed to in writing,
  software distributed under the License is distributed on an
  "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
  KIND, either express or implied.  See the License for the
  specific language governing permissions and limitations
  under the License.
-->

# Maka plugin SDK

[简体中文](README.zh-CN.md)

TypeScript contracts for trusted Rust-Host plugins. Host SDK API **1** is independent of Maka's application version. This workspace is not yet published.

```ts
import type { HostPlugin } from '@maka-agent/plugin-sdk/host';

const activate: HostPlugin = async (ctx) => {
  await ctx.tools.register<{ text: string }>(
    {
      name: 'Echo',
      description: 'Return the supplied text.',
      inputSchema: {
        type: 'object',
        properties: { text: { type: 'string' } },
        required: ['text'],
        additionalProperties: false,
      },
    },
    ({ text }) => ({ text }),
  );
};
export default activate;
```

Bundle one ESM entrypoint without imports or top-level await. Set `runtime: { entry: "index.mjs", sdkVersion: 1, vm: "shared" }` in `maka.extension.json`; `dedicated` requests a separate VM for that package generation.

Prompt providers receive a tagged Session or model-step context, never fabricated tool authority. Sections and dynamic context default to templates; use `format: 'plain'` for resolved or user-authored text. A `complete` section replaces other prompt Contributions, not explicit Session/child instructions. Physical retries reuse the same frozen composition.

- Activation stages registrations. Start business loops with `ctx.run`, after publication. Register cleanup with `ctx.effect`; observe `ctx.signal`.
- `ctx.input.prepare(name, callback)` prepares text before admission, returning unchanged, ready with a receipt, or blocked. It has no invocation authority and cannot replace attachments or prior receipts. Host stamps provenance and preserves accepted input across replay. Keep callbacks pure; close/re-register when mutable sources change so stale preparations cannot be admitted.
- Tool and executor callbacks receive invocation-bound services and processes. Old call handles expire; instance processes must be reopened through the next call's `processes.open(id)`. Unloading owns their cleanup.
- Process spawn requires the invocation's current Bypass permission, uses its frozen workspace, and accepts an absolute executable plus argv. Default lifetime is the invocation. Strings written to stdin are UTF-8; decode output incrementally with `TextDecoder`.
- `call.terminals` uses the same command and lifetime contract with native PTYs, serialized input/resize receipts, bounded output with explicit reset events, and durable exit/cleanup. Instance terminals must be reopened by later calls; retirement closes them. The Host shares its terminal parser VM rather than allocating one per PTY.
- `call.http.request` provides invocation-bound HTTP using Host proxy settings and Bypass permission. Read bounded byte chunks with `response.next()`; `null` means complete, whereas truncation fails. Responses close on invocation completion or retirement. No automatic retries or redirects: remote side-effect recovery belongs to the plugin.
- Session-scoped execution commands use stable operation IDs: equal retries return the original receipt; changed content conflicts. Profile entries receive no implicit Session authority.
- `executions.submit({ orchestrationMode })` selects a mode for that execution without changing the Session default. `query()` exposes `attentionId` for the current blocking interaction set or handoff, stable across unrelated log writes.
- `call.clients.tools()` lists only the invocation's frozen Client Capability tools; `call.clients.call({ name, input })` uses Host permissions, approval/forms, cancellation and durable settlement. Model and Executor invocations share this boundary. Later publication or permission widening does not expand it.
- Storage is package/scope namespaced with CAS revisions and atomic batches. Tombstones retain revisions. Business migrations belong to the plugin.
- `call.llm.generate` uses the invocation's frozen model and Host proxy/OAuth, sharing the main model executor. It sends only its explicit prompt/system, with no tools or inherited conversation. Defaults to 2048 output tokens; input is limited to 256 KiB and the response stream to 2 MiB. Results and reported usage settle durably before delivery; missing usage remains unknown. Abandoned calls are cancelled and drained with their invocation.
- `ctx.credentials` stores package/scope-isolated secrets in the Host's private credential database, not ordinary storage or execution history. Writes compare revisions; deletion retains a tombstone. Limits: 64 KiB per value, 256 stable keys per namespace. Protection follows the existing vault's file permissions/ACL, not an additional encryption layer.
- `call.files` provides typed read/write/edit/glob/grep/patch operations through Host tools and durable settlement. Permissions cannot exceed the admitted or current Session boundary or tool ceiling. Reads return bounded pages or persisted image references; search results report completeness. File effects remain owned even if their Promise is abandoned. Service forwarding preserves this settlement obligation.
- Only encoding and URL globals are installed. Files, network, timers, and processes are not ambient Node APIs; use Host SDK services. No hostile-code isolation is promised.

`npm --workspace @maka-agent/plugin-sdk run typecheck` also checks the actual plugin fixture exercised by Rust Host integration tests.

`ctx.executions.createChild({ ..., workspace: 'isolated_git' })` binds a Host-owned linked worktree to the child Session. The parent must permit writes and have a clean repository-root workspace. Replays and Host restarts preserve child changes. After execution and workspace writers settle, `workspacePatch(operationId)` publishes an immutable, base-relative Git patch artifact, including committed and uncommitted changes; it never merges into the parent. Export before advancing the child to another Turn. Workspaces remain available for resumption; disabling a plugin does not delete them. Sparse checkout, submodules, external Git filters, and patches exceeding 50 MiB fail explicitly. Host Git operations use gix, not a system Git executable.

## Client SDK

Client SDK API **1** uses React supplied by Desktop. Export a `ClientPlugin` from `@maka-agent/plugin-sdk/client`; its `activate(ctx, config)` stages keyed Slot registrations and effects. Business setup belongs in `ctx.effect`; return cleanup or observe `ctx.signal`. Registration closes after initialization. Cleanup failure requires reloading the document before that Entry can activate again.

Build with `buildClient({ packageId, entryPoint })` from `@maka-agent/plugin-sdk/build` (requires esbuild in the author's build environment). Save the returned JavaScript and declare `client: { entry: "client.js", sdkVersion: 1 }` in the manifest. The loader checks exact bytes and SDK compatibility before execution. Bundles share the trusted Renderer, not a sandbox; they have no Node compatibility layer.

Slots include `session.composer.before`, `workspace.composer.before` and `workspace.manage`. Workspace props describe a candidate, not an authorized filesystem path. Composer Slots offer draft-only `appendText` and `publishSuggestions`. A publication has `update(items)` and `dispose()`; update the same owner on refresh and dispose it on effect cleanup. Item identities survive refresh; publications retire with their owner or target and never submit a message. Each registration has an Entry-local key and optional numeric order. Declare package imports in manifest dependencies; React, `react/jsx-runtime`, and the Client SDK are supplied by Desktop. Do not bundle another React instance.

Optional `ctx.localFiles.pick()` / `open(path)` use Desktop-local paths only. They are unavailable for remote Host files. Desktop validates the published Client identity before native actions and discards picker results after navigation or retirement.

Desktop supplies `@maka/ui/plugin` as a shared UI module (currently `Button`). Import supported components from this entry instead of bundling another component-library instance. It is not the internal UI package's complete API.

Host plugins publish `ctx.remote.method(name, callback)` or `ctx.remote.stream(name, open)`. Client plugins obtain a callable with `ctx.remote.method<Input, Output>(name, sessionId?)` or an async-iterable factory with `ctx.remote.stream<Input, Output>(name, sessionId?)`. Calls start only after UI publication. Handles retain their original Host connection and backend registration; replacement never redirects them. Breaking iteration closes its stream; retiring UI or navigating closes its document. Remote callers are not Agent invocations and receive no implicit process permission.

Native endpoints accepting caller-supplied Host paths declare `Endpoint::requiring_host_paths()`. Host checks that grant at both bind and call, even for a borrowed registration target. Project-ID and existing-Session queries do not require raw-path authority; plugins receive explicitly injected read-only views.
