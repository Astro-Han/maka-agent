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

# Rust 功能等价与内置插件划分

[English](rust-parity.md)

对照基线：Rust `c6874775d`、main `f02ac9433`（2026-09-18）。
这里记录已知功能缺口和完整领域的内置插件迁移计划，不是完整验收报告。
下文的目标归属与 API 扩展不代表已经实现。

用我们自己的业务模块开发和验证插件 API：迁移一个已有领域，同时补齐它的缺失行为，
随后删除 Host 专用业务路径。给不变的 Host 业务逻辑套一个插件入口不算迁移完成。

## 边界

业务决策归插件，接受执行及保存执行事实归 Host。
需要持久化不等于必须放进核心：Graph 和 Scheduler 已经通过内置插件拥有业务状态与恢复逻辑。

- **Host：**规范日志、执行回执、Session 谱系、权限、凭据权威、计量、网络策略、进程／PTY 所有权，以及已接受操作的关闭与恢复。
- **插件：**业务流程、领域数据及迁移、外部协议适配、派生索引、报表和业务 UI；不能改写 Host 事实或伪造调用权限。
- **交界：**使用窄而类型化的 Host 服务，一份数据只有一个持久化所有者。Rust 与 JS 使用同等授权能力；领域可自管存储和迁移，所有插件都不能任意操作 Host 表。

内置 Rust 插件是静态链接、经现有 Fiber 生命周期激活的代码，不意味着新增进程、线程、V8 或独立 crate。
禁用撤下新能力准入，保留业务数据。Host 继续结算已接受的工作，业务负责重新启用后的协调；
依赖插件存活的 Executor 则留下真实中断结果。

现有客户端仍使用的协议，可以像 Scheduler 一样，由薄 Host 路由调用类型化插件 Contribution。
新插件业务接口使用 Remote。不要同时维护两套实现，也不要向插件内核的枚举不断添加业务 action。
允许修改客户端以删除过时路径，Graph 就是已有例子。

## 已有领域的迁移

