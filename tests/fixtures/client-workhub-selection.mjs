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
import { createInput, querySession } from './client-runtime-policy-fixture.mjs';

const sessionId = 'maka_workhub_coordination';

export async function setupSelection(request, workspace) {
  await request('session.create', createInput(workspace, 'another', 'bypass'));
  for (const id of ['target', 'another']) {
    const session = await querySession(request, id);
    await request('session.metadata.update', {
      sessionId: id,
      expectedRevision: session.revision,
      patch: { name: '界'.repeat(70) + id },
    });
  }
}

export async function chooseTarget(request, act, observer, input, workspace) {
  const offer = async (actionId) => {
    const frame = await observer.waitFor(
      (frame) =>
        frame.kind === 'subscription.session_projection' &&
        frame.snapshot.interactions.pending.some(
          (form) => form.request.kind === 'form' && form.request.toolUseId === actionId,
        ),
    );
    return frame.snapshot.interactions.pending.find((form) => form.request.toolUseId === actionId);
  };
  const answer = (form, result) =>
    request('interaction.answer', {
      sessionId,
      interactionId: form.interactionId,
      answer: { kind: 'form', ...result },
    });
  const cancelledInput = { ...input, actionId: 'cancelled-selection' };
  const cancelled = act(cancelledInput);
  cancelled.catch(() => {});
  const dismissed = await offer(cancelledInput.actionId);
  await answer(dismissed, { action: 'cancel' });
  assert.deepEqual(await cancelled, { kind: 'cancelled' });
  assert.deepEqual(await act(cancelledInput), { kind: 'cancelled' });
  const pending = act(input);
  pending.catch(() => {});
  const form = await offer(input.actionId);
  const field = form.request.fields[0];
  assert.equal(field.kind, 'single_select');
  assert.equal(field.options.length, 2);
  assert.equal(new Set(field.options.map((option) => option.label)).size, 2);
  await assert.rejects(
    answer(form, { action: 'accept', values: { target: 'not-an-offered-value' } }),
    (error) => error.code === 'operation_conflict',
  );
  // While the chooser waits, another Session can change without invalidating
  // this still-eligible target or changing the already-published form.
  const other = await querySession(request, 'another');
  await request('session.metadata.update', {
    sessionId: other.id,
    expectedRevision: other.revision,
    patch: { name: 'unrelated change' },
  });
  for (let i = 0; i < 32; i++)
    await request('session.create', createInput(workspace, 'new-' + i, 'bypass'));
  const changed = await request('workhub.coordination.candidates', {});
  assert.notEqual(changed.candidateSetId, input.candidateSetId);
  assert(!changed.candidates.some((candidate) => candidate.sessionId === 'target'));
  const selected = field.options.find((option) => JSON.parse(option.value)[1] === 'target');
  assert(selected);
  await answer(form, { action: 'accept', values: { target: selected.value } });
  const receipt = await pending;
  assert.deepEqual(await act(input), receipt);
  return receipt;
}
