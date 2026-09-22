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
import { once } from 'node:events';
import { createServer } from 'node:http';
import { setTimeout as delay } from 'node:timers/promises';

export const text = ('pruning needle ' + 'x'.repeat(80) + '\n').repeat(128);
export function summary(ref) {
  return [
    '## Goal',
    'PRUNING_PRIVATE_SUMMARY: Continue using the archived workspace evidence.',
    '## Progress',
    'Captured command output and bounded archive reading completed successfully.',
    '## Next Steps',
    'Use Read with the retained reference as path without reopening the source file.',
    '## Critical Context',
    'The persistent evidence reference is ' + ref + '.',
  ].join('\n');
}
function bounded(message) {
  assert(message.content.length <= 7500);
  return JSON.parse(message.content);
}
function read(value, offset, limit) {
  assert.equal(value.offset, offset);
  assert.equal(value.returnedLines, limit);
  assert.equal(
    value.content,
    text
      .split('\n')
      .slice(offset, offset + limit)
      .join('\n'),
  );
  assert.equal(value.totalLines, 129);
  assert.equal(value.next, null);
  assert.equal(value.metadata.kind, 'terminal');
  assert.equal(value.metadata.exitCode, 0);
  assert.equal(value.metadata.stdoutTruncated, false);
}
export async function pruningFixture(port = 0, reopened = false) {
  let count = 0,
    failure,
    arrive,
    release;
  const arrived = new Promise((resolve) => {
    arrive = resolve;
  });
  const gate = new Promise((resolve) => {
    release = resolve;
  });
  const state = {};
  const server = createServer(async (request, response) => {
    try {
      assert.equal(request.url, '/v1/chat/completions');
      assert.equal(request.headers.authorization, 'Bearer pruning-fixture');
      const chunks = [];
      let bytes = 0;
      for await (const chunk of request) {
        bytes += chunk.length;
        assert(bytes <= 128 * 1024);
        chunks.push(chunk);
      }
      const input = JSON.parse(Buffer.concat(chunks).toString());
      const stage = ++count + (reopened ? 8 : 0);
      assert(stage <= (reopened ? 10 : 8));
      assert.equal(input.model, 'fixture-model');
      assert.equal(input.stream, true);
      const tools = input.messages.filter((message) => message.role === 'tool');
      if ([2, 3, 4].includes(stage)) {
        const original = tools.find((message) => message.tool_call_id === 'original-read');
        const placeholder = JSON.parse(original.content);
        assert.equal(placeholder.kind, 'maka.archived_tool_result');
        assert.equal(placeholder.toolName, 'Shell');
        assert.equal(placeholder.reason, 'tool_result_pruned');
        assert.equal(placeholder.previousTransitionId, undefined);
        assert(placeholder.originalBytes > Buffer.byteLength(text));
        assert.match(placeholder.resourceRef, /^archive:[0-9a-f-]{36}$/);
        for (const field of [
          'runtimeEventId',
          'toolCallId',
          'sourceProjectionDigest',
          'bodySha256',
          'rewriteVersion',
        ])
          assert.equal(
            placeholder[field],
            undefined,
            'internal integrity evidence stays out of the model prompt',
          );
        state.ref ??= placeholder.resourceRef;
        assert.equal(placeholder.resourceRef, state.ref);
        assert(original.content.length <= 7500);
        assert(text.startsWith(placeholder.page.content));
        assert(placeholder.page.next, 'first page exposes continuation');
      }
      if (stage === 3) {
        read(bounded(tools.find((message) => message.tool_call_id === 'archive-read')), 0, 5);
      }
      if ([5, 6, 9, 10].includes(stage)) {
        assert(
          JSON.stringify(input.messages).includes(JSON.stringify(summary(state.ref)).slice(1, -1)),
        );
        assert(!tools.some((message) => message.tool_call_id === 'original-read'));
      }
      if ([6, 10].includes(stage))
        read(
          bounded(
            tools.findLast((message) => message.tool_call_id === 'retained-read'),
            state.ref,
          ),
          100,
          5,
        );
      if (stage === 6) {
        const forged = tools.find((message) => message.tool_call_id === 'forged-ref');
        assert.match(forged.content, /invalid|noncanonical/i);
      }
      if (stage === 8) assert.equal(bounded(tools.at(-1)).error, 'not_found');
      const call = (id, name, args) => ({
        index: 0,
        id,
        type: 'function',
        function: { name, arguments: JSON.stringify(args) },
      });
      let calls = [];
      if (stage === 1)
        calls = [
          call('original-read', 'Shell', {
            command:
              process.platform === 'win32'
                ? '[Console]::Write([IO.File]::ReadAllText((Join-Path (Get-Location) evidence.txt)))'
                : 'cat evidence.txt',
          }),
        ];
      if (stage === 2) calls = [call('archive-read', 'Read', { path: state.ref, limit: 5 })];
      if ([5, 9].includes(stage))
        calls = [call('retained-read', 'Read', { path: state.ref, offset: 100, limit: 5 })];
      if (stage === 5) calls.push(call('forged-ref', 'Read', { path: state.ref + '?forged=1' }));
      if (stage === 7) calls = [call('foreign-read', 'Read', { path: state.ref, limit: 5 })];
      if (stage === 4) {
        assert(!input.tools?.length);
        assert.equal(input.messages.at(-1).role, 'user');
        arrive();
        await gate;
      } else {
        assert(input.tools.some((tool) => tool.function.name === 'Read'));
        assert(!input.tools.some((tool) => tool.function.name === 'ArchiveRead'));
      }
      const delta = calls.length
        ? { tool_calls: calls.map((tool, index) => ({ ...tool, index })) }
        : { content: stage === 4 ? summary(state.ref) : 'pruning complete' };
      const frame = (delta, finish_reason = null) =>
        'data: ' +
        JSON.stringify({
          id: 'pruning-' + stage,
          object: 'chat.completion.chunk',
          created: 1,
          model: input.model,
          choices: [{ index: 0, delta, finish_reason }],
        }) +
        '\n\n';
      response.writeHead(200, { 'Content-Type': 'text/event-stream', Connection: 'close' });
      response.end(
        frame(delta) + frame({}, calls.length ? 'tool_calls' : 'stop') + 'data: [DONE]\n\n',
      );
    } catch (error) {
      error.message = `model stage ${count}: ${error.message}`;
      failure = error;
      response.destroy(error);
    }
  });
  server.listen(port, '127.0.0.1');
  await once(server, 'listening');
  return {
    state,
    release,
    baseUrl: 'http://127.0.0.1:' + server.address().port + '/v1',
    check() {
      if (failure) throw failure;
    },
    async waitForSummary() {
      await Promise.race([
        arrived,
        delay(10000, undefined, { ref: false }).then(() => {
          this.check();
          throw new Error('Summary request was not received');
        }),
      ]);
      this.check();
    },
    verify() {
      this.check();
      assert.equal(count, reopened ? 2 : 8);
    },
    async close() {
      release();
      server.closeAllConnections();
      await new Promise((resolve) => server.close(resolve));
    },
  };
}
