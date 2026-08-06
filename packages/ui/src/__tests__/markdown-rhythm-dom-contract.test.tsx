/**
 * The DOM half of the transcript rhythm contract.
 *
 * The rhythm table in styles.css keys entirely on DOM that Astryx generates at
 * runtime — `data-density` on the document root, `astryx-markdown-heading` plus
 * `data-level` on headings, `astryx-list`/`astryx-list-item` inside a list.
 * None of those names appear in Maka source as literals; they come from
 * Astryx's `themeProps()`, whose prefix has a single upstream owner
 * (`naming.ts`). So an Astryx bump that renames one of them makes every
 * selector in the table silently stop matching, and the spacing regresses to
 * the inverted ladder with no failing check anywhere: the CSS contract test
 * (apps/desktop/src/main/__tests__/markdown-rhythm-contract.test.ts) reads the
 * stylesheet as text and stays green against DOM it never sees, and the
 * `check-dead-css` entries are an allowlist that marks these classes live
 * rather than asserting they exist.
 *
 * This pins the other half: the hooks the stylesheet selects on are really
 * emitted. Together the two tests fail on either side of the join.
 */
import assert from 'node:assert/strict';
import { describe, it } from 'node:test';
import { readdir, readFile } from 'node:fs/promises';
import { join, relative, resolve } from 'node:path';
import { renderToStaticMarkup } from 'react-dom/server';
import { MarkdownBody } from '../markdown-body.js';

const UI_SRC = resolve(import.meta.dirname, '..', '..', 'src');

async function tsxFiles(dir: string): Promise<string[]> {
  const out: string[] = [];
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) out.push(...(await tsxFiles(path)));
    else if (entry.name.endsWith('.tsx') && !entry.name.includes('.test.')) out.push(path);
  }
  return out;
}

const SAMPLE = ['## Heading two', '', 'A paragraph.', '', '1. First item', '2. Second item'].join('\n');

function compactMarkup(): string {
  return renderToStaticMarkup(<MarkdownBody text={SAMPLE} density="compact" />);
}

describe('transcript markdown rhythm — DOM hooks', () => {
  it('emits the density attribute the rhythm table scopes on', () => {
    const markup = compactMarkup();
    // Every rule in the table is prefixed with this, so it is the one hook whose
    // two halves — selector prefix and runtime attribute — nothing else joins.
    // Rename it and the CSS contract still passes on text it never renders.
    assert.match(
      markup,
      /data-maka-contract="markdown"/,
      'the `data-maka-contract="markdown"` wrapper is gone. Every rule in the rhythm ' +
        'table is scoped on it, so all compact prose spacing and the heading scale are ' +
        'now dead — and the stylesheet-side contract cannot see it.',
    );
    assert.match(
      markup,
      /<div[^>]*role="document"[^>]*data-density="compact"|<div[^>]*data-density="compact"[^>]*role="document"/,
      'the document root no longer carries `data-density="compact"`. Every rule in the ' +
        'rhythm table is scoped on it, so all compact prose spacing is now dead.',
    );
    assert.match(
      markup,
      /class="[^"]*\bastryx-markdown\b/,
      'the document root no longer carries the `astryx-markdown` class the table selects on',
    );
  });

  it('emits the heading hooks the two-step scale selects on', () => {
    const markup = compactMarkup();
    assert.match(
      markup,
      /<h2[^>]*class="[^"]*\bastryx-markdown-heading\b/,
      'headings no longer carry `astryx-markdown-heading`; the transcript heading scale is dead',
    );
    assert.match(
      markup,
      /<h2[^>]*data-level="2"/,
      'headings no longer carry `data-level`. The table splits h1/h2 from h3-h6 on it, so ' +
        'without it every heading collapses to one tier — the exact defect this branch replaced.',
    );
  });

  /**
   * The rhythm table carries the transcript's heading typography, not just its
   * spacing, and keys both on `density`. That is a deliberate bet, and it is a
   * bet: Astryx's own density RFC (facebook/astryx#839) draws the line at
   * "density shifts heights and spacing, not typography", so the key is
   * honest only while `compact` and "transcript" are the same set. They are
   * today — the transcript is the only caller that asks for compact, and the
   * Daily Review renders a document at the default.
   *
   * A separate surface attribute would decouple them, but nothing needs it
   * yet and it would add a prop to a public component for a caller that does
   * not exist. Guard the assumption instead: the moment a second surface asks
   * for compact markdown it silently inherits transcript heading sizes and
   * dimmed deep headings, and this test is what tells whoever adds it that
   * they have to choose — inherit deliberately, or split the key then.
   */
  it('keeps compact markdown a transcript-only surface', async () => {
    const callers: string[] = [];
    for (const file of await tsxFiles(UI_SRC)) {
      const source = await readFile(file, 'utf8');
      // `<Markdown …>` opening tags only — not MarkdownBody's internal plumbing
      // and not the density Maka hands its own code-block renderers.
      for (const tag of source.matchAll(/<Markdown\s[^>]*?\/?>/gs)) {
        if (/density=(["'])compact\1/.test(tag[0])) callers.push(relative(UI_SRC, file));
      }
    }

    assert.deepEqual(
      [...new Set(callers)].sort(),
      ['chat-turn.tsx'],
      'a new caller renders markdown at compact density. The rhythm table treats ' +
        '`density="compact"` as "this is a transcript" and gives it flattened heading sizes ' +
        'plus secondary-colour h4-h6 — typography, which Astryx\'s density is explicitly not ' +
        'supposed to carry (facebook/astryx#839). Either that is what the new surface wants, ' +
        'and this list grows, or the typography rules need their own key. ' +
        `Found: ${JSON.stringify([...new Set(callers)].sort())}`,
    );
  });

  it('emits the list hooks whose control padding the table neutralizes', () => {
    const markup = compactMarkup();
    assert.match(
      markup,
      /class="[^"]*\bastryx-list\b/,
      'the markdown list no longer renders through Astryx `List`. If it stopped being a ' +
        'control list that is good news, but the padding-zeroing rule is now dead and the ' +
        'list rhythm needs re-measuring rather than silently inheriting whatever replaced it.',
    );
    assert.match(
      markup,
      /class="[^"]*\bastryx-list-item\b/,
      'list rows no longer carry `astryx-list-item`; the rule that spends their control-row ' +
        'padding no longer applies and list items revert to sitting wider apart than paragraphs',
    );
  });
});
