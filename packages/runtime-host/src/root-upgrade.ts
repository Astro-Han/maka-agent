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
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { chmod, cp, lstat, mkdir, readdir, rename, rm, stat, writeFile } from 'node:fs/promises';
import { userInfo } from 'node:os';
import { dirname, isAbsolute, join } from 'node:path';
import { z } from 'zod';
import {
  resolveStorageRoot,
  resolveRootOwnershipNamespace,
  withStorageRootUpgrade,
  inspectStorageRootFormat,
  StorageRootAuthorityError,
  type StorageRootUpgradeSession,
  type StorageRootCapability,
  repairStorageRootAfterRemount,
} from '@maka/storage/root-authority';
import {
  hardenDirectory,
  syncDirectoryChain,
  syncFile,
  readStableBoundedFile,
} from '@maka/storage/stable-storage';
import { readAccessCredentialFile, ACCESS_FILE_NAME } from './server/access-credential-store.js';
import { HostPluginCompositionStore } from './server/plugin-composition-store.js';
import {
  decodeRuntimeHostManagedDeploymentAuthorityRecord,
  decodeRuntimeHostManagedDeploymentConfig,
  type RuntimeHostManagedDeploymentAuthorityRecord,
  type RuntimeHostManagedDeploymentConfig,
  locateRuntimeHostManagedRoot,
  type RuntimeHostManagedDeploymentAuthorityOptions,
} from './operator/managed-deployment.js';
import { resolveRuntimeHostNpmDeploymentLayout } from './operator/update-package-evidence.js';

const source = z.string().refine(isAbsolute).nullable();
const planSchema = z
  .object({
    data: source,
    deployment: source,
    locator: source,
    locks: z.array(z.string().refine(isAbsolute)).max(4),
    targetDeployment: z.unknown().optional(),
  })
  .strict();
type UpgradePlan = z.infer<typeof planSchema>;
const COMPLETION = '.upgrade-complete.json';

/** The only startup entry that can initialize or resume the Host's root format. */
export interface RuntimeHostRootUpgradeOptions {
  /** The existing installer stages its package before the format fence. */
  readonly prepareDeployment?: (
    current: RuntimeHostManagedDeploymentConfig,
  ) => Promise<RuntimeHostManagedDeploymentConfig>;
  readonly retireDeployment?: (current: RuntimeHostManagedDeploymentConfig) => Promise<void>;
}