| 领域／当前耦合 | 目标与完成条件 |
| --- | --- |
| **Skills：**`maka.skills` 拥有发现、输入准备、每步工具／上下文快照、治理、偏好 CAS、预览、导入及 workspace／user 发布；已发布的 Client Contribution 拥有 Session／新工作区选择器、管理页与草稿建议。 | Desktop 提供目标绑定的 Slot、通用 Remote 传输及授权的原生文件操作；旧扫描、导入、控制器和 Skills IPC／preload 门面已删除。Host 仅保留薄外部协议适配、准入与不可变回执，不再解析 Skill。 |
| **WorkHub：**`maka.workhub` 拥有协调会话配置、回答组合、原生 `workhub_tasks`、路由／选择／纠正／Stop／Resume、steering／followup 及恢复策略。Session behavior 为初始及后续 Turn 一致冻结工具与 Direct／Code Mode。 | 发布的 Client 在 `packages/workhub` 中拥有完整主窗口／浮动界面，通过 Remote 及原 Host 绑定的 Session／附件端口工作。Desktop 负责原生呈现，不编排任务；Host 负责受管 Session 原子创建、精确准入、规范回执和结算。Client 换代按原身份协调待确认提交，不跨 Host epoch 重投。 |
| **默认助手行为：**`maka.assistant` 发布默认 behavior、persona、个性化和工作区指令。 | 每逻辑模型步骤冻结来源；停用后不保留隐藏 persona。显式 Session／子任务指令独立于可替换的提示词 Contribution；执行／压缩不变量仍由 Host 维护。 |
| **Session 待办：**`maka.todo` 拥有 `todo_read`／`todo_write`、类型化文档和输入框实时 Client。 | 公共命名空间存储提供修订检查；Remote 按调用者 Session 分页推送完整快照。停用／重启保留数据，不保留 Todo 专用 Host 服务或 Desktop IPC。 |
| **Graph／Swarm：**已经是内置插件，behavior 按开放的类型化身份选择。Graph 使用公共授权 Session／执行命令、作用域数据和只读偏好，不接收 Host 私有句柄。 | 保持已有编排与唤醒行为；语义相同时复用窄命令，类型化领域 repository 可以保留。 |
| **Scheduler：**插件拥有计划、冻结触发、漏触发／重试策略和通知撤销；激活只接收公共存储、授权、执行与通知能力，不使用调度专用 Host 服务。 | Host 解析授权并准入执行／原生投递。暂停撤销尚未准入的通知，包括 provider 的迟到接受；已接受执行仍归 Host。恢复复用精确 Fire 身份，不重放结果不确定的通知。现有 Desktop 操作保留为插件的薄适配。 |
| **Web：**`maka.web` 拥有无浏览器 WebFetch、Tavily 搜索、来源选择、凭据验证与设置 Client；原生搜索通过公共 provider-tool 契约在每个模型步骤绑定。 | Host 负责授权 HTTP、代理、资源结算与命名空间凭据；Rust／JS 共用绑定，供应商结果和引用保留为规范事实。旧 Web RPC、全局设置和 Tavily 专用凭据槽已删除。 |
| **Recall：**`maka.recall` 拥有 Unicode 字面词匹配、BM25 排序、Session 多样性及 RecallMore 片段扩展。 | 公共 Rust／JS 历史 API 提供固定水位 UTF-8 分页和归档元数据；Host 拥有访问检查及 SQLx 文本投影，不提供 Recall 专用服务，不需要 V8。隐私模式撤下工具，来源缺失和片段截断明确报告。 |
| **Code Mode：**模式选择、嵌套派发及历史投影跨越多个 crate。 | 首批领域迁移后，在有实际 Contribution 边界收益时迁移面向用户的工具和模式策略；V8 所有权、嵌套调用权限、派发／结算及规范历史保留 runtime。不为搬迁 `exec` 发明万能执行 hook。 |
| **文件与 Shell 工具：**Host 基于现有文件／进程 owner 装配注册。 | 工具定义与装配可以成为内置 Contribution；资源所有权、写入协调和 PTY 取消仍归 Host。结构迁移等能消除具体耦合时再做，不为每个工具建插件。 |
| **Client Capability／MCP、provider 与传输** | 保持当前资源与权威边界；Desktop MCP 不搬进 Host，模型厂商不强制逐个插件化。功能缺口独立于结构迁移完成。 |

任何获准插件都可通过 `createRoot({ managed: true, ... })` 请求受管所有权。
Host 原子提交包／作用域所有者与创建身份；普通修改和其他插件不能绕过该所有权，重放不接管已有的无关 Session。

“完整领域”指单一业务实现和生命周期，不是搬走所有相关类型和表。
Host 标记的输入回执和执行事实是通用 runtime 契约；Skill 解释与 WorkHub 委派／纠正关系只归插件。
沿用 SQLx migrations 和领域存储；归属变化不要求改变磁盘格式、全部搬入 KV 或一个插件一个 crate。
runtime 契约不能反向依赖插件实现，协议适配层可以保留现有客户端词汇。

## 功能缺口与归属

“插件 + Host”表示明确分工，不表示任何一半可以留待以后。

