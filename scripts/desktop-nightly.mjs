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

import {
  appendFile,
  copyFile,
  mkdir,
  readFile,
  readdir,
  rm,
  stat,
  writeFile,
} from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { verifyDesktopUpdateArtifacts } from './desktop-update-contract.mjs';
import {
  assertProductNightlyAdvances,
  assertProductNightlyVersion,
  parseProductNightlyVersion,
  productNightlyRunNumber,
} from './release-version.mjs';

const repoRoot = dirname(dirname(fileURLToPath(import.meta.url)));

export function assertDesktopNightlyVersion(version, productVersion) {
  return assertProductNightlyVersion(version, productVersion);
}

export function resolveDesktopBuildVersion(productVersion, environment = process.env) {
  const nightlyVersion = environment.MAKA_DESKTOP_NIGHTLY_VERSION?.trim();
  return nightlyVersion
    ? assertDesktopNightlyVersion(nightlyVersion, productVersion)
    : productVersion;
}

export function resolveRuntimeHostSetupPackage(productVersion, environment = process.env) {
  return `maka-agent@${resolveDesktopBuildVersion(productVersion, environment)}`;
}

export async function assertDesktopNightlyFeedAdvance({
  directory,
  candidateVersion,
  productVersion,
}) {
  const { parse } = await import('yaml');
  for (const name of ['latest-mac.yml', 'latest.yml']) {
    let source;
    try {
      source = await readFile(join(directory, name), 'utf8');
    } catch (error) {
      if (error?.code === 'ENOENT') continue;
      throw error;
    }
    const currentVersion = parse(source)?.version;
    if (typeof currentVersion !== 'string') {
      throw new Error(`Desktop Nightly feed ${name} has no valid version`);
    }
    assertProductNightlyAdvances(candidateVersion, currentVersion, productVersion);
  }
  return candidateVersion;
}

async function readLegacyNightlyFeed(directory) {
  const { parse } = await import('yaml');
  const entries = [];
  for (const name of ['latest-mac.yml', 'latest.yml']) {
    const metadata = parse(await readFile(join(directory, name), 'utf8'));
    if (typeof metadata?.version !== 'string' || typeof metadata?.path !== 'string') {
      throw new Error(`Desktop Nightly feed ${name} has no valid versioned payload`);
    }
    entries.push({ name, metadata });
  }
  return entries;
}

async function readCutoverMarker(directory) {
  let source;
  try {
    source = await readFile(join(directory, 'github-cutover.json'), 'utf8');
  } catch (error) {
    if (error?.code === 'ENOENT') return undefined;
    throw error;
  }
  let marker;
  try {
    marker = JSON.parse(source);
  } catch (error) {
    throw new Error('Desktop Nightly cutover marker is invalid', { cause: error });
  }
  if (
    marker?.schemaVersion !== 1 ||
    marker?.authority !== 'github-releases' ||
    typeof marker?.version !== 'string'
  ) {
    throw new Error('Desktop Nightly cutover marker is invalid');
  }
  parseProductNightlyVersion(marker.version);
  return marker;
}

export function desktopNightlyReleaseAssetNames(version) {
  const names = nightlyArtifactNames(version);
  return [
    names.macDmg,
    names.macZip,
    `${names.macZip}.blockmap`,
    names.windowsExe,
    `${names.windowsExe}.blockmap`,
    names.windowsZip,
    `Maka-${version}-attestation.sigstore.json`,
    'dev-mac.yml',
    'dev.yml',
  ].sort();
}

function latestPublishedDesktopNightly(releases) {
  let latest;
  const versions = new Set();
  for (const release of releases.flat()) {
    if (release?.draft === true || release?.prerelease !== true) continue;
    const match = /^v(.+-dev\..+)$/u.exec(release?.tag_name ?? '');
    if (!match) continue;
    const version = match[1];
    parseProductNightlyVersion(version);
    const actual = (release.assets ?? []).map(({ name }) => name).sort();
    const expected = desktopNightlyReleaseAssetNames(version);
    if (JSON.stringify(actual) !== JSON.stringify(expected)) {
      throw new Error(`GitHub dev prerelease assets are invalid for v${version}`);
    }
    versions.add(version);
    if (!latest || productNightlyRunNumber(version) > productNightlyRunNumber(latest)) {
      latest = version;
    }
  }
  return { latest, versions };
}

