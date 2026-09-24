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
  ranges: ['24 hours', '7 days', '30 days', 'All time'],
  outcomes: ['All', 'Success', 'Error', 'Cancelled', 'Unknown', 'Rejected'],
  overview: 'Overview',
  activity: 'Activity',
  providers: 'Providers',
  models: 'Models',
  tools: 'Tools',
  pricing: 'Pricing',
  title: 'Usage & pricing',
  loadView: 'Load saved view',
  loadingView: 'Loading view…',
  pages: 'Usage pages',
  range: 'Range',
  refresh: 'Refresh snapshot',
  saveView: 'Save this view',
  readingSnapshot: 'Reading snapshot…',
  kind: 'Kind',
  all: 'All',
  model: 'Model',
  tool: 'Tool',
  outcome: 'Outcome',
  search: 'Search',
  applyFilters: 'Apply filters',
  filterDescription:
    'Filters affect this list only. Overview and breakdowns keep complete totals from the same snapshot.',
  matchingActivities: 'matching activities',
  previous: 'Previous',
  next: 'Next',
};

export const copy = {
  en,
  'zh-CN': {
    ranges: ['24 小时', '7 天', '30 天', '全部时间'],
    outcomes: ['全部', '成功', '失败', '取消', '未知', '拒绝'],
    overview: '概览',
    activity: '活动',
    providers: '提供商',
    models: '模型',
    tools: '工具',
    pricing: '报价',
    title: '用量与报价',
    loadView: '载入已保存视图',
    loadingView: '正在读取视图…',
    pages: '统计页面',
    range: '范围',
    refresh: '刷新快照',
    saveView: '保存此视图',
    readingSnapshot: '正在读取快照…',
    kind: '类型',
    all: '全部',
    model: '模型',
    tool: '工具',
    outcome: '结果',
    search: '搜索',
    applyFilters: '应用筛选',
    filterDescription: '筛选仅作用于活动列表；概览和分组仍使用同一快照的完整统计。',
    matchingActivities: '条匹配活动',
    previous: '上一页',
    next: '下一页',
  },
  'zh-TW': {
    ranges: ['24 小時', '7 天', '30 天', '全部時間'],
    outcomes: ['全部', '成功', '失敗', '取消', '未知', '拒絕'],
    overview: '概覽',
    activity: '活動',
    providers: '供應商',
    models: '模型',
    tools: '工具',
    pricing: '價格',
    title: '用量與價格',
    loadView: '載入已儲存檢視',
    loadingView: '正在讀取檢視…',
    pages: '統計頁面',
    range: '範圍',
    refresh: '重新整理快照',
    saveView: '儲存此檢視',
    readingSnapshot: '正在讀取快照…',
    kind: '類型',
    all: '全部',
    model: '模型',
    tool: '工具',
    outcome: '結果',
    search: '搜尋',
    applyFilters: '套用篩選',
    filterDescription: '篩選僅影響活動清單；概覽和分組仍使用同一快照的完整統計。',
    matchingActivities: '筆符合的活動',
    previous: '上一頁',
    next: '下一頁',
  },
} satisfies Record<ClientLocale, typeof en>;
