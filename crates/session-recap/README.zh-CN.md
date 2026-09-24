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

# 会话回顾

[English](./README.md)

`maka.session-recap` 在 Session Inspector 中手动生成一句回顾，也提供独立 Remote 供客户端调用。
仅使用公共历史、模型、偏好和插件存储 API，不修改规范历史或 Session 元数据。

`read` 需要目标 Session 的历史读取权限；`generate { operationId }` 还需要模型权限。
缓存读取同样检查当前授权，隐私模式拒绝读取和生成。

生成前原子保存操作意图和最新指针，完成只更新该操作的回执。重复 ID 不再次调用模型，
旧请求完成不覆盖新请求。重启或丢回复后可查询原回执；Pending 表示结果未确认，
不自动重发。新 ID 是另一次显式请求，可能产生额外费用。

输入固定历史水位，最多扫描 256 页／准备步骤，保留最近 32 KiB 文本。
模型调用限制为 30 秒、1024 输出 token；不完整或空结果不发布为成功。
回顾是文本摘要，不是任务成功证明；自动 idle 回顾与 Daily review 尚未实现。

验证：`cargo nextest run -p maka-session-recap`；Host 集成测试覆盖授权、真实历史、模型调用、停用与重启。