export async function resolveDesktopNightlyCutover({
  candidateVersion,
  cutoverSourceCommit,
  feedDirectory,
  productVersion,
  releases,
  sourceCommit,
}) {
  assertDesktopNightlyVersion(candidateVersion, productVersion);
  if (!Array.isArray(releases)) throw new Error('GitHub Releases must be an array');
  const published = latestPublishedDesktopNightly(releases);
  const previousVersion = published.latest;
  assertProductNightlyAdvances(candidateVersion, previousVersion, productVersion);
  const feeds = await readLegacyNightlyFeed(feedDirectory);
  const marker = await readCutoverMarker(feedDirectory);
  const bridgeStates = feeds.map(({ name, metadata }) => {
    const artifact =
      name === 'latest-mac.yml'
        ? `Maka-${metadata.version}-mac-arm64.zip`
        : `Maka-${metadata.version}-win-x64.exe`;
    const expected = githubNightlyAssetUrl(metadata.version, artifact);
    const githubPath = metadata.path.startsWith(
      'https://github.com/apache/maka/releases/download/',
    );
    if (githubPath && metadata.path !== expected) {
      throw new Error(`${name} does not point to the exact GitHub Release asset`);
    }
    if (
      githubPath &&
      Array.isArray(metadata.files) &&
      (metadata.files.length !== 1 || metadata.files[0]?.url !== expected)
    ) {
      throw new Error(`${name} does not point to the exact GitHub Release asset`);
    }
    const legacy = `versions/${metadata.version}/${artifact}`;
    if (
      !githubPath &&
      (metadata.path !== legacy ||
        (Array.isArray(metadata.files) &&
          (metadata.files.length !== 1 || metadata.files[0]?.url !== legacy)))
    ) {
      throw new Error(`${name} does not point to the exact legacy Nightlies asset`);
    }
    return githubPath;
  });
  const bridged = bridgeStates.every(Boolean);
  if (marker) {
    if (
      !bridged ||
      feeds.some(({ metadata }) => metadata.version !== marker.version) ||
      !published.versions.has(marker.version)
    ) {
      throw new Error('Desktop Nightly completed bridge does not match a GitHub dev prerelease');
    }
    return { bridge: false, previousVersion };
  }
  for (const [index, state] of bridgeStates.entries()) {
    if (state && !published.versions.has(feeds[index].metadata.version)) {
      throw new Error('Desktop Nightly legacy feed points to a missing GitHub dev prerelease');
    }
  }
  await assertDesktopNightlyFeedAdvance({
    directory: feedDirectory,
    candidateVersion,
    productVersion,
  });
  if (cutoverSourceCommit !== sourceCommit || !/^[0-9a-f]{40}$/u.test(sourceCommit)) {
    throw new Error('Desktop Nightly cutover source SHA does not match the exact build source');
  }
  return { bridge: true, previousVersion };
}

function nightlyArtifactNames(version) {
  return {
    macZip: `Maka-${version}-mac-arm64.zip`,
    macDmg: `Maka-${version}-mac-arm64.dmg`,
    windowsExe: `Maka-${version}-win-x64.exe`,
    windowsZip: `Maka-${version}-win-x64.zip`,
  };
}

function githubNightlyAssetUrl(version, name) {
  return `https://github.com/apache/maka/releases/download/v${encodeURIComponent(version)}/${encodeURIComponent(name)}`;
}

async function writeLegacyBridgeMetadata(source, destination, version) {
  const { parse, stringify } = await import('yaml');
  const metadata = parse(await readFile(source, 'utf8'));
  metadata.path = githubNightlyAssetUrl(version, metadata.path);
  metadata.files = metadata.files.map((file) => ({
    ...file,
    url: githubNightlyAssetUrl(version, file.url),
  }));
  await writeFile(destination, stringify(metadata), 'utf8');
}

function nightlyIndex(version, sourceCommit, names) {
  return `<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><meta name="viewport" content="width=device-width"><title>Maka Desktop Nightly</title></head>
<body>
<main>
<h1>Maka Desktop Nightly</h1>
<p><strong>Desktop Nightly is a developer snapshot, not an Apache release.</strong> It may be unstable and its files are temporary.</p>
<p>Version <code>${version}</code>, built from source commit <code>${sourceCommit}</code>.</p>
<ul>
<li><a href="${githubNightlyAssetUrl(version, names.macDmg)}">Download for macOS arm64</a></li>
<li><a href="${githubNightlyAssetUrl(version, names.windowsExe)}">Download for Windows x64</a> (unsigned preview)</li>
</ul>
<p>Installed Nightly builds update automatically from this channel.</p>
</main>
</body>
</html>
`;
}

