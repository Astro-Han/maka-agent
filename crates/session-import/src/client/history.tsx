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

import type { Copies, Copy } from './model.js';
export function History({
  page,
  busy,
  zh,
  next,
  settle,
  open,
}: {
  page: Copies;
  busy: boolean;
  zh: boolean;
  next: (after: string | null) => void;
  settle: (copy: Copy, abandon: boolean) => void;
  open?: (id: string) => void;
}) {
  return (
    <section aria-label={zh ? '导入记录' : 'Import history'}>
      <header>
        <h3>{zh ? '导入记录' : 'Import history'}</h3>
        <button type="button" disabled={busy} onClick={() => next(null)}>
          {zh ? '刷新' : 'Refresh'}
        </button>
      </header>
      {!page.copies.length && <p>{zh ? '尚无导入记录' : 'No imports yet'}</p>}
      <ul>
        {page.copies.map((copy) => (
          <li key={copy.operationId}>
            <strong>{copy.title}</strong>{' '}
            <span>
              {copy.sourceName} · {copy.records} {zh ? '条历史记录' : 'historical records'}
            </span>
            {!copy.receipt ? (
              <>
                <span>
                  {zh ? '等待交付或确认回执' : 'Awaiting delivery or receipt reconciliation'}
                </span>
                <button type="button" disabled={busy} onClick={() => settle(copy, false)}>
                  {zh ? '继续／确认' : 'Continue / reconcile'}
                </button>
                <button type="button" disabled={busy} onClick={() => settle(copy, true)}>
                  {zh ? '放弃导入' : 'Abandon import'}
                </button>
              </>
            ) : copy.receipt.state === 'published' ? (
              <>
                <span>{zh ? '已发布' : 'Published'}</span>
                {open && (
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => open(copy.receipt!.sessionId)}
                  >
                    {zh ? '打开会话' : 'Open Session'}
                  </button>
                )}
              </>
            ) : (
              <span>{zh ? '已放弃' : 'Abandoned'}</span>
            )}
          </li>
        ))}
      </ul>
      {page.next && (
        <button type="button" disabled={busy} onClick={() => next(page.next)}>
          {zh ? '下一页' : 'Next page'}
        </button>
      )}
    </section>
  );
}
