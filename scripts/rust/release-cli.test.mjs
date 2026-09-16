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
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { previewVersion, releaseNativeCli } from './release-cli.mjs';

test('preview builds have distinct package versions without changing the source version', () => {
  assert.equal(previewVersion('0.2.0', '20260916.1'), '0.2.0-rust-preview.20260916.1');
  assert.equal(previewVersion('0.2.0', '20260916.2'), '0.2.0-rust-preview.20260916.2');
  assert.equal(previewVersion('0.2.0', '123.2.gabc123'), '0.2.0-rust-preview.123.2.gabc123');
  for (const buildId of [undefined, '', '01', '1..2', '1+sha', 'a/b', 'a'.repeat(256)]) {
    assert.throws(
      () => previewVersion('0.2.0', buildId),
      /build-id|release version|256 characters/,
    );
  }
  assert.throws(() => previewVersion('0.2.0-beta', '1'), /stable source version/);
});

test('source identity failures cannot reach dependency installation or compilation', async () => {
  const stage = await mkdtemp(join(tmpdir(), 'maka-source-release-test-'));
  try {
    const name = 'apache-maka-1.2.3-incubating';
    const sourceRoot = join(stage, name);
    await mkdir(join(sourceRoot, 'src'), { recursive: true });
    const files = {
      LICENSE: 'Apache-2.0\n',
      NOTICE: 'Apache Maka\n',
      'DISCLAIMER-WIP': 'Apache Maka is undergoing incubation.\n',
      'package.json': JSON.stringify({ name: 'maka', version: '1.2.3', license: 'Apache-2.0' }),
      'package-lock.json': JSON.stringify({
        version: '1.2.3',
        lockfileVersion: 3,
        packages: { '': { version: '1.2.3' } },
      }),
      'Cargo.toml': '[package]\nname = "maka-cli"\nversion = "0.0.0"\nedition = "2024"\n',
      'src/main.rs': 'fn main() {}\n',
    };
    for (const [path, content] of Object.entries(files)) {
      await writeFile(join(sourceRoot, path), content);
    }
    const source = join(stage, name + '-src.tar.gz');
    execFileSync('tar', ['-czf', source, '-C', stage, name]);
    const digest = createHash('sha512')
      .update(await readFile(source))
      .digest('hex');
    await writeFile(source + '.sha512', digest + '  ' + name + '-src.tar.gz\n');
    const args = {
      buildId: '1',
      source,
      target: 'linux-x64-gnu',
      // Deliberately absent: source identity must fail before any of these is used.
      notices: join(stage, 'notices'),
      validator: join(stage, 'validator'),
      output: join(stage, 'output'),
    };
    await assert.rejects(releaseNativeCli(args), /Native CLI version does not match/);
    await writeFile(source, 'changed since the checksum was created');
    await assert.rejects(releaseNativeCli(args), /SHA-512 mismatch/);
    await assert.rejects(releaseNativeCli({ ...args, target: 'toString' }), /supported target/);
  } finally {
    await rm(stage, { recursive: true, force: true });
  }
});
