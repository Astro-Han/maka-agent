import type { Page } from '@playwright/test';
import { expect, test, COMPOSER_INPUT } from './fixtures';

async function createStarterSkillAndReload(page: Page): Promise<void> {
  const result = await page.evaluate(() => window.maka.skills.createStarter());
  expect(result.ok).toBe(true);
  await page.reload();
  await expect(page.locator(COMPOSER_INPUT)).toBeVisible();
  // The `/` menu lists what the renderer has projected, and that projection is
  // refreshed asynchronously after load. Wait for the Skill to be invocable so
  // an empty first search is never mistaken for a missing suggestion.
  await expect
    .poll(async () =>
      (await page.evaluate(() => window.maka.skills.listInvocable(undefined))).map(
        (skill) => skill.id,
      ),
    )
    .toContain('starter-skill');
}

/** The staged Skill's inline chip, addressed by the token it serializes to. */
const STARTER_CHIP = '[data-astryx-token-value="/skill:starter-skill"]';

/**
 * Pick 示例技能 from the `/` menu. `append` types the trigger after whatever
 * is already in the draft; the default replaces the draft, which is what a
 * chip-only send needs.
 */
async function selectStarterSkill(
  page: Page,
  options: { append?: boolean } = {},
): Promise<void> {
  const composer = page.locator(COMPOSER_INPUT);
  if (options.append) {
    await composer.click();
    await composer.pressSequentially(' /');
  } else {
    await composer.fill('/');
  }
  const listbox = page.getByRole('listbox', { name: '技能' });
  await expect(listbox).toBeVisible();
  await expect(listbox.getByRole('option', { name: /示例技能/ })).toBeVisible();
  await composer.press('Enter');
  await expect(page.locator(STARTER_CHIP)).toContainText('示例技能');
}

/**
 * A chosen Skill is a chip in the draft, not a selection held beside it: the
 * caret lands after it, it serializes to the `/skill:<id>` token a user could
 * have typed, and Backspace behind it takes it away.
 */
test('the composer selects a structured Skill from slash suggestions', async ({
  window: page,
}) => {
  await createStarterSkillAndReload(page);
  await selectStarterSkill(page);

  const composer = page.locator(COMPOSER_INPUT);
  const chip = page.locator(STARTER_CHIP);
  await expect(chip).toContainText('示例技能');

  // Two presses, as for a `@` file chip: `insertToken` anchors the chip with a
  // trailing U+00A0, so the first Backspace eats the anchor and the second the
  // chip. One press would mean deleting the chip while a space still separates
  // the caret from it.
  await composer.press('Backspace');
  await composer.press('Backspace');
  await expect(chip).toHaveCount(0);
  await expect(composer).toHaveText('');
});

/**
 * Backspace eats the chip only from directly behind it. Get that wrong and the
 * Skill disappears silently while the user is deleting a character in a word
 * they typed after it.
 */
test('Backspace away from the chip deletes a character, not the staged Skill', async ({
  window: page,
}) => {
  await createStarterSkillAndReload(page);
  await selectStarterSkill(page);

  const composer = page.locator(COMPOSER_INPUT);
  await composer.click();
  await composer.pressSequentially('abc');
  await composer.press('Backspace');

  await expect(composer).toContainText('ab');
  await expect(page.locator(STARTER_CHIP)).toHaveCount(1);
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
    // The wait is Astryx's, not ours: a float that has just light-dismissed
    // ignores clicks for ~100ms, and every step below closes the `/` menu
    // right before reaching for ＋. Under that threshold the click on ＋
    // reaches nothing at all.
    await page.waitForTimeout(300);
    await plusMenu.click();
    await page.getByRole('menuitem', { name: '选择技能' }).click();
  };

  // Empty draft: the trigger is at a line start, so no space is needed.
  await openFromPlus();
  await expect(listbox).toBeVisible();
  await expect(listbox.getByRole('option', { name: /示例技能/ })).toBeVisible();
  await composer.press('Enter');
  await expect(page.locator(STARTER_CHIP)).toContainText('示例技能');

  // After a chip: `insertToken` anchors it with U+00A0, which is not a space,
  // so ＋ has to insert one or the menu never opens.
  await openFromPlus();
  await expect(listbox).toBeVisible();
  await page.keyboard.press('Escape');

  // After a word: same rule, the case a user hits by typing.
  await composer.fill('看看');
  await openFromPlus();
  await expect(listbox).toBeVisible();

  // Escape leaves the typed trigger behind, exactly as it does when the user
  // types `/` themselves — it is ordinary draft text, not a surface to dismiss.
  await page.keyboard.press('Escape');
  await expect(listbox).toBeHidden();
  await expect(composer).toContainText('看看 /');
});

/**
 * The ＋ entry writes into the draft, so it must not be reachable with an empty
 * catalog. It used to be: with no Skills installed, choosing it left a stray `/`
 * in the draft and opened an empty menu whose light dismiss then swallowed the
 * user's next click on the footer — three surprises for one click, and the
 * panel this replaced simply said "no skills available".
 */
test('the ＋ Skills entry is unavailable when no Skill is installed', async ({
  window: page,
}) => {
  const composer = page.locator(COMPOSER_INPUT);
  await expect(composer).toBeVisible();
  expect(await page.evaluate(() => window.maka.skills.listInvocable(undefined))).toEqual([]);

  await composer.click();
  await composer.pressSequentially('看看');
  await page.locator('.maka-composer-plus-menu').getByRole('button').click();

  const entry = page.getByRole('menuitem', { name: '选择技能' });
  await expect(entry).toBeVisible();
  await expect(entry).toBeDisabled();
  // Disabled AND answered, on screen: a grey row with no reason tells the user
  // nothing, and a reason only assistive tech can read does not reach them.
  await expect(entry).toContainText('当前没有可用技能');

  // The draft is untouched: no slash, and no menu was opened over the footer.
  await page.keyboard.press('Escape');
  await expect(composer).toHaveText('看看');
  await expect(page.getByRole('listbox', { name: '技能' })).toHaveCount(0);

  // And the footer still answers the very first click. Assert the menu it
  // owns opened, not where focus went: the trigger keeps focus or hands it to
  // the menu depending on how fast the float mounts, and either way the click
  // was received — which is the whole complaint this covers.
  const permission = page.locator('.maka-composer-left-controls button').nth(1);
  await permission.click();
  await expect(permission).toHaveAttribute('aria-expanded', 'true');
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
  const composer = page.locator(COMPOSER_INPUT);
  await composer.fill('run it');
  await selectStarterSkill(page, { append: true });
  const disabled = await page.evaluate(() => window.maka.skills.setEnabled('starter-skill', false));
  expect(disabled.ok).toBe(true);

  await composer.press('Enter');

  await expect(page.getByText('Skill 调用失败，消息未发送')).toBeVisible();
  await expect(composer).toContainText('run it');
  await expect(page.locator(STARTER_CHIP)).toContainText('示例技能');
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
