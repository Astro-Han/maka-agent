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

# Goal

[English](./README.md)

`maka.goal` 通过公共插件 API 提供目标续跑、Session Inspector、Remote 管理和
`GoalStatus` 工具。插件拥有目标、轮次策略和持久提交意图；Host 拥有授权、执行、回执与用量事实。

- 创建时明确授权目标 Session 的后台执行及用量读取；保存与启动是不同操作。
- 每轮先持久保存不可变请求和操作 ID，再提交。重启或丢回复后查询同一操作，不换 ID 重试。
- 模型报告只作用于精确的 Goal Invocation，且在该执行正常结束后生效。
- 暂停停止后续派发，不阻止已接受工作结算，也不因未提交意图占用 Host 保活。
- 取消只定位原操作及其 Host 后继执行。接受结果未知时保留原 ID，不重发、不持续保活。
- 控制携带目标 ID 和存储 revision。旧授权失败不能覆盖新授权、并发控制或替换后的目标。
- 停用停止插件工作；重新启用按持久回执协调。撤销授权后，继续或取消恢复需要重新授权。

`maxIterations` 限制预留轮数。可选 `tokenBudget` 是创建以来整个 Session 的已观测
输入与输出 token 阈值，包含其他活动，不是单请求硬上限。用量缺失或回退会停止续跑。

Remote 接受 `read`、`arm`、`control`；控制为 `pause`、`resume`、`cancel`、`complete`。
封存的 Host handoff 需要先通过 Session 控制恢复，再显式恢复 Goal。

验证：`cargo nextest run -p maka-goal`；Host 端到端验收位于
`crates/runtime-host/tests/integration/goal_plugin.rs`。
