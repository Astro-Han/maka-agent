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

# Rust Runtime Host

[English](./rust-runtime.md)

Rust workspace 重写 Maka runtime 与 host，保留 TypeScript client 协议与交互。双方使用协议 epoch 152。
重写尚未完成；未实现的操作明确返回错误。

## 构建与运行

需要 Rust 1.98+、原生 C/C++ 工具链、Node 和仓库 npm 依赖。Provider SDK 与终端解析器
在构建时打包，运行二进制不需要 Node。

```sh
npm install
cargo build --locked -p maka-cli
cargo run --locked -p maka-cli -- --help
npm run dev
```

Desktop 默认启动 Rust host，使用 `userData/runtime-host-rust`，不迁移或接管已有
TypeScript State Root。独立运行时使用新目录：

```sh
maka host init --root /absolute/path/to/new-root
maka host serve --root /absolute/path/to/new-root
```

二进制未加入 PATH 时使用 `target/debug/maka`。Linux/macOS 使用 Unix socket，
Windows 使用私有 named pipe。可选 `--websocket 127.0.0.1:0` 监听需要认证，尚不支持 TLS。

唯一二进制还提供 Desktop 启动用的 `host candidate`、从 stdin 读取 JavaScript cell 的
`code --log <file>`，以及查看已提交执行事实的 `inspect --log <file>`。

**没有 OS 沙箱。** 代码与工具使用当前用户的系统权限。不要执行不可信代码，也不要让测试
实例接管已有用户数据。

## 设计

- **Log Is the Runtime：**模型历史、transcript 与恢复来自已提交的语义事实。上下文压缩
  改变模型投影，不改写历史。
  失败响应的片段只用于展示，不纳入模型历史；用户取消不显示为 provider 失败。
- 模型消息、内容块与工具结果在 provider 投影中保持类型化；路由与发现共享类型化契约。
  工具 JSON、schema 和厂商扩展保留开放结构。
- 每个 State Root 只有一个写入与执行 authority；Session、Turn、Run、invocation
  身份保持独立。
- 手动 resume 先只读检查已封口的源，再原子开启续跑。模型只回放选定谱系，不混入后来的旁支或
  未完成的响应片段。未知副作用阻止准入；重复已接受的请求返回原 Turn，重启后仍然如此。
- 类型化模型列表贯通发现、存储和目录投影，连接自有声明单独保存；
  容量、主动压缩阈值与单次回复预算互相独立。
- 工具先提交派发，再执行副作用，最后提交结果。结果未知不代表可以重做；取消必须等待
  已准入工作收尾。
- 执行中可单独扩大权限，新工具调用捕获已提交的边界；收紧权限须等待执行静止及原生
  资源清理完成。
- Read 用 `path` 读取文件和 Session 资源，返回有界页与校验内容的续页地址。
  事件地址只读取冻结的模型证据，不暴露模型投影省略的原始输出。大段文本结果在下次
  模型请求前持久化有界首屏；媒体和原始执行事实保持完整。
- 延迟工具在搜索成功后的下一步才可调用，已提交的上下文压缩会卸载它们。Code Mode 只暴露
  `exec`，说明中列出当前可嵌套调用的工具；已有调用保持捕获时的作用域。
  每个逻辑步骤成对捕获工具定义与执行器，物理重试及返回的工具调用共用该视图。
- `turn.start` 与 `turn.message.submit` 的显式 Skills 在准入时冻结正文和回执。排队消息保留必需工具集合；
  promote 与后继执行按实际目标 Run 校验，不重新加载技能文件。
- `SkillSearch`、`Skill` 与显式加载共用 Run 的冻结目录。搜索只返回有界元数据，
  加载的正文保留可读的归档分页。
- Agent 模式的技能选择器按当前权限预览，不绑定 Session 或解析模型；分页绑定修订。
  内置及本地来源目录反映真实安装占用，并识别经校验的托管来源别名。治理查询展示校验、
  偏好和来源更新状态，不读取 baseline，也不冒充 Run 内已加载状态。Plan 与技能变更仍未实现。
- Rust 管理存储、网络路由、工具和原生进程／PTY。一个惰性启动的长期 V8 并发处理模型
  请求与终端解析；Code Mode 使用独立短生命周期 isolate。数量与字节限制提供背压，
  V8 heap 限制不等于进程内存隔离。
- 插件后续须通过目录注册和有作用域的 Host 服务接入，复用日志、权限与排空机制，不替换 Engine。
- Code Mode 限制累计 VM 执行时间，不计异步工具等待或收尾时间。
- 请求的代理策略同时覆盖 HTTP 与 Responses WebSocket。WS 握手失败后指数退避重试
  5 次，再经同一网络策略降级 HTTP。主模型请求另对已识别的临时 provider 故障最多尝试
  10 次，使用冻结输入与可取消退避。Provider 工具活动或重放元数据阻止重试；未知/本地
  错误、尚未分类的网络故障与超时不重试。

## 代码组织

所有 crate 位于 `crates/`，目录按职责命名。

| 边界 | Crate |
| --- | --- |
| 事实与持久化 | `runtime`、`event-log`、`presentation`、`config` |
| 执行 | `agent`、`model`、`js-runtime`、`tools`、`fs-tools`、`process`、`apply-patch`、`skills` |
| 客户端与 Host | `protocol`、`transport`、`client-capability`、`network`、`runtime-host` |
| 可执行程序 | `cli` |

Runtime core 不依赖 V8 或 SQLite；持久 schema 由 SQLx migration 管理。
Client Capability 注册与反向调用所有权位于 `client-capability`，Host 负责组合执行。

## 开发验证

```sh
cargo fmt --all --check
cargo nextest run --locked --workspace -j 4
cargo test --locked --workspace --doc
cargo clippy --locked --workspace --all-targets -- -D warnings
node scripts/asf-license-headers.mjs check
```

单元测试放源码模块末尾，集成测试放 `tests/`。业务契约优先使用 struct／enum，
JSON Value 只用于真正开放的载荷和 schema。依赖 V8 的测试在各 crate 内共用一个
测试二进制，避免重复链接。普通测试使用本地 fixture；真实服务测试需要显式启用及凭据。
独立 worktree 可将 `MAKA_JS_DEPS` 和 `NODE_PATH` 分别指向已有依赖的 checkout
及其 `node_modules`。
共享跨语言 fixture 放在根目录 `tests/fixtures`，通过 `tests/support/source.mjs` 加载当前 TypeScript 源码，不读取 workspace `dist`。

Grep 差分测试需要 PATH 中有 `rg`；runtime 本身不依赖该可执行文件。

## 当前限制

已实现项目／会话管理、已结束 Turn 导航、配置、附件、文件工具、shell／PTY、Client Capability 工具、
模型流式交互与上下文压缩。Codex 订阅已接入执行；Copilot／xAI 推理适配和实测后置。

WorkHub 已支持专属会话解析、查询、模型配置、受限 Desktop 工具与附件读取，以及候选发现和向已有空闲或新建会话委派任务。
委派时原子转移附件；关联操作与交互式目标选择仍未完成。

Skills 变更与更新预览、高级恢复与协调、
编排、部分 capability 服务、受管升级及其它协议域
仍未完成。完整 Desktop 验收和 Linux、macOS、Windows 发布打包仍待完成。
插件实现排在 Agent Graph 之后；OS 沙箱暂缓。Memory 留待单独重做，不移植旧实现，也不纳入本次重写。内容脱敏不实现。
