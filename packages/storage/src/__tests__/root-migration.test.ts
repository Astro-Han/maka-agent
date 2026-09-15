/*
 * Licensed to the Apache Software Foundation (ASF) under one
 * or more contributor license agreements. See the NOTICE file
 * distributed with this work for additional information
 * regarding copyright ownership. The ASF licenses this file
 * to you under the Apache License, Version 2.0 (the
 * "License"); you may not use this file except in compliance
 * with the License. You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing,
 * software distributed under the License is distributed on an
 * "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
 * KIND, either express or implied. See the License for the
 * specific language governing permissions and limitations
 * under the License.
 */

import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import { mkdir, mkdtemp, open, readFile, rm, writeFile } from 'node:fs/promises';
import { syncBuiltinESMExports } from 'node:module';
import os from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { tryLock } from 'fs-native-extensions';
import {
  resolveStorageRoot,
  tryAcquireStateRootOwner,
  STORAGE_ROOT_MARKER_FILE,
  resolveRootOwnershipNamespace,
  resolveRootHostDataDirectory,
} from '../root-authority.js';

test('fresh roots acquire and reopen without an account home', async (t) => {
  const root = await mkdtemp(join(os.tmpdir(), 'maka-root-no-home-'));
  t.mock.method(os, 'userInfo', () => {
    throw new Error('Account home unavailable');
  });
  syncBuiltinESMExports();
  try {
    const capability = await resolveStorageRoot({ path: root, kind: 'interactive' });
    const first = await tryAcquireStateRootOwner(capability);
    assert.ok(first);
    await writeFile(join(first.hostDataDirectory, 'settings.json'), '{"kept":true}');
    await rm(first.controlDirectory, { recursive: true, force: true });
    assert.equal(await tryAcquireStateRootOwner(capability), undefined);
    await first.close();
    const second = await tryAcquireStateRootOwner(
      await resolveStorageRoot({ path: root, kind: 'interactive' }),
    );
    assert.ok(second);
    assert.equal(
      await readFile(join(second.hostDataDirectory, 'settings.json'), 'utf8'),
      '{"kept":true}',
    );
    await second.close();
  } finally {
    t.mock.restoreAll();
    syncBuiltinESMExports();
    await rm(root, { recursive: true, force: true });
  }
});

test('legacy cutover fences old owners, retries staging, and preserves durable data and deployment', async (t) => {
  const base = await mkdtemp(join(os.tmpdir(), 'maka-root-migrate-'));
  const root = join(base, 'state');
  const home = join(base, 'home');
  await mkdir(home);
  const info = os.userInfo();
  t.mock.method(os, 'userInfo', () => ({ ...info, homedir: home }));
  syncBuiltinESMExports();
  let legacy: Awaited<ReturnType<typeof open>> | undefined;
  try {
    const capability = await resolveStorageRoot({ path: root, kind: 'interactive' });
    const markerPath = join(root, STORAGE_ROOT_MARKER_FILE);
    const marker = JSON.parse(await readFile(markerPath, 'utf8'));
    await writeFile(markerPath, JSON.stringify({ ...marker, schemaVersion: 1 }));
    const cache =
      process.platform === 'darwin'
        ? join(home, 'Library', 'Caches', 'Maka')
        : process.platform === 'win32'
          ? join(home, 'AppData', 'Local', 'Maka')
          : join(home, '.cache', 'maka');
    const durable =
      process.platform === 'darwin'
        ? join(home, 'Library', 'Application Support', 'Maka')
        : process.platform === 'win32'
          ? join(home, 'AppData', 'Local', 'Maka')
          : join(home, '.local', 'share', 'Maka');
    const control = join(cache, 'runtime-hosts', capability.rootId);
    await mkdir(control, { recursive: true, mode: 0o700 });
    await writeFile(join(control, 'plugin-state.json'), 'saved-plugin-state');
    await writeFile(join(control, 'runtime-host-access.json'), 'saved-access-state');
    const deployment = join(durable, 'runtime-host-deployments', capability.rootId);
    await mkdir(deployment, { recursive: true, mode: 0o700 });
    await writeFile(join(deployment, 'runtime-host-deployment.json'), 'saved-deployment');
    legacy = await open(join(control, 'owner.lock'), 'a+', 0o600);
    assert.equal(tryLock(legacy.fd), true);
    await assert.rejects(resolveStorageRoot({ path: root, kind: 'interactive' }), {
      code: 'root_migration_busy',
    });
    assert.equal(JSON.parse(await readFile(markerPath, 'utf8')).schemaVersion, 1);
    await legacy.close();
    legacy = undefined;
    const rename = fs.rename;
    const publication = t.mock.method(
      fs,
      'rename',
      async (...[source, destination]: Parameters<typeof fs.rename>) => {
        if (destination === join(capability.canonicalPath, STORAGE_ROOT_MARKER_FILE)) {
          throw Object.assign(new Error('Interrupted cutover'), { code: 'EIO' });
        }
        return rename(source, destination);
      },
    );
    syncBuiltinESMExports();
    await assert.rejects(resolveStorageRoot({ path: root, kind: 'interactive' }));
    assert.equal(JSON.parse(await readFile(markerPath, 'utf8')).schemaVersion, 1);
    assert.equal(await readFile(join(control, 'plugin-state.json'), 'utf8'), 'saved-plugin-state');
    publication.mock.restore();
    syncBuiltinESMExports();
    const staging = join(resolveRootOwnershipNamespace(root), 'migration-data');
    await mkdir(staging, { recursive: true });
    await writeFile(join(staging, 'incomplete'), 'interrupted-copy');
    const migrated = await resolveStorageRoot({ path: root, kind: 'interactive' });
    assert.equal(migrated.rootId, capability.rootId);
    assert.equal(JSON.parse(await readFile(markerPath, 'utf8')).schemaVersion, 2);
    assert.equal(
      await readFile(join(resolveRootHostDataDirectory(root), 'plugin-state.json'), 'utf8'),
      'saved-plugin-state',
    );
    assert.equal(
      await readFile(join(resolveRootHostDataDirectory(root), 'runtime-host-access.json'), 'utf8'),
      'saved-access-state',
    );
    assert.equal(
      await readFile(
        join(resolveRootOwnershipNamespace(root), 'deployment', 'runtime-host-deployment.json'),
        'utf8',
      ),
      'saved-deployment',
    );
    assert.equal(await readFile(join(control, 'plugin-state.json'), 'utf8'), 'saved-plugin-state');
    assert.deepEqual(JSON.parse(await readFile(join(deployment, 'root-location.json'), 'utf8')), {
      rootId: capability.rootId,
      rootPath: capability.canonicalPath,
    });
    await assert.rejects(readFile(join(resolveRootHostDataDirectory(root), 'incomplete')), {
      code: 'ENOENT',
    });
  } finally {
    await legacy?.close();
    t.mock.restoreAll();
    syncBuiltinESMExports();
    await rm(base, { recursive: true, force: true });
  }
});
