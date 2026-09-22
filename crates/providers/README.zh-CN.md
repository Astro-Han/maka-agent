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

# 模型提供商

[English](README.md)

通过公共 `maka-plugins::provider` 契约实现随应用分发的模型提供商。提供商负责认证交换、模型发现和请求策略；Host 负责连接、凭据、代理路由与持久结算。

ChatGPT 提供商注册订阅策略并复用原生 Responses 适配器，不需要 V8。
