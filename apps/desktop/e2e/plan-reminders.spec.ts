import { expect, test } from './fixtures.js';

// The seeded reminders carry distinct createdAt values so the default
// 创建时间倒序 sort has a single answer. Without them they tie in the same
// millisecond and the fallback comparator decides the order, which made any
// position-based assertion here flaky between runs. Rows are `li > button`
// now, not articles: per Astryx's List guidance an interactive list item
// carries no interactive children.
test('orders the seeded reminders deterministically under the default sort', async ({
  planRemindersWindow: page,
}) => {
  const rows = page.locator('.maka-module-page-rows > li');
  await expect(rows).toHaveCount(8);
  const expected = ['已触发的本地提醒', '每周竞品动态追踪', '暂停的发布检查', '同步项目风险'];
  for (const [index, title] of expected.entries()) {
    await expect(rows.nth(index)).toContainText(title);
  }
});

// Rows are inert selectors now: per Astryx's List guidance, interactive
// elements do not belong inside an interactive list item. Every action a row
// used to carry (switch, trigger, snooze, edit, duplicate, clear, delete) lives
// in the end-panel inspector, as a plain labelled button.
test('acts on a reminder through the inspector and keeps deletion reversible', async ({
  planRemindersWindow: page,
}) => {
  const pausedRow = page.getByRole('button', { name: /暂停的发布检查/ });
  await pausedRow.click();

  const deleteButton = page.getByRole('button', { name: '删除', exact: true });
  const enableSwitch = page.getByRole('switch', { name: '启用' });
  await expect(page.getByRole('button', { name: '立即触发' })).toBeVisible();
  await expect(deleteButton).toBeVisible();

  // The inspector is the only surface these actions have left, so one of them
  // has to prove the whole chain: row selection → inspector control → main
  // process → the row's own state. A paused reminder enables into 待触发.
  await expect(enableSwitch).not.toBeChecked();
  await enableSwitch.click();
  await expect(enableSwitch).toBeChecked();
  // The row carries its lifecycle state only on the StatusDot's accessible
  // name — a sibling of the row button, not inside it — and its description
  // picks up the schedule it just regained.
  const pausedItem = page
    .locator('.maka-module-page-rows > li')
    .filter({ hasText: '暂停的发布检查' });
  await expect(pausedItem.getByRole('img', { name: '待触发' })).toBeVisible();
  await expect(pausedRow).toHaveText(/下次触发/);

  await deleteButton.click();
  const deleteDialog = page.getByRole('alertdialog');
  await expect(deleteDialog).toBeVisible();
  await expect(deleteDialog.getByRole('button', { name: '取消' })).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(deleteDialog).toBeHidden();
  await expect(pausedRow).toBeVisible();

  await deleteButton.click();
  await expect(deleteDialog).toBeVisible();
  await page.keyboard.press('Tab');
  await expect(deleteDialog.getByRole('button', { name: '删除' })).toBeFocused();
  await page.keyboard.press('Enter');

  // The row is gone, and so is the inspector — nothing is selected any more.
  await expect(pausedRow).toHaveCount(0);
  await expect(page.getByRole('button', { name: '立即触发' })).toBeHidden();
  // The 删除 button went down with the inspector, so something has to catch
  // focus: without this the keyboard user lands on `body`, at the top of the
  // document, with the list scrolled wherever they left it.
  await expect(page.locator('.maka-module-page-rows > li button:focus')).toHaveCount(1);
});

test('opens the edit dialog from the inspector and restores focus on Escape', async ({
  planRemindersWindow: page,
}) => {
  await page.getByRole('button', { name: /每周竞品动态追踪/ }).click();
  const editButton = page.getByRole('button', { name: '编辑', exact: true });
  await editButton.click();

  const dialog = page.getByRole('dialog', { name: '编辑定时任务' });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole('textbox', { name: '标题' })).toBeFocused();

  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
  await expect(editButton).toBeFocused();
});

test('narrow windows keep the inspector reachable and hand focus back', async ({
  planRemindersWindow: page,
}) => {
  const row = page.getByRole('button', { name: /每周竞品动态追踪/ });
  await row.click();

  // Crossing the breakpoint is not an action on this page: it must not open a
  // modal by itself, so the selection goes with the placement change.
  await page.setViewportSize({ width: 900, height: 800 });
  await expect(page.locator('dialog[open]')).toHaveCount(0);

  // The side panel is gone at this width, but every action it carries is not.
  await row.click();
  const sheet = page.locator('dialog[open]');
  await expect(sheet).toHaveCount(1);
  await expect(sheet.getByRole('button', { name: '立即触发' })).toBeVisible();

  await page.keyboard.press('Escape');
  await expect(sheet).toHaveCount(0);
  await expect(row).toBeFocused();
});
