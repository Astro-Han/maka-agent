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

# Rust runtime host

[简体中文](./rust-runtime.zh-CN.md)

The Rust workspace replaces Maka's runtime and host while preserving the
TypeScript client protocol and interactions. Both use protocol epoch 152. The rewrite is incomplete;
unsupported operations return explicit errors.

## Build and run

Requires Rust 1.98+, a native C/C++ toolchain, Node, and the repository's npm
dependencies. Provider SDKs and the terminal parser are bundled at build time;
the executable does not require Node at runtime.

```sh
npm install
cargo build --locked -p maka-cli
cargo run --locked -p maka-cli -- --help
npm run dev
```

Desktop defaults to the Rust host, using `userData/runtime-host-rust`. Existing
TypeScript State Roots are not migrated or adopted. Use a new root for standalone
execution:

```sh
maka host init --root /absolute/path/to/new-root
maka host serve --root /absolute/path/to/new-root
```

Use `target/debug/maka` if the binary is not on PATH. Local transport is a Unix
socket on Linux/macOS or a private named pipe on Windows. An optional
`--websocket 127.0.0.1:0` listener requires authentication; TLS is not implemented.

The single binary also provides `host candidate` for Desktop-owned startup,
`code --log <file>` for a JavaScript cell read from stdin, and
`inspect --log <file>` for committed execution facts.

**There is no OS sandbox.** Code and tools execute with the user's OS permissions.
Do not run untrusted code or point test instances at existing user data.

## Design

- **Log Is the Runtime:** model history, transcript and recovery derive from
  committed semantic facts. Compaction changes the model projection, not history.
  Failed response fragments remain display evidence, not accepted model history;
  user cancellation is not displayed as a provider failure.
- Model messages, content and tool outcomes are typed through provider projection.
  Routing and discovery share typed provider contracts. Tool JSON, schemas and
  provider extensions remain open-ended.
- A State Root has one writer and execution authority. Session, Turn, Run and
  invocation identities remain distinct.
- Manual resume checks a sealed source without claiming it, then atomically opens
  a new continuation. Replay follows the selected lineage, excluding later branches
  and unfinished response fragments. Unknown effects block admission; repeating
  the same admitted request returns its original Turn, including after restart.
- Typed model inventory flows from discovery through storage and catalog projection;
  connection-owned overrides remain separate. Capacity,
  proactive compaction threshold and per-request output budget are independent.
- Tool dispatch commits before effects; outcomes commit afterward. An uncertain
  outcome is not permission to repeat an effect. Cancellation drains admitted work.
- Permission-only grants can widen during execution. New tool calls capture the
  committed boundary; narrowing waits for quiescence and native resource cleanup.
- Read uses `path` for files and Session resources, with bounded pages and
  content-checked continuations. Event addresses expose frozen model evidence,
  not omitted raw output. Large text results persist a bounded first page before
  the next model request; media and original execution facts remain intact.
- Deferred tools become callable on the step after a successful search; committed
  compaction unloads them. Code Mode exposes only `exec`, with the available
  nested catalog in its description. Existing calls retain their captured scope.
  Each logical step captures schemas and handlers together; physical retries
  and returned tool calls reuse that view.
- Explicit Skills in `turn.start` and `turn.message.submit` freeze instructions and receipts at
  admission. Queued messages retain their required tools; promotion and successor
  execution check the actual target Run without reloading Skill files.
- `SkillSearch` and `Skill` share the Run's frozen inventory with explicit loading.
  Discovery exposes bounded metadata; loaded instructions retain readable archive pages.
- The agent-mode Skill selector previews current permissions without binding a Session or
  resolving a model. Bundled and local-library source catalogs report actual installation
  occupancy and validated managed-source aliases. Governance exposes validation,
  preferences and source updates without reading baselines or claiming Run advertisement.
  Pages are revision-bound; Plan and Skill mutations remain unavailable.
- Rust owns storage, network routing, tools and native process/PTY lifetimes.
  Future plugins must enter through catalogs and scoped Host services, sharing
  journaled effects, permissions and draining rather than replacing the Engine.
  One lazy, long-lived V8 serves concurrent model requests and terminal parsers.
  Code Mode cells use separate short-lived isolates. Count and byte limits provide
  backpressure; V8 heap limits are not process-memory containment.
- Code Mode budgets cumulative VM execution, excluding asynchronous tool waits
  and cleanup.
- Request-scoped proxy policy applies to HTTP and Responses WebSocket transport.
  Failed WS handshakes retry five times with exponential backoff, then use HTTP
  through the same policy. Separately, main requests allow up to ten attempts for
  identified transient provider failures, using frozen inputs and cancellable backoff.
  Provider tool activity or replay metadata blocks retries. Unknown/local errors
  and unclassified network failures or deadlines are not retried.

## Code layout

All crates are in `crates/`; directory names describe their responsibilities.

| Boundary | Crates |
| --- | --- |
| Facts and persistence | `runtime`, `event-log`, `presentation`, `config` |
| Execution | `agent`, `model`, `js-runtime`, `tools`, `fs-tools`, `process`, `apply-patch`, `skills` |
| Client and host | `protocol`, `transport`, `client-capability`, `network`, `runtime-host` |
| Executable | `cli` |

The runtime core has no V8 or SQLite dependency. SQLx migrations own persistent
schema changes. Client Capability registration and reverse-call ownership live in
`client-capability`; the host composes them with execution.

## Development

```sh
cargo fmt --all --check
cargo nextest run --locked --workspace -j 4
cargo test --locked --workspace --doc
cargo clippy --locked --workspace --all-targets -- -D warnings
node scripts/asf-license-headers.mjs check
```

Unit tests belong at the end of source modules; integration tests belong in
`tests/`. Prefer structs and enums for domain contracts; reserve JSON values
for genuinely open payloads and schemas.
Shared cross-language fixtures live in root `tests/fixtures` and use
`tests/support/source.mjs` to load current TypeScript sources, never workspace `dist`.
Grep differential tests require `rg` on PATH; the runtime itself does not.
V8-dependent suites share a test binary per crate to avoid repeated
linking. Ordinary tests use local fixtures; real-provider tests require explicit
opt-in and credentials. An isolated worktree can set `MAKA_JS_DEPS` and
`NODE_PATH` to the dependency-bearing checkout and its `node_modules`.

## Current limits

Project/session control and settled Turn navigation, configuration, attachments, file tools, shell/PTY,
Client Capability tools, model streaming and context compaction are implemented.
Codex subscription execution is supported; Copilot/xAI inference adaptation and
live verification are deferred.

WorkHub supports session resolution, queries, model configuration and conversation
with scoped Desktop tools and attachment reads, plus candidate discovery and delegation
to existing idle sessions with atomic attachment transfer. Task creation, linked operations
and multi-selection remain incomplete.

Skill mutations and update previews,
advanced recovery/reconciliation, WorkHub coordination and orchestration,
some capability services, managed upgrades and other protocol domains remain incomplete. Full Desktop
acceptance and release packaging across Linux, macOS and Windows are still
required. Plugin implementation follows Agent Graph; OS sandboxing is deferred. Memory is excluded pending a
separate redesign; its existing implementation is not ported. Content redaction is omitted.
