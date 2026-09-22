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

import { useState } from 'react';
import type { UiLocale } from '@maka/core/ui-locale';
import { resumeParkToastCopy } from '@maka/ui';
import { getShellCopy, localizedShellErrorMessage } from './locales/shell-copy.js';

type ToastApi = {
  info(title: string, description?: string): void;
  error(
    title: string,
    description?: string,
    diagnosticDetails?: string,
    diagnosticTarget?: { sessionId: string },
  ): void;
};

/** Resume only the selection that requested readiness; settle its own pending state. */
export function useShellResume(options: {
  activeId: string | undefined;
  toastApi: ToastApi;
  shellCopy: ReturnType<typeof getShellCopy>['app'];
  uiLocale: UiLocale;
  captureSelection(): () => boolean;
  checkExecutionReadiness(): Promise<boolean>;
}): {
  resumePendingSessionId: string | null;
  resumeParkDescriptionBySession: Record<string, string>;
  resumeInterruptedSession: () => Promise<void>;
} {
  const { activeId, toastApi, shellCopy, uiLocale } = options;
  const [resumePendingSessionId, setResumePendingSessionId] = useState<string | null>(null);
  const [resumeParkDescriptionBySession, setResumeParkDescriptionBySession] = useState<Record<string, string>>({});

  async function resumeInterruptedSession(): Promise<void> {
    const sessionId = activeId;
    if (!sessionId || resumePendingSessionId !== null) return;
    const selectionIsCurrent = options.captureSelection();
    setResumePendingSessionId(sessionId);
    try {
      if (!(await options.checkExecutionReadiness()) || !selectionIsCurrent()) return;
      const result = await window.maka.sessions.resumeLatest(sessionId);
      if (result.disposition === 'park') {
        const parkCopy = resumeParkToastCopy(result.rejectionReasons, uiLocale);
        setResumeParkDescriptionBySession((current) => ({
          ...current,
          [sessionId]: parkCopy.description,
        }));
        toastApi.error(parkCopy.title, parkCopy.description, undefined, { sessionId });
      } else {
        setResumeParkDescriptionBySession((current) => {
          const { [sessionId]: _removed, ...remaining } = current;
          void _removed;
          return remaining;
        });
        toastApi.info(shellCopy.resumeStartedTitle, shellCopy.resumeStartedDescription);
      }
    } catch (error) {
      toastApi.error(
        shellCopy.resumeFailedTitle,
        localizedShellErrorMessage(
          error,
          shellCopy.resumeFailedFallback,
          uiLocale,
        ),
        undefined,
        { sessionId },
      );
    } finally {
      setResumePendingSessionId((current) => current === sessionId ? null : current);
    }
  }

  return {
    resumePendingSessionId,
    resumeParkDescriptionBySession,
    resumeInterruptedSession,
  };
}
