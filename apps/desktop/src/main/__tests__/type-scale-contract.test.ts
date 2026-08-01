/**
 * Type-scale contracts.
 *
 * The renderer has exactly one type-scale authority: `typography.scale` in
 * astryx-theme/makaTheme.ts, whose generated ladder the product aliases. Three
 * things make that arrangement work, and all three are invisible at the call
 * site — reverting any one of them silently hands back Astryx's neutral
 * defaults or an implicit rem multiplier, with nothing else failing:
 *
 *   1. the root font-size stays at the browser default,
 *   2. the generated theme layer sits after the Astryx component layer,
 *   3. the product size names stay aliases instead of holding values.
 *
 * Each is a pure text declaration, so it belongs here rather than in e2e —
 * the same demotion #1854 made for the settings floor layout. What text
 * cannot prove is what the three resolve to together in a live document;
 * `apps/desktop/e2e/type-scale.spec.ts` measures that with computed styles.
 */
import { strict as assert } from 'node:assert';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { describe, it } from 'node:test';
import {
  REPO_ROOT,
  stripCssComments,
  cssRuleBody,
  assertCssRuleDecls,
  assertCustomPropPinnedOnce,
  parseCssCustomProps,
  readAllRendererCss,
} from './css-test-helpers.js';

const RENDERER = resolve(REPO_ROOT, 'apps/desktop/src/renderer');

async function read(rel: string): Promise<string> {
  return readFile(resolve(RENDERER, rel), 'utf8');
}

