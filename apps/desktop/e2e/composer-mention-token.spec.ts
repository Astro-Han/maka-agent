import { expect, test, COMPOSER_INPUT } from './fixtures';

/**
 * The `@` file trigger: menu → inline token → the path the backend receives.
 * The token is a real chip in the draft now, so this also pins the one cascade
 * invariant it depends on — the token span must shrink to its badge. A
 * `[contenteditable]` selector that also matched the token's
 * `contenteditable="false"` stretched it to the full line and pushed the
 * surrounding text onto separate rows.
 */
test('a picked file mention becomes an inline token and sends as its path', async ({
  invocableSkillsWindow: page,
}) => {
  const composer = page.locator(COMPOSER_INPUT);
  await composer.fill('看一下 @agent');

  const listbox = page.getByRole('listbox', { name: '工作区文件' });
  await expect(listbox.getByRole('option').first()).toBeVisible();
  await composer.press('Enter');
  await composer.pressSequentially('里的说明');

  const token = composer.locator('[data-astryx-token]');
  await expect(token).toHaveAttribute(
    'data-astryx-token-value',
    '@.maka/skills/agent-write/SKILL.md',
  );
  const [tokenWidth, lineWidth] = await Promise.all([
    token.evaluate((element) => element.getBoundingClientRect().width),
    composer.evaluate((element) => element.getBoundingClientRect().width),
  ]);
  expect(tokenWidth).toBeLessThan(lineWidth / 2);

  await composer.press('Enter');
  await expect(
    page.getByText(/Fake backend received: 看一下 @\.maka\/skills\/agent-write\/SKILL\.md/),
  ).toBeVisible();
});
