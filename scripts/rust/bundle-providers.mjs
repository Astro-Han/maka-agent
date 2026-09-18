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
import { resolve, join } from 'node:path';
import { writeFile } from 'node:fs/promises';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));
const dependencyRoot = process.env.MAKA_JS_DEPS || root;
const require = createRequire(join(dependencyRoot, 'package.json'));
const { build, transform } = require('esbuild');
const output = process.argv[2];
if (!output) throw new Error('Cargo output directory is required');
const locked = JSON.parse(readFileSync(join(root, 'package-lock.json'), 'utf8'));
const entry = resolve(root, 'crates/js-runtime/trusted/adapter.js');
for (const name of [
  '@ai-sdk/openai',
  '@ai-sdk/anthropic',
  '@ai-sdk/openai-compatible',
  '@ai-sdk/open-responses',
]) {
  if (
    require(`${name}/package.json`).version !== locked.packages[`node_modules/${name}`]?.version
  ) {
    throw new Error(`Provider SDK does not match repository lockfile: ${name}`);
  }
}
await build({
  entryPoints: [entry],
  outfile: resolve(output, 'providers.js'),
  bundle: true,
  platform: 'browser',
  format: 'iife',
  globalName: 'MakaProvider',
  target: 'es2024',
  nodePaths: [join(dependencyRoot, 'node_modules')],
  plugins: [
    {
      name: 'locked-provider-dependencies',
      setup(build) {
        build.onResolve(
          { filter: /^@ai-sdk\/(openai|anthropic|openai-compatible|open-responses)$/ },
          (args) => {
            if (args.importer !== entry) return;
            // Keep import conditions and transitive resolution, but start from the checked install.
            return build.resolve(args.path, { resolveDir: dependencyRoot, kind: args.kind });
          },
        );
      },
    },
  ],
  legalComments: 'eof',
});
// Tests check these unmodified sources against the locked Deno extension.
// Transpilation needs neither a build-time V8 nor a compiler in the running host.
for (const name of ['telemetry', 'util']) {
  const source = readFileSync(
    join(root, 'crates/js-runtime/third-party/deno-telemetry', `${name}.ts`),
    'utf8',
  );
  const { code } = await transform(source, {
    loader: 'ts',
    target: 'es2024',
    legalComments: 'inline',
  });
  await writeFile(join(output, `${name}.js`), code);
}
