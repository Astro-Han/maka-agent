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

import type { ClientContext, ClientWorkspace } from '@maka-agent/plugin-sdk/client';
import { authorizeUser } from './files.js';

export type Page = { revision: string; cursor: string };
export type View = 'governance' | 'bundled' | 'managed_sources';
export type Skill = {
  kind: 'skill' | 'discovery_diagnostic';
  ref: string;
  id: string;
  name: string;
  description: string;
  enabled: boolean;
  pinned: boolean;
  manageable: boolean;
  validationStatus: string;
  managedUpdateStatus: string | null;
  userModified: boolean;
};
export type Source = {
  kind: 'bundled' | 'managed_source';
  id: string;
  name: string;
  description: string;
  installed: boolean;
};
export type Catalog = {
  userRecovery: string | null;
  kind: 'page';
  view: View;
  revision: string;
  items: (Skill | Source)[];
  nextCursor: string | null;
};
export type Invocable = {
  kind: 'page';
  revision: string;
  items: { ref: string; id: string; name: string; description: string }[];
  nextCursor: string | null;
};
export type Conflict = {
  kind: 'revision_changed' | 'revision_conflict';
  expectedRevision: string;
  actualRevision: string;
};
export type Rejected = { kind: 'rejected'; reason: string };
export type Mutation =
  | { kind: 'create_starter' }
  | { kind: 'install'; sourceType: 'bundled' | 'managed'; sourceId: string }
  | { kind: 'delete'; ref: string }
  | { kind: 'set_enabled'; ref: string; enabled: boolean }
  | { kind: 'set_pinned'; ref: string; pinned: boolean }
  | {
      kind: 'update_managed';
      ref: string;
      force: boolean;
      expectedCurrentSha256: string | null;
      expectedSourceSha256: string | null;
    };
export type Preview = {
  kind: 'preview';
  revision: string;
  currentSnippet: string;
  sourceSnippet: string;
  currentTruncated: boolean;
  sourceTruncated: boolean;
  expectedCurrentSha256: string;
  expectedSourceSha256: string;
};
export type Request =
  | { kind: 'resolve_path'; ref: string; target: 'file' | 'directory' }
  | { kind: 'invocable'; page: Page | null }
  | { kind: 'catalog'; view: View; page: Page | null }
  | { kind: 'mutate'; expectedRevision: string; mutation: Mutation; grant?: string }
  | { kind: 'preview'; expectedRevision: string; ref: string };
export type Reply =
  | { kind: 'resolved'; path: string; target: 'file' | 'directory' }
  | Catalog
  | Invocable
  | Conflict
  | Rejected
  | Preview
  | { kind: 'committed' | 'unchanged'; revision: string };
export type Target =
  | { kind: 'session'; sessionId: string }
  | ({ kind: 'workspace' } & ClientWorkspace);
export type Call = (input: Request) => Promise<Reply>;
export function request(context: ClientContext, target: Target): Call {
  const ordinary = workspaceRequest(context, target);
  return async (request) => {
    if (
      request.kind !== 'mutate' ||
      request.mutation.kind !== 'delete' ||
      !request.mutation.ref.startsWith('user:')
    )
      return ordinary(request);
    const grant = await authorizeUser(context);
    const workspace =
      target.kind === 'workspace'
        ? {
            workspace: target.workspace,
            permissionMode: target.permissionMode,
            collaborationMode: target.collaborationMode,
          }
        : null;
    return context.remote.method<{ workspace: typeof workspace; request: Request }, Reply>(
      'user-request',
      target.kind === 'session' ? target.sessionId : undefined,
    )({ workspace, request: { ...request, grant } });
  };
}
function workspaceRequest(context: ClientContext, target: Target): Call {
  if (target.kind === 'session')
    return context.remote.method<Request, Reply>('request', target.sessionId);
  const { workspace, permissionMode, collaborationMode } = target;
  if (workspace.kind === 'project') {
    const call = context.remote.method<
      {
        projectId: string;
        permissionMode: ClientWorkspace['permissionMode'];
        collaborationMode: ClientWorkspace['collaborationMode'];
        request: Request;
      },
      Reply
    >('project-request');
    return (request) =>
      call({ projectId: workspace.projectId, permissionMode, collaborationMode, request });
  }
  const call = context.remote.method<
    {
      path: string;
      permissionMode: ClientWorkspace['permissionMode'];
      collaborationMode: ClientWorkspace['collaborationMode'];
      request: Request;
    },
    Reply
  >('path-request');
  return (request) => call({ path: workspace.path, permissionMode, collaborationMode, request });
}
export const copy = {
  en: {
    title: 'Skills',
    available: 'Use a Skill',
    manage: 'Manage',
    import: 'Import source',
    openFile: 'Open file',
    openFolder: 'Open folder',
    installed: 'Installed',
    bundled: 'Bundled',
    sources: 'Local sources',
    refresh: 'Refresh',
    recover: 'Recover user library',
    next: 'Next page',
    add: 'Create starter',
    install: 'Install',
    remove: 'Delete',
    enable: 'Enable',
    disable: 'Disable',
    pin: 'Pin',
    unpin: 'Unpin',
    preview: 'Preview update',
    current: 'Current',
    source: 'Incoming',
    apply: 'Apply this update',
    cancel: 'Cancel',
    confirm: 'Delete this Skill?',
    empty: 'No Skills',
    failed: 'Operation failed. Refresh to retry.',
    changed: 'Sources changed. Refresh before continuing.',
    truncated: 'Preview shortened; the update uses the full files.',
    filter: 'Filter Skills',
  },
  'zh-CN': {
    title: 'Skills',
    available: '使用技能',
    manage: '管理',
    import: '导入来源',
    openFile: '打开文件',
    openFolder: '打开目录',
    installed: '已安装',
    bundled: '内置来源',
    sources: '本地来源',
    refresh: '刷新',
    recover: '恢复用户技能目录',
    next: '下一页',
    add: '创建入门技能',
    install: '安装',
    remove: '删除',
    enable: '启用',
    disable: '停用',
    pin: '置顶',
    unpin: '取消置顶',
    preview: '预览更新',
    current: '当前内容',
    source: '新内容',
    apply: '应用此更新',
    cancel: '取消',
    confirm: '删除此技能？',
    empty: '没有技能',
    failed: '操作失败，请刷新后重试。',
    changed: '来源已变化，请刷新后继续。',
    truncated: '预览已缩短，更新将使用完整文件。',
    filter: '筛选技能',
  },
} as const;
export type Copy = (typeof copy)[keyof typeof copy];
