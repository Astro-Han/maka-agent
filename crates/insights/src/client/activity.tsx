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

import type { Activity, UsagePage } from '@maka-agent/plugin-sdk/host';
import { dollars, number } from './totals.js';

function detail(row: Activity, zh: boolean) {
  if (row.kind === 'model') {
    const attempt = row.attempt;
    return (
      <>
        <strong>{attempt.modelId}</strong>
        <small>
          {attempt.quote?.providerId ?? (zh ? '未知提供商' : 'Unknown provider')}
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
  zh,
  onOpenSession,
}: {
  page: UsagePage;
  zh: boolean;
  onOpenSession?: (id: string) => void;
}) {
  const status = {
    success: zh ? '成功' : 'Success',
    error: zh ? '失败' : 'Error',
    aborted: zh ? '取消' : 'Cancelled',
    unknown: zh ? '未知' : 'Unknown',
    rejected: zh ? '拒绝' : 'Rejected',
  };
  return (
    <div className="insights-table">
      <table>
        <thead>
          <tr>
            {[
              zh ? '结算时间' : 'Settled',
              zh ? '模型 / 工具' : 'Model / tool',
              zh ? 'Session' : 'Session',
              zh ? '结果' : 'Outcome',
              zh ? '输入 / 输出 token' : 'Input / output tokens',
              zh ? '费用' : 'Cost',
            ].map((label) => (
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
                <td>{new Date(attempt.completedAt).toLocaleString()}</td>
                <td>{detail(row, zh)}</td>
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
                    ? `${row.attempt.usage.input_tokens == null ? '?' : number(row.attempt.usage.input_tokens)} / ${row.attempt.usage.output_tokens == null ? '?' : number(row.attempt.usage.output_tokens)}`
                    : '—'}
                </td>
                <td>
                  {row.kind === 'model'
                    ? row.attempt.costUsd === null
                      ? '?'
                      : dollars(row.attempt.costUsd)
                    : '—'}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
      {page.total === 0 ? (
        <p>{zh ? '此快照没有匹配的活动。' : 'No matching activity in this snapshot.'}</p>
      ) : null}
    </div>
  );
}
