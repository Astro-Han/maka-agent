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
  title: 'Import history',
  refresh: 'Refresh',
  empty: 'No imports yet',
  records: 'historical records',
  awaiting: 'Awaiting delivery or receipt reconciliation',
  reconcile: 'Continue / reconcile',
  abandon: 'Abandon import',
  published: 'Published',
  open: 'Open Session',
  abandoned: 'Abandoned',
  next: 'Next page',
};
export const copy = {
  en,
  'zh-CN': {
    title: '导入记录',
    refresh: '刷新',
    empty: '尚无导入记录',
    records: '条历史记录',
    awaiting: '等待交付或确认回执',
    reconcile: '继续／确认',
    abandon: '放弃导入',
    published: '已发布',
    open: '打开会话',
    abandoned: '已放弃',
    next: '下一页',
  },
  'zh-TW': {
    title: '匯入記錄',
    refresh: '重新整理',
    empty: '尚無匯入記錄',
    records: '筆歷史記錄',
    awaiting: '等待交付或確認回執',
    reconcile: '繼續／確認',
    abandon: '放棄匯入',
    published: '已發布',
    open: '開啟對話',
    abandoned: '已放棄',
    next: '下一頁',
  },
} satisfies Record<ClientLocale, typeof en>;
