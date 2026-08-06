/**
 * Transcript markdown rhythm contract.
 *
 * The defect this pins was not a wrong number — it was a wrong ORDER. Under
 * `density="compact"` the transcript spaced list items ~10px apart (Astryx's
 * List control row padding, which `density` cannot reach from outside) while
 * paragraphs sat 4px apart, so items at the same level read as further apart
 * than separate paragraphs. Visual distance stopped tracking semantic
 * distance.
 *
 * So the invariant is the ladder's ORDER, not its values: retuning 8px to 10px
 * is a design decision and should stay green here; making list gaps meet or
 * exceed block gaps is the regression, and must fail. A screenshot cannot lock
 * that — it fixes one rendering of one sample rather than the relation — which
 * is why AGENTS.md asks for a computed-style or text contract on cascade and
 * layout invariants. The visual half lives in the `TranscriptTurn` story
 * (packages/ui/stories/markdown.stories.tsx).
 *
 * Scoped to the compact surface only. Document mode is Astryx's own rhythm and
 * the Daily Review renders through it, so nothing here should constrain it.
 */
import { strict as assert } from 'node:assert';
import { describe, it } from 'node:test';
import { readAllRendererCss, stripCssComments } from './css-test-helpers.js';

/** The compact ladder, ordered from tightest (same level) to widest (chapter). */
const LADDER = ['--md-gap-list', '--md-gap-block', '--md-gap-section', '--md-gap-chapter'] as const;

/** `var(--space-N)` → N, the 4px-grid step count. Non-scale values return null. */
function spaceSteps(value: string): number | null {
  const match = /^var\(\s*--space-(\d+)\s*\)$/.exec(value.trim());
  return match ? Number(match[1]) : null;
}

describe('transcript markdown rhythm', () => {
  it('declares the compact ladder in strictly increasing order', async () => {
    const css = stripCssComments(await readAllRendererCss());
    const block = /\.astryx-markdown\[data-density="compact"\]\s*\{([^}]*)\}/.exec(css);
    assert.ok(block, 'no `.astryx-markdown[data-density="compact"]` custom-property block found');

    const declared = new Map<string, string>();
    for (const m of block[1].matchAll(/(--md-gap-[\w-]+)\s*:\s*([^;]+);/g)) {
      declared.set(m[1], m[2].trim());
    }

    const steps = LADDER.map((name) => {
      const value = declared.get(name);
      assert.ok(value, `${name} is not declared on the compact surface`);
      const n = spaceSteps(value);
      assert.ok(
        n !== null,
        `${name} is \`${value}\`; the ladder must stay on the --space-* scale so the ` +
          'order is comparable and a bare px value cannot drift off the 4px grid',
      );
      return { name, steps: n };
    });

    for (let i = 1; i < steps.length; i += 1) {
      const prev = steps[i - 1];
      const cur = steps[i];
      assert.ok(
        cur.steps > prev.steps,
        `${cur.name} (--space-${cur.steps}) must be strictly wider than ${prev.name} ` +
          `(--space-${prev.steps}). Visual distance has to rise with semantic distance: ` +
          'same-level list items closer than blocks, blocks closer than section breaks.',
      );
    }
  });

  it('expresses every block gap as an adjacent-sibling relation', async () => {
    const css = stripCssComments(await readAllRendererCss());
    const compactBlockRules = [
      ...css.matchAll(
        /(\[data-maka-contract="markdown"\]\s*\.astryx-markdown\[data-density="compact"\]\s*>[^{]*)\{([^}]*)\}/g,
      ),
    ].filter(([, , body]) => /margin-block-start\s*:/.test(body));

    assert.ok(compactBlockRules.length > 0, 'no compact block-gap rules found');

    for (const [, selector, body] of compactBlockRules) {
      assert.match(
        selector,
        />\s*\*\s*\+/,
        'every block gap must be an adjacent-sibling rule (`> * + …`). A gap is a relation ' +
          'between two blocks, so the first block should match no gap rule at all. The ' +
          '`> *` + `> :first-child` reset form looks equivalent but loses on specificity: ' +
          `:first-child scores (0,4,0) against the heading rules' (0,5,0), so a turn opening ` +
          'with a heading keeps a chapter gap above its first line and pushes off the top of ' +
          `the bubble. Offending rule: \`${selector.trim()}\` { ${body.trim()} }`,
      );
    }

    assert.doesNotMatch(
      css,
      /\.astryx-markdown\[data-density="compact"\]\s*>\s*:first-child/,
      'the `:first-child` gap reset is back. It cannot beat the heading rules on ' +
        'specificity — use adjacent-sibling gap rules so the first block is never matched.',
    );
  });

  it('spends the list-item row padding it re-spaces as prose', async () => {
    const css = stripCssComments(await readAllRendererCss());
    // Astryx's ListItem carries the control-row padding that inverted the
    // ladder. Zeroing it is what makes --md-gap-list the whole distance between
    // two items; leave it in and the gap variable understates the real spacing.
    const rule = new RegExp(
      String.raw`\.astryx-markdown\[data-density="compact"\][^{]*\.astryx-list-item\s*\{([^}]*)\}`,
    ).exec(css);
    assert.ok(rule, 'the compact surface no longer neutralizes `.astryx-list-item` padding');
    assert.match(
      rule[1],
      /padding-block\s*:\s*0/,
      'ListItem block padding must be zeroed on the compact surface, otherwise the real ' +
        'list-item gap is padding + gap and the ladder above is not the spacing you get',
    );
  });

  it('keeps two heading size steps on the compact surface', async () => {
    const css = stripCssComments(await readAllRendererCss());
    const fonts = [
      ...css.matchAll(
        /\.astryx-markdown\[data-density="compact"\][^{]*\.astryx-markdown-heading[^{]*\{([^}]*)\}/g,
      ),
    ]
      .map((m) => /font\s*:\s*var\(\s*(--maka-text-heading-\d)\s*\)/.exec(m[1])?.[1])
      .filter((tier): tier is string => tier !== undefined);

    assert.ok(
      new Set(fonts).size >= 2,
      'the transcript heading scale collapsed to one size tier. Flattening Astryx\'s ' +
        'document ladder is deliberate, but flattening it to ZERO steps is the bug this ' +
        'replaced: h2 and h3 then differ in nothing — not size, weight, or colour. ' +
        `Found tiers: ${JSON.stringify(fonts)}`,
    );
  });
});
