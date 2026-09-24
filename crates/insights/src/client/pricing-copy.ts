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
  ratesChanged: 'Rates changed. Refresh the catalog.',
  reviewChanges: 'Rates changed. Refresh and review before deciding to save again.',
  saved: 'Saved; applies to future admissions only.',
  invalidRates: 'Rates must be finite, non-negative numbers.',
  modelRequired: 'Enter a model key.',
  title: 'Pricing',
  description:
    'USD per million tokens. Blank cache rates are unspecified, not free. Historical calls retain their admission quote.',
  refresh: 'Refresh rates',
  setRates: 'Set model rates',
  modelKey: 'Model key',
  rateLabels: ['Input', 'Output', 'Cache read (optional)', 'Cache write (optional)'],
  save: 'Save rates',
  clear: 'Clear draft',
  columns: ['Model', 'Input', 'Output', 'Cache read', 'Cache write', 'Source', 'Actions'],
  builtin: 'Built-in',
  custom: 'Custom',
  edit: 'Edit',
  restore: 'Restore built-in',
  remove: 'Remove rates',
  entries: 'Entries',
  first: 'First',
  next: 'Next',
};
export const copy = {
  en,
  'zh-CN': {
    ratesChanged: '报价已变化，请刷新。',
    reviewChanges: '报价已被修改。刷新后检查，再决定是否保存。',
    saved: '已保存；仅影响此后准入的调用。',
    invalidRates: '单价必须是非负有限数字。',
    modelRequired: '请输入模型标识。',
    title: '报价',
    description: '美元 / 百万 token。空白缓存价表示未配置，不是免费。历史调用保留准入时的报价。',
    refresh: '刷新报价',
    setRates: '设置模型报价',
    modelKey: '模型标识',
    rateLabels: ['输入', '输出', '缓存读取（可选）', '缓存写入（可选）'],
    save: '保存报价',
    clear: '清空草稿',
    columns: ['模型', '输入', '输出', '缓存读取', '缓存写入', '来源', '操作'],
    builtin: '内置',
    custom: '自定义',
    edit: '编辑',
    restore: '恢复内置价',
    remove: '移除报价',
    entries: '当前条目',
    first: '首页',
    next: '下一页',
  },
  'zh-TW': {
    ratesChanged: '價格已變更，請重新整理。',
    reviewChanges: '價格已被修改。重新整理並檢查後，再決定是否儲存。',
    saved: '已儲存；僅影響此後准入的呼叫。',
    invalidRates: '單價必須是非負有限數字。',
    modelRequired: '請輸入模型識別碼。',
    title: '價格',
    description: '美元／百萬 token。空白快取價格表示未設定，不是免費。歷史呼叫保留准入時的價格。',
    refresh: '重新整理價格',
    setRates: '設定模型價格',
    modelKey: '模型識別碼',
    rateLabels: ['輸入', '輸出', '快取讀取（選填）', '快取寫入（選填）'],
    save: '儲存價格',
    clear: '清空草稿',
    columns: ['模型', '輸入', '輸出', '快取讀取', '快取寫入', '來源', '操作'],
    builtin: '內建',
    custom: '自訂',
    edit: '編輯',
    restore: '還原內建價格',
    remove: '移除價格',
    entries: '目前項目',
    first: '首頁',
    next: '下一頁',
  },
} satisfies Record<ClientLocale, typeof en>;
