# Changelog

## Unreleased

## 0.1.5 - 2026-08-05

### Highlights

- Added provider-native web search: one `WebSearch` routing layer that selects
  the current model provider or explicit Tavily execution, with OpenAI and
  Anthropic implementations, durable replay, citation metadata, and privacy-mode
  coverage across Desktop, CLI, Runtime Host, and opt-in Headless Harbor.
- Extended Runtime Host ownership to Plan turn transitions, Plan and Daily
  Review runtime state, Deep Research state, and derived Session effects, and
  added its Desktop client foundation: session adapter, catalog, execution, and
  domain IPC, plus effective pricing projection.
- Bundled an English Computer Use Skill tailored to the shipping semantic action
  surface, auto-installed only when a Computer Use backend is available without
  overwriting modified or untrusted copies.
- Made app updates background and task-aware: discovery and download now live in
  Electron main, and restart/install is guarded by a main-owned activity
  snapshot that confirms how many tasks it interrupts.
- Restored the native application menu and unified platform command routing.
- Gated managed-workspace execution behind an owner-bound, short-lived authority
  scope and sealed baseline admission with a durable Git receipt, so a
  capability is usable only while the storage root, workspace binding, and
  canonical head describe the same Git state.
- Continued the Astryx redesign of Settings: every capability gets its own
  disclosure, a settled value stays a row until you ask to edit it, and the
  specs and jargon nobody can act on are gone; project files left Settings, the
  junk drawer emptied into the workbar toggle, and project and branch joined the
  composer footer control row.
- Let the Inspector trace filter by what a reader actually asks, and closed the
  follow-ups left on the trace panel.

### Performance

- Lazy-mounted sidebar session trees.
- Removed the swarm ledger full-scan hot path.

### Reliability and developer experience

- Fixed copied tool authority facts on branched tools, preserved stream liveness
  across filtered parts, returned proxied responses at headers instead of body
  EOF, landed the terminal fact a stop claims, and made the execution boundary
  the sole file-path authority in one realpath space.
- Made realtime session creation single-flight, released settled bot typing
  listeners, honored the default permission mode immediately, reset incompatible
  operational state, and resolved permission overlay assets in bundled dev.
- Bounded Headless teardown and marked the output replay, reaped the scoped
  command's replay, kept Harbor archive ids within ref grammar, stopped handing
  graded competitor arms this repo's tree, made Maka's settlement window
  reachable at the Harbor boundary, honored the command-timeout floor, resolved
  `host.docker.internal` for in-container arms, unified `canonicalJson` into a
  single serializer, added the Reasonix benchmark arm, and raised the A/B
  pair-concurrency cap to 16.
- Rehydrated an unanswered user question the surface never received, and made
  thinking strength follow the chosen level on Kimi/StepFun/MiniMax coding
  plans.
- Added an a11y audit that flags `aria-label` on elements whose role cannot hold
  a name, and established the Windows support baseline.
- Replaced fixed waits with explicit barriers in runtime tests, owned the temp
  namespace in the test runners, seeded a real project in the sidebar rename
  spec, eliminated Desktop smoke false failures, and fixed the serial workspace
  batch that never ran after a parallel failure.

### Removed

- Removed dead storage modules, consumer-less Desktop IPC bridge surface, dead
  shell CSS recipes, and the reusable raw worktree path from runtime consumers.

### Distribution

- Ships for Apple Silicon macOS as a signed and notarized DMG and ZIP.
- The bundled Computer Use skill ships with the app, but the Computer Use
  executor remains excluded from this release.

## 0.1.4 - 2026-08-04

### Highlights

- Rebuilt Computer Use around `maka-cu` as the executor that actually runs:
  rewrote what the model reads (observation, action surface, refusals), made the
  agent cursor land where it is aimed, mirrored the driven window instead of
  competing for the screen, and let a model read back the call it made.
- Gave driving the computer its own conversation row, stopped a run when the
  machine locks, and kept the machine awake while it runs.
- Made SQLite the sole operational authority in storage, moved the project
  catalog into the operational database, and converged concurrent fresh WAL
  initialization.
