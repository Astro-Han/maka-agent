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
  authorize: 'Allow scheduled background work',
  authorizationDenied: 'Background authorization was not granted',
  deleteTitle: 'Delete scheduled task?',
  deleteDescription: 'Already accepted executions will not be cancelled.',
  noModel: 'This Session has no model',
  accessTitle: 'Scheduled background access',
  allowResume: 'Allow resuming this Session',
  allowCreate: 'Allow new Sessions in this workspace',
  activeTasks: 'Active scheduled tasks',
};

export const copy = {
  en,
  'zh-CN': {
    authorize: '允许定时任务在后台执行',
    authorizationDenied: '未获得后台授权',
    deleteTitle: '删除定时任务？',
    deleteDescription: '已接受的执行不会被取消。',
    noModel: '此会话没有模型',
    accessTitle: '定时任务后台权限',
    allowResume: '允许继续此会话',
    allowCreate: '允许在此工作区创建会话',
    activeTasks: '活动定时任务',
  },
  'zh-TW': {
    authorize: '允許排程工作在背景執行',
    authorizationDenied: '未取得背景授權',
    deleteTitle: '刪除排程工作？',
    deleteDescription: '已接受的執行不會被取消。',
    noModel: '此對話沒有模型',
    accessTitle: '排程工作背景權限',
    allowResume: '允許繼續此對話',
    allowCreate: '允許在此工作區建立對話',
    activeTasks: '執行中的排程工作',
  },
} satisfies Record<ClientLocale, typeof en>;