export async function prepareRuntimeHostRoot(
  path: string,
  options: RuntimeHostRootUpgradeOptions = {},
): Promise<StorageRootCapability<'interactive'>> {
  try {
    return await resolveStorageRoot({ path, kind: 'interactive' });
  } catch (error) {
    if (
      !(error instanceof StorageRootAuthorityError) ||
      error.code !== 'legacy_root_requires_migration'
    )
      throw error;
  }
  await withStorageRootUpgrade(path, async (session) => {
    let plan: UpgradePlan;
    if (session.upgrade) {
      plan = planSchema.parse(session.upgrade.payload);
      // The durable fence rejects old code, but a writer admitted before it
      // was published still has to release its actual OS lock.
      for (const lock of plan.locks) await lockIfParentPresent(session, lock);
    } else {
      plan = await inspectLegacySources(session);
      const rootStat = await stat(session.canonicalPath);
      if (
        process.platform !== 'win32' &&
        typeof process.getuid === 'function' &&
        rootStat.uid !== process.getuid()
      ) {
        throw new Error('Upgrade must run as the account that owns the legacy root');
      }
      const current = await validateDeploymentSource(
        plan.deployment,
        session.rootId,
        session.canonicalPath,
      );
      if (current) {
        const active = current.state === 'active' ? current : current.to;
        if (!active)
          throw new Error('Finish the legacy deployment retirement before upgrading its root');
        const target =
          current.state === 'active' && options.prepareDeployment
            ? decodeRuntimeHostManagedDeploymentConfig(await options.prepareDeployment(current))
            : active;
        if (
          target.root.id !== session.rootId ||
          target.root.path !== session.canonicalPath ||
          target.deploymentId !== active.deploymentId
        )
          throw new Error('Prepared deployment targets another root or installation');
        await assertCompatibleDeployment(target);
        if (JSON.stringify(target) !== JSON.stringify(active)) {
          if (target.configRevision <= active.configRevision)
            throw new Error('Prepared deployment must advance its revision');
          plan.targetDeployment = target;
        }
        await options.retireDeployment?.(active);
      }
      for (const lock of plan.locks) {
        // On first admission, create writable legacy lock directories so a
        // simultaneous old startup cannot create a different, unlocked path.
        // An inaccessible absent parent cannot admit an old writer either.
        try {
          await mkdir(dirname(lock), { recursive: true, mode: 0o700 });
        } catch (error) {
          const code = (error as NodeJS.ErrnoException).code;
          if (
            (code === 'EACCES' || code === 'EROFS' || code === 'ENOENT') &&
            !(await present(dirname(lock)))
          )
            continue;
          throw error;
        }
        await session.acquireLegacyLock(lock);
      }
      const lockedSources = await inspectLegacySources(session);
      const lockedDeployment = await validateDeploymentSource(
        lockedSources.deployment,
        session.rootId,
        session.canonicalPath,
      );
      if (JSON.stringify(current) !== JSON.stringify(lockedDeployment))
        throw new Error('Legacy deployment changed while preparing its upgrade');
      plan = {
        ...lockedSources,
        ...(plan.targetDeployment ? { targetDeployment: plan.targetDeployment } : {}),
      };
      await session.begin(plan);
    }
    const transaction = session.upgrade!;
    const authority = resolveRootOwnershipNamespace(session.canonicalPath);
    const state = join(authority, 'state');
    const staging = join(authority, `upgrade-${transaction.id}`);
    if (!(await completedSnapshot(state, transaction.id))) {
      if (await present(state))
        throw new Error('Existing Host state is not the snapshot owned by this upgrade');
      if (!(await completedSnapshot(staging, transaction.id))) {
        // Only incomplete staging owned by this transaction is disposable.
        await rm(staging, { recursive: true, force: true });
        await hardenDirectory(staging);
        for (const [name, path] of [
          ['data', plan.data],
          ['deployment', plan.deployment],
        ] as const) {
          const target = join(staging, name);
          if (path === null) await hardenDirectory(target);
          else {
            // cp must fail if a previously-present source is now missing.
            await cp(path, target, {
              recursive: true,
              dereference: false,
              filter: (entry) =>
                entry !== join(path, 'owner.lock') &&
                entry !== join(path, '.maka-artifact-writer.lock'),
            });
          }
        }
        if (plan.targetDeployment) {
          const current = await validateDeploymentSource(
            join(staging, 'deployment'),
            session.rootId,
            session.canonicalPath,
          );
          if (current?.state !== 'active')
            throw new Error('Prepared upgrade no longer has its source deployment');
          const transition = decodeRuntimeHostManagedDeploymentAuthorityRecord({
            schemaVersion: 1,
            state: 'transition',
            transactionId: transaction.id,
            operation: 'update',
            recovery: 'complete_to',
            root: current.root,
            from: current,
            to: decodeRuntimeHostManagedDeploymentConfig(plan.targetDeployment),
          });
          await writeFile(
            join(staging, 'deployment', 'runtime-host-deployment.json'),
            JSON.stringify(transition),
            { mode: 0o600 },
          );
        }
        await readAccessCredentialFile(join(staging, 'data', ACCESS_FILE_NAME));
        await new HostPluginCompositionStore(join(staging, 'data')).read();
        await validateDeploymentSource(
          join(staging, 'deployment'),
          session.rootId,
          session.canonicalPath,
        );
        await syncTree(staging);
        await writeFile(
          join(staging, COMPLETION),
          JSON.stringify({ migrationId: transaction.id }),
          { flag: 'wx', mode: 0o600 },
        );
        await syncFile(join(staging, COMPLETION));
        await syncDirectoryChain(staging, authority);
      }
      await rename(staging, state);
      await syncDirectoryChain(authority, session.canonicalPath);
    }
    // Only the locator is account-side. Its source is captured before the
    // durable fence; recovery never consults the current account environment.
    const committedDeployment = await validateDeploymentSource(
      join(state, 'deployment'),
      session.rootId,
      session.canonicalPath,
    );
    if (committedDeployment) {
      const target =
        committedDeployment.state === 'active' ? committedDeployment : committedDeployment.to;
      if (!target) throw new Error('Upgraded deployment has no compatible recovery target');
      await assertCompatibleDeployment(target);
    }
    if (plan.locator) {
      // The locator is a projection of the completed root snapshot. Recreate its
      // directory without recreating or rereading any legacy data source.
      let locatorBoundary = dirname(plan.locator);
      while (!(await present(locatorBoundary))) locatorBoundary = dirname(locatorBoundary);
      await hardenDirectory(dirname(plan.locator));
      const temporary = `${plan.locator}.${transaction.id}.tmp`;
      await writeFile(
        temporary,
        JSON.stringify({ rootId: session.rootId, rootPath: session.canonicalPath }),
        { mode: 0o600 },
      );
      await syncFile(temporary);
      await rename(temporary, plan.locator);
      await syncDirectoryChain(dirname(plan.locator), locatorBoundary);
    }
    await session.commit();
  });
  return resolveStorageRoot({ path, kind: 'interactive' });
}

