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
  missingCounter: (missing: number) => `${missing} calls omitted this counter`,
  unvaluedCalls: (unvalued: number, unpriced: number) =>
    `${unvalued} unvalued calls; ${unpriced} without rates`,
  modelCalls: 'Model calls',
  valuedCost: 'Valued cost',
  inputTokens: 'Input tokens',
  outputTokens: 'Output tokens',
  toolAttempts: 'Tool attempts',
  outcomes: 'Success / error / cancelled / unknown: ',
  cacheHit: 'Cache hit rate',
  modelTime: 'Cumulative model time',
  toolTime: 'Cumulative tool time',
  cacheRead: 'Cache read',
  cacheWrite: 'Cache write',
  reasoningTokens: 'Reasoning tokens',
  pending: 'Pending models / tools',
  unvalued: 'Unvalued / unpriced',
  uncertainty:
    '“+ ?” means some data is unknown, not zero. Cumulative durations may overlap; they are not elapsed session time. Completed totals use settlement time; pending counts use admission time. Rate edits do not change historical costs.',
  tool: 'Tool',
  attempts: 'Attempts',
  success: 'Success',
  error: 'Error',
  rejected: 'Rejected',
  unknown: 'Unknown',
  meanDuration: 'Mean duration',
  unknownProvider: 'Unknown provider',
  provider: 'Provider',
  model: 'Model',
  calls: 'Calls',
  cost: 'Cost',
};
export const copy = {
  en,
  'zh-CN': {
    missingCounter: (missing: number) => `${missing} 次调用缺少此计数`,
    unvaluedCalls: (unvalued: number, unpriced: number) =>
      `${unvalued} 次未估价，其中 ${unpriced} 次没有报价`,
    modelCalls: '模型调用',
    valuedCost: '已估价费用',
    inputTokens: '输入 token',
    outputTokens: '输出 token',
    toolAttempts: '工具尝试',
    outcomes: '成功 / 失败 / 取消 / 未知：',
    cacheHit: '缓存命中率',
    modelTime: '模型累计耗时',
    toolTime: '工具累计耗时',
    cacheRead: '缓存读取',
    cacheWrite: '缓存写入',
    reasoningTokens: '推理 token',
    pending: '待结算模型 / 工具',
    unvalued: '未估价 / 无报价',
    uncertainty:
      '“+ ?” 表示部分数据未知，不等于零。累计耗时可能因并行调用而重叠，不是会话经过时间。已完成统计按结算时间，待结算数按准入时间。修改报价不改变历史费用。',
    tool: '工具',
    attempts: '尝试',
    success: '成功',
    error: '失败',
    rejected: '拒绝',
    unknown: '未知',
    meanDuration: '平均耗时',
    unknownProvider: '未知提供商',
    provider: '提供商',
    model: '模型',
    calls: '调用',
    cost: '费用',
  },
  'zh-TW': {
    missingCounter: (missing: number) => `${missing} 次呼叫缺少此計數`,
    unvaluedCalls: (unvalued: number, unpriced: number) =>
      `${unvalued} 次未估價，其中 ${unpriced} 次沒有價格`,
    modelCalls: '模型呼叫',
    valuedCost: '已估價費用',
    inputTokens: '輸入 token',
    outputTokens: '輸出 token',
    toolAttempts: '工具嘗試',
    outcomes: '成功／失敗／取消／未知：',
    cacheHit: '快取命中率',
    modelTime: '模型累計耗時',
    toolTime: '工具累計耗時',
    cacheRead: '快取讀取',
    cacheWrite: '快取寫入',
    reasoningTokens: '推理 token',
    pending: '待結算模型／工具',
    unvalued: '未估價／無價格',
    uncertainty:
      '「+ ?」表示部分資料未知，不等於零。累計耗時可能因並行呼叫而重疊，不是對話經過時間。已完成統計依結算時間，待結算數依准入時間。修改價格不改變歷史費用。',
    tool: '工具',
    attempts: '嘗試',
    success: '成功',
    error: '失敗',
    rejected: '拒絕',
    unknown: '未知',
    meanDuration: '平均耗時',
    unknownProvider: '未知供應商',
    provider: '供應商',
    model: '模型',
    calls: '呼叫',
    cost: '費用',
  },
} satisfies Record<ClientLocale, typeof en>;
