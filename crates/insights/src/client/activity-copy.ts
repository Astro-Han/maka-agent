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
  unknownProvider: 'Unknown provider',
  success: 'Success',
  error: 'Error',
  cancelled: 'Cancelled',
  unknown: 'Unknown',
  rejected: 'Rejected',
  settled: 'Settled',
  modelOrTool: 'Model / tool',
  session: 'Session',
  outcome: 'Outcome',
  tokens: 'Input / output tokens',
  cost: 'Cost',
  empty: 'No matching activity in this snapshot.',
};
export const copy = {
  en,
  'zh-CN': {
    unknownProvider: '未知提供商',
    success: '成功',
    error: '失败',
    cancelled: '取消',
    unknown: '未知',
    rejected: '拒绝',
    settled: '结算时间',
    modelOrTool: '模型 / 工具',
    session: 'Session',
    outcome: '结果',
    tokens: '输入 / 输出 token',
    cost: '费用',
    empty: '此快照没有匹配的活动。',
  },
  'zh-TW': {
    unknownProvider: '未知供應商',
    success: '成功',
    error: '失敗',
    cancelled: '取消',
    unknown: '未知',
    rejected: '拒絕',
    settled: '結算時間',
    modelOrTool: '模型／工具',
    session: '對話',
    outcome: '結果',
    tokens: '輸入／輸出 token',
    cost: '費用',
    empty: '此快照沒有符合的活動。',
  },
} satisfies Record<ClientLocale, typeof en>;
