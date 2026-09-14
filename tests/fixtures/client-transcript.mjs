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
import { readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { decodeStoredMessage, userFacingText } from '../../packages/core/src/session.ts';
import { watchSession } from './client-subscription.mjs';
import { assertReferencedRow } from './client-message-references.mjs';

const tail = { kind: 'tail', maxBytes: 2 };
const pageInput = (subscription, source = 'durable') => ({
  source,
  direction: 'older',
  throughSequence: subscription.transcriptBootstrap.throughSequence,
  cursor: null,
  anchorSequence: null,
  maxBytes: 97,
});

function bootstrap(subscription) {
  const value = subscription.transcriptBootstrap;
  assert(value);
  assert(value.durable.rawBytes + value.overlay.rawBytes <= 2);
  return value;
}

async function withTail(connection, sessionId, verify) {
  const observer = await watchSession(connection, sessionId, tail);
  try {
    bootstrap(observer.subscription);
    const rows = await observer.subscription.loadTranscript(decodeStoredMessage);
    assert.equal(new Set(rows.map((row) => row.id)).size, rows.length);
    await verify(rows, observer.subscription);
  } finally {
    await observer.close();
  }
}

function assistant(rows, turnId, frames, expected) {
  const row = rows.find((row) => row.type === 'assistant' && row.turnId === turnId);
  assert(row, `Missing assistant for ${turnId}`);
  assert.equal(row.text, expected);
  assert.equal(row.modelId, 'fixture-model');
  const delta = frames.find(
    (frame) => frame.kind === 'subscription.session_delta' && frame.delta.turnId === turnId,
  );
  assert.equal(row.id, delta.delta.messageId, 'transcript and live stream share identity');
  return row;
}

export async function emptyTail(connection, sessionId) {
  await withTail(connection, sessionId, (rows, subscription) => {
    assert.deepEqual(rows, []);
    const value = bootstrap(subscription);
    assert.equal(value.overlayMessageCount, 0);
    for (const page of [value.durable, value.overlay]) {
      assert.equal(page.rawBytes, 0);
      assert.deepEqual(page.fragments, []);
      assert.equal(page.nextCursor, null);
    }
  });
}

export async function finishTranscriptObservers(connection, sessionId, none, live) {
  // A fresh post-terminal bootstrap supplies the exact fence the live observer must reach.
  await withTail(connection, sessionId, async (_rows, subscription) => {
    const throughSequence = bootstrap(subscription).throughSequence;
    await live.waitFor(
      (frame) =>
        frame.kind === 'subscription.transcript_advanced' &&
        frame.throughSequence >= throughSequence,
    );
  });
  const advances = live.frames.filter((frame) => frame.kind === 'subscription.transcript_advanced');
  assert(advances.length >= 3, 'committed turns announce transcript progress');
  for (let index = 1; index < advances.length; index++) {
    assert(advances[index].throughSequence > advances[index - 1].throughSequence);
  }
  await assert.rejects(
    connection.request(
      'session.transcript.page',
      {
        subscriptionId: none.subscription.subscriptionId,
        source: 'durable',
        direction: 'older',
        throughSequence: null,
        cursor: null,
        anchorSequence: null,
        maxBytes: 97,
      },
      3000,
    ),
    (error) => error.code === 'operation_unavailable',
    'transcript:none must not grant raw durable paging',
  );
  assert.equal(
    none.frames.some((frame) => frame.kind === 'subscription.transcript_advanced'),
    false,
    'transcript:none observers receive no transcript advancement',
  );
  await live.close();
}

export async function completedTail(connection, sessionId, frames, startedAt) {
  await withTail(connection, sessionId, async (rows, subscription) => {
    assert.deepEqual(
      rows.map((row) => row.type),
      ['user', 'assistant', 'token_usage', 'turn_state'],
    );
    assistant(rows, 'first-turn', frames, 'completed😀 fixture');
    assert.equal(rows[0].text, 'first question');
    assert.equal(userFacingText(rows[0]), 'First visible question 😀 @source.rs');
    assertReferencedRow(rows[0], connection.rootId);
    assert.equal(rows[2].input, 7);
    assert.equal(rows[2].output, 3);
    assert.equal(rows[3].status, 'completed');
    for (const row of rows) assert(row.ts >= startedAt && row.ts <= Date.now());
    const initial = bootstrap(subscription).durable;
    assert(initial.nextCursor, 'two-byte bootstrap must require fragmented continuation');
    assert(
      initial.fragments.every((fragment) => /^sha256:[a-f0-9]{64}$/.test(fragment.payloadDigest)),
    );
    const older = await subscription.decodeTranscriptPage(initial, decodeStoredMessage);
    assert.deepEqual(
      older.messages.map((entry) => entry.message),
      rows,
      'whole-turn older range',
    );
    const newer = await subscription.loadTranscriptPage({
      ...pageInput(subscription),
      direction: 'newer',
    });
    const decoded = await subscription.decodeTranscriptPage(newer, decodeStoredMessage);
    assert.deepEqual(
      decoded.messages.map((entry) => entry.message),
      rows,
      'whole-turn newer range',
    );
    for (const [direction, anchorSequence] of [
      ['older', older.messages[0].identity],
      ['newer', older.messages.at(-1).identity],
    ]) {
      const outside = await subscription.loadTranscriptPage({
        ...pageInput(subscription),
        direction,
        anchorSequence,
      });
      assert.deepEqual(outside.fragments, [], 'anchors exclude their own identity');
    }
  });
}

export async function activeTail(connection, observer, connectSibling) {
  const { subscription } = observer;
  assert(bootstrap(subscription).overlayMessageCount > 0);
  const sibling = await connectSibling();
  try {
    await assert.rejects(
      sibling.request(
        'session.transcript.overlay.release',
        { subscriptionId: subscription.subscriptionId },
        3000,
      ),
      (error) => error.code === 'not_found',
    );
  } finally {
    await sibling.close();
  }
  // Load after the suffix was committed: the overlay must retain the opening prefix.
  const rows = await subscription.loadTranscript(decodeStoredMessage);
  const row = rows.find((row) => row.type === 'assistant' && row.turnId === 'cancelled-turn');
  assert.equal(row.text, 'incomplete😀 fixture');
  assert.equal(row.id, subscription.activeAssistantStreams[0].messageId);
  assert.equal(
    row.id,
    observer.frames.find((frame) => frame.kind === 'subscription.session_delta').delta.messageId,
  );
  // Actual client loadTranscript automatically released the nonempty frozen overlay.
  await assert.rejects(subscription.loadTranscriptPage(pageInput(subscription, 'overlay')));
  await subscription.loadTranscriptPage(pageInput(subscription));
  assert.deepEqual(
    await connection.request(
      'session.transcript.overlay.release',
      { subscriptionId: subscription.subscriptionId },
      3000,
    ),
    { subscriptionId: subscription.subscriptionId },
    'release is idempotent and retains durable access',
  );
  assert.equal(
    observer.frames.some((frame) => frame.kind === 'subscription.transcript_advanced'),
    false,
    'a text-only suffix must not announce nonexistent durable rows',
  );
}

export async function cancelledTail(connection, sessionId, frames) {
  await withTail(connection, sessionId, (rows, subscription) => {
    assistant(rows, 'cancelled-turn', frames, 'incomplete😀 fixture 🐈 suffix');
    const turn = rows.filter((row) => row.turnId === 'cancelled-turn');
    assert.deepEqual(turn[0].inlineReferences, [], 'explicit empty reference marker survives');
    assert.deepEqual(
      turn.map((row) => row.type),
      ['user', 'assistant', 'turn_state'],
    );
    assert.equal(turn.at(-1).status, 'aborted');
    assert.equal(turn.at(-1).abortSource, 'runtime_cancellation');
    assert.equal(bootstrap(subscription).overlayMessageCount, 0);
  });
}

export async function persistentTail(connection, workspace, reopened) {
  await withTail(connection, 'rust-interop-session', async (rows) => {
    const path = join(workspace, 'transcript-before-restart.json');
    if (reopened) {
      assert.deepEqual(
        rows,
        JSON.parse(await readFile(path, 'utf8')),
        'IDs, timestamps and payloads survive host restart',
      );
    } else {
      await writeFile(path, JSON.stringify(rows));
    }
    console.log(
      JSON.stringify({
        check: 'original-client-transcript',
        reopened: Boolean(reopened),
        messages: rows.length,
        result: 'passed',
      }),
    );
  });
}
