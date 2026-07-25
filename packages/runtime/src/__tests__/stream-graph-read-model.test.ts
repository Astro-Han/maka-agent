import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import type {
  AgentGraphOperatorProvision,
  AgentGraphScheduleUpdate,
} from '@maka/core';
import type { AgentGraphSupervisorObservation } from '../stream-graph-dispatch.js';
import type {
  AgentGraphActivationState,
  AgentGraphRecord,
} from '../stream-graph-projection.js';
import {
  buildAgentGraphClientSnapshot,
  inspectAgentGraphOperator,
} from '../stream-graph-read-model.js';

describe('agent graph client read model', () => {
  test('bounds terminal history with a reconnect-safe opaque cursor', () => {
    const graphId = 'graph-1';
    const rootSessionId = 'root-session';
    const operatorId = 'operator-1';
    const childSessionId = 'child-session';
    const records = Array.from({ length: 70 }, (_, index) =>
      terminalRecord({
        graphId,
        operatorId,
        childSessionId,
        index,
      }),
    );
    const activations = Object.fromEntries(
      records.map((record, index) => [
        record.activationId,
        activation(record, index),
      ]),
    );
    const observation = {
      projection: {
        graphId,
        operators: [{ operatorId, sessionId: childSessionId }],
        ignoredPartialEvents: 0,
        records,
        supervisorMetaStream: [],
        state: {
          graphId,
          latestEventTime: 69,
          appliedRecordIds: records.map((record) => record.recordId),
          operators: {
            [operatorId]: {
              operatorId,
              sessionId: childSessionId,
              status: 'completed',
              currentActivationId: 'run-69',
              activations,
            },
          },
        },
      },
      readiness: {
        schemaVersion: 1,
        graphId,
        topologyFingerprint: 'sha256:topology',
        trace: { graphId },
        readiness: {},
        supervisorView: [],
      },
      claims: [],
    } as unknown as AgentGraphSupervisorObservation;
    const input = {
      rootSessionId,
      graphId,
      provisions: [provision(graphId, operatorId, childSessionId)],
      scheduleUpdates: [],
      observation,
    };

    const first = buildAgentGraphClientSnapshot(input);
    assert.equal(first.terminalHistory.records.length, 64);
    assert.ok(first.terminalHistory.nextCursor);
    assert.equal(first.terminalHistory.records[0]?.recordId, 'record-69');
    const second = buildAgentGraphClientSnapshot(input, {
      terminalCursor: first.terminalHistory.nextCursor,
    });
    assert.equal(second.terminalHistory.records.length, 6);
    assert.equal(second.terminalHistory.records[0]?.recordId, 'record-5');
    assert.equal(second.terminalHistory.nextCursor, undefined);
    assert.equal(first.snapshotVersion, second.snapshotVersion);
    assert.notEqual(first.terminalHistory.nextCursor, 'record-5');

    const inspection = inspectAgentGraphOperator(input, operatorId);
    assert.equal(inspection.activations.length, 64);
    assert.equal(inspection.omittedActivationCount, 6);
    assert.equal(inspection.recentRecords.length, 70);
    assert.equal(inspection.operator.childSessionId, childSessionId);
  });

  test('rejects a terminal cursor from another graph', () => {
    const firstInput = emptyInput('graph-1');
    const cursorSource = buildAgentGraphClientSnapshot({
      ...firstInput,
      provisions: [provision('graph-1', 'operator-1', 'child-1')],
      observation: observationWithOneTerminal('graph-1', 'operator-1', 'child-1'),
    });
    // One terminal record has no next page, so create a valid cursor by using
    // the first page of a larger source graph.
    const many = Array.from({ length: 65 }, (_, index) =>
      terminalRecord({
        graphId: 'graph-1',
        operatorId: 'operator-1',
        childSessionId: 'child-1',
        index,
      }),
    );
    const source = buildAgentGraphClientSnapshot({
      ...firstInput,
      provisions: [provision('graph-1', 'operator-1', 'child-1')],
      observation: observationFromRecords('graph-1', 'operator-1', 'child-1', many),
    });
    assert.equal(cursorSource.terminalHistory.nextCursor, undefined);
    assert.ok(source.terminalHistory.nextCursor);
    assert.throws(
      () =>
        buildAgentGraphClientSnapshot(emptyInput('graph-2'), {
          terminalCursor: source.terminalHistory.nextCursor,
        }),
      /another graph/,
    );
  });

  test('keeps schedule payloads bounded while preserving durable control refs', () => {
    const graphId = 'graph-1';
    const operatorId = 'operator-1';
    const childSessionId = 'child-1';
    const instruction = 'x'.repeat(600);
    const snapshot = buildAgentGraphClientSnapshot({
      rootSessionId: 'root-session',
      graphId,
      provisions: [provision(graphId, operatorId, childSessionId)],
      scheduleUpdates: [scheduleUpdate(graphId, instruction)],
      observation: observationWithOneTerminal(graphId, operatorId, childSessionId),
    });
    assert.equal(snapshot.work.length, 1);
    assert.equal(snapshot.work[0]?.instructionTruncated, true);
    assert.equal(snapshot.work[0]?.instructionPreview.length, 501);
    assert.equal(snapshot.recentControlDecisions[0]?.source.agentRunId, 'root-run');
    assert.deepEqual(snapshot.recentControlDecisions[0]?.addedWorkIds, [
      'graph_work_00000000000000000000000000000000',
    ]);
  });
});

