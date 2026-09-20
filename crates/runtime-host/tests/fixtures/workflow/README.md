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

# Public plugin acceptance

[简体中文](README.zh-CN.md)

This external package uses only the public Host and Client SDKs. It requires no model account. Its sources are checked by the SDK acceptance typecheck.

Copy this directory outside the source tree. Compile `client.tsx` to `client.js` with `buildClient` from `@maka-agent/plugin-sdk/build`, using package ID `example.workflow`. Install that directory with `plugin.package.install` in an isolated Desktop profile.

In Scheduled tasks, choose **Authorize and run** and approve the application-owned dialog. The result must reach `ended/completed` with a managed Session and execution receipt.

Disable both composition Entries: UI disappears, accepted Session remains. Enable them again: the receipt is identical. Choose **Revoke authorization**, then restart Host: recovery reports `revoked`, never a new execution. Remove the package and isolated profile afterwards.
