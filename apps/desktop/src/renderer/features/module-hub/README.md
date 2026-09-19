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

# Module Hub

Module Hub owns navigation/header composition, Scheduled Tasks, keep-awake settings and Daily Review.
Skills UI is a Client Contribution; Module Hub mounts the supplied extension content without a Skills controller or catalog.

Production imports use `index.ts`; tests use `testing.ts`. Environment I/O enters through
`ModuleHubServices`, implemented by `platform/desktop/create-module-hub-services.ts`.
MCP retains its page-owned bridge.

`ModuleHubProvider` keeps controllers below AppShell. A stable command port serves Shell actions;
the Scheduled Tasks boundary updates its reader without waking the Shell.
Default-Host changes refresh Scheduled Tasks. Reads and mutation feedback retain their originating
Host/surface; cleanup disposes subscriptions and cannot detach a newer command-port owner.
Daily Review append validates the captured Session and composer after awaiting.

[中文](README.zh-CN.md)
