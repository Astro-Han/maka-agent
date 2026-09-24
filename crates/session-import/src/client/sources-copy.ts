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
  title: 'Manage import sources',
  description: 'Paths belong to this Host, not the desktop client. Sources are read only.',
  remove: 'Remove source',
  format: 'Format',
  name: 'Name',
  database: 'Absolute Host database path',
  root: 'Host source root',
  add: 'Add source',
};
export const copy = {
  en,
  'zh-CN': {
    title: '管理导入来源',
    description: '路径属于当前 Host，不是桌面客户端。来源仅用于只读导入。',
    remove: '移除来源',
    format: '格式',
    name: '名称',
    database: 'Host 数据库绝对路径',
    root: 'Host 来源根目录',
    add: '添加来源',
  },
  'zh-TW': {
    title: '管理匯入來源',
    description: '路徑屬於目前 Host，不是桌面用戶端。來源僅用於唯讀匯入。',
    remove: '移除來源',
    format: '格式',
    name: '名稱',
    database: 'Host 資料庫絕對路徑',
    root: 'Host 來源根目錄',
    add: '新增來源',
  },
} satisfies Record<ClientLocale, typeof en>;
