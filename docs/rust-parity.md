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

# Rust parity and built-in plugins

[简体中文](rust-parity.zh-CN.md)

Baseline: Rust `c6874775d`, main `f02ac9433` (2026-09-18).
This records known functional gaps and the whole-domain built-in plugin migration plan, not a
complete acceptance audit. Target ownership and API extensions below are not claims of implementation.

Use our own business modules to develop the plugin API: migrate an existing domain and complete
its missing behavior together, then remove its special-purpose Host path. Registering a wrapper
around unchanged Host business logic does not complete a migration.

## Boundary

Business policy belongs in plugins; accepting work and preserving its facts belong in Host.
A feature needing durable data is not automatically a core feature: Graph and Scheduler already
own business state and recovery through built-in plugins.

- **Host:** canonical log, execution receipts, Session lineage, permissions, credential authority,
  metering, network policy, process/PTY ownership, shutdown and recovery of accepted operations.
- **Plugin:** workflow decisions, domain data/migrations, external protocol adapters, derived indexes,
  reports and business UI. It cannot rewrite Host facts or manufacture invocation authority.
- **Shared boundary:** plugins use narrow typed Host services. Each datum has one durable owner.
  Built-in domain stores may use SQLx; external plugins do not receive arbitrary Host SQL access.

A built-in Rust plugin is statically linked code activated through the existing Fiber lifecycle,
not a new executable, thread, V8 or necessarily a new crate. Disabling withdraws new capability
admission and preserves business data. Host continues settling accepted work; the domain decides
how to reconcile it on reactivation. Plugin-dependent executors settle truthful interruptions.

Keep existing wire contracts where they serve current clients: a thin Host router can call a typed
plugin contribution, as Scheduler already does. New plugin business interfaces use Remote.
Do not maintain two implementations or grow the kernel's enum with business actions.
Client changes are allowed when they remove an obsolete path, as with Graph.

## Existing domains to migrate

| Domain / current coupling | Target and completion condition |
| --- | --- |
| **Skills:** `maka.skills` owns discovery, input preparation, per-step tool/context snapshots, governance, preference CAS, preview, import and workspace/user publication. Published Client Contributions own Session/new-workspace pickers, management and draft suggestions. | Desktop supplies target-bound Slots, generic Remote transport and authorized native file actions. The old scanner, importer, controller and Skills IPC/preload facade are removed. Host retains thin external protocol adapters, admission and immutable receipts, not Skill resolution. |
| **WorkHub:** `maka.workhub` owns coordinator configuration, answer composition, native `workhub_tasks`, routing/selection/correction/Stop/Resume workflows and recovery policy. Desktop supplies window control and selected workspace/preferences, not task orchestration. Cancellation closes pending selection forms; Host retains canonical outcomes, atomic delivery and accepted settlement. | Move Remote and actual Desktop UI into the built-in domain. Preserve exact-message targeting and atomic admission/cancellation/receipts; never split a correction into independent plugin and Host writes. |
| **Default assistant behavior:** `maka.assistant` publishes the default behavior, persona, personalization and workspace instructions. | Sources are frozen per logical model step; disabling the plugin removes its persona. Explicit Session/child instructions remain independent of replaceable prompt Contributions. Execution/compaction invariants remain Host-owned. |
| **Graph/Swarm and Scheduler:** already built-in plugins; behavior selection uses an open typed identity. Graph uses atomic activate/stop/retire-idle commands and read-only preferences, without `Executions`, configuration writers or Host locks. | Preserve existing orchestration and wakeup behavior. Reuse these narrow commands where semantics match; review Scheduler against the same ownership rule. Typed domain repositories remain legitimate. |
| **Code Mode:** mode selection, nested dispatch and history projection cross several crates. | After the first domain migrations, move its user-facing tool and mode policy where a real contribution boundary helps. Keep V8 ownership, nested-call permissions, dispatch/settlement and canonical history in the runtime. Do not invent a universal executor hook merely to move `exec`. |
| **Files and Shell tools:** registrations are assembled in Host over existing filesystem/process owners. | Tool definitions and assembly can become built-in contributions. Resource ownership, write coordination and PTY cancellation remain Host services. Defer this structural migration until it removes concrete coupling; a plugin per tool adds no value. |
| **Client Capability / MCP, providers and transport** | Keep their current resource/authority boundaries. Desktop-owned MCP does not move into Host, and model vendors do not each require a plugin. Complete their functional gaps independently of structural migrations. |

