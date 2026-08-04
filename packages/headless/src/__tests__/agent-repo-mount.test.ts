import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, test } from 'node:test';
import {
  buildAgentRepoMounts,
  competitorRepoFiles,
  CONTAINER_MAKA_REPO,
} from '../agent-repo-mount.js';
import type { HarnessAgentId } from '../harness-agent-registry.js';

const COMPETITORS: readonly Exclude<HarnessAgentId, 'maka'>[] = [
  'opencode',
  'kimi-code',
  'codex',
  'claude-code',
  'reasonix',
];

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '../../../..');

describe('agent repo mounts', () => {
  test('gives Maka the tree it executes out of', () => {
    assert.deepEqual(buildAgentRepoMounts('maka', '/repo'), [
      { type: 'bind', source: '/repo', target: CONTAINER_MAKA_REPO, read_only: true },
    ]);
  });

  for (const agent of COMPETITORS) {
    test(`hands ${agent} files, never a directory it can walk`, () => {
      const mounts = buildAgentRepoMounts(agent, '/repo') as Array<{
        source: string;
        target: string;
        read_only: boolean;
      }>;
      // The whole point: no target is the repo root, so there is nothing to
      // enumerate. A directory mount here is how Codex reached the benchmark's
      // pinned revision and retrieved a task's reference solution.
      assert.ok(
        mounts.every((mount) => mount.target !== CONTAINER_MAKA_REPO),
        `${agent} must not receive the repo root`,
      );
      assert.deepEqual(
        mounts.map((mount) => mount.target),
        competitorRepoFiles(agent).map((file) => `${CONTAINER_MAKA_REPO}/${file}`),
      );
      assert.ok(
        mounts.every((mount) => mount.read_only),
        `${agent} must not receive a writable repo path`,
      );
    });

    test(`every file ${agent} declares exists in the repo`, () => {
      // A declared-but-missing path is worse than a missing mount: Docker
      // materialises the target as an empty directory, so the adapter reads a
      // directory where it expected its config and fails inside the container
      // rather than here.
      for (const file of competitorRepoFiles(agent)) {
        assert.ok(existsSync(join(REPO_ROOT, file)), `${agent} declares missing ${file}`);
      }
    });
  }

  test('keeps the benchmark identity out of every competitor container', () => {
    // run-harness-ab.mjs carries TERMINAL_BENCH_2_1_REVISION and the upstream
    // repository URL; docs/eval carries earlier per-task results. Neither is a
    // file any arm needs, and both convert a graded run into retrieval.
    for (const agent of COMPETITORS) {
      for (const file of competitorRepoFiles(agent)) {
        assert.ok(
          !file.startsWith('docs/'),
          `${agent} must not be handed evaluation records (${file})`,
        );
        assert.notEqual(
          file,
          'packages/headless/harbor/run-harness-ab.mjs',
          `${agent} must not be handed the harness manifest source`,
        );
      }
    }
  });
});
