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
import { createHash } from 'node:crypto';
import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { stringify } from 'yaml';
import {
  addDesktopNightlyAttestation,
  assertDesktopNightlyFeedAdvance,
  resolveDesktopNightlyCutover,
  stageDesktopNightly,
} from './desktop-nightly.mjs';
import { verifyDesktopUpdateArtifacts } from './desktop-update-contract.mjs';

async function writeUpdateSet(directory, version, platform) {
  const isMac = platform === 'mac';
  const artifact = isMac ? `Maka-${version}-mac-arm64.zip` : `Maka-${version}-win-x64.exe`;
  const metadata = isMac ? 'dev-mac.yml' : 'dev.yml';
  const bytes = Buffer.from(`${platform} nightly bytes`);
  const sha512 = createHash('sha512').update(bytes).digest('base64');
  await writeFile(join(directory, artifact), bytes);
  await writeFile(join(directory, `${artifact}.blockmap`), `${platform} blockmap`);
  await writeFile(
    join(directory, metadata),
    stringify({
      version,
      files: [{ url: artifact, sha512, size: bytes.byteLength }],
      path: artifact,
      sha512,
      releaseDate: '2026-08-29T18:17:00.000Z',
    }),
  );
}

function publishedNightly(version) {
  return {
    tag_name: `v${version}`,
    draft: false,
    prerelease: true,
    assets: [
      `Maka-${version}-mac-arm64.dmg`,
      `Maka-${version}-mac-arm64.zip`,
      `Maka-${version}-mac-arm64.zip.blockmap`,
      `Maka-${version}-win-x64.exe`,
      `Maka-${version}-win-x64.exe.blockmap`,
      `Maka-${version}-win-x64.zip`,
      `Maka-${version}-attestation.sigstore.json`,
      'dev-mac.yml',
      'dev.yml',
    ].map((name) => ({ name })),
  };
}

async function writeCutoverMarker(directory, version) {
  await writeFile(
    join(directory, 'github-cutover.json'),
    `${JSON.stringify({ schemaVersion: 1, authority: 'github-releases', version })}\n`,
  );
}

