/**
 * Settings row track contract.
 *
 * `.settingsRow` splits label and control across two grid tracks. The
 * default split gives the control `1fr` against the label's `0.36fr`,
 * which is correct only for a control that actually fills its column.
 * Switches (32px) and the time field (~84px) do not, so on a 718px row
 * the label was pinned to 173px with 465px of empty track beside it —
 * and the hint wrapped after ~9 characters, mid-word, because Chinese
 * has no spaces to break at.
 *
 * The rule is therefore per control bucket, and each bucket's track is
 * pinned here because the numbers are the whole point: an intrinsic
 * track collapses a select's `w-full` trigger to its selected text
 * (measured 320px → 118px), and a flat 320px cap eats the label instead
 * (82px at a 468px card, just above the 460px stacking breakpoint).
 * Only the both-caps form survives every width.
 */

import { strict as assert } from 'node:assert';
import { describe, it } from 'node:test';
import { readAllRendererCss, stripCssComments } from './css-test-helpers.js';

/** Rule body for an exact selector, anchored so prefixes don't match. */
function ruleBody(css: string, selector: string): string {
  const escaped = selector.replace(/[.[\]/+*()>:="-]/g, (ch) => `\\${ch}`);
  const re = new RegExp(`(?:^|\\n|\\})\\s*${escaped}[^{}]*\\{([^}]*)\\}`, 'm');
  return css.match(re)?.[1] ?? '';
}

describe('settings row track contract', () => {
  it('gives the label the slack that narrow controls cannot use', async () => {
    const css = stripCssComments(await readAllRendererCss());

    // Switch rows and the compact bucket size their control to content.
    // `auto` is what makes the label take everything else; an `fr` here
    // is the bug this contract exists to prevent.
    const intrinsic = ruleBody(css, '.settingsRow[data-control-width="compact"]');
    assert.ok(intrinsic, '.settingsRow[data-control-width="compact"] track rule must exist');
    assert.match(
      intrinsic,
      /grid-template-columns:\s*minmax\(0,\s*1fr\)\s+auto/,
      'narrow controls must size to content so the label keeps the rest of the row',
    );

    // The switch selector shares that rule; assert it is still attached to
    // it, since a tag-qualified rewrite of this selector has rotted before.
    assert.match(
      css,
      /\.settingsRow:has\(>\s*\[role="switch"\]\)[^{]*\{[^}]*grid-template-columns:\s*minmax\(0,\s*1fr\)\s+auto/,
      'switch-only rows must share the intrinsic-control track',
    );
  });

  it('caps the select track by BOTH an absolute and a proportional bound', async () => {
    const css = stripCssComments(await readAllRendererCss());
    const select = ruleBody(css, '.settingsRow[data-control-width="select"]');
    assert.ok(select, '.settingsRow[data-control-width="select"] track rule must exist');

    // `min(320px, 45%)`: the 320px alone leaves an 82px label at a 468px
    // card; the percentage alone would let the trigger grow unbounded on
    // a wide card. Both bounds, or the split breaks at one end.
    assert.match(
      select,
      /grid-template-columns:\s*minmax\(0,\s*1fr\)\s+minmax\(0,\s*min\(320px,\s*45%\)\)/,
      'select track must be capped by min(320px, 45%) — see the header for what each bound prevents',
    );
    assert.doesNotMatch(
      select,
      /grid-template-columns:[^;]*\bauto\b/,
      'an intrinsic select track collapses the w-full trigger to its selected option text',
    );
  });

  it('keeps the default row split for controls that do fill their column', async () => {
    const css = stripCssComments(await readAllRendererCss());
    // Exact `.settingsRow {` — no suffix, so `:hover` / `> [role=…]` and
    // the bucket rules above can't stand in for the base declaration.
    const base = css.match(/(?:^|\n|\})\s*\.settingsRow\s*\{([^}]*)\}/)?.[1] ?? '';
    assert.ok(base, '.settingsRow base rule must exist');
    assert.match(
      base,
      /grid-template-columns:\s*minmax\(150px,\s*0\.36fr\)\s+minmax\(0,\s*1fr\)/,
      'the base split still applies to rows whose control fills the value column',
    );
  });
});
