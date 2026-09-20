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
import { createServer } from 'node:http';
import { once } from 'node:events';
import { setTimeout as delay } from 'node:timers/promises';
import { configureModel } from './client-runtime-policy-fixture.mjs';
import { toggleWorkhub, workhubRemote } from './client-workhub-plugin.mjs';

export async function verifyWorkhubQueue(connection, codeMode) {
  const sessionId = 'maka_workhub_coordination';
  const request = (op, input) => connection.request(op, input, 5000);
  const first = Promise.withResolvers();
  const release = Promise.withResolvers();
  const requests = [];
  const remotes = [];
  let failure;
  const server = createServer(async (request, response) => {
    try {
      let body = '';
      for await (const part of request) body += part;
      const input = JSON.parse(body);
      requests.push(input);
      assert(requests.length <= 3, 'queues must not duplicate model effects');
      const definitions = input.tools.map((tool) => tool.function);
      const exec = definitions.find((tool) => tool.name === 'exec');
      if (codeMode)
        assert(
          exec,
          `request ${requests.length} lost Code Mode: ${definitions.map((tool) => tool.name)}`,
        );
      const names = codeMode
        ? [
            ...JSON.parse(exec.description.split('Available nested functions:\n')[1]),
            ...definitions.filter((tool) => tool.name !== 'exec'),
          ].map((tool) => tool.name)
        : definitions.map((tool) => tool.name);
      assert.deepEqual(names.sort(), [
        'AskUserQuestion',
        'Read',
        'mcp__desktop_workhub__control',
        'workhub_tasks',
      ]);
      assert(
        input.messages.some(
          (message) => message.role === 'system' && message.content.includes('WorkHub assistant'),
        ),
      );
      if (requests.length === 1) {
        first.resolve();
        await release.promise;
      }
      response.writeHead(200, { 'Content-Type': 'text/event-stream' });
      response.end(
        'data: ' +
          JSON.stringify({
            id: 'queue-' + requests.length,
            object: 'chat.completion.chunk',
            created: 1,
            model: 'fixture-model',
            choices: [
              {
                index: 0,
                delta:
                  requests.length === 1
                    ? {
                        tool_calls: [
                          {
                            index: 0,
                            id: 'discover',
                            type: 'function',
                            function: {
                              name: codeMode ? 'exec' : 'workhub_tasks',
                              arguments: JSON.stringify(
                                codeMode
                                  ? {
                                      code: 'return await tools.workhub_tasks({request:{operation:"candidates"}});',
                                    }
                                  : { request: { operation: 'candidates' } },
                              ),
                            },
                          },
                        ],
                      }
                    : { content: 'QUEUE_OK' },
                finish_reason: requests.length === 1 ? 'tool_calls' : 'stop',
              },
            ],
          }) +
          '\n\ndata: [DONE]\n\n',
      );
    } catch (error) {
      failure = error;
      first.reject(error);
      response.writeHead(500);
      response.end(String(error));
    }
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  try {
    await configureModel(request, `http://127.0.0.1:${server.address().port}/v1`);
    if (codeMode) {
      const { revision, policy } = await request('runtime.policy.query', {});
      const configured = await request('runtime.policy.mutate', {
        expectedRevision: revision,
        operation: {
          kind: 'set_chat_defaults',
          value: { ...policy.chatDefaults, codeModeEnabled: true },
        },
      });
      assert.equal(configured.kind, 'committed');
    }
    await connection.replaceClientCapabilities(
      {
        offers: () => [
          {
            offerId: 'coordination',
            version: '1',
            affinity: 'session',
            hostPathAccess: 'none',
            label: 'WorkHub',
            tools: ['control', 'context'].map((name) => ({
              serverId: 'desktop_workhub',
              name,
              inputSchema: { type: 'object' },
            })),
          },
        ],
        call() {
          throw new Error('The model must not execute Client tools');
        },
        close() {},
      },
      3000,
    );
    let remote = await workhubRemote(connection);
    remotes.push(remote);
    await remote.method('resolve')();
    const lookup = await request('session.catalog.query', { kind: 'get', sessionId });
    assert.deepEqual(lookup.session, await remote.method('query')());
    const turnId = 'queue-root';
    await remote.method('answer')({ turnId, text: 'INITIAL' });
    await first.promise;
    const input = {
      originHostEpoch: connection.hostEpoch,
      expectedTurnId: turnId,
      messageId: 'followup',
      content: { text: 'FOLLOWUP' },
      placement: 'next_turn',
    };
    const enqueue = remote.method('enqueue');
    await assert.rejects(
      enqueue({ ...input, expectedTurnId: 'unrelated' }),
      (error) => error.code === 'operation_conflict',
    );
    const { expectedTurnId: _expectedTurn, ...ordinary } = input;
    await assert.rejects(
      request('turn.message.submit', { ...ordinary, sessionId }),
      (error) => error.code === 'operation_conflict',
    );
    const steering = {
      ...input,
      messageId: 'steering',
      content: { text: 'STEERING' },
      placement: 'current_turn',
    };
    assert.equal((await enqueue(steering)).disposition, 'steering');
    const receipt = await enqueue(input);
    assert.equal(receipt.disposition, 'followup');
    assert.deepEqual(await enqueue(input), receipt);
    await assert.rejects(
      enqueue({ ...input, content: { text: 'CHANGED' } }),
      (error) => error.code === 'operation_conflict',
    );
    // Retraction/order control accepted work; editing must prepare new content
    // against an active behavior and promotion must retain the frozen policy.
    await enqueue({ ...input, messageId: 'promoted', content: { text: 'PROMOTED' } });
    await enqueue({ ...input, messageId: 'discarded', content: { text: 'DISCARDED' } });
    await toggleWorkhub(connection, true);
    const scope = { sessionId, originHostEpoch: connection.hostEpoch };
    const reordered = await request('queue.entries.reorder', {
      ...scope,
      reorderId: 'reorder',
      entryIds: ['discarded', 'followup', 'promoted'],
    });
    const edit = {
      ...scope,
      updateId: 'edit',
      entryId: 'followup',
      expectedQueueRevision: reordered.queueRevision,
      text: 'EDITED_FOLLOWUP',
    };
    await assert.rejects(
      request('queue.entry.update', edit),
      (error) => error.code === 'operation_unavailable',
    );
    await toggleWorkhub(connection, false);
    await request('queue.entry.update', edit);
    await request('queue.entry.promote', { ...scope, promoteId: 'promote', entryId: 'promoted' });
    await toggleWorkhub(connection, true);
    const retract = { ...scope, retractId: 'discard', entryId: 'discarded' };
    assert.deepEqual(
      await request('queue.entry.retract', retract),
      await request('queue.entry.retract', retract),
    );
    assert.deepEqual(
      (
        await request('turn.message.execution.query', {
          sessionId,
          messageIds: ['discarded'],
        })
      ).resolutions,
      [{ messageId: 'discarded', state: 'cancelled' }],
    );
    await toggleWorkhub(connection, false);
    remote = await workhubRemote(connection);
    remotes.push(remote);
    const replay = remote.method('enqueue');
    assert.deepEqual(
      await replay(input),
      receipt,
      'editing cannot change the original admission receipt',
    );
    release.resolve();
    const deadline = Date.now() + 8000;
    let resolutions;
    for (;;) {
      if (failure) throw failure;
      const page = await request('turn.message.execution.query', {
        sessionId,
        messageIds: ['steering', 'promoted', 'followup'],
      });
      resolutions = page.resolutions;
      if (resolutions.length === 3 && resolutions.every((item) => item.state === 'owned')) {
        const turns = await Promise.all(
          resolutions.map((item) => request('turn.query', { sessionId, turnId: item.turnId })),
        );
        if (turns.every((turn) => turn.status === 'completed')) break;
      }
      assert(Date.now() < deadline, 'managed queue did not settle');
      await delay(10);
    }
    assert.equal(resolutions.find((item) => item.messageId === 'steering').turnId, turnId);
    assert.equal(resolutions.find((item) => item.messageId === 'promoted').turnId, turnId);
    assert.notEqual(resolutions.find((item) => item.messageId === 'followup').turnId, turnId);
    assert.equal(requests.length, 3);
    assert(JSON.stringify(requests[1].messages).includes('STEERING'));
    assert(JSON.stringify(requests[1].messages).includes('PROMOTED'));
    assert(JSON.stringify(requests[2].messages).includes('EDITED_FOLLOWUP'));
    assert(!JSON.stringify(requests).includes('DISCARDED'));
    await assert.rejects(
      replay({ ...input, messageId: 'late' }),
      (error) => error.code === 'operation_conflict',
    );
    assert.equal(
      (await replay(input)).disposition,
      'followup',
      'exact replay precedes active-Turn checks',
    );
    await toggleWorkhub(connection, true);
    await assert.rejects(replay({ ...input, messageId: 'retired' }));
    await toggleWorkhub(connection, false);
    remote = await workhubRemote(connection);
    remotes.push(remote);
    assert.equal(
      (await remote.method('enqueue')({ ...input, originHostEpoch: 'past-epoch' })).disposition,
      'followup',
    );
    await assert.rejects(
      remote.method('enqueue')({ ...input, originHostEpoch: 'past-epoch', messageId: 'unproven' }),
      (error) => error.code === 'outcome_unknown',
    );
    assert.equal(requests.length, 3);
  } finally {
    release.resolve();
    await Promise.all(remotes.map((remote) => remote.close()));
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  }
}