- Extended Runtime Host ownership over input and artifact preparation,
  interactive session actions, safe-boundary continuation authority, and
  sandbox boundary interactions.
- Added a session trace as a workbar Inspector tab, projected over both ledgers.
- Added configurable subagent model routing with its own settings page, and
  raised agent swarm concurrency to 32 ways.
- Added Mermaid rendering to Markdown.
- Switched Codex OAuth to the ChatGPT device-code flow and added Grok PKCE.

### Performance

- Streamed the transcript incrementally instead of re-deriving it per token.
- Subscribed to session UI state at the granularity each surface reads, and
  stopped idle-session event-health polling from re-rendering the whole shell.

### Reliability and developer experience

- Fixed turn execution scope isolation, streamed tool-call delta resolution
  against live aliases, streamed tool calls dropped when a gateway skips index
  0, tool-free child summaries, and AgentRun ledgers written by other versions.
- Made macOS development permission grants reliable and the dev TCC grant
  durable behind an opt-in, and made startup say which step has not come back.
- Told a refused tool call what to do next, not just what went wrong.
- Derived thinking-strength controls from models.dev reasoning options, and
  unified cron expression authority.
- Continued the Astryx redesign across Settings, the subagent page, the
  extensions page, plan reminders, skill chips and empty states, and the
  composer model and voice controls.
- Put every headless benchmark arm under the same tool surface and model
  budget, registered `ArchiveRead` for Harbor-archived tool results, and
  required validated verifier grades.
- Routed CI into dedicated lanes for heavy suites and the alignment audit, and
  measured Computer Use rendering changes against recorded trajectories.

### Removed

- Removed `cua-driver` now that `maka-cu` is the Computer Use executor.
- Removed isolated runtime utilities and the Cursor subscription integration.

### Distribution

- Ships for Apple Silicon macOS as a signed and notarized DMG and ZIP.
- Computer Use remains excluded from this release.

## 0.1.3 - 2026-08-03

### Highlights

- Expanded Runtime Host ownership across hosted child agents, Agent Graph
  execution, Web Research, Goals, Automation, OAuth enrollment, session
  lifecycle, runtime resources, and live execution inspection.
- Strengthened Graph and Swarm coordination with structured operator handoffs,
  non-blocking supervisor turns, durable child summaries, and session-bound
  graph retirement.
- Migrated the Desktop shell, Settings, Composer, conversation surfaces,
  typography, navigation, resizing, and common controls to Astryx 0.2.0.
- Added SQLite-backed local memory, managed Git workspaces, verified operational
  backups, canonical model-call metering, and confirmed agent
  self-configuration tools.
- Added `opencode-free` as a zero-credential default provider and improved
  Runtime Host OAuth-backed execution.

### Reliability and developer experience

- Fixed Electron development and packaged startup failures, onboarding snapshot
  handoff, Runtime lifecycle races, PTY backpressure, clipboard writes, and
  several Graph, Headless, and UI regressions.
- Reduced low-signal, duplicate, source-shape, and happy-path tests; routed CI
  suites by affected surface; and kept build, typecheck, Storybook, and E2E
  gates intact.

### Removed

- Removed the desktop Open Gateway HTTP/SSE server and its Settings and IPC
  surfaces. Model providers, OpenAI-compatible connections, Vercel AI Gateway,
  and bot event gateways are unchanged.
- Removed the superseded Expert Team mode and its Notes-era architecture
  artifacts now that Graph and Swarm own multi-agent coordination.
- Retired legacy storage writer exports and Astryx compatibility adapters that
  no longer had active callers.

### Distribution

- Ships for Apple Silicon macOS as a signed and notarized DMG and ZIP.
- Computer Use remains excluded from this release.

## 0.1.2 - 2026-07-31

### Runtime kernel extraction

This change set turns the runtime execution path from a large implicit
`SessionManager` / `AiSdkBackend` flow into an internal runtime-kernel shape.
It keeps the existing desktop, renderer, IPC, session JSONL, settings, and bot
surfaces stable while moving model, tool, trace, run-ledger, and
startup-recovery responsibilities behind explicit internal boundaries.

