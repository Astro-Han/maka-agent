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

# Goal

`maka.goal` is a statically linked Profile plugin with a separate Desktop Client
in Session Inspector. Its business state uses public plugin Store/CAS; Host owns
Session identity, authorization, execution admission, receipts and accounting.

## Workflow

1. Enter an objective, 1–100 iterations and an optional observed token threshold.
2. Approve Host background `executions` and `read_usage` access to this Session.
3. Save for later (`arm`) or start immediately. Saved goals start only on explicit
   resume; this version does not adopt an unrelated user Turn automatically.
4. Each iteration uses a stable operation ID and an immutable, durably saved
   Submit request. Restart queries/retries that original operation. Unknown
   acceptance never creates a replacement identity.
5. `GoalStatus` lets the exact owned Invocation report progress, achieved with
   evidence, waiting, or impossible. This is a model assertion, not an independent
   evaluator. It applies only after Host reports normal completion. Without a
   terminal report, the plugin continues up to the configured iteration limit.

Pause stops further dispatch decisions; already dispatching/accepted work may
finish. The durable dispatch decision and Host admission are separate operations,
so an in-flight submission can become visible after a pause reply. Cancel saves
intent and cancels only the original operation, including Host-owned successors;
it never targets the Session's latest arbitrary Turn. Undispatched intents can
be cleared. If acceptance remains unknown, `cancellation_unknown` retains the pending ID
and only checks its receipt about every 30 seconds while Host is running; it does
not keep Host awake or redispatch a cancelled request. The user can explicitly
recheck. The UI continues to show the pending iteration until settlement is
confirmed, and cannot replace it with another Goal while acceptance is unknown.

Failed/cancelled runs require explicit resume. Waiting-for-user never triggers
another execution. A sealed Host handoff pause stops the Goal and releases its
keepalive; the user must resume that execution through Session controls before
resuming the Goal. Its last operation ID is retained for inspection. Cancelling
a Goal does not revoke the user's ability to explicitly resume a sealed Host Run
through Session controls; that manual continuation is outside this Goal's owner.
This plugin does not implement automatic handoff resume. Plugin retirement stops new plugin
work; Host still owns accepted executions. Re-enabling reconciles those receipts.
Revoked authority stops scheduling and releases background keepalive; renewed
Session consent is checked before resume or cancellation recovery. Incognito
stops continuation. Controls carry Goal ID and storage revision, rejecting stale
UI writes and changes to confirmed terminal outcomes. Only cancellation recovery
can transition between cancelled and cancellation-unknown. Creating a new Goal requires the previous one to be terminal and have
no unsettled operation. Reusing an older arm ID cannot resurrect a replaced Goal.

## Budget semantics

`maxIterations` counts reserved Goal iterations, at most 100. It is a plugin
scheduling policy. Host separately enforces its existing per-Run step limit.
There is no aggregate Host step ledger in this plugin.

The optional `tokenBudget` is a **retrospective Session scheduling threshold**:
reported input + output tokens since an idle baseline captured at creation,
including other activity in that Session. It is not precise Goal attribution or
a hard provider-request cap; a running request can overshoot. Missing usage or a
regressing counter stops further scheduling as `budget_unknown`. A completed
Goal may report success even if its last iteration crossed the threshold.

## Interfaces and verification

- Paired Remote `request` and standalone `manage`, always bound to a Session:
  `read`, `arm { arm }`, `control { id, revision, action, grant? }`.
- Controls: `pause`, `resume`, `cancel`, `complete` (only without pending work).
- Agent tool: `GoalStatus { status, note }`; no arbitrary Session ID or background
  authority can be supplied through tool input.
- No legacy `goal.*` protocol routes, TS evaluator or Jev coupling.

Tests cover atomic arm retry, lost storage replies, immutable outbox recovery,
stale CAS, terminal monotonicity, limits/missing usage, settlement despite usage
failure, sealed handoff handling, plus real Host consent, continuation, pause,
report-before-settlement, budget stop, revocation/renewal, cancellation,
retirement, restart with an accepted execution, and an unknown cancelled dispatch that does
not pin idle Host. The actual Client is also
checked in a browser preview with mock Remote responses; this is not a full
Electron integration test.
