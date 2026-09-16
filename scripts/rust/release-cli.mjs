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

import { execFile, spawn } from 'node:child_process';
import { constants } from 'node:fs';
import { copyFile, mkdir, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { parseArgs, promisify } from 'node:util';
import { controlledProcessEnvironment, verifySourceCandidate } from '../asf-source-release.mjs';
import { npmSpawnOptions } from '../npm-spawn.mjs';
import { buildCli, rustTarget } from './build-cli.mjs';
import { nativeCliTargets, packNativeCli } from './pack-cli.mjs';

const run = promisify(execFile);

/** Build only the verified archive, never caller-supplied executable bytes. Does not publish. */
export async function releaseNativeCli({ source, keys, target, notices, validator, output }) {
  const platform = Object.hasOwn(nativeCliTargets, target) ? nativeCliTargets[target] : undefined;
  if (!platform || !source || !notices || !validator || !output) {
    throw new Error('source, supported target, notices, validator, and output are required');
  }
  const stage = await mkdtemp(join(tmpdir(), 'maka-native-source-'));
  try {
    // Snapshot before validation so later changes to the input cannot change the build.
    const archivePath = join(stage, basename(source));
    for (const suffix of ['', '.sha512', ...(keys ? ['.asc'] : [])]) {
      await copyFile(resolve(source) + suffix, archivePath + suffix, constants.COPYFILE_EXCL);
    }
    const candidate = await verifySourceCandidate({
      archivePath,
      ...(keys ? { keysPath: resolve(keys) } : {}),
    });
    const extraction = join(stage, 'source');
    await mkdir(extraction);
    await run('tar', ['-xzf', archivePath, '-C', extraction], {
      env: controlledProcessEnvironment({
        excludedNames: ['GZIP', 'TAR_OPTIONS', 'TAR_READER_OPTIONS'],
      }),
      timeout: 180_000,
      windowsHide: true,
    });
    const repositoryRoot = join(extraction, candidate.rootDirectory);
    // JS must come from this install, not an ambient MAKA_JS_DEPS checkout.
    const env = controlledProcessEnvironment({
      excludedNames: ['MAKA_JS_DEPS', 'CARGO_BUILD_TARGET'],
      overrides: { MAKA_JS_DEPS: repositoryRoot },
    });
    const metadata = JSON.parse(
      (
        await run('cargo', ['metadata', '--locked', '--no-deps', '--format-version', '1'], {
          cwd: repositoryRoot,
          env,
          encoding: 'utf8',
          maxBuffer: 4 * 1024 * 1024,
          timeout: 180_000,
          windowsHide: true,
        })
      ).stdout,
    );
    const cli = metadata.packages.find((pkg) => pkg.name === 'maka-cli');
    if (cli?.version !== candidate.version) {
      throw new Error('Native CLI version does not match the source archive');
    }
    // Only the dependency patches needed by the Rust JS bundles; no Electron,
    // Git hooks or unrelated workspace lifecycle scripts.
    await execute(
      'npm',
      ['ci', '--include=dev', '--include=optional', '--ignore-scripts', '--no-audit', '--no-fund'],
      npmSpawnOptions({ cwd: repositoryRoot, env }),
    );
    await execute(process.execPath, ['scripts/apply-dependency-patches.mjs'], {
      cwd: repositoryRoot,
      env,
    });
    const binary = await buildCli({
      release: true,
      target: rustTarget(platform.os, platform.cpu),
      repositoryRoot,
      env,
    });
    return await packNativeCli({
      target,
      version: candidate.version,
      binary,
      notices,
      validator,
      output,
      repositoryRoot,
      source: { archive: basename(source), sha512: candidate.digest },
    });
  } finally {
    await rm(stage, { recursive: true, force: true });
  }
}

async function execute(command, args, options) {
  await new Promise((resolveCommand, reject) => {
    const child = spawn(command, args, { ...options, stdio: 'inherit', windowsHide: true });
    child.once('error', reject);
    child.once('exit', (code, signal) => {
      if (code === 0) resolveCommand();
      else reject(new Error('Native source build failed: ' + command + ' ' + (signal ?? code)));
    });
  });
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { values } = parseArgs({
    options: Object.fromEntries(
      ['source', 'keys', 'target', 'notices', 'validator', 'output'].map((name) => [
        name,
        { type: 'string' },
      ]),
    ),
  });
  console.log(JSON.stringify(await releaseNativeCli(values)));
}
