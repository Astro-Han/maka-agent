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
  testSucceeded: 'Connection test succeeded',
  description:
    'Structured Jev decisions for plugins. Keys are saved per endpoint; configure a key after changing the URL. Calls are disabled in incognito mode.',
  refresh: 'Refresh',
  enable: 'Enable Jev',
  model: 'Model',
  timeout: 'Timeout (ms)',
  save: 'Save configuration',
  apiKey: 'API key',
  saved: 'Saved',
  headers: 'Custom headers (JSON)',
  replaceWarning:
    'Saving replaces all authentication and headers for this endpoint. Saved header names: ',
  invalidHeaders: 'Headers must be a JSON object with string values',
  replaceCredentials: 'Replace authentication and headers',
  removeKey: 'Remove key',
  test: 'Test saved configuration',
};

export const copy = {
  en,
  'zh-CN': {
    testSucceeded: '连接测试成功',
    description:
      '为插件提供 Jev 结构化决策。密钥分别保存到每个端点；更改 URL 后请为新端点配置密钥。隐私模式下禁止调用。',
    refresh: '刷新',
    enable: '启用 Jev',
    model: '模型',
    timeout: '超时（毫秒）',
    save: '保存配置',
    apiKey: '密钥',
    saved: '已保存',
    headers: '自定义请求头（JSON）',
    replaceWarning: '保存会替换当前端点的全部认证与请求头。已保存的请求头：',
    invalidHeaders: '请求头必须是字符串值的 JSON 对象',
    replaceCredentials: '替换认证与请求头',
    removeKey: '删除密钥',
    test: '测试已保存配置',
  },
  'zh-TW': {
    testSucceeded: '連線測試成功',
    description:
      '為外掛提供 Jev 結構化決策。金鑰分別儲存至各端點；變更 URL 後請為新端點設定金鑰。隱私模式下禁止呼叫。',
    refresh: '重新整理',
    enable: '啟用 Jev',
    model: '模型',
    timeout: '逾時（毫秒）',
    save: '儲存設定',
    apiKey: '金鑰',
    saved: '已儲存',
    headers: '自訂請求標頭（JSON）',
    replaceWarning: '儲存會取代目前端點的全部驗證資訊與請求標頭。已儲存的標頭：',
    invalidHeaders: '請求標頭必須是值為字串的 JSON 物件',
    replaceCredentials: '取代驗證資訊與請求標頭',
    removeKey: '刪除金鑰',
    test: '測試已儲存設定',
  },
} satisfies Record<ClientLocale, typeof en>;
