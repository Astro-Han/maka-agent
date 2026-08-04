import assert from 'node:assert/strict';
import { afterEach, describe, it } from 'node:test';
import {
  readSessionWorkbarTab,
  readSessionWorkbarWidth,
  SESSION_WORKBAR_DEFAULT_WIDTH,
} from '../../renderer/session-workbar-layout.js';
import { readRendererContractCss } from './contract-css-helpers.js';

// This is the only thing that decides how wide the workbar comes back after a
// restart (#1861), and the only width the app reads without having produced it.
// `useResizable` clamps what it is handed but never rounds, so whatever this
// lets through reaches the panel, storage and `aria-valuenow` unchanged.
function storedWidth(value: string | null): void {
  (globalThis as { localStorage?: unknown }).localStorage = {
    getItem: (key: string) => (key === 'maka-session-workbar-width-v1' ? value : null),
  };
}

describe('readSessionWorkbarWidth', () => {
  afterEach(() => {
    delete (globalThis as { localStorage?: unknown }).localStorage;
  });

  it('rounds a fractional stored width', () => {
    storedWidth('400.5');
    assert.equal(readSessionWorkbarWidth(), 401);
  });

  it('leaves out-of-range widths for useResizable to clamp', () => {
    storedWidth('9999');
    assert.equal(readSessionWorkbarWidth(), 9999);
  });

  it('falls back to the default for missing, unparseable and non-positive widths', () => {
    for (const value of [null, '', 'wide', '0', '-10']) {
      storedWidth(value);
      assert.equal(readSessionWorkbarWidth(), SESSION_WORKBAR_DEFAULT_WIDTH, `stored: ${value}`);
    }
  });
});

/**
 * The persistence whitelist decides which tab survives a restart. A tab that
 * renders but is not listed here silently reverts to Tasks, which looks like
 * the panel forgetting rather than like a missing case.
 */
function storedTab(value: string | null): void {
  (globalThis as { localStorage?: unknown }).localStorage = {
    getItem: (key: string) => (key === 'maka-session-workbar-tab-v1' ? value : null),
  };
}

describe('readSessionWorkbarTab', () => {
  afterEach(() => {
    delete (globalThis as { localStorage?: unknown }).localStorage;
  });

  it('restores every persistable tab, including the Inspector', () => {
    for (const tab of ['browser', 'files', 'inspector'] as const) {
      storedTab(tab);
      assert.equal(readSessionWorkbarTab(), tab);
    }
  });

  it('falls back to tasks for the transient quote tab and for junk', () => {
    // 'quote' only exists while an excerpt is staged, so it is never restored.
    for (const stored of ['quote', 'nonsense', null]) {
      storedTab(stored);
      assert.equal(readSessionWorkbarTab(), 'tasks');
    }
  });
});

// The column's teardown is scheduled on the motion box's width transition
// ending. `transition: none` and `transition-property: none` do not shorten
// that transition — they CANCEL it, and no `transitionend` ever arrives — so
// every rule that quiets the box has to cap the duration instead. Miss this and
// the panel still looks shut while the column behind it keeps its task
// subscription and any live embedded browser. Two rules already need the quiet:
// the stacked layout under 990px, and the drag gate.
describe('workbar motion CSS contract', () => {
  it('never removes the motion box transition, only caps it', async () => {
    const css = await readRendererContractCss();
    const rules = [...css.matchAll(/\.maka-workbar-motion[^{]*\{([\s\S]*?)\}/g)].map(
      (match) => match[1] ?? '',
    );
    assert.ok(rules.length >= 3, `expected the box to be styled in several places, saw ${rules.length}`);

    for (const body of rules) {
      assert.doesNotMatch(
        body,
        /transition(-property)?:\s*none/,
        `a .maka-workbar-motion rule cancels its transition instead of capping it: ${body.trim()}`,
      );
    }

    // And the transition it caps to still has to reach zero-ish, not stay long.
    assert.match(css, /\.maka-workbar-motion[^{]*\{[^}]*transition-duration:\s*0\.01ms/);
  });

  it('animates the box and holds the column still inside it', async () => {
    const css = await readRendererContractCss();
    // The E2E fixture caps every transition, so no rendered test can see these
    // frames; this is where they are held. The box eases its own width, and the
    // column keeps a floor width so it does not reflow its whole layout on the
    // way out — the two halves of "the panel slides, its contents do not".
    assert.match(css, /\.maka-workbar-motion\s*\{[^}]*transition:\s*width\s+var\(--duration-large\)/);
    assert.match(css, /\.maka-session-workbar\s*\{[^}]*min-width:\s*320px/);
  });
});
