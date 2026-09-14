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

export async function resumeDelegation(request, act, input, receipt, observer, release) {
  const targetSessionId = receipt.targetSessionId;
  const observation = {
    turnId: input.turnId,
    actionId: 'resume-observation',
    proposal: {
      operation: 'resume',
      resumesActionId: input.actionId,
      expects: { targetSessionId },
    },
  };
  const observed = await act(observation);
  assert.deepEqual(observed, {
    disposition: 'resume_work',
    outcome: 'already_running',
    targetSessionId,
  });
  const target = await request('turn.query', {
    sessionId: targetSessionId,
    turnId: receipt.targetTurnId,
  });
  await request('turn.stop', {
    sessionId: targetSessionId,
    turnId: target.turnId,
    runId: target.runId,
  });
  const cancelled = (snapshot) =>
    snapshot.rootTurn?.turnId === target.turnId && snapshot.rootTurn.status === 'cancelled';
  if (!cancelled(observer.subscription.snapshot))
    await observer.waitFor(
      (frame) => frame.kind === 'subscription.session_projection' && cancelled(frame.snapshot),
    );
  assert.equal(
    (await request('turn.query', { sessionId: targetSessionId, turnId: target.turnId })).status,
    'cancelled',
  );
  assert.deepEqual(await act(observation), observed, 'retry retains the original observation');
  release();
  const start = { ...observation, actionId: 'resume-start' };
  const resumed = await act(start);
  assert.equal(resumed.disposition, 'resume_work');
  assert.equal(resumed.outcome, 'resume_started');
  assert.equal(resumed.targetSessionId, targetSessionId);
  assert.notEqual(resumed.targetTurnId, target.turnId);
  assert.deepEqual(await act(start), resumed);
  for (const changed of [
    { ...start, actionId: input.actionId },
    { ...start, proposal: { ...start.proposal, resumesActionId: 'another-delegation' } },
    { ...start, proposal: { ...start.proposal, expects: { targetSessionId: 'side' } } },
    { ...input, actionId: start.actionId },
  ]) {
    await assert.rejects(act(changed), (error) => error.code === 'operation_conflict');
  }
  return [
    { input: observation, receipt: observed },
    { input: start, receipt: resumed },
  ];
}
