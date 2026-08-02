import { test, expect, COMPOSER_INPUT } from './fixtures';

/**
 * Session management: a second chat can be created without clobbering the
 * first, and both are listed in the sidebar (empty sessions are filtered out
 * by lastMessageAt, so B only appears once it has a message).
 *
 * Switching back to A via the sidebar row is left for a follow-up: rows can
 * sit inside a collapsed status group, so driving that interaction needs the
 * list's expand/group behavior pinned down first — a flaky click would be
 * worse than no assertion.
 */
test('creating a second chat keeps both in the sidebar', async ({ window: page }) => {
  const composer = page.locator(COMPOSER_INPUT);
  await composer.fill('alpha-marker');
  await composer.press('Enter');
  await expect(page.getByText(/Fake backend received: alpha-marker/)).toBeVisible();

  await page.getByRole('button', { name: '展开侧边栏' }).click();
  const sessions = page
    .getByRole('navigation', { name: '对话列表' })
    .locator('[data-session-id]');
  await expect(sessions).toHaveCount(1);

  await page
    .getByRole('navigation', { name: '对话列表' })
    .getByRole('button', { name: '新任务', exact: true })
    .click();
  await expect(page.getByRole('region', { name: '开始对话' })).toBeVisible();
  await composer.fill('beta-marker');
  await composer.press('Enter');
  await expect(page.getByText(/Fake backend received: beta-marker/)).toBeVisible();

  await expect(sessions).toHaveCount(2);
});

/**
 * An unsent draft belongs to its session and has to survive navigating away and
 * back. The swap runs while the composer is blurred (the click landed in the
 * sidebar), which is also the path that used to restore the text but leave the
 * caret at offset 0 — so type after returning and assert where it landed.
 */
test('an unsent draft survives leaving and returning to its session', async ({
  window: page,
}) => {
  const composer = page.locator(COMPOSER_INPUT);
  await composer.fill('alpha-marker');
  await composer.press('Enter');
  await expect(page.getByText(/Fake backend received: alpha-marker/)).toBeVisible();

  await composer.fill('draft that was never sent');

  await page.getByRole('button', { name: '展开侧边栏' }).click();
  const sidebar = page.getByRole('navigation', { name: '对话列表' });
  await sidebar.getByRole('button', { name: '新任务', exact: true }).click();
  await expect(page.getByRole('region', { name: '开始对话' })).toBeVisible();
  await expect(composer).toHaveText('');

  await sidebar.locator('[data-session-id]').first().click();
  await expect(composer).toHaveText('draft that was never sent');

  await composer.press('End');
  await composer.pressSequentially('!');
  await expect(composer).toHaveText('draft that was never sent!');
});
