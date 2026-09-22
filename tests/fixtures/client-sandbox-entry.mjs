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
import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { createMessageSession } from './client-message-fixture.mjs';
import { connect } from 'node:net';
import { once } from 'node:events';
import { parseArgs } from 'node:util';
import { connectRuntimeHostMessageTransport } from '../../packages/runtime-host/src/client/connection.ts';
import { FramedTransport } from '../../packages/runtime-host/src/transport/framed-transport.ts';
import {
  INTERACTIVE_RUNTIME_HOST_COMPOSITION_ID,
  RUNTIME_HOST_PROTOCOL_VERSION,
} from '../../packages/runtime-host/src/protocol/index.ts';

const sessionId = 'sandbox-default';
async function verifySandbox(connection, workspace, reopened) {
  const request = (op, input) => connection.request(op, input, 15000);
  const saved = join(workspace, 'sandbox-result.json');
  const get = async (ref) =>
    (await request('runtime.resource.query', { kind: 'get', sessionId, ref })).resource.result;
  if (reopened) {
    const result = JSON.parse(await readFile(saved, 'utf8'));
    assert.deepEqual(await get(result.ref), result);
    assert.equal(await readFile(join(workspace, 'host-effect'), 'utf8'), 'once');
    return;
  }
  await createMessageSession(connection, workspace, sessionId, 'http://127.0.0.1:1/v1');
  await mkdir(join(workspace, '.agents'), { recursive: true });
  await writeFile(join(workspace, '.agents', 'keep'), 'unchanged');
  const quote = (s) => "'" + s.replaceAll("'", "''") + "'";
  const command = `
    $ErrorActionPreference='Stop'
    [IO.File]::AppendAllText((Join-Path $pwd 'host-effect'), 'once')
    $temporary = [IO.Path]::GetTempFileName()
    [IO.File]::WriteAllText($temporary, 'temporary')
    [IO.File]::Delete($temporary)
    try { [IO.File]::WriteAllText((Join-Path $pwd '.agents/keep'), 'escaped'); exit 81 }
    catch { if (($_.Exception.GetBaseException().HResult -band 65535) -ne 5) { exit 82 } }
    try { [IO.File]::WriteAllText((Join-Path $pwd '.git/config'), 'escaped'); exit 83 }
    catch { if (($_.Exception.GetBaseException().HResult -band 65535) -ne 5) { exit 84 } }
    try { [IO.Directory]::GetFiles(${quote(process.env.MAKA_TEST_STATE_ROOT)}); exit 85 }
    catch { if (($_.Exception.GetBaseException().HResult -band 65535) -ne 5) { exit 86 } }
    [Console]::Write('default-policy-settled')
  `;
  const input = { sessionId, launchId: 'default-policy', command };
  const started = await request('runtime.resource.start', input);
  const ref = started.resource.ref;
  let result;
  const deadline = Date.now() + 20000;
  for (;;) {
    result = await get(ref);
    if (!['starting', 'running'].includes(result.status)) break;
    assert(Date.now() < deadline, JSON.stringify(result));
    await delay(30);
  }
  assert.equal(result.status, 'completed', JSON.stringify(result));
  assert.equal(result.exitCode, 0, JSON.stringify(result));
  assert.equal(result.output.stdout, 'default-policy-settled');
  assert.equal(await readFile(join(workspace, 'host-effect'), 'utf8'), 'once');
  assert.equal(await readFile(join(workspace, '.agents', 'keep'), 'utf8'), 'unchanged');
  await writeFile(saved, JSON.stringify(result));
}

const { values } = parseArgs({
  options: {
    socket: { type: 'string' },
    'root-id': { type: 'string' },
    'sandbox-workspace': { type: 'string' },
    reopened: { type: 'boolean' },
  },
});
const deadline = setTimeout(() => {
  throw new Error('Sandbox acceptance timed out');
}, 40000);
const socket = connect(values.socket);
const transport = new FramedTransport(socket);
let connection;
try {
  await once(socket, 'connect');
  ({ connection } = await connectRuntimeHostMessageTransport({
    expectedRootId: values['root-id'],
    compositionId: INTERACTIVE_RUNTIME_HOST_COMPOSITION_ID,
    protocol: { min: RUNTIME_HOST_PROTOCOL_VERSION, max: RUNTIME_HOST_PROTOCOL_VERSION },
    handshakeTimeoutMs: 3000,
    transport,
  }));
  await verifySandbox(connection, values['sandbox-workspace'], values.reopened);
} finally {
  if (connection) await connection.close();
  transport.abort();
  clearTimeout(deadline);
}
