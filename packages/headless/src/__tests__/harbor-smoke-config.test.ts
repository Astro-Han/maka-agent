import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { describe, test } from 'node:test';
import { fileURLToPath } from 'node:url';
import {
  buildSmokeJobConfig,
  resolveSmokeRunTargets,
  type SmokeManifest,
} from '../harbor-smoke-config.js';
import { MAKA_SETTLEMENT_GRACE_SEC } from '../maka-settlement.js';

const repoRoot = resolve(fileURLToPath(new URL('../../../..', import.meta.url)));

async function loadManifest(): Promise<SmokeManifest> {
  const path = resolve(repoRoot, 'packages/headless/harbor/terminal-bench-smoke-profiles.json');
  return JSON.parse(await readFile(path, 'utf8')) as SmokeManifest;
}

const fixedNow = () => new Date('2026-07-16T12:34:56.000Z');

describe('harbor smoke config generation', () => {
  test('unknown profile throws with available names', async () => {
    const manifest = await loadManifest();
    assert.throws(
      () => buildSmokeJobConfig({ manifest, profileName: 'does-not-exist' }),
      /unknown profile "does-not-exist"\. Available profiles: .*maka-basic/,
    );
  });

  test('maka profiles drive maka_agent:MakaAgent in task-run mode and tag the dataset', async () => {
    const manifest = await loadManifest();
    for (const profileName of [
      'maka-basic',
      'maka-heavy',
      'maka-heavy-prune',
      'maka-prune-default',
      'maka-stale-off',
      'maka-retrieval-on',
    ]) {
      const { config } = buildSmokeJobConfig({
        manifest,
        profileName,
        overrides: { jobName: `job-${profileName}` },
      });
      const agent = (config.agents as Array<Record<string, unknown>>)[0]!;
      const env = agent.env as Record<string, string>;
      assert.equal(agent.import_path, 'maka_agent:MakaAgent', profileName);
      assert.equal(env.MAKA_HARBOR_MODE, 'task-run', profileName);
      assert.equal(env.MAKA_BENCHMARK_DATASET, 'terminal-bench-sample', profileName);
      const datasets = config.datasets as Array<Record<string, unknown>>;
      assert.equal(datasets[0]!.name, 'terminal-bench-sample', profileName);
    }
  });

  test('heavy profile preserves heavy-task env verbatim', async () => {
    const manifest = await loadManifest();
    const { config } = buildSmokeJobConfig({
      manifest,
      profileName: 'maka-heavy',
      overrides: { jobName: 'job' },
    });
    const env = (config.agents as Array<Record<string, unknown>>)[0]!.env as Record<string, string>;
    assert.equal(env.MAKA_HEAVY_TASK_MODE, '1');
    assert.equal(env.MAKA_HARBOR_USE_TASK_RUN, '1');
    assert.equal(env.MAKA_MAX_STEPS, '100');
    assert.equal(env.MAKA_CELL_TIMEOUT_SEC, '7200');
    assert.equal(env.MAKA_HARBOR_AGENT_TIMEOUT_SEC, undefined);
  });

  // Harbor resolves the agent phase as
  // `min(override_timeout_sec ?? task_declared, max_timeout_sec ?? inf) * multiplier`
  // (harbor/trial/trial.py, _resolve_timeout_sec). Every deadline assertion below
  // goes through this rather than reading one field: a settlement tail published
  // on max_timeout_sec satisfies a field-shaped assertion while folding straight
  // back to the task's own timeout, which is how the window stayed unreachable
  // through the smoke path while these tests were green.
  type SmokeAgentEntry = {
    env: Record<string, string>;
    override_timeout_sec?: number | null;
    max_timeout_sec?: number | null;
  };
  const harborAgentPhaseSec = (
    config: Record<string, unknown>,
    taskDeclaredSec: number,
  ): number => {
    const agent = (config.agents as SmokeAgentEntry[])[0]!;
    const multiplier =
      (config.agent_timeout_multiplier as number | null) ?? (config.timeout_multiplier as number);
    return (
      Math.min(
        agent.override_timeout_sec ?? taskDeclaredSec,
        agent.max_timeout_sec ?? Number.POSITIVE_INFINITY,
      ) * multiplier
    );
  };

  // The declared agent timeouts actually present in the smoke dataset. The
  // profile multipliers were tuned against 900 alone, so anything else is where
  // a multiplier-derived phase silently stops tracking the cell budget.
  const DECLARED_AGENT_TIMEOUTS_SEC = [600, 900, 1800, 3600];

  test('the smoke agent phase outlasts the cell budget by the settlement window', async () => {
    // The regression this pins: publishing budget + grace on max_timeout_sec.
    // For maka-basic on the default 900s task Harbor folded that to
    // min(900, 3630) * 4 = 3600 — the cell budget exactly, so the cell was
    // SIGKILLed at the instant it stopped calling the model and began writing
    // maka-cell-output.json, and a scored trial read as an infra failure.
    const manifest = await loadManifest();
    for (const profileName of ['maka-basic', 'maka-heavy']) {
      const { config } = buildSmokeJobConfig({
        manifest,
        profileName,
        overrides: { jobName: 'job' },
      });
      const agent = (config.agents as SmokeAgentEntry[])[0]!;
      const budget = Number(agent.env.MAKA_CELL_TIMEOUT_SEC);
      // The budget is the cell's, so the phase is the same on every task the
      // dataset declares — not only the 900s one the multiplier was tuned for.
      for (const declared of DECLARED_AGENT_TIMEOUTS_SEC) {
        assert.equal(
          harborAgentPhaseSec(config, declared),
          budget + MAKA_SETTLEMENT_GRACE_SEC,
          `${profileName} @ declared=${declared}`,
        );
      }
      // A ceiling cannot raise a base, so leaving one behind can only re-clamp it.
      assert.equal(agent.max_timeout_sec, null, profileName);
    }
  });

  test('a non-maka smoke profile keeps its multiplier and gets no settlement window', async () => {
    const manifest = await loadManifest();
    // opencode has no cell budget to publish, so the multiplier stays its only
    // control — and on the default 900s task it still buys the same 3600s of
    // model time the maka arms get, which is what makes --compare comparable.
    const { config: opencode } = buildSmokeJobConfig({
      manifest,
      profileName: 'opencode',
      overrides: { jobName: 'job' },
    });
    assert.equal(opencode.agent_timeout_multiplier, 4);
    assert.equal(harborAgentPhaseSec(opencode, 900), 3600);
    assert.equal((opencode.agents as SmokeAgentEntry[])[0]!.override_timeout_sec, null);

    const { config } = buildSmokeJobConfig({
      manifest,
      profileName: 'oracle',
      overrides: { jobName: 'job' },
    });
    const agent = (config.agents as SmokeAgentEntry[])[0]!;
    assert.equal(agent.max_timeout_sec, null);
    assert.equal(agent.override_timeout_sec, null);
    // No multiplier of its own: the phase is the task's own declared timeout.
    assert.equal(config.agent_timeout_multiplier, null);
    assert.equal(harborAgentPhaseSec(config, 900), 900);
  });

  test('--agent-timeout-sec moves the whole phase, not just the cell budget', async () => {
    const manifest = await loadManifest();
    const { config } = buildSmokeJobConfig({
      manifest,
      profileName: 'maka-heavy',
      overrides: { jobName: 'job', agentTimeoutSec: '180' },
    });
    const agent = (config.agents as SmokeAgentEntry[])[0]!;
    assert.equal(agent.env.MAKA_CELL_TIMEOUT_SEC, '180');
    // Pre-fix this resolved to min(900, 210) * 8 = 1680 — an operator asking for
    // a 3-minute smoke run got a 28-minute one, and never saw the kill at all.
    assert.equal(harborAgentPhaseSec(config, 900), 180 + MAKA_SETTLEMENT_GRACE_SEC);
  });

  test('a maka profile with an unparseable cell budget falls back to the multiplier', async () => {
    // Nothing absolute to publish, so Harbor's own base has to stand — clamping
    // it to a phase we could not derive would be worse than leaving it alone.
    const manifest = await loadManifest();
    const profile = manifest.profiles!['maka-basic']!;
    const patched: SmokeManifest = {
      ...manifest,
      profiles: {
        ...manifest.profiles,
        'maka-basic': {
          ...profile,
          agent: {
            ...profile.agent,
            env: { ...profile.agent!.env, MAKA_CELL_TIMEOUT_SEC: 'nope' },
          },
        },
      },
    };
    const { config } = buildSmokeJobConfig({
      manifest: patched,
      profileName: 'maka-basic',
      overrides: { jobName: 'job' },
    });
    assert.equal((config.agents as SmokeAgentEntry[])[0]!.override_timeout_sec, null);
    assert.equal(config.agent_timeout_multiplier, 4);
    assert.equal(harborAgentPhaseSec(config, 900), 3600);
  });

  test('--model override targets MAKA_MODEL for maka and model_name for non-maka', async () => {
    const manifest = await loadManifest();
    const maka = buildSmokeJobConfig({
      manifest,
      profileName: 'maka-basic',
      overrides: { jobName: 'j', model: 'deepseek/deepseek-vX' },
    });
    const makaAgent = (maka.config.agents as Array<Record<string, unknown>>)[0]!;
    assert.equal((makaAgent.env as Record<string, string>).MAKA_MODEL, 'deepseek/deepseek-vX');
    assert.equal(makaAgent.model_name, null);

    const opencode = buildSmokeJobConfig({
      manifest,
      profileName: 'opencode',
      overrides: { jobName: 'j', model: 'deepseek/other' },
    });
    const ocAgent = (opencode.config.agents as Array<Record<string, unknown>>)[0]!;
    assert.equal(ocAgent.model_name, 'deepseek/other');
    assert.equal(ocAgent.import_path, 'opencode_title_harbor_agent:OpenCodeTitleAgent');
    assert.deepEqual(ocAgent.env, {});
  });

  test('n-tasks replaces task_names with a task count', async () => {
    const manifest = await loadManifest();
    const withPattern = buildSmokeJobConfig({
      manifest,
      profileName: 'oracle',
      overrides: { jobName: 'j', taskPattern: '*foo' },
    });
    const withCount = buildSmokeJobConfig({
      manifest,
      profileName: 'oracle',
      overrides: { jobName: 'j', nTasks: 3 },
    });
    const dsPattern = (withPattern.config.datasets as Array<Record<string, unknown>>)[0]!;
    const dsCount = (withCount.config.datasets as Array<Record<string, unknown>>)[0]!;
    assert.deepEqual(dsPattern.task_names, ['*foo']);
    assert.equal(dsPattern.n_tasks, null);
    assert.equal(dsCount.task_names, null);
    assert.equal(dsCount.n_tasks, 3);
  });

  test('rejects non-positive n-tasks', async () => {
    const manifest = await loadManifest();
    assert.throws(
      () =>
        buildSmokeJobConfig({
          manifest,
          profileName: 'oracle',
          overrides: { jobName: 'j', nTasks: 0 },
        }),
      /--n-tasks must be a positive integer/,
    );
  });

  test('dataset name/version overrides flow into the dataset and MAKA_BENCHMARK_DATASET', async () => {
    const manifest = await loadManifest();
    const { config } = buildSmokeJobConfig({
      manifest,
      profileName: 'maka-basic',
      overrides: { jobName: 'j', datasetName: 'terminal-bench', datasetVersion: '3.1' },
    });
    const ds = (config.datasets as Array<Record<string, unknown>>)[0]!;
    assert.equal(ds.name, 'terminal-bench');
    assert.equal(ds.version, '3.1');
    const env = (config.agents as Array<Record<string, unknown>>)[0]!.env as Record<string, string>;
    assert.equal(env.MAKA_BENCHMARK_DATASET, 'terminal-bench');
  });

  test('oracle profile keeps the built-in agent and null import path', async () => {
    const manifest = await loadManifest();
    const { config } = buildSmokeJobConfig({
      manifest,
      profileName: 'oracle',
      overrides: { jobName: 'j' },
    });
    const agent = (config.agents as Array<Record<string, unknown>>)[0]!;
    assert.equal(agent.name, 'oracle');
    assert.equal(agent.import_path, null);
    assert.equal(config.agent_timeout_multiplier, null);
  });

  test('generated job name uses the injected clock when no explicit name is given', () => {
    const manifest: SmokeManifest = {
      defaults: { taskPattern: '*sqlite-with-gcov' },
      profiles: { 'maka-basic': { agent: { importPath: 'maka_agent:MakaAgent', env: {} } } },
    };
    const { jobName } = buildSmokeJobConfig({
      manifest,
      profileName: 'maka-basic',
      overrides: { now: fixedNow },
    });
    assert.equal(jobName, 'maka-basic-terminal-bench-sample-sqlite-with-gcov-20260716T123456Z');
  });

  test('resolveSmokeRunTargets returns a single target without compare', () => {
    assert.deepEqual(
      resolveSmokeRunTargets({ compare: false, profile: 'maka-heavy', jobName: 'run1' }),
      [{ profileName: 'maka-heavy', jobName: 'run1' }],
    );
  });

  test('resolveSmokeRunTargets splits compare profiles and suffixes job names', () => {
    assert.deepEqual(
      resolveSmokeRunTargets({
        compare: true,
        compareProfiles: 'maka-heavy, opencode',
        profile: 'x',
        jobName: 'run1',
      }),
      [
        { profileName: 'maka-heavy', jobName: 'run1-maka-heavy' },
        { profileName: 'opencode', jobName: 'run1-opencode' },
      ],
    );
  });

  test('resolveSmokeRunTargets leaves job names blank when none is supplied', () => {
    assert.deepEqual(
      resolveSmokeRunTargets({
        compare: true,
        compareProfiles: 'maka-basic,opencode',
        profile: 'x',
      }),
      [
        { profileName: 'maka-basic', jobName: '' },
        { profileName: 'opencode', jobName: '' },
      ],
    );
  });
});