“Whole domain” means one business implementation and lifecycle, not moving every related type or
table. Canonical Skill receipts and WorkHub execution facts may remain typed runtime contracts.
Reuse SQLx migrations and domain stores; changing ownership does not require changing disk format,
moving all data into KV, or creating a crate per plugin. Runtime contracts must not depend on plugin
implementations; wire adapters may retain existing client vocabulary.

## Missing functionality and placement

“Plugin + Host” identifies separate responsibilities, not permission to defer either half.

| Domain | Remaining functionality | Target owner / necessary boundary |
| --- | --- | --- |
| Plan | Query/control/start, planning artifacts, approval and transition to execution; non-Agent collaboration mode currently fails admission. | **Plugin + Host.** Workflow and records in a plugin; Host owns approval evidence, grants and Turn admission. Extend behavior selection beyond Graph, not the execution engine per workflow. |
| Goal | Query/arm/control, continuation, termination, budget and recovery semantics. | **Plugin + Host.** Goal policy owns subsequent submissions; Host enforces admitted hard limits and records usage. Retiring the plugin must close future submission admission. |
| Session todos | `todo_read`/`todo_write` and `session.todo.query`. | **Plugin.** One typed todo domain backs tools and UI, with durable revisions; not a second execution queue. |
| Deep research | Research workflow, query/progress, results and recovery. | **Plugin.** Reuse Graph/Swarm, Web, bounded model calls and reliable submission. Do not build another generic orchestration engine. |
| Daily review / recap | Daily-review query/mutate, scheduled review and `session.recap.generate`. | **Plugin + Host.** Summarization/selection/output in plugins; reuse Scheduler and authorized history/model services. Host retains any canonical Session metadata commit. |
| Web | Built-in WebSearch/WebFetch, `web-search.execute`, provider selection/settings, extraction and explicit truncation. | **Plugin + Host.** Providers and extraction in a Web plugin. Host retains network/proxy/credential/permission boundaries. Invocation HTTP exists; user/background entrypoints still need explicit authority, not a fabricated Tool call. |
| Recall | Ranked cross-Session passages and RecallMore expansion. | **Plugin + Host.** Ranking and rebuildable indexes belong to Recall; Host supplies scoped history/citations and checks current access/deletion. Existing per-operation event reads are not a cross-Session history API. Recall is not Memory. |
| External agents | Setup start/query/cancel; execution adapters, configuration, auth, conversation identity, adapter-specific attachments/interactions/resume/fork; Command Code GO execution. | **Plugin + Host.** Implement concrete CLI/ACP adapters as Executor plugins over owned processes/HTTP. Generic Executor support is not a shipped adapter. Host owns authorization, cancellation and canonical external-event recording. |
| Usage / Pricing | Usage queries, revision-consistent screens/activity pages, pricing query/mutate and valuation. | **Plugin + Host.** Reports, price policy and rebuildable projections may be an Insights domain; Host records usage independently of plugin availability and provides consistent reads. Missing usage must not become zero. |
| Background health | BackgroundTaskHealth process and endpoint checks. | **Plugin + Host.** Health interpretation and Tool in a plugin; Host exposes authorized resource observations and bounded probes. A stored PID is not resource ownership. |
| Session lineage | Branch/revision create/abandon, regeneration. Ordinary resume/startup recovery exist. | **Host.** One transactional lineage and workspace authority; plugins may request changes through commands, not reproduce them in private storage. |
| Session lifecycle | Removal/preview and shared Session queries. | **Host.** Coordinate references, running work, attachments, owned worktrees, grants and cleanup. Shared queries depend on real collaboration authorization. |
| Session transfer | Bundle export/import and unified external catalog/source/import for Codex, Claude Code and OpenCode. | **Plugin + Host.** Source parsing/discovery can be adapters; Host owns bounded canonical import, identities, artifacts, provenance and atomic publication. Native bundle format remains a Host contract. Never import source event bytes as trusted execution authority. |
| Runtime policy | Shell/web-search/external-agent consumers and ordinary named tool profiles. | **Split.** Shell launch policy and capability ceilings stay in Host. Web/external-agent settings belong to their domains. Profiles contribute definitions; Host applies the intersection at admission and per-step capture. Storing settings alone is insufficient. |
| Access / collaboration | Credential rotation prepare/revoke; principal revoke; collaboration access, invitation, grant revoke, principal rename/revoke; Turn-request create/query/decide/acknowledge/withdraw. | **Host.** Reuse credential and durable admission authority. Plugins may provide workflows/UI but cannot decide grants, bypass revocation or own canonical accepted Turn requests. |
| Peer Mesh | Create/query/invite/join/leave/remove/close/reconcile, rename/display-name and transit control. | **Host for this rewrite.** Identity, routing and transport recovery must work during plugin recovery/unavailability. Do not add a transport-plugin platform to complete these protocols. |
| Credential export | `configuration.credentials.export`. | **Host.** Explicitly authorized export from the real vault; plugin-scoped credentials are not blanket vault access. |
| Model providers | Google/Cohere; remaining declared auth, reasoning/usage/options behavior; runtime models.dev refresh; Copilot/xAI inference and credential-backed verification. | **Model/Host layer initially.** Keep shared transport, streaming, retries, metering and request snapshots. Metadata-source policies can later be contributions; no one-plugin-per-vendor mandate. Command Code CLI execution belongs with Executors above. |
| Diagnostics / hosted runs | `execution.inspect.query`, `host.resources.query`, `hosted.execution.start/cancel`. | **Host**, with optional plugin presentation/orchestration. Inspection reads canonical evidence; hosted execution must preserve environment, ownership and cancellation. It is not merely another Executor name. |

