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
import { once } from 'node:events';
import { createServer } from 'node:http';
import { readFile, realpath, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { decodeStoredMessage } from '../../packages/core/src/session.ts';
import { watchSession } from './client-subscription.mjs';
import { createInput, querySession } from './client-runtime-policy-fixture.mjs';
import { upload } from './client-artifact-upload.mjs';
import { createdTarget, prepareRouting } from './client-workhub-routing.mjs';
import { chooseTarget, setupSelection } from './client-workhub-selection.mjs';
import { resumeDelegation } from './client-workhub-resume.mjs';

const sessionId = 'maka_workhub_coordination';
const turnId = 'delegate-request';

async function coordinationRecord(observer, sequence, kind, actionId) {
  for (;;) {
    const frame = await observer.waitFor(
      (frame) => frame.kind === 'subscription.transcript_advanced' && frame.sequence > sequence,
    );
    sequence = frame.sequence;
    // loadTranscript is a frozen bootstrap; live reads use the announced fence.
    const page = await observer.subscription.loadTranscriptPage({
      source: 'durable',
      direction: 'older',
      throughSequence: frame.throughSequence,
      cursor: null,
      anchorSequence: null,
      maxBytes: 48 * 1024,
    });
    const decoded = await observer.subscription.decodeTranscriptPage(page, decodeStoredMessage);
    assert.equal(decoded.nextCursor, null);
    const row = decoded.messages.find(
      ({ message }) =>
        message.type === 'workhub_coordination' &&
        message.kind === kind &&
        message.actionId === actionId,
    );
    if (row) return row.message;
  }
}

export async function verifyWorkhubDelegation(connection, workspace, reopened, mode = 'existing') {
  const createNew = mode === 'created';
  const selectTarget = mode === 'selected';
  const stopTarget = mode === 'stopped';
  const steerTarget = mode === 'steered';
  const resumeTarget = mode === 'resumed';
  let resumed = false;
  let resumeReceipts = [];
  const targetReady = Promise.withResolvers();
  const initialTarget = Promise.withResolvers();
  const delegated = Promise.withResolvers();
  const finishTarget = Promise.withResolvers();
  const targetSessionId = createNew ? createdTarget('delegation-action') : 'target';
  const request = (operation, input) => connection.request(operation, input, 5000);
  const file = join(workspace, 'delegation.json');
  const act = async (input) => {
    const result = await request(
      selectTarget ? 'workhub.coordination.selectAndDelegate' : 'workhub.coordination.actFromTurn',
      input,
    );
    return selectTarget && result.kind === 'delegated' ? result.result : result;
  };
  if (reopened) {
    const saved = JSON.parse(await readFile(file, 'utf8'));
    assert.deepEqual(await act(saved.input), saved.receipt);
    if (saved.stopInput) assert.deepEqual(await act(saved.stopInput), saved.stopReceipt);
    for (const item of saved.resumeReceipts ?? [])
      assert.deepEqual(await act(item.input), item.receipt);
    if (saved.coordinationRows) {
      const source = await watchSession(connection, sessionId, { kind: 'tail', maxBytes: 2 });
      try {
        assert.deepEqual(
          await source.subscription.loadTranscript(decodeStoredMessage),
          saved.coordinationRows,
        );
      } finally {
        await source.close();
      }
    }
    assert.deepEqual(
      await request('turn.query', {
        sessionId: saved.receipt.targetSessionId,
        turnId: saved.target.turnId,
      }),
      saved.target,
    );
    const observer = await watchSession(connection, saved.receipt.targetSessionId, {
      kind: 'tail',
      maxBytes: 2,
    });
    try {
      assert.deepEqual(await observer.subscription.loadTranscript(decodeStoredMessage), saved.rows);
    } finally {
      await observer.close();
    }
    return;
  }
  let failure,
    calls = 0,
    input,
    receipt,
    assignmentRow,
    attachment,
    stopInput,
    stopReceipt,
    coordinationRows;
  const server = createServer(async (req, response) => {
    try {
      let body = '';
      for await (const chunk of req) body += chunk;
      const data = JSON.parse(body);
      if (data.messages.at(-1)?.content === 'UNRELATED_FAILURE') {
        response.writeHead(400, { 'Content-Type': 'application/json', Connection: 'close' });
        response.end(
          JSON.stringify({
            error: { message: 'unrelated failure', type: 'invalid_request_error' },
          }),
        );
        return;
      }
      const target = data.messages.some(
        (message) =>
          typeof message.content === 'string' &&
          message.content.includes('Delegated task:\nImplement the requested task'),
      );
      const bootstrap =
        !target &&
        data.messages.some((message) => message.content === 'Existing task is already running');
      const finished = target
        ? data.messages.some(
            (message) =>
              message.role === 'tool' &&
              JSON.stringify(message.content).includes('TRANSFER_EVIDENCE'),
          )
        : data.messages.some((message) => message.role === 'tool');
      const path = target
        ? body.match(/maka:\/\/runtime\/attachments\/[A-Za-z0-9_-]+/u)?.[0]
        : undefined;
      if (target) {
        assert(path);
        assert(!path.endsWith('/' + attachment.ref.relativePath));
        if (finished) assert(body.includes('TRANSFER_EVIDENCE'));
        if (finished && (stopTarget || steerTarget || (resumeTarget && !resumed))) {
          response.writeHead(200, { 'Content-Type': 'text/event-stream' });
          response.flushHeaders();
          targetReady.resolve();
          if (stopTarget || resumeTarget) return;
          await finishTarget.promise;
        }
      }
      if (bootstrap) {
        initialTarget.resolve();
        await delegated.promise;
      }
      const delta = finished
        ? { content: target ? 'target completed' : 'delegation completed' }
        : {
            tool_calls: [
              {
                index: 0,
                id: 'delegate-call',
                type: 'function',
                function: {
                  name: target || bootstrap ? 'Read' : 'mcp__desktop_workhub__tasks',
                  arguments: JSON.stringify(
                    target ? { path } : bootstrap ? { path: join(workspace, 'seed.txt') } : {},
                  ),
                },
              },
            ],
          };
      if (target) {
        assert(
          data.messages.some(
            (message) =>
              typeof message.content === 'string' &&
              message.content.includes('User request:\nPlease implement the requested task'),
          ),
        );
        assert(!data.tools.some((tool) => tool.function.name.startsWith('mcp__desktop_workhub__')));
      }
      const chunk = (delta, finish_reason) => ({
        id: 'chat-delegation',
        object: 'chat.completion.chunk',
        created: 1,
        model: 'fixture-model',
        choices: [{ index: 0, delta, finish_reason }],
      });
      if (!response.headersSent)
        response.writeHead(200, { 'Content-Type': 'text/event-stream', Connection: 'close' });
      response.end(
        [chunk(delta, null), chunk({}, finished ? 'stop' : 'tool_calls')]
          .map((event) => 'data: ' + JSON.stringify(event) + '\n\n')
          .join('') + 'data: [DONE]\n\n',
      );
    } catch (error) {
      failure = error;
      response.destroy(error);
    }
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  let sourceObserver, targetObserver;
  try {
    const baseUrl = 'http://127.0.0.1:' + server.address().port + '/v1';
    const created = await request('connection.catalog.create', {
      expectedCatalogRevision: 0,
      connection: {
        slug: 'delegation-fixture',
        name: 'Delegation fixture',
        providerType: 'openai-compatible',
        baseUrl,
        enabled: true,
        enabledModelIds: ['fixture-model'],
      },
    });
    const basis = created.connection;
    await request('credential.vault.set', {
      locator: { scope: 'connection', connectionId: basis.connectionId, kind: 'api_key' },
      expected: null,
      expectedConnection: {
        ...basis,
        slug: 'delegation-fixture',
        providerType: 'openai-compatible',
        effectiveBaseUrl: baseUrl,
      },
      secret: 'fixture-only',
    });
    await request('connection.catalog.set-default-target', {
      expectedCatalogRevision: created.catalogRevision,
      target: { connectionId: basis.connectionId, modelId: 'fixture-model' },
    });
    for (const [id, extra] of createNew
      ? []
      : [
          ['target', {}],
          ['side', { labels: ['mode:side_conversation'] }],
          ['planned', { collaborationMode: 'plan' }],
          ['archived', {}],
        ])
      await request('session.create', { ...createInput(workspace, id, 'bypass'), ...extra });
    if (!createNew)
      await request('session.lifecycle.set', { sessionId: 'archived', state: 'archived' });
    if (selectTarget) await setupSelection(request, workspace);
    await request('workhub.coordination.resolve', {});
    attachment = await upload(
      request,
      sessionId,
      'transfer',
      Buffer.from('TRANSFER_EVIDENCE'),
      'evidence.txt',
      'text/plain',
    );
    if (steerTarget) {
      await writeFile(join(workspace, 'seed.txt'), 'BEFORE_STEERING');
      await request('turn.start', {
        sessionId: targetSessionId,
        turnId: 'already-running',
        content: { text: 'Existing task is already running' },
      });
      await initialTarget.promise;
    }
    const initial = await request('workhub.coordination.candidates', {});
    assert.deepEqual(
      initial.candidates.map((candidate) => candidate.sessionId).sort(),
      createNew ? [] : selectTarget ? ['another', 'target'] : ['target'],
    );
    if (!createNew)
      targetObserver = await watchSession(connection, targetSessionId, {
        kind: 'tail',
        maxBytes: 2,
      });
    await connection.replaceClientCapabilities(
      {
        offers: () => [
          {
            offerId: 'workhub',
            label: 'Desktop WorkHub',
            version: '1',
            affinity: 'session',
            hostPathAccess: 'none',
            tools: ['control', 'tasks'].map((name) => ({
              serverId: 'desktop_workhub',
              name,
              inputSchema: { type: 'object' },
            })),
          },
        ],
        async call(frame, { accept }) {
          try {
            assert.equal(frame.sessionId, sessionId);
            assert.equal(frame.turnId, turnId);
            assert.equal(frame.toolName, 'tasks');
            await accept({ kind: 'none' });
            input = await prepareRouting({
              request,
              act,
              initial,
              turnId,
              workspace,
              model: basis,
              createNew,
              selectTarget,
            });
            await assert.rejects(
              act({ ...input, turnId: 'not-active' }),
              (error) => error.code === 'operation_conflict',
            );
            const beforeAssignment = sourceObserver.frames.at(-1)?.sequence ?? 0;
            receipt = selectTarget
              ? await chooseTarget(request, act, sourceObserver, input, workspace)
              : await act(input);
            assert.equal(receipt.disposition, createNew ? 'create_new' : 'delegate_existing');
            assert.equal(receipt.targetSessionId, targetSessionId);
            const assignment = await coordinationRecord(
              sourceObserver,
              beforeAssignment,
              'delegation_assigned',
              input.actionId,
            );
            assert.equal(assignment.delegationId, assignment.id);
            assignmentRow = assignment;
            assert.equal(assignment.targetSessionId, targetSessionId);
            assert.equal(assignment.targetTurnId, receipt.targetTurnId);
            assert.equal(assignment.disposition, receipt.disposition);
            assert.equal(assignment.userText, 'Please implement the requested task');
            assert.deepEqual(assignment.attachments, [attachment]);
            assert.equal(assignment.targetAttachments.length, 1);
            assert.equal(assignment.targetAttachments[0].ref.sessionId, targetSessionId);
            assert.equal(assignment.steered, steerTarget ? true : undefined);
            if (createNew)
              assert.deepEqual(assignment.create, {
                title: input.proposal.title,
                workspace: input.create.workspace,
                defaults: input.newWorkDefaults,
              });
            else assert.equal(assignment.create, undefined);
            if (steerTarget) {
              assert.equal(receipt.steered, true);
              assert.equal(receipt.targetTurnId, 'already-running');
              delegated.resolve();
            }
            if (resumeTarget) {
              await targetReady.promise;
              resumeReceipts = await resumeDelegation(
                request,
                act,
                input,
                receipt,
                targetObserver,
                () => {
                  resumed = true;
                },
              );
            }
            if (stopTarget || steerTarget) {
              await targetReady.promise;
              stopInput = {
                turnId,
                actionId: 'stop-action',
                proposal: {
                  operation: 'stop',
                  expects: { targetSessionId },
                },
              };
              const sourceSequence = sourceObserver.frames.at(-1)?.sequence ?? 0;
              stopReceipt = await act(stopInput);
              // The coordinator stays inside this tool call: no subsequent
              // Invocation boundary or reconnect can hide a missed control wakeup.
              await coordinationRecord(
                sourceObserver,
                sourceSequence,
                'delegation_stop_resolved',
                stopInput.actionId,
              );
              assert.deepEqual(stopReceipt, {
                disposition: 'stop_work',
                outcome: steerTarget ? 'not_owned' : 'stop_delivered',
                targetSessionId,
                targetTurnId: receipt.targetTurnId,
              });
              assert.deepEqual(await act(stopInput), stopReceipt);
              await assert.rejects(
                act({
                  ...stopInput,
                  proposal: { operation: 'stop', expects: { targetSessionId: 'side' } },
                }),
                (error) => error.code === 'operation_conflict',
              );
              if (steerTarget) finishTarget.resolve();
            }
            if (createNew) {
              const created = await querySession(request, targetSessionId);
              assert.equal(created.name, 'Created task');
              assert.equal(created.permissionMode, 'bypass');
              assert.equal(created.collaborationMode, 'agent');
              assert.equal(created.orchestrationMode, 'default');
              assert.equal(created.workspace.hostCwd, await realpath(workspace));
            }
            await request('artifact.delete', {
              sessionId,
              artifactId: attachment.ref.relativePath,
            });
            assert.deepEqual(await act(input), receipt);
            await assert.rejects(
              act({ ...input, delegationText: 'changed request' }),
              (error) => error.code === 'operation_conflict',
            );
            calls++;
            return { content: [{ type: 'text', text: 'Delegation admitted' }] };
          } catch (error) {
            failure = error;
            throw error;
          }
        },
        close() {},
      },
      3000,
    );
    sourceObserver = await watchSession(connection, sessionId, { kind: 'tail', maxBytes: 2 });
    await request('workhub.coordination.answer', {
      turnId,
      text: 'Please implement the requested task',
      attachments: [attachment],
    });
    await sourceObserver.waitFor(
      (frame) =>
        frame.kind === 'subscription.session_projection' &&
        frame.snapshot.rootTurn?.turnId === turnId &&
        ['completed', 'failed', 'cancelled'].includes(frame.snapshot.rootTurn.status),
    );
    if (failure) throw failure;
    assert.equal(calls, 1);
    assert.equal((await request('turn.query', { sessionId, turnId })).status, 'completed');
    targetObserver ??= await watchSession(connection, targetSessionId, {
      kind: 'tail',
      maxBytes: 2,
    });
    const targetTurnId = resumeReceipts.at(-1)?.receipt.targetTurnId ?? receipt.targetTurnId;
    const targetFinished = (snapshot) =>
      snapshot.rootTurn?.turnId === targetTurnId &&
      ['completed', 'failed', 'cancelled'].includes(snapshot.rootTurn.status);
    // Creation can finish before subscribing; the bootstrap is authoritative too.
    if (!targetFinished(targetObserver.subscription.snapshot))
      await targetObserver.waitFor(
        (frame) =>
          frame.kind === 'subscription.session_projection' && targetFinished(frame.snapshot),
      );
    if (failure) throw failure;
    const target = await request('turn.query', {
      sessionId: targetSessionId,
      turnId: targetTurnId,
    });
    assert.equal(target.status, stopTarget ? 'cancelled' : 'completed');
    if (stopTarget)
      assert.equal(
        target.abortSource,
        'workhub.direct_stop.' +
          createHash('sha256').update(stopInput.actionId).digest('hex').slice(0, 48),
      );
    if (resumeTarget) {
      await request('turn.start', {
        sessionId: targetSessionId,
        turnId: 'unrelated-failure',
        content: { text: 'UNRELATED_FAILURE' },
      });
      const unrelatedFinished = (snapshot) =>
        snapshot.rootTurn?.turnId === 'unrelated-failure' && snapshot.rootTurn.status === 'failed';
      if (!unrelatedFinished(targetObserver.subscription.snapshot))
        await targetObserver.waitFor(
          (frame) =>
            frame.kind === 'subscription.session_projection' && unrelatedFinished(frame.snapshot),
        );
    }
    await targetObserver.close();
    const beforeRename = await querySession(request, targetSessionId);
    await request('session.metadata.update', {
      sessionId: targetSessionId,
      expectedRevision: beforeRename.revision,
      patch: { name: 'Renamed after assignment' },
    });
    targetObserver = await watchSession(connection, targetSessionId, { kind: 'tail', maxBytes: 2 });
    const rows = await targetObserver.subscription.loadTranscript(decodeStoredMessage);
    {
      await sourceObserver.close();
      sourceObserver = await watchSession(connection, sessionId, { kind: 'tail', maxBytes: 2 });
      coordinationRows = await sourceObserver.subscription.loadTranscript(decodeStoredMessage);
      const assignments = coordinationRows.filter(
        (row) => row.type === 'workhub_coordination' && row.kind === 'delegation_assigned',
      );
      assert.deepEqual(assignments, [assignmentRow]);
      const candidates = await request('workhub.coordination.candidates', {});
      assert.equal(
        candidates.candidates.find((candidate) => candidate.sessionId === targetSessionId)
          ?.latestDelegationActionId,
        stopTarget ? undefined : input.actionId,
      );
    }
    if (stopInput) {
      const control = coordinationRows.filter(
        (row) => row.type === 'workhub_coordination' && row.kind !== 'delegation_assigned',
      );
      assert.equal(control.length, 2);
      assert.equal(control[0].kind, 'delegation_stop_requested');
      assert.equal(control[1].kind, 'delegation_stop_resolved');
      for (const row of control) {
        assert.equal(row.schemaVersion, 3);
        assert.equal(row.actionId, stopInput.actionId);
        assert.equal(row.stopsActionId, input.actionId);
        assert.equal(row.targetSessionId, targetSessionId);
        assert.equal(row.coordinationTurnId, turnId);
      }
      assert.equal(control[1].outcome, stopReceipt.outcome);
    }
    if (!stopTarget)
      assert(rows.some((row) => row.type === 'assistant' && row.text === 'target completed'));
    await connection.unregisterClientCapabilities(3000);
    await request('connection.catalog.remove', {
      expected: { connectionId: basis.connectionId, revision: basis.revision },
    });
    assert.deepEqual(await act(input), receipt);
    if (stopInput) assert.deepEqual(await act(stopInput), stopReceipt);
    for (const item of resumeReceipts) assert.deepEqual(await act(item.input), item.receipt);
    await writeFile(
      file,
      JSON.stringify({
        input,
        receipt,
        target,
        rows,
        stopInput,
        stopReceipt,
        resumeReceipts,
        coordinationRows,
      }),
    );
  } finally {
    await sourceObserver?.close();
    await targetObserver?.close();
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  }
}
