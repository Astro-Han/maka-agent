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
import { querySession } from './client-runtime-policy-fixture.mjs';

export const createdTarget = (actionId) =>
  'whs_' +
  createHash('sha256')
    .update('create\0' + actionId)
    .digest('hex')
    .slice(0, 48);

export async function prepareRouting({
  request,
  act,
  initial,
  turnId,
  workspace,
  model,
  createNew,
}) {
  const base = {
    turnId,
    actionId: 'delegation-action',
    delegationText: 'Implement the requested task',
  };
  if (createNew) {
    const input = {
      ...base,
      proposal: { disposition: 'create_new', title: 'Created task' },
      create: { workspace: { kind: 'host_path', path: workspace } },
      newWorkDefaults: {
        model: {
          llmConnectionId: model.connectionId,
          llmConnectionSlug: 'delegation-fixture',
          model: 'fixture-model',
        },
        permissionMode: 'bypass',
      },
    };
    await assert.rejects(
      act({ ...input, create: { workspace: { kind: 'host_path', path: workspace + '/missing' } } }),
      (error) => error.code === 'operation_conflict',
    );
    await assert.rejects(
      act({
        ...input,
        newWorkDefaults: {
          ...input.newWorkDefaults,
          model: { ...input.newWorkDefaults.model, llmConnectionSlug: 'stale' },
        },
      }),
      (error) => error.code === 'operation_conflict',
    );
    const missing = await request('session.catalog.query', {
      kind: 'get',
      sessionId: createdTarget(input.actionId),
    });
    assert.deepEqual(missing, { kind: 'session', session: null });
    return input;
  }
  assert.deepEqual(
    await request('workhub.coordination.candidates', {}),
    initial,
    'WorkHub steps must not invalidate an unchanged candidate set',
  );
  const selected = initial.candidates[0];
  const current = await querySession(request, selected.sessionId);
  await request('session.metadata.update', {
    sessionId: current.id,
    expectedRevision: current.revision,
    patch: { name: 'renamed target' },
  });
  const input = {
    ...base,
    candidateSetId: initial.candidateSetId,
    proposal: { disposition: 'delegate_existing', candidateRef: selected.candidateRef },
  };
  await assert.rejects(act(input), (error) => error.code === 'candidate_set_stale');
  const fresh = await request('workhub.coordination.candidates', {});
  input.candidateSetId = fresh.candidateSetId;
  input.proposal.candidateRef = fresh.candidates[0].candidateRef;
  return input;
}
