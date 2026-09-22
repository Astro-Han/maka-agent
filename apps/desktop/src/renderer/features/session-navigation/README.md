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

# Session Navigation

[中文](README.zh-CN.md)

Owns rail membership, Host/Project grouping, linked-session navigation, geometry,
and archive/restore/rename/delete/purge actions.

Composition owns the shared Session catalog. The rail and archived-task page
subscribe directly; the shell reads only breadcrumb, revision navigation and
geometry. The command palette shares the revision-aware projection through
`application/contracts/session-catalog`.

- Production consumers use the feature index; tests may use `testing.ts`.
- Session mutations use `SessionNavigationServices`; only its Desktop adapter
  accesses the bridge. No preload, main-process or AppShell implementation imports.
- `SessionNavigationProvider` owns the controller and publishes separate data
  and chrome contexts. AppShell supplies cross-feature navigation intents.
- Opening a Session exits WorkHub, selects Sessions, then replaces or clears the
  transcript target. A linked child highlights its visible root.
- Mutations retain revision-family semantics. Renderer state is cleared only
  after Host confirmation. Purge cannot expand beyond the confirmed target set.
- Host and Project jointly identify groups. Width persistence is trailing-debounced;
  collapse, width and grouping retain their existing storage keys.
