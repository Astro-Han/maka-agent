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
import { createInput } from './client-runtime-policy-fixture.mjs';
import { createdTarget } from './client-workhub-routing.mjs';
import { watchSession } from './client-subscription.mjs';
import { resumeDelegation } from './client-workhub-resume.mjs';

export async function correctDelegation({
  connection,
  request,
  act,
  sourceObserver,
  input,
  receipt,
  assignment,
  workspace,
  createNew,
  readRecord,
  ready,
  release,
  toggleWorkhub,
}) {
  const actionId = 'correction-action';
  if (!createNew) await request('session.create', createInput(workspace, 'replacement', 'bypass'));
  const candidates = await request('workhub.coordination.candidates', {});
  const target = candidates.candidates.find((candidate) => candidate.sessionId === 'replacement');
  const correction = {
    turnId: input.turnId,
    actionId,
    proposal: {
      operation: 'correct',
      replacesActionId: input.actionId,
      target: createNew
        ? { disposition: 'create_new', title: '  Replacement task  ' }
        : { disposition: 'delegate_existing', candidateRef: target.candidateRef },
    },
    ...(createNew
      ? { create: { workspace: { kind: 'host_path', path: workspace + '/.' } } }
      : { candidateSetId: candidates.candidateSetId }),
    delegationText: 'Corrected replacement task',
  };
  const before = sourceObserver.frames.at(-1)?.sequence ?? 0;
  await toggleWorkhub(true);
  await assert.rejects(act(correction), (error) => error.code === 'operation_unavailable');
  await toggleWorkhub(false);
  const [result, concurrentReceipt] = await Promise.all([act(correction), act(correction)]);
  assert.deepEqual(concurrentReceipt, result);
  assert.equal(result.disposition, 'replace');
  assert.equal(result.replacementDisposition, createNew ? 'create_new' : 'delegate_existing');
  const targetId = createNew ? createdTarget(actionId) : 'replacement';
  assert.equal(result.targetSessionId, targetId);
  const requested = await readRecord(
    sourceObserver,
    before,
    'delegation_replacement_requested',
    actionId,
  );
  const replacement = await readRecord(sourceObserver, before, 'delegation_assigned', actionId);
  const superseded = await readRecord(sourceObserver, before, 'delegation_superseded', actionId);
  for (const row of [requested, replacement, superseded]) {
    assert.equal(row.schemaVersion, 2);
    assert.equal(row.coordinationTurnId, input.turnId);
  }
  for (const row of [requested, replacement]) {
    assert.equal(row.replacesActionId, input.actionId);
    assert.equal(row.replacesDelegationId, assignment.id);
    assert.equal(row.targetSessionId, targetId);
    assert.equal(row.userText, assignment.userText);
    assert.deepEqual(row.attachments, assignment.attachments);
    assert.equal(row.delegationText, correction.delegationText);
  }
  assert.equal(requested.replacedTargetSessionId, receipt.targetSessionId);
  assert.equal(requested.replacedTargetMessageId, assignment.targetMessageId);
  assert.equal(replacement.delegationId, replacement.id);
  assert.equal(replacement.targetAttachments[0].ref.sessionId, targetId);
  assert.equal(superseded.supersededActionId, input.actionId);
  assert.equal(superseded.supersededDelegationId, assignment.id);
  assert.equal(superseded.replacementDelegationId, replacement.id);
  if (createNew)
    assert.deepEqual(replacement.create, {
      title: correction.proposal.target.title,
      workspace: correction.create.workspace,
    });
  await toggleWorkhub(true);
  await toggleWorkhub(false);
  assert.deepEqual(await act(correction), result);
  await assert.rejects(
    act({ ...correction, delegationText: 'different correction' }),
    (error) => error.code === 'operation_conflict',
  );
  const observer = await watchSession(connection, targetId, { kind: 'tail', maxBytes: 2 });
  let resumeReceipts;
  try {
    await ready;
    resumeReceipts = await resumeDelegation(request, act, correction, result, observer, release);
    const finalTurnId = resumeReceipts.at(-1).receipt.targetTurnId;
    const finished = (snapshot) =>
      snapshot.rootTurn?.turnId === finalTurnId &&
      ['completed', 'failed', 'cancelled'].includes(snapshot.rootTurn.status);
    if (!finished(observer.subscription.snapshot))
      await observer.waitFor(
        (frame) => frame.kind === 'subscription.session_projection' && finished(frame.snapshot),
      );
    assert.equal(
      (await request('turn.query', { sessionId: targetId, turnId: finalTurnId })).status,
      'completed',
    );
  } finally {
    await observer.close();
  }
  const latest = await request('workhub.coordination.candidates', {});
  assert.equal(
    latest.candidates.find((candidate) => candidate.sessionId === targetId)
      .latestDelegationActionId,
    actionId,
  );
  assert.equal(
    latest.candidates.find((candidate) => candidate.sessionId === receipt.targetSessionId)
      ?.latestDelegationActionId,
    undefined,
  );
  return {
    input: correction,
    receipt: result,
    assignment: replacement,
    superseded,
    requested,
    resumeReceipts,
  };
}
