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

# 公共插件验收

[English](README.md)

此外部包只使用公共 Host／Client SDK，不需要模型账号。源码纳入 SDK 验收类型检查。

将此目录复制到源码树外，用 `@maka-agent/plugin-sdk/build` 的 `buildClient` 将 `client.tsx` 编译为 `client.js`，包 ID 为 `example.workflow`。在隔离的 Desktop 配置中，通过 `plugin.package.install` 安装该目录。

打开定时任务页面，点击 **Authorize and run**，确认应用展示的授权对话框。结果必须达到 `ended/completed`，并包含受管 Session 和执行回执。

停用两个 Composition Entry：界面消失，已接受的 Session 保留。重新启用：回执完全相同。点击 **Revoke authorization** 后重启 Host：恢复报告 `revoked`，不能产生新执行。结束后清理插件和隔离配置。
