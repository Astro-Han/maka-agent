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

import type { build as Build } from 'esbuild';

/** Bundle trusted Client code without eval, native ESM retention, or Node shims. */
export async function buildClient(
  options: {
    readonly packageId: string;
    readonly entryPoint: string;
    readonly dependencies?: readonly string[];
  },
  compile?: typeof Build,
): Promise<string> {
  const identifier = /^[a-z][a-z0-9]*(?:[._:-][a-z0-9]+)*$/;
  const dependencies = options.dependencies ?? [];
  for (const id of [options.packageId, ...dependencies]) {
    if (id.length > 128 || !identifier.test(id)) throw new Error('Invalid Client package identity');
  }
  if (dependencies.length > 128) throw new Error('Client dependency limit exceeded');
  const build = compile ?? (await import('esbuild')).build;
  const result = await build({
    entryPoints: [options.entryPoint],
    bundle: true,
    write: false,
    platform: 'browser',
    format: 'cjs',
    target: 'es2022',
    jsx: 'automatic',
    loader: { '.css': 'text' },
    legalComments: 'inline',
    external: [
      'react',
      'react/jsx-runtime',
      '@maka-agent/plugin-sdk/client',
      '@maka/ui/plugin',
      ...dependencies,
    ],
    banner: {
      js:
        'window.__MakaClientBundle__({id:' +
        JSON.stringify(options.packageId) +
        ',factory(require){const module={exports:{}};const exports=module.exports;',
    },
    footer: { js: 'return module.exports;}});' },
  });
  const output = result.outputFiles[0];
  if (result.outputFiles.length !== 1 || output.contents.byteLength > 8 * 1024 * 1024)
    throw new Error('Client bundle must be one JavaScript file of at most 8 MiB');
  return output.text;
}
