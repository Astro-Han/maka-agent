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
  credentialChanged: 'Credential changed. Refresh before retrying.',
  valid: 'Valid',
  invalidCredentials: 'Invalid credentials',
  rateLimited: 'Rate limited',
  networkError: 'Network or provider response error',
  timedOut: 'Timed out',
  refresh: 'Refresh',
  description:
    'The Web plugin manages search sources. Search and fetching are disabled in incognito mode.',
  enable: 'Enable web search',
  source: 'Search source',
  nativeSearch: 'Model-native search',
  nativeDescription:
    'Uses native search supported by the selected model protocol; never silently switches sources.',
  tavilyDescription: 'Fetches titles, source links and snippets through Tavily.',
  apiKey: 'Tavily API key',
  savedKey: 'Saved; enter a new key to replace',
  saveKey: 'Save key',
  removeKey: 'Remove key',
  testKey: 'Test saved key',
  notTested: 'Not tested',
  noCredential: 'No credential configured',
  trySearch: 'Try a search',
  search: 'Search',
  loading: 'Loading Web configuration…',
  truncated: 'Some results or snippets were truncated. Omitted results: ',
};

export const copy = {
  en,
  'zh-CN': {
    credentialChanged: '密钥已被修改，请刷新后重试。',
    valid: '验证成功',
    invalidCredentials: '密钥无效',
    rateLimited: '请求限流',
    networkError: '网络或服务响应错误',
    timedOut: '请求超时',
    refresh: '刷新',
    description: '搜索来源由 Web 插件管理。隐私模式下不提供搜索和网页抓取。',
    enable: '启用联网搜索',
    source: '搜索来源',
    nativeSearch: '模型内置搜索',
    nativeDescription: '使用当前模型协议支持的原生搜索；不支持时不会偷偷改用其他来源。',
    tavilyDescription: '通过 Tavily 获取标题、来源链接和摘要。',
    apiKey: 'Tavily 密钥',
    savedKey: '已保存；输入新密钥以替换',
    saveKey: '保存密钥',
    removeKey: '删除密钥',
    testKey: '测试已保存密钥',
    notTested: '尚未验证',
    noCredential: '未配置密钥',
    trySearch: '测试搜索',
    search: '搜索',
    loading: '正在读取 Web 插件配置…',
    truncated: '部分结果或摘要已截断。省略结果数：',
  },
  'zh-TW': {
    credentialChanged: '金鑰已被修改，請重新整理後重試。',
    valid: '驗證成功',
    invalidCredentials: '金鑰無效',
    rateLimited: '請求受限流',
    networkError: '網路或服務回應錯誤',
    timedOut: '請求逾時',
    refresh: '重新整理',
    description: '搜尋來源由 Web 外掛管理。隱私模式下不提供搜尋和網頁擷取。',
    enable: '啟用網路搜尋',
    source: '搜尋來源',
    nativeSearch: '模型內建搜尋',
    nativeDescription: '使用目前模型協定支援的原生搜尋；不支援時不會暗中改用其他來源。',
    tavilyDescription: '透過 Tavily 取得標題、來源連結和摘要。',
    apiKey: 'Tavily 金鑰',
    savedKey: '已儲存；輸入新金鑰以取代',
    saveKey: '儲存金鑰',
    removeKey: '刪除金鑰',
    testKey: '測試已儲存金鑰',
    notTested: '尚未驗證',
    noCredential: '未設定金鑰',
    trySearch: '測試搜尋',
    search: '搜尋',
    loading: '正在讀取 Web 外掛設定…',
    truncated: '部分結果或摘要已截斷。省略結果數：',
  },
} satisfies Record<ClientLocale, typeof en>;
