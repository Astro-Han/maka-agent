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

# maka-sandbox

[English](README.md)

沙箱策略与平台启动准备，设计参考 [OpenAI Codex](https://github.com/openai/codex)。

Host 负责授权、审批与执行事实；本 crate 计算文件和网络限制，为已授权命令准备
操作系统隔离。进程 I/O、PTY 所有权和取消仍由 `maka-process` 管理。
无法实现隔离时必须明确失败，不得退回无限制执行。
命令计划不分配原生沙箱资源；准入后的进程 worker 统一负责准备、启动和清理。

网络授权指定确切主机和端口，与上游代理分离。按目的地限制的 HTTP／CONNECT 流量
经过执行拥有的网关，重定向目标需要单独获准。macOS 只允许连接本网关，Linux 将
监听器放入私有网络命名空间，Windows 将账户绑定到专属网关端口。上游路由和认证
留在 Host 内；执行结束关闭隧道，权限变更不会扩大已运行进程的权限。

不隐式继承动态加载器钩子。Unix 托管进程不能通过环境覆盖修改隔离启动器的加载器；
确有需要时，在沙箱内的目标命令中设置，例如 `env NAME=value program`。

Linux 需要 `/usr/bin/bwrap` 提供 bubblewrap 0.11 或更新版本；源码包不内嵌它。
缺失的受保护叶路径使用临时挂载占位，由内核租约约束生命周期。缺失祖先共用叶节点的
租约，按子节点优先清理，保留用户写入的内容。清理同时检查 inode
和所有权标记，不递归删除工作区路径。暂存区选择与工作区同文件系统的 `/tmp` 或
用户主目录缓存，要求支持用户 xattr。其他文件系统上的缺失受保护路径、缺失的
可写根和精确目录规则暂不支持，遇到这些策略会明确失败。

Windows 托管命令使用预配置的低权限账户、账户级 WFP 规则、缓存权限表面和每执行独立的
Job。相同权限表面复用写入能力，可写对象身份改变时撤销旧能力；修改 ACL 必须等待槽位
空闲且原生 Job 已清空。缓存本身不是授权，每次启动仍检查当前权限。
前台命令、管道与 ConPTY 复用准备和清理链路。缺失保护路径使用持久化、
校验文件身份的占位，直到最后一个执行退出才回收。目前仅支持默认可读的路径规则；
精确目录规则会明确失败。Host 显式提供安装位置与可信 runner，
捕获命令不会隐式安装账户。
只读 TEMP 可能使 PowerShell 的策略探测进入 ConstrainedLanguage；启动器兼容该模式，
不关闭系统应用控制，也不隐式增加临时目录写入权限。

Linux／Windows 在每次启动前把拒绝 glob 展开为已有路径，包含隐藏文件、目录与
匹配符号链接的目标；更具体的路径授权不能重新开放它们。扫描必须具有文件系统根
以下的字面目录前缀；扫描不完整或超限时，启动失败。快照不保护启动后新出现的
匹配路径，下一次启动会重新扫描。Host 文件操作始终在访问时检查 glob；macOS 的
进程隔离也会实时检查。

默认主目录读取由安装级共享读组和限时后台 helper 准备。前台修改 ACL 前先停止该
helper；必要读取、拒绝规则与写入权限仍同步完成。完整授权复用，中断的传播依据
持久对象身份重试；卸载停止 helper 并撤销授权。准备期间部分默认路径可能暂时
不可读，不会因此退回无隔离执行。

Windows 下，`maka sandbox setup --root PATH` 和 `remove --root PATH` 通过一次性
助手请求管理员授权，Host 不常驻提权，私有恢复记录仍归发起用户所有。
`status --root PATH` 无需提权即可查询持久化安装状态。安装／卸载的等待由
`--timeout-ms` 限制（默认 180000）；取消等待不撤销已接受的工作。
Desktop 首次使用时引导启用，不暴露账户或 ACL 设置；取消应用确认或系统 UAC 均保留草稿。
再次启用会在同一次管理员授权下自动补齐缺失账户，或修复被中断的卸载。

这是模型驱动操作的执行边界，不是针对恶意受信任插件的隔离边界。

新会话默认使用 `workspace-write` 与 `on-request`：允许普通工作区写入；工具联网和
超出允许根目录的写入需要审批。`never` 拒绝扩权，不会关闭沙箱。
Desktop 输入框的权限菜单可经明确确认后，为当前任务完全绕过保护；
`maka code --dangerously-bypass-approvals-and-sandbox` 使用相同的
`danger-full-access + never` 组合。两者均不改变后续任务默认值。
Code cell 复用生产文件工具；没有审批界面时，被拒绝的操作直接失败。

## 命令诊断

`maka sandbox run --policy policy.json --cwd /absolute/workspace --command 'your command'`
复用生产 Shell、环境过滤与沙箱后端。Windows 还需传入 `--root PATH` 指定已配置的
Host 安装位置。策略文件保存序列化的 `Sandbox`，例如只读且禁网：

```json
{"kind":"managed","filesystem":{"default":"read","rules":[],"denyGlobs":[]},"network":"denied"}
```

保留输出流与退出码，标准输入关闭。`--timeout-ms` 默认为 120000；
超时清理后返回 124，Ctrl-C 清理后返回 130。不支持的策略在执行前失败，
不能在此声明已建立外部隔离。

## 源码来源

`src/seatbelt/{base,network}.sbpl` 原样复制自 Codex 的
[基础策略](https://github.com/openai/codex/blob/d1e3f9dfe3d5f6105b7e4c3958b9bf2ac6c32818/codex-rs/sandboxing/src/seatbelt_base_policy.sbpl)
和[网络策略](https://github.com/openai/codex/blob/d1e3f9dfe3d5f6105b7e4c3958b9bf2ac6c32818/codex-rs/sandboxing/src/seatbelt_network_policy.sbpl)，
对应提交 `d1e3f9dfe3d5f6105b7e4c3958b9bf2ac6c32818`。
版权归 OpenAI，采用 Apache-2.0 许可，保留上游源码注释。
