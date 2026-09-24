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

# Providers

[简体中文](README.zh-CN.md)

Bundled model providers using the public `maka-plugins::provider` contract. Provider code owns authentication exchanges, model discovery and request policy; Host owns connections, credentials, proxy routing and durable settlement.

API providers own their bundled model facts, authentication, bounded inventory discovery and protocol policy. The ChatGPT provider composes the native Responses adapter; neither provider authentication nor discovery requires V8. Discovery support does not imply support for every inference protocol.

Cargo embeds the checked-in facts without Node. To refresh model and pricing data from the repository metadata snapshot and provider definitions, run from the repository root:

```sh
node scripts/rust/generate-catalog-facts.mjs crates/providers/data providers
node scripts/rust/generate-catalog-facts.mjs crates/config/data pricing
```

Review the generated data changes together with provider tests; metadata updates do not implement new inference protocols.
