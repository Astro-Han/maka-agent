/*
 * Licensed to the Apache Software Foundation (ASF) under one
 * or more contributor license agreements.  See the NOTICE file
 * distributed with this work for additional information
 * regarding copyright ownership.  The ASF licenses this file
 * to you under the Apache License, Version 2.0 (the
 * "License"); you may not use this file except in compliance
 * with the License.  You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing,
 * software distributed under the License is distributed on an
 * "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
 * KIND, either express or implied.  See the License for the
 * specific language governing permissions and limitations
 * under the License.
 */

import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdir, readFile, realpath, rm, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { decodeStoredMessage } from '../../packages/core/src/session.ts';
import { composeSkillInvocationMessage } from '../../packages/runtime/src/skill-invocation.ts';
import { modelFixture } from './client-turn.mjs';
import {
  createMessageSession,
  waitMessageTerminal as waitTerminal,
} from './client-message-fixture.mjs';
import { watchSession } from './client-subscription.mjs';
import { pluginRemote } from './client-plugin-remote.mjs';
import { readRuntimeHostSkills } from '../../packages/cli/src/runtime-host-skills.ts';

export async function verifyMessageSubmit(connection, workspace, reopened, openClient) {
  const skills = await pluginRemote(connection, 'maka.skills');
  try {
    const sessionId = 'message-submit';
    const catalog = (input) =>
      skills.method('path-request')({
        path: workspace,
        permissionMode: 'ask',
        collaborationMode: 'agent',
        request: { kind: 'catalog', ...input },
      });
    const invocable = (input) =>
      skills.method('request', sessionId)({ kind: 'invocable', ...input });
    const request = (op, input) => connection.request(op, input, 3000);
    const submit = (input, client = connection) =>
      client.request('turn.message.submit', input, 3000);
    const saved = join(workspace, 'message-submit.json');
    const preparedText = (userText, needsShell = false) =>
      composeSkillInvocationMessage({
        userText,
        skills: [
          needsShell
            ? { id: 'tools', name: 'Tools', instructions: 'Frozen tool instructions.' }
            : { id: 'review', name: 'Review', instructions: 'Frozen review instructions.' },
        ],
      });
    const rows = async () => {
      const watch = await watchSession(connection, sessionId, { kind: 'tail', maxBytes: 2 });
      try {
        return await watch.subscription.loadTranscript(decodeStoredMessage);
      } finally {
        await watch.close();
      }
    };
    if (reopened) {
      const previous = JSON.parse(await readFile(saved, 'utf8'));
      assert.notEqual(previous.first.originHostEpoch, connection.hostEpoch);
      assert.deepEqual(await submit(previous.first), previous.result);
      assert.deepEqual(
        await submit({ ...previous.first, originHostEpoch: connection.hostEpoch }),
        previous.result,
      );
      assert.deepEqual(await rows(), previous.rows);
      assert.deepEqual(await catalog(previous.sourceQuery), previous.sourceResult);
      assert.deepEqual(await request('turn.start', previous.legacy), previous.legacyResult);
      await assert.rejects(
        submit({ ...previous.first, messageId: 'never-admitted' }),
        (error) => error.code === 'outcome_unknown',
      );
      await assert.rejects(
        submit({ ...previous.first, content: { text: 'changed' } }),
        (error) => error.code === 'operation_conflict',
      );
      console.log('message-submit-reopened');
      return;
    }

    await verifyInvocableCatalog(connection, request, skills, workspace);
    const sourcePage = (view) => catalog({ view });
    const bundled = await sourcePage('bundled');
    assert.equal(bundled.items[0].id, 'computer-use');
    assert.equal(
      bundled.items[0].installed,
      true,
      'an occupied directory remains occupied without valid SKILL.md',
    );
    assert(bundled.items[0].category.length > 0);
    assert.equal(bundled.resolvedWorkspace.hostCwd, await realpath(workspace));
    const managed = await sourcePage('managed_sources');
    assert.deepEqual(managed.items, [
      {
        kind: 'managed_source',
        id: 'library',
        name: 'Library',
        description: 'managed source',
        category: '效率工具',
        sourceType: 'local',
        metadataTruncated: false,
        installed: false,
      },
    ]);
    assert(process.env.MAKA_TEST_STATE_ROOT);
    const namespace = createHash('sha256')
      .update(JSON.stringify(['maka.skills', 'profile']))
      .digest('hex');
    const alias = join(
      process.env.MAKA_TEST_STATE_ROOT,
      'plugin-data',
      namespace,
      'skills/library-alias',
    );
    await mkdir(alias);
    const original = '---\nname: Installed library\ndescription: local alias\n---\nInstalled body.';
    const hash = 'sha256:' + createHash('sha256').update(original).digest('hex');
    await writeFile(join(alias, 'SKILL.md'), original + '\nLocal modification.');
    const lock = {
      schemaVersion: 1,
      id: 'library-alias',
      sourceType: 'managed',
      sourceName: 'local-library',
      sourceVersion: '1',
      sourceId: 'LIBRARY',
      contentSha256: hash,
      sourceContentSha256: hash,
    };
    await writeFile(join(alias, 'skill.lock.json'), JSON.stringify(lock));
    const governance = await sourcePage('governance');
    const installed = governance.items.find((item) => item.id === 'library-alias');
    assert.equal(installed.kind, 'skill');
    assert.equal(installed.sourceType, 'managed');
    assert.equal(installed.userModified, true);
    assert.equal(installed.managedUpdateStatus, 'local_modified');
    assert.equal(installed.contextStatus, 'unknown');
    assert.equal(installed.contextRank, null);
    assert.equal(installed.manageable, true, 'the built-in Skills domain owns workspace mutations');
    const empty = governance.items.find((item) => item.id === 'computer-use');
    assert.equal(empty.ref, 'workspace:legacy:computer-use');
    assert.equal(empty.validationStatus, 'metadata_error');
    assert.deepEqual(empty.validationCodes, ['missing_frontmatter']);
    assert.equal(empty.enabled, false);
    const disabled = governance.items.find((item) => item.id === 'disabled');
    assert.equal(disabled.enabled, false);
    assert.equal(disabled.runtimeStatus, 'disabled');
    assert.equal(disabled.contextStatus, 'disabled');
    const archival = governance.items.find((item) => item.id === 'archival');
    assert.equal(archival.metadataTruncated, true);
    assert(archival.validationCodes.includes('projection_truncated'));
    assert(disabled.validationCodes.includes('duplicate_name'));
    await writeFile(join(alias, 'SKILL.md'), original);
    lock.sourceId = 'library';
    await writeFile(join(alias, 'skill.lock.json'), JSON.stringify(lock));
    assert.equal(
      (await sourcePage('governance')).items.find((item) => item.id === 'library-alias')
        .managedUpdateStatus,
      'update_available',
    );
    const governanceChanged = await catalog({
      view: 'governance',
      page: { revision: governance.revision, cursor: 'obsolete' },
    });
    assert.equal(governanceChanged.kind, 'revision_changed');
    const libraryPath = join(workspace, 'skill-home/.maka/skill-sources/library/SKILL.md');
    const sourceBody = await readFile(libraryPath, 'utf8');
    await writeFile(libraryPath, original);
    const upToDate = await sourcePage('governance');
    assert.equal(
      upToDate.items.find((item) => item.id === 'library-alias').managedUpdateStatus,
      'up_to_date',
    );
    await mkdir(join(alias, 'skill.baseline.md'));
    assert.equal(
      (await sourcePage('governance')).revision,
      upToDate.revision,
      'read-only governance does not inspect an installation baseline',
    );
    await writeFile(libraryPath, sourceBody);
    const aliased = await sourcePage('managed_sources');
    assert.equal(
      aliased.items[0].installed,
      true,
      'valid origin survives a renamed directory and local edits',
    );
    lock.id = 'wrong-directory';
    await writeFile(join(alias, 'skill.lock.json'), JSON.stringify(lock));
    const invalidated = await catalog({
      view: 'managed_sources',
      page: { revision: aliased.revision, cursor: 'obsolete' },
    });
    assert.equal(
      invalidated.kind,
      'revision_changed',
      'lock-only changes invalidate source pagination',
    );
    assert.equal((await sourcePage('managed_sources')).items[0].installed, false);
    const invalidOrigin = (await sourcePage('governance')).items.find(
      (item) => item.id === 'library-alias',
    );
    assert.equal(invalidOrigin.sourceType, 'unknown');
    assert.deepEqual(invalidOrigin.validationCodes, ['id_mismatch']);
    await rm(alias, { recursive: true });
    await writeFile(
      join(workspace, 'skill-home/.maka/skill-sources/library/SKILL.md'),
      '---\nname: Library\ndescription: changed source\n---\nNot installed.',
    );
    const sourceChanged = await catalog({
      view: 'managed_sources',
      page: { revision: managed.revision, cursor: 'obsolete' },
    });
    assert.equal(sourceChanged.kind, 'revision_changed');
    const sourceQuery = { view: 'managed_sources' };
    const sourceResult = await catalog(sourceQuery);
    assert.equal(sourceResult.items[0].description, 'changed source');
    const call = (name, input) => [
      {
        index: 0,
        id: 'skill-' + name,
        type: 'function',
        function: { name, arguments: JSON.stringify(input) },
      },
    ];
    const model = await modelFixture(async (input, index) => {
      const lastTool = () =>
        JSON.parse(input.messages.filter((message) => message.role === 'tool').at(-1).content);
      if (index === 5) {
        assert(
          input.messages.some(
            (message) =>
              message.role === 'user' && message.content.includes('project:maka:archival'),
          ),
        );
        const path = join(workspace, '.maka/skills/archival/SKILL.md');
        const document = await readFile(path, 'utf8');
        await writeFile(
          path,
          document.replace(/^description:.*$/m, 'description: refreshed archival'),
        );
        return call('SkillSearch', { query: 'archival', limit: 1 });
      }
      if (index === 6) {
        const search = lastTool();
        assert.equal(search.matches[0].ref, 'project:maka:archival');
        assert.equal(search.matches[0].metadataTruncated, true);
        assert.equal(search.matches[0].description.length, 1024);
        assert.equal(search.matchedCount, 1);
        assert.equal(search.truncated, false);
        assert(!JSON.stringify(search).includes('Frozen archival line'));
        assert(
          input.messages.some(
            (message) => message.role === 'user' && message.content.includes('refreshed archival'),
          ),
          'the next logical step refreshes the Skill inventory',
        );
        await writeFile(
          join(workspace, '.maka/skills/archival/SKILL.md'),
          'invalid changed after request capture',
        );
        return call('Skill', { name: search.matches[0].ref });
      }
      if (index === 7) {
        const archive = lastTool();
        assert.equal(archive.kind, 'maka.archived_tool_result');
        assert(
          archive.page.totalLines >= 400,
          'skill instructions retain multiline archive semantics',
        );
        assert(archive.page.content.includes('Frozen archival line'));
        return call('Read', { ...archive.page.next, limit: 5 });
      }
      if (index === 8) {
        const page = lastTool();
        assert.equal(page.returnedLines, 5);
        assert(page.content.includes('Frozen archival line'));
        assert(!page.content.includes('changed after request capture'));
      }
    });
    try {
      const basis = await createMessageSession(connection, workspace, sessionId, model.baseUrl);
      const skillPage = await invocable({});
      assert.deepEqual(
        skillPage.items.map((skill) => skill.id),
        ['archival', 'legacy', 'review'],
      );
      assert.equal(skillPage.items[0].description.length, 4096);
      const first = {
        sessionId,
        originHostEpoch: connection.hostEpoch,
        messageId: 'original-user-id',
        content: {
          text: 'model input 😀 /skill:review',
          displayText: 'visible input 😀 /skill:review',
          inlineReferences: [],
        },
        placement: 'next_turn',
      };
      const resolutions = (messageIds) =>
        request('turn.message.execution.query', { sessionId, messageIds });
      await assert.rejects(
        submit({
          ...first,
          content: {
            text: 'foreign',
            directoryReferences: [{ hostId: 'foreign', path: '/missing' }],
          },
        }),
        (error) => error.code === 'operation_unavailable',
      );
      assert.deepEqual(await resolutions([first.messageId]), { resolutions: [] });
      assert.equal(model.requests.length, 0);
      const blocked = await submit({
        ...first,
        messageId: 'disabled-skill',
        placement: 'current_turn',
        content: { text: 'do not run' },
        inputSelections: { 'maka.skills': ['project:maka:disabled'] },
      });
      assert.equal(blocked.disposition, 'blocked');
      assert.equal(blocked.preparation[0].receipt.failed[0].reason, 'disabled');
      assert.deepEqual(await resolutions(['disabled-skill']), { resolutions: [] });
      assert.equal(model.requests.length, 0);

      const sibling = await openClient();
      let result;
      try {
        const concurrent = await Promise.all([submit(first), submit(first, sibling)]);
        assert.deepEqual(
          concurrent[0],
          concurrent[1],
          'one canonical root for concurrent identity retries',
        );
        result = concurrent[0];
      } finally {
        await sibling.close();
      }
      assert.equal(result.disposition, 'turn_started', 'idle next_turn opens its own Turn');
      assert.deepEqual(result.preparation[0].receipt.loaded, [{ id: 'review', name: 'Review' }]);
      const terminal = await waitTerminal(request, sessionId, result.turnId);
      assert.equal(terminal.status, 'completed');
      assert.notEqual(terminal.runId, terminal.turnId);
      assert.deepEqual(await resolutions([first.messageId]), {
        resolutions: [
          {
            messageId: first.messageId,
            state: 'owned',
            turnId: terminal.turnId,
            runId: terminal.runId,
          },
        ],
      });
      assert.equal(model.requests.length, 1);
      await assert.rejects(
        submit({ ...first, messageId: terminal.terminalEventId }),
        (error) => error.code === 'operation_conflict',
        'a visible canonical ID collision is a caller conflict, not a Host drain',
      );
      assert.equal(model.requests.length, 1);
      assert.equal(
        model.requests[0].messages.find((m) => m.role === 'user').content,
        preparedText('model input 😀'),
      );
      const transcript = await rows();
      const user = transcript.find((row) => row.id === first.messageId);
      assert(user, 'original client sees the original message ID, not an invented event row');
      assert.equal(user.text, preparedText('model input 😀'));
      assert.equal(user.displayText, first.content.displayText);
      assert.deepEqual(user.inlineReferences, [
        {
          kind: 'skill',
          value: '/skill:review',
          label: 'Review',
          start: first.content.displayText.indexOf('/skill:review'),
        },
      ]);
      await assert.rejects(
        submit({ ...first, placement: 'current_turn' }),
        (error) => error.code === 'operation_conflict',
      );
      await assert.rejects(
        submit({ ...first, content: { text: 'changed' } }),
        (error) => error.code === 'operation_conflict',
      );

      const second = {
        ...first,
        messageId: 'cancelled-user-id',
        content: { text: 'held model' },
        placement: 'current_turn',
      };
      const active = await submit(second);
      await model.partial;
      assert.deepEqual(
        await submit(second),
        active,
        'active exact retry does not start another Run',
      );
      const owned = (await resolutions([second.messageId])).resolutions[0];
      assert.equal(owned.turnId, active.turnId);
      assert(!model.requests[1].tools.some((tool) => tool.function.name === 'Bash'));
      const session = (await request('session.catalog.query', { kind: 'get', sessionId })).session;
      const widened = await request('session.configuration.update', {
        sessionId,
        expectedRevision: session.revision,
        patch: { permissionMode: 'bypass' },
      });
      assert.equal(widened.kind, 'committed');
      const changedSkills = await invocable({
        page: { revision: skillPage.revision, cursor: 'obsolete' },
      });
      assert.equal(changedSkills.kind, 'revision_changed');
      const futureSkills = await invocable({});
      assert(
        futureSkills.items.some((skill) => skill.id === 'tools'),
        'selector previews future admission; active steering remains frozen',
      );
      const incompatible = await submit({
        ...second,
        messageId: 'wrong-run-tools',
        content: { text: '/skill:tools' },
      });
      assert.equal(incompatible.disposition, 'blocked');
      assert.equal(incompatible.preparation[0].receipt.failed[0].reason, 'host_incompatible');
      assert.deepEqual(await resolutions(['wrong-run-tools']), { resolutions: [] });
      const followup = {
        ...second,
        messageId: 'followup-user-id',
        placement: 'next_turn',
        content: { text: 'next input' },
      };
      const accepted = await submit(followup);
      assert.equal(accepted.disposition, 'followup');
      await request('queue.entry.update', {
        sessionId,
        originHostEpoch: connection.hostEpoch,
        updateId: 'edit-followup',
        entryId: followup.messageId,
        expectedQueueRevision: accepted.queueRevision,
        text: 'edited next input /skill:tools',
      });
      await writeFile(
        join(workspace, '.maka/skills/tools/SKILL.md'),
        '---\nname: Tools\ndescription: work\n---\nChanged after admission.',
      );
      await assert.rejects(
        request('queue.entry.promote', {
          sessionId,
          originHostEpoch: connection.hostEpoch,
          entryId: followup.messageId,
          promoteId: 'wrong-target',
        }),
        (error) => error.code === 'operation_conflict',
        'promotion uses saved requirements, not changed files',
      );
      assert.deepEqual(
        await submit(followup),
        accepted,
        'retry returns original receipt after editing',
      );
      const retractable = { ...second, messageId: 'retracted-user-id' };
      const retractedReceipt = await submit(retractable);
      assert.equal(retractedReceipt.disposition, 'steering');
      await assert.rejects(
        request('queue.entry.update', {
          sessionId,
          originHostEpoch: connection.hostEpoch,
          entryId: retractable.messageId,
          updateId: 'wrong-steering-tools',
          expectedQueueRevision: retractedReceipt.queueRevision,
          text: '/skill:write',
        }),
        (error) => error.code === 'operation_conflict',
      );
      await request('queue.entry.retract', {
        sessionId,
        originHostEpoch: connection.hostEpoch,
        retractId: 'retract-message',
        entryId: retractable.messageId,
      });
      assert.deepEqual(
        await submit(retractable),
        retractedReceipt,
        'retry does not undo cancellation',
      );
      assert.deepEqual((await resolutions([retractable.messageId])).resolutions, [
        { messageId: retractable.messageId, state: 'cancelled' },
      ]);
      const late = {
        ...second,
        messageId: 'late-steering-user-id',
        content: { text: 'late steering input /skill:review' },
      };
      const lateReceipt = await submit(late);
      assert.equal(lateReceipt.disposition, 'steering');
      assert.deepEqual(lateReceipt.preparation[0].receipt.loaded, [
        { id: 'review', name: 'Review' },
      ]);
      await writeFile(
        join(workspace, '.maka/skills/review/SKILL.md'),
        '---\nname: Review\ndescription: review code\n---\nChanged after admission.',
      );
      await assert.rejects(
        request('session.lifecycle.set', { sessionId, state: 'archived' }),
        (error) => error.code === 'session_busy',
      );
      await request('turn.stop', { sessionId, turnId: owned.turnId, runId: owned.runId });
      assert.equal((await waitTerminal(request, sessionId, owned.turnId)).status, 'cancelled');
      const waitOwned = async (id) => {
        for (let index = 0; index < 300; index++) {
          const item = (await resolutions([id])).resolutions[0];
          if (item?.state === 'owned') return item;
          await delay(10);
        }
        throw new Error('Queued message did not acquire canonical ownership');
      };
      const lateOwner = await waitOwned(late.messageId);
      const nextOwner = await waitOwned(followup.messageId);
      assert.notEqual(lateOwner.turnId, owned.turnId);
      assert.notEqual(lateOwner.turnId, nextOwner.turnId);
      assert.equal((await waitTerminal(request, sessionId, lateOwner.turnId)).status, 'completed');
      assert.equal((await waitTerminal(request, sessionId, nextOwner.turnId)).status, 'completed');
      assert.equal(model.requests.length, 4);
      assertPreparedInput(model.requests[2], preparedText('late steering input'));
      assertPreparedInput(model.requests[3], preparedText('edited next input', true));
      assert(model.requests[3].tools.some((tool) => tool.function.name === 'Bash'));
      assert.deepEqual(
        await submit(followup),
        accepted,
        'original receipt survives canonical delivery',
      );
      assert.deepEqual(await submit(late), lateReceipt);
      const legacy = {
        sessionId,
        turnId: 'legacy-skills',
        content: { text: 'legacy input /skill:legacy' },
        inputSelections: { 'maka.skills': ['project:maka:legacy', 'missing'] },
      };
      const legacyBlocked = await request('turn.start', {
        ...legacy,
        content: { text: '/skill:disabled' },
        inputSelections: { 'maka.skills': ['disabled'] },
      });
      assert.equal(legacyBlocked.kind, 'blocked');
      assert.equal(legacyBlocked.preparation[0].receipt.loaded.length, 0);
      assert.equal(model.requests.length, 4);
      await assert.rejects(
        request('turn.query', { sessionId, turnId: legacy.turnId }),
        (error) => error.code === 'not_found',
      );
      const legacyStarted = await request('turn.start', legacy);
      assert.equal(legacyStarted.kind, 'started');
      assert.deepEqual(legacyStarted.preparation[0].receipt.loaded, [
        { id: 'legacy', name: 'Legacy' },
      ]);
      assert.deepEqual(legacyStarted.preparation[0].receipt.failed, [
        { request: 'missing', reason: 'not_found' },
      ]);
      const legacyTerminal = await waitTerminal(request, sessionId, legacy.turnId);
      model.check();
      assert.equal(legacyTerminal.status, 'completed');
      const legacyResult = { ...legacyStarted, turn: legacyTerminal };
      assert.equal(model.requests.length, 8);
      assertPreparedInput(
        model.requests[4],
        composeSkillInvocationMessage({
          userText: 'legacy input',
          skills: [{ id: 'legacy', name: 'Legacy', instructions: 'Frozen legacy instructions.' }],
        }),
      );
      await writeFile(join(workspace, '.maka/skills/legacy/SKILL.md'), 'invalid changed document');
      await assert.rejects(
        request('turn.start', { ...legacy, inputSelections: { 'maka.skills': ['legacy'] } }),
        (error) => error.code === 'operation_conflict',
        'identity retains the original skill selection, not just prepared content',
      );
      // Replay no longer needs credentials, a model route, or an unarchived Session.
      await request('connection.catalog.remove', { expected: basis });
      await request('session.lifecycle.set', { sessionId, state: 'archived' });
      await assert.rejects(invocable({}), (error) => error.code === 'invalid_request');
      assert.deepEqual(await submit(first), result);
      assert.deepEqual(await submit(second), active);
      assert.deepEqual(await request('turn.start', legacy), legacyResult);
      const finalSources = await catalog(sourceQuery);
      assert.deepEqual(finalSources.items, sourceResult.items);
      assert.notEqual(
        finalSources.revision,
        sourceResult.revision,
        'catalog revisions cover all Skill mutation inputs, not just the selected view',
      );
      await writeFile(
        saved,
        JSON.stringify({
          first,
          result,
          legacy,
          legacyResult,
          sourceQuery,
          sourceResult: finalSources,
          rows: await rows(),
        }),
      );
      console.log('message-submit-passed');
    } finally {
      await model.close();
    }
  } finally {
    await skills.close();
  }
}

