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

import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { createReadStream } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { basename, join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';
import { compareProductReleaseVersions, parseProductReleaseVersion } from '../release-version.mjs';

export const previewTargets = ['darwin-arm64', 'linux-x64-gnu', 'win32-x64'];

/** Validate the complete set before the first external write; npm has no multi-package transaction. */
export async function readPreviewRelease(directory) {
  const packages = [];
  for (const target of previewTargets) {
    const receipt = JSON.parse(await readFile(join(directory, target + '.json'), 'utf8'));
    const { name, version, integrity } = receipt;
    if (
      name !== '@maka-agent/cli-' + target ||
      receipt.target !== target ||
      !parseProductReleaseVersion(version).prerelease.join('.').startsWith('rust-preview.')
    ) {
      throw new Error('Invalid native preview receipt');
    }
    const filename = 'maka-agent-cli-' + target + '-' + version + '.tgz';
    if (basename(receipt.archive) !== filename) throw new Error('Native archive name differs');
    const archive = resolve(directory, filename);
    const hash = createHash('sha512');
    for await (const chunk of createReadStream(archive)) hash.update(chunk);
    if (integrity !== 'sha512-' + hash.digest('base64'))
      throw new Error('Native archive integrity differs');
    const manifest = JSON.parse(
      execFileSync('tar', ['-xOf', archive, 'package/package.json'], {
        encoding: 'utf8',
        maxBuffer: 64 * 1024,
        timeout: 30_000,
      }),
    );
    if (
      manifest.name !== name ||
      manifest.version !== version ||
      !/^[a-f0-9]{128}$/.test(manifest.makaSource?.sha512 ?? '') ||
      manifest.publishConfig?.tag !== 'rust-preview'
    )
      throw new Error('Native package provenance differs');
    if (
      packages.length &&
      (version !== packages[0].version ||
        JSON.stringify(manifest.makaSource) !== JSON.stringify(packages[0].source))
    ) {
      throw new Error('All platforms must come from the same source archive and build identifier');
    }
    packages.push({ name, version, archive, integrity, source: manifest.makaSource });
  }
  return packages;
}

async function metadata(path) {
  const response = await fetch('https://registry.npmjs.org/' + path, {
    redirect: 'error',
    signal: AbortSignal.timeout(30_000),
  });
  if (response.status === 404) return null;
  if (!response.ok) throw new Error('npm registry query failed: ' + response.status);
  const text = await response.text();
  if (text.length > 1024 * 1024) throw new Error('npm metadata exceeds limit');
  return JSON.parse(text);
}

export async function publishPreviewRelease(directory, { provenance = false } = {}) {
  const packages = await readPreviewRelease(directory);
  // Preflight every platform before publishing any. Existing matching immutable
  // versions are a successful retry, never grounds to upload different bytes.
  for (const pkg of packages) {
    const existing = await metadata(pkg.name + '/' + pkg.version);
    if (existing && existing.dist?.integrity !== pkg.integrity)
      throw new Error('Published version has different bytes: ' + pkg.name);
    pkg.published = Boolean(existing);
    const tags = await metadata('-/package/' + pkg.name.replace('/', '%2f') + '/dist-tags');
    if (
      tags?.['rust-preview'] &&
      compareProductReleaseVersions(tags['rust-preview'], pkg.version) > 0
    ) {
      throw new Error('Refusing to move rust-preview backwards: ' + pkg.name);
    }
  }
  for (const pkg of packages) {
    if (!pkg.published) {
      execFileSync(
        'npm',
        [
          'publish',
          pkg.archive,
          '--tag',
          'rust-preview',
          '--access',
          'public',
          '--ignore-scripts',
          ...(provenance ? ['--provenance'] : []),
        ],
        { stdio: 'inherit', timeout: 180_000 },
      );
    }
    let verified = false;
    for (let attempt = 0; attempt < 5; attempt++) {
      const observed = await metadata(pkg.name + '/' + pkg.version);
      if (observed?.dist?.integrity === pkg.integrity) {
        verified = true;
        break;
      }
      await new Promise((resolveDelay) => setTimeout(resolveDelay, 1000 * 2 ** attempt));
    }
    if (!verified)
      throw new Error('Publication is unconfirmed; retry this same artifact set: ' + pkg.name);
    console.log(pkg.name + '@' + pkg.version + ' verified');
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { values, positionals } = parseArgs({
    allowPositionals: true,
    options: {
      publish: { type: 'boolean', default: false },
      provenance: { type: 'boolean', default: false },
    },
  });
  if (positionals.length !== 1)
    throw new Error('Usage: publish-cli.mjs <artifact-directory> [--publish] [--provenance]');
  if (values.publish) await publishPreviewRelease(positionals[0], values);
  else console.log(JSON.stringify(await readPreviewRelease(positionals[0])));
}
