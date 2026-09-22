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

import { valuesEqual } from '@maka/ui';
import { useCallback } from 'react';
import type { SessionCatalogController, SessionCatalogState } from '../../../application/contracts/session-catalog/session-catalog-state.js';
import { useExternalStoreSelector } from '../../../application/contracts/session-catalog/use-external-store-selector.js';
import { deriveBranchBanner, type BranchBanner } from '../model/branch-banner.js';
import { sessionMatchesRail } from '../model/session-nav-filter.js';
import { deriveSessionRail } from '../../../application/contracts/session-catalog/session-rail.js';
import {
  selectRailLayout,
  sessionRailLayoutStore,
  type SessionRailLayoutState,
} from '../model/session-rail-layout-store.js';
import {
  deriveSessionRevisionNavigation,
  type SessionRevisionNavigation,
} from '../model/session-revisions.js';
import type { SessionNavigationSession } from '../ports.js';

export interface SessionNavigationReads {
  activeParentSession: { id: string; name: string } | undefined;
  branchBanner: BranchBanner | undefined;
  revisionNavigation: SessionRevisionNavigation | undefined;
  layout: SessionRailLayoutState;
}

/** Only breadcrumb, revision navigation and geometry reach the shell. */
export function useSessionNavigationReads(input: {
  catalog: SessionCatalogController;
  activeSessionId: string | undefined;
  activeSession: SessionNavigationSession | undefined;
  hiddenSessionIds: ReadonlySet<string>;
}): SessionNavigationReads {
  const { activeSession, activeSessionId, hiddenSessionIds, catalog } = input;
  const select = useCallback(({ sessions }: SessionCatalogState) => {
    const { activeParentSession: parent } = deriveSessionRail(sessions, activeSessionId,
      (session) => !hiddenSessionIds.has(session.id) && sessionMatchesRail(session));
    return {
      activeParentSession: parent ? { id: parent.id, name: parent.name } : undefined,
      branchBanner: deriveBranchBanner(activeSession, sessions),
      revisionNavigation: deriveSessionRevisionNavigation(sessions, activeSessionId),
    };
  }, [activeSession, activeSessionId, hiddenSessionIds]);
  const facts = useExternalStoreSelector(catalog, select, undefined, valuesEqual);
  const layout = useExternalStoreSelector(sessionRailLayoutStore, selectRailLayout);
  return { ...facts, layout };
}
