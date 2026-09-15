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
import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';
import { parseArgs } from 'node:util';
import { connectRuntimeHostWslEnvironment } from '../../packages/runtime-host/src/client/wsl-environment.js';
import { RUNTIME_HOST_COMPATIBILITY_EPOCH } from '../../packages/runtime-host/src/protocol/index.js';

const { values } = parseArgs({
  options: {
    bridge: { type: 'string' },
    'root-id': { type: 'string' },
    'host-epoch': { type: 'string' },
    'project-roots': { type: 'string' },
  },
});
const children = [];
const args = ['host', 'connect', '--framed', '--root-id', values['root-id']];
function launch(input = args) {
  const child = spawn(values.bridge, input, { stdio: 'pipe', timeout: 8_000 });
  child.stderr.pipe(process.stderr, { end: false });
  const exited = new Promise((resolve) => {
    child.once('exit', (code, signal) => resolve({ code, signal }));
    child.once('error', (error) => resolve({ error }));
  });
  children.push({ child, exited });
  return { child, exited };
}
const hello = {
  kind: 'hello',
  clientInstanceId: 'native-bridge-fixture',
  protocolMin: 0,
  protocolMax: 0,
  compatibilityEpoch: RUNTIME_HOST_COMPATIBILITY_EPOCH,
  compositionId: 'maka.interactive',
};
const line = (frame) => JSON.stringify(frame) + '\n';
const request = (operation, input, id = 'bridge-request') => ({
  requestId: id,
  operation,
  input,
});

try {
  // Exercise the real WSL client and its unchanged launch arguments/transport.
  // Only the OS process factory is replaced for ordinary three-platform tests.
  const connection = await connectRuntimeHostWslEnvironment(
    {
      distribution: 'Maka-Test',
      operator: { kind: 'native', platform: 'posix', executablePath: '/maka' },
      rootId: values['root-id'],
      clientInstanceId: 'original-wsl-client',
    },
    {
      wslExecutable: 'fixture-wsl',
      processFactory: (executable, input) => {
        assert.equal(executable, 'fixture-wsl');
        assert.deepEqual(input.slice(0, 4), ['--distribution', 'Maka-Test', '--exec', '/maka']);
        assert(input.includes('--repair-root-after-remount'));
        return launch(input.slice(4)).child;
      },
    },
  );
  try {
    assert.equal(connection.rootId, values['root-id']);
    assert.equal(connection.hostEpoch, values['host-epoch']);
    assert.equal(
      (await connection.request('host.diagnostics.query', {})).hostEpoch,
      values['host-epoch'],
    );
    if (values['project-roots'] !== undefined) {
      assert.deepEqual(
        await connection.request('project.catalog.query', { kind: 'directory_roots' }),
        { kind: 'directory_roots', roots: JSON.parse(values['project-roots']) },
      );
    }
  } finally {
    await connection.close();
  }

  if (process.platform !== 'win32') {
    const { child, exited } = launch();
    // One write pipelines the request beyond hello; EOF must not discard either
    // the input prefetch or the Host's final response.
    child.stdin.end(line(hello) + line(request('host.diagnostics.query', {})));
    let response;
    for await (const text of createInterface({ input: child.stdout })) {
      const frame = JSON.parse(text);
      if (frame.requestId === 'bridge-request') response = frame;
    }
    assert.deepEqual(await exited, { code: 0, signal: null });
    assert.equal(response?.ok, true, JSON.stringify(response));
    assert.equal(response.result.hostEpoch, values['host-epoch']);
  }

  const { child, exited } = launch();
  child.stdin.write(line(hello));
  let prepared;
  for await (const text of createInterface({ input: child.stdout })) {
    const frame = JSON.parse(text);
    if (frame.kind === 'accepted') {
      assert.equal(frame.rootId, values['root-id']);
      assert.equal(frame.hostEpoch, values['host-epoch']);
      child.stdin.write(
        line(
          request('host.upgrade.prepare', {
            expectedHostEpoch: values['host-epoch'],
            allowInterruptActiveTasks: false,
            allowCooperativeHandoff: true,
          }),
        ),
      );
    }
    if (frame.requestId === 'bridge-request') prepared = frame;
  }
  // stdin was never ended: a process-lifetime stdio read must not prevent exit.
  assert.deepEqual(await exited, { code: 0, signal: null });
  assert.equal(prepared?.ok, true, JSON.stringify(prepared));
  assert.equal(prepared.result.kind, 'prepared', JSON.stringify(prepared));
} finally {
  for (const { child, exited } of children) {
    if (child.exitCode === null && child.signalCode === null) child.kill();
    await exited;
  }
}
