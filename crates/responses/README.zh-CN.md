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

# Responses

[English](README.md)

原生 Rust Responses 编码、流解码及可丢弃的 WebSocket 续接状态，不依赖 V8。凭据、请求准入、规范历史与用量结算由 Host 管理。

设计参考 [OpenAI Codex](https://github.com/openai/codex/tree/94174e44cbc54cece45f6052328ca0c2cd7a8a2a/codex-rs/codex-api)。HTTP 与 WebSocket 共用事件解码器；连接缓存仅作优化，不构成重放权威。
