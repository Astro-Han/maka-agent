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

通过公共插件读取能力，将 Codex rollout 和 Claude Code transcript 转换为历史记录。调用方提供固定文件句柄和所选来源身份；本 crate 不重新打开路径、不派发工具，也不写入规范事件。

- Codex 使用对话事件与工具响应记录，应用来源回滚，排除重复的 provider 消息镜像。
- Claude 选择修改后的用户提问，但保留并行工具结果及压缩前历史。三次校验摘要的读取依次解析分支、合并回复分片、生成历史。
- 工具调用和结果保留来源顺序与关联；缺失的结果仍然缺失。
- 文件目录查询只读有界摘要（Codex：512 KiB；Claude：头尾各 256 KiB），按工作目录和归档条件筛选，以修改时间和相对路径分页。游标绑定查询，即使页面因 48 KiB 传输预算提前结束，也从最后实际交付的记录之后继续。
- 来源工作目录和模型名称仅是观察信息，不是执行配置；凭据和 provider options 不进入结果。
- 中间 JSON 损坏、来源身份不符或多次读取内容变化时拒绝导入。只有末尾未完成的 JSON 写入会被省略，并记录在文件指纹中。

读取有界：来源前缀最多 2 GiB，单行 64 MiB，来源行数一百万，每行 JSON 结构标记 65,536 个。保留载荷在解码前检查容量；编码后的历史受 Runtime 的 6 MiB／7,500 条记录限制。Claude 另有限制分支索引和回复分片的预算。超限时拒绝，不截断历史。

返回的 transcript 尚未发布为 Session；发布和重试回执由公共 Session 导入能力负责。
