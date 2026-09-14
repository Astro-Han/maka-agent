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

export async function verifyModelStdin({
  pty,
  sessionId,
  other,
  reference,
  hidden,
  request,
  model,
  turn,
  result,
}) {
  const write = (ref, extra = {}) => ({
    name: 'WriteStdin',
    args: { ref, input: 'wrong\r', ...extra },
  });
  const rejected = (message) => (input) => {
    assert.equal(input.messages.at(-1).content, message);
    return { answer: 'input rejected without effect' };
  };
  model.extend([
    write(hidden),
    rejected('Runtime background task not found in this session'),
    write(reference),
    rejected('Runtime background task not found in this session'),
  ]);
  await turn(sessionId, 'hidden-input');
  await turn(other, 'foreign-input');
  if (!pty) {
    model.extend([write(reference), rejected('WriteStdin requires a PTY background task ref')]);
    await turn(sessionId, 'reject-pipe-input');
    return;
  }
  const identity = { sessionId, ref: reference, controllerId: 'model-launched-pty' };
  await request('runtime.resource.controller.acquire', identity);
  model.extend([
    write(reference, { size: { cols: 110, rows: 31 } }),
    rejected('This PTY is controlled by a connected Client'),
  ]);
  await turn(sessionId, 'client-controls-input');
  const query = () => request('runtime.resource.query', { kind: 'get', sessionId, ref: reference });
  const before = (await query()).resource.result.output;
  assert.equal(before.cols, 80);
  assert.equal(before.rows, 24);
  // Client and model contribute to the same line, preserving UTF-8 across ownership.
  await request('runtime.resource.controller.control', {
    ...identity,
    sequence: 1,
    control: { kind: 'input', input: 'continue' },
  });
  await request('runtime.resource.controller.release', identity);
  const size = { cols: 100, rows: 30 };
  model.extend([
    { name: 'WriteStdin', args: { ref: reference, actions: [], size } },
    (input) => {
      const value = result(input);
      assert.deepEqual(value.operation, {
        kind: 'pty_control',
        failed: false,
        resize: { ...size, applied: true, changed: true },
      });
      assert.equal(value.output.cols, size.cols);
      assert.equal(value.output.rows, size.rows);
      return { name: 'WriteStdin', args: { ref: reference, actions: null, size } };
    },
    (input) => {
      assert.deepEqual(result(input).operation, {
        kind: 'pty_control',
        failed: false,
        resize: { ...size, applied: true, changed: false },
      });
      return {
        name: 'WriteStdin',
        args: {
          ref: reference,
          size: { cols: 0, rows: null },
          actions: [
            { type: 'text', text: '😀', key: '', event: null, x: 0, modifiers: [] },
            { type: 'key', key: 'enter', text: '', direction: 0, modifiers: null },
          ],
        },
      };
    },
    (input) => {
      assert.deepEqual(result(input).operation, {
        kind: 'pty_control',
        failed: false,
        input: { bytes: 5, queued: true },
      });
      return { answer: 'input accepted; Read observes later output' };
    },
  ]);
  await turn(sessionId, 'model-input');
}

export async function verifyTerminalStdin({ sessionId, reference, snapshot, model, turn, result }) {
  const size = { cols: 120, rows: 40 };
  model.extend([
    { name: 'WriteStdin', args: { ref: reference, input: 'never\r', size } },
    (input) => {
      assert.deepEqual(result(input), {
        ...snapshot,
        operation: {
          kind: 'pty_control',
          failed: false,
          input: { bytes: 6, queued: false },
          resize: { ...size, applied: false, changed: false },
        },
      });
      return { answer: 'terminal input is a no-op' };
    },
  ]);
  await turn(sessionId, 'terminal-input');
}
