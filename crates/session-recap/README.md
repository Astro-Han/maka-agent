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

# Session recap

[简体中文](./README.zh-CN.md)

`maka.session-recap` is a statically linked Profile plugin with a separate
Desktop Client. The Client adds a manual recap to Session Inspector.

The Session-bound Remote endpoints `request` (Client) and `manage` (standalone)
accept `{ "kind": "read" }` and
`{ "kind": "generate", "operationId": "<UUID>" }`. The Session comes from the
Host's binding, not a caller-supplied payload field. Reads require ReadHistory;
generation additionally requires Models for that Session and uses its selected
model. Incognito mode refuses both. Cached results recheck history access.

Before invoking the model, the plugin atomically commits an operation intent and
a latest-operation pointer in its own scoped storage. A completion updates only
its operation receipt. Concurrent duplicates and retries with the same ID never
invoke the model twice. An older completion cannot replace a newer request.
After a lost reply or restart, read discovers the latest receipt. Pending means
that the result is not confirmed; reusing the ID only reads that receipt.
A new ID starts a new model request and may incur another charge.

The model call uses public Host history and model services. Host retains model
selection, authorization, transport and usage accounting. The plugin owns the
summary as derived data. It does not modify canonical conversation history or
Session metadata. Retirement cancels/drains scoped work; stored results survive.

The input uses a fixed history watermark and retains up to the latest 32 KiB of
text. Reads are bounded to 256 pages/preparation steps. Preparation delays and
history exceeding that scan have separate errors and do not dispatch a model.
The public text-history API lacks structured tool success/failure projections;
the recap is a best-effort textual summary, not evidence of task completion.
Generation has a 30-second timeout and a 1024-token output cap; incomplete or
empty output is not published as a successful recap.

This plugin provides manual generation and durable inspection. Automatic idle
recaps, Daily review, and the old TS `session.recap.generate` protocol operation
are not implemented by this plugin. No broad background model grant is created.

## Native terminal client

In a Session, press Ctrl+P and choose **Session recap**. Opening the dialog only
reads the saved result. **Generate new** explicitly uses the Session's selected
model and may incur charges. Esc or an outside click closes the dialog without
editing or sending the composer draft. Arrow keys scroll longer results.

The TUI writes the original operation identity to its Root-bound checkpoint
before generation. After disconnect or restart, **Resolve pending recap** reads
Host state; **Retry original** explicitly reuses that identity. It does not
silently start a replacement request. A recorded Pending receipt remains an
unconfirmed result; generating a new recap is a separate explicit request.
The dialog calls the existing standalone `manage` Remote on the current Client
connection and closes its document after each request. It implements no summary,
model, credential, or permission policy.

## Verification

- `cargo test -p maka-session-recap --lib`: persistence/retry, restart, concurrent
  generation order, unknown results, cached access refusal, privacy and bounded history.
- `cargo test -p maka-runtime-host --test integration session_recap_plugin`:
  actual Session history and selected-model generation through the registered
  Remote/Client binding, duplicate prevention, retirement and Host restart.
