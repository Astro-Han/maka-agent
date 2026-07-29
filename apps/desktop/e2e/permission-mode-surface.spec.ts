import { expect, test } from './fixtures';

/**
 * #1611 + #1616: the composer's permission control is the only place a user
 * learns what the running session may do. This covers the one journey that
 * crosses the main/renderer boundary — a session whose stored boundary is a
 * managed read-only profile — and checks that the surface names it, keeps the
 * option list intact, and never hands the reader the word "沙箱".
 */
test('a read-only session names its boundary and can still be raised to full access', async ({
  readOnlyBoundaryWindow: page,
}) => {
  const trigger = page.locator('.maka-composer-left-controls [data-slot="select-trigger"]');
  await expect(trigger).toHaveText('只读');
  await expect(trigger).toHaveAttribute('aria-label', '权限模式：只读');
  await expect(trigger).toHaveAttribute(
    'title',
    '本会话只能读取和搜索工作区里的文件；写入文件或访问网络前会先来问你。',
  );

  await trigger.click();
  const options = page.getByRole('option');
  await expect(options).toHaveCount(2);
  await expect(options.nth(0)).toContainText('自动');
  await expect(options.nth(1)).toContainText('完全权限');
  await expect(page.getByRole('listbox')).not.toContainText('沙箱');
});