| 领域 | 剩余功能 | 目标归属／必要边界 |
| --- | --- | --- |
| Plan | 状态与持久回执层完成：修订／放弃、版本与重规划来源检查、冻结提交、进度／中断／恢复／取消、精确重试及固定水位历史分页。工具、Behavior、Remote／Desktop 和实际执行观察尚未接入，非 Agent collaboration mode 仍拒绝准入。 | **插件 + Host。** 插件通过公共存储拥有流程与记录；审批计划不授予沙箱权限，Host 保留授权和 Turn 准入。没有 Host 回执只表示待准入，不能标记正在执行。 |
| Goal | 查询、arm、控制、续跑、终止、预算与恢复语义。 | **插件 + Host。** Goal 决定后续提交；Host 执行已准入的硬限制并记录用量。插件退休后不能继续提交。 |
| Deep research | 研究流程、进度查询、结果与恢复。 | **插件。** 复用 Graph／Swarm、Web、有界模型调用及可靠提交，不新增通用编排引擎。 |
| Daily review／recap | daily-review 查询／修改、定时复盘、`session.recap.generate`。 | **插件 + Host。** 选择、总结及输出由插件负责，复用 Scheduler 和授权历史／模型服务；规范 Session 元数据的提交仍归 Host。 |
| 外部 agent | setup start/query/cancel；具体执行适配、配置、鉴权、对话身份，以及附件／交互／resume／fork；Command Code GO 执行。 | **插件 + Host。** CLI／ACP 适配作为 Executor 插件，使用受管理进程／HTTP。已有 Executor 框架不等于已有具体 adapter。Host 负责授权、取消和外部事件落盘。 |
| Usage／Pricing | Usage 查询、一致版本视图及活动分页，Pricing 查询／修改和估价。 | **插件 + Host。** 报表、价格策略和可重建投影可归 Insights 领域；Host 不依赖插件存活来记录用量，并提供一致快照。缺失用量不能视为零。 |
| 后台健康 | BackgroundTaskHealth 的进程和端点检查。 | **插件 + Host。** 插件解释健康状态并提供工具；Host 提供授权资源观察和有界探测。保存 PID 不等于拥有进程。 |
| Session 谱系 | branch、revision create/abandon、regenerate；普通 resume 和启动恢复已有。 | **Host。** 谱系及工作区只有一个事务权威；插件可以请求命令，不能在私有存储中重做。 |
| Session 生命周期 | 删除／预览、shared 查询。 | **Host。** 协调引用、运行中工作、附件、托管 worktree、授权及清理。shared 查询依赖真实协作授权。 |
| Session 迁入迁出 | bundle 导入／导出；Codex、Claude Code、OpenCode 的统一外部 catalog/source/import。 | **插件 + Host。** 来源解析／发现可以作为 adapter；Host 拥有有界规范导入、身份、附件、来源记录和原子发布。原生 bundle 格式仍是 Host 合同，不能把外部事件字节直接作为可信执行权威。 |
| Runtime policy | shell／external-agent 消费，普通 named tool profiles。 | **拆分。** shell 启动策略和能力上限留在 Host；外部 agent 设置由对应领域消费。Profile 提供定义，Host 在准入和每步捕获时取能力交集。只保存设置不算完成。 |
| 接入／协作 | credential rotation prepare/revoke、principal revoke；collaboration access、邀请、grant revoke、principal rename/revoke；Turn-request create/query/decide/acknowledge/withdraw。 | **Host。** 复用凭据和持久化准入权威。插件可以提供流程／UI，但不能决定授权、绕过撤销或拥有规范的已接受 Turn request。 |
| Peer Mesh | create/query/invite/join/leave/remove/close/reconcile，rename/display-name、transit 控制。 | **本次重写保留 Host 实现。** 身份、路由与传输恢复必须在插件不可用时工作，不为补这些协议再造传输插件平台。 |
| 凭据导出 | `configuration.credentials.export`。 | **Host。** 从实际 vault 进行明确授权的导出；插件自己的凭据空间不授予整个 vault 的访问权。 |
| 模型 provider | Google／Cohere、其余声明的鉴权及 reasoning／usage／选项行为、运行中 models.dev 刷新；Copilot／xAI 推理及凭证实测。 | **先保留 Model／Host 层。** 共用传输、流式处理、重试、计量和请求快照；元数据来源策略以后可成为 Contribution，不强求一家厂商一个插件。Command Code CLI 归上面的 Executor。 |
| 诊断／托管执行 | `execution.inspect.query`、`host.resources.query`、`hosted.execution.start/cancel`。 | **Host**，呈现／编排可以由插件提供。检查读取规范证据；hosted execution 必须保证环境、所有权及取消，不能简单视为另一个 Executor 名称。 |

## 由真实消费者驱动的 API 工作

内核已有，不等于当前 SDK 能直接实现全部业务。旧 TS 插件不与新 SDK 源码兼容。

