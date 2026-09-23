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

# Session import

[简体中文](README.zh-CN.md)

Converts Codex rollouts and Claude Code transcripts into historical records through public plugin read capabilities. The caller supplies a pinned file and the selected source identity; this crate cannot reopen paths, dispatch tools or write canonical events.

- Codex uses conversation events and tool response items, applies recorded rollbacks, and excludes provider message mirrors.
- Claude selects rewritten prompts without dropping parallel results or compaction roots. Three digest-checked passes resolve lineage, assemble response fragments and emit history.
- Tool calls and results retain their source order and correlation. Missing outcomes stay missing.
- Filesystem catalogs read bounded summaries (Codex: 512 KiB; Claude: 256 KiB head and tail), apply workspace/archive filters, and paginate by modification time and relative path. Query-bound cursors resume after the last delivered entry, including when the 48 KiB wire budget cuts a page short.
- Source workspace/model values are observations, not execution configuration. No credentials or provider options enter the result.
- Corrupt interior JSON, inconsistent source identity and changing multi-pass input fail the import. Only an unfinished final JSON write is omitted and identified in the fingerprint.

Reads are bounded: 2 GiB per source prefix, 64 MiB per JSONL line, one million source lines and 65,536 structural JSON tokens per line. Retained payloads are checked before decoding; encoded history is limited to the Runtime's 6 MiB / 7,500-record budget. Claude additionally bounds its lineage index and response fragments. Oversized input is rejected, not truncated.

The returned transcript is not a published Session. Publication and retry receipts belong to the public Session import capability.
