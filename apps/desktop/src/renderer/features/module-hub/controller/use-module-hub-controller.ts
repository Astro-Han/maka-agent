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

import { useEffect, useMemo, useRef } from 'react';
import type { ScheduledTask } from '@maka/core/scheduled-task';
import type { NavSelection } from '@maka/ui';
import { useToast, useUiLocale } from '@maka/ui';
import { useModuleHubServices } from '../services-context.js';
import { startModuleHubLifecycle } from './module-hub-lifecycle.js';
import {
  useDailyReviewController,
  type ActiveComposerClaim,
  type DailyReviewController,
} from './use-daily-review-controller.js';
import {
  useKeepSystemAwakeController,
  type KeepSystemAwakeController,
} from './use-keep-system-awake-controller.js';
import {
  useScheduledTasksController,
  type ScheduledTasksController,
} from './use-scheduled-tasks-controller.js';

export interface ModuleHubHostModel {
  readonly selection: NavSelection;
  readonly selectModule: (selection: NavSelection) => void;
  readonly scheduledTasks: ScheduledTasksController;
  readonly keepSystemAwake: KeepSystemAwakeController;
  readonly dailyReview: DailyReviewController;
  readonly openSession: (sessionId: string) => void;
}

export interface ModuleHubCommands {
  openScheduledTaskCreate(): void;
  copyTodayDailyReview(): Promise<void>;
  pasteTodayDailyReview(): Promise<void>;
  saveTodayDailyReview(): Promise<void>;
}

export interface ModuleHubController {
  readonly host: ModuleHubHostModel;
  readonly commands: ModuleHubCommands;
  readonly selectors: {
    readonly scheduledTasks: readonly ScheduledTask[];
  };
}

export interface UseModuleHubControllerInput {
  readonly selection: NavSelection;
  readonly selectModule: (selection: NavSelection) => void;
  readonly openSession: (sessionId: string) => void;
  readonly appendComposerText: (text: string) => void;
  readonly captureActiveComposerClaim: () => ActiveComposerClaim | undefined;
}

/** Public ownership boundary for every Module Hub surface except the MCP leaf. */
export function useModuleHubController(
  input: UseModuleHubControllerInput,
): ModuleHubController {
  const services = useModuleHubServices();
  const uiLocale = useUiLocale();
  const toastApi = useToast();
  const scheduledTasks = useScheduledTasksController({
    uiLocale,
    toastApi,
    selection: input.selection,
    selectModule: input.selectModule,
  });
  const keepSystemAwake = useKeepSystemAwakeController(services);
  const selectionRef = useRef(input.selection);
  selectionRef.current = input.selection;
  const dailyReview = useDailyReviewController({
    services,
    uiLocale,
    toastApi,
    appendComposerText: input.appendComposerText,
    captureActiveComposerClaim: input.captureActiveComposerClaim,
    isDailyReviewSurfaceActive: () =>
      selectionRef.current.section === 'automations' &&
      selectionRef.current.module === 'daily-review',
  });

  const refreshScheduledTasksRef = useRef(scheduledTasks.refresh);
  refreshScheduledTasksRef.current = scheduledTasks.refresh;

  useEffect(() => {
    return startModuleHubLifecycle({
      runtimeHosts: services.runtimeHosts,
      refreshScheduledTasks: () => void refreshScheduledTasksRef.current(),
    });
  }, [services.runtimeHosts]);

  return useMemo(
    () => ({
      host: {
        selection: input.selection,
        selectModule: input.selectModule,
        scheduledTasks,
        keepSystemAwake,
        dailyReview,
        openSession: input.openSession,
      },
      commands: {
        openScheduledTaskCreate: scheduledTasks.openCreate,
        copyTodayDailyReview: dailyReview.copyToday,
        pasteTodayDailyReview: dailyReview.pasteToday,
        saveTodayDailyReview: dailyReview.saveToday,
      },
      selectors: {
        scheduledTasks: scheduledTasks.scheduledTasks,
      },
    }),
    [
      dailyReview,
      input.openSession,
      input.selectModule,
      input.selection,
      keepSystemAwake,
      scheduledTasks,
    ],
  );
}
