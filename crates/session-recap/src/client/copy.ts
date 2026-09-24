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
  title: 'Session recap',
  description: 'Summarize this conversation in one sentence using its selected model.',
  unconfirmed:
    'The result is unconfirmed. Check the original request first; generating again may incur another model charge.',
  failed: 'A complete recap could not be generated. Check the session model and try again.',
  checkOriginal: 'Check original request',
  working: 'Working…',
  generate: 'Generate new recap',
};

export const copy = {
  en,
  'zh-CN': {
    title: '任务回顾',
    description: '根据会话历史生成一句回顾，使用本会话选定的模型。',
    unconfirmed: '结果尚未确认，请先查询原请求。生成新回顾可能产生额外模型费用。',
    failed: '未能生成完整回顾，请检查会话模型后重试。',
    checkOriginal: '查询原请求',
    working: '处理中…',
    generate: '生成新回顾',
  },
  'zh-TW': {
    title: '任務回顧',
    description: '依據對話記錄產生一句回顧，使用本對話選定的模型。',
    unconfirmed: '結果尚未確認，請先查詢原請求。產生新回顧可能增加模型費用。',
    failed: '未能產生完整回顧，請檢查對話模型後重試。',
    checkOriginal: '查詢原請求',
    working: '處理中…',
    generate: '產生新回顧',
  },
} satisfies Record<ClientLocale, typeof en>;
