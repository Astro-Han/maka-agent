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
import { setTimeout as delay } from 'node:timers/promises';
import { watchSession } from './client-subscription.mjs';

export async function watchResourceStream(connection, other, sessionId, ref) {
  const observer = await watchSession(connection, sessionId);
  const frames = [];
  const stop = observer.subscription.subscribePtyData((frame) => frames.push(frame));
  const subscriptionId = observer.subscription.subscriptionId;
  const interest = (refs) =>
    connection.request('subscription.pty_interest.set', { subscriptionId, refs }, 3000);
  try {
    await assert.rejects(
      other.request('subscription.pty_interest.set', { subscriptionId, refs: [ref] }, 3000),
      { code: 'not_found' },
    );
    assert.deepEqual(await interest([ref]), { subscriptionId });
  } catch (error) {
    stop();
    await observer.close();
    throw error;
  }
  return {
    frames,
    async expect(text, after = 0) {
      const deadline = Date.now() + 5000;
      while (Date.now() < deadline) {
        const output = frames
          .filter((frame) => frame.ptySequence > after && !frame.reset)
          .map((frame) => frame.data)
          .join('');
        if (output.includes(text)) {
          for (const frame of frames) {
            assert.equal(frame.sessionId, sessionId);
            assert.equal(frame.ref, ref);
            assert.equal(Object.hasOwn(frame, 'sequence'), false);
            assert(Buffer.byteLength(JSON.stringify(frame)) <= 65535);
          }
          assert(
            observer.frames.every(
              (frame) => frame.kind !== 'subscription.runtime_resource_pty_data',
            ),
            'PTY data must not enter the sequenced Session iterator',
          );
          return;
        }
        await delay(5);
      }
      throw new Error('Missing raw PTY output ' + text);
    },
    async pause() {
      await interest([]);
      return frames.length;
    },
    async resume() {
      await interest([ref]);
    },
    async close() {
      await interest([]);
      stop();
      await observer.close();
    },
  };
}
