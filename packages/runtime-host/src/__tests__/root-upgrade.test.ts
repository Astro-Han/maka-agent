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

/// <reference path="../../../storage/src/fs-native-extensions.d.ts" />
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
  resolveExistingStorageRoot,
  resolveRootHostDataDirectory,
  STORAGE_ROOT_MARKER_FILE,
} from '@maka/storage/root-authority';
import { prepareRuntimeHostRoot } from '../root-upgrade.js';
import {
  createAccessCredentialFile,
  writeAccessCredentialFile,
  ACCESS_FILE_NAME,
} from '../server/access-credential-store.js';

for (const interruptedAt of ['takeover', 'copy', 'snapshot', 'ready']) {
  test(`upgrade automatically resumes after interruption at ${interruptedAt}`, async (t) => {
    const base = await mkdtemp(join(os.tmpdir(), 'maka-upgrade-'));
    const home = join(base, 'home');
    await mkdir(home);
    const info = os.userInfo();
    t.mock.method(os, 'userInfo', () => ({ ...info, homedir: home }));
    syncBuiltinESMExports();
    try {
      const root = join(base, 'state');
      const capability = await resolveStorageRoot({ path: root, kind: 'interactive' });
      const markerPath = join(capability.canonicalPath, STORAGE_ROOT_MARKER_FILE);
      const original = JSON.parse(await readFile(markerPath, 'utf8'));
      await writeFile(markerPath, JSON.stringify({ ...original, schemaVersion: 1 }));
      const cache =
        process.platform === 'darwin'
          ? join(home, 'Library', 'Caches', 'Maka')
          : process.platform === 'win32'
            ? join(home, 'AppData', 'Local', 'Maka')
            : join(home, '.cache', 'maka');
      const source = join(cache, 'runtime-hosts', capability.rootId);
      await mkdir(source, { recursive: true, mode: 0o700 });
      await writeAccessCredentialFile(
        join(source, ACCESS_FILE_NAME),
        createAccessCredentialFile([]),
      );
      await writeFile(join(source, 'plugin-state.json'), '{"value":"durable"}');
      const held = await open(join(source, 'owner.lock'), 'a+', 0o600);
      try {
        assert.ok(tryLock(held.fd));
        await assert.rejects(prepareRuntimeHostRoot(root), { code: 'root_migration_busy' });
        assert.equal(JSON.parse(await readFile(markerPath, 'utf8')).schemaVersion, 1);
      } finally {
        await held.close();
      }
      const rename = fs.rename;
      const failCommit = t.mock.method(
        fs,
        'rename',
        async (...[from, to]: Parameters<typeof fs.rename>) => {
          if (to === markerPath) {
            const candidate = JSON.parse(await readFile(from, 'utf8'));
            if (
              (interruptedAt === 'ready' && !candidate.upgrade) ||
              (interruptedAt === 'takeover' && candidate.upgrade)
            )
              throw Object.assign(new Error('commit interrupted'), { code: 'EIO' });
          }
          if (
            interruptedAt === 'snapshot' &&
            to === join(capability.canonicalPath, '.maka-host', 'state')
          )
            throw Object.assign(new Error('snapshot interrupted'), { code: 'EIO' });
          return rename(from, to);
        },
      );
      const copy = fs.cp;
      const failCopy = t.mock.method(fs, 'cp', async (...args: Parameters<typeof fs.cp>) => {
        if (interruptedAt === 'copy')
          throw Object.assign(new Error('copy interrupted'), { code: 'EIO' });
        return copy(...args);
      });
      syncBuiltinESMExports();
      await assert.rejects(prepareRuntimeHostRoot(root), { code: 'EIO' });
      await assert.rejects(
        resolveExistingStorageRoot({
          path: root,
          kind: 'interactive',
          expectedRootId: capability.rootId,
        }),
        { code: 'legacy_root_requires_migration' },
      );
      assert.equal(
        Boolean(JSON.parse(await readFile(markerPath, 'utf8')).upgrade),
        interruptedAt !== 'takeover',
      );
      failCommit.mock.restore();
      failCopy.mock.restore();
      if (interruptedAt === 'snapshot' || interruptedAt === 'ready')
        await rm(source, { recursive: true, force: true });
      if (interruptedAt !== 'takeover')
        t.mock.method(os, 'userInfo', () => {
          throw new Error('account unavailable after interruption');
        });
      syncBuiltinESMExports();
      if (interruptedAt === 'copy') {
        await rm(source, { recursive: true, force: true });
        await assert.rejects(prepareRuntimeHostRoot(root), { code: 'ENOENT' });
        assert.ok(JSON.parse(await readFile(markerPath, 'utf8')).upgrade);
        await mkdir(source, { recursive: true, mode: 0o700 });
        await writeAccessCredentialFile(
          join(source, ACCESS_FILE_NAME),
          createAccessCredentialFile([]),
        );
        await writeFile(join(source, 'plugin-state.json'), '{"value":"durable"}');
      }
      const recovered = await prepareRuntimeHostRoot(root);
      assert.equal(recovered.rootId, capability.rootId);
      assert.equal(
        await readFile(join(resolveRootHostDataDirectory(root), 'plugin-state.json'), 'utf8'),
        '{"value":"durable"}',
      );
      assert.deepEqual(JSON.parse(await readFile(markerPath, 'utf8')), original);
    } finally {
      t.mock.restoreAll();
      syncBuiltinESMExports();
      await rm(base, { recursive: true, force: true });
    }
  });
}

test('an uninitialized legacy root upgrades with an inaccessible absent account home', async (t) => {
  const base = await mkdtemp(join(os.tmpdir(), 'maka-upgrade-no-home-'));
  const root = join(base, 'state');
  const missingHome = join(base, 'not-created');
  try {
    const capability = await resolveStorageRoot({ path: root, kind: 'interactive' });
    const path = join(root, STORAGE_ROOT_MARKER_FILE);
    const marker = JSON.parse(await readFile(path, 'utf8'));
    await writeFile(path, JSON.stringify({ ...marker, schemaVersion: 1 }));
    const info = os.userInfo();
    t.mock.method(os, 'userInfo', () => ({ ...info, homedir: missingHome }));
    const mkdir = fs.mkdir;
    t.mock.method(fs, 'mkdir', async (...args: Parameters<typeof fs.mkdir>) => {
      if (String(args[0]).startsWith(missingHome))
        throw Object.assign(new Error('account home is not writable'), { code: 'EACCES' });
      return mkdir(...args);
    });
    syncBuiltinESMExports();
    await writeFile(join(root, 'business-state.json'), '{"retained":true}');
    await assert.rejects(prepareRuntimeHostRoot(root), /persisted state.*account data/);
    assert.equal(JSON.parse(await readFile(path, 'utf8')).schemaVersion, 1);
    await rm(join(root, 'business-state.json'));
    const upgraded = await prepareRuntimeHostRoot(root);
    assert.equal(upgraded.rootId, capability.rootId);
    await assert.rejects(fs.stat(missingHome), { code: 'ENOENT' });
    assert.deepEqual(await fs.readdir(resolveRootHostDataDirectory(root)), []);
  } finally {
    t.mock.restoreAll();
    syncBuiltinESMExports();
    await rm(base, { recursive: true, force: true });
  }
});