async function verifyInvocableCatalog(connection, request, skills, workspace) {
  const target = {
    path: workspace,
    collaborationMode: 'agent',
    permissionMode: 'ask',
  };
  const query = (page) =>
    skills.method('path-request')({ ...target, request: { kind: 'invocable', page } });
  const directories = Array.from({ length: 129 }, (_, n) =>
    join(workspace, '.maka/skills', 'catalog-' + String(n).padStart(3, '0')),
  );
  try {
    for (const [n, directory] of directories.entries()) {
      await mkdir(directory);
      await writeFile(
        join(directory, 'SKILL.md'),
        `---\nname: Catalog ${n}\ndescription: selectable entry\n---\nInstructions.`,
      );
    }
    const first = await query(null);
    assert.equal(first.kind, 'page');
    assert.equal(first.items.length, 128);
    assert(first.nextCursor);
    const last = await query({
      revision: first.revision,
      cursor: first.nextCursor,
    });
    assert.equal(last.nextCursor, null);
    const all = [...first.items, ...last.items];
    assert.equal(all.length, 132);
    assert.equal(new Set(all.map((item) => item.ref)).size, 132);
    assert.deepEqual(
      await readRuntimeHostSkills(connection, { kind: 'host_path', path: workspace }, 'ask'),
      all,
      'the real CLI picker uses the same public Remote pages',
    );
    await assert.rejects(
      query({ revision: first.revision, cursor: 'invalid' }),
      (error) => error.code === 'invalid_request',
    );
    for (const [n, directory] of directories.slice(0, 24).entries()) {
      await writeFile(
        join(directory, 'SKILL.md'),
        `---\nname: Catalog ${n}\ndescription: ${JSON.stringify('"\\\n'.repeat(1300))}\n---\nChanged instructions.`,
      );
    }
    const changed = await query({
      revision: first.revision,
      cursor: first.nextCursor,
    });
    assert.equal(changed.kind, 'revision_changed', 'content changes invalidate the continuation');
    let page = await query(null);
    const seen = [];
    let pages = 0;
    while (true) {
      assert.equal(page.kind, 'page');
      assert.equal(page.revision, changed.actualRevision);
      assert(Buffer.byteLength(JSON.stringify(page), 'utf8') <= 48 * 1024);
      seen.push(...page.items.map((item) => item.ref));
      pages++;
      if (!page.nextCursor) break;
      page = await query({ revision: page.revision, cursor: page.nextCursor });
    }
    assert(pages >= 3, 'escaped metadata respects byte bounds, not only item count');
    assert.deepEqual(new Set(seen), new Set(all.map((item) => item.ref)));
    assert.equal(seen.length, all.length);
    const registered = await request('project.catalog.mutate', {
      kind: 'register',
      path: workspace,
    });
    const projectTarget = {
      projectId: registered.project.id,
      permissionMode: 'ask',
      collaborationMode: 'agent',
      request: { kind: 'invocable' },
    };
    const projectQuery = skills.method('project-request');
    const projectPage = await projectQuery(projectTarget);
    assert.equal(projectPage.kind, 'page');
    assert.notEqual(
      projectPage.revision,
      page.revision,
      'identical paths do not erase target identity',
    );
    await request('project.catalog.mutate', { kind: 'archive', projectId: registered.project.id });
    await assert.rejects(projectQuery(projectTarget), (error) => error.code === 'invalid_request');
    const planSkills = await skills.method('path-request')({
      ...target,
      collaborationMode: 'plan',
      request: { kind: 'invocable' },
    });
    assert.deepEqual(
      planSkills.items,
      [],
      'a surface without Skill cannot advertise invocable Skills',
    );
    const sessions = await request('session.catalog.query', { kind: 'list_start' });
    assert.equal(sessions.sessions.length, 0, 'a model-free preview does not create a Session');
  } finally {
    for (const directory of directories) await rm(directory, { recursive: true, force: true });
  }
}

function assertPreparedInput(request, expected) {
  assert.equal(
    request.messages.filter((message) => message.role === 'user' && message.content === expected)
      .length,
    1,
    'accepted instructions are delivered exactly once, independently of ephemeral plugin context',
  );
}
