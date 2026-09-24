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

API 提供商拥有随包模型事实、认证、有界模型发现和协议策略。ChatGPT 提供商复用原生 Responses 适配器；提供商认证和发现均不需要 V8。发现可用不代表所有推理协议都已支持。

Cargo 直接嵌入已提交的数据，无需 Node。从仓库元数据快照及提供商定义刷新模型和计费数据时，在仓库根目录执行：

```sh
node scripts/rust/generate-catalog-facts.mjs crates/providers/data providers
node scripts/rust/generate-catalog-facts.mjs crates/config/data pricing
```

结合提供商测试审查生成的数据变更；元数据更新不等于实现新的推理协议。
