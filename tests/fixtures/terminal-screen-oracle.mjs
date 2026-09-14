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

import { createRequire } from 'node:module';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { withSourceModule } from '../support/source.mjs';

const root = fileURLToPath(new URL('../../', import.meta.url));
const require = createRequire(resolve(process.env.MAKA_JS_DEPS || root, 'package.json'));
const { Terminal } = require('@xterm/headless');
const { Unicode11Addon } = require('@xterm/addon-unicode11');
let input = '';
for await (const chunk of process.stdin) input += chunk;
await withSourceModule(
  'packages/runtime/src/pty-screen-collector.ts',
  async ({ PtyScreenCollector }) => {
    const observations = [];
    let replies = '',
      failure;
    const collector = new PtyScreenCollector({
      stack: { Terminal, Unicode11Addon },
      cols: 10,
      rows: 3,
      onProtocolReply: (data) => {
        replies += data;
      },
      onDirty() {},
      onFailure: (error) => {
        failure = error;
      },
    });
    try {
      for (const action of JSON.parse(input)) {
        replies = '';
        if (action.write !== undefined) collector.accept(action.write);
        else await collector.mutateAtCut(() => collector.resize(action.cols, action.rows));
        const { output } = await collector.snapshotAtCut();
        if (failure) throw failure;
        const { mode, cols, rows, redacted, ...screen } = output;
        if (mode !== 'pty' || redacted)
          throw new Error('Oracle fixture must remain unredacted PTY data');
        const { cols: _cols, rows: _rows, ...modes } = collector.currentInputState();
        observations.push({ screen: { ...screen, size: { cols, rows }, input: modes }, replies });
      }
      process.stdout.write(JSON.stringify(observations));
    } finally {
      collector.dispose();
    }
  },
);