export async function prepareRuntimeHostManagedRoot(
  rootId: string,
  authority: RuntimeHostManagedDeploymentAuthorityOptions = {},
): Promise<void> {
  const location = await locateRuntimeHostManagedRoot(rootId, authority);
  if (!location) return;
  if (authority.repairRootAfterRemount)
    await repairStorageRootAfterRemount({
      path: location.rootPath,
      kind: 'interactive',
      expectedRootId: rootId,
    });
  const identity = await inspectStorageRootFormat(location.rootPath);
  if (identity.rootId !== rootId) throw new Error('Managed locator points to another root');
  await prepareRuntimeHostRoot(location.rootPath);
}

async function assertCompatibleDeployment(
  config: RuntimeHostManagedDeploymentConfig,
): Promise<void> {
  const layout = resolveRuntimeHostNpmDeploymentLayout(
    config.deploymentRoot,
    config.launch.package.integrity,
  );
  const authority = join(
    layout.packageRoot,
    'node_modules',
    '@maka',
    'storage',
    'dist',
    'root-authority.js',
  );
  // Ask the exact prepared package, not the invoking Client's version. Importing
  // storage authority creates no root and makes no model/network calls.
  const { stdout } = await promisify(execFile)(
    config.launch.nodePath,
    [
      '--input-type=module',
      '-e',
      'const m=await import((await import("node:url")).pathToFileURL(process.argv[1]).href); process.stdout.write(String(m.STORAGE_ROOT_MARKER_SCHEMA_VERSION));',
      authority,
    ],
    { timeout: 15_000, maxBuffer: 4096 },
  );
  if (stdout !== '2')
    throw new Error('The prepared managed package cannot open the upgraded State Root');
}

async function inspectLegacySources(session: StorageRootUpgradeSession): Promise<UpgradePlan> {
  const home = userInfo().homedir;
  if (!isAbsolute(home)) throw new Error('Legacy account home must be absolute');
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
  const control = join(cache, 'runtime-hosts', session.rootId);
  const deployment = join(durable, 'runtime-host-deployments', session.rootId);
  const identity = await stat(session.canonicalPath, { bigint: true });
  const bootstrapId = createHash('sha256').update(`${identity.dev}:${identity.ino}`).digest('hex');
  const locks = [
    join(durable, 'state-root-owners', `${session.rootId}.lock`),
    join(control, 'owner.lock'),
    join(cache, 'runtime-hosts', 'artifact-writer-bootstrap', `${bootstrapId}.lock`),
    join(control, '.maka-artifact-writer.lock'),
  ];
  const hasDeployment = await present(deployment);
  return {
    data: (await present(control)) ? control : null,
    deployment: hasDeployment ? deployment : null,
    locator: hasDeployment ? join(deployment, 'root-location.json') : null,
    locks,
  };
}

async function lockIfParentPresent(
  session: StorageRootUpgradeSession,
  path: string,
): Promise<void> {
  if (await present(dirname(path))) await session.acquireLegacyLock(path);
}

async function present(path: string): Promise<boolean> {
  try {
    const entry = await lstat(path);
    if (!entry.isDirectory()) throw new Error(`Upgrade source is not a directory: ${path}`);
    return true;
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return false;
    throw error;
  }
}

async function completedSnapshot(path: string, id: string): Promise<boolean> {
  try {
    if (!(await present(path))) return false;
    const bytes = await readStableBoundedFile({
      path: join(path, COMPLETION),
      maxBytes: 256,
      invalidFile: () => new Error('Invalid upgrade completion record'),
    });
    const value = JSON.parse(bytes.toString('utf8'));
    if (value.migrationId !== id || Object.keys(value).length !== 1)
      throw new Error('Snapshot belongs to another upgrade');
    return true;
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return false;
    throw error;
  }
}

async function validateDeploymentSource(
  path: string | null,
  rootId: string,
  rootPath: string,
): Promise<RuntimeHostManagedDeploymentAuthorityRecord | undefined> {
  if (!path) return undefined;
  let contents: Buffer;
  try {
    contents = await readStableBoundedFile({
      path: join(path, 'runtime-host-deployment.json'),
      maxBytes: 512 * 1024,
      invalidFile: () => new Error('Invalid legacy deployment'),
    });
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return undefined;
    throw error;
  }
  const record = decodeRuntimeHostManagedDeploymentAuthorityRecord(
    JSON.parse(contents.toString('utf8')),
  );
  if (record.root.id !== rootId || record.root.path !== rootPath)
    throw new Error('Legacy deployment does not belong to the upgrading root');
  return record;
}

async function syncTree(path: string): Promise<void> {
  await chmod(path, 0o700);
  for (const entry of await readdir(path, { withFileTypes: true })) {
    const child = join(path, entry.name);
    if (entry.isDirectory()) await syncTree(child);
    else if (entry.isFile()) {
      await chmod(child, ((await lstat(child)).mode & 0o700) | 0o600);
      await syncFile(child);
    } else throw new Error(`Unsupported entry in legacy Host data: ${child}`);
  }
  await syncDirectoryChain(path, path);
}
