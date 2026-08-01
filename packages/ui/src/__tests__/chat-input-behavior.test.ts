import assert from 'node:assert/strict';
import { describe, it } from 'node:test';
import {
  createChatInputActionOwner,
  fileTransferContainsFiles,
  isChatInputComposing,
  mentionQueryMatches,
  skillMentionQuery,
} from '../chat-input-behavior.js';
import { addUniqueComposerSkillSelection } from '../use-composer-skill-draft.js';

describe('shared chat input behavior', () => {
  it('recognizes composition and file transfers across browser event shapes', () => {
    assert.equal(isChatInputComposing({ key: 'Enter', nativeEvent: { isComposing: true } }), true);
    assert.equal(isChatInputComposing({ key: 'Process', nativeEvent: {} }), true);
    assert.equal(isChatInputComposing({ nativeEvent: {} }, true), true);
    assert.equal(isChatInputComposing({ key: 'Enter', nativeEvent: {} }), false);
    assert.equal(fileTransferContainsFiles(['text/plain', 'Files'], 0), true);
    assert.equal(fileTransferContainsFiles(['text/plain'], 1), true);
    assert.equal(fileTransferContainsFiles(['text/plain'], 0), false);
  });

  it('serializes async actions and releases only their owned pending state', async () => {
    const states: Array<string | null> = [];
    const owner = createChatInputActionOwner<string>((action) => states.push(action));
    let release!: () => void;
    const first = owner.run(
      'drop',
      () => new Promise<string>((resolve) => (release = () => resolve('done'))),
    );
    assert.equal(await owner.run('paste', async () => 'ignored'), undefined);
    release();
    assert.equal(await first, 'done');
    assert.equal(owner.pending, null);
    assert.deepEqual(states, ['drop', null]);
  });

  it('does not let late completion clear state after reset', async () => {
    const states: Array<string | null> = [];
    const owner = createChatInputActionOwner<string>((action) => states.push(action));
    let release!: () => void;
    const action = owner.run('drop', () => new Promise<void>((resolve) => (release = resolve)));
    owner.reset();
    release();
    await action;
    assert.deepEqual(states, ['drop']);
  });
});

describe('mention filtering', () => {
  it('matches case-insensitive AND tokens and treats an empty query as universal', () => {
    assert.equal(mentionQueryMatches('SRC APP', 'src/app.tsx'), true);
    assert.equal(mentionQueryMatches('src app', 'src/main.tsx'), false);
    assert.equal(mentionQueryMatches('', 'anything'), true);
  });

  it('normalizes /skill prefixes while preserving bare queries', () => {
    assert.equal(skillMentionQuery('skill:wri'), 'wri');
    assert.equal(skillMentionQuery('SKILL:wri'), 'wri');
    assert.equal(skillMentionQuery('writer'), 'writer');
  });
});

describe('structured Skill selections', () => {
  it('deduplicates stable identities while keeping same-id selections from distinct refs', () => {
    const alpha = { id: 'alpha', name: 'Alpha' };
    assert.deepEqual(
      addUniqueComposerSkillSelection([alpha], { id: 'ALPHA', name: 'Renamed Alpha' }),
      [alpha],
    );
    const project = { ref: 'project:maka:writer', id: 'writer', name: 'Project Writer' };
    const user = { ref: 'user:agents:writer', id: 'writer', name: 'User Writer' };
    assert.deepEqual(addUniqueComposerSkillSelection([project], user), [project, user]);
  });
});
