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

/**
 * Upstream contract: the trigger boundaries are Astryx's `findActiveTrigger`
 * now, and it is NOT equivalent to the `detectMentionTrigger` it replaced — a
 * space ends an `@` query, so a path with a space in it can no longer be
 * searched. Pin the grammar we actually depend on so an Astryx upgrade that
 * moves a boundary fails here rather than in front of a user.
 */
test('the trigger menu opens exactly on the boundaries we depend on', async ({
  invocableSkillsWindow: page,
}) => {
  const composer = page.locator(COMPOSER_INPUT);
  const expanded = () => composer.getAttribute('aria-expanded');
  const files = page.getByRole('listbox', { name: '工作区文件' });

  await composer.fill('看一下 @agent');
  await expect(files).toBeVisible();

  // A space ends the query — narrowing an `@` search by a second word, which
  // the retired popup allowed, is gone.
  await composer.pressSequentially(' write');
  await expect.poll(expanded).toBe('false');

  // A non-boundary `@` is not a trigger.
  await composer.fill('mail user@host.com');
  await expect.poll(expanded).toBe('false');

  // The nearest boundary wins.
  await composer.fill('@a /b');
  await expect(page.getByRole('listbox', { name: '技能' })).toBeVisible();
  await expect(files).toHaveCount(0);
});

/**
 * An open menu with nothing highlighted (still loading, or no matches) leaves
 * Enter unconsumed. It must not send the draft out from under the popup — and
 * it must not deadlock either: "no matches" is a stable state, so swallowing
 * Enter forever would leave the keyboard unable to send at all.
 */
test('Enter with an open, empty trigger menu withholds one send, not every send', async ({
  invocableSkillsWindow: page,
}) => {
  const composer = page.locator(COMPOSER_INPUT);
  await composer.fill('@zzzznomatchzzzz');
  await expect(page.getByRole('listbox', { name: '工作区文件' })).toBeVisible();

  await composer.press('Enter');
  await expect(page.getByText(/Fake backend received/)).toHaveCount(0);

  await composer.press('Enter');
  await expect(page.getByText('Fake backend received: @zzzznomatchzzzz')).toBeVisible();
});
