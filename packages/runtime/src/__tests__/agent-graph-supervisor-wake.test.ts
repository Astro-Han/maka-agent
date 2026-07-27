import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import { createSqliteSessionMetadataStore } from '@maka/storage';
import { AgentGraphSupervisorWakeCoordinator } from '../agent-graph-supervisor-wake.js';
import { SessionActivityRegistry, type GoalTurnOutcome } from '../goal-turn-lifecycle.js';
import type { AgentGraphClientSnapshot } from '../stream-graph-read-model.js';
import type { AgentGraphScheduleReconciliationResult } from '../stream-graph-schedule-reconcile.js';

describe('Agent Graph supervisor wake delivery', () => {
  test('retries failures before and after prompt persistence, delivering only a completed turn', async () => {
    const store = createSqliteSessionMetadataStore(':memory:');
    const persistedAttempts: string[] = [];
    let attempt = 0;
    const coordinator = new AgentGraphSupervisorWakeCoordinator({
      activityRegistry: new SessionActivityRegistry(),
      wakeStore: store,
      readSnapshot: async () => snapshot(),
      startTurn: async (_sessionId, input): Promise<GoalTurnOutcome> => {
        attempt += 1;
        if (attempt === 1) throw new Error('failed before prompt persistence');
        persistedAttempts.push(input.origin?.kind === 'agent_graph' ? input.origin.attemptId : '');
        if (attempt === 2) {
          return { kind: 'errored', turnId: input.turnId, reason: 'provider failed' };
        }
        return { kind: 'completed', turnId: input.turnId };
      },
      newId: sequentialIds(),
    });
    try {
      coordinator.notify('root-session', reconciliation());
      await coordinator.waitForIdle();
      const wake = await store.readAgentGraphSupervisorWake('graph-1', 'graph-1:snapshot-1');
      assert.equal(wake?.status, 'delivered');
      assert.equal(wake?.attemptCount, 3);
      assert.equal(new Set(persistedAttempts).size, 2);
      assert.deepEqual(
        (await store.listAgentGraphSupervisorWakeAttempts('graph-1', 'graph-1:snapshot-1')).map(
          (candidate) => candidate.status,
        ),
        ['retryable_failed', 'retryable_failed', 'delivered'],
      );
    } finally {
      await coordinator.close();
      store.close();
    }
  });

  test('retries a suspended outcome instead of treating its persisted prompt as delivery', async () => {
    const store = createSqliteSessionMetadataStore(':memory:');
    let attempt = 0;
    const coordinator = new AgentGraphSupervisorWakeCoordinator({
      activityRegistry: new SessionActivityRegistry(),
      wakeStore: store,
      readSnapshot: async () => snapshot(),
      startTurn: async (_sessionId, input): Promise<GoalTurnOutcome> => {
        attempt += 1;
        return attempt === 1
          ? { kind: 'suspended', turnId: input.turnId, reason: 'permission handoff' }
          : { kind: 'completed', turnId: input.turnId };
      },
      newId: sequentialIds(),
    });
    try {
      coordinator.notify('root-session', reconciliation());
      await coordinator.waitForIdle();
      assert.equal(
        (await store.readAgentGraphSupervisorWake('graph-1', 'graph-1:snapshot-1'))?.attemptCount,
        2,
      );
    } finally {
      await coordinator.close();
      store.close();
    }
  });

  test('recovers a crash-interrupted running attempt and redelivers it', async () => {
    const store = createSqliteSessionMetadataStore(':memory:');
    await store.claimAgentGraphSupervisorWake({
      schemaVersion: 1,
      graphId: 'graph-1',
      wakeId: 'graph-1:snapshot-1',
      snapshotVersion: 'snapshot-1',
      rootSessionId: 'root-session',
    });
    await store.beginAgentGraphSupervisorWakeAttempt({
      graphId: 'graph-1',
      wakeId: 'graph-1:snapshot-1',
      attemptId: 'crashed-attempt',
      turnId: 'crashed-turn',
    });
    let delivered = 0;
    const coordinator = new AgentGraphSupervisorWakeCoordinator({
      activityRegistry: new SessionActivityRegistry(),
      wakeStore: store,
      readSnapshot: async () => snapshot(),
      startTurn: async (_sessionId, input) => {
        delivered += 1;
        return { kind: 'completed', turnId: input.turnId };
      },
      newId: sequentialIds(),
    });
    try {
      assert.equal(await coordinator.recover(), 1);
      await coordinator.waitForIdle();
      assert.equal(delivered, 1);
      assert.equal(
        (await store.readAgentGraphSupervisorWake('graph-1', 'graph-1:snapshot-1'))?.status,
        'delivered',
      );
    } finally {
      await coordinator.close();
      store.close();
    }
  });

  test('close aborts a queued activity acquisition and never starts a turn afterward', async () => {
    const store = createSqliteSessionMetadataStore(':memory:');
    const activities = new SessionActivityRegistry();
    const busy = activities.reserve('root-session');
    let turns = 0;
    const coordinator = new AgentGraphSupervisorWakeCoordinator({
      activityRegistry: activities,
      wakeStore: store,
      readSnapshot: async () => snapshot(),
      startTurn: async (_sessionId, input) => {
        turns += 1;
        return { kind: 'completed', turnId: input.turnId };
      },
      newId: sequentialIds(),
    });
    try {
      coordinator.notify('root-session', reconciliation());
      await new Promise<void>((resolve) => setImmediate(resolve));
      await coordinator.close();
      busy.release();
      await new Promise<void>((resolve) => setImmediate(resolve));
      assert.equal(turns, 0);
    } finally {
      busy.release();
      await coordinator.close();
      store.close();
    }
  });
});

function snapshot(): AgentGraphClientSnapshot {
  return {
    schemaVersion: 1,
    rootSessionId: 'root-session',
    graphId: 'graph-1',
    snapshotVersion: 'snapshot-1',
    status: 'waiting',
    scheduleRevision: 1,
    topologyFingerprint: 'topology-1',
    closed: false,
    operators: [],
    edges: [],
    work: [],
    stoppedTargets: [],
    claims: [],
    recentControlDecisions: [],
    recentActivity: [],
    terminalHistory: { records: [] },
    omitted: {
      operators: 0,
      edges: 0,
      work: 0,
      stoppedTargets: 0,
      claims: 0,
      controlDecisions: 0,
      recentActivity: 0,
    },
  };
}

function reconciliation(): AgentGraphScheduleReconciliationResult {
  return {
    status: 'reconciled',
    dispatches: [{}],
    failures: [],
  } as unknown as AgentGraphScheduleReconciliationResult;
}

function sequentialIds(): () => string {
  let value = 0;
  return () => `id-${++value}`;
}
