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

# Module Hub

Module Hub 拥有导航／页头组合、定时任务、保持唤醒设置和 Daily Review。
Skills UI 属于 Client Contribution；此处仅挂载传入的扩展内容，不持有 Skills 控制器或目录。

生产代码通过 `index.ts` 导入，测试通过 `testing.ts` 导入。环境 I/O 经
`ModuleHubServices` 进入，由 `platform/desktop/create-module-hub-services.ts` 实现。
MCP 保留页面自身的 bridge。

`ModuleHubProvider` 将控制器放在 AppShell 下层。Shell 通过稳定命令端口操作；
定时任务边界只唤醒对应读取者。默认 Host 变化时刷新定时任务。
读取和修改反馈固定到原 Host／界面，退出清理订阅，旧清理不能断开新命令端口。
Daily Review 在等待后重新校验已捕获的 Session 和输入框再追加。

[English](README.md)
