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

# 会话导入

[English](README.md)

通过公共插件读取能力，将 Codex、Claude Code 和 OpenCode 对话转换为历史记录。调用方提供所选来源身份，以及固定文件句柄或已授权的数据库读取视图；本 crate 不自行打开环境路径、不派发工具，也不写入规范事件。

- Codex 使用对话事件与工具响应记录，应用来源回滚，排除重复的 provider 消息镜像。
- Claude 选择修改后的用户提问，但保留并行工具结果及压缩前历史。三次校验摘要的读取依次解析分支、合并回复分片、生成历史。
- OpenCode 在同一个 SQLite 快照中读取所选根 Session、消息和片段，应用整条消息或部分片段的撤回标记。字段类型错误、重复身份、孤立片段或已完成工具缺少结果时拒绝导入；未完成调用保持未完成。
- 工具调用和结果保留来源顺序与关联；缺失的结果仍然缺失。
- 文件目录查询只读有界摘要（Codex：512 KiB；Claude：头尾各 256 KiB），按工作目录和归档条件筛选，以修改时间和相对路径分页。游标绑定查询，即使页面因 48 KiB 传输预算提前结束，也从最后实际交付的记录之后继续。
- OpenCode 目录分批读取根 Session，按来源时间和身份排序。工作目录和文本筛选复用文件目录的规范化规则；游标绑定查询和数据库。分页观察仍在变化的来源，不持有长期快照。
- Codex 目录选择代数最高的 `state_N.sqlite`，在公共数据库预算内读取最多 100,000 项索引。Rust 统一规范化时间戳，只保留下一页的候选；rollout 路径仍须经公共文件能力校验。首页数据库不可用时回退到文件，不选旧数据库；后续页保留已选的数据库代数或文件来源。
- 来源工作目录和模型名称仅是观察信息，不是执行配置；凭据和 provider options 不进入结果。
- 中间 JSON 损坏、来源身份不符或多次读取内容变化时拒绝导入。只有末尾未完成的 JSON 写入会被省略，并记录在文件指纹中。

JSONL 读取有界：来源前缀最多 2 GiB，单行 64 MiB，行数一百万，每条记录的 JSON 结构标记 65,536 个。OpenCode 快照同时受公共数据库的行数、字节数和工作量限制。保留载荷在解码前检查容量；编码后的历史受 Runtime 的 6 MiB／7,500 条记录限制。Claude 另有限制分支索引和回复分片的预算。超限时拒绝，不截断历史。

返回的 transcript 尚未发布为 Session；发布和重试回执由公共 Session 导入能力负责。
