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
import { readFileSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const dependencies = process.env.MAKA_JS_DEPS || root;
const require = createRequire(resolve(dependencies, 'package.json'));
const { build } = require('esbuild');
const locked = JSON.parse(readFileSync(resolve(root, 'package-lock.json'), 'utf8'));
const modules = ['@xterm/headless', '@xterm/addon-unicode11'];
for (const name of modules) {
  const installed = require(`${name}/package.json`);
  if (installed.version !== locked.packages[`node_modules/${name}`]?.version) {
    throw new Error(`Terminal dependency does not match repository lockfile: ${name}`);
  }
}
await build({
  entryPoints: [resolve(root, 'crates/js-runtime/terminal.js')],
  outfile: resolve(process.argv[2], 'terminal.js'),
  bundle: true,
  platform: 'browser',
  format: 'iife',
  banner: {
    js: `/*\n${readFileSync(resolve(root, 'crates/js-runtime/TERMINAL_SOURCE.md'), 'utf8')}\n*/`,
  },
  plugins: [
    {
      name: 'locked-terminal-dependencies',
      setup(build) {
        build.onResolve({ filter: /^@xterm\// }, ({ path }) => ({ path: require.resolve(path) }));
      },
    },
  ],
});
