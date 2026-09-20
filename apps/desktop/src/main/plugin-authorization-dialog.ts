/*
 * Licensed to the Apache Software Foundation (ASF) under one
 * or more contributor license agreements.  See the NOTICE file
 * distributed with this work for additional information
 * regarding copyright ownership.  The ASF licenses this file
 * to you under the Apache License, Version 2.0 (the
 * "License"); you may not use this file except in compliance
 * with the License.  You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing,
 * software distributed under the License is distributed on an
 * "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
 * KIND, either express or implied.  See the License for the
 * specific language governing permissions and limitations
 * under the License.
 */

import type { AuthorizationCapability } from '@maka-agent/plugin-sdk/client';
import type { PluginAuthorizationInput } from '@maka/runtime-host/protocol';
import type { MessageBoxOptions } from 'electron';

/** Application-owned copy; plugins supply a proposal, never the confirmation UI. */
export function pluginAuthorizationDialog(input: PluginAuthorizationInput, locale: string, signal: AbortSignal): MessageBoxOptions {
  if (input.command.kind !== 'approve') throw new Error('Consent requires a proposal');
  const zh = locale.startsWith('zh');
  const labels: Record<AuthorizationCapability, readonly [string,string]> = {
    read_files: ['Read workspace files', '读取工作区文件'],
    write_files: ['Modify workspace files', '修改工作区文件'],
    network: ['Make network requests', '发送网络请求'],
    models: ['Call models (may incur usage costs)', '调用模型（可能产生费用）'],
    processes: ['Run programs and terminals', '运行程序与终端'],
    client_capabilities: ['Invoke connected client capabilities', '调用已连接客户端的能力'],
    executions: ['Create, submit, inspect and control executions', '创建、提交、查询与控制执行'],
    notifications: ['Send notifications', '发送通知'],
  };
  const {request} = input.command;
  const target = request.target.kind === 'profile' ? 'profile' : request.target.kind === 'session'
    ? `${request.target.sessionId} (${zh ? '当前工作区与权限' : 'current workspace and permissions'})`
    : `${request.target.workspace.kind === 'project' ? request.target.workspace.projectId : request.target.workspace.path} (${request.target.permissionMode})`;
  return {
    type: 'warning', title: zh ? '插件后台授权' : 'Plugin background authorization',
    message: `${input.client.extensionId}\n${zh ? '允许此插件在后台工作？' : 'Allow this plugin to work in the background?'}`,
    detail: [request.title, `${zh ? '范围' : 'Scope'}: ${input.scope}`, `${zh ? '目标' : 'Target'}: ${target}`,
      request.capabilities.map((capability) => labels[capability][zh ? 1 : 0]).join('\n'),
      zh ? '授权在重启后仍有效，直到撤销。目标权限变化会使授权失效。' : 'Authorization survives restart until revoked. Changes to the target permissions invalidate it.',
    ].join('\n\n'),
    buttons: zh ? ['取消','允许'] : ['Cancel','Allow'], cancelId:0, defaultId:0, signal,
  };
}
