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
  details: 'Work details',
  result: 'Result',
  patch: 'Workspace patch',
  patchUnavailable: 'Patch is not available yet. Retry after the worker finishes cleanup.',
  more: 'Read more',
  target: 'Target',
  replaces: 'Replaces',
  title: 'Agent Graph',
  loading: 'Loading…',
  failed: 'Graph unavailable',
  retry: 'Retry',
  stop: 'Stop graph',
  stopping: 'Stop requested',
  finished: 'Completed',
  empty: 'No scheduled work',
  next: 'Next page',
  first: 'First page',
  history: 'Older graphs',
  omitted: 'Instruction preview',
  session: 'Open agent',
  inputs: 'Inputs',
  authorize: 'Allow Agent Graph background work',
  authorizeButton: 'Authorize background execution',
};

export const copy = {
  en,
  'zh-CN': {
    details: '工作详情',
    result: '执行结果',
    patch: '工作区补丁',
    patchUnavailable: '补丁尚不可用，请在工作进程清理完成后重试。',
    more: '继续读取',
    target: '目标',
    replaces: '替代工作',
    title: 'Agent Graph',
    loading: '加载中…',
    failed: 'Graph 暂不可用',
    retry: '重试',
    stop: '停止 Graph',
    stopping: '已请求停止',
    finished: '已完成',
    empty: '尚未安排工作',
    next: '下一页',
    first: '第一页',
    history: '更早的 Graph',
    omitted: '指令预览',
    session: '打开 Agent',
    inputs: '输入',
    authorize: '允许 Agent Graph 后台执行',
    authorizeButton: '授权后台执行',
  },
  'zh-TW': {
    details: '工作詳情',
    result: '執行結果',
    patch: '工作區補丁',
    patchUnavailable: '補丁尚不可用，請在工作程序清理完成後重試。',
    more: '繼續讀取',
    target: '目標',
    replaces: '替代工作',
    title: 'Agent Graph',
    loading: '載入中…',
    failed: 'Graph 暫不可用',
    retry: '重試',
    stop: '停止 Graph',
    stopping: '已請求停止',
    finished: '已完成',
    empty: '尚未安排工作',
    next: '下一頁',
    first: '第一頁',
    history: '更早的 Graph',
    omitted: '指令預覽',
    session: '開啟 Agent',
    inputs: '輸入',
    authorize: '允許 Agent Graph 在背景執行',
    authorizeButton: '授權背景執行',
  },
} satisfies Record<ClientLocale, typeof en>;
