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
import { existsSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { createInterface } from 'node:readline';
import { ClientCapabilityChannel } from '../../packages/runtime-host/src/client/client-capability-channel.ts';
import { decodeClientCapabilityHostFrame } from '../../packages/runtime-host/src/protocol/client-capability.ts';

// JSON lines are private test plumbing, not a Runtime Host operation or transport.
export async function verifyCapabilityService(workspace) {
  const emit = (message) => process.stdout.write(`${JSON.stringify(message)}\n`);
  const deadline = setTimeout(() => {
    console.error('Client capability service probe timed out');
    process.exit(1);
  }, 10000);
  const lines = createInterface({ input: process.stdin });
  const replies = new Map();
  let nextId = 0;
  const request = (kind, input) =>
    new Promise((resolve) => {
      const id = ++nextId;
      replies.set(id, resolve);
      emit({ kind, id, input });
    });
  let closeCount = 0;
  let releaseCount = 0;
  let invocationReleaseCount = 0;
  let resultChunks = 0;
  let executed = false;
  const toolExecutions = new Set();
  const registrationIds = new Map();
  let resolveClosed;
  const closed = new Promise((resolve) => {
    resolveClosed = resolve;
  });
  const effect = join(workspace, 'provider-effect.txt');
  const channel = new ClientCapabilityChannel({
    write: async (frame) => {
      if (frame.kind === 'client.capability.result_chunk') resultChunks += 1;
      emit({ kind: 'frame', frame });
    },
    replace: (input) => request('replace', input),
    unregister: (input) => request('unregister', input),
    onFailure: (error) => {
      console.error(error);
      process.exit(1);
    },
  });
  const provider = (generation) => ({
    offers: () =>
      ['none', 'cwd'].map((access) => ({
        offerId: access,
        version: '1',
        affinity: 'session',
        hostPathAccess: access,
        label: `Tool ${access}`,
        tools: [
          { serverId: `test-server-${access}`, name: 'write', inputSchema: { type: 'object' } },
        ],
      })),
    services: () => [{ serviceId: 'test_effect', version: '1' }],
    async call(frame, { accept, signal }) {
      const access = frame.offerId;
      assert(['none', 'cwd'].includes(access));
      assert.equal(frame.registrationId, registrationIds.get(generation));
      assert.equal(frame.serverId, `test-server-${access}`);
      assert.equal(frame.toolName, 'write');
      assert.equal(frame.sessionId, 'interop-session');
      assert.equal(frame.turnId, 'interop-turn');
      assert.equal(frame.toolCallId, `interop-call-${access}`);
      assert.deepEqual(frame.arguments, { text: access });
      if (access === 'none') assert.equal(Object.hasOwn(frame, 'cwd'), false);
      else assert.equal(frame.cwd, workspace);
      await accept({ kind: 'none' });
      assert.equal(signal.aborted, false);
      writeFileSync(join(workspace, `tool-${access}.txt`), access, { flag: 'wx' });
      toolExecutions.add(access);
      return {
        content: [{ type: 'text', text: access }],
        structuredContent: { generation, registrationId: frame.registrationId },
      };
    },
    async callService(frame, { accept, signal }) {
      assert.equal(frame.serviceId, 'test_effect');
      assert.equal(frame.version, '1');
      assert.equal(frame.method, 'write');
      assert.deepEqual(frame.input, { text: 'admitted effect' });
      await accept({ kind: 'none' });
      assert.equal(signal.aborted, false);
      writeFileSync(effect, frame.input.text, { flag: 'wx' });
      executed = true;
      return { text: 'é'.repeat(40000), effect: frame.input.text };
    },
    close() {
      closeCount += 1;
      if (closeCount === 2) resolveClosed();
    },
  });
  const register = async (generation, kind) => {
    const result = await channel.replace(provider(generation), 3000);
    registrationIds.set(generation, result.registrationId);
    emit({ kind });
  };
  try {
    // Read concurrently: replace/unregister await actual Rust registry replies.
    const registered = register(1, 'ready');
    for await (const line of lines) {
      const message = JSON.parse(line);
      switch (message.kind) {
        case 'reply': {
          const resolve = replies.get(message.id);
          assert(resolve, 'unmatched test bridge response');
          replies.delete(message.id);
          resolve(message.result);
          break;
        }
        case 'host': {
          const frame = decodeClientCapabilityHostFrame(message.frame);
          if (frame.kind === 'client.capability.registration_release') releaseCount += 1;
          if (frame.kind === 'client.capability.release') invocationReleaseCount += 1;
          channel.accept(frame);
          break;
        }
        case 'checkpoint':
          assert.equal(executed, false);
          assert.equal(existsSync(effect), false);
          emit({ kind: 'checkpoint' });
          break;
        case 'unregister':
          void channel.unregister(3000).then(() => emit({ kind: 'unregistered' }));
          break;
        case 'replace':
          void register(2, 'replaced');
          break;
        case 'tool-checkpoint':
          assert.equal(toolExecutions.has(message.access), false);
          assert.equal(existsSync(join(workspace, `tool-${message.access}.txt`)), false);
          if (message.access === 'none') assert.equal(closeCount, 0);
          emit({ kind: 'tool-checkpoint' });
          break;
        case 'finish':
          await registered;
          await closed;
          assert.equal(executed, true);
          assert.deepEqual([...toolExecutions], ['none', 'cwd']);
          assert.equal(releaseCount, 2);
          assert.equal(invocationReleaseCount, 3);
          assert(resultChunks > 1, 'original client must send multiple result chunks');
          assert.equal(closeCount, 2);
          channel.close(new Error('Test host shut down'));
          channel.close(new Error('Repeated shutdown'));
          assert.equal(closeCount, 2);
          assert.equal(replies.size, 0);
          emit({ kind: 'done', resultChunks });
          return;
        default:
          assert.fail(`Unknown bridge message: ${message.kind}`);
      }
    }
    assert.fail('Test host closed before completing the service lifecycle');
  } finally {
    channel.close(new Error('Test bridge closed'));
    lines.close();
    process.stdin.pause();
    clearTimeout(deadline);
  }
}
