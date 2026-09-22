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

# 会话导航

[English](README.md)

负责侧栏成员、Host／Project 分组、关联会话导航、布局，以及归档、恢复、重命名、删除和批量清理。

组合层持有共享会话目录。侧栏与归档页直接订阅；Shell 只读取面包屑、版本导航和布局。
命令面板通过 `application/contracts/session-catalog` 复用考虑修订关系的投影。

- 生产消费者使用 feature 入口；测试可使用 `testing.ts`。
- 会话修改经过 `SessionNavigationServices`，仅 Desktop adapter 访问 bridge；
  不导入 preload、main 或 AppShell 实现。
- `SessionNavigationProvider` 持有 controller，分开发布数据和外观 Context；
  AppShell 提供跨 feature 导航动作。
- 打开会话依次退出 WorkHub、进入会话页面、替换或清除转录目标；关联子会话高亮其可见根。
- 修改保留修订族语义，Host 确认后才清除渲染状态；批量清理不扩大用户确认的目标集合。
- Host 与 Project 联合标识分组。宽度保存采用尾沿防抖，折叠、宽度和分组保留既有存储键。
