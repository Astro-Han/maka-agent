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

# maka-tools

[中文](README.zh-CN.md)

Journaled tool dispatch and Code Mode, inspired by [OpenAI Codex](https://github.com/openai/codex).
Catalogs, permissions and execution facts remain authoritative outside JavaScript.

## Code Mode

Configure Code Mode and `apply_patch` in each connection's model parameters.
Auto follows the Host's model defaults; explicit choices apply to new Runs.
Resume and handoff retain the admitted choices. Enabling `apply_patch` replaces
`Edit` and `Write`; disabling it exposes those structured editing tools instead.

`exec({code, yield_time_ms?, max_output_tokens?})` starts a fresh bounded V8 cell.
`wait({cell_id, yield_time_ms?, max_output_tokens?, terminate?})` observes it.
Results distinguish `running`, `completed` and `terminated`; observations contain
only new output. Termination requests cancellation, not instant cleanup.

Helpers: `text`, `image`, `audio`, `generatedImage`, `notify`, `yield_control`,
`store`, `load`, `setTimeout`, `clearTimeout`, `exit`, and `ALL_TOOLS`.
Images accept Host image references, MCP image blocks or base64 data URLs.
Audio is retained in raw evidence; current model adapters receive audio metadata,
not native audio input. No helper grants filesystem, network or client authority.

A Run owns up to four uncollected cells. Tools execute with the cell's captured
catalog, including normal preflight and journaling. Search affects the next
model step, never the current cell. TypeScript descriptions are generated from
the same JSON Schema used for validation; descriptions are not validators.

Each cell has a 64 KiB source limit, 64 MiB V8 heap guard, 30-second synchronous
execution budget, 32 tool-call budget and eight concurrent tool slots.
Excess concurrent calls queue within the call budget. Async Host waits do not
consume the synchronous budget. Output and JSON scratch values are bounded;
V8 heap limits are not process isolation.

Scratch data is Run-local, not durable or an authorization store. A cell reads
a snapshot and publishes its writes after settlement; later completions win
for the same key. Compaction clears scratch data, and old cells cannot restore it.
Run end, handoff and Host restart do not preserve cells or scratch data.

## Ownership

The provider's `exec` is a control operation. Its independently journaled
`CodeCell` owns nested `CodeMode` operations. The control call can settle while
the cell runs; the cell cannot settle before its admitted children.

Call `RunTools::shutdown` before closing the Run or log. It cancels cells,
drains accepted work, and propagates persistence or cleanup uncertainty.
Dropping `RunTools` requests cancellation but cannot await cleanup.
Handoff seals only an idle cell execution boundary. Recovery never replays
an interrupted cell or silently repeats its effects.