| 消费者／所需能力 | 现状／最小扩展 |
| --- | --- |
| 输入准备 | 原生类型化 Contribution 与 JS `ctx.input.prepare` 共用有序准备、来源回执和退休检查；原生 revision 支持非阻塞的准入／失效排序。队列编辑和 steering 在准入前准备，已接受输入的提升／重放不再扫描来源。 |
| Skills：工具发布 | Skill／SkillSearch 是普通 Contribution，不享有包名特权。每步绑定在工具上限内共同捕获 handler 和支持上下文，物理重试保持原快照。 |
| WorkHub：精确执行命令 | 类型化命令携带稳定操作 ID、精确目标及预期 revision。纠正先冻结插件意图，再精确控制／提交 Host 工作，最后原子记录业务回执；已接受工作不依赖插件可用性继续结算。队列编辑保留原提交凭证。插件不获得 SQL 事务回调或无限制执行句柄。 |
| WorkHub／Graph／Plan：可选择的 behavior | 已用开放的 `BehaviorId` 选择类型化 Contribution，Graph／Swarm 独立注册；非内置业务已通过 Host 验收。保留 Session 默认值和持久单 Turn 选择；请求的 behavior 不可用时明确失败。behavior 准备与输入准备是独立契约，不合并为 hook 总线。 |
| Skills／Web／Recall／Insights：授权服务 | Rust／JS 公共历史 API 提供固定水位文本分页及归档 Session 元数据。已准入 Agent 可读受信任 profile，Remote／后台调用保留相应范围的历史授权。用量查询仍需按消费者补契约。领域目录／修改接口可作为类型化插件 Service，不必成为内核方法。 |
| Skills／WorkHub／默认行为：业务 UI 与 Prompt 上下文 | 发布的 Client 通过 Slot 和 Remote 拥有真实 Skills 选择器／管理页及 WorkHub 界面。原生适配验证原 Host 与 document；连接换代撤销旧 Remote 租约，不重放调用。Prompt Contribution 拥有业务指令。功能停用明确显示不可用，不阻塞普通聊天。 |
| 其余 TS 扩展服务 | 新 SDK 尚缺等价的公开 LSP 路由、Commands、Skills／Goals 查询、shell 环境变量 Contribution、Settings 定义、授权流程及 LLM adapter 注册；问题／表单、源输入与附件复制已有公共契约；权限审批仍归 Host。能由插件服务实现的领域注册放在插件侧，敏感行为权威仍归 Host。`llm.generate` 不等于 adapter 注册。 |

这些是需要补齐或适配的功能接口，不是逐个复制 TS 方法。
TS 的 LLM adapter 注册服务于插件模型调用，本身不等于主 Session 的模型传输注册。
不要先造通用 provider 框架、事件总线或万能 repository。

内置 Rust 直接使用类型化调用，不绕行 JSON／V8。Rust 与 JS 适配共用能力语义、授权及退休保证；
新增跨语言能力契约有实际消费者时同步补公开 JS 绑定，不要求 Rust 领域 repository 一并暴露为 JS API。
不能以给内置插件无限 Host 访问权来推迟必要的 API 工作。

待确认提交由 Desktop document 按原 Host 和 Session 持有；解析器撤下界面或 Client 换代不会丢失原输入和 Stop 意图。
纠正在 Host 命令之前持久化冻结的业务意图。恢复读取精确回执，不以当前策略或默认值重新解释结果不确定的工作。

## 实现顺序

1. **公共 API 消费者已迁移：**Skills、默认助手、Scheduler、Graph、WorkHub 与外部插件使用同等受限契约；新增消费者时维持 Rust／JS 对等。
2. **外部验收：**JS workflow fixture 覆盖 UI 授权、持久后台工作、精确回执、停用／恢复以及跨 Host 重启的授权撤销。
3. **缺失业务领域：**完成 Plan／Goal 和研究／复盘；完成外部 adapter、Insights／健康。复用领域边界，不先在 Host 写新业务再搬一次。
4. **其余核心等价：**完成 Session 生命周期／谱系／迁入迁出、policy、接入／协作、Peer Mesh、provider 和诊断。前面消费者所需的核心命令前置到对应阶段，核心工作不等待全部插件或商店。首批领域验证边界后评估 Code Mode／工具装配迁移，不将其作为功能等价的前提。

