import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { chmod, mkdtemp, rm, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { describe, it } from 'node:test';
import {
  createComputerUseHost,
  computerUseServiceHealth,
} from '../computer-use-host.js';

describe('Computer Use host health', () => {
  const snapshot = (
    state: 'idle' | 'starting' | 'ready' | 'backing_off' | 'unavailable' | 'disposed',
  ) => ({ state, generation: 1, restartAttempts: 0 });

  it('does not report a binary-only executor as healthy before first use', () => {
    assert.deepEqual(computerUseServiceHealth('maka-cu', snapshot('idle')), {
      state: 'not_run',
      reason: 'maka-cu 已可用，将在首次调用时启动。',
    });
  });

  it('reports ready, recovery, and unavailable states', () => {
    assert.equal(computerUseServiceHealth('maka-cu', snapshot('ready')).state, 'healthy');
    assert.equal(
      computerUseServiceHealth('maka-cu', snapshot('backing_off')).reason,
      'maka-cu service 正在启动或恢复。',
    );
    assert.equal(
      computerUseServiceHealth('maka-cu', snapshot('starting')).state,
      'degraded',
    );
    assert.deepEqual(computerUseServiceHealth('maka-cu', snapshot('unavailable')), {
      state: 'not_available',
      reason: 'maka-cu service 启动失败或已退出。',
    });
    assert.deepEqual(computerUseServiceHealth('maka-cu', snapshot('disposed')), {
      state: 'not_available',
      reason: 'maka-cu service 已停止。',
    });
  });

  it('reports a missing backend as unavailable', () => {
    assert.equal(computerUseServiceHealth('none', undefined).state, 'not_available');
  });

  it('reads the executor that is selected, not the role pair one of them happens to have', () => {
    // maka-cu supervises one child (§11) and reports its own shape, so it has
    // no `action`/`capture` pair to read. This function took only that pair,
    // while the availability half of the same capability card had already been
    // widened to "any selected executor" — executed against the built desktop
    // module with a genuinely ready maka-cu backend, the card read:
    //
    //   executorState()      = {"state":"ready","generation":1}
    //   serviceState (boot)  = undefined
    //   health               = not_available, reason naming cua-driver
    //   artifactAvailable    = true
    //
    // available, state not_available, and a reason naming an executor that is
    // not the one running.
    assert.deepEqual(
      computerUseServiceHealth('maka-cu', { state: 'ready', generation: 1, restartAttempts: 0 }),
      { state: 'healthy', reason: 'maka-cu 操作与截图服务已就绪。' },
    );
    assert.equal(
      computerUseServiceHealth('maka-cu', {
        state: 'backing_off',
        generation: 1,
        restartAttempts: 1,
      }).state,
      'degraded',
    );
    assert.deepEqual(
      computerUseServiceHealth('maka-cu', {
        state: 'unavailable',
        generation: 1,
        restartAttempts: 3,
      }),
      { state: 'not_available', reason: 'maka-cu service 启动失败或已退出。' },
    );
    assert.equal(
      computerUseServiceHealth('maka-cu', { state: 'idle', generation: 0, restartAttempts: 0 })
        .state,
      'not_run',
    );
  });

  it('constructs a backend only when the local artifact matches the manifest hash', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'maka-cu-host-'));
    try {
      const binaryPath = join(directory, 'maka-cu');
      const manifestPath = join(directory, 'bundled-tools.json');
      const bytes = Buffer.from('#!/bin/sh\nexit 0\n');
      await writeFile(binaryPath, bytes);
      await chmod(binaryPath, 0o755);
      const hash = createHash('sha256').update(bytes).digest('hex');
      await writeFile(manifestPath, JSON.stringify({
        makaCu: { binarySha256: hash, distributionReady: false },
      }));

      const validForDevelopment = createComputerUseHost({
        isPackaged: false,
        resourcesPath: directory,
        manifestPath,
        binaryPath,
        physicalInputRecentlyActive: () => false,
      });
      assert.equal(validForDevelopment.selected.backendId, process.platform === 'darwin'
        ? 'maka-cu'
        : 'none');

      const blockedForDistribution = createComputerUseHost({
        isPackaged: true,
        resourcesPath: directory,
        manifestPath,
        binaryPath,
        physicalInputRecentlyActive: () => false,
      });
      assert.equal(blockedForDistribution.selected.backendId, 'none');

      await writeFile(manifestPath, JSON.stringify({
        makaCu: { binarySha256: hash, distributionReady: true },
      }));
      const validForDistribution = createComputerUseHost({
        isPackaged: true,
        resourcesPath: directory,
        manifestPath,
        binaryPath,
        physicalInputRecentlyActive: () => false,
      });
      assert.equal(validForDistribution.selected.backendId, process.platform === 'darwin'
        ? 'maka-cu'
        : 'none');

      await writeFile(manifestPath, JSON.stringify({
        makaCu: {
          binarySha256: '0'.repeat(64),
          distributionReady: true,
        },
      }));
      const invalid = createComputerUseHost({
        isPackaged: false,
        resourcesPath: directory,
        manifestPath,
        binaryPath,
        physicalInputRecentlyActive: () => false,
      });
      assert.equal(invalid.selected.backendId, 'none');

      const linkedBinaryPath = join(directory, 'linked-maka-cu');
      await symlink(binaryPath, linkedBinaryPath);
      const linked = createComputerUseHost({
        isPackaged: false,
        resourcesPath: directory,
        manifestPath,
        binaryPath: linkedBinaryPath,
        physicalInputRecentlyActive: () => false,
      });
      assert.equal(linked.selected.backendId, 'none');
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });

});
