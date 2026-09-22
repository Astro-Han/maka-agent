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

# maka-process

[English](README.md)

Linux、macOS 和 Windows 的原生进程／PTY 传输、取消、沙箱启动接入和无界面终端状态。

`terminal::Screen` 使用 `alacritty_terminal`，不启用其 PTY 事件循环或渲染器。
解析器归现有 PTY worker 所有，不需要 JavaScript runtime、额外线程或跨 runtime 序列化。
Host 保留执行准入、规范日志、背压和最终输出排空；Desktop 仍用 xterm 渲染。

快照提供可见屏幕、500 行历史、光标／输入模式和最后一次备用屏幕。
历史丢失或文本超限会设置 `truncated`。写入、转义序列扩展、组合字符、OSC 存储和
协议应答均有资源上限。解析失败不发布部分状态，Host 保留最后一次成功快照。
剪贴板、标题和超链接副作用不会执行。转义序列行为遵循 Alacritty，不模拟全部 xterm 差异。