test('staging creates the exact GitHub assets and a payload-free legacy bridge', async (t) => {
  const root = await mkdtemp(join(tmpdir(), 'maka-desktop-nightly-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const input = join(root, 'input');
  const output = join(root, 'output');
  const version = '0.2.0-dev.42.20260829';
  await mkdir(input);
  await Promise.all([
    writeUpdateSet(input, version, 'mac'),
    writeUpdateSet(input, version, 'win'),
    writeFile(join(input, `Maka-${version}-mac-arm64.dmg`), 'dmg'),
    writeFile(join(input, `Maka-${version}-win-x64.zip`), 'windows zip'),
  ]);

  await stageDesktopNightly({
    inputDirectory: input,
    outputDirectory: output,
    version,
    sourceCommit: 'a'.repeat(40),
  });

  const payloadNames = [
    `Maka-${version}-mac-arm64.dmg`,
    `Maka-${version}-mac-arm64.zip`,
    `Maka-${version}-mac-arm64.zip.blockmap`,
    `Maka-${version}-win-x64.exe`,
    `Maka-${version}-win-x64.exe.blockmap`,
    `Maka-${version}-win-x64.zip`,
  ];
  const release = join(output, 'release');
  for (const name of payloadNames) {
    assert.deepEqual(await readFile(join(release, name)), await readFile(join(input, name)), name);
  }
  await Promise.all([
    verifyDesktopUpdateArtifacts({
      directory: release,
      metadataName: 'dev-mac.yml',
      version,
      artifactName: `Maka-${version}-mac-arm64.zip`,
    }),
    verifyDesktopUpdateArtifacts({
      directory: release,
      metadataName: 'dev.yml',
      version,
      artifactName: `Maka-${version}-win-x64.exe`,
    }),
  ]);

  const macMetadata = (await import('yaml')).parse(
    await readFile(join(output, 'bridge', 'feed', 'latest-mac.yml'), 'utf8'),
  );
  const windowsMetadata = (await import('yaml')).parse(
    await readFile(join(output, 'bridge', 'feed', 'latest.yml'), 'utf8'),
  );
  assert.equal(
    macMetadata.files[0].url,
    `https://github.com/apache/maka/releases/download/v${version}/Maka-${version}-mac-arm64.zip`,
  );
  assert.equal(
    windowsMetadata.path,
    `https://github.com/apache/maka/releases/download/v${version}/Maka-${version}-win-x64.exe`,
  );
  const index = await readFile(join(output, 'bridge', 'feed', 'index.html'), 'utf8');
  assert.match(index, /Desktop Nightly is a developer snapshot, not an Apache release/u);
  assert.match(index, new RegExp(`source commit <code>${'a'.repeat(40)}</code>`, 'u'));
  assert.match(
    index,
    new RegExp(`releases/download/v${version}/Maka-${version}-mac-arm64\\.dmg`, 'u'),
  );
  assert.match(
    index,
    new RegExp(`releases/download/v${version}/Maka-${version}-win-x64\\.exe`, 'u'),
  );
  assert.deepEqual((await readdir(join(output, 'bridge', 'feed'))).sort(), [
    'index.html',
    'latest-mac.yml',
    'latest.yml',
  ]);
  assert.deepEqual(
    JSON.parse(await readFile(join(output, 'bridge', 'completion', 'github-cutover.json'), 'utf8')),
    { schemaVersion: 1, authority: 'github-releases', version },
  );
  assert.deepEqual(
    (await readdir(release)).sort(),
    [...payloadNames, 'dev-mac.yml', 'dev.yml'].sort(),
  );
  assert.deepEqual((await readdir(join(output, 'bridge'))).sort(), ['completion', 'feed']);
});

test('one attestation bundle is staged as a GitHub asset and at the legacy fixed path', async (t) => {
  const root = await mkdtemp(join(tmpdir(), 'maka-desktop-nightly-attestation-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const output = join(root, 'output');
  const release = join(output, 'release');
  const version = '0.2.0-dev.42.20260829';
  const bundle = join(root, 'bundle.json');
  const bytes = Buffer.from('one offline Sigstore bundle');
  await Promise.all([mkdir(release, { recursive: true }), writeFile(bundle, bytes)]);

  await addDesktopNightlyAttestation({ outputDirectory: output, version, bundlePath: bundle });

  const name = `Maka-${version}-attestation.sigstore.json`;
  assert.deepEqual(await readFile(join(release, name)), bytes);
  assert.deepEqual(await readFile(join(output, 'bridge', 'versions', version, name)), bytes);
});

test('the Desktop feed advances only to a newer npm run number', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'maka-desktop-nightly-feed-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  await Promise.all([
    writeFile(join(directory, 'latest-mac.yml'), 'version: 0.2.0-dev.42.20260829\n'),
    writeFile(join(directory, 'latest.yml'), 'version: 0.2.0-dev.42.20260829\n'),
  ]);
  await assert.doesNotReject(
    assertDesktopNightlyFeedAdvance({
      directory,
      candidateVersion: '0.3.0-dev.43.20260828',
      productVersion: '0.3.0',
    }),
  );
  await assert.rejects(
    assertDesktopNightlyFeedAdvance({
      directory,
      candidateVersion: '0.2.0-dev.41.20260830',
      productVersion: '0.2.0',
    }),
    /does not advance current run/u,
  );
});

test('the pending legacy feed bridges only for its explicitly authorized source SHA', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'maka-desktop-nightly-cutover-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const previous = '0.2.0-dev.41.20260828';
  const candidate = '0.2.0-dev.42.20260829';
  const sourceCommit = 'a'.repeat(40);
  await Promise.all([
    writeFile(
      join(directory, 'latest-mac.yml'),
      `version: ${previous}\npath: versions/${previous}/Maka-${previous}-mac-arm64.zip\n`,
    ),
    writeFile(
      join(directory, 'latest.yml'),
      `version: ${previous}\npath: versions/${previous}/Maka-${previous}-win-x64.exe\n`,
    ),
  ]);

  assert.deepEqual(
    await resolveDesktopNightlyCutover({
      candidateVersion: candidate,
      cutoverSourceCommit: sourceCommit,
      feedDirectory: directory,
      productVersion: '0.2.0',
      releases: [],
      sourceCommit,
    }),
    { bridge: true, previousVersion: undefined },
  );
  await assert.rejects(
    resolveDesktopNightlyCutover({
      candidateVersion: candidate,
      cutoverSourceCommit: 'b'.repeat(40),
      feedDirectory: directory,
      productVersion: '0.2.0',
      releases: [],
      sourceCommit,
    }),
    /cutover source SHA/u,
  );
  await writeFile(
    join(directory, 'latest.yml'),
    `version: ${previous}\npath: https://example.invalid/Maka-${previous}-win-x64.exe\n`,
  );
  await assert.rejects(
    resolveDesktopNightlyCutover({
      candidateVersion: candidate,
      cutoverSourceCommit: sourceCommit,
      feedDirectory: directory,
      productVersion: '0.2.0',
      releases: [],
      sourceCommit,
    }),
    /legacy Nightlies asset/u,
  );
});

