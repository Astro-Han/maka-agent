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
import { copy } from './activity-copy.js';

import type { Activity, UsagePage } from '@maka-agent/plugin-sdk/host';
import { dollars, number } from './totals.js';

function detail(row: Activity, locale: ClientLocale) {
  const t = copy[locale];
  if (row.kind === 'model') {
    const attempt = row.attempt;
    return (
      <>
        <strong>{attempt.modelId}</strong>
        <small>
          {attempt.quote?.providerId ?? t.unknownProvider}
          {attempt.binding ? ' / ' + attempt.binding.connection_slug : ''}
        </small>
      </>
    );
  }
  return (
    <>
      <strong>{row.attempt.name}</strong>
      <small>{row.attempt.call.origin.kind}</small>
    </>
  );
}

export function ActivityTable({
  page,
  locale,
  onOpenSession,
}: {
  page: UsagePage;
  locale: ClientLocale;
  onOpenSession?: (id: string) => void;
}) {
  const t = copy[locale];
  const status = {
    success: t.success,
    error: t.error,
    aborted: t.cancelled,
    unknown: t.unknown,
    rejected: t.rejected,
  };
  return (
    <div className="insights-table">
      <table>
        <thead>
          <tr>
            {[t.settled, t.modelOrTool, t.session, t.outcome, t.tokens, t.cost].map((label) => (
              <th key={label}>{label}</th>
            ))}
          </tr>
        </thead>
        <tbody>
          {page.attempts.map((row) => {
            const attempt = row.attempt;
            const session =
              row.kind === 'model' ? row.attempt.sessionId : row.attempt.invocation.session_id;
            const outcome =
              row.kind === 'model'
                ? row.attempt.outcome
                : row.attempt.result.kind === 'rejected'
                  ? 'rejected'
                  : row.attempt.result.outcome;
            return (
              <tr key={attempt.requestId}>
                <td>{new Date(attempt.completedAt).toLocaleString(locale)}</td>
                <td>{detail(row, locale)}</td>
                <td>
                  {session && onOpenSession ? (
                    <button type="button" onClick={() => onOpenSession(session)}>
                      {session}
                    </button>
                  ) : (
                    (session ?? 'Host')
                  )}
                </td>
                <td>
                  {status[outcome]}
                  {row.kind === 'tool' && row.attempt.result.kind === 'rejected' ? (
                    <small>{row.attempt.result.reason}</small>
                  ) : null}
                </td>
                <td>
                  {row.kind === 'model'
                    ? `${row.attempt.usage.input_tokens == null ? '?' : number(row.attempt.usage.input_tokens, locale)} / ${row.attempt.usage.output_tokens == null ? '?' : number(row.attempt.usage.output_tokens, locale)}`
                    : '—'}
                </td>
                <td>
                  {row.kind === 'model'
                    ? row.attempt.costUsd === null
                      ? '?'
                      : dollars(row.attempt.costUsd, locale)
                    : '—'}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
      {page.total === 0 ? <p>{t.empty}</p> : null}
    </div>
  );
}
