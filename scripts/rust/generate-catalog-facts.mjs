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
import { readFile, writeFile } from 'node:fs/promises';
import { isAbsolute, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
const root = fileURLToPath(new URL('../../', import.meta.url));
const directory = resolve(process.argv[2]);
const require = createRequire(join(resolve(process.env.MAKA_JS_DEPS || root), 'package.json'));
const { build } = require('esbuild');
const metadata = join(directory, 'model-metadata.generated.ts');
const { main } = await import('../sync-model-metadata.mjs');
await main([
  'node',
  'sync-model-metadata.mjs',
  '--snapshot',
  join(root, 'scripts/model-metadata/models-dev-api.snapshot.json'),
  '--output',
  metadata,
]);
const bundle = join(directory, 'catalog-facts-source.mjs');
const result = await build({
  entryPoints: [fileURLToPath(new URL('./catalog-facts-entry.mjs', import.meta.url))],
  outfile: bundle,
  bundle: true,
  platform: 'node',
  format: 'esm',
  target: 'node22',
  metafile: true,
  legalComments: 'eof',
  plugins: [
    {
      name: 'current-source',
      setup(build) {
        build.onResolve({ filter: /^\.\/model-metadata\.generated\.js$/ }, () => ({
          path: metadata,
        }));
        build.onResolve({ filter: /^@maka\// }, async ({ path }) => {
          const [, name, ...subpath] = path.split('/');
          const packageRoot = join(root, 'packages', name);
          const pkg = JSON.parse(await readFile(join(packageRoot, 'package.json'), 'utf8'));
          const exported = pkg.exports[subpath.length ? './' + subpath.join('/') : '.'];
          if (typeof exported !== 'string' || !exported.startsWith('./dist/')) {
            throw new Error('Cannot resolve current source: ' + path);
          }
          return {
            path: join(packageRoot, exported.replace('./dist/', 'src/').replace(/\.js$/, '.ts')),
          };
        });
        build.onResolve({ filter: /^[^./]/ }, ({ path }) => {
          if (isAbsolute(path)) return;
          const resolved = require.resolve(path);
          return {
            path: isAbsolute(resolved) ? pathToFileURL(resolved).href : resolved,
            external: true,
          };
        });
      },
    },
  ],
});
if (
  Object.keys(result.metafile.inputs).some((path) =>
    /packages\/[^/]+\/dist\//.test(path.replaceAll('\\', '/')),
  )
) {
  throw new Error('Catalog generator used stale workspace dist');
}
const source = await import(pathToFileURL(bundle));
await writeFile(
  join(directory, 'catalog-facts.json'),
  JSON.stringify(source.outputProviderFacts()),
);
await writeFile(join(directory, 'catalog-oracle.json'), JSON.stringify(source.oracleFixtures()));
