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

import type { ClientLocale } from '@maka-agent/plugin-sdk/client';
import { copy } from './history-copy.js';

import type { Copies, Copy } from './model.js';
export function History({
  page,
  busy,
  locale,
  next,
  settle,
  open,
}: {
  page: Copies;
  busy: boolean;
  locale: ClientLocale;
  next: (after: string | null) => void;
  settle: (copy: Copy, abandon: boolean) => void;
  open?: (id: string) => void;
}) {
  const t = copy[locale];
  return (
    <section aria-label={t.title}>
      <header>
        <h3>{t.title}</h3>
        <button type="button" disabled={busy} onClick={() => next(null)}>
          {t.refresh}
        </button>
      </header>
      {!page.copies.length && <p>{t.empty}</p>}
      <ul>
        {page.copies.map((copy) => (
          <li key={copy.operationId}>
            <strong>{copy.title}</strong>{' '}
            <span>
              {copy.sourceName} · {copy.records} {t.records}
            </span>
            {!copy.receipt ? (
              <>
                <span>{t.awaiting}</span>
                <button type="button" disabled={busy} onClick={() => settle(copy, false)}>
                  {t.reconcile}
                </button>
                <button type="button" disabled={busy} onClick={() => settle(copy, true)}>
                  {t.abandon}
                </button>
              </>
            ) : copy.receipt.state === 'published' ? (
              <>
                <span>{t.published}</span>
                {open && (
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => open(copy.receipt!.sessionId)}
                  >
                    {t.open}
                  </button>
                )}
              </>
            ) : (
              <span>{t.abandoned}</span>
            )}
          </li>
        ))}
      </ul>
      {page.next && (
        <button type="button" disabled={busy} onClick={() => next(page.next)}>
          {t.next}
        </button>
      )}
    </section>
  );
}
