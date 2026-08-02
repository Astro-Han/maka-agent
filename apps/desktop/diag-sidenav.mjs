// Throwaway diagnostic harness for the four post-#1876 sidebar reports.
// Boots the real built app in a visible window and probes computed styles.
import { _electron as electron } from 'playwright';
import { mkdtemp } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { buildFixtureEnv } from '../../scripts/fixture-env.mjs';

const userDataDir = await mkdtemp(path.join(tmpdir(), 'maka-diag-'));
const homeDir = await mkdtemp(path.join(tmpdir(), 'maka-diag-home-'));
const env = buildFixtureEnv(userDataDir, homeDir, {
  scenario: 'sidebar-long-sessions',
  locale: 'zh',
  theme: 'light',
  showWindow: true,
});

const app = await electron.launch({ args: ['.'], env, cwd: process.cwd(), timeout: 60000 });
app.on('console', (m) => console.log('[main]', m.text()));
const page = await app.firstWindow();
page.on('pageerror', (e) => console.log('[pageerror]', e.message));
await page.waitForSelector('.maka-session-panel', { timeout: 60000 });
await page.waitForTimeout(2500);

const probe = await page.evaluate(() => {
  const q = (s) => document.querySelector(s);
  const cs = (el, props) =>
    el
      ? Object.fromEntries(props.map((p) => [p, getComputedStyle(el).getPropertyValue(p)]))
      : null;

  const panel = q('.maka-session-panel');
  const parent = panel?.parentElement ?? null;
  const siblings = parent
    ? Array.from(parent.children).map((c) => ({
        tag: c.tagName,
        cls: c.className?.toString?.().slice(0, 90),
        role: c.getAttribute('role'),
        dataResizing: c.getAttribute('data-resizing'),
      }))
    : [];

  // 2) spacing between the last nav item (定时任务) and the divider below it
  const navTop = q('.maka-session-panel-top');
  const navItems = navTop ? Array.from(navTop.querySelectorAll('.astryx-side-nav-item')) : [];
  const lastItem = navItems[navItems.length - 1] ?? null;
  const divider = panel?.querySelector('.astryx-divider, hr, [class*="divider"]') ?? null;

  // 3) type sizes of 会话 heading + group labels
  const textOf = (el) => (el?.textContent ?? '').trim().slice(0, 20);
  const headings = Array.from(document.querySelectorAll('.maka-session-list [class*="heading"], .maka-session-list h2, .maka-session-list h3, .maka-session-list .astryx-side-nav-section'))
    .slice(0, 8)
    .map((el) => ({
      text: textOf(el),
      cls: el.className?.toString?.().slice(0, 100),
      ...cs(el, ['font-size', 'line-height', 'font-weight', 'color']),
    }));
  const sectionTitles = Array.from(document.querySelectorAll('.maka-session-list *'))
    .filter((el) => ['会话', '最近', '置顶', '今天', '昨天'].includes(textOf(el)))
    .slice(0, 10)
    .map((el) => ({
      text: textOf(el),
      tag: el.tagName,
      cls: el.className?.toString?.().slice(0, 100),
      ...cs(el, ['font-size', 'line-height', 'font-weight', 'letter-spacing', 'color']),
    }));

  return {
    root: {
      htmlFontSize: getComputedStyle(document.documentElement).fontSize,
      bodyFontSize: getComputedStyle(document.body).fontSize,
      fontSizeBase: getComputedStyle(document.documentElement).getPropertyValue('--font-size-base'),
      fontSizeUi: getComputedStyle(document.documentElement).getPropertyValue('--font-size-ui'),
      fontSizeCaption: getComputedStyle(document.documentElement).getPropertyValue('--font-size-caption'),
      astryxSm: getComputedStyle(document.documentElement).getPropertyValue('--font-size-sm'),
      astryxXs: getComputedStyle(document.documentElement).getPropertyValue('--font-size-xs'),
    },
    panel: {
      cls: panel?.className?.toString?.(),
      inlineWidth: panel?.style?.width ?? null,
      rect: panel?.getBoundingClientRect().toJSON(),
      ...cs(panel, ['transition-property', 'transition-duration', 'transition-timing-function', 'width']),
    },
    parentTag: parent?.tagName,
    parentCls: parent?.className?.toString?.().slice(0, 120),
    siblings,
    lastNavItem: lastItem
      ? { text: textOf(lastItem), rect: lastItem.getBoundingClientRect().toJSON() }
      : null,
    divider: divider
      ? {
          cls: divider.className?.toString?.(),
          rect: divider.getBoundingClientRect().toJSON(),
          ...cs(divider, ['margin-top', 'margin-bottom', 'padding-top', 'padding-bottom', 'border-top-width', 'background-color', 'height']),
        }
      : null,
    dividerParent: divider?.parentElement
      ? {
          cls: divider.parentElement.className?.toString?.().slice(0, 120),
          rect: divider.parentElement.getBoundingClientRect().toJSON(),
          ...cs(divider.parentElement, ['padding-top', 'padding-bottom', 'gap', 'display', 'flex-direction']),
        }
      : null,
    headings,
    sectionTitles,
  };
});
console.log('=== STATIC PROBE ===');
console.log(JSON.stringify(probe, null, 2));

// 1) does the width actually animate on collapse?
const sample = await page.evaluate(async () => {
  const panel = document.querySelector('.maka-session-panel');
  const btn =
    document.querySelector('[data-testid="sidebar-collapse-toggle"]') ??
    Array.from(document.querySelectorAll('button')).find((b) =>
      (b.getAttribute('aria-label') ?? '').includes('侧边栏'),
    );
  if (!panel || !btn) return { error: 'no toggle', labels: Array.from(document.querySelectorAll('button')).map((b) => b.getAttribute('aria-label')).filter(Boolean).slice(0, 30) };
  const widths = [];
  let running = true;
  const tick = () => {
    if (!running) return;
    widths.push(Math.round(panel.getBoundingClientRect().width));
    requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
  btn.click();
  await new Promise((r) => setTimeout(r, 900));
  running = false;
  return {
    widths,
    distinct: Array.from(new Set(widths)),
    transitionProperty: getComputedStyle(panel).transitionProperty,
    transitionDuration: getComputedStyle(panel).transitionDuration,
    afterInlineWidth: panel.style.width,
  };
});
console.log('=== COLLAPSE ANIMATION SAMPLE ===');
console.log(JSON.stringify(sample, null, 2));

await page.screenshot({ path: '/tmp/maka-diag-collapsed.png' });
await page.waitForTimeout(500);
await app.close();
