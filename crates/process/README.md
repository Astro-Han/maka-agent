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

# maka-process

[中文](README.zh-CN.md)

Native process and PTY transport, cancellation, sandbox launch integration and
headless terminal state for Linux, macOS and Windows.

`terminal::Screen` uses `alacritty_terminal` without its PTY event loop or renderer.
The existing PTY worker owns the parser; no JavaScript runtime, extra thread or
cross-runtime serialization is needed. The Host retains admission, canonical
records, backpressure and final output drain. Desktop rendering remains xterm.

Snapshots expose the viewport, 500 rows of history, cursor/input modes and the
last alternate screen. History loss or text-budget clipping sets `truncated`.
Writes, escape expansion, combining characters, OSC storage and protocol replies
are bounded. A failed parser cannot publish partial state; the Host retains its
last successful snapshot. Clipboard, title and hyperlink effects are suppressed.
Terminal escape handling follows Alacritty, not exact xterm emulation.
