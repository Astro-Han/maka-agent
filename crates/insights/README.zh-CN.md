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

[English](README.md)

作为公共插件 API 消费者实现的用量与报价设置。

- Host 拥有计量事实、授权和报价目录；本 crate 拥有报表与视图偏好。
- 活动、总额和分组使用同一不可变查询快照，缺失计数与估价保持未知。
- 报价修改按目录 revision 提交，仅影响此后准入的调用；冲突后必须重新读取。
- 偏好使用插件作用域存储 CAS。停用撤下 UI 和 Remote 注册，不撤销 Host 已接受的工作或计量事实。

原生插件不需要 V8。客户端使用公共 `settings.page` 与 `session.inspector.overview` 插槽，包身份不构成授权捷径。Inspector 读取限定于当前会话，随计量事件刷新，不随流式文本刷新。

在仓库根目录运行 `node --test crates/insights/tests/client.test.mjs` 验证客户端生命周期。
