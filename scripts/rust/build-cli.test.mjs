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
import childProcess from 'node:child_process';
import { EventEmitter } from 'node:events';
import { syncBuiltinESMExports } from 'node:module';
import { join } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { buildCli, rustTarget } from './build-cli.mjs';

test('release builders and Desktop consumers agree on the target and artifact path', async (t) => {
  const root = fileURLToPath(new URL('../../', import.meta.url));
  const previous = process.env.CARGO_TARGET_DIR;
  process.env.CARGO_TARGET_DIR = 'target/isolated release';
  let invocation;
  let exitCode = 0;
  t.mock.method(childProcess, 'spawn', (command, args, options) => {
    invocation = { command, args, options };
    const child = new EventEmitter();
    queueMicrotask(() => child.emit('exit', exitCode, null));
    return child;
  });
  syncBuiltinESMExports();
  t.after(() => {
    t.mock.restoreAll();
    syncBuiltinESMExports();
    if (previous === undefined) delete process.env.CARGO_TARGET_DIR;
    else process.env.CARGO_TARGET_DIR = previous;
  });
  for (const target of [
    undefined,
    'x86_64-unknown-linux-gnu',
    'aarch64-unknown-linux-gnu',
    'aarch64-apple-darwin',
    'x86_64-pc-windows-msvc',
  ]) {
    const triple = target ?? rustTarget();
    const linux = triple.includes('-linux-');
    const directory = join(root, process.env.CARGO_TARGET_DIR);
    const output = await buildCli({ release: true, target });
    assert.equal(invocation.command, 'cargo');
    assert.equal(invocation.options.cwd, root);
    assert.deepEqual(invocation.args, [
      linux ? 'zigbuild' : 'build',
      '--locked',
      '-p',
      'maka-cli',
      '--target-dir',
      directory,
      '--release',
      ...(linux ? ['--target', triple + '.2.28'] : target ? ['--target', target] : []),
    ]);
    assert.equal(
      output,
      join(
        directory,
        ...(linux || target ? [triple] : []),
        'release',
        triple.includes('-windows-') ? 'maka.exe' : 'maka',
      ),
    );
  }
  assert.equal(
    await buildCli(),
    join(root, 'target/debug', process.platform === 'win32' ? 'maka.exe' : 'maka'),
  );
  assert.deepEqual(invocation.args, [
    'build',
    '--locked',
    '-p',
    'maka-cli',
    '--target-dir',
    join(root, 'target'),
  ]);
  const sourceRoot = join(root, 'extracted-source');
  const sourceEnv = { MAKA_JS_DEPS: sourceRoot };
  assert.equal(
    await buildCli({
      release: true,
      target: 'aarch64-apple-darwin',
      repositoryRoot: sourceRoot,
      env: sourceEnv,
    }),
    join(sourceRoot, 'target/aarch64-apple-darwin/release/maka'),
  );
  assert.equal(invocation.options.cwd, sourceRoot);
  assert.equal(invocation.options.env, sourceEnv);
  exitCode = 1;
  await assert.rejects(buildCli({ release: true }), /Maka build failed: 1/u);
});
