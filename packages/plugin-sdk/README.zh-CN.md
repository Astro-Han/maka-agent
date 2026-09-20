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

# Maka 插件 SDK

[English](README.md)

供可信 Rust Host 插件使用的 TypeScript 合同。Host SDK API **1** 独立于 Maka 应用版本；此 workspace 尚未发布。

```ts
import type { HostPlugin } from '@maka-agent/plugin-sdk/host';

const activate: HostPlugin = async (ctx) => {
  await ctx.tools.register<{ text: string }>(
    {
      name: 'Echo',
      description: '返回传入的文本。',
      inputSchema: {
        type: 'object',
        properties: { text: { type: 'string' } },
        required: ['text'],
        additionalProperties: false,
      },
    },
    ({ text }) => ({ text }),
  );
};
export default activate;
```

打包为不含 import 和顶层 await 的单个 ESM 入口。在 `maka.extension.json` 中声明 `runtime: { entry: "index.mjs", sdkVersion: 1, vm: "shared" }`；`dedicated` 为该包当前加载代申请独立 VM。

Prompt 回调接收类型化的 Session 或模型步骤上下文，不伪造工具调用权限。section 和动态 context 默认解析模板；已解析内容或用户文本使用 `format: 'plain'`。一个 `complete` section 替换其它提示词 Contribution，不删除显式 Session／子任务指令。物理重试复用同一份冻结组合。

- 激活阶段暂存注册；通过 `ctx.run` 在发布生效后启动业务循环。用 `ctx.effect` 注册清理，观察 `ctx.signal`。
- Tool 和 Executor 回调获得绑定调用身份的服务与进程能力。旧调用句柄会失效；实例级进程需通过下一次调用的 `processes.open(id)` 重新绑定，卸载时由 Host 清理。
- 启动进程要求当前调用仍有 Bypass 权限，使用冻结的工作目录，以及绝对可执行路径和 argv。默认随调用结束。stdin 字符串按 UTF-8 编码；输出用 `TextDecoder` 增量解码。
- `call.terminals` 使用同样的命令与生命周期约定，提供原生 PTY、串行输入／尺寸变更回执、带明确 reset 事件的有界输出，以及持久退出和清理结果。后续调用需重新打开实例级终端；卸载会关闭它们。终端解析复用 Host 的共享 VM，不按 PTY 分配。
- `call.http.request` 提供绑定调用身份的 HTTP，使用 Host 代理配置并要求 Bypass 权限。通过 `response.next()` 分块读取字节；`null` 表示完整结束，截断则报错。调用结束或插件卸载时关闭响应。不自动重试或重定向，远端副作用的恢复由插件负责。
- Session 范围的执行命令使用稳定 operation ID：相同内容重试返回原收据，内容变化则冲突。profile Entry 不会自动获得 Session 权限。
- `executions.submit({ orchestrationMode })` 仅选择该次执行的模式，不改变 Session 默认值。`query()` 的 `attentionId` 标识当前阻塞交互集合或交接暂停，不随无关日志写入变化。
- `call.clients.tools()` 仅列出调用准入时冻结的客户端工具；`call.clients.call({ name, input })` 复用 Host 权限、审批／表单、取消和持久化结算。Model 与 Executor 遵守同一边界，后续发布能力或放宽权限不会扩张它。
- 存储按包和范围隔离，提供 CAS revision 与原子批次。删除保留 revision；业务迁移由插件负责。
- `ctx.credentials` 将按包和范围隔离的秘密写入 Host 私有凭据库，不进入普通存储或执行历史。写入比较 revision，删除保留墓碑。每个值最多 64 KiB，每个命名空间最多 256 个稳定键。沿用现有 vault 的文件权限／ACL 保护，不另加一层加密。
- 仅安装编解码和 URL 全局对象。文件、网络、计时器和进程不是环境自带的 Node API，应使用 Host SDK 服务；不承诺恶意代码隔离。
- `call.files` 提供类型化的 read/write/edit/glob/grep/patch，复用 Host 工具与持久化结算。权限不超过调用准入时及当前 Session 的权限和工具上限。读取返回有界分页或已持久化的图片引用；搜索结果标明完整性。丢弃 Promise 不会丢弃文件操作的收尾责任，跨 Service 转发也保留这一约束。
- `call.llm.generate` 使用调用准入时冻结的模型及 Host 的代理/OAuth，与主模型共享执行器。只发送显式 prompt/system，不携带工具或父对话。默认输出预算 2048 token；输入上限 256 KiB，响应流上限 2 MiB。结果及供应商报告的用量落盘后才返回；缺失用量保持未知。未等待的调用也随 invocation 取消并完成清理。

