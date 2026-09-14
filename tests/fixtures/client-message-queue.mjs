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
import { readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { watchSession } from './client-subscription.mjs';
import { modelFixture } from './client-turn.mjs';
import { createMessageSession, waitMessageTerminal } from './client-message-fixture.mjs';

export async function verifyMessageQueue(connection, workspace, reopened, openClient) {
  const sessionId = 'queue';
  const originHostEpoch = connection.hostEpoch;
  const invoke = (op, input, client = connection) => client.request(op, input, 3000);
  const reject = (op, input, code) =>
    assert.rejects(connection.request(op, input, 3000), (error) => error.code === code);
  const ids = ['root-message', 'one', 'two', 'steer', 'absent'];
  const query = () => invoke('turn.message.execution.query', { sessionId, messageIds: ids });
  let model;
  let rootProof;
  let active;
  if (!reopened) {
    model = await modelFixture();
    try {
      await createMessageSession(connection, workspace, sessionId, model.baseUrl);
      const submit = (messageId, content, placement = 'next_turn') =>
        invoke('turn.message.submit', {
          sessionId,
          originHostEpoch,
          messageId,
          content,
          placement,
        });
      const root = await submit('root-message', { text: 'root message' });
      await waitMessageTerminal(invoke, sessionId, root.turnId);
      rootProof = (await query()).resolutions[0];
      active = await submit('active-message', { text: 'hold current provider' });
      await model.partial;
      for (const [id, placement] of [
        ['one', 'next_turn'],
        ['two', 'next_turn'],
        ['steer', 'current_turn'],
      ]) {
        await submit(
          id,
          {
            text: '😀 @file',
            inlineReferences: [{ kind: 'workspace_file', value: '@file', label: 'file', start: 3 }],
          },
          placement,
        );
      }
    } catch (error) {
      await model.close();
      throw error;
    }
  }
  const watch = await watchSession(connection, sessionId);
  const queue = watch.subscription.snapshot.queue;
  assert.equal(queue.hostEpoch, originHostEpoch);
  const old = join(workspace, 'message-queue-epoch.json');
  try {
    if (reopened) {
      assert.deepEqual(queue.steering, []);
      assert.deepEqual(queue.followup, []);
      const previous = JSON.parse(await readFile(old, 'utf8'));
      assert.notEqual(previous.retract.originHostEpoch, originHostEpoch);
      await reject('queue.retract', previous.retract, 'outcome_unknown');
      assert.deepEqual(await invoke('turn.message.query', { sessionId, messageIds: ids }), {
        cancelledMessageIds: ['one', 'two', 'steer'],
      });
      assert.deepEqual((await query()).resolutions, [
        previous.rootProof,
        ...['one', 'two', 'steer'].map((messageId) => ({ messageId, state: 'cancelled' })),
      ]);
      console.log('message-queue-reopened');
      return;
    }
    assert.deepEqual(
      queue.steering.map((e) => e.entryId),
      ['steer'],
    );
    assert.deepEqual(
      queue.followup.map((e) => e.entryId),
      ['one', 'two'],
    );
    assert.deepEqual((await query()).resolutions, [
      rootProof,
      ...['one', 'two', 'steer'].map((messageId) => ({ messageId, state: 'pending' })),
    ]);
    assert.deepEqual(await invoke('turn.message.query', { sessionId, messageIds: ids }), {
      cancelledMessageIds: [],
    });
    assert.deepEqual(
      await invoke('turn.message.execution.query', { sessionId: 'foreign', messageIds: ids }),
      { resolutions: [] },
    );
    await reject(
      'queue.entry.promote',
      { sessionId, originHostEpoch, entryId: 'absent', promoteId: 'missing' },
      'not_found',
    );
    const reorder = { sessionId, originHostEpoch, reorderId: 'order', entryIds: ['two', 'one'] };
    await reject('session.lifecycle.set', { sessionId, state: 'archived' }, 'session_busy');
    let result = await invoke('queue.entries.reorder', reorder);
    assert.equal(result.queueRevision, queue.queueRevision + 1);
    await watch.waitFor(
      (f) =>
        f.kind === 'subscription.session_projection' &&
        f.snapshot.queue.queueRevision === result.queueRevision &&
        f.snapshot.queue.followup.map((e) => e.entryId).join(',') === 'two,one',
    );
    const update = {
      sessionId,
      originHostEpoch,
      updateId: 'edit',
      entryId: 'one',
      expectedQueueRevision: result.queueRevision,
      text: 'prefix 😀 @file done',
    };
    result = await invoke('queue.entry.update', update);
    const editResult = result;
    const frame = await watch.waitFor(
      (f) =>
        f.kind === 'subscription.session_projection' &&
        f.snapshot.queue.queueRevision === result.queueRevision,
    );
    const edited = frame.snapshot.queue.followup.find((e) => e.entryId === 'one');
    assert.equal(edited.content.text, update.text);
    assert.equal(edited.content.inlineReferences[0].start, 10);
    await reject('queue.entry.update', { ...update, updateId: 'stale' }, 'operation_conflict');
    const second = await openClient();
    try {
      assert.deepEqual(
        await invoke('queue.entry.update', update, second),
        editResult,
        'cross-connection replay has no second mutation',
      );
    } finally {
      await second.close();
    }
    result = await invoke('queue.entry.update', {
      ...update,
      updateId: 'remove-reference',
      expectedQueueRevision: result.queueRevision,
      text: 'plain',
    });
    const plain = await watch.waitFor(
      (f) =>
        f.kind === 'subscription.session_projection' &&
        f.snapshot.queue.queueRevision === result.queueRevision,
    );
    assert.deepEqual(
      plain.snapshot.queue.followup.find((e) => e.entryId === 'one').content.inlineReferences,
      [],
    );
    assert.deepEqual(
      await invoke('queue.entry.update', update),
      editResult,
      'old exact command keeps its original result',
    );
    await reject('queue.entry.update', { ...update, text: 'different' }, 'operation_conflict');
    const retractOne = { sessionId, originHostEpoch, retractId: 'single', entryId: 'one' };
    const one = await invoke('queue.entry.retract', retractOne);
    assert.deepEqual(await invoke('queue.entry.retract', retractOne), one);
    await reject('queue.entry.retract', { ...retractOne, entryId: 'two' }, 'operation_conflict');
    const promoted = await invoke('queue.entry.promote', {
      sessionId,
      originHostEpoch,
      entryId: 'two',
      promoteId: 'promote-two',
    });
    assert.equal(promoted.queueRevision, one.queueRevision + 1);
    const retract = { sessionId, originHostEpoch, retractId: 'all' };
    const all = await invoke('queue.retract', retract);
    assert.deepEqual(
      all.retracted.map((e) => [e.messageId, e.state]),
      [
        ['steer', 'retracted'],
        ['two', 'retracted'],
      ],
    );
    assert.deepEqual(await invoke('queue.retract', retract), all);
    console.log('message-queue-receipt-retention-started');
    for (let index = 0; index < 1030; index++) {
      assert.deepEqual(
        await invoke('queue.entries.reorder', {
          sessionId,
          originHostEpoch,
          reorderId: `noop-${index}`,
          entryIds: [],
        }),
        { queueRevision: all.queueRevision },
      );
    }
    assert.deepEqual(
      await invoke('queue.entry.update', update),
      editResult,
      'the oldest receipt survives beyond the former lifetime capacity',
    );
    console.log('message-queue-receipt-retention-passed');
    await watch.waitFor(
      (f) =>
        f.kind === 'subscription.session_projection' &&
        f.snapshot.queue.queueRevision === all.queueRevision &&
        f.snapshot.queue.steering.length === 0 &&
        f.snapshot.queue.followup.length === 0,
    );
    assert.deepEqual(await invoke('turn.message.query', { sessionId, messageIds: ids }), {
      cancelledMessageIds: ['one', 'two', 'steer'],
    });
    const current = await invoke('turn.query', { sessionId, turnId: active.turnId });
    await invoke('turn.stop', { sessionId, turnId: current.turnId, runId: current.runId });
    await waitMessageTerminal(invoke, sessionId, current.turnId);
    assert.equal(model.requests.length, 2, 'retracted work never reaches a successor model');
    await writeFile(old, JSON.stringify({ retract, rootProof }));
    console.log('message-queue-passed');
  } finally {
    await watch.close();
    await model?.close();
  }
}
