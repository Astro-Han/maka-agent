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

import type { SessionChangedEvent, SessionSummary } from '@maka/core/session';
import type { SessionEventStreamSnapshot } from '@maka/core/session-event-health';
import type { SessionPatchResult } from './session-catalog-state.js';
import { recordSessionEventStreamChange } from './session-event-health.js';

export function handleSessionChangedEvent(event: SessionChangedEvent, options: {
  activeIdRef: { current: string | undefined };
  refreshSession(id: string): Promise<SessionPatchResult>;
  refreshSessions(): Promise<SessionSummary[]>;
  refreshProjects(): Promise<unknown>;
  refreshMessages(id: string): Promise<boolean>;
  retireSession(id: string): void;
  retiredSessionIds(sessions: readonly { id: string }[]): string[];
  clearPendingTurnActionsForSession(id: string): void;
  setSessionEventHealthBySession(update: (current: Record<string, SessionEventStreamSnapshot>) => Record<string, SessionEventStreamSnapshot>): void;
  notifyModelRebound(modelId: string | undefined): void;
}): void {
      if (event.sessionId) {
        const id = event.sessionId;
        void options.refreshSession(id).then((result) => {
          if (result.kind === 'observed' && result.session === null) options.retireSession(id);
          else if (result.kind === 'superseded') void options.refreshSessions();
        }).catch(() => {
          void options.refreshSessions();
        });
      } else {
        void options.refreshSessions().then((sessions) => {
          options.retiredSessionIds(sessions).forEach(options.retireSession);
        });
      }
      if (event.reason === 'archived' && event.sessionId) options.retireSession(event.sessionId);
      if (event.reason === 'created' || event.reason === 'migrated') {
        void options.refreshProjects();
      }
    if (event.sessionId) {
      options.setSessionEventHealthBySession((current) => {
        const previous = current[event.sessionId!];
        if (!previous) return current;
        return {
          ...current,
          [event.sessionId!]: recordSessionEventStreamChange(previous, event.ts),
        };
      });
    }
    if (
      event.sessionId &&
      (event.reason === 'turn-status-change' || event.reason === 'message-appended' || event.reason === 'deleted')
    ) {
      options.clearPendingTurnActionsForSession(event.sessionId);
    }
    const changedSessionId = event.sessionId;
    if (event.reason === 'message-appended' && changedSessionId && changedSessionId === options.activeIdRef.current) {
      void options.refreshMessages(changedSessionId);
    }
    if (event.reason === 'rebound') options.notifyModelRebound(event.modelId);
}