`npm --workspace @maka-agent/plugin-sdk run typecheck` 同时检查 Rust Host 集成测试实际执行的插件 fixture。

`ctx.preferences.read()` 返回带 revision 的个性化设置及工作区指令开关，不暴露凭据或完整 Host 配置。激活期间即可读取，随插件退休失效；它不授予资源或执行权限。

`ctx.executions.createChild({ ..., workspace: 'isolated_git' })` 为子 Session 绑定 Host 管理的 linked worktree。父会话必须允许写入，且工作目录是干净仓库的根目录。重试与 Host 重启保留子任务改动。执行及工作区写入者结束后，`workspacePatch(operationId)` 发布相对于初始提交的不可变 Git patch artifact，包含已提交与未提交改动，不自动合并到父目录。需在子会话进入下一 Turn 前导出。工作区保留用于恢复，不随插件禁用而删除。稀疏检出、子模块、外部 Git filter 和超过 50 MiB 的补丁会明确报错。Host 的 Git 操作使用 gix，不依赖系统 Git 可执行文件。

输入准备使用 `ctx.input.prepare(name, callback)`，返回不变、附回执的准备文本或明确拒绝。此时没有 invocation 权限，不能替换附件或已有回执；Host 标注来源，已接受输入在重放时不重新准备。回调应无副作用；可变来源更新时关闭并重新注册，阻止旧准备结果继续准入。

## Client SDK

Client SDK API **1** 使用 Desktop 提供的 React。导出来自 `@maka-agent/plugin-sdk/client` 的 `ClientPlugin`；其 `activate(ctx, config)` 暂存带 key 的 Slot 注册和 Effect。业务启动放在 `ctx.effect`，返回清理函数并观察 `ctx.signal`。初始化结束后关闭注册；清理失败时，该 Entry 必须等待页面重载，不能自动重新激活。

用 `@maka-agent/plugin-sdk/build` 的 `buildClient({ packageId, entryPoint })` 构建（作者的构建环境需安装 esbuild）。保存返回的 JavaScript，并在 manifest 中声明 `client: { entry: "client.js", sdkVersion: 1 }`。加载器在执行前校验字节和 SDK 版本。插件共享可信 Renderer，不是沙箱，也不提供 Node 兼容层。

Slot 包括 `session.composer.before`、`workspace.composer.before` 和 `workspace.manage`。工作区参数只是候选目标，不是路径授权。Composer Slot 提供只编辑草稿的 `appendText` 和 `publishSuggestions`。发布对象提供 `update(items)` 和 `dispose()`：刷新时更新同一 owner，effect 清理时销毁。条目身份跨刷新稳定；建议随发布者或目标退出而撤下，不提交消息。每个注册拥有 Entry 内唯一 key 和可选数值排序。包导入需列入 manifest dependencies；React、`react/jsx-runtime` 和 Client SDK 由 Desktop 提供，不要重复打包 React。

可选的 `ctx.localFiles.pick()` / `open(path)` 仅处理 Desktop 本地路径，不用于远程 Host 文件。Desktop 在原生操作前校验 Client 发布身份，导航或退休后返回的文件选择结果会被丢弃。

Desktop 通过 `@maka/ui/plugin` 提供共享 UI 模块（目前为 `Button`）。使用该入口支持的组件，不再打包一份组件库实例；它不暴露内部 UI 包的完整 API。

Host 插件通过 `ctx.remote.method(name, callback)` 或 `ctx.remote.stream(name, open)` 发布接口。Client 插件通过 `ctx.remote.method<Input, Output>(name, sessionId?)` 获取调用函数，或通过 `ctx.remote.stream<Input, Output>(name, sessionId?)` 获取异步迭代器工厂。UI 发布后才能调用；句柄固定到原 Host 连接和后端注册，不随替换重定向。退出迭代会关闭流，UI 卸载或页面导航会关闭所属文档。Remote 调用不是 Agent 调用，不隐含进程权限。

Remote 回调可以抛出携带 `RemoteFailure.code` 的 `Error`。`outcome_unknown` 保留业务结果不确定的语义，需要领域恢复，不能盲目重试；它不会隔离已正常结算的插件。资源清理未确认时由 Host 独立隔离。未分类异常映射为 `operation_unavailable`。

接受调用者 Host 路径的 Rust endpoint 声明 `Endpoint::requiring_host_paths()`。Host 在绑定和调用时都检查路径授权，借用其他连接的注册目标也不能绕过。项目 ID 和已有 Session 查询不要求原始路径权限；插件通过显式注入的只读视图访问它们。
