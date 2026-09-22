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

import type { SessionCatalogController } from '../../../application/contracts/session-catalog/session-catalog-state.js';

/** A not-yet-admitted draft is not a deleted Session. */
export function observeRevisionDraftRetirement(
  catalog: SessionCatalogController,
  draft: { sourceSessionId: string; draftSessionId: string },
  retire: () => void,
): () => void {
  const ids = new Set([draft.sourceSessionId, draft.draftSessionId]);
  const seen = new Set<string>();
  let retired = false;
  const observe = () => {
    if (retired) return;
    const rows = catalog.getState().sessions;
    for (const id of ids) {
      const row = rows.find((session) => session.id === id);
      if (row?.isArchived || (!row && seen.has(id))) {
        retired = true;
        retire();
        return;
      }
      if (row) seen.add(id);
    }
  };
  const unsubscribe = catalog.subscribe(observe);
  observe();
  return unsubscribe;
}