function emptyInput(graphId: string) {
  return {
    rootSessionId: 'root-session',
    graphId,
    provisions: [] as AgentGraphOperatorProvision[],
    scheduleUpdates: [],
    observation: {
      projection: {
        graphId,
        operators: [],
        ignoredPartialEvents: 0,
        records: [],
        supervisorMetaStream: [],
        state: { graphId, appliedRecordIds: [], operators: {} },
      },
      readiness: {
        schemaVersion: 1,
        graphId,
        topologyFingerprint: 'sha256:empty',
        trace: { graphId },
        readiness: {},
        supervisorView: [],
      },
      claims: [],
    } as unknown as AgentGraphSupervisorObservation,
  };
}

function observationWithOneTerminal(
  graphId: string,
  operatorId: string,
  childSessionId: string,
): AgentGraphSupervisorObservation {
  return observationFromRecords(
    graphId,
    operatorId,
    childSessionId,
    [terminalRecord({ graphId, operatorId, childSessionId, index: 0 })],
  );
}

function observationFromRecords(
  graphId: string,
  operatorId: string,
  childSessionId: string,
  records: AgentGraphRecord[],
): AgentGraphSupervisorObservation {
  const activations = Object.fromEntries(
    records.map((record, index) => [record.activationId, activation(record, index)]),
  );
  return {
    projection: {
      graphId,
      operators: [{ operatorId, sessionId: childSessionId }],
      ignoredPartialEvents: 0,
      records,
      supervisorMetaStream: [],
      state: {
        graphId,
        latestEventTime: records.at(-1)?.eventTime,
        appliedRecordIds: records.map((record) => record.recordId),
        operators: {
          [operatorId]: {
            operatorId,
            sessionId: childSessionId,
            status: 'completed',
            currentActivationId: records.at(-1)!.activationId,
            activations,
          },
        },
      },
    },
    readiness: {
      schemaVersion: 1,
      graphId,
      topologyFingerprint: 'sha256:topology',
      trace: { graphId },
      readiness: {},
      supervisorView: [],
    },
    claims: [],
  } as unknown as AgentGraphSupervisorObservation;
}

function provision(
  graphId: string,
  operatorId: string,
  childSessionId: string,
): AgentGraphOperatorProvision {
  return {
    schemaVersion: 1,
    provisionId: 'graph_provision_00000000000000000000000000000000',
    provisionFingerprint: `sha256:${'0'.repeat(64)}`,
    graphId,
    workId: 'graph_work_00000000000000000000000000000000',
    agentId: 'local-read',
    operatorId,
    initialTurnId: 'turn-0',
    initialRunId: 'run-0',
    edges: [],
    targetSessionId: childSessionId,
    provisionedAt: 0,
  };
}

function scheduleUpdate(
  graphId: string,
  instruction: string,
): AgentGraphScheduleUpdate {
  return {
    schemaVersion: 1,
    updateId: 'graph_update_00000000000000000000000000000000',
    updateFingerprint: `sha256:${'1'.repeat(64)}`,
    graphId,
    source: {
      sessionId: 'root-session',
      runId: 'root-run',
      turnId: 'root-turn',
      toolCallId: 'root-tool-call',
    },
    addWork: [
      {
        workId: 'graph_work_00000000000000000000000000000000',
        target: { kind: 'agent', agentId: 'local-read' },
        instruction,
        inputIds: [],
      },
    ],
    stop: [],
    revision: 1,
    committedAt: 1,
  };
}

function terminalRecord(input: {
  graphId: string;
  operatorId: string;
  childSessionId: string;
  index: number;
}): AgentGraphRecord {
  return {
    schemaVersion: 1,
    recordId: `record-${input.index}`,
    graphId: input.graphId,
    operatorId: input.operatorId,
    activationId: `run-${input.index}`,
    sessionId: input.childSessionId,
    agentRunId: `run-${input.index}`,
    eventTime: input.index,
    orderKey: {
      runCreatedAt: input.index,
      operatorId: input.operatorId,
      runId: `run-${input.index}`,
      committedEventOrdinal: 0,
      runtimeEventId: `event-${input.index}`,
    },
    type: 'agent_runtime_event',
    facets: ['completed'],
    supervisorSignals: [{ kind: 'terminal', status: 'completed' }],
    source: {
      kind: 'runtime_event',
      runtimeEventId: `event-${input.index}`,
      sessionId: input.childSessionId,
      runId: `run-${input.index}`,
      turnId: `turn-${input.index}`,
      ts: input.index,
    },
  };
}

function activation(
  record: AgentGraphRecord,
  index: number,
): AgentGraphActivationState {
  return {
    activationId: record.activationId,
    agentRunId: record.agentRunId,
    status: 'completed',
    recordCount: 1,
    firstEventTime: index,
    lastEventTime: index,
    lastRecordId: record.recordId,
    terminalRecordId: record.recordId,
  };
}