export async function stageDesktopNightly({
  inputDirectory,
  outputDirectory,
  version,
  sourceCommit,
}) {
  const productManifest = JSON.parse(await readFile(join(repoRoot, 'package.json'), 'utf8'));
  assertDesktopNightlyVersion(version, productManifest.version);
  if (typeof sourceCommit !== 'string' || !/^[0-9a-f]{40}$/u.test(sourceCommit)) {
    throw new Error('Desktop Nightly requires an exact source commit');
  }
  const names = nightlyArtifactNames(version);
  const payloads = [
    names.macDmg,
    names.macZip,
    `${names.macZip}.blockmap`,
    names.windowsExe,
    `${names.windowsExe}.blockmap`,
    names.windowsZip,
  ];
  const metadataNames = ['dev-mac.yml', 'dev.yml'];
  const expected = [...payloads, ...metadataNames].sort();
  const actual = (await readdir(inputDirectory)).sort();
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `Desktop Nightly input is ${JSON.stringify(actual)}, expected ${JSON.stringify(expected)}`,
    );
  }

  await Promise.all([
    verifyDesktopUpdateArtifacts({
      directory: inputDirectory,
      metadataName: 'dev-mac.yml',
      version,
      artifactName: names.macZip,
    }),
    verifyDesktopUpdateArtifacts({
      directory: inputDirectory,
      metadataName: 'dev.yml',
      version,
      artifactName: names.windowsExe,
    }),
  ]);

  await rm(outputDirectory, { recursive: true, force: true });
  const releaseDirectory = join(outputDirectory, 'release');
  const bridgeFeedDirectory = join(outputDirectory, 'bridge', 'feed');
  const bridgeCompletionDirectory = join(outputDirectory, 'bridge', 'completion');
  await Promise.all([
    mkdir(releaseDirectory, { recursive: true }),
    mkdir(bridgeFeedDirectory, { recursive: true }),
    mkdir(bridgeCompletionDirectory, { recursive: true }),
  ]);
  await Promise.all(
    [...payloads, ...metadataNames].map(async (name) => {
      const source = join(inputDirectory, name);
      const info = await stat(source);
      if (!info.isFile()) throw new Error(`Desktop Nightly payload is not a file: ${source}`);
      await copyFile(source, join(releaseDirectory, name));
    }),
  );
  await Promise.all([
    writeLegacyBridgeMetadata(
      join(inputDirectory, 'dev-mac.yml'),
      join(bridgeFeedDirectory, 'latest-mac.yml'),
      version,
    ),
    writeLegacyBridgeMetadata(
      join(inputDirectory, 'dev.yml'),
      join(bridgeFeedDirectory, 'latest.yml'),
      version,
    ),
  ]);
  await writeFile(
    join(bridgeFeedDirectory, 'index.html'),
    nightlyIndex(version, sourceCommit, names),
  );
  await writeFile(
    join(bridgeCompletionDirectory, 'github-cutover.json'),
    `${JSON.stringify({ schemaVersion: 1, authority: 'github-releases', version })}\n`,
  );
}

export async function addDesktopNightlyAttestation({ outputDirectory, version, bundlePath }) {
  const productManifest = JSON.parse(await readFile(join(repoRoot, 'package.json'), 'utf8'));
  assertDesktopNightlyVersion(version, productManifest.version);
  const details = await stat(bundlePath);
  if (!details.isFile() || details.size === 0) {
    throw new Error('Desktop Nightly attestation must be a non-empty regular file');
  }
  const name = `Maka-${version}-attestation.sigstore.json`;
  const legacyDirectory = join(outputDirectory, 'bridge', 'versions', version);
  await mkdir(legacyDirectory, { recursive: true });
  await Promise.all([
    copyFile(bundlePath, join(outputDirectory, 'release', name)),
    copyFile(bundlePath, join(legacyDirectory, name)),
  ]);
  return name;
}

async function main(args) {
  const [command, ...rest] = args;
  if (command === 'stage' && rest.length === 4) {
    const [inputDirectory, outputDirectory, version, sourceCommit] = rest;
    await stageDesktopNightly({
      inputDirectory,
      outputDirectory,
      version,
      sourceCommit,
    });
    return;
  }
  if (command === 'add-attestation' && rest.length === 3) {
    const [outputDirectory, version, bundlePath] = rest;
    await addDesktopNightlyAttestation({ outputDirectory, version, bundlePath });
    return;
  }
  if (command === 'resolve-cutover' && rest.length === 4) {
    const [feedDirectory, releasesPath, candidateVersion, sourceCommit] = rest;
    const productManifest = JSON.parse(await readFile(join(repoRoot, 'package.json'), 'utf8'));
    const releases = JSON.parse(await readFile(releasesPath, 'utf8'));
    const result = await resolveDesktopNightlyCutover({
      candidateVersion,
      cutoverSourceCommit: process.env.DESKTOP_NIGHTLY_GITHUB_CUTOVER_SOURCE_SHA ?? '',
      feedDirectory,
      productVersion: productManifest.version,
      releases,
      sourceCommit,
    });
    if (!process.env.GITHUB_OUTPUT) {
      throw new Error('resolve-cutover requires GITHUB_OUTPUT');
    }
    await appendFile(
      process.env.GITHUB_OUTPUT,
      `bridge=${String(result.bridge)}\nprevious_version=${result.previousVersion ?? ''}\n`,
      'utf8',
    );
    return;
  }
  if (command === 'assert-feed-advance' && rest.length === 2) {
    const [directory, candidateVersion] = rest;
    const productManifest = JSON.parse(await readFile(join(repoRoot, 'package.json'), 'utf8'));
    await assertDesktopNightlyFeedAdvance({
      directory,
      candidateVersion,
      productVersion: productManifest.version,
    });
    return;
  }
  throw new Error(
    'usage: desktop-nightly.mjs stage <input-directory> <output-directory> <version> <source-commit> | add-attestation <output-directory> <version> <bundle-path> | resolve-cutover <feed-directory> <releases-json> <candidate-version> <source-commit> | assert-feed-advance <feed-directory> <candidate-version>',
  );
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  await main(process.argv.slice(2));
}
