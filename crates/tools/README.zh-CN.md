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

# maka-tools

[English](README.md)

带日志结算的工具派发与 Code Mode，设计参考 [OpenAI Codex](https://github.com/openai/codex)。
工具目录、权限与执行事实的权威始终在 JavaScript 之外。

## Code Mode

在连接的模型参数中设置 Code Mode 和 `apply_patch`。自动采用 Host 的模型默认值；
明确选择从新 Run 生效，续跑和交接保留已准入的选择。
开启 `apply_patch` 时替代 `Edit`、`Write`，关闭时使用这两个结构化编辑工具。

`exec({code, yield_time_ms?, max_output_tokens?})` 创建新的有界 V8 cell；
`wait({cell_id, yield_time_ms?, max_output_tokens?, terminate?})` 继续观察。
结果区分 `running`、`completed`、`terminated`，每次只返回新增输出。
终止表示请求取消，不表示清理已经完成。

提供 `text`、`image`、`audio`、`generatedImage`、`notify`、`yield_control`、
`store`、`load`、`setTimeout`、`clearTimeout`、`exit` 和 `ALL_TOOLS`。
图片接受 Host 图片引用、MCP 图片块或 base64 data URL。
音频保存在原始结果证据中；当前模型适配器收到的是音频元数据，不是原生音频输入。
这些辅助函数不授予文件、网络或客户端权限。

每个 Run 最多持有四个尚未收取最终结果的 cell。工具沿 cell 捕获的目录执行，
仍经过正常准入和日志结算。搜索只影响下一模型步骤，不改变当前 cell。
TypeScript 工具说明由用于校验的同一份 JSON Schema 生成；说明本身不是校验器。

每个 cell 限制源码 64 KiB、V8 堆 64 MiB、同步执行时间 30 秒、
工具调用 32 次、同时运行的工具 8 个。超出并发数的调用在总调用预算内排队。
异步 Host 等待不消耗同步执行预算。输出与 JSON 临时数据均有大小上限；
V8 堆限制不是进程隔离。

临时数据只属于当前 Run，不持久化，也不是权限存储。cell 读取快照，结算后发布写入；
同一键以后完成的写入为准。压缩清空临时数据，旧 cell 不能将其恢复。
Run 结束、交接或 Host 重启均不保留 cell 和临时数据。

## 生命周期

模型调用的 `exec` 是控制操作；独立记账的 `CodeCell` 拥有嵌套 `CodeMode` 操作。
控制调用可以先结束，cell 必须等待已接受的子工具结算后才能结束。

关闭 Run 或日志前须调用 `RunTools::shutdown`：取消 cell、等待已接受工作收尾，
并传递持久化或清理状态不明的错误。丢弃 `RunTools` 只发出取消，不能异步等待清理。
交接只封存 cell 已停止执行的边界。恢复不会重放中断的 cell，也不会静默重复其副作用。
