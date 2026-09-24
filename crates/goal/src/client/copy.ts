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
  title: 'Goal',
  incomplete: 'incomplete',
  statuses: {
    armed: 'Ready to start',
    active: 'Running',
    paused: 'Paused',
    waiting: 'Waiting for input',
    achieved: 'Completed',
    impossible: 'Cannot complete',
    cancelled: 'Cancelled',
    cancellation_unknown: 'Cancellation unconfirmed',
    max_iterations: 'Iteration limit reached',
    budget_limited: 'Token threshold reached',
    budget_unknown: 'Usage incomplete',
    blocked: 'Execution needs attention',
  },
  invalidGoal: 'Enter an objective, 1–100 iterations and a valid token threshold',
  authorizeContinuation: 'Allow this Goal to continue in this Session',
  backgroundDenied: 'Background access was not granted',
  renewAccess: 'Renew Goal background access',
  accessDenied: 'Access was not granted',
  description:
    'Authorize automatic continuation toward an objective. Model completion reports apply after the execution ends successfully.',
  observedTokens: 'Observed additional Session tokens',
  unsettled:
    'The current iteration is unsettled. Pause stops later iterations; cancellation waits on the original operation, which may still be admitted if dispatch already began.',
  checkCancellation: 'Check cancellation (renew access if needed)',
  pause: 'Pause',
  resume: 'Start / resume',
  cancel: 'Cancel goal',
  complete: 'Mark complete',
  objective: 'Objective',
  iterations: 'Maximum iterations',
  tokenThreshold: 'Additional Session token threshold (optional)',
  budgetDescription:
    'Includes other Session activity since Goal creation. Stops continuation after the observed threshold or missing usage; not a hard request limit.',
  save: 'Save for later',
  start: 'Authorize and start',
  retry: 'Retry original request',
};

export const copy = {
  en,
  'zh-CN': {
    title: '目标',
    incomplete: '不完整',
    statuses: {
      armed: '待启动',
      active: '执行中',
      paused: '已暂停',
      waiting: '等待输入',
      achieved: '已完成',
      impossible: '无法完成',
      cancelled: '已取消',
      cancellation_unknown: '取消待核对',
      max_iterations: '达到轮数上限',
      budget_limited: '达到用量阈值',
      budget_unknown: '用量不完整',
      blocked: '需要处理执行问题',
    },
    invalidGoal: '请填写目标、1–100轮及有效 token 阈值',
    authorizeContinuation: '允许此目标在会话中自动续跑',
    backgroundDenied: '未获得后台授权',
    renewAccess: '重新授权此目标续跑',
    accessDenied: '未获得授权',
    description: '明确目标后允许自动续跑。模型报告完成后，还需本轮执行正常结束。',
    observedTokens: '本会话新增已观测 token',
    unsettled:
      '当前一轮尚未结算；暂停只停止后续轮次。取消会按原操作 ID 等待结果，已进入派发的请求可能先被 Host 接受。',
    checkCancellation: '重查取消状态（必要时重新授权）',
    pause: '暂停',
    resume: '启动／继续',
    cancel: '取消目标',
    complete: '标记完成',
    objective: '目标',
    iterations: '最多续跑轮数',
    tokenThreshold: '本会话新增 token 阈值（可选）',
    budgetDescription:
      '包含创建目标后的其他会话活动；达到已观测阈值或用量缺失时停止续跑，不是单次请求的硬上限。',
    save: '保存，稍后启动',
    start: '授权并启动',
    retry: '查询／重试原请求',
  },
  'zh-TW': {
    title: '目標',
    incomplete: '不完整',
    statuses: {
      armed: '待啟動',
      active: '執行中',
      paused: '已暫停',
      waiting: '等待輸入',
      achieved: '已完成',
      impossible: '無法完成',
      cancelled: '已取消',
      cancellation_unknown: '取消待確認',
      max_iterations: '達到輪數上限',
      budget_limited: '達到用量閾值',
      budget_unknown: '用量不完整',
      blocked: '需要處理執行問題',
    },
    invalidGoal: '請填寫目標、1–100 輪及有效 token 閾值',
    authorizeContinuation: '允許此目標在對話中自動繼續執行',
    backgroundDenied: '未取得背景授權',
    renewAccess: '重新授權此目標繼續執行',
    accessDenied: '未取得授權',
    description: '明確目標後允許自動繼續執行。模型回報完成後，仍需本輪執行正常結束。',
    observedTokens: '本對話新增已觀測 token',
    unsettled:
      '目前這一輪尚未結算；暫停只停止後續輪次。取消會依原操作 ID 等待結果，已開始派送的請求可能先被 Host 接受。',
    checkCancellation: '重新檢查取消狀態（必要時重新授權）',
    pause: '暫停',
    resume: '啟動／繼續',
    cancel: '取消目標',
    complete: '標記完成',
    objective: '目標',
    iterations: '最多繼續執行輪數',
    tokenThreshold: '本對話新增 token 閾值（選填）',
    budgetDescription:
      '包含建立目標後的其他對話活動；達到已觀測閾值或用量缺失時停止繼續執行，不是單次請求的硬性上限。',
    save: '儲存，稍後啟動',
    start: '授權並啟動',
    retry: '查詢／重試原請求',
  },
} satisfies Record<ClientLocale, typeof en>;