test('the completed bridge advances from the latest valid GitHub dev prerelease without writing Nightlies', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'maka-desktop-nightly-steady-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const previous = '0.2.0-dev.41.20260828';
  const candidate = '0.2.0-dev.42.20260829';
  const sourceCommit = 'a'.repeat(40);
  const base = `https://github.com/apache/maka/releases/download/v${previous}`;
  await Promise.all([
    writeFile(
      join(directory, 'latest-mac.yml'),
      `version: ${previous}\npath: ${base}/Maka-${previous}-mac-arm64.zip\n`,
    ),
    writeFile(
      join(directory, 'latest.yml'),
      `version: ${previous}\npath: ${base}/Maka-${previous}-win-x64.exe\n`,
    ),
  ]);
  const input = {
    candidateVersion: candidate,
    cutoverSourceCommit: sourceCommit,
    feedDirectory: directory,
    productVersion: '0.2.0',
    releases: [publishedNightly(previous)],
    sourceCommit,
  };

  assert.deepEqual(await resolveDesktopNightlyCutover(input), {
    bridge: true,
    previousVersion: previous,
  });
  await writeCutoverMarker(directory, previous);
  assert.deepEqual(await resolveDesktopNightlyCutover(input), {
    bridge: false,
    previousVersion: previous,
  });
  await assert.rejects(
    resolveDesktopNightlyCutover({ ...input, candidateVersion: previous }),
    /does not advance current run/u,
  );
  await assert.rejects(
    resolveDesktopNightlyCutover({
      ...input,
      releases: [{ ...publishedNightly(previous), assets: [] }],
    }),
    /GitHub dev prerelease assets/u,
  );
  await Promise.all([
    writeFile(
      join(directory, 'latest-mac.yml'),
      `version: ${previous}\npath: https://github.com/apache/maka/releases/download/v9.9.9/Maka-${previous}-mac-arm64.zip\n`,
    ),
    writeFile(
      join(directory, 'latest.yml'),
      `version: ${previous}\npath: https://github.com/apache/maka/releases/download/v9.9.9/Maka-${previous}-win-x64.exe\n`,
    ),
  ]);
  await assert.rejects(resolveDesktopNightlyCutover(input), /exact GitHub Release/u);
  const missing = '0.2.0-dev.40.20260827';
  const missingBase = `https://github.com/apache/maka/releases/download/v${missing}`;
  await Promise.all([
    writeFile(
      join(directory, 'latest-mac.yml'),
      `version: ${missing}\npath: ${missingBase}/Maka-${missing}-mac-arm64.zip\n`,
    ),
    writeFile(
      join(directory, 'latest.yml'),
      `version: ${missing}\npath: ${missingBase}/Maka-${missing}-win-x64.exe\n`,
    ),
  ]);
  await assert.rejects(resolveDesktopNightlyCutover(input), /completed bridge/u);
});

test('a partially published legacy bridge converges on the next authorized fresh run', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'maka-desktop-nightly-partial-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const legacy = '0.2.0-dev.40.20260827';
  const published = '0.2.0-dev.41.20260828';
  const candidate = '0.2.0-dev.42.20260829';
  const sourceCommit = 'a'.repeat(40);
  await Promise.all([
    writeFile(
      join(directory, 'latest-mac.yml'),
      `version: ${published}\npath: https://github.com/apache/maka/releases/download/v${published}/Maka-${published}-mac-arm64.zip\n`,
    ),
    writeFile(
      join(directory, 'latest.yml'),
      `version: ${legacy}\npath: versions/${legacy}/Maka-${legacy}-win-x64.exe\n`,
    ),
  ]);

  assert.deepEqual(
    await resolveDesktopNightlyCutover({
      candidateVersion: candidate,
      cutoverSourceCommit: sourceCommit,
      feedDirectory: directory,
      productVersion: '0.2.0',
      releases: [publishedNightly(published)],
      sourceCommit,
    }),
    { bridge: true, previousVersion: published },
  );
});

test('retained Nightlies from an older product line remain the cross-version ordering authority', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'maka-desktop-nightly-retained-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const previous = '0.2.0-dev.42.20260829';
  const candidate = '0.3.0-dev.43.20260830';
  const base = `https://github.com/apache/maka/releases/download/v${previous}`;
  await Promise.all([
    writeFile(
      join(directory, 'latest-mac.yml'),
      `version: ${previous}\npath: ${base}/Maka-${previous}-mac-arm64.zip\n`,
    ),
    writeFile(
      join(directory, 'latest.yml'),
      `version: ${previous}\npath: ${base}/Maka-${previous}-win-x64.exe\n`,
    ),
  ]);
  await writeCutoverMarker(directory, previous);

  assert.deepEqual(
    await resolveDesktopNightlyCutover({
      candidateVersion: candidate,
      cutoverSourceCommit: '',
      feedDirectory: directory,
      productVersion: '0.3.0',
      releases: [publishedNightly(previous)],
      sourceCommit: 'a'.repeat(40),
    }),
    { bridge: false, previousVersion: previous },
  );
});