## Consumer-driven API work

The kernel exists, but the current SDK is not a universal implementation surface.
Original TS plugins are not source-compatible with the new SDK.

| Consumer / needed capability | Current evidence / smallest useful extension |
| --- | --- |
| Input preparation | Native typed Contributions and JS `ctx.input.prepare` share ordered preparation, stamped receipts and retirement checks. Native revisions provide nonblocking admission/invalidation ordering. Queue edits and steering prepare before admission; accepted promotion/replay never rescan sources. |
| Skills: reserved tool ownership | Skill/SkillSearch are ordinary Contributions reserved for their designated package. Per-step bindings capture handlers and supporting context together, after tool ceilings; physical retries retain the same snapshot. |
| WorkHub: precise execution commands | Existing correction atomically changes intent, pending delivery and canonical link facts. Extract typed commands with stable operation IDs, exact targets and expected revisions. Preserve their transaction and recovery boundary; never expose a SQL transaction callback or downgrade to unrelated writes to fit the SDK. Business policy belongs outside that boundary. |
| WorkHub / Graph / Plan: selectable behavior | Open `BehaviorId` selects a typed Contribution; Graph/Swarm register independently. Host acceptance covers a non-builtin business. Preserve Session defaults and durable per-Turn choices; a requested unavailable behavior fails explicitly. Behavior preparation and input preparation are separate contracts, not one hook bus. |
| Skills / Web / Recall / Insights: authorized services | Execution SDK reads are scoped to its submitted operation. Add bounded history/usage/resource queries and user/background resource capabilities when their first consumer needs them. Invocation, Remote and background callers retain distinct authority; broad `Executions`/Host handles are not a substitute. Domain catalog/mutation APIs can be typed plugin Services rather than new kernel methods. |
| Skills / WorkHub / default behavior: business UI and prompt context | SDK has prompt Contributions, Session/workspace Slots, draft suggestions, optional local files and Remote methods/streams. Migrate the actual picker, management and WorkHub consumers; add only required Slots/routes and scoped prompt-source reads. A Remote endpoint alone does not supply UI integration. Inactive features remain visibly unavailable without blocking ordinary chat. |
| Other TS extension services | New SDK lacks equivalent public registrations/services for LSP routing, commands, Skills/Goals queries, shell environment contributions, Settings definitions, authorization flows and LLM adapter registration; direct ask/approval and attachment-service convenience APIs also need consumer-driven adaptation. Implement domain registries as plugin services where possible, retaining Host authority for sensitive actions. `llm.generate` is not adapter registration. |

These are functional extension points to assess and implement, not a promise to copy every TS method.
TS LLM adapter registration serves plugin model calls; it does not itself register a main Session transport.
Do not prebuild a generic provider framework, event bus or universal repository for them.

Built-in Rust uses typed calls directly, not a JSON/V8 round trip. Rust and JS adapters share
capability semantics, authorization and retirement guarantees; public JS bindings are added for
new cross-language capability contracts with a real consumer. A Rust-only domain repository need
not become a JS API. Do not postpone necessary API work by granting a built-in unrestricted Host access.

## Delivery order

1. **Skills:** migrate existing discovery/invocation and finish governance/publication in one domain.
   Exercise preparation, reserved registration, domain Services, prompt and real Desktop consumers.
2. **WorkHub:** migrate the complete implemented domain, including correction, attachments, stop,
   resume and recovery. Develop precise Host commands without weakening existing atomicity.
3. **Default behavior and existing plugin convergence:** move persona/workspace policy; use Graph
   and Scheduler to validate the resulting APIs and remove unnecessary private Host shortcuts.
4. **Missing business domains:** complete Web, Recall and todo, then Plan/Goal and research/review;
   finish external adapters and Insights/health. Reuse the domain boundaries rather than first
   implementing new business logic in Host and moving it later.
