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
import {
  createRuntimeHostSessionProjectionSeed,
  RuntimeHostSessionProjector,
} from '../../packages/runtime-host/src/adapter/session-projector.ts';

export async function watchSession(
  connection,
  sessionId,
  transcript = { kind: 'none' },
  ready = true,
) {
  const subscription = await connection.openSessionSubscription(
    {
      sessionId,
      transcript,
    },
    3000,
  );
  const frames = [];
  // Reconnected streams replay their committed prefix from offset zero.
  const projector = new RuntimeHostSessionProjector(
    subscription.snapshot,
    createRuntimeHostSessionProjectionSeed([], subscription.snapshot),
    Date.now,
    subscription.activeAssistantStreams,
  );
  let failure;
  const task = (async () => {
    try {
      for await (const frame of subscription) {
        projector?.accept(frame);
        frames.push(frame);
      }
    } catch (error) {
      failure = error;
    }
  })();
  if (ready) await subscription.ready();
  return {
    subscription,
    frames,
    async waitFor(predicate) {
      // Model-driven transitions include 3.1 s of WS backoff before HTTP.
      // This observes the exact committed result, not a 3 s model latency SLO.
      const deadline = Date.now() + 10000;
      while (Date.now() < deadline) {
        if (failure) throw failure;
        const frame = frames.find(predicate);
        if (frame) return frame;
        await delay(5);
      }
      throw new Error('Committed subscription frame was not delivered');
    },
    async terminal(turn) {
      const frame = await this.waitFor(
        (frame) =>
          frame.kind === 'subscription.session_projection' &&
          frame.snapshot.rootTurn?.turnId === turn.turnId &&
          ['completed', 'failed', 'cancelled'].includes(frame.snapshot.rootTurn.status),
      );
      assert.deepEqual(frame.snapshot.rootTurn, turn);
    },
    async close() {
      await subscription.close();
      await task;
      if (failure) throw failure;
    },
  };
}

export function assertText(frames, turnId, expected) {
  const deltas = frames
    .filter((frame) => frame.kind === 'subscription.session_delta' && frame.delta.turnId === turnId)
    .map((frame) => frame.delta);
  assert(deltas.length > 1, 'text must include a stream completion');
  let text = '';
  for (const delta of deltas) {
    assert.equal(delta.kind, 'text');
    assert.equal(delta.messageId, deltas[0].messageId);
    assert.equal(delta.startOffset, text.length, 'offset counts UTF-16 code units');
    text += delta.text;
  }
  assert.equal(text, expected);
  assert.equal(deltas.at(-1).complete, true);
}
