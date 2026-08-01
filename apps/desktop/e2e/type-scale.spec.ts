import { expect, test } from './fixtures.js';

// The one thing the type-scale CSS contracts cannot prove.
//
// `type-scale-contract.test.ts` locks the three declarations the scale rests
// on — root unpinned, generated theme layered after the Astryx component
// sheet, product names kept as aliases. What no amount of text can show is
// what those three resolve to together in a real document, because custom
// properties resolve by tree distance while rules resolve by layer, and the
// two disagree at `:root`. That disagreement is not hypothetical: an earlier
// revision of this work shipped the aliases against a layer order that left
// them resolving to Astryx's neutral defaults, and every file read correctly.
//
// So this measures, per AGENTS.md's rule for cascade conclusions. One window,
// no new fixture: the disclosure-output scenario already boots a transcript
// with a live tool row, which is also the surface that most needed the retune.
//
// Deliberately NOT here: Markdown heading sizes and the monospace routing.
// Both are pinned as text contracts, and both follow arithmetically from the
// tokens asserted below — a second Electron cold start would buy no new
// information.
test('resolves one type scale from the root down to the transcript', async ({
  disclosureOutputWindow: page,
}) => {
  // Resolve a token AT :root and report the used length in px. Measuring
  // rather than string-comparing the declaration: Chromium normalizes
  // `0.875rem` to `.875rem`, and px is the number the design decision is
  // actually about. The probe hangs off <html> so it sees exactly what a
  // portaled Astryx component outside the Theme wrapper sees.
  const rootTokenPx = (name: string) =>
    page.evaluate((prop) => {
      const probe = document.createElement('div');
      probe.style.fontSize = `var(${prop})`;
      document.documentElement.append(probe);
      const px = getComputedStyle(probe).fontSize;
      probe.remove();
      return px;
    }, name);

  await test.step('the root is the browser default, not a density knob', async () => {
    // 13px here would silently rescale the whole rem-based ladder plus the
    // radius and spacing constants Astryx compiles against a 16px root.
    expect(
      await page.evaluate(() => getComputedStyle(document.documentElement).fontSize),
    ).toBe('16px');
  });

  await test.step('the product aliases resolve to Maka ladder rungs at :root', async () => {
    // Resolved AT :root on purpose. Inside the Theme wrapper these would pass
    // even under the broken layer order, because the theme is nearer in the
    // tree; `:root` is where layer order alone decides, and it is what
    // portaled Astryx components see.
    //
    // heading and stat are the discriminating pair: Astryx's neutral default
    // (`{base: 14, ratio: 1.2}`) happens to agree with Maka on base and sm,
    // but puts lg at 17px and 2xl at 24px. If the theme ever stops winning
    // at :root, those two are what move.
    expect(await rootTokenPx('--font-size-heading')).toBe('16px'); // neutral: 17
    expect(await rootTokenPx('--font-size-stat')).toBe('20px'); //    neutral: 24
    expect(await rootTokenPx('--font-size-ui')).toBe('14px');
    expect(await rootTokenPx('--font-size-caption')).toBe('12px');
  });

  await test.step('body copy and the tool disclosure share the body tier', async () => {
    expect(await page.evaluate(() => getComputedStyle(document.body).fontSize)).toBe('14px');

    // The reasoning/tool rows carry real content — what the agent is doing —
    // and Astryx styles their inner spans as `supporting`. The transcript
    // pulls them back to body size; de-emphasis stays with weight and colour.
    //
    // Assert the ROLE TOKEN on the trigger, not just one span's size. The
    // token is what makes this independent of Astryx's child order: an
    // earlier revision styled `> span:not(:last-child)` and silently missed
    // ChatReasoning, which nests its label one <div> deeper. Anything that
    // opts into the supporting role inherits this, at any depth.
    const trigger = page
      .locator('.maka-turn .astryx-chat-tool-calls [role="button"]')
      .first();
    await expect(trigger).toBeVisible();
    expect(
      await trigger.evaluate((el) => {
        const probe = document.createElement('span');
        probe.style.fontSize = 'var(--text-supporting-size)';
        el.append(probe);
        const px = getComputedStyle(probe).fontSize;
        probe.remove();
        return px;
      }),
    ).toBe('14px'); // Astryx's supporting role is 12px on this ladder

    // And that something actually consumes it: ChatToolCalls puts its call
    // name in the second direct span (the same one the font-family rule in
    // chat-message.css has always targeted).
    const label = trigger.locator('> span:nth-child(2)');
    await expect(label).toBeVisible();
    expect(await label.evaluate((el) => getComputedStyle(el).fontSize)).toBe('14px');
  });
});
