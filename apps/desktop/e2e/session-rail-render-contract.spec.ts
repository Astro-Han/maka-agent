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

import { expect } from '@playwright/test';
import {
  ensureSidebarExpanded,
  RAIL_RENDER_SESSION_COUNT,
  test,
} from './fixtures.js';

/**
 * Switching a session moves one row's selection. What it must not do is rewrite
 * the rest of the rail.
 *
 * Deliberately a budget on the OUTCOME rather than an assertion about
 * identities or `memo`. The rail's cost has had several independent causes —
 * `setActiveId` changing identity every AppShell render, `Intl` formatters
 * rebuilt per row, catalog refreshes replacing unchanged row objects — and each
 * was invisible to the others. A DOM-write budget catches all of them and the
 * ones not yet found, including anything that raises the number of commits a
 * switch produces (#4109).
 *
 * The budget counts inline `style` writes on rail buttons because that is the
 * dominant term: every Astryx button removes and re-adds its `anchor-name` per
 * render, so one wasted rail render is two style writes per button plus the
 * style recalculation they force.
 *
 * Measured on this fixture: 4 writes when the rail behaves — the leaving row
 * and the arriving row, two each — against 336 with `setActiveId` restored to a
 * per-render identity. The budget sits an order of magnitude clear of both, so
 * it fails on a regression rather than on scheduling noise.
 */
const RAIL_STYLE_WRITE_BUDGET = 3 * RAIL_RENDER_SESSION_COUNT;

test('switching sessions does not rewrite the whole Session rail', async ({
  railRenderWindow: page,
}) => {
  await ensureSidebarExpanded(page);

  const rows = page.locator('.maka-session-row');
  await expect(rows).toHaveCount(RAIL_RENDER_SESSION_COUNT);

  const target = page.locator('.maka-session-row button.astryx-side-nav-item', {
    hasText: 'Rail row 3',
  });
  const selected = page.locator('.maka-session-row button.astryx-side-nav-item.selected');
  await expect(target).toBeVisible();

  // Settle first: the budget is about a switch, not about arriving.
  await page.waitForTimeout(500);

  await page.evaluate(() => {
    const counter = { styleWrites: 0 };
    (window as unknown as { __railWrites: typeof counter }).__railWrites = counter;
    const observer = new MutationObserver((records) => {
      for (const record of records) {
        const target = record.target as Element;
        if (!target.closest?.('.maka-session-row')) continue;
        counter.styleWrites += 1;
      }
    });
    observer.observe(document.body, {
      subtree: true,
      attributes: true,
      attributeFilter: ['style'],
    });
    (window as unknown as { __railObserver: MutationObserver }).__railObserver = observer;
  });

  await target.click();
  await expect(selected).toHaveText(/Rail row 3/);
  // Let the post-switch commit cascade finish before reading the counter.
  await page.waitForTimeout(1500);

  const styleWrites = await page.evaluate(() => {
    const scope = window as unknown as {
      __railWrites: { styleWrites: number };
      __railObserver: MutationObserver;
    };
    scope.__railObserver.disconnect();
    return scope.__railWrites.styleWrites;
  });

  expect(
    styleWrites,
    `rail inline-style writes for one session switch (${RAIL_RENDER_SESSION_COUNT} rows)`,
  ).toBeLessThanOrEqual(RAIL_STYLE_WRITE_BUDGET);
});
