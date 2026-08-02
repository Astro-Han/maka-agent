import { expect, test } from './fixtures.js';

// The one thing the type-scale CSS contracts cannot prove.
//
// `type-scale-contract.test.ts` locks the three declarations the scale rests
// on — root unpinned, generated theme layered after the Astryx component
// sheet, product names kept as aliases. What no amount of text can show is
// what those three resolve to together in a real document: a custom property
// is declared in one place and read in another, and whether the read sees the
// theme depends on layer order, `@scope` roots and tree position interacting.
// That is not hypothetical — an earlier revision of this work shipped the
// aliases against a layer order under which the generated theme applied
// nowhere at all, and every file still read correctly.
//
// So this measures, per AGENTS.md's rule for cascade conclusions. One window,
// no new fixture: the disclosure-output scenario already boots a transcript
// with a live tool row, which is also the surface that most needed the retune.
//
// Deliberately NOT here: Markdown heading sizes. Astryx's heading atoms sit
// in `astryx-components` and the contract already asserts `components` is the
// last layer, so the product rule wins transitively whatever its specificity.
//
// The monospace routing IS here, and the reason is worth stating: it looked
// like a text fact and is not one. Astryx's reset declares the identical
// zero-specificity `:where(code, kbd, samp, pre)` selector, so which stack a
// `<code>` gets is decided by `reset` preceding `base` — an ordering no
// contract in this repo asserts. Asserting the rule's text only proves it was
// written, not that it wins.
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
    // 13px here would silently rescale the whole rem-based ladder, plus
    // Astryx's rem-sized Icon atoms, which it documents as the px-equivalents
    // at a 16px root.
    expect(
      await page.evaluate(() => getComputedStyle(document.documentElement).fontSize),
    ).toBe('16px');
  });

  await test.step('the product aliases resolve to Maka ladder rungs at :root', async () => {
    // Resolved AT :root, which is also where a portaled Astryx component
    // reads them. Measured on main, the wrong layer order left the generated
    // theme inert at BOTH `<html>` and the inner Theme wrapper — see the note
    // in cascade-layers.css — so this probe is not merely covering an edge.
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
    //
    // Known gap, stated rather than hidden: only ChatToolCalls is probed
    // live. No e2e fixture renders a ChatReasoning row today, and seeding one
    // means adding live-turn data to a shared scenario every other spec also
    // sees. The rebind names both components in one selector and works by
    // inheritance, so it cannot reach one and miss the other for the
    // DOM-shape reason the old rule did — the contract test pins that
    // selector's text. What stays uncovered is an Astryx upgrade that stops
    // styling ChatReasoning's label with the supporting role at all.
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

    // The leading half of the same rebind, and the only check that
    // `--maka-line-body` resolves to a real length. It is the one derived
    // token here (`calc(--text-body-size * --text-body-leading)`); if its
    // definition goes missing, `line-height: <invalid>` falls back to normal
    // leading, which reads as slightly loose text rather than as a break.
    // Compared with a tolerance, not as a string: the leading is a unitless
    // ratio the theme rounds to 1.4286, so the product lands on 20.0004px.
    // Pinning the exact string would make this fail on a rounding change that
    // moves nothing a reader could see.
    expect(
      await trigger.evaluate((el) => {
        const probe = document.createElement('span');
        probe.style.lineHeight = 'var(--text-supporting-leading)';
        el.append(probe);
        const leading = Number.parseFloat(getComputedStyle(probe).lineHeight);
        probe.remove();
        return leading;
      }),
    ).toBeCloseTo(20, 1);

    // And that something actually consumes it: ChatToolCalls puts its call
    // name in the second direct span (the same one the font-family rule in
    // chat-message.css has always targeted).
    const label = trigger.locator('> span:nth-child(2)');
    await expect(label).toBeVisible();
    expect(await label.evaluate((el) => getComputedStyle(el).fontSize)).toBe('14px');
  });

  await test.step('code elements resolve the theme mono stack, not the reset one', async () => {
    expect(
      await page.evaluate(() => {
        const probe = document.createElement('code');
        document.body.append(probe);
        const family = getComputedStyle(probe).fontFamily;
        probe.remove();
        return family;
      }),
    ).toContain('Geist Mono');
  });
});
