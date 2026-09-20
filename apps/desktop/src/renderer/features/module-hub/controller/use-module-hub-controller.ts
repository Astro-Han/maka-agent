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

import { useMemo, useRef, useState } from 'react';
import type { NavSelection } from '@maka/ui';
import { useToast, useUiLocale } from '@maka/ui';
import { useModuleHubServices } from '../services-context.js';
import {
  useDailyReviewController,
  type ActiveComposerClaim,
  type DailyReviewController,
} from './use-daily-review-controller.js';
import {
  useKeepSystemAwakeController,
  type KeepSystemAwakeController,
} from './use-keep-system-awake-controller.js';

export interface ModuleHubHostModel {
  readonly selection: NavSelection;
  readonly selectModule: (selection: NavSelection) => void;
  readonly action?: { id: number; name: string; handled: () => void };
  readonly keepSystemAwake: KeepSystemAwakeController;
  readonly dailyReview: DailyReviewController;
  readonly openSession: (sessionId: string) => void;
}

export interface ModuleHubCommands {
  openAction(selection: NavSelection, name: string): void;
  copyTodayDailyReview(): Promise<void>;
  pasteTodayDailyReview(): Promise<void>;
  saveTodayDailyReview(): Promise<void>;
}

export interface ModuleHubController {
  readonly host: ModuleHubHostModel;
  readonly commands: ModuleHubCommands;
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
  const [action, setAction] = useState<ModuleHubHostModel['action']>();
  const actionId = useRef(0);
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

  return useMemo(
    () => ({
      host: {
        selection: input.selection,
        selectModule: input.selectModule,
        action,
        keepSystemAwake,
        dailyReview,
        openSession: input.openSession,
      },
      commands: {
        openAction: (selection: NavSelection, name: string) => {
          input.selectModule(selection);
          const id = ++actionId.current;
          setAction({ id, name, handled: () => setAction((current) => current?.id === id ? undefined : current) });
        },
        copyTodayDailyReview: dailyReview.copyToday,
        pasteTodayDailyReview: dailyReview.pasteToday,
        saveTodayDailyReview: dailyReview.saveToday,
      },
    }),
    [
      dailyReview,
      input.openSession,
      input.selectModule,
      input.selection,
      keepSystemAwake,
      action,
    ],
  );
}
