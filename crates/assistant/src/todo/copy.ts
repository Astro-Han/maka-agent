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
  statuses: { pending: 'Pending', in_progress: 'In progress', completed: 'Completed' },
  retry: 'Retry',
  title: 'Checklist',
  description: 'Model-reported progress, not verified execution evidence.',
};

export const copy = {
  en,
  'zh-CN': {
    statuses: { pending: '待处理', in_progress: '进行中', completed: '已完成' },
    retry: '重试',
    title: '待办',
    description: '状态由模型报告，不代表执行结果已验证。',
  },
  'zh-TW': {
    statuses: { pending: '待處理', in_progress: '進行中', completed: '已完成' },
    retry: '重試',
    title: '待辦清單',
    description: '狀態由模型回報，不代表執行結果已驗證。',
  },
} satisfies Record<ClientLocale, typeof en>;
