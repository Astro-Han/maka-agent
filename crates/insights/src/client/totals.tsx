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

import type { UsageSummary, UsageTokens } from '@maka-agent/plugin-sdk/host';
import type { Tab } from './model.js';

export const number = (value: number) =>
  value.toLocaleString('en-US', { maximumFractionDigits: 6 });
export const dollars = (value: number) => '$' + number(value);
export function Tokens({ value, zh }: { value: UsageTokens; zh: boolean }) {
  return (
    <span
      title={
        zh ? `${value.missing} 次调用缺少此计数` : `${value.missing} calls omitted this counter`
      }
    >
      {number(value.known)}
      {value.missing ? ' + ?' : ''}
    </span>
  );
}
function Cost({ value, zh }: { value: UsageSummary['models']['cost']; zh: boolean }) {
  return (
    <span
      title={
        zh
          ? `${value.unvalued} 次未估价，其中 ${value.unpriced} 次没有报价`
          : `${value.unvalued} unvalued calls; ${value.unpriced} without rates`
      }
    >
      {dollars(value.knownUsd)}
      {value.unvalued ? ' + ?' : ''}
    </span>
  );
}

export function Totals({ summary, tab, zh }: { summary: UsageSummary; tab: Tab; zh: boolean }) {
  const model = summary.models;
  const cacheRatio =
    model.input.missing === 0 &&
    model.cacheRead.missing === 0 &&
    model.input.known > 0 &&
    model.cacheRead.known <= model.input.known
      ? model.cacheRead.known / model.input.known
      : undefined;
  if (tab === 'overview')
    return (
      <>
        <div className="insights-cards">
          <div>
            <small>{zh ? '模型调用' : 'Model calls'}</small>
            <strong>{number(model.calls)}</strong>
          </div>
          <div>
            <small>{zh ? '已估价费用' : 'Valued cost'}</small>
            <strong>
              <Cost value={model.cost} zh={zh} />
            </strong>
          </div>
          <div>
            <small>{zh ? '输入 token' : 'Input tokens'}</small>
            <strong>
              <Tokens value={model.input} zh={zh} />
            </strong>
          </div>
          <div>
            <small>{zh ? '输出 token' : 'Output tokens'}</small>
            <strong>
              <Tokens value={model.output} zh={zh} />
            </strong>
          </div>
          <div>
            <small>{zh ? '工具尝试' : 'Tool attempts'}</small>
            <strong>{number(summary.tools.calls)}</strong>
          </div>
        </div>
        <p>
          {zh ? '成功 / 失败 / 取消 / 未知：' : 'Success / error / cancelled / unknown: '}
          {[model.success, model.error, model.aborted, model.unknown].map(number).join(' / ')}
        </p>
        <dl className="insights-facts">
          <dt>{zh ? '缓存命中率' : 'Cache hit rate'}</dt>
          <dd>{cacheRatio === undefined ? '?' : number(cacheRatio * 100) + '%'}</dd>
          <dt>{zh ? '模型累计耗时' : 'Cumulative model time'}</dt>
          <dd>
            {number(model.durationMs)} ms{model.unknown ? ' + ?' : ''}
          </dd>
          <dt>{zh ? '工具累计耗时' : 'Cumulative tool time'}</dt>
          <dd>
            {number(summary.tools.durationMs)} ms{summary.tools.unknown ? ' + ?' : ''}
          </dd>
          <dt>{zh ? '缓存读取' : 'Cache read'}</dt>
          <dd>
            <Tokens value={model.cacheRead} zh={zh} />
          </dd>
          <dt>{zh ? '缓存写入' : 'Cache write'}</dt>
          <dd>
            <Tokens value={model.cacheWrite} zh={zh} />
          </dd>
          <dt>{zh ? '推理 token' : 'Reasoning tokens'}</dt>
          <dd>
            <Tokens value={model.reasoning} zh={zh} />
          </dd>
          <dt>{zh ? '待结算模型 / 工具' : 'Pending models / tools'}</dt>
          <dd>
            {summary.pending.models} / {summary.pending.tools}
          </dd>
          <dt>{zh ? '未估价 / 无报价' : 'Unvalued / unpriced'}</dt>
          <dd>
            {model.cost.unvalued} / {model.cost.unpriced}
          </dd>
        </dl>
        <p className="insights-note">
          {zh
            ? '“+ ?” 表示部分数据未知，不等于零。累计耗时可能因并行调用而重叠，不是会话经过时间。已完成统计按结算时间，待结算数按准入时间。修改报价不改变历史费用。'
            : '“+ ?” means some data is unknown, not zero. Cumulative durations may overlap; they are not elapsed session time. Completed totals use settlement time; pending counts use admission time. Rate edits do not change historical costs.'}
        </p>
      </>
    );
  if (tab === 'tools')
    return (
      <div className="insights-table">
        <table>
          <thead>
            <tr>
              {[
                zh ? '工具' : 'Tool',
                zh ? '尝试' : 'Attempts',
                zh ? '成功' : 'Success',
                zh ? '失败' : 'Error',
                zh ? '拒绝' : 'Rejected',
                zh ? '未知' : 'Unknown',
                zh ? '平均耗时' : 'Mean duration',
              ].map((label) => (
                <th key={label}>{label}</th>
              ))}
            </tr>
          </thead>
          <tbody>
            {summary.byTool.map((row) => (
              <tr key={row.name}>
                <td>{row.name}</td>
                <td>{row.totals.calls}</td>
                <td>{row.totals.success}</td>
                <td>{row.totals.error}</td>
                <td>{row.totals.rejected}</td>
                <td>{row.totals.unknown}</td>
                <td>
                  {row.totals.meanLatencyMs === null
                    ? '—'
                    : number(row.totals.meanLatencyMs) + ' ms'}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    );
  const rows =
    tab === 'providers'
      ? summary.byProvider.map((row) => ({
          key: JSON.stringify(row.providerId),
          name: row.providerId ?? (zh ? '未知提供商' : 'Unknown provider'),
          totals: row.totals,
        }))
      : summary.byModel.map((row) => ({ key: row.modelId, name: row.modelId, totals: row.totals }));
  return (
    <div className="insights-table">
      <table>
        <thead>
          <tr>
            {[
              tab === 'providers' ? (zh ? '提供商' : 'Provider') : zh ? '模型' : 'Model',
              zh ? '调用' : 'Calls',
              zh ? '输入 token' : 'Input tokens',
              zh ? '输出 token' : 'Output tokens',
              zh ? '费用' : 'Cost',
            ].map((label) => (
              <th key={label}>{label}</th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((row) => (
            <tr key={row.key}>
              <td>{row.name}</td>
              <td>{row.totals.calls}</td>
              <td>
                <Tokens value={row.totals.input} zh={zh} />
              </td>
              <td>
                <Tokens value={row.totals.output} zh={zh} />
              </td>
              <td>
                <Cost value={row.totals.cost} zh={zh} />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
