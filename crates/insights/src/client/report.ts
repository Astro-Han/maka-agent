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

import { useCallback, useEffect, useRef, useState } from 'react';
import type {
  UsagePage,
  UsageRead,
  UsageSelection,
  UsageSummary,
} from '@maka-agent/plugin-sdk/host';
import { type Api, type Range, rangeBounds } from './model.js';

type Report = {
  page?: UsagePage;
  summary?: UsageSummary;
  history: readonly string[];
  busy: boolean;
  error: string;
};

export function useReport(api: Api, signal: AbortSignal) {
  const [report, setReport] = useState<Report>({ history: [], busy: false, error: '' });
  const version = useRef(0);
  const current = useRef(report);
  current.current = report;
  useEffect(
    () => () => {
      version.current++;
    },
    [api],
  );
  const load = useCallback(
    async (read: UsageRead, mode: 'fresh' | 'refine' | 'next' | 'back') => {
      const ticket = ++version.current;
      const before = current.current;
      setReport({
        ...(mode === 'fresh' ? { history: [] } : before),
        page: undefined,
        busy: true,
        error: '',
      });
      try {
        const page = await api.activity(read);
        if (ticket !== version.current || signal.aborted) return;
        const history =
          mode === 'next'
            ? [...before.history, page.cursor]
            : mode === 'back'
              ? before.history.slice(0, -1)
              : [page.cursor];
        setReport({
          page,
          summary: mode === 'fresh' ? undefined : before.summary,
          history,
          busy: true,
          error: '',
        });
        const summary = mode === 'fresh' ? await api.summary(page.cursor) : before.summary;
        if (ticket !== version.current || signal.aborted) return;
        setReport({ page, summary, history, busy: false, error: '' });
      } catch (error) {
        if (ticket === version.current && !signal.aborted)
          setReport((state) => ({
            ...state,
            busy: false,
            error: error instanceof Error ? error.message : String(error),
          }));
      }
    },
    [api, signal],
  );
  const refresh = useCallback(
    (range: Range, selection: UsageSelection) =>
      load({ kind: 'start', filter: { ...rangeBounds(range), activity: selection } }, 'fresh'),
    [load],
  );
  return {
    ...report,
    refresh,
    refine: (selection: UsageSelection) => {
      const cursor = current.current.page?.cursor;
      return cursor ? load({ kind: 'refine', cursor, selection }, 'refine') : Promise.resolve();
    },
    next: () => {
      const cursor = current.current.page?.nextCursor;
      return cursor ? load({ kind: 'continue', cursor }, 'next') : Promise.resolve();
    },
    back: () => {
      const cursor = current.current.history.at(-2);
      return cursor ? load({ kind: 'continue', cursor }, 'back') : Promise.resolve();
    },
  };
}
