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

import { createContext, useCallback, useContext, useMemo, type ReactNode } from 'react';
import type { ComposerSuggestion } from '@maka-agent/plugin-sdk/client';
import type { DesktopNewTaskTarget } from '../preload/bridge-contract.js';
import { ComposerSuggestionsProvider, useComposerSuggestions } from './features/client-plugins/index.js';

export interface ComposerMentions {
  suggestions: readonly ComposerSuggestion[];
  searchMentionFiles(query: string): Promise<ReadonlyArray<{ relativePath: string }>>;
}
export interface ComposerMentionsSurfaceInput {
  scope: string;
  sessionId?: string;
  newTaskTarget?: DesktopNewTaskTarget;
}
const Context = createContext<ComposerMentions | undefined>(undefined);

function Projection({ children, sessionId, newTaskTarget }: ComposerMentionsSurfaceInput & { children: ReactNode }) {
  const suggestions = useComposerSuggestions();
  const searchMentionFiles = useCallback(async (query: string) => {
    try {
      const result = sessionId
        ? await window.maka.workspace.searchFiles(query, { sessionId })
        : newTaskTarget
          ? await window.maka.newTasks.searchFiles(newTaskTarget, query)
          : { ok: false as const };
      return result.ok ? result.files : [];
    } catch { return []; }
  }, [sessionId, newTaskTarget?.profileId, newTaskTarget?.hostId, newTaskTarget?.projectId]);
  const value = useMemo(() => ({suggestions, searchMentionFiles}), [suggestions, searchMentionFiles]);
  return <Context.Provider value={value}>{children}</Context.Provider>;
}

/** A target switch replaces publications immediately, without remounting the draft. */
export function ComposerMentionsProvider(props: ComposerMentionsSurfaceInput & { children: ReactNode }) {
  return <ComposerSuggestionsProvider scope={props.scope}>
    <Projection {...props} />
  </ComposerSuggestionsProvider>;
}
export function useComposerMentionsContext() { return useContext(Context); }
