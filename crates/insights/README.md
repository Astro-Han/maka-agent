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

# Insights

[简体中文](README.zh-CN.md)

Usage and pricing settings implemented as a public plugin API consumer.

- Host owns accounting facts, authorization and the pricing catalog; this crate owns reports and view preferences.
- Activity, totals and breakdowns share one immutable query fence. Missing counters and valuations remain unknown.
- Rate edits use catalog revisions and affect future admissions only. Conflicts require a fresh read.
- Preferences use plugin-scoped storage CAS. Retirement withdraws the UI and Remote registrations, not accepted Host work or accounting.

The native plugin needs no V8. Its Client uses the public `settings.page` slot; package identity is not an authorization shortcut.
