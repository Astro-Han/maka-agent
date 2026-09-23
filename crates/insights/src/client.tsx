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

import { useEffect, useMemo, useState } from 'react';
import type { ClientContext, ClientPlugin, ClientSlots } from '@maka-agent/plugin-sdk/client';
import type { UsageSelection } from '@maka-agent/plugin-sdk/host';
import {
  api as connect,
  type Preferences,
  type Range,
  type Snapshot,
  type Tab,
} from './client/model.js';
import { useReport } from './client/report.js';
import { ActivityTable } from './client/activity.js';
import { Totals } from './client/totals.js';
import { Pricing } from './client/pricing.js';
import { Inspector } from './client/inspector.js';
import { style } from './client/style.js';

function Insights({
  context,
  locale,
  onOpenSession,
}: ClientSlots['settings.page'] & { context: ClientContext }) {
  const zh = locale !== 'en';
  const api = useMemo(() => connect(context), [context]);
  const report = useReport(api, context.signal);
  const refresh = report.refresh;
  const [snapshot, setSnapshot] = useState<Snapshot>();
  const [view, setView] = useState<Preferences>();
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');
  const [reload, setReload] = useState(0);
  useEffect(() => {
    let live = true;
    setSnapshot(undefined);
    setView(undefined);
    setError('');
    void api
      .preferences()
      .then((saved) => {
        if (!live || context.signal.aborted) return;
        setSnapshot(saved);
        setView(saved.preferences);
        if (saved.preferences.tab !== 'pricing')
          void refresh(saved.preferences.range, saved.preferences.selection);
      })
      .catch((reason) => {
        if (live) setError(String(reason));
      });
    return () => {
      live = false;
    };
  }, [api, context, refresh, reload]);

  async function saveView() {
    if (!view || !snapshot || saving) return;
    setSaving(true);
    setError('');
    try {
      const saved = await api.save(snapshot, view);
      if (!context.signal.aborted) setSnapshot(saved);
    } catch (reason) {
      if (!context.signal.aborted) setError(String(reason));
    } finally {
      if (!context.signal.aborted) setSaving(false);
    }
  }
  const tabs: readonly [Tab, string][] = [
    ['overview', zh ? '概览' : 'Overview'],
    ['activity', zh ? '活动' : 'Activity'],
    ['providers', zh ? '提供商' : 'Providers'],
    ['models', zh ? '模型' : 'Models'],
    ['tools', zh ? '工具' : 'Tools'],
    ['pricing', zh ? '报价' : 'Pricing'],
  ];
  function select(selection: UsageSelection) {
    if (view) setView({ ...view, selection });
  }
  return (
    <section data-maka-insights>
      <header>
        <h2>{zh ? '用量与报价' : 'Usage & pricing'}</h2>
        <button
          type="button"
          disabled={saving || report.busy}
          onClick={() => setReload((value) => value + 1)}
        >
          {zh ? '载入已保存视图' : 'Load saved view'}
        </button>
      </header>
      {error && <p role="alert">{error}</p>}
      {!view ? (
        !error && <p role="status">{zh ? '正在读取视图…' : 'Loading view…'}</p>
      ) : (
        <>
          <nav aria-label={zh ? '统计页面' : 'Usage pages'}>
            {tabs.map(([tab, label]) => (
              <button
                type="button"
                key={tab}
                aria-current={view.tab === tab ? 'page' : undefined}
                onClick={() => {
                  setView({ ...view, tab });
                  if (tab !== 'pricing' && !report.page && !report.busy)
                    void report.refresh(view.range, view.selection);
                }}
              >
                {label}
              </button>
            ))}
          </nav>
          <div className="insights-actions">
            <label>
              {zh ? '范围' : 'Range'}
              <select
                value={view.range}
                disabled={report.busy}
                onChange={(event) => {
                  const range = event.target.value as Range;
                  setView({ ...view, range });
                  void report.refresh(range, view.selection);
                }}
              >
                {(['24h', '7d', '30d', 'all'] as const).map((range, index) => (
                  <option key={range} value={range}>
                    {
                      (zh
                        ? ['24 小时', '7 天', '30 天', '全部时间']
                        : ['24 hours', '7 days', '30 days', 'All time'])[index]
                    }
                  </option>
                ))}
              </select>
            </label>
            {view.tab !== 'pricing' && (
              <button
                type="button"
                disabled={report.busy}
                onClick={() => void report.refresh(view.range, view.selection)}
              >
                {zh ? '刷新快照' : 'Refresh snapshot'}
              </button>
            )}
            <button type="button" disabled={saving} onClick={() => void saveView()}>
              {zh ? '保存此视图' : 'Save this view'}
            </button>
          </div>
          {view.tab === 'pricing' ? (
            <Pricing api={api} signal={context.signal} zh={zh} />
          ) : (
            <>
              {report.error && <p role="alert">{report.error}</p>}
              {report.busy && <p role="status">{zh ? '正在读取快照…' : 'Reading snapshot…'}</p>}
              {view.tab === 'activity' ? (
                <>
                  <form
                    className="insights-actions"
                    onSubmit={(event) => {
                      event.preventDefault();
                      void report.refine(view.selection);
                    }}
                  >
                    <label>
                      {zh ? '类型' : 'Kind'}
                      <select
                        value={view.selection.kind ?? ''}
                        onChange={(event) =>
                          select({
                            ...view.selection,
                            kind: (event.target.value || null) as UsageSelection['kind'],
                          })
                        }
                      >
                        <option value="">{zh ? '全部' : 'All'}</option>
                        <option value="model">{zh ? '模型' : 'Model'}</option>
                        <option value="tool">{zh ? '工具' : 'Tool'}</option>
                      </select>
                    </label>
                    <label>
                      {zh ? '结果' : 'Outcome'}
                      <select
                        value={view.selection.status ?? ''}
                        onChange={(event) =>
                          select({
                            ...view.selection,
                            status: (event.target.value || null) as UsageSelection['status'],
                          })
                        }
                      >
                        {['', 'success', 'error', 'aborted', 'unknown', 'rejected'].map(
                          (status, index) => (
                            <option key={status} value={status}>
                              {
                                (zh
                                  ? ['全部', '成功', '失败', '取消', '未知', '拒绝']
                                  : [
                                      'All',
                                      'Success',
                                      'Error',
                                      'Cancelled',
                                      'Unknown',
                                      'Rejected',
                                    ])[index]
                              }
                            </option>
                          ),
                        )}
                      </select>
                    </label>
                    <label>
                      {zh ? '搜索' : 'Search'}
                      <input
                        value={view.selection.search ?? ''}
                        onChange={(event) =>
                          select({ ...view.selection, search: event.target.value })
                        }
                      />
                    </label>
                    <button type="submit" disabled={report.busy || !report.page}>
                      {zh ? '应用筛选' : 'Apply filters'}
                    </button>
                  </form>
                  <p className="insights-note">
                    {zh
                      ? '筛选仅作用于活动列表；概览和分组仍使用同一快照的完整统计。'
                      : 'Filters affect this list only. Overview and breakdowns keep complete totals from the same snapshot.'}
                  </p>
                  {report.page && (
                    <>
                      <ActivityTable
                        page={report.page}
                        zh={zh}
                        onOpenSession={
                          onOpenSession
                            ? (id) => {
                                void onOpenSession(id).catch((reason) => setError(String(reason)));
                              }
                            : undefined
                        }
                      />
                      <div className="insights-actions">
                        <span>
                          {report.page.total} {zh ? '条匹配活动' : 'matching activities'}
                        </span>
                        <button
                          type="button"
                          disabled={report.busy || report.history.length < 2}
                          onClick={() => void report.back()}
                        >
                          {zh ? '上一页' : 'Previous'}
                        </button>
                        <button
                          type="button"
                          disabled={report.busy || !report.page.nextCursor}
                          onClick={() => void report.next()}
                        >
                          {zh ? '下一页' : 'Next'}
                        </button>
                      </div>
                    </>
                  )}
                </>
              ) : (
                report.summary && <Totals summary={report.summary} tab={view.tab} zh={zh} />
              )}
            </>
          )}
        </>
      )}
    </section>
  );
}

const plugin: ClientPlugin = {
  activate(context) {
    context.style(style);
    context.slots.register('session.inspector.overview', 'usage', (props) => (
      <Inspector {...props} context={context} />
    ));
    context.slots.register(
      'settings.page',
      'usage',
      (props) => <Insights {...props} context={context} />,
      { label: { en: 'Usage & pricing', 'zh-CN': '用量与报价', 'zh-TW': '用量與報價' }, order: 30 },
    );
  },
};
export default plugin;
