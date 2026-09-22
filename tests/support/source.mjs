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
import { spawnSync } from 'node:child_process';
import { readFile, mkdtemp, rm } from 'node:fs/promises';
import { isAbsolute, join, relative, resolve } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath, pathToFileURL } from 'node:url';

export async function withSourceModule(entry, verify) {
  return withSourceBundle(entry, async (outfile) =>
    verify(await import(pathToFileURL(outfile).href)),
  );
}

export async function withSourceBundle(entry, verify) {
  const root = fileURLToPath(new URL('../../', import.meta.url));
  const dependencyRoot = resolve(process.env.MAKA_JS_DEPS || root);
  const require = createRequire(join(dependencyRoot, 'package.json'));
  const temporary = await mkdtemp(join(tmpdir(), 'maka-source-contract-'));
  try {
    const outfile = join(temporary, 'source.mjs');
    const result = await require('esbuild').build({
      entryPoints: [resolve(root, entry)],
      outfile,
      bundle: true,
      platform: 'node',
      format: 'esm',
      target: 'node22',
      metafile: true,
      legalComments: 'eof',
      plugins: [currentSourcePlugin(temporary)],
    });

    const inputs = Object.keys(result.metafile.inputs).map((path) => path.replaceAll('\\', '/'));
    if (inputs.some((input) => /packages\/[^/]+\/dist\//.test(input))) {
      throw new Error('Bundle contains stale workspace dist');
    }
    return await verify(outfile, inputs);
  } finally {
    await rm(temporary, { recursive: true, force: true });
  }
}

/** Resolve original workspace sources consistently for every cross-language oracle. */
export function currentSourcePlugin(temporary) {
  const root = fileURLToPath(new URL('../../', import.meta.url));
  const dependencyRoot = resolve(process.env.MAKA_JS_DEPS || root);
  const require = createRequire(join(dependencyRoot, 'package.json'));
  let metadata;
  return {
    name: 'worktree-source',
    setup(build) {
      build.onResolve({ filter: /^\.\/model-metadata\.generated\.js$/ }, async () => {
        metadata ??= (async () => {
          const generated = join(temporary, 'model-metadata.generated.ts');
          const { main } = await import('../../scripts/sync-model-metadata.mjs');
          await main([
            'node',
            'sync-model-metadata.mjs',
            '--snapshot',
            join(root, 'scripts/model-metadata/models-dev-api.snapshot.json'),
            '--output',
            generated,
          ]);
          return generated;
        })();
        return { path: await metadata };
      });
      build.onResolve(
        { filter: /^@(?:maka\/|maka-agent\/plugin-sdk(?:\/|$))/ },
        async ({ path }) => {
          const [, name, ...subpath] = path.split('/');
          const packageRoot = join(root, 'packages', name);
          const metadata = JSON.parse(await readFile(join(packageRoot, 'package.json'), 'utf8'));
          const entry = metadata.exports[subpath.length ? './' + subpath.join('/') : '.'];
          const exported = typeof entry === 'string' ? entry : entry?.default;
          if (typeof exported !== 'string' || !exported.startsWith('./dist/')) throw Error(path);
          return {
            path: join(packageRoot, exported.replace('./dist/', 'src/').replace(/\.js$/, '.ts')),
          };
        },
      );
      build.onResolve({ filter: /^[^./]/ }, ({ path, resolveDir }) => {
        if (isAbsolute(path) || path.startsWith('@maka/')) return;
        // Preserve the importing workspace's nested dependency versions;
        // root hoisting can contain a different major (e.g. proxy agents).
        const resolved = require.resolve(path, {
          paths: [join(dependencyRoot, relative(root, resolveDir)), dependencyRoot],
        });
        return {
          path: isAbsolute(resolved) ? pathToFileURL(resolved).href : resolved,
          external: true,
        };
      });
    },
  };
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  if (process.argv.length !== 3) throw new Error('Expected one source fixture path');
  await withSourceBundle(process.argv[2], (bundle) => {
    const child = spawnSync(process.execPath, [bundle], { stdio: 'inherit', timeout: 15000 });
    if (child.error) throw child.error;
    if (child.signal) throw new Error(`Source fixture terminated by ${child.signal}`);
    process.exitCode = child.status ?? 1;
  });
}
