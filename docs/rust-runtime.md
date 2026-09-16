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
TypeScript client protocol and interactions. Both use protocol epoch 154. The rewrite is incomplete;
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
maka host status --root /absolute/path/to/new-root
maka host retire --root /absolute/path/to/new-root
```

Use `target/debug/maka` if the binary is not on PATH. Local transport is a Unix
socket on Linux/macOS or a private named pipe on Windows. An optional
`--websocket 127.0.0.1:0` listener requires authentication; TLS is not implemented.
Against a running Host, `host access prepare --root <directory> --principal <id>`
prints pairing JSON containing a secret. Keep it private. It expires after 15
minutes unless the importing client finalizes it, then reconnects with the same
client identity. This Desktop-owner policy does not grant arbitrary Host paths.
`host access revoke --root <directory> --credential-id <id>` revokes it and closes
its remote connections. These commands neither start the Host nor migrate data.
Development builds retain line-number backtraces; `CARGO_PROFILE_DEV_DEBUG=full`
enables full debugger information.
Windows MSVC builds use the static CRT required by the official V8 archive.
The two vendored Deno TypeScript files must match `deno_telemetry` exactly;
update them together when upgrading that dependency. They avoid a build-time V8.

`host connect --root-id <rootId> --framed` activates that deployment and bridges the
client protocol over stdin/stdout; diagnostics use stderr. Linux/macOS input EOF
half-closes the connection and drains responses. Windows pipe EOF disconnects;
clients must receive their responses before closing stdin. WSL passes
`--repair-root-after-remount`: this explicitly confirms an unchanged Linux inode
after remount, preserving Root ID. Do not use it to adopt copied or legacy roots.

`host install --root <directory>` pins the current executable and on-demand policy;
`--mode supervised` selects persistent serving. Only the returned `executable`
may start that managed root. `host activate --root-id <rootId> --framed` reuses a ready
Host or starts the pinned executable. In supervised mode, activation registers and
starts an account-level systemd service, LaunchAgent or Windows scheduled task.
Linux requires an active user manager with lingering enabled; macOS requires an
Aqua login and Windows an interactive user session. Installation alone does not
start a service or change account policy.
`host setup --root <directory>` combines installation and activation; optional
`--principal <id>` also returns a short-lived Desktop pairing credential. Protect
this JSON as a secret. Interactive launchers use `--framed` and hide the reserved
`__MAKA_NATIVE_HOST_SETUP__` result line. Repeating setup preserves the existing code and
unspecified configuration; changes still require `host update`. Without `--root`, install/setup
use the account's native `runtime-host-rust` directory, separate from legacy TS state.

`host fetch --target <target> --version <exact-version> --cache <directory>` prepares
`@maka-agent/cli-<target>` from npm without installing or starting a Host. Targets:
`darwin-arm64`, `darwin-x64`, `linux-arm64-gnu`, `linux-x64-gnu`, `win32-x64`.
It verifies SHA-512, package identity and binary headers; validated cache hits work
offline and recheck file hashes. CLI proxy environment variables apply. Local
packages use `--archive <file.tgz> --integrity sha512-<base64>` instead of npm.
The returned JSON identifies both Windows executables. `--directory <verified-package>
--receipt-sha256 <digest>` imports a transferred package against its original verifier's receipt.
The default cache is the account's `native-cli` directory; saved profiles reference its executables.
Native preview packages use the separate `rust-preview` npm channel, never `latest`.

Desktop SSH/WSL onboarding downloads and verifies the exact Desktop version locally, transfers
the complete package, removes the upload staging directory, and sets up the native Host.
The target needs no Node/npm/Rust. Native npm releases are not published yet; development builds
can set `MAKA_NATIVE_CLI_VERSION` and `MAKA_NATIVE_CLI_PACKAGES` (a `host fetch` cache directory).
These overrides are ignored in packaged Desktop. Existing profiles start offline.

Desktop reuses managed native Hosts and activates pinned code when needed;
its own generation and exit do not govern their lifetime. Pausing local launches
waits for outstanding activations before handing off the Root.
Desktop can manage local native deployments and existing SSH/WSL native-operator
profiles. Remote lifecycle commands use SSH/WSL OS authority, not WebSocket
credentials. Stop/uninstall retain the connection pause. Start/restart/update
resume normal reconnection even after an unconfirmed result; activation checks
the actual deployment rather than replaying the mutation.
Deployment authority lives in account-level
SQLite, outside the State Root; startup checks it before database migrations.
On Windows, supervised installations use the sibling `maka-service.exe`, a
windowless entry to the same Host. Distribute it alongside `maka.exe`.
On-demand activation requires permission to leave the launcher's Windows Job;
run the built executable directly, not through `cargo run`.
Once shutdown begins, the CLI allows ten seconds for cleanup before exiting with
code 70. Interrupted work is recovered from the log, never assumed rolled back.

For a code update, run the new binary with
`host update --root-id <rootId> --expected-deployment-id <deploymentId> --expected-revision <revision>`.
The same update can set `--mode`, `--websocket`, and repeatable
`--project-root-json '{"label":"Projects","path":"/absolute/path"}'` declarations.
`--no-project-roots` publishes none; `--default-project-roots` restores the account
default. Omitted settings are preserved. Code and configuration share one target
and revision; `reconcile` never selects different settings.
Active clients or non-cooperative work defer the switch; `host reconcile` with the
same identity arguments finishes the recorded update. A committed target is never
automatically rolled back, even if startup fails. Supervised activation replaces
the service definition only while holding the Root. Upgrades target recoverable
restarts, not uninterrupted sockets or PTYs; no separate control daemon is planned.
Unattended release selection is not yet connected.

`host stop`, `host restart` and `host uninstall` take the same identity arguments.
Stop and restart preserve pending updates. Uninstall revokes startup before removing
the service; it retains Root data, packages and a deployment tombstone. Retry the
same uninstall if `cleanup.kind` is `pending`. Explicit installation grants a new
deployment identity after the old service has been removed.

`host status --root-id <rootId>` reads the deployment, pending update, OS service
and live Host independently; it never starts or repairs them. An unavailable Host
is not proof that its process stopped. `host logs --root-id <rootId>` returns up to
48 KiB of supervised diagnostics, with `byteTruncated` marking omitted bytes.
Linux selects the latest 200 journal entries; macOS/Windows read stderr. These are
diagnostics, not execution history. On-demand stderr is not captured.

The `maka` command also provides `host candidate` for Desktop-owned startup,
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
  invocation identities remain distinct. Cooperative continuation preserves the
  public Run identity; claims and cleanup still address exact physical Runs.
  Sealed work can be cancelled without loading a provider or repeating effects.
- Host diagnostics and retirement share active-work accounting, including pending
  OAuth authorization. Retirement targets an exact Host epoch and retains authority
  through response flush and resource cleanup. Cooperative handoff seals settled
  steps and resumes their frozen composition after restart; missing client owners
  leave work paused. Preparation can be withdrawn before sealing and never grants
  permission to interrupt other connected clients, PTYs or OAuth flows.
  The legacy `nodeVersion` field reports `not applicable (Rust)`.
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
  identified transient provider or native network failures, using frozen inputs and cancellable backoff.
  Provider tool activity or replay metadata blocks retries. Unknown/local errors
  and unclassified network failures or idle timeouts are not retried.
  Model activity refreshes the 120-second idle budget; active streams have no fixed
  two-minute duration limit. User cancellation still closes and drains the request.
  A real provider finish releases the stream without waiting for transport EOF;
  synthetic finishes from truncated streams are not successful completions.

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

`node scripts/rust/pack-cli.mjs --target <target> --version <exact-version>
--binary <target-maka> --validator <local-maka> --notices <reviewed-notices>
--output <directory>` packs prebuilt native code and verifies the resulting npm
archive using the local CLI. It never executes foreign-target code, runs install
scripts, publishes, or overwrites an existing output. Notices must cover Rust,
V8 and embedded JavaScript; legacy Node CLI notices alone are insufficient.
Linux release builds still need an established and verified glibc baseline.

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
opt-in. The ignored `original_client_live_provider` test accepts `MAKA_LIVE_PROTOCOL`
(`chat`, `responses`, `messages`), `MAKA_LIVE_BASE_URL`, `MAKA_LIVE_MODEL`, and
`MAKA_LIVE_API_KEY`; its default is the development SGLang endpoint.
An isolated worktree can set `MAKA_JS_DEPS` and
`NODE_PATH` to the dependency-bearing checkout and its `node_modules`.

## Current limits

Project management, basic Session/Turn control, model configuration, attachments, file tools, shell/PTY,
Client Capability tools, model streaming and context compaction are implemented.
Codex subscription execution is supported; Copilot/xAI inference adaptation and
live verification are deferred.

WorkHub supports scoped conversation, candidate discovery, interactive target selection,
delegation to existing or new sessions, steering, stop, resume and correction.
Control actions follow the exact delegated Message, never an unrelated Run.
Correction records intent before retirement, then atomically commits the replacement,
attachments and queued delivery. Recovery can finish after the coordinator Run ends;
unavailable replacements produce durable aborts. Shared Runs are not cancelled.
Canonical records preserve original creation choices and attachment ownership;
candidate queries expose the latest active association, including waiting and blocked work.
Discovery does not authorize delegation; admission rechecks interactions and unsettled effects.
Pending interactions drive the shared Session catalog and its change notifications,
so WorkHub's “Needs you” view agrees with candidate discovery and clears after resolution.

Skill mutations, update previews and their transaction recovery remain incomplete.
Session branching, removal, import/export and recap, along with additional runtime
policy settings, also remain incomplete.
Orchestration, some capability services, managed upgrades and other protocol domains also remain incomplete. Full Desktop
acceptance and release packaging across Linux, macOS and Windows are still
required. Plugin implementation follows Agent Graph; OS sandboxing is deferred. Memory is excluded pending a
separate redesign; its existing implementation is not ported. Content redaction is omitted.
