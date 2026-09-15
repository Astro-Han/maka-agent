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
import childProcess from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { EventEmitter } from 'node:events';
import fs from 'node:fs/promises';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { syncBuiltinESMExports } from 'node:module';
import os from 'node:os';
import { dirname, join } from 'node:path';
import { PassThrough } from 'node:stream';
import test from 'node:test';
import { resolveStorageRoot, STORAGE_ROOT_MARKER_FILE } from '@maka/storage/root-authority';
import {
  claimRuntimeHostManagedDeployment,
  prepareRuntimeHostRoot,
  resolveRuntimeHostManagedDeploymentAuthorityRoot,
  resolveRuntimeHostNpmDeploymentLayout,
  type RuntimeHostManagedDeploymentConfig,
} from '@maka/runtime-host/operator';
import { resolveManagedRuntimeHostUpdateSelection } from '../runtime-host-update-discovery.js';
import {
  runManagedRuntimeHostUpdateCli,
  runManagedRuntimeHostSelectedUpdateCli,
  type RuntimeHostUpdateFrame,
} from '../runtime-host-update-command.js';
import { resolveRecoverableRuntimeHostManagedDeployment } from '../runtime-host-lifecycle-transaction.js';

for (const failure of ['activation', 'locator']) {
  test(`successor CLI selects before takeover and resumes after ${failure} failure`, async (t) => {
    const base = await mkdtemp(join(os.tmpdir(), 'maka-successor-upgrade-'));
    const home = join(base, 'home');
    await mkdir(home);
    const account = os.userInfo();
    t.mock.method(os, 'userInfo', () => ({ ...account, homedir: home }));
    syncBuiltinESMExports();
    t.after(async () => {
      t.mock.restoreAll();
      syncBuiltinESMExports();
      await rm(base, { recursive: true, force: true, maxRetries: 10 });
    });
    const root = await resolveStorageRoot({ path: join(base, 'root'), kind: 'interactive' });
    const current: RuntimeHostManagedDeploymentConfig = {
      schemaVersion: 1,
      state: 'active',
      deploymentId: randomUUID(),
      configRevision: 1,
      deploymentRoot: join(base, 'deployment'),
      root: { id: root.rootId, path: root.canonicalPath },
      projectDirectoryRoots: [],
      launch: {
        kind: 'exact_package',
        nodePath: process.execPath,
        package: {
          kind: 'npm_registry',
          version: '1.2.3',
          integrity: `sha512-${Buffer.alloc(64, 1).toString('base64')}`,
        },
      },
      listeners: {
        localIpc: true,
        websocket: { host: '127.0.0.1', port: 0, path: '/runtime-host' },
      },
      lifecycle: { mode: 'on_demand', availability: 'activation' },
      reconciliation: { trigger: 'manual' },
    };
    await claimRuntimeHostManagedDeployment(root, current);
    const legacyDirectory = join(resolveRuntimeHostManagedDeploymentAuthorityRoot(), root.rootId);
    const legacyRecord = join(legacyDirectory, 'runtime-host-deployment.json');
    await writeFile(legacyRecord, JSON.stringify(current));
    await rm(join(root.canonicalPath, '.maka-host', 'state'), { recursive: true });
    const markerPath = join(root.canonicalPath, STORAGE_ROOT_MARKER_FILE);
    const marker = JSON.parse(await readFile(markerPath, 'utf8'));
    await writeFile(markerPath, JSON.stringify({ ...marker, schemaVersion: 1 }));
    await writeFile(join(root.canonicalPath, 'desktop-settings.json'), '{"retained":true}');
    const target = {
      kind: 'npm_registry' as const,
      version: '1.2.4',
      integrity: `sha512-${Buffer.alloc(64, 2).toString('base64')}`,
    };
    const sourceLayout = resolveRuntimeHostNpmDeploymentLayout(
      current.deploymentRoot,
      current.launch.package.integrity,
    );
    const targetLayout = resolveRuntimeHostNpmDeploymentLayout(
      current.deploymentRoot,
      target.integrity,
    );
    const manifest = JSON.parse(
      await readFile(new URL('../../package.json', import.meta.url), 'utf8'),
    );
    const status = {
      schemaVersion: 1,
      action: 'status',
      service: {
        manager: 'on_demand',
        installed: true,
        enabled: false,
        active: false,
        state: 'stopped',
        pid: null,
        lastExitCode: null,
        installedVersion: current.launch.package.version,
        lifecycle: current.lifecycle,
        reconciliation: current.reconciliation,
        config: {
          schemaVersion: 2,
          managedDeploymentRoot: current.deploymentRoot,
          rootPath: root.canonicalPath,
          projectDirectoryRoots: [],
          websocket: current.listeners.websocket,
          launch: { nodePath: process.execPath, cliPath: sourceLayout.cliPath },
        },
      },
    };
    const eventsFile = join(base, 'source-events.jsonl');
    // The installed old package is an external version boundary. Its fixture
    // refuses calls after takeover and returns metadata, never a current capability.
    await mkdir(dirname(sourceLayout.cliPath), { recursive: true });
    await writeFile(
      join(sourceLayout.packageRoot, 'package.json'),
      JSON.stringify({
        type: 'module',
        name: 'maka-agent',
        version: '1.2.3',
        maka: { managedRuntimeHostUpdateCompatibility: 1 },
      }),
    );
    await writeFile(sourceLayout.cliPath, '');
    const sourcePrelude = `
    import assert from 'node:assert/strict';
    import { readFile, appendFile } from 'node:fs/promises';
    async function observe(kind) {
      assert.equal(JSON.parse(await readFile(${JSON.stringify(markerPath)}, 'utf8')).schemaVersion, 1);
      await appendFile(${JSON.stringify(eventsFile)}, JSON.stringify(kind) + '\\n');
    }
  `;
    await writeFile(
      join(dirname(sourceLayout.cliPath), 'runtime-host-managed-lifecycle-manager.js'),
      `${sourcePrelude}
    export async function manageRuntimeHostManagedLifecycle() { await observe('status'); return ${JSON.stringify(status)}; }
  `,
    );
    await writeFile(
      join(dirname(sourceLayout.cliPath), 'runtime-host-lifecycle-transaction.js'),
      `${sourcePrelude}
    export async function resolveRecoverableRuntimeHostManagedDeployment() {
      await observe('read'); return {kind: 'active', config: JSON.parse(await readFile(${JSON.stringify(legacyRecord)}, 'utf8'))};
    }
    export async function retireRuntimeHostLifecycleOwner(input) {
      await observe('retire');
      if (!input.allowInterruptActiveTasks) return {kind:'active_tasks'};
      return {kind:'retired', owner:{close: async () => observe('close')}};
    }
  `,
    );
    for (const [layout, schema] of [
      [sourceLayout, 1],
      [targetLayout, 2],
    ] as const) {
      const modulePath = join(
        layout.packageRoot,
        'node_modules',
        '@maka',
        'storage',
        'dist',
        'root-authority.js',
      );
      await mkdir(dirname(modulePath), { recursive: true });
      await writeFile(modulePath, `export const STORAGE_ROOT_MARKER_SCHEMA_VERSION = ${schema};`);
    }
    await mkdir(dirname(targetLayout.cliPath), { recursive: true });
    await writeFile(targetLayout.cliPath, '');
    await mkdir(dirname(targetLayout.candidateEntrypoint), { recursive: true });
    await writeFile(targetLayout.candidateEntrypoint, '');
    const spawn = childProcess.spawn;
    t.mock.method(childProcess, 'spawn', (...args: Parameters<typeof childProcess.spawn>) => {
      if (args[0] !== 'npm') return spawn(...args);
      const child = Object.assign(new EventEmitter(), {
        stdout: new PassThrough(),
        kill: () => true,
      });
      queueMicrotask(() => {
        child.stdout.end(
          JSON.stringify({
            version: target.version,
            'dist.integrity': target.integrity,
            'maka.managedRuntimeHostUpdateCompatibility':
              manifest.maka.managedRuntimeHostUpdateCompatibility,
          }),
        );
        child.emit('close', 0);
      });
      return child;
    });
    syncBuiltinESMExports();
    const selection = await resolveManagedRuntimeHostUpdateSelection({
      clientDataRoot: join(base, 'client'),
      defaultRootPath: root.canonicalPath,
      managedRootId: root.rootId,
      selector: { kind: 'exact', version: target.version },
    });
    assert.deepEqual(selection.outcome, {
      kind: 'manual_action',
      reason: 'compatibility_mismatch',
    });
    assert.equal(JSON.parse(await readFile(markerPath, 'utf8')).schemaVersion, 1);
    let rollbacks = 0;
    const frames: RuntimeHostUpdateFrame[] = [];
    const update = (allowInterruptActiveTasks: boolean, allowManualUpdate = true) =>
      runManagedRuntimeHostSelectedUpdateCli(
        {
          json: false,
          framed: true,
          clientDataRoot: join(base, 'client'),
          defaultRootPath: root.canonicalPath,
          managedRootId: root.rootId,
          selector: { kind: 'exact', version: target.version },
          expectedTarget: {
            serviceId: root.rootId,
            rootId: root.rootId,
            rootPath: root.canonicalPath,
            deploymentId: current.deploymentId,
          },
          allowInterruptActiveTasks,
          allowManualUpdate,
        },
        {
          withPackage: async (_candidate, apply) => apply(targetLayout.packageRoot),
          update: (options, overrides, sink) =>
            runManagedRuntimeHostUpdateCli(
              options,
              {
                ...overrides,
                prepareDeployment: async () => {
                  assert.equal(JSON.parse(await readFile(markerPath, 'utf8')).schemaVersion, 1);
                  return {
                    version: target.version,
                    root: current.deploymentRoot,
                    cliPath: targetLayout.cliPath,
                    activate: async () => {},
                    cleanup: async () => {},
                    rollback: async () => {
                      rollbacks++;
                    },
                  };
                },
                activateDesired: async () => {
                  throw new Error('injected activation failure');
                },
                prunePackages: async () => {},
              },
              sink,
            ),
        },
        (frame) => frames.push(frame),
      );
    assert.equal(await update(false, false), 1);
    assert.ok(
      frames.some((frame) => frame.kind === 'error' && frame.error.code === 'update_not_admitted'),
    );
    assert.equal(rollbacks, 0);
    frames.length = 0;
    assert.equal(await update(false), 1);
    assert.equal(rollbacks, 1);
    assert.equal(JSON.parse(await readFile(markerPath, 'utf8')).schemaVersion, 1);
    assert.ok(
      frames.some((frame) => frame.kind === 'result' && frame.update.kind === 'active_tasks'),
    );
    frames.length = 0;
    if (failure === 'locator') {
      const rename = fs.rename;
      t.mock.method(fs, 'rename', async (...args: Parameters<typeof fs.rename>) => {
        await rename(...args);
        if (args[1] === join(root.canonicalPath, '.maka-host', 'state')) {
          // The complete snapshot is durable; its old metadata is now dispensable.
          await rm(legacyDirectory, { recursive: true });
          throw new Error('interrupted after snapshot publication');
        }
      });
      syncBuiltinESMExports();
    }
    assert.equal(await update(true), 1);
    assert.equal(
      JSON.parse(await readFile(markerPath, 'utf8')).schemaVersion,
      2,
      JSON.stringify(frames),
    );
    assert.equal(rollbacks, 1, 'the package bound by the format fence cannot be discarded');
    await prepareRuntimeHostRoot(root.canonicalPath);
    const recovered = await resolveRecoverableRuntimeHostManagedDeployment(root.rootId, {
      resolveProvider: () => assert.fail('on-demand deployment has no supervisor'),
      convergeOperator: async () => {},
      verifyOperator: async () => {},
    });
    assert.equal(recovered.kind, 'active');
    if (recovered.kind === 'active') assert.deepEqual(recovered.config.launch.package, target);
    assert.equal(
      await readFile(join(root.canonicalPath, 'desktop-settings.json'), 'utf8'),
      '{"retained":true}',
    );
    assert.match(await readFile(eventsFile, 'utf8'), /"retire"\n"close"/);
    assert.equal(await readFile(targetLayout.cliPath, 'utf8'), '');
  });
}
