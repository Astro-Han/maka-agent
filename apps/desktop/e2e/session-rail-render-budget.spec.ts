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

/*
 * What one session switch is allowed to cost the Session rail (#4109).
 *
 * Result-shaped on purpose. The defect it guards is "the rail re-renders on
 * every AppShell commit", and the causes are many and will change: an unstable
 * callback, a new context, an effect cascade, a prop object rebuilt in the
 * shell's JSX. An assertion about any one of those goes stale the moment the
 * next cause arrives, so this measures the OUTCOME the user sees — how much of
 * the sidebar's DOM was touched — through a `MutationObserver`.
 *
 * Why DOM mutations stand in for renders: every Astryx button in the rail
 * rewrites its inline `anchor-name` on each render ("--a, --b" → "--b" →
 * "--a, --b"), so a rail render is also a burst of `style` writes. On this
 * fixture one switch used to produce 2,214 of them (2,000 grouped by project);
 * a rail that renders only for its own state change produces 272 (280). The
 * budget is set with headroom above the measured figure, so a change has to be
 * a regression in kind, not in noise.
 *
 * The second half is the timing half, and it is why this is not only a counter:
 * the switch must not flicker. The row carrying the selection changes exactly
 * once — no intermediate commit puts it on a third row and takes it back — and
 * the streaming / stale badges are not removed and re-added underneath it.
 */

import type { Page } from '@playwright/test';
import { expect, test } from './fixtures';

/**
 * Inline `style` writes inside the rail for one switch. Measured at 272–280
 * across both view modes, against 2,000–2,214 for a rail re-rendered by every
 * AppShell commit — so this sits well clear of the noise on one side and of the
 * defect on the other.
 */
const STYLE_WRITE_BUDGET = 450;

/** Total mutation records inside the rail for one switch (measured 278–286). */
const MUTATION_BUDGET = 500;

interface RailMutationReport {
  total: number;
  styleWrites: number;
  activeRowChanges: number;
  statusNodeChanges: number;
  activeRowIds: string[];
}

declare global {
  interface Window {
    __makaRailWatch?: {
      stop(): RailMutationReport;
    };
  }
}

async function revealSidebar(page: Page) {
  await page.keyboard.press('Escape');
  await expect(page.locator('[data-maka-contract="search-modal"]')).not.toBeVisible();
  const sidebar = page.getByRole('navigation', { name: '任务列表' });
  await expect(sidebar).toBeVisible();
  return sidebar;
}

/**
 * Watch the rail until `stop()`.
 *
 * `activeRowIds` records the selected row after every batch of records rather
 * than counting attribute writes, so a re-render that rewrites the same
 * selection is not mistaken for the selection moving — and a switch that lands
 * on a third row in between is.
 */
async function watchRail(page: Page): Promise<void> {
  await page.evaluate(() => {
    const rail = document.querySelector('nav.maka-session-panel');
    if (!rail) throw new Error('session rail is not mounted');

    const selectedRowId = (): string | null => {
      const row = rail.querySelector(
        '[data-maka-contract="session-row"]:has([aria-current]), ' +
          '[data-maka-contract="session-row"]:has([aria-selected="true"]), ' +
          '[data-maka-contract="session-row"]:has(.isSelected)',
      );
      return row?.getAttribute('data-session-id') ?? null;
    };

    const report: RailMutationReport = {
      total: 0,
      styleWrites: 0,
      activeRowChanges: 0,
      statusNodeChanges: 0,
      activeRowIds: [],
    };
    let lastActive = selectedRowId();
    report.activeRowIds.push(lastActive ?? '(none)');

    const observer = new MutationObserver((records) => {
      report.total += records.length;
      for (const record of records) {
        if (record.type === 'attributes' && record.attributeName === 'style') {
          report.styleWrites += 1;
        }
        if (record.type === 'childList') {
          const touched = [...record.addedNodes, ...record.removedNodes];
          for (const node of touched) {
            if (
              node instanceof Element &&
              (node.matches('[data-session-status]') ||
                node.querySelector?.('[data-session-status]'))
            ) {
              report.statusNodeChanges += 1;
            }
          }
        }
      }
      const active = selectedRowId();
      if (active !== lastActive) {
        lastActive = active;
        report.activeRowChanges += 1;
        report.activeRowIds.push(active ?? '(none)');
      }
    });
    observer.observe(rail, {
      subtree: true,
      childList: true,
      attributes: true,
      characterData: true,
    });

    window.__makaRailWatch = {
      stop() {
        observer.takeRecords();
        observer.disconnect();
        delete window.__makaRailWatch;
        return report;
      },
    };
  });
}

async function stopWatching(page: Page): Promise<RailMutationReport> {
  return page.evaluate(() => {
    const watch = window.__makaRailWatch;
    if (!watch) throw new Error('the rail watcher was never installed');
    return watch.stop();
  });
}

for (const viewMode of ['按时间', '按项目'] as const) {
  test(`one session switch stays inside the rail's render budget (${viewMode})`, async ({
    projectSidebarWindow: page,
  }) => {
    const sidebar = await revealSidebar(page);
    await sidebar.getByRole('radio', { name: viewMode, exact: true }).click();

    const rows = sidebar.locator('[data-maka-contract="session-row"]');
    await expect(rows.first()).toBeVisible();
    const rowCount = await rows.count();
    expect(rowCount).toBeGreaterThan(8);

    // Warm up: the first switch after boot also pays for the transcript's first
    // load, which is not what this budget is about.
    await rows.nth(1).locator('button').first().click();
    await page.waitForTimeout(2_000);

    const target = rows.nth(5);
    const targetId = await target.getAttribute('data-session-id');
    expect(targetId).toBeTruthy();

    await watchRail(page);
    await target.locator('button').first().click();
    // Long enough for the whole async cascade a switch sets off — the catalog
    // refresh, the message loads, the event-stream health updates. Each of them
    // used to arrive as its own full-tree render.
    await page.waitForTimeout(3_000);
    const report = await stopWatching(page);
    // eslint-disable-next-line no-console -- the numbers are the point of the run
    console.log(`[rail budget ${viewMode}]`, JSON.stringify(report));

    expect(report.styleWrites).toBeLessThanOrEqual(STYLE_WRITE_BUDGET);
    expect(report.total).toBeLessThanOrEqual(MUTATION_BUDGET);

    // The selection moves once, and it moves to the row that was clicked. Two
    // changes mean the rail showed an intermediate selection the user did not
    // ask for.
    expect(report.activeRowChanges).toBe(1);
    expect(report.activeRowIds.at(-1)).toBe(targetId);

    // Status dots belong to sessions whose state did not change. Rebuilding
    // them is the visible half of the rail re-rendering: the badges flash.
    expect(report.statusNodeChanges).toBe(0);
  });
}
