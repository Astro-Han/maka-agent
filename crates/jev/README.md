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

# Jev structured evaluation

[简体中文](./README.zh-CN.md)

`maka.jev` is a statically linked Profile plugin. It publishes the callable
service `maka.jev.evaluate`; the separate Desktop UI entry contributes a Settings
page. Consumers use the same service from native Rust or JavaScript.

## Configuration

The Settings page manages an explicit enable switch, a full HTTP(S) endpoint,
model, and timeout (100–60000 ms). The defaults are disabled,
`https://api.typesafe.ai/v1/systemone`, `jev-latest`, and 8000 ms.
Any compatible endpoint can replace the default, including a local gateway.

API keys and **all custom header values** are stored in Host credentials. Reads
return only whether credentials exist, their revision, and header names. Saving
credentials replaces the endpoint's complete authentication/header record. An
empty record explicitly permits an unauthenticated local service. A supplied API
key generates `Authorization: Bearer …`; leave it empty to supply another
Authorization scheme through custom headers. Content-Type is always JSON.
Routing/framing headers and case-insensitive duplicates are rejected.

Credentials are bound to the full endpoint URL. Changing a URL does not forward
previous credentials; returning to a previously configured URL restores that
endpoint's credentials. Configuration and credentials use revision checks so a
stale Settings page cannot overwrite another writer. Incognito mode blocks calls.

## Consumer contract

The request and answer shapes follow [TypeSafe's System One API](https://docs.typesafe.ai/api).
Callers provide `state` and named `questions`; the plugin supplies the configured
`model`. `noul`, `choice`, and `score` answers retain probability/uncertainty and
provider token usage. Limits: 64 questions, 32 KiB evaluation input, 64 KiB response.

A consumer declares `inject: ["maka.jev.evaluate"]` on its Composition Entry
(or Definition), so activation waits for the provider. Settings are shared at
Profile scope; service binding/intercept configuration does not override them.

Native consumers resolve `decision::SERVICE` through their injected Services view
and call with `decision::Evaluation`, receiving `decision::EvaluationResult`.
JSON consumers use the same method name and JSON values. For example:

```json
{
  "state": { "tests": "passed", "remaining": [] },
  "questions": {
    "complete": { "type": "noul", "instructions": "Is all requested work complete?" }
  }
}
```

Every call requires an admitted caller scope with Host network authority. Service
availability does not grant authority. Desktop's test button obtains a separate
Network authorization and tests the saved configuration. Plugin retirement
invalidates captured service handles. Cancellation and timeout settle the scoped
HTTP resources before returning. No redirects or automatic retries forward
credentials or duplicate a potentially paid operation; a timeout/network error
can mean the provider processed the request. Provider response bodies are never
included in errors.

Jev is a structured evaluation service, not a chat model adapter. Returned usage
is provider-reported metadata; it is not added to Host model usage accounting.
A consumer must handle unavailable service, missing configuration, authorization
refusal, uncertain network outcomes, and probabilistic answers explicitly.

## Verification

`cargo test -p maka-jev` covers protocol validation, native/JSON consumers,
retirement, custom headers, privacy, redirects, limits, timeout and cancellation.
`cargo test -p maka-runtime-host --test integration jev_plugin` covers the actual
composition/Remote configuration path, revision conflicts, secret non-disclosure,
endpoint isolation, retirement and Host restart. Tests use no paid API key.
