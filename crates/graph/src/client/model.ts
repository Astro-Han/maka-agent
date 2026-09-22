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

export type Epoch = {
  graphId: string;
  epoch: number;
  createdAt: number;
  mode: 'graph' | 'swarm';
  rootSessionId: string;
};
export type Cursor = { revision: number; workId: string };
export type WorkState =
  | 'requested'
  | 'waiting'
  | 'running'
  | 'blocked'
  | 'completed'
  | 'failed'
  | 'cancelled'
  | 'stopped'
  | 'superseded';
export const states = {
  en: {
    requested: 'Requested',
    waiting: 'Waiting',
    running: 'Running',
    blocked: 'Needs attention',
    completed: 'Completed',
    failed: 'Failed',
    cancelled: 'Cancelled',
    stopped: 'Stopped',
    superseded: 'Replaced',
  },
  'zh-CN': {
    requested: '已安排',
    waiting: '等待中',
    running: '执行中',
    blocked: '需要处理',
    completed: '已完成',
    failed: '失败',
    cancelled: '已取消',
    stopped: '已停止',
    superseded: '已替代',
  },
  'zh-TW': {
    requested: '已安排',
    waiting: '等待中',
    running: '執行中',
    blocked: '需要處理',
    completed: '已完成',
    failed: '失敗',
    cancelled: '已取消',
    stopped: '已停止',
    superseded: '已替代',
  },
} satisfies Record<'en' | 'zh-CN' | 'zh-TW', Record<WorkState, string>>;
export type Work = {
  workId: string;
  instruction: string;
  instructionTruncated: boolean;
  status: 'requested' | 'stopped' | 'superseded';
  execution: {
    sessionId: string;
    turnId: string;
    state: WorkState;
    resultRecordId: string | null;
  } | null;
};
export type Target =
  | { kind: 'agent'; agentId: string }
  | { kind: 'preset'; presetId: string }
  | { kind: 'executor'; executorId: string }
  | { kind: 'operator'; operatorId: string };
export type Detail = {
  workId: string;
  target: Target;
  instruction: string;
  offset: number;
  totalBytes: number;
  nextOffset: number | null;
  inputIds: string[];
  selectedResultInputs: { sourceGraphId: string; resultId: string }[];
  replaces: string | null;
};
export type Graph = {
  epoch: Epoch;
  revision: number;
  stopRequested: boolean;
  finished: boolean;
  selectedResultIds: string[];
  work: Work[];
  totalWork: number;
  nextAfter: Cursor | null;
};
export type Query =
  | {
      kind: 'result';
      graphId: string;
      workId: string;
      recordId: string;
      offset: number;
      part?: 'answer' | 'patch';
    }
  | { kind: 'work'; graphId: string; workId: string; offset: number }
  | { kind: 'epochs'; before: number | null }
  | { kind: 'snapshot'; graphId: string; after: Cursor | null };
export type Reply =
  | { kind: 'result'; result: ResultPage | null }
  | { kind: 'work'; work: Detail | null }
  | { kind: 'epochs'; epochs: Epoch[]; currentEpoch: number | null; nextBefore: number | null }
  | { kind: 'snapshot'; graph: Graph | null };
export type ResultPage = {
  part: 'answer' | 'patch';
  isolatedWorkspace: boolean;
  graphId: string;
  workId: string;
  recordId: string;
  text: string;
  offset: number;
  totalBytes: number;
  nextOffset: number | null;
};
export const copy = {
  en: {
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
  },
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
  },
};