| Area | Summary |
| --- | --- |
| Tool runtime | Extracted an internal `ToolRuntime` around tool input validation, permission checks, watchdog pause/resume, abort propagation, telemetry, artifact recording, and failure classification. |
| Model adapter | Extracted a minimal `ModelAdapter` so provider stream/error/usage normalization no longer lives directly in the backend orchestration shell. |
| Runtime trace | Added best-effort `RunTrace` events for model, tool, permission, abort, and usage milestones without changing renderer-visible `SessionEvent` behavior. |
| AgentRun ledger | Added core `AgentRun` types and a file-backed `AgentRunStore` at `sessions/<sessionId>/runs/<runId>/run.json` plus `events.jsonl`. |
| AgentRun execution | Moved the heavy turn execution lifecycle from `SessionManager.sendMessage()` into internal `AgentRun.execute()`, including user-message append, backend stream drive, status projection, abort/failure handling, and durable trace writes. |
| Startup recovery | Made `recoverInterruptedSessions()` prefer the AgentRun ledger when available, repairing stale non-terminal runs and preserving the legacy message/turn-state fallback for older sessions. |

See `docs/archive/runtime-kernel.md` for the historical design rationale, boundaries, and
verification details.

### Hardening phases 1-5

This change set collects the first five maintenance hardening phases from the
Rive deep-read follow-up work.

| Phase | Area | Summary |
| --- | --- | --- |
| 1 | Runtime permission and usage handling | Made stream watchdog pause/resume accounting robust for concurrent tool calls, added permission timeout handling, integrated Office document abort propagation, and fixed cache/reasoning token usage extraction. |
| 2 | Session JSONL recovery | Recovered sessions with corrupt JSONL rows by parsing message lines independently, surfacing landed corrupt rows as `system_note`, and dropping malformed truncated tail rows. |
| 3 | Bot and OpenGateway abuse controls | Added bot inbound rate and session-binding limits, bounded bot dedupe state, forced bot-bound sessions to `explore`, and capped OpenGateway SSE connections with idle cleanup. |
| 4 | Credential-store secret kind expansion | Extended encrypted credential-store support for bot tokens, bot app secrets, proxy passwords, gateway tokens, and Tavily API keys while preserving legacy API-key/OAuth-token key formats. |
| 5 | Connection credential IPC input hardening | Added shared main-process validation for renderer-controlled connection slugs and API keys before store, credential, or provider side effects. |

### Active tool-result pruning default-on

`activeToolResultPrune` (current-turn large tool-result pruning) is now
enabled by default on both desktop and headless. Tool results above the
2048 estimated-token threshold are archived and replaced with a
metadata-only placeholder (artifact id + content hash) in the next
provider-visible request; the raw payload is preserved in the archive
and is not lost. On desktop this runs before the already-default-on
`semanticCompact` summary step. On headless the placeholder reaches the
next provider step directly (no default compaction/retrieval), which
benchmark A/B evidence in #340 showed is non-inferior within the 10pp
margin while saving ~31.6% cost.

Opt out with `MAKA_CONTEXT_ACTIVE_TOOL_RESULT_PRUNE=off` (both sides).
Tune the threshold with `MAKA_CONTEXT_ACTIVE_TOOL_RESULT_MAX_ESTIMATED_TOKENS`
and the start step with `MAKA_CONTEXT_ACTIVE_TOOL_RESULT_MIN_STEP_NUMBER`.

### Verification

- Headless and desktop context-budget tests for activeToolResultPrune
  default-on, opt-out, and env knobs.
- Runtime package typecheck/build and full runtime test suite.
- Desktop main build/typecheck.
- Storage package build and AgentRun store tests.
- Runtime package typecheck/build and focused runtime tests.
- Storage package build and focused session-store tests.
- Desktop main build/typecheck and focused bot/OpenGateway, credential-store,
  settings/web-search, connection IPC, OAuth, and model-provider regression
  suites.
- `git diff --check` before each pushed phase.
