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
import test from 'node:test';
import { launchNativeRuntimeHostCandidate } from '../client/native-launcher.js';
import { initializeNativeRuntimeHost } from '../client/native-initialization.js';

test('native candidate uses an owned byte pipe and distinguishes release from drain', {
  timeout: 5_000,
}, async () => {
  for (const released of [false, true]) {
    const preload = `
      import assert from 'node:assert/strict';
      import { basename } from 'node:path';
      assert.deepEqual([basename(process.argv[1]), process.argv[2]], ['host', 'candidate']);
      assert.ok(process.argv.includes('--owner-stdin'));
      assert.equal(process.channel, undefined);
      let input = '';
      process.stdin.setEncoding('utf8');
      for await (const chunk of process.stdin) input += chunk;
      assert.equal(input, ${JSON.stringify(released ? '{"kind":"runtime-host-launch-owner-release"}\n' : '')});
      process.stderr.write('candidate-finished');
      process.exit(0);
    `;
    const launch = launchNativeRuntimeHostCandidate(process.execPath, {
      rootPath: '/unused',
      expectedRootId: 'a'.repeat(64),
      entrypoint: 'unused',
      env: { NODE_OPTIONS: '--import=data:text/javascript,' + encodeURIComponent(preload) },
    });
    const host = await launch.spawned;
    try {
      if (released) host.releaseToEnvironment();
      assert.equal(await host.settle(2_000), true, JSON.stringify(await host.exited));
      assert.equal((await host.exited)?.stderr, 'candidate-finished');
    } finally {
      await host.settle(1_000);
    }
  }
});

test('native initialization sends private settings only over stdin and suppresses child diagnostics', {
  timeout: 5_000,
  concurrency: false,
}, async () => {
  const previous = process.env.NODE_OPTIONS;
  const preload = `
    import assert from 'node:assert/strict';
    import { basename } from 'node:path';
    assert.deepEqual([basename(process.argv[1]), process.argv[2]], ['host', 'init']);
    assert.ok(process.argv.includes('--settings-stdin'));
    assert.ok(!process.argv.join(' ').includes('p@ss'));
    let input = '';
    for await (const chunk of process.stdin) input += chunk;
    assert.deepEqual(JSON.parse(input), {
      privacy:{incognitoActive:true},
      networkProxy:{enabled:true,protocol:'http',host:'::1',port:8080,authEnabled:true,
        username:'user',bypassList:[],autoBypassDomains:[]},
      proxyPassword:'p@ss',
    });
    if (process.argv.includes('reject')) {
      process.stderr.write(input);
      process.exit(7);
    }
    console.log(JSON.stringify({rootId:'a'.repeat(64)}));
    process.exit(0);
  `;
  process.env.NODE_OPTIONS = '--import=data:text/javascript,' + encodeURIComponent(preload);
  try {
    const settings = { incognito: true as const, proxyUrl: 'http://user:p%40ss@[::1]:8080' };
    await initializeNativeRuntimeHost(process.execPath, 'fresh', settings);
    await assert.rejects(initializeNativeRuntimeHost(process.execPath, 'reject', settings), {
      message: 'Native State Root initialization failed',
    });
    await assert.rejects(
      initializeNativeRuntimeHost(process.execPath, 'fresh', {
        incognito: true,
        proxyUrl: 'http://user:p%40ss@localhost:8080/not-a-proxy',
      }),
      { message: 'Invalid hosted HTTP proxy' },
    );
  } finally {
    if (previous === undefined) delete process.env.NODE_OPTIONS;
    else process.env.NODE_OPTIONS = previous;
  }
});
