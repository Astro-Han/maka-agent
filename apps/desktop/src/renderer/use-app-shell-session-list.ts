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

import { useMemo, useRef } from 'react';
import { useUiLocale } from '@maka/ui';
import { getDesktopConversationCopy } from './locales/conversation-copy.js';
import { localizedShellErrorMessage } from './locales/shell-copy.js';
import { normalizeSessionSummaryForDisplay } from './session-status-presentation.js';
import {
  selectAuthoritativeSessionIds, selectCatalogRevision, selectSessions, type SessionCatalogController,
} from './application/contracts/session-catalog/session-catalog-state.js';
import { sessionIdSetsEqual } from './features/conversation/index.js';
import { useExternalStoreSelector } from './application/contracts/session-catalog/use-external-store-selector.js';
import { createDesktopSessionCatalogSync } from './platform/desktop/session-catalog-sync.js';

export function useAppShellSessionList(
  toastApi: { error(title: string, description?: string): void },
  { catalog }: { catalog: SessionCatalogController },
) {
  const uiLocale = useUiLocale();
  const presentation = useRef({ uiLocale, toastApi });
  presentation.current = { uiLocale, toastApi };
  const sessions = useExternalStoreSelector(catalog, selectSessions);
  const catalogRevision = useExternalStoreSelector(catalog, selectCatalogRevision);
  const authoritativeSessionIds = useExternalStoreSelector(
    catalog, selectAuthoritativeSessionIds, undefined, sessionIdSetsEqual,
  );
  const sessionsRef = useMemo(() => ({ get current() { return catalog.getState().sessions; } }), [catalog]);
  const actions = useMemo(() => createDesktopSessionCatalogSync(catalog, normalizeSessionSummaryForDisplay, (error) => {
    const { uiLocale: locale, toastApi } = presentation.current;
    const copy = getDesktopConversationCopy(locale).actions;
    toastApi.error(copy.refreshSessionsFailedTitle,
      localizedShellErrorMessage(error, copy.refreshSessionsFailedFallback, locale));
  }), [catalog]);
  return { sessions, catalogRevision, authoritativeSessionIds, sessionsRef, ...actions };
}
