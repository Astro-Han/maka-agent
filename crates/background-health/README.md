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

# Background task health

`maka.background-health` is a statically linked Profile plugin exposing the
`BackgroundTaskHealth` Agent tool. Desktop uses its existing tool result UI.

Input: `ref` from Shell, optional `include_logs` (false by default), optional
HTTP(S) `url`. The plugin reads the canonical Shell snapshot through Host's
Agent-authorized `Read(ref)` service. It cannot inspect another Session's task,
reattach a PID, or infer liveness after Host recovery marks a task orphaned.
Read must be available within the Agent's tool ceiling. Timestamps describe the
persisted snapshot; `updatedAt` is not a heartbeat or last-output guarantee.

Process state and endpoint health are separate observations. HEAD falls back to
GET only for 405/501. Responses are discarded and settled; redirects are never
followed. 2xx means healthy, 3xx unknown, and 4xx/5xx unhealthy. Connection failures,
denied access and timeouts mean unknown. Endpoint responses do not prove that the
tracked process owns the listener or that a browser application is ready.

Network permission interaction and transport timeouts are owned by Host. The
plugin does not time out a user awaiting permission approval. `elapsedMs` includes
that approval time. Cancellation drains the child scope; unconfirmed cleanup is
an error. Incognito permits local task observation but skips endpoint probing.
Logs are opt-in and retain Host's bounded/redacted output representation.

This plugin has no Remote endpoint or private process registry. Tests cover the
real Agent → Files → Shell path and cross-Session denial, plus HTTP fallback,
body disposal, cancellation, privacy, and delayed HTTP service behavior (permission waiting is verified by code-path audit).
