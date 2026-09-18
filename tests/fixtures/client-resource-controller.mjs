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
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { watchResourceStream } from './client-resource-stream.mjs';

export async function verifyResourceController(
  connection,
  openConnection,
  workspace,
  sessionId,
  ref,
) {
  const request = (peer, operation, input) =>
    peer.request('runtime.resource.controller.' + operation, input, 5000);
  const identity = { sessionId, ref, controllerId: 'terminal-owner' };
  const other = await openConnection();
  let stream;
  let second;
  const append = (text) =>
    process.platform === 'win32'
      ? "[IO.File]::AppendAllText('controller-effect', '" + text + "')\r"
      : "printf '" + text + "' >> controller-effect\r";
  const effect = async (expected) => {
    const deadline = Date.now() + 5000;
    for (;;) {
      let value;
      try {
        value = await readFile(join(workspace, 'controller-effect'), 'utf8');
      } catch (error) {
        if (error.code !== 'ENOENT') throw error;
      }
      if (value === expected) return;
      assert(Date.now() < deadline, 'Expected application effect ' + expected + ', got ' + value);
      await delay(10);
    }
  };
  const conflict = (promise) => assert.rejects(promise, { code: 'operation_conflict' });
  try {
    stream = await watchResourceStream(connection, other, sessionId, ref);
    const acquired = await request(connection, 'acquire', identity);
    assert.equal(acquired.controllerId, identity.controllerId);
    assert.equal(acquired.nextSequence, 1);
    assert.equal(acquired.pty.sessionId, sessionId);
    assert.equal(acquired.pty.ref, ref);
    assert.equal(typeof acquired.pty.buffer, 'string');
    assert.deepEqual((await request(connection, 'acquire', identity)).nextSequence, 1);
    await conflict(request(other, 'acquire', identity));
    await conflict(request(other, 'release', identity));
    await conflict(
      request(other, 'control', {
        ...identity,
        sequence: 1,
        control: { kind: 'input', input: append('X') },
      }),
    );
    second = (
      await connection.request(
        'runtime.resource.start',
        { sessionId, launchId: 'controller-second' },
        5000,
      )
    ).resource;
    await conflict(request(connection, 'acquire', { ...identity, ref: second.ref }));
    // A distinct controller and PTY can progress independently.
    const independent = { ...identity, ref: second.ref, controllerId: 'independent' };
    assert.equal((await request(other, 'acquire', independent)).nextSequence, 1);
    await request(other, 'control', {
      ...independent,
      sequence: 1,
      control: { kind: 'resize', cols: 75, rows: 22 },
    });
    assert.deepEqual(await request(other, 'release', independent), {
      controllerId: 'independent',
      released: true,
    });
    const first = {
      ...identity,
      sequence: 1,
      control: { kind: 'input_and_resize', input: append('一'), cols: 91, rows: 31 },
    };
    const accepted = await request(connection, 'control', first);
    assert.equal(accepted.sequence, 1);
    assert.deepEqual(accepted, { controllerId: identity.controllerId, sequence: 1 });
    const resized = await request(connection, 'acquire', identity);
    assert.deepEqual(resized.pty.size, { cols: 91, rows: 31 });
    await effect('一');
    await stream.expect('一', acquired.pty.sequence);
    const pausedFrames = await stream.pause();
    assert.deepEqual(await request(connection, 'control', first), accepted);
    await conflict(
      request(connection, 'control', { ...first, control: { kind: 'input', input: append('X') } }),
    );
    await conflict(request(connection, 'control', { ...first, sequence: 3 }));
    assert.equal((await request(connection, 'acquire', identity)).nextSequence, 2);
    const next = { ...identity, sequence: 2, control: { kind: 'input', input: append('二') } };
    const duplicates = await Promise.all([
      request(connection, 'control', next),
      request(connection, 'control', next),
    ]);
    assert.deepEqual(
      duplicates[0],
      duplicates[1],
      'concurrent retry returns the exact accepted cut',
    );
    await effect('一二');
    assert.equal(stream.frames.length, pausedFrames, 'removed interest must stop raw delivery');
    await stream.resume();
    await conflict(request(connection, 'control', first));
    assert.deepEqual(await request(connection, 'release', identity), {
      controllerId: identity.controllerId,
      released: true,
    });
    assert.deepEqual(await request(connection, 'release', identity), {
      controllerId: identity.controllerId,
      released: false,
    });
    assert.equal((await request(other, 'acquire', identity)).nextSequence, 1);
    await other.close();
    const deadline = Date.now() + 5000;
    for (;;) {
      try {
        assert.equal((await request(connection, 'acquire', identity)).nextSequence, 1);
        break;
      } catch (error) {
        if (error.code !== 'operation_conflict') throw error;
        assert(Date.now() < deadline, 'Disconnected controller retained ownership');
        await delay(10);
      }
    }
    const reconnected = {
      ...identity,
      sequence: 1,
      control: { kind: 'input', input: append('三') },
    };
    await request(connection, 'control', reconnected);
    await effect('一二三');
    await stream.expect('三');
    await request(connection, 'release', identity);
    assert.equal((await request(connection, 'acquire', identity)).nextSequence, 1);
    await request(connection, 'control', {
      ...reconnected,
      control: { kind: 'input', input: append('四') },
    });
    await effect('一二三四');
    await request(connection, 'release', identity);
    assert.deepEqual(
      await request(connection, 'release', { sessionId: 'missing', ref, controllerId: 'absent' }),
      { controllerId: 'absent', released: false },
    );
    if (process.platform !== 'win32') {
      const blocked = { ...identity, ref: second.ref, controllerId: 'blocked-paste' };
      await request(connection, 'acquire', blocked);
      await request(connection, 'control', {
        ...blocked,
        sequence: 1,
        control: {
          kind: 'input',
          input: 'stty raw -echo; : >controller-blocked-ready; sleep 60\r',
        },
      });
      const deadline = Date.now() + 5000;
      for (;;) {
        try {
          await readFile(join(workspace, 'controller-blocked-ready'));
          break;
        } catch (error) {
          if (error.code !== 'ENOENT') throw error;
          assert(Date.now() < deadline, 'PTY did not enter raw non-reading state');
          await delay(10);
        }
      }
      let settled = false;
      const input = {
        ...blocked,
        sequence: 2,
        control: { kind: 'input_and_resize', input: 'x'.repeat(32 * 1024), cols: 79, rows: 23 },
      };
      const writing = request(connection, 'control', input).then(
        () => {
          settled = true;
          return null;
        },
        (error) => {
          settled = true;
          return error;
        },
      );
      for (;;) {
        const current = await connection.request(
          'runtime.resource.query',
          { kind: 'get', sessionId, ref: second.ref },
          3000,
        );
        if (
          current.resource.result.output.cols === 79 &&
          current.resource.result.output.rows === 23
        )
          break;
        assert(Date.now() < deadline, 'Combined input did not commit its resize');
        await delay(10);
      }
      await request(connection, 'acquire', identity);
      await request(connection, 'control', {
        ...identity,
        sequence: 1,
        control: { kind: 'resize', cols: 83, rows: 25 },
      });
      assert.equal(settled, false, 'non-reading PTY must backpressure this input');
      const stopped = await connection.request(
        'runtime.resource.stop',
        { sessionId, ref: second.ref },
        5000,
      );
      assert.deepEqual(stopped, {});
      const stoppedState = await connection.request(
        'runtime.resource.query',
        { kind: 'get', sessionId, ref: second.ref },
        5000,
      );
      assert.equal(stoppedState.resource.result.status, 'cancelled');
      assert.equal((await writing)?.code, 'operation_conflict');
      await conflict(request(connection, 'control', input));
      assert.equal(
        (await connection.status(3000)).state,
        'ready',
        'known partial input must not drain Host',
      );
      await request(connection, 'control', {
        ...identity,
        sequence: 2,
        control: { kind: 'resize', cols: 84, rows: 26 },
      });
      await request(connection, 'release', identity);
    }
  } finally {
    await stream?.close();
    await other.close();
    await request(connection, 'release', identity).catch(() => {});
    if (second)
      await connection.request('runtime.resource.stop', { sessionId, ref: second.ref }, 5000);
  }
}