每个领域按“真实消费者及不变量 → 最小类型化 API 与消费者一起实现 → 验证生命周期和失败行为 → 删除旧 Host 业务路径”推进。
确实共享契约时用第二个已有消费者验证，不虚构业务来证明抽象。契约变化在同一切片更新 SDK。
若 API 需要任意 Host 访问、第二份权威或大量业务例外，应重划边界。

一个领域必须覆盖操作、错误区分、授权、取消、丢回复、重启恢复及实际 Desktop 消费，才能标为完成。
保留少量包含退休／重新启用的端到端验收；注册了工具或 schema 测试通过都不等于完成。

首批迁移还必须证明：

- Skills 停用时普通聊天可用，新显式 Skill 请求明确失败；已接受的内容及回执跨文件修改、更新、退休和重启保持不变，不重扫 Skills；pending 提升仍检查当前权限。准备与退休并发不能准入过期工作。
- Skill 发布检测本地修改、处理提交结果未知，并完整恢复 bytes／lock／baseline；发现、工具和 UI 使用同一领域 revision。
- WorkHub 丢回复或纠正中重启不重复投递、不误操作后来无关的工作；停用停止新编排，Host 继续结算已接受操作，重新启用先协调回执。
- 真实 Desktop 通过已发布插件路由调用并拒绝旧代请求；源码检查确认 Host 不再扫描 Skills、决定 WorkHub 策略或提供重复默认 persona。

搬迁或扩展既有高价值测试，不保留重复的新旧套件。保留跨平台权限、文件系统和 PTY 验证，不能借迁移削弱覆盖。

## 已有能力与排除项

WorkHub、普通 resume、文件／shell／PTY、含 Desktop MCP 的 Client Capability、模型流式交互、
压缩与动态工具加载已实现。插件内核、Rust／JS 加载、共享／独立插件 V8、作用域存储／凭据／
文件／HTTP／进程／PTY／模型／客户端调用、Executor、Client Remote，以及 Graph／Swarm 和 Scheduler 已接线。
旧 `agent.graph.*` RPC 已被插件路由替代，不再重建第二套 Graph。

Memory、脱敏排除；OS 沙箱后置。Copilot／xAI 凭证实测按既有约定后置。
部署交付不等于完整产品兼容：原生 CLI 运维命令不替代 TS 交互式 CLI／ACP 产品面，
也不会自动迁移 TS state root。这些产品／数据迁移决策不能藏在插件内。
不要求把 TS 客户端／TUI 一并改成 Rust，但需要验收其与原生 Host 的兼容性。
旧 state root 的迁移是单独的范围决策，不能从功能等价任务中自动推导授权。

## 依据

- [Host 注册](../crates/runtime-host/src/server/operations.rs)、[dispatch](../crates/runtime-host/src/server/dispatch.rs)、[协议词汇](../crates/protocol/src/operation.rs)。
- [执行准备](../crates/runtime-host/src/execution/prepare/environment.rs)、[工具装配](../crates/runtime-host/src/execution/tools.rs)、[设置消费](../crates/runtime-host/src/server/configuration/policy.rs)、[provider 路由](../crates/runtime-host/src/provider_route.rs)。
- [Skills 领域](../crates/skills/src/lib.rs)、[输入准备](../crates/runtime-host/src/execution/input/prepared.rs)、[WorkHub 流程](../crates/workhub/src/control.rs)、[Host 命令](../crates/runtime-host/src/execution/plugins.rs)、[业务事务](../crates/workhub/src/repository.rs)、[默认 Prompt](../crates/assistant/src/prompt.rs)、[Graph 接线](../crates/runtime-host/src/plugins/graph.rs)。
- [SDK](../packages/plugin-sdk/README.zh-CN.md)、[执行服务](../crates/plugins/src/execution.rs)、[Session behavior](../crates/plugins/src/session.rs)、[Scheduler 路由](../crates/runtime-host/src/server/scheduler.rs)。
- TS [composition](../packages/runtime-host/src/server/execution-composition.ts)、[交互工具](../packages/runtime-host/src/server/interactive-run-composer.ts)、[执行检查](../packages/runtime-host/src/server/execution-inspect-coordinator.ts)、[外部导入](architecture/external-session-import-design.zh-CN.md)。