describe('type scale contracts', () => {
  it('leaves the root font-size at the browser default', async () => {
    // `html { font-size: 13px }` is not a type scale: it is an implicit
    // ×0.8125 on every rem in the document, including the radius and spacing
    // constants Astryx compiles against a 16px root. The whole generated
    // ladder is expressed in rem, so a re-pinned root moves every tier at
    // once and nothing else in the suite notices.
    const tokens = stripCssComments(await read('maka-tokens.css'));
    assert.equal(
      cssRuleBody(tokens, 'html'),
      null,
      'maka-tokens.css must not declare an `html` rule — the root is not a density knob',
    );
    assert.doesNotMatch(
      stripCssComments(await readAllRendererCss()),
      /(?:^|[{}])\s*(?:html|:root)\s*\{[^}]*\bfont-size\s*:/,
      'no renderer stylesheet may pin font-size on html or :root',
    );
  });

  it('layers the generated theme after the Astryx component sheet', async () => {
    // astryx.css ships the neutral defaults on `:root`; a theme layered
    // before it can never override them there, so the product aliases below
    // would resolve to neutral values. This is also the order Astryx's own
    // README integration snippet prescribes.
    const decl = /@layer\s+([^;]+);/.exec(await read('cascade-layers.css'));
    assert.ok(decl, 'cascade-layers.css must declare the layer order');
    const layers = decl![1].split(',').map((s) => s.trim());
    assert.ok(
      layers.indexOf('astryx-tokens') > layers.indexOf('astryx-components'),
      `astryx-tokens must come after astryx-components; got ${layers.join(', ')}`,
    );
    assert.equal(layers.at(-1), 'components', 'product CSS must stay last');
  });

  it('keeps the product size names as aliases of the ladder', async () => {
    const tokens = await read('maka-tokens.css');
    assertCustomPropPinnedOnce(tokens, '--font-size-heading', 'var(--font-size-lg)');
    assertCustomPropPinnedOnce(tokens, '--font-size-stat', 'var(--font-size-2xl)');
    assertCustomPropPinnedOnce(tokens, '--font-size-ui', 'var(--font-size-base)');
    assertCustomPropPinnedOnce(tokens, '--font-size-caption', 'var(--font-size-sm)');
    assertCustomPropPinnedOnce(tokens, '--font-sans', 'var(--font-family-body)');
    assertCustomPropPinnedOnce(tokens, '--font-mono', 'var(--font-family-code)');
    assert.equal(
      parseCssCustomProps(tokens).get('--font-size-base'),
      undefined,
      '--font-size-base IS the Astryx token; redefining it here shadows the scale and makes --font-size-ui self-referential',
    );
  });

  it('pins the ladder rungs those aliases point at', async () => {
    // Only the four rungs the product consumes. Pinning all twelve would
    // charge every Astryx upgrade a test rewrite — the failure mode that got
    // the previous generation of scanner contracts deleted.
    const theme = await read('astryx-theme/maka.css');
    const rung = (name: string, rem: string) =>
      assertCustomPropPinnedOnce(theme, name, rem, 'astryx-theme/maka.css');
    rung('--font-size-sm', '0.75rem'); //   12px — caption
    rung('--font-size-base', '0.875rem'); // 14px — body / ui
    rung('--font-size-lg', '1rem'); //       16px — heading
    rung('--font-size-2xl', '1.25rem'); //   20px — stat
  });

  it('routes code elements through the monospace token', async () => {
    // Astryx's reset hard-codes a stack on :where(code, kbd, samp, pre) that
    // never consults --font-family-code, so every code element silently opted
    // out of the theme. The regression is subtle enough that only a contract
    // catches it.
    assertCssRuleDecls(
      stripCssComments(await read('maka-tokens.css')),
      ':where(code, kbd, samp, pre)',
      [/font-family:\s*var\(--font-mono\)/],
    );
  });

  it('flattens transcript headings to two steps, inside the turn only', async () => {
    // An agent turn is not a document, but a Daily Review report is — and
    // both render through the same MarkdownBody contract, so the scope has to
    // be the turn rather than the contract.
    const chat = stripCssComments(await read('styles/chat-message.css'));
    assertCssRuleDecls(
      chat,
      '.maka-turn [data-maka-contract="markdown"] h1',
      [/font-size:\s*var\(--font-size-lg\)/],
    );
    assertCssRuleDecls(
      chat,
      '.maka-turn [data-maka-contract="markdown"] :is(h2, h3, h4, h5, h6)',
      [/font-size:\s*var\(--text-body-size\)/],
    );
    assert.doesNotMatch(
      chat,
      /(?:^|[{},])\s*\[data-maka-contract="markdown"\]\s+(?:h[1-6]|:is\(h)/,
      'heading overrides must be scoped to .maka-turn, not to the shared Markdown contract',
    );
  });

  it('retunes the disclosure rows by rebinding the role token', async () => {
    // Not by restyling spans. `> span:not(:last-child)` looked like "every
    // span but the chevron" and was in fact "whatever Astryx happens to put
    // there this release" — it missed ChatReasoning's nested label entirely.
    const chat = stripCssComments(await read('styles/chat-message.css'));
    assertCssRuleDecls(
      chat,
      '.maka-turn :is(.astryx-chat-reasoning, .astryx-chat-tool-calls) [role="button"]',
      [
        /--text-supporting-size:\s*var\(--text-body-size\)/,
        /--text-supporting-leading:\s*var\(--maka-line-body\)/,
      ],
    );
    assert.doesNotMatch(
      chat,
      /font-size:[^;]*!important/,
      'the disclosure rows no longer need !important — product CSS is in the last layer',
    );
  });

  it('keeps font-size off em multipliers and rem', async () => {
    // Both are ways of re-deriving the ladder per site. `em` compounds off
    // whatever the parent happens to be (the hero title was hand-derived from
    // a 15px body, then silently rendered 27.7px under a 13px one); `rem`
    // reintroduces the root as a density knob. The ladder is the only source.
    const css = stripCssComments(await readAllRendererCss());
    assert.deepEqual(
      [...css.matchAll(/font-size:\s*[\d.]+(?:em|rem)\b/g)].map((m) => m[0]),
      [],
      'font-size must reference the type scale, not an em/rem multiplier',
    );
  });
});
