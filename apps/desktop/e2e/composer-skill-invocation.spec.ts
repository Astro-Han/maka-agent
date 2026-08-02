import type { Page } from '@playwright/test';
import { expect, test, COMPOSER_INPUT } from './fixtures';

async function createStarterSkillAndReload(page: Page): Promise<void> {
  const result = await page.evaluate(() => window.maka.skills.createStarter());
  expect(result.ok).toBe(true);
  await page.reload();
  await expect(page.locator(COMPOSER_INPUT)).toBeVisible();
}

async function selectStarterSkill(page: Page): Promise<void> {
  const composer = page.locator(COMPOSER_INPUT);
  await composer.fill('/');
  const listbox = page.getByRole('listbox', { name: '技能' });
  await expect(listbox).toBeVisible();
  await expect(listbox.getByRole('option', { name: /示例技能/ })).toBeVisible();
  await composer.press('Enter');
  await expect(page.locator('.maka-composer-skill-token')).toContainText('示例技能');
  await expect(composer).toHaveText('');
}

test('the composer selects a structured Skill from slash suggestions', async ({
  window: page,
}) => {
  await createStarterSkillAndReload(page);
  await selectStarterSkill(page);

  const composer = page.locator(COMPOSER_INPUT);
  const token = page.locator('.maka-composer-skill-token');
  await expect(token).toContainText('示例技能');
  const removeButton = token.getByRole('button');
  await removeButton.focus();
  await removeButton.press('Enter');
  await expect(token).toHaveCount(0);
  await expect(composer).toBeFocused();

  await selectStarterSkill(page);
  await composer.press('Backspace');
  await expect(token).toHaveCount(0);
});

/**
 * Backspace eats the staged Skill only from the very start of the draft. On a
 * textarea that was `selectionStart === 0`; on a contentEditable it is a Range
 * comparison, and getting it wrong is silent — the Skill disappears while the
 * user is deleting a character in the middle of a word.
 */
test('Backspace away from the start deletes a character, not the staged Skill', async ({
  window: page,
}) => {
  await createStarterSkillAndReload(page);
  await selectStarterSkill(page);

  const composer = page.locator(COMPOSER_INPUT);
  const token = page.locator('.maka-composer-skill-token');
  await composer.click();
  await composer.pressSequentially('abc');
  await composer.press('Backspace');

  await expect(composer).toHaveText('ab');
  await expect(token).toHaveCount(1);
});

test('slash suggestions follow Runtime project discovery and host gating', async ({
  invocableSkillsWindow: page,
}) => {
  const composer = page.locator(COMPOSER_INPUT);
  await composer.fill('/');
  const listbox = page.getByRole('listbox', { name: '技能' });
  await expect(listbox).toBeVisible();
  await expect(listbox).toContainText('Project Only');
  await expect(listbox).toContainText('Workspace Only');
  await expect(listbox).toContainText('Agent Write');
  await expect(listbox).not.toContainText('Host Incompatible');

  const planNames = await page.evaluate(async () =>
    (await window.maka.skills.listInvocable(undefined, {
      collaborationMode: 'plan',
    })).map((skill) => skill.name),
  );
  expect(planNames).not.toContain('Agent Write');
});

test('slash suggestions in a Deep Research session drop non-research Skills', async ({
  invocableSkillsWindow: page,
}) => {
  const composer = page.locator(COMPOSER_INPUT);
  const listbox = page.getByRole('listbox', { name: '技能' });

  await composer.fill('/');
  await expect(listbox).toBeVisible();
  await expect(listbox).not.toContainText('Deep Research Only');
  await composer.fill('');

  await page.getByRole('button', { name: '更多操作' }).click();
  await page.getByRole('menuitem', { name: '打开命令面板' }).click();
  await page.getByRole('dialog', { name: '命令面板' }).getByRole('option', { name: /新建深度研究/ }).click();
  await expect(page.getByLabel('深度研究，只读探索').filter({ visible: true })).toBeVisible();

  await composer.fill('/');
  await expect(listbox).toContainText('Deep Research Only');
});

