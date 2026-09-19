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

import type {
  DailyReviewArchive,
  DailyReviewArchiveSummary,
  DailyReviewRange,
  DailyReviewSummary,
} from '@maka/core/daily-review';
import type { Result } from '@maka/core/result';
import type {
  CreateScheduledTaskInput,
  ScheduledTask,
  UpdateScheduledTaskInput,
} from '@maka/core/scheduled-task';

export type ModuleHubUnsubscribe = () => void;

export interface ModuleHubRuntimeHostRef {
  readonly profileId: string;
  readonly hostId: string;
}

export interface ModuleHubRuntimeHostChangedEvent {
  readonly profileId: string;
  readonly readiness: 'connecting' | 'ready' | 'reconnecting' | 'unavailable';
  readonly hostId?: string;
  readonly isDefault: boolean;
  readonly removed?: boolean;
}

export interface ModuleHubRuntimeHostsService {
  getDefault(): Promise<ModuleHubRuntimeHostRef>;
  subscribeChanges(
    handler: (event: ModuleHubRuntimeHostChangedEvent) => void,
  ): ModuleHubUnsubscribe;
}

export type ScheduledTaskCreateInput = Omit<CreateScheduledTaskInput, 'createdBy'>;

export interface ModuleHubScheduledTasksService {
  list(host: ModuleHubRuntimeHostRef): Promise<ScheduledTask[]>;
  create(
    input: ScheduledTaskCreateInput,
    host: ModuleHubRuntimeHostRef,
  ): Promise<ScheduledTask>;
  update(
    id: string,
    patch: UpdateScheduledTaskInput,
    host: ModuleHubRuntimeHostRef,
  ): Promise<ScheduledTask>;
  setEnabled(
    id: string,
    enabled: boolean,
    host: ModuleHubRuntimeHostRef,
  ): Promise<ScheduledTask>;
  triggerNow(id: string, host: ModuleHubRuntimeHostRef): Promise<ScheduledTask>;
  snooze(id: string, host: ModuleHubRuntimeHostRef): Promise<ScheduledTask>;
  clearRunHistory(id: string, host: ModuleHubRuntimeHostRef): Promise<ScheduledTask>;
  delete(id: string, host: ModuleHubRuntimeHostRef): Promise<void>;
  subscribeChanges(
    handler: (event: {
      type: 'scheduled_tasks_changed';
      reason: string;
      taskId?: string;
      ts: number;
    }) => void,
  ): ModuleHubUnsubscribe;
  subscribeDue(
    handler: (task: Pick<ScheduledTask, 'id' | 'title'>) => void,
  ): ModuleHubUnsubscribe;
}

export interface ModuleHubClientSettingsService {
  readonly supported: boolean;
  getKeepSystemAwake(): Promise<boolean>;
  setKeepSystemAwake(next: boolean): Promise<boolean>;
  subscribeChanges(handler: () => void): ModuleHubUnsubscribe;
}

export interface ModuleHubDailyReviewService {
  day(
    offsetDays: number,
    daySpan: number | undefined,
    host: ModuleHubRuntimeHostRef,
  ): Promise<Result<DailyReviewSummary>>;
  runOnce(input: {
    range: DailyReviewRange;
    offsetDays?: number;
    modelKey?: string;
  }): Promise<{ archiveId: string }>;
  listArchives(): Promise<DailyReviewArchiveSummary[]>;
  getArchive(archiveId: string): Promise<DailyReviewArchive | null>;
  saveMarkdownToFile(input: {
    markdown: string;
    defaultName: string;
  }): Promise<
    | { ok: true; path: string }
    | { ok: false; reason: 'canceled' | 'write_failed' | 'invalid_input' }
  >;
}

export interface ModuleHubClipboardService {
  writeText(text: string): Promise<void>;
}

/** Environment capabilities owned by the Module Hub feature slice. */
export interface ModuleHubServices {
  runtimeHosts: ModuleHubRuntimeHostsService;
  scheduledTasks: ModuleHubScheduledTasksService;
  clientSettings: ModuleHubClientSettingsService;
  dailyReview: ModuleHubDailyReviewService;
  clipboard: ModuleHubClipboardService;
}
