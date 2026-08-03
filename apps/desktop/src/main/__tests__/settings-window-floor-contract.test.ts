/**
 * Settings window-floor layout contract (#1304 / #1361 / #1364).
 *
 * Pin load-bearing CSS on the rule that owns each declaration (not a
 * cross-`}` scan). User-visible containment at SAFE_MIN_WIDTH remains a live
 * e2e smoke in settings.spec.ts; product journeys stay in e2e.
 */
import { strict as assert } from 'node:assert';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { describe, it } from 'node:test';
import {
  REPO_ROOT,
  stripCssComments,
  assertCssRuleDecls,
  cssMediaBody,
} from './css-test-helpers.js';

const SETTINGS_CSS = resolve(REPO_ROOT, 'apps/desktop/src/renderer/styles/settings');

async function readSettingsCss(name: string): Promise<string> {
  return stripCssComments(await readFile(resolve(SETTINGS_CSS, name), 'utf8'));
}

describe('settings window-floor CSS contract', () => {
  it('keeps health and usage metric strips auto-fit with legible floors', async () => {
    const health = await readSettingsCss('health.css');
    const usage = await readSettingsCss('bot.css');

    assertCssRuleDecls(
      health,
      '.settingsHealthSummaryLine',
      [/display:\s*flex/, /flex-wrap:\s*wrap/],
      'the health summary line must wrap its counters at the window floor',
    );
    assertCssRuleDecls(
      usage,
      '.settingsUsageSummary',
      [
        /grid-template-columns:\s*repeat\(\s*auto-fit\s*,\s*minmax\(\s*min\(\s*120px\s*,\s*100%\s*\)\s*,\s*1fr\s*\)\s*\)/,
      ],
      'usage summary must auto-fit instead of four hard tracks',
    );
    assertCssRuleDecls(
      usage,
      '.settingsUsagePage',
      [/grid-template-columns:\s*minmax\(\s*0\s*,\s*1fr\s*\)/],
      'usage page must pin a 0-floor column so wide children scroll inside, not expand the page',
    );
    assertCssRuleDecls(
      usage,
      '.settingsUsageTabsBar',
      [/overflow-x:\s*auto/],
      'usage tabs must scroll within themselves at the window floor',
    );
  });

  it('wraps unbreakable web-search tokens and keeps inputs shrinkable', async () => {
    const css = await readSettingsCss('web-search.css');

    assertCssRuleDecls(
      css,
      '.settingsWebSearchDisabledReason',
      [/overflow-wrap:\s*anywhere/],
      'env-var hint must wrap unbreakable tokens at the floor',
    );
    assertCssRuleDecls(
      css,
      '.settingsWebSearchResult a',
      [/overflow-wrap:\s*anywhere/],
      'result titles that are bare URLs must wrap inside the card',
    );
    assertCssRuleDecls(
      css,
      '.settingsWebSearchResult small',
      [/overflow-wrap:\s*anywhere/],
      'result hostnames must wrap inside the card',
    );
    assertCssRuleDecls(
      css,
      '.settingsWebSearchControlCluster,\n.settingsWebSearchSearchControls',
      [/min-width:\s*0/, /flex-wrap:\s*wrap/],
      'search control clusters must shrink and wrap at the floor',
    );
  });

  it('keeps form surfaces min-width free at the window floor', async () => {
    const form = await readSettingsCss('form.css');
    const rows = await readSettingsCss('rows.css');

    assertCssRuleDecls(
      form,
      '.settingsSurface',
      [/min-height:\s*0/, /overflow:\s*hidden/],
      'settings surface must be a constrained flex child',
    );
    assertCssRuleDecls(
      rows,
      '.settingsRows',
      [/grid-template-columns:\s*minmax\(\s*0\s*,\s*1fr\s*\)/],
      'the open row group must pin a 0-floor track so wide rows wrap instead of widening the page',
    );
  });

  it('keeps remote-access rows and detail headers readable at the window floor', async () => {
    const css = await readSettingsCss('bot.css');

    assertCssRuleDecls(
      css,
      '.settingsRemoteAccessItemTitle',
      [/flex-wrap:\s*wrap/, /overflow-wrap:\s*anywhere/],
      'channel titles must wrap status chips instead of overflowing',
    );
    assertCssRuleDecls(
      css,
      '.settingsRemoteAccessItemDescription',
      [/overflow-wrap:\s*anywhere/],
      'channel diagnostics must wrap unbreakable tokens',
    );
    assertCssRuleDecls(
      css,
      '.settingsBotStatusGrid dd',
      [/overflow-wrap:\s*anywhere/],
      'runtime status values must wrap inside the grid',
    );

    const media = cssMediaBody(css, '(max-width: 620px)');
    assert.ok(media, 'narrow remote-access media query must exist');
    assertCssRuleDecls(
      media!,
      '.settingsRemoteAccessItemActions',
      [/display:\s*none/],
      'narrow overview must drop the redundant chevron column',
    );
    assertCssRuleDecls(
      media!,
      '.settingsBotDetailHeaderBody',
      [/grid-column:\s*1\s*\/\s*-1/],
      'narrow detail body must span the full header row',
    );
    assertCssRuleDecls(
      media!,
      '.settingsBotStatusGrid',
      [/grid-template-columns:\s*1fr/],
      'narrow runtime status must collapse to one column',
    );
    // Grouped selector owns the stack — pin the group rule body.
    assertCssRuleDecls(
      media!,
      '.settingsRemoteAccessSectionHeader, .settingsBotConfigurationHeader',
      [/flex-direction:\s*column/],
      'narrow section headers must stack',
    );
  });
});
