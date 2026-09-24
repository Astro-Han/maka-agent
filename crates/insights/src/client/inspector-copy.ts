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
  title: 'Session usage',
  refresh: 'Refresh',
  refreshFailed: 'Refresh failed; showing the last successful snapshot.',
  loading: 'Loading usage…',
};

export const copy = {
  en,
  'zh-CN': {
    title: '会话用量',
    refresh: '刷新',
    refreshFailed: '刷新失败；以下是上次成功读取的快照。',
    loading: '正在读取用量…',
  },
  'zh-TW': {
    title: '對話用量',
    refresh: '重新整理',
    refreshFailed: '重新整理失敗；以下是上次成功讀取的快照。',
    loading: '正在讀取用量…',
  },
} satisfies Record<ClientLocale, typeof en>;
