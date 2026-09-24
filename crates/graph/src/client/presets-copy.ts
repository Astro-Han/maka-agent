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
  add: 'Add agent preset',
  reload: 'Reload',
  save: 'Save',
  remove: 'Remove',
  cancel: 'Cancel',
  id: 'ID',
  name: 'Name',
  description: 'Instructions for choosing this preset',
  connection: 'Connection slug',
  model: 'Model',
  profile: 'Capabilities',
  thinking: 'Reasoning effort',
  enabled: 'Enabled',
  inherited: 'Default',
  empty: 'No presets. General-purpose agents remain available.',
  loading: 'Loading…',
  local_read: 'Read-only repository',
  web_research: 'Web research',
  implementation: 'Isolated implementation',
};
export const copy = {
  en,
  'zh-CN': {
    add: '添加 Agent 预设',
    reload: '重新加载',
    save: '保存',
    remove: '删除',
    cancel: '取消',
    id: '标识',
    name: '名称',
    description: '选择此预设的说明',
    connection: '连接标识',
    model: '模型',
    profile: '能力',
    thinking: '思考程度',
    enabled: '启用',
    inherited: '默认',
    empty: '尚无预设，仍可使用通用 Agent。',
    loading: '加载中…',
    local_read: '只读代码库',
    web_research: '网络调研',
    implementation: '独立工作区实现',
  },
  'zh-TW': {
    add: '新增 Agent 預設',
    reload: '重新載入',
    save: '儲存',
    remove: '刪除',
    cancel: '取消',
    id: '識別碼',
    name: '名稱',
    description: '選擇此預設的說明',
    connection: '連線識別碼',
    model: '模型',
    profile: '能力',
    thinking: '思考程度',
    enabled: '啟用',
    inherited: '預設',
    empty: '尚無預設，仍可使用通用 Agent。',
    loading: '載入中…',
    local_read: '唯讀程式碼庫',
    web_research: '網路研究',
    implementation: '獨立工作區實作',
  },
} satisfies Record<ClientLocale, typeof en>;
