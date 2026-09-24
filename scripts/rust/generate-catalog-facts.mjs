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
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { isAbsolute, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
const root = fileURLToPath(new URL('../../', import.meta.url));
const kind = process.argv[3];
if (kind !== 'providers' && kind !== 'pricing') {
  throw new Error('Expected providers or pricing generation');
}
const destination = resolve(process.argv[2]);
await mkdir(destination, { recursive: true });
const directory = await mkdtemp(join(destination, '.catalog-'));
try {
  const require = createRequire(join(resolve(process.env.MAKA_JS_DEPS || root), 'package.json'));
  const { build } = require('esbuild');
  const metadata = join(directory, 'model-metadata.generated.ts');
  const pricing = join(directory, 'model-pricing.generated.ts');
  const { main } = await import('../sync-model-metadata.mjs');
  await main([
    'node',
    'sync-model-metadata.mjs',
    '--snapshot',
    join(root, 'scripts/model-metadata/models-dev-api.snapshot.json'),
    '--output',
    metadata,
    '--pricing-output',
    pricing,
  ]);
  const bundle = join(directory, 'catalog-facts-source.mjs');
  const result = await build({
    entryPoints: [
      fileURLToPath(
        new URL(
          kind === 'providers' ? './catalog-facts-entry.mjs' : './pricing-facts-entry.mjs',
          import.meta.url,
        ),
      ),
    ],
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
          build.onResolve({ filter: /^\.\/model-pricing\.generated\.js$/ }, () => ({
            path: pricing,
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
  if (kind === 'providers') {
    await writeFile(
      join(destination, 'catalog-facts.json'),
      JSON.stringify(source.outputProviderFacts()),
    );
  } else {
    const pricingFacts = JSON.stringify(
      [...source.output].sort((left, right) =>
        left.modelKey < right.modelKey ? -1 : left.modelKey > right.modelKey ? 1 : 0,
      ),
    );
    await writeFile(join(destination, 'pricing-facts.json'), pricingFacts);
  }
} finally {
  await rm(directory, { recursive: true, force: true });
}
