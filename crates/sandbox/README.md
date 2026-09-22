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

# maka-sandbox

[中文](README.zh-CN.md)

Sandbox policy and platform launch preparation, inspired by [OpenAI Codex](https://github.com/openai/codex).

The Host owns authorization, approvals and execution records. This crate evaluates
filesystem and network restrictions and prepares authorized commands for OS
isolation. Process I/O, PTY ownership and cancellation remain with `maka-process`.
Unsupported isolation must fail explicitly, never fall back to unrestricted execution.
Capturing a command does not allocate native sandbox resources; the admitted
process worker owns preparation, startup and cleanup.

Network grants name exact hosts and ports, not upstream proxies. Destination-restricted
HTTP and CONNECT traffic passes through an execution-owned gateway; redirects need their
own destination grant. macOS restricts connections to that gateway, Linux places
its listener in a private network namespace, and Windows binds each account to
its reserved gateway port. Configured upstream routing and authentication stay
in the Host. Closing the execution closes its tunnels; changing permissions does
not widen an already running process.

Inherited loader hooks are removed. On Unix, managed process environment overrides
cannot configure the isolation wrapper's loader. Set necessary loader variables
inside the sandboxed command instead, for example with `env NAME=value program`.

Linux requires bubblewrap 0.11 or newer at `/usr/bin/bwrap`; it is not vendored.
Missing protected leaves use temporary mount targets with kernel lifetime leases.
Missing ancestors share the leaf's lease and are collected child-first; user
content is preserved. Cleanup checks inode identity and an ownership marker;
it never recursively deletes workspace paths. Staging uses `/tmp` or the user's home cache on the workspace's
filesystem, which must support user xattrs. Protected missing leaves on other
filesystems, missing writable roots and exact-directory rules
currently fail explicitly on Linux.

Windows managed commands use provisioned low-privilege accounts, account-scoped
WFP rules, cached permission surfaces and per-execution Jobs. Identical surfaces
reuse write capabilities; changing writable object identities retires them.
ACL changes require an idle slot and drained native Jobs. The cache grants no
authority: each launch still checks current authorization. Foreground commands, pipes
and ConPTY share preparation and cleanup. Missing protected paths have durable
identity-checked guards retained until their last execution exits. Policies must
currently be read-default path rules; exact-directory rules fail explicitly.
The Host supplies its installation and trusted runner binary;
capturing a command never installs accounts implicitly.
Read-only PowerShell can enter ConstrainedLanguage when its policy probe cannot
write TEMP. The shell launcher works in that mode without disabling application
control or granting temporary writes implicitly.

Linux and Windows expand deny globs to existing paths before each launch,
including hidden files, directories and matching symlink targets. More specific
path grants cannot reopen these denials. Scans need a literal directory prefix
below the filesystem root; incomplete or oversized scans fail before launch.
This snapshot does not protect future glob matches until the next launch.
Host file operations check globs at access time; macOS also enforces them live.

Default home reads use an installation-wide read group and a bounded background
helper. Foreground preparation stops that helper before changing ACLs; required
reads, denies and writes remain synchronous. Completed grants are reused;
interrupted propagation is retried from durable object identities. Removal stops
the helper and revokes its grants. Default reads may be temporarily unavailable
while preparation is pending; execution never falls back to unrestricted access.

On Windows, `maka sandbox setup --root PATH` and `remove --root PATH` request
administrator consent through a one-shot helper. The Host stays unelevated;
private recovery records remain owned by the invoking user. `status --root PATH`
reads durable installation state without elevation. Setup/removal observation is
bounded by `--timeout-ms` (180000 by default); cancellation does not undo accepted work.
Desktop requests setup on first use, without exposing account or ACL settings.
Cancelling either confirmation preserves the draft. Retrying setup repairs missing
accounts or an interrupted removal under the same administrator consent.

This is an execution boundary for model-directed effects, not an isolation boundary
against malicious trusted plugins.

New Sessions default to `workspace-write` with `on-request` approvals: ordinary
workspace writes are allowed; tool network access and writes outside the allowed
roots need approval. `never` refuses escalation without disabling the sandbox.
Desktop's composer permissions menu offers an explicitly confirmed full bypass
for the current task. `maka code --dangerously-bypass-approvals-and-sandbox` selects
the same `danger-full-access` + `never` combination. Neither changes future task defaults.
Code cells use the production file tools; without an approval UI, denied operations fail.

## Diagnose a command

`maka sandbox run --policy policy.json --cwd /absolute/workspace --command 'your command'`
uses the production shell, environment filtering and sandbox backend. On Windows,
also pass `--root PATH` for the configured Host installation. The policy file
contains a serialized `Sandbox`; for example, read-only files with no network:

```json
{"kind":"managed","filesystem":{"default":"read","rules":[],"denyGlobs":[]},"network":"denied"}
```

Output streams and exit status are preserved. Stdin is closed; `--timeout-ms`
defaults to 120000. Timeout returns 124 and Ctrl-C returns 130 after cleanup.
An unsupported policy fails before execution; external isolation cannot be asserted here.

## Source provenance

`src/seatbelt/{base,network}.sbpl` are copied unchanged from Codex's
[base policy](https://github.com/openai/codex/blob/d1e3f9dfe3d5f6105b7e4c3958b9bf2ac6c32818/codex-rs/sandboxing/src/seatbelt_base_policy.sbpl)
and [network policy](https://github.com/openai/codex/blob/d1e3f9dfe3d5f6105b7e4c3958b9bf2ac6c32818/codex-rs/sandboxing/src/seatbelt_network_policy.sbpl),
revision `d1e3f9dfe3d5f6105b7e4c3958b9bf2ac6c32818`.
Copyright OpenAI; Apache-2.0. The upstream source comments are retained.
