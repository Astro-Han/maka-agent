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

import { NativeHostBudget, NativeHostWaitError } from './native-runtime-host-operation.js';

export interface AppQuitEvent {
  preventDefault(): void;
}

export interface AppQuitCoordinator {
  focusOrCreateWindow(): Promise<void>;
  handleBeforeQuit(event: AppQuitEvent): void;
}

export interface AppQuitCoordinatorDeps {
  prepareToQuit(signal: AbortSignal): Promise<'ready' | 'cancelled'>;
  readonly timeoutMs?: number;
  cleanup(): Promise<void>;
  focusOrCreateWindow(signal: AbortSignal): void | Promise<void>;
  onPreparationError(error: unknown): void;
  onCleanupError(error: unknown): void;
  onWindowCreationError(error: unknown): void;
  resumeQuit(): void;
}

type AppQuitPhase = 'running' | 'preparing' | 'cleaning' | 'ready-to-exit';

export function createAppQuitCoordinator(deps: AppQuitCoordinatorDeps): AppQuitCoordinator {
  let phase: AppQuitPhase = 'running';
  const windowCreationAbort = new AbortController();

  const focusOrCreateWindow = (): Promise<void> => {
    if (phase !== 'running') return Promise.resolve();
    try {
      return Promise.resolve(deps.focusOrCreateWindow(windowCreationAbort.signal)).catch(
        deps.onWindowCreationError,
      );
    } catch (error) {
      deps.onWindowCreationError(error);
      return Promise.resolve();
    }
  };

  return {
    focusOrCreateWindow,
    handleBeforeQuit(event): void {
      if (phase === 'ready-to-exit') return;
      event.preventDefault();
      if (phase !== 'running') return;
      phase = 'preparing';
      const quitAbort = new AbortController();
      const budget = new NativeHostBudget(deps.timeoutMs ?? 8_000);
      const finishCleanup = () => {
        // `before-quit` was cancelled inside Electron's native quit transaction.
        // Resuming from the cleanup Promise's microtask re-enters that transaction:
        // Electron closes the windows but emits `window-all-closed` instead of
        // `will-quit`, leaving the macOS process alive. Start a fresh transaction
        // only after the current event-loop turn has unwound.
        setImmediate(() => {
          phase = 'ready-to-exit';
          deps.resumeQuit();
        });
      };
      void Promise.resolve()
        .then(() => budget.wait(deps.prepareToQuit(quitAbort.signal), 'quit preparation'))
        .catch((error) => {
          if (!(error instanceof NativeHostWaitError)) throw error;
          quitAbort.abort(error);
          deps.onPreparationError(error);
          // Leaving Desktop does not authorize killing Host or releasing its lock.
          return 'ready' as const;
        })
        .then(
          (preparation) => {
            if (preparation === 'cancelled') {
              phase = 'running';
              focusOrCreateWindow();
              return;
            }
            phase = 'cleaning';
            windowCreationAbort.abort();
            return Promise.resolve()
              .then(() => budget.wait(deps.cleanup(), 'quit cleanup'))
              .then(finishCleanup, (error) => {
                deps.onCleanupError(error);
                finishCleanup();
              });
          },
          (error) => {
            phase = 'running';
            deps.onPreparationError(error);
            focusOrCreateWindow();
          },
        );
    },
  };
}
