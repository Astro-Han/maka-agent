import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import { AGENT_GRAPH_SUPERVISOR_WAKE_SCHEMA_VERSION } from '@maka/core';
import { createSqliteSessionMetadataStore } from '../sqlite-session-metadata-store.js';

describe('SQLite Agent Graph supervisor wakes', () => {
  test('tracks retry attempts separately and marks delivery only after completion', async () => {
    let now = 10;
    const store = createSqliteSessionMetadataStore(':memory:', { now: () => now++ });
    try {
      const claim = await store.claimAgentGraphSupervisorWake({
        schemaVersion: AGENT_GRAPH_SUPERVISOR_WAKE_SCHEMA_VERSION,
        graphId: 'graph-1',
        wakeId: 'wake-1',
        snapshotVersion: 'snapshot-1',
        rootSessionId: 'session-1',
      });
      assert.equal(claim.created, true);
      assert.equal(claim.wake.status, 'pending');

      const first = await store.beginAgentGraphSupervisorWakeAttempt({
        graphId: 'graph-1',
        wakeId: 'wake-1',
        attemptId: 'attempt-1',
        turnId: 'turn-1',
      });
      assert.equal(first.acquired, true);
      assert.equal(first.wake.status, 'running');
      assert.equal(first.wake.attemptCount, 1);
      await store.completeAgentGraphSupervisorWakeAttempt({
        graphId: 'graph-1',
        wakeId: 'wake-1',
        attemptId: 'attempt-1',
        status: 'retryable_failed',
        failureReason: 'provider failed after prompt persistence',
      });

      const second = await store.beginAgentGraphSupervisorWakeAttempt({
        graphId: 'graph-1',
        wakeId: 'wake-1',
        attemptId: 'attempt-2',
        turnId: 'turn-2',
      });
      assert.equal(second.acquired, true);
      assert.equal(second.wake.attemptCount, 2);
      const delivered = await store.completeAgentGraphSupervisorWakeAttempt({
        graphId: 'graph-1',
        wakeId: 'wake-1',
        attemptId: 'attempt-2',
        status: 'delivered',
      });
      assert.equal(delivered.status, 'delivered');
      assert.deepEqual(
        (await store.listAgentGraphSupervisorWakeAttempts('graph-1', 'wake-1')).map(
          (attempt) => attempt.status,
        ),
        ['retryable_failed', 'delivered'],
      );
      assert.equal(
        (
          await store.beginAgentGraphSupervisorWakeAttempt({
            graphId: 'graph-1',
            wakeId: 'wake-1',
            attemptId: 'attempt-3',
            turnId: 'turn-3',
          })
        ).acquired,
        false,
      );
    } finally {
      store.close();
    }
  });

  test('recovers both pre-run pending wakes and crash-interrupted running attempts', async () => {
    let now = 20;
    const store = createSqliteSessionMetadataStore(':memory:', { now: () => now++ });
    try {
      await store.claimAgentGraphSupervisorWake({
        schemaVersion: AGENT_GRAPH_SUPERVISOR_WAKE_SCHEMA_VERSION,
        graphId: 'graph-pending',
        wakeId: 'wake-pending',
        snapshotVersion: 'snapshot-pending',
        rootSessionId: 'session-pending',
      });
      await store.claimAgentGraphSupervisorWake({
        schemaVersion: AGENT_GRAPH_SUPERVISOR_WAKE_SCHEMA_VERSION,
        graphId: 'graph-running',
        wakeId: 'wake-running',
        snapshotVersion: 'snapshot-running',
        rootSessionId: 'session-running',
      });
      await store.beginAgentGraphSupervisorWakeAttempt({
        graphId: 'graph-running',
        wakeId: 'wake-running',
        attemptId: 'attempt-running',
        turnId: 'turn-running',
      });

      assert.equal(await store.recoverAgentGraphSupervisorWakes(), 2);
      assert.equal(
        (await store.readAgentGraphSupervisorWake('graph-pending', 'wake-pending'))?.status,
        'retryable_failed',
      );
      assert.equal(
        (await store.readAgentGraphSupervisorWake('graph-running', 'wake-running'))?.status,
        'retryable_failed',
      );
      assert.equal(
        (await store.listAgentGraphSupervisorWakeAttempts('graph-running', 'wake-running'))[0]
          ?.failureReason,
        'host_restart',
      );
    } finally {
      store.close();
    }
  });
});
