import assert from 'node:assert/strict';
import test from 'node:test';

// electron-builder rejects an unknown option outright (its schema sets
// `additionalProperties: false`), and the release workflow is the only thing
// that ever loads this config — so a misspelled or removed option is invisible
// until release day, on both platforms at once, because macOS and Windows share
// one config object. This asserts the config against electron-builder's own
// validator rather than restating its rules. If a future electron-builder moves
// this module, update the path here; the version is pinned in package.json.
const { validateConfiguration } = await import(
  new URL('../node_modules/app-builder-lib/out/util/config/config.js', import.meta.url)
);

test('the desktop release config is valid for the installed electron-builder', async () => {
  const config = (
    await import(new URL('../apps/desktop/electron-builder.config.mjs', import.meta.url))
  ).default;

  await validateConfiguration(structuredClone(config), { add: () => {} });
});

test('an unknown release option is rejected rather than ignored', async () => {
  const config = (
    await import(new URL('../apps/desktop/electron-builder.config.mjs', import.meta.url))
  ).default;

  await assert.rejects(
    validateConfiguration(
      { ...structuredClone(config), win: { ...structuredClone(config.win), sign: false } },
      { add: () => {} },
    ),
    /unknown property 'sign'/,
  );
});