5. **Remaining core parity:** complete Session lifecycle/lineage/transfer, policy, access/collaboration,
   Peer Mesh, providers and diagnostics. Pull required core commands into their consumer's earlier
   stage; core work is not blocked on all plugins or a marketplace. Assess Code Mode/tool assembly
   migration after the first domains establish a useful boundary, not as a prerequisite to parity.

Within each domain: identify a real consumer and invariant → implement the smallest typed API and
consumer together → verify lifecycle and failure behavior → remove the old Host business path.
Use a second existing consumer where it actually shares the contract; do not invent a mock business
to justify an abstraction. Update the SDK contract in the same slice when it changes. If a proposed
API needs arbitrary Host access, a second authority or many business exceptions, revise the boundary.

Every domain must cover its mutation/query surface, error distinctions, authorization, cancellation,
lost replies, restart recovery and actual Desktop consumers before being marked complete.
Use a few end-to-end acceptance cases including retirement and reactivation; a registered Tool or
passing schema test is not acceptance.

The first migrations must also prove:

- Skills-disabled ordinary chat works; a new explicit Skill request fails clearly. Accepted content
  and receipts survive edits, updates, retirement and restart without rescanning Skills; pending
  promotion still enforces current permissions. Preparation racing retirement cannot admit stale work.
- Skill publication detects local edits, handles commit uncertainty and recovers complete bytes,
  lock and baseline together. Discovery, tools and UI observe the same domain revisions.
- WorkHub lost replies and restart during correction neither duplicate delivery nor target newer
  unrelated work. Disabling stops new orchestration while Host settles accepted operations; re-enable
  reconciles their receipts before continuing.
- Real Desktop consumers use the published plugin route with stale-call rejection. Source review
  confirms Host no longer scans Skills, chooses WorkHub policy or supplies a duplicate default persona.

Move or extend existing high-value tests instead of keeping duplicate old/new suites. Retain the
existing cross-platform permission, filesystem and PTY coverage; migration is not a reason to weaken it.

## Already present and excluded

WorkHub; ordinary resume; core files/shell/PTY; Client Capability including Desktop-owned MCP;
model streaming, compaction and dynamic tool loading; the plugin kernel, Rust/JS loading,
shared/dedicated plugin V8s, scoped storage/credentials/files/HTTP/process/PTY/model/client calls,
Executors and Client Remote; Graph/Swarm and Scheduler are implemented.
Old `agent.graph.*` RPCs were replaced by the plugin route, not left as a second Graph implementation.

Memory and redaction are excluded; OS sandboxing is deferred. Copilot/xAI live adaptation is deferred
until suitable credentials. Packaging/deployment is not full product parity: native CLI operators do
not replace the TS interactive CLI/ACP surface or migrate legacy TS state roots automatically.
Those product/data migration decisions must not be hidden inside a plugin.
The TS client/TUI need not be rewritten in Rust; its native-Host compatibility still needs acceptance.
Legacy state-root migration is a separate scope decision, not implicitly authorized by parity work.

## Evidence

- [Host registry](../crates/runtime-host/src/server/operations.rs), [dispatch](../crates/runtime-host/src/server/dispatch.rs), [operation vocabulary](../crates/protocol/src/operation.rs).
- [Execution preparation](../crates/runtime-host/src/execution/prepare/environment.rs), [tool assembly](../crates/runtime-host/src/execution/tools.rs), [policy consumers](../crates/runtime-host/src/server/configuration/policy.rs), [provider routing](../crates/runtime-host/src/provider_route.rs).
- [Skills domain](../crates/skills/src/lib.rs), [input preparation](../crates/runtime-host/src/execution/input/prepared.rs), [WorkHub workflow](../crates/runtime-host/src/plugins/workhub/correction.rs), [Host commands](../crates/runtime-host/src/execution/workhub/commands.rs), [correction transaction](../crates/event-log/src/workhub/correction.rs), [default prompt](../crates/runtime-host/src/plugins/assistant/prompt.rs), [Graph wiring](../crates/runtime-host/src/plugins/graph.rs).
- [SDK contracts](../packages/plugin-sdk/README.md), [execution services](../crates/plugins/src/execution.rs), [Session behavior](../crates/plugins/src/session.rs), [Scheduler router](../crates/runtime-host/src/server/scheduler.rs).
- TS [composition](../packages/runtime-host/src/server/execution-composition.ts), [interactive tools](../packages/runtime-host/src/server/interactive-run-composer.ts), [inspection](../packages/runtime-host/src/server/execution-inspect-coordinator.ts), [external imports](architecture/external-session-import-design.md).