test('open Skill suggestions follow current collaboration capabilities', async ({
  invocableSkillsWindow: page,
}) => {
  const composer = page.locator(COMPOSER_INPUT);
  await expect(composer).toBeVisible();
  await composer.fill('Open a session');
  await composer.press('Enter');
  await expect.poll(async () => (await page.evaluate(() => window.maka.sessions.list())).length).toBe(1);
  const [session] = await page.evaluate(() => window.maka.sessions.list());
  if (!session) throw new Error('the composer did not create a session');

  const listNames = (sessionId: string) =>
    page.evaluate(
      async (id) => (await window.maka.skills.listInvocable(id)).map((skill) => skill.name),
      sessionId,
    );

  await expect.poll(() => listNames(session.id)).toContain('Agent Write');
  await composer.fill('/');
  const listbox = page.getByRole('listbox', { name: '技能' });
  await expect(listbox).toContainText('Agent Write');

  await expect
    .poll(async () => (await page.evaluate(() => window.maka.sessions.list()))[0]?.status)
    .not.toBe('running');
  await page.evaluate(
    ({ sessionId }) => window.maka.sessions.setCollaborationMode(sessionId, 'plan'),
    { sessionId: session.id },
  );
  await expect.poll(() => listNames(session.id)).not.toContain('Agent Write');
  await expect(listbox).not.toContainText('Agent Write');
});

/**
 * #1912: ＋ → 选择技能 opens the SAME `/` menu the keyboard opens, by typing the
 * trigger. There is no second Skill surface — the multi-select panel that used
 * to live here is gone, and with it the transparent, product-owned popover that
 * was the reported defect.
 *
 * The half that only a real window can show is the trigger boundary.
 * `useTriggerMenu` recognizes `/` at a line start or after a space or newline,
 * so ＋ on a draft ending in a word has to insert the space itself. Get that
 * wrong and ＋ does nothing at all — silently, and only when a draft is present.
 */
test('the ＋ Skills entry opens the `/` menu, on an empty draft and after a word', async ({
  window: page,
}) => {
  await createStarterSkillAndReload(page);

  const composer = page.locator(COMPOSER_INPUT);
  const plusMenu = page.locator('.maka-composer-plus-menu').getByRole('button');
  const listbox = page.getByRole('listbox', { name: '技能' });
  const openFromPlus = async () => {
    await plusMenu.click();
    await page.getByRole('menuitem', { name: '选择技能' }).click();
  };

  // Empty draft: the trigger is at a line start, so no space is needed.
  await openFromPlus();
  await expect(listbox).toBeVisible();
  await expect(listbox.getByRole('option', { name: /示例技能/ })).toBeVisible();
  await composer.press('Enter');
  await expect(page.locator('.maka-composer-skill-token')).toContainText('示例技能');

  // After a word: without an inserted space the menu would never open, and the
  // slash would sit in the draft as literal text.
  await composer.click();
  await composer.pressSequentially('看看');
  await openFromPlus();
  await expect(listbox).toBeVisible();

  // Escape leaves the typed trigger behind, exactly as it does when the user
  // types `/` themselves — it is ordinary draft text, not a surface to dismiss.
  await page.keyboard.press('Escape');
  await expect(listbox).toBeHidden();
  await expect(composer).toHaveText('看看 /');
});

test('chip-only send renders a readable user message', async ({ window: page }) => {
  await createStarterSkillAndReload(page);
  await selectStarterSkill(page);

  const composer = page.locator(COMPOSER_INPUT);
  await composer.press('Enter');

  await expect(page.getByLabel('你发送的消息').first()).toContainText('/skill:starter-skill');
});

test('a blocked Skill invocation keeps the complete composer draft', async ({
  window: page,
}) => {
  await createStarterSkillAndReload(page);
  await selectStarterSkill(page);
  const disabled = await page.evaluate(() => window.maka.skills.setEnabled('starter-skill', false));
  expect(disabled.ok).toBe(true);

  const composer = page.locator(COMPOSER_INPUT);
  await composer.fill('run it');
  await composer.press('Enter');

  await expect(page.getByText('Skill 调用失败，消息未发送')).toBeVisible();
  await expect(composer).toHaveText('run it');
  await expect(page.locator('.maka-composer-skill-token')).toContainText('示例技能');
  await expect(page.locator('.maka-turn')).toHaveCount(0);
  // #1433: the composer creates the session BEFORE it sends, so a rejected
  // first send has to remove it again. Otherwise every blocked invocation
  // leaves a nameless empty session in the sidebar. `quick-chat.ts` used to
  // carry unit tests for this; when the composer became the only first-send
  // path, nothing was asserting it any more.
  await expect
    .poll(async () => (await page.evaluate(() => window.maka.sessions.list())).length)
    .toBe(0);
});
