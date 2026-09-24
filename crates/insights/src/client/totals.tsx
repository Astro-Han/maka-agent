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
import { copy } from './totals-copy.js';

import type { UsageSummary, UsageTokens } from '@maka-agent/plugin-sdk/host';
import type { Tab } from './model.js';

export const number = (value: number, locale: ClientLocale) =>
  value.toLocaleString(locale, { maximumFractionDigits: 6 });
export const dollars = (value: number, locale: ClientLocale) => '$' + number(value, locale);
export function Tokens({ value, locale }: { value: UsageTokens; locale: ClientLocale }) {
  const t = copy[locale];
  return (
    <span title={t.missingCounter(value.missing)}>
      {number(value.known, locale)}
      {value.missing ? ' + ?' : ''}
    </span>
  );
}
function Cost({ value, locale }: { value: UsageSummary['models']['cost']; locale: ClientLocale }) {
  const t = copy[locale];
  return (
    <span title={t.unvaluedCalls(value.unvalued, value.unpriced)}>
      {dollars(value.knownUsd, locale)}
      {value.unvalued ? ' + ?' : ''}
    </span>
  );
}

export function Totals({
  summary,
  tab,
  locale,
}: {
  summary: UsageSummary;
  tab: Tab;
  locale: ClientLocale;
}) {
  const t = copy[locale];
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
            <small>{t.modelCalls}</small>
            <strong>{number(model.calls, locale)}</strong>
          </div>
          <div>
            <small>{t.valuedCost}</small>
            <strong>
              <Cost value={model.cost} locale={locale} />
            </strong>
          </div>
          <div>
            <small>{t.inputTokens}</small>
            <strong>
              <Tokens value={model.input} locale={locale} />
            </strong>
          </div>
          <div>
            <small>{t.outputTokens}</small>
            <strong>
              <Tokens value={model.output} locale={locale} />
            </strong>
          </div>
          <div>
            <small>{t.toolAttempts}</small>
            <strong>{number(summary.tools.calls, locale)}</strong>
          </div>
        </div>
        <p>
          {t.outcomes}
          {[model.success, model.error, model.aborted, model.unknown]
            .map((value) => number(value, locale))
            .join(' / ')}
        </p>
        <dl className="insights-facts">
          <dt>{t.cacheHit}</dt>
          <dd>{cacheRatio === undefined ? '?' : number(cacheRatio * 100, locale) + '%'}</dd>
          <dt>{t.modelTime}</dt>
          <dd>
            {number(model.durationMs, locale)} ms{model.unknown ? ' + ?' : ''}
          </dd>
          <dt>{t.toolTime}</dt>
          <dd>
            {number(summary.tools.durationMs, locale)} ms{summary.tools.unknown ? ' + ?' : ''}
          </dd>
          <dt>{t.cacheRead}</dt>
          <dd>
            <Tokens value={model.cacheRead} locale={locale} />
          </dd>
          <dt>{t.cacheWrite}</dt>
          <dd>
            <Tokens value={model.cacheWrite} locale={locale} />
          </dd>
          <dt>{t.reasoningTokens}</dt>
          <dd>
            <Tokens value={model.reasoning} locale={locale} />
          </dd>
          <dt>{t.pending}</dt>
          <dd>
            {summary.pending.models} / {summary.pending.tools}
          </dd>
          <dt>{t.unvalued}</dt>
          <dd>
            {model.cost.unvalued} / {model.cost.unpriced}
          </dd>
        </dl>
        <p className="insights-note">{t.uncertainty}</p>
      </>
    );
  if (tab === 'tools')
    return (
      <div className="insights-table">
        <table>
          <thead>
            <tr>
              {[t.tool, t.attempts, t.success, t.error, t.rejected, t.unknown, t.meanDuration].map(
                (label) => (
                  <th key={label}>{label}</th>
                ),
              )}
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
                    : number(row.totals.meanLatencyMs, locale) + ' ms'}
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
          name: row.providerId ?? t.unknownProvider,
          totals: row.totals,
        }))
      : summary.byModel.map((row) => ({ key: row.modelId, name: row.modelId, totals: row.totals }));
  return (
    <div className="insights-table">
      <table>
        <thead>
          <tr>
            {[
              tab === 'providers' ? t.provider : t.model,
              t.calls,
              t.inputTokens,
              t.outputTokens,
              t.cost,
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
                <Tokens value={row.totals.input} locale={locale} />
              </td>
              <td>
                <Tokens value={row.totals.output} locale={locale} />
              </td>
              <td>
                <Cost value={row.totals.cost} locale={locale} />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
