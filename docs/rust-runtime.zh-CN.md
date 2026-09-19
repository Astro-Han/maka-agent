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

Rust workspace 重写 Maka runtime 与 host，保留 TypeScript client 协议与交互。双方使用协议 epoch 163。
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
maka host status --root /absolute/path/to/new-root
maka host retire --root /absolute/path/to/new-root
```

Desktop 在 Host 就绪前开放导航和草稿编辑。Host 不可用不会退出应用，可以重试、切换 Host、
复制诊断或退出。草稿文本由 Desktop 保存，不进入发送队列；离线发送会被拒绝并保留草稿。
启动分别记录 `mainInteractiveMs`（主界面可交互）与 `hostReadyMs`（Host 就绪）。

有限 CLI 命令接受 `--timeout-ms`（1–600000）：status/logs 默认 15 秒，其余操作默认 180 秒。
Desktop 每次恢复共享 45 秒预算、最多尝试五次；退出包含清理，共享 8 秒预算。子阶段使用剩余
时间。下载报告实际字节进度，重复心跳不会重置停滞检测。超时只结束观察，不取消已接受的工作
或释放其锁；一次性独立命令进程可能继续收尾，它不是常驻控制进程。结果待确认时先查询
`host status` 再决定是否重试。`operation: in_progress` 表示执行锁被持有，与 Host 正常活动
分开报告。恢复使用原有 pending update 的冻结目标，不强杀归属不明的进程，也不删锁接管。

二进制未加入 PATH 时使用 `target/debug/maka`。Linux/macOS 使用 Unix socket，
Windows 使用私有 named pipe。可选 `--websocket 127.0.0.1:0` 监听需要认证，尚不支持 TLS。
对运行中的 Host 执行 `host access prepare --root <目录> --principal <id>` 可获得配对 JSON，
其中包含秘密凭据，须私密传递。待确认凭据在 15 分钟后过期；导入客户端须先确认配对，再用相同客户端身份重连。
该 Desktop owner 策略不授予任意 Host 路径访问权。
`host access revoke --root <目录> --credential-id <id>` 撤销凭据并关闭其远程连接。
这些命令不会启动 Host 或迁移数据。
开发构建保留文件与行号回溯；`CARGO_PROFILE_DEV_DEBUG=full` 可启用完整调试信息。
Windows MSVC 构建使用与官方 V8 静态库一致的静态 CRT。
两份随仓库保存的 Deno TypeScript 须与 `deno_telemetry` 完全一致，升级依赖时须同步；
构建直接转译它们，不再加载构建期 V8。

`host connect --root-id <rootId> --framed` 激活该部署，通过 stdin/stdout 桥接客户端协议，
诊断写入 stderr。Linux/macOS 的输入 EOF 半关闭连接并排空响应；Windows 管道 EOF 表示断连，
客户端须先收完响应再关闭 stdin。WSL 传入 `--repair-root-after-remount`，显式确认重挂载后
Linux inode 未变并保留 Root ID。不要用它接管复制的根或旧版数据。

`host install --root <目录>` 固定当前可执行文件和按需策略；`--mode supervised` 选择持续运行。
只有返回的 `executable` 可以启动该托管根。`host activate --root-id <rootId> --framed`
复用就绪 Host，或启动固定版本；supervised 模式会注册并启动账户级 systemd 服务、LaunchAgent
或 Windows 计划任务。Linux 要求用户服务管理器已运行且启用 linger；macOS 要求 Aqua 登录，
Windows 要求交互式用户会话。单独安装不会启动服务或修改账户策略。
`host setup --root <目录>` 合并安装与激活；可选 `--principal <id>` 同时返回短期 Desktop
配对凭据，须将该 JSON 视为秘密。交互式启动器使用 `--framed` 并隐藏
`__MAKA_NATIVE_HOST_SETUP__` 回执行。重复 setup 保留已有代码和未显式指定的配置，变更仍须使用
`host update`。install/setup 省略 `--root` 时使用账户的原生 `runtime-host-rust` 目录，与旧 TS 状态分离。

`host fetch --target <目标> --version <精确版本> --cache <目录>` 从 npm 预备
`@maka-agent/cli-<目标>`，不安装或启动 Host。目标支持 `darwin-arm64`、`darwin-x64`、
`linux-arm64-gnu`、`linux-x64-gnu`、`win32-x64`。下载验证 SHA-512、包身份及二进制头；
缓存命中可离线使用，仍重新校验文件摘要。遵循 CLI 代理环境变量。本地包可改用
`--archive <文件.tgz> --integrity sha512-<base64>`，不访问 npm。
返回的 JSON 包含 Windows 两个入口路径。`--directory <已验证包目录> --receipt-sha256 <摘要>`
根据原验证器的回执导入传输后的包。默认缓存为账户的 `native-cli` 目录，已保存配置会引用其中的可执行文件。
现阶段原生包使用独立 npm channel `rust-preview`，不使用 `latest`。

Linux 发布支持 glibc 2.28 及以上。`node scripts/rust/build-cli.mjs --release` 使用
`cargo zigbuild`，显式指定 `x86_64-unknown-linux-gnu.2.28` 或 `aarch64-unknown-linux-gnu.2.28`；
构建机器须安装 cargo-zigbuild 和 Zig。开发构建仍使用普通 Cargo，SSH／WSL 引导在下载前拒绝旧版 glibc。

Desktop SSH／WSL 引导在本机下载并验证 `nativeRuntimeHostVersion` 固定的完整包，再传输、清理暂存目录并设置原生 Host。
该字段是独立于 Desktop 版本的精确 npm 版本，打包时可通过 `MAKA_NATIVE_CLI_VERSION` 指定。
已发布的 preview 覆盖 macOS arm64、Linux x64、Windows x64。
目标无需 Node/npm/Rust。开发构建可设置 `MAKA_NATIVE_CLI_VERSION` 和
`MAKA_NATIVE_CLI_PACKAGES`（由 `host fetch` 填充的缓存目录），正式打包的 Desktop 忽略这些覆盖。
已有配置的启动无需访问 npm。

Desktop 复用原生托管 Host，需要时激活固定版本；自身版本变化和退出不控制托管 Host 的寿命。
暂停本地启动后，会等待已发出的激活收尾，再交接 Root。
Desktop 可管理本机原生部署及已有的 SSH／WSL 原生 operator 配置。远程生命周期命令使用
SSH／WSL 的操作系统权限，不扩大 WebSocket 凭据权限。停止/卸载保持连接暂停；启动/重启/更新
即使结果未确认也恢复正常重连，由激活流程检查真实部署，不自动重放变更。
部署权威保存在 State Root 之外的账户级 SQLite 中，启动时先校验再执行数据库迁移。
Windows 托管服务使用同目录的 `maka-service.exe`，它是同一 Host 的无窗口入口，须与 `maka.exe` 一起分发。
按需激活要求启动环境允许脱离 Windows Job；应直接运行已构建的程序，不要通过 `cargo run` 激活。
关闭开始后，CLI 最多等待十秒清理，超期以 70 退出；中断工作的结果由日志恢复判定，不视为已经回滚。

更新部署时，用新二进制运行
`host update --root-id <rootId> --expected-deployment-id <deploymentId> --expected-revision <revision>`。
同一次更新可设置 `--mode`、`--websocket`，以及可重复的
`--project-root-json '{"label":"Projects","path":"/absolute/path"}'`。
`--no-project-roots` 不发布目录；`--default-project-roots` 恢复账户默认目录。省略的配置保留不变。
代码和配置共用一个目标及 revision；`reconcile` 不重新选择配置。
活跃客户端或不可交接任务会推迟切换；随后用相同身份参数运行 `host reconcile` 完成已记录的更新。
目标一旦提交，即使启动失败也不自动回退。Supervised 激活仅在持有 Root 时替换服务定义。
升级以可恢复的短暂重启为目标，不保证 socket 或 PTY 连续存活，不计划增加独立控制进程。

`host upgrade` 使用相同身份参数，先下载 `rust-preview`（或 `--version` 指定的精确版本），再委派该版本完成更新。
Desktop 同样先完成下载和 SSH／WSL 传输，再暂停连接。
`host update-policy --root-id <rootId>` 查询自动更新策略；修改时附加
`--policy rust-preview|manual --expected-policy-revision <revision> --expected-deployment-id <id>`。
默认手动更新。自动更新使用独立 OS 定时任务，不增加常驻进程；成功后每小时检查，失败或工作繁忙时每十分钟重试。
空闲客户端在切换后重连，执行中的任务、PTY 和 OAuth 会推迟切换；按需 Host 不被更新任务唤醒。
关闭策略会阻止已排队的自动更新，卸载同时删除定时任务。`lastError` 报告尝试失败；
`schedulingError` 表示策略已保存但 OS 任务未就绪，可重复同一请求修复。

`host stop`、`host restart`、`host uninstall` 使用相同的身份参数。
停止和重启保留待更新目标。卸载先撤销启动资格，再注销服务；Root 数据、代码包及部署撤销记录均保留。
若 `cleanup.kind` 为 `pending`，重试相同卸载命令；显式安装会在旧服务清理完成后授予新的部署身份。

`host status --root-id <rootId>` 分别读取部署、待更新目标、OS 服务及活体 Host，不启动或修复它们；
Host 不可连接不代表进程已停止。`host logs --root-id <rootId>` 返回最多 48 KiB 的托管诊断尾部，
`byteTruncated` 标明省略的字节。Linux 选择最近 200 条 journal 记录；macOS/Windows 读取 stderr。
这些是诊断信息，不是执行历史；按需 Host 不捕获 stderr。

`maka` 命令还提供 Desktop 启动用的 `host candidate`、从 stdin 读取 JavaScript cell 的
`code --log <file>`，以及查看已提交执行事实的 `inspect --log <file>`。

**没有 OS 沙箱。** 代码与工具使用当前用户的系统权限。不要执行不可信代码，也不要让测试
实例接管已有用户数据。

## 设计

原生插件通过 `PluginContext.data` 使用按 package/scope 划分的私有文件目录。
文件工作持有 Fiber 至完成，退休拒绝新操作但不删除数据。Root 只校验核心文件和目录安全，
不识别业务名称；文件格式、锁与恢复属于插件。既有用户／项目内容路径与私有 journal 分开。

- **Log Is the Runtime：**模型历史、transcript 与恢复来自已提交的语义事实。上下文压缩
  改变模型投影，不改写历史。
  失败响应的片段只用于展示，不纳入模型历史；用户取消不显示为 provider 失败。
- 模型消息、内容块与工具结果在 provider 投影中保持类型化；路由与发现共享类型化契约。
  工具 JSON、schema 和厂商扩展保留开放结构。
- 订阅收到 `subscription.ready` 后才交付帧。重连按背压从已提交日志补发活动文本，
  不复制整份 transcript overlay。
- 每个 State Root 只有一个写入与执行 authority；Session、Turn、Run、invocation
  身份保持独立。协作续接保留对外 Run 身份；恢复凭证与清理仍绑定精确物理 Run。
  已封口的任务可直接取消，无需加载 provider，也不重做副作用。
- Host 诊断与退出共用活动工作统计，包含等待授权的 OAuth 登录。退出绑定精确 Host epoch，
  回复发送与资源清理完成前不释放 authority。协作交接在步骤结算后封口，重启按冻结组合续接；
  缺少原客户端所有者时保持暂停。封口前可撤销准备，协作许可不授予中断其它客户端连接、PTY
  或 OAuth 流程的权限。旧协议字段 `nodeVersion` 如实返回 `not applicable (Rust)`。
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
- `SkillSearch`、`Skill` 在每逻辑模型步骤共同绑定目录、handler 和支持上下文；物理重试不变，下一步可观察变化。
  搜索只返回有界元数据，
  加载的正文保留可读的归档分页。
- Agent 模式的技能选择器按当前权限预览，不绑定 Session 或解析模型；分页绑定修订。
  内置及本地来源目录反映真实安装占用，并识别经校验的托管来源别名。治理查询展示校验、
  偏好和来源更新状态，不读取 baseline，也不冒充 Run 内已加载状态。目录视图共享版本，游标另绑定视图。
  `maka.skills` 内置插件拥有发现、输入展开、启用／固定 CAS、原始字节更新预览及可恢复的创建／安装／删除／更新。
  Client bundle 提供管理、选择器和草稿建议；Desktop 提供目标绑定的 Slot 和授权原生文件操作。
  停用后新显式引用失败，普通聊天与已接受回执不受影响。Plan 模式执行属于独立领域，尚未实现。
- Rust 管理存储、网络路由、工具和原生进程／PTY。一个惰性启动的长期 V8 并发处理模型
  请求与终端解析；Code Mode 使用独立短生命周期 isolate。数量与字节限制提供背压，
  V8 heap 限制不等于进程内存隔离。
- 插件通过目录注册和有作用域的 Host 服务接入，复用日志、权限与排空机制，不替换 Engine。
- Code Mode 限制累计 VM 执行时间，不计异步工具等待或收尾时间。
- Responses reasoning 遵循声明的加密、明文正文或明文摘要契约。摘要重放保留 item 身份和
  Unicode 安全的分段边界，不重复存储正文；无效元数据不参与重放。
- 请求的代理策略同时覆盖 HTTP 与 Responses WebSocket。WS 握手失败后指数退避重试
  5 次，再经同一网络策略降级 HTTP。主模型请求另对已识别的临时 provider 或原生网络故障最多尝试
  10 次，使用冻结输入与可取消退避。Provider 工具活动或重放元数据阻止重试；未知/本地
  错误、尚未分类的网络故障与空闲超时不重试。
  模型活动刷新 120 秒空闲预算，持续输出不受两分钟总时限限制；用户取消仍关闭请求并等待收尾。
  真实 provider finish 到达后释放流，不等待传输 EOF；残缺流合成的 finish 不算成功完成。

## 代码组织

所有 crate 位于 `crates/`，目录按职责命名。

| 边界 | Crate |
| --- | --- |
| 事实与持久化 | `runtime`、`event-log`、`presentation`、`config` |
| 执行 | `agent`、`model`、`js-runtime`、`tools`、`fs-tools`、`process`、`apply-patch`、`skills` |
| 插件生命周期与工具目录 | `plugins`、`tool-catalog` |
| 客户端与 Host | `protocol`、`transport`、`client-capability`、`network`、`runtime-host` |
| 可执行程序 | `cli` |

Runtime core 不依赖 V8 或 SQLite；持久 schema 由 SQLx migration 管理。
Client Capability 注册与反向调用所有权位于 `client-capability`，Host 负责组合执行。

## 开发验证

`node scripts/rust/release-cli.mjs --source <源码.tar.gz> --keys <KEYS>
--target <目标> --validator <本机maka> --notices <已审阅许可证文档>
--build-id <构建标识> --output <目录>` 校验源码归档及相邻的校验和、签名文件，安装锁定的 npm 依赖并应用仓库补丁，
再编译、打包原生 CLI。Cargo workspace 与 `maka --version` 保持源码版本；npm 使用
`<源码版本>-rust-preview.<构建标识>`，例如 `0.2.0-rust-preview.20260916.1`。
同一构建的全部平台使用相同标识，每次发布使用新标识；CI 可使用 `<run-id>.<attempt>`。
标识遵循 SemVer 预发布规则。包内 `makaSource` 记录源码归档名、源码版本与 SHA-512，
用于溯源，不是签名构建证明。
仅本地未签名候选可省略 `--keys`。本机构建会验证版本和 V8 执行；仅跨平台打包必须指定 `--validator`，
许可证清单默认取自源码中的 Rust 依赖清单。
`Native CLI preview` workflow 用同一份冻结源码构建三个平台。
`node scripts/rust/publish-cli.mjs <产物目录>` 校验三者的共同来源；
附加 `--publish` 才会发布完整产物集到 `rust-preview`，CI 提供 npm provenance。
可用 `CARGO_TARGET_DIR` 保留构建缓存；`MAKA_JS_DEPS` 固定为解包源码自身的安装目录。

`node scripts/rust/pack-cli.mjs --target <目标> --version <精确版本>
--binary <目标平台maka> --validator <本机maka> --notices <已审阅许可证文档>
--output <目录>` 打包预构建原生程序，并用本机 CLI 校验实际 npm 归档。
不执行跨平台程序或安装脚本，不发布，也不覆盖已有输出。许可证文档须覆盖 Rust、V8
及嵌入 JavaScript；旧 Node CLI 文档不足以代替。此底层打包器本身不证明源码来源。

Rust 许可证检查沿用 [OpenDAL 的 cargo-deny 做法](https://github.com/apache/opendal/blob/main/scripts/dependencies.py)：
`deny.toml` 定义五个发布目标、许可证白名单和限定版本的 MPL 例外。
安装 cargo-deny 0.20.2 后运行 `node scripts/rust/dependencies.mjs check`；
依赖变更后将 `check` 换为 `generate`，审阅
[`DEPENDENCIES.rust.tsv`](../crates/cli/DEPENDENCIES.rust.tsv)。
清单包含构建依赖，排除仅用于测试的依赖；许可证策略检查也覆盖测试。
清单不是二进制许可证文本包。

ASF 投票制品是源码归档，许可审阅针对实际随包源码，包括根 `LICENSE` 和 `NOTICE`
记录的 Codex 补丁改编代码与 Deno telemetry 拷贝；lockfile 引用不等于打包代码。
npm 原生包是对应源码归档的便利构建，不是另一份源码发布；须保留来源版本和构建溯源。
当前源码审计不作二进制许可认证。

```sh
cargo fmt --all --check
cargo nextest run --locked --workspace -j 4
cargo test --locked --workspace --doc
cargo clippy --locked --workspace --all-targets -- -D warnings
node scripts/asf-license-headers.mjs check
```

单元测试放源码模块末尾，集成测试放 `tests/`。业务契约优先使用 struct／enum，
JSON Value 只用于真正开放的载荷和 schema。依赖 V8 的测试在各 crate 内共用一个
测试二进制，避免重复链接。普通测试使用本地 fixture；真实服务测试需要显式启用。
ignored 测试 `original_client_live_provider` 接受 `MAKA_LIVE_PROTOCOL`（`chat`、
`responses`、`messages`）、`MAKA_LIVE_BASE_URL`、`MAKA_LIVE_MODEL` 和
`MAKA_LIVE_API_KEY`，默认使用开发环境的 SGLang 端点。
独立 worktree 可将 `MAKA_JS_DEPS` 和 `NODE_PATH` 分别指向已有依赖的 checkout
及其 `node_modules`。
共享跨语言 fixture 放在根目录 `tests/fixtures`，通过 `tests/support/source.mjs` 加载当前 TypeScript 源码，不读取 workspace `dist`。

Grep 差分测试需要 PATH 中有 `rg`；runtime 本身不依赖该可执行文件。

## 当前限制

已实现项目管理、基本 Session／Turn 控制、模型配置、附件、文件工具、shell／PTY、Client Capability 工具、
模型流式交互与上下文压缩。Codex 订阅已接入执行；Copilot／xAI 推理适配和实测后置。

WorkHub 已支持受限对话、候选发现、交互式目标选择、向已有或新建会话委派、steering、停止、恢复和纠正。
控制操作追踪精确的委派 Message，不操作无关 Run。纠正先持久化意图，再退休旧关联，
原子提交替换、附件与排队消息；协调 Run 结束后仍可恢复完成，新目标不可用时持久化中止结果，
不会取消共享 Run。结构化记录保留原始创建选项与附件归属，候选查询提供最新有效关联。
等待确认或阻塞的任务仍可发现；发现不授予委派权限，准入时仍检查待决交互与未决副作用。
待决交互驱动共用 Session 目录及变更通知，WorkHub 的“需要你”列表与候选发现保持一致，解决交互后及时清除。

剩余功能、完整领域的内置插件迁移计划及 SDK／客户端接缝统一维护在
[功能等价与内置插件清单](rust-parity.zh-CN.md)，不在此重复列举。

Recall 是会话历史检索，不属于排除的 Memory 子系统。原生部署和更新能力不代表完整产品兼容。
插件平台支持静态链接 Rust 包、共享／独立 V8 的 JavaScript 包、作用域 Host 服务、外部 Executor，
以及 Desktop Slot／Remote stream。Graph／Swarm 与定时任务是内置插件。
Graph 实现类工作使用 Host 管理的 gix worktree，发布不可变补丁，不自动合并。
单次 Turn 编排跨 yield 和 resume 保留，不改变 Session 默认值；Swarm checkpoint 提供状态和最终结果 ID，
通过历史分页取回正文。接口与限制见[插件 SDK](../packages/plugin-sdk/README.zh-CN.md)。
OS 沙箱暂缓。Memory 留待单独重做，不移植旧实现，也不纳入本次重写。内容脱敏不实现。
