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

import type { ClientLocale } from '@maka-agent/plugin-sdk/client';

const en = {
  title: 'Import external conversations',
  refresh: 'Refresh state',
  description:
    'Create an independent copy of historical observations. Tools are not replayed and source permissions are not inherited.',
  working: 'Working… Leaving this page does not undo work accepted by the Host.',
  source: 'Source',
  chooseSource: 'Choose a source',
  search: 'Search',
  workspaceFilter: 'Source workspace filter',
  includeArchived: 'Include archived',
  readCatalog: 'Read catalog',
  chooseConversation: 'Choose a conversation',
  noConversations: 'No matching conversations',
  unknownTime: 'Unknown time',
  nextPage: 'Next page',
  executionSettings: 'Execution settings for the new Session',
  destinationWorkspace: 'Destination Host workspace',
  findModel: 'Find a model',
  searchModels: 'Search models',
  incompleteModels: 'Results are incomplete. Refine the model search.',
  model: 'Model',
  chooseModel: 'Choose a model',
  sandbox: 'Sandbox',
  readOnly: 'Read only',
  workspaceWrite: 'Workspace write',
  bypass: 'Bypass sandbox',
  retry: 'Retry this import',
  create: 'Create imported copy',
  savedImports: 'View saved imports',
  retryDescription:
    'Retry keeps the same operation. To change the input, set this attempt aside. This does not undo an import accepted by the Host; starting again creates another copy.',
  setAside: 'Set aside this attempt',
};

export const copy = {
  en,
  'zh-CN': {
    title: '导入外部会话',
    refresh: '刷新状态',
    description: '创建独立副本，保留历史观察；不会重放工具，也不会继承来源的执行权限。',
    working: '正在处理…关闭页面不会撤销 Host 已接受的工作。',
    source: '来源',
    chooseSource: '请选择来源',
    search: '搜索',
    workspaceFilter: '来源工作目录筛选',
    includeArchived: '包括归档',
    readCatalog: '读取目录',
    chooseConversation: '选择会话',
    noConversations: '没有匹配会话',
    unknownTime: '时间未知',
    nextPage: '下一页',
    executionSettings: '新会话的执行配置',
    destinationWorkspace: '目标 Host 工作目录',
    findModel: '搜索模型',
    searchModels: '查询模型',
    incompleteModels: '结果不完整，请缩小模型搜索范围。',
    model: '模型',
    chooseModel: '请选择模型',
    sandbox: '沙箱',
    readOnly: '只读',
    workspaceWrite: '工作区可写',
    bypass: '完全绕过沙箱',
    retry: '重试同一次导入',
    create: '创建导入副本',
    savedImports: '查看持久记录',
    retryDescription:
      '重试会沿用同一操作。若需更改输入，可放下这次尝试；这不会撤销 Host 已接受的导入，之后新建会产生另一份副本。',
    setAside: '放下这次尝试',
  },
  'zh-TW': {
    title: '匯入外部對話',
    refresh: '重新整理狀態',
    description: '建立獨立副本，保留歷史觀察；不會重播工具，也不會繼承來源的執行權限。',
    working: '正在處理…關閉頁面不會撤銷 Host 已接受的工作。',
    source: '來源',
    chooseSource: '請選擇來源',
    search: '搜尋',
    workspaceFilter: '來源工作目錄篩選',
    includeArchived: '包括封存',
    readCatalog: '讀取目錄',
    chooseConversation: '選擇對話',
    noConversations: '沒有符合的對話',
    unknownTime: '時間未知',
    nextPage: '下一頁',
    executionSettings: '新對話的執行設定',
    destinationWorkspace: '目標 Host 工作目錄',
    findModel: '搜尋模型',
    searchModels: '查詢模型',
    incompleteModels: '結果不完整，請縮小模型搜尋範圍。',
    model: '模型',
    chooseModel: '請選擇模型',
    sandbox: '沙箱',
    readOnly: '唯讀',
    workspaceWrite: '工作區可寫入',
    bypass: '完全略過沙箱',
    retry: '重試同一次匯入',
    create: '建立匯入副本',
    savedImports: '檢視持久記錄',
    retryDescription:
      '重試會沿用同一操作。若需變更輸入，可擱置這次嘗試；這不會撤銷 Host 已接受的匯入，之後新增會產生另一份副本。',
    setAside: '擱置這次嘗試',
  },
} satisfies Record<ClientLocale, typeof en>;
