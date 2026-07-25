import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import type {
  AgentGraphScheduleControlStore,
  AgentGraphScheduleUpdateRequest,
} from '@maka/core/agent-graph-schedule';
import type { AgentGraphIntentClaimStore } from '@maka/core/agent-graph-control';
import { createSqliteSessionMetadataStore } from '@maka/storage';
import type {
  AgentGraphIntentExecutor,
  AgentGraphSupervisorObservation,
} from '../stream-graph-dispatch.js';
import { reconcileAgentGraphSchedule } from '../stream-graph-schedule-reconcile.js';
import {
  compileAgentGraphScheduleUpdate,
  type UpdateAgentGraphToolInput,
} from '../stream-graph-supervisor-tools.js';
import type { AgentGraphTraceTopology } from '../stream-graph-trace.js';

describe('stream graph schedule reconciliation', () => {
  test('executes existing operators durably and leaves new agents waiting for topology', async () => {
    const store = createSqliteSessionMetadataStore(':memory:', { now: nextNumber(100) });
    const observation = new MemoryGraphObservation();
    const executor = new MemoryScheduleExecutor(store, observation);
    const stopController = new MemoryStopController(observation);
    let observedSnapshots = 0;
    let observedActivations = 0;
    try {
      await commitSchedule(store, 'tool-add', {
        add_work: [
          {
            operator_id: 'writer',
            instruction: 'Revise the answer with the selected evidence.',
            input_ids: ['record-input'],
          },
          {
            agent_id: 'local-read',
            instruction: 'Check one more source.',
            input_ids: [],
          },
        ],
      });

      const first = await reconcileAgentGraphSchedule({
        topology: topology(),
        controlStore: store,
        executor,
        stopController,
        newId: nextId(),
        maxNewActivations: 1,
        observeGraph: () => observation.read(),
        renderPrompt: ({ work, inputRecords }) =>
          `${work.instruction}\nInputs: ${inputRecords.map((record) => record.recordId).join(',')}`,
        supervisor: {
          onObservation() {
            observedSnapshots += 1;
            throw new Error('presentation observer must not gate reconciliation');
          },
          onActivationReady() {
            observedActivations += 1;
          },
        },
      });

      assert.equal(first.status, 'waiting');
      assert.equal(first.newActivationCount, 1);
      assert.equal(first.observedExistingActivationCount, 0);
      assert.equal(first.dispatches.length, 1);
      assert.equal(first.dispatches[0]?.intent.policyKind, 'supervisor');
      assert.deepEqual(first.dispatches[0]?.intent.triggerRecordIds, ['record-input']);
      assert.deepEqual(
        first.deferredWork.map((item) => item.reason),
        ['agent_topology_required'],
      );
      assert.equal(executor.backendInvocations, 1);
      assert.ok(observedSnapshots >= 2);
      assert.equal(observedActivations, 1);

      const retry = await reconcileAgentGraphSchedule({
        topology: topology(),
        controlStore: store,
        executor,
        stopController,
        newId: nextId(),
        maxNewActivations: 0,
        observeGraph: () => observation.read(),
        renderPrompt: ({ work, inputRecords }) =>
          `${work.instruction}\nInputs: ${inputRecords.map((record) => record.recordId).join(',')}`,
      });

      assert.equal(retry.status, 'waiting');
      assert.equal(retry.newActivationCount, 0);
      assert.equal(retry.observedExistingActivationCount, 1);
      assert.equal(executor.backendInvocations, 1);
    } finally {
      store.close();
    }
  });

  test('stops an admitted activation before considering later work', async () => {
    const store = createSqliteSessionMetadataStore(':memory:', { now: nextNumber(200) });
    const observation = new MemoryGraphObservation();
    const executor = new MemoryScheduleExecutor(store, observation, 'running');
    const stopController = new MemoryStopController(observation);
    try {
      const added = await commitSchedule(store, 'tool-add', {
        add_work: [
          {
            operator_id: 'writer',
            instruction: 'Keep drafting until stopped.',
            input_ids: [],
          },
        ],
      });
      const workId = added.addWork[0]!.workId;
      const first = await reconcileAgentGraphSchedule({
        topology: topology(),
        controlStore: store,
        executor,
        stopController,
        newId: nextId(),
        maxNewActivations: 1,
        observeGraph: () => observation.read(),
        renderPrompt: ({ work }) => work.instruction,
      });
      assert.equal(first.status, 'reconciled');
      assert.equal(first.dispatches[0]?.result.status, 'running');

      await commitSchedule(store, 'tool-stop', {
        stop: [{ target_id: workId, reason: 'The draft is no longer useful.' }],
      });
      const stopped = await reconcileAgentGraphSchedule({
        topology: topology(),
        controlStore: store,
        executor,
        stopController,
        newId: nextId(),
        maxNewActivations: 0,
        observeGraph: () => observation.read(),
        renderPrompt: ({ work }) => work.instruction,
      });

      assert.equal(stopped.status, 'reconciled');
      assert.deepEqual(stopController.calls, [
        { sessionId: 'session-writer', source: 'graph_supervisor' },
      ]);
      assert.deepEqual(
        stopped.stops.map((result) => [result.targetId, result.status, result.activationId]),
        [[workId, 'stopped', first.dispatches[0]!.claim.targetRunId]],
      );
      assert.equal(stopped.dispatches.length, 0);
      assert.equal(stopped.schedule.work[0]?.status, 'stopped');
    } finally {
      store.close();
    }
  });

  test('does not admit stale work when a stop wins the SQLite revision race', async () => {
    const store = createSqliteSessionMetadataStore(':memory:', { now: nextNumber(300) });
    const observation = new MemoryGraphObservation();
    const executor = new MemoryScheduleExecutor(store, observation);
    const stopController = new MemoryStopController(observation);
    try {
      const added = await commitSchedule(store, 'tool-add', {
        add_work: [
          {
            operator_id: 'writer',
            instruction: 'This must not start after stop commits.',
            input_ids: [],
          },
        ],
      });
      const workId = added.addWork[0]!.workId;
      let raced = false;
      const racingStore: AgentGraphScheduleControlStore = {
        commitAgentGraphScheduleUpdate: (request) => store.commitAgentGraphScheduleUpdate(request),
        listAgentGraphScheduleUpdates: (graphId) => store.listAgentGraphScheduleUpdates(graphId),
        claimAgentGraphIntent: (request) => store.claimAgentGraphIntent(request),
        readAgentGraphIntentClaim: (graphId, intentId) =>
          store.readAgentGraphIntentClaim(graphId, intentId),
        listAgentGraphIntentClaims: (graphId) => store.listAgentGraphIntentClaims(graphId),
        beginAgentGraphIntentExecutionAtScheduleRevision: (graphId, intentId, expectedRevision) =>
          store.beginAgentGraphIntentExecutionAtScheduleRevision(
            graphId,
            intentId,
            expectedRevision,
          ),
        cancelAgentGraphIntentExecution: (graphId, intentId, reason) =>
          store.cancelAgentGraphIntentExecution(graphId, intentId, reason),
        async claimAgentGraphIntentAtScheduleRevision(request, expectedRevision) {
          if (!raced) {
            raced = true;
            await commitSchedule(store, 'tool-stop', {
              stop: [{ target_id: workId, reason: 'Stop won the revision race.' }],
            });
          }
          return await store.claimAgentGraphIntentAtScheduleRevision(request, expectedRevision);
        },
      };

      const result = await reconcileAgentGraphSchedule({
        topology: topology(),
        controlStore: racingStore,
        executor,
        stopController,
        newId: nextId(),
        maxNewActivations: 1,
        observeGraph: () => observation.read(),
        renderPrompt: ({ work }) => work.instruction,
      });

      assert.equal(result.status, 'reconciled');
      assert.equal(result.newActivationCount, 0);
      assert.equal(result.dispatches.length, 0);
      assert.equal(result.stops[0]?.status, 'cancelled_before_runtime');
      assert.equal(executor.backendInvocations, 0);
      assert.deepEqual(await store.listAgentGraphIntentClaims(GRAPH_ID), []);
    } finally {
      store.close();
    }
  });

  test('durably cancels a claim when stop wins before Runtime execution admission', async () => {
    const store = createSqliteSessionMetadataStore(':memory:', { now: nextNumber(400) });
    const observation = new MemoryGraphObservation();
    const executor = new MemoryScheduleExecutor(store, observation);
    const stopController = new MemoryStopController(observation);
    const claimed = deferred<void>();
    const releaseClaim = deferred<void>();
    let pauseClaim = true;
    try {
      const added = await commitSchedule(store, 'tool-add', {
        add_work: [
          {
            operator_id: 'writer',
            instruction: 'Do not start if the supervisor stops this claim.',
            input_ids: [],
          },
        ],
      });
      const workId = added.addWork[0]!.workId;
      const pausingStore: AgentGraphScheduleControlStore = {
        commitAgentGraphScheduleUpdate: (request) => store.commitAgentGraphScheduleUpdate(request),
        listAgentGraphScheduleUpdates: (graphId) => store.listAgentGraphScheduleUpdates(graphId),
        claimAgentGraphIntent: (request) => store.claimAgentGraphIntent(request),
        readAgentGraphIntentClaim: (graphId, intentId) =>
          store.readAgentGraphIntentClaim(graphId, intentId),
        listAgentGraphIntentClaims: (graphId) => store.listAgentGraphIntentClaims(graphId),
        beginAgentGraphIntentExecutionAtScheduleRevision: (graphId, intentId, expectedRevision) =>
          store.beginAgentGraphIntentExecutionAtScheduleRevision(
            graphId,
            intentId,
            expectedRevision,
          ),
        cancelAgentGraphIntentExecution: (graphId, intentId, reason) =>
          store.cancelAgentGraphIntentExecution(graphId, intentId, reason),
        async claimAgentGraphIntentAtScheduleRevision(request, expectedRevision) {
          const result = await store.claimAgentGraphIntentAtScheduleRevision(
            request,
            expectedRevision,
          );
          if (pauseClaim) {
            pauseClaim = false;
            claimed.resolve();
            await releaseClaim.promise;
          }
          return result;
        },
      };
      const firstPromise = reconcileAgentGraphSchedule({
        topology: topology(),
        controlStore: pausingStore,
        executor,
        stopController,
        newId: nextId(),
        maxNewActivations: 1,
        observeGraph: () => observation.read(),
        renderPrompt: ({ work }) => work.instruction,
      });
      await claimed.promise;

      await commitSchedule(store, 'tool-stop', {
        stop: [{ target_id: workId, reason: 'Stop before Runtime admission.' }],
      });
      const stopper = await reconcileAgentGraphSchedule({
        topology: topology(),
        controlStore: store,
        executor,
        stopController,
        newId: nextId(),
        maxNewActivations: 0,
        observeGraph: () => observation.read(),
        renderPrompt: ({ work }) => work.instruction,
      });
      releaseClaim.resolve();
      const first = await firstPromise;

      assert.equal(stopper.status, 'reconciled');
      assert.equal(stopper.stops[0]?.status, 'cancelled_before_runtime');
      assert.equal(first.status, 'reconciled');
      assert.equal(first.dispatches.length, 0);
      assert.equal(executor.backendInvocations, 0);
    } finally {
      releaseClaim.resolve();
      store.close();
    }
  });

  test('ignores unknown stop and replacement targets without poisoning later work', async () => {
    const store = createSqliteSessionMetadataStore(':memory:', { now: nextNumber(500) });
    const observation = new MemoryGraphObservation();
    const executor = new MemoryScheduleExecutor(store, observation);
    const stopController = new MemoryStopController(observation);
    try {
      await commitSchedule(store, 'tool-unknown-stop', {
        stop: [{ target_id: 'typo-target', reason: 'This target was mistyped.' }],
      });
      await commitSchedule(store, 'tool-add', {
        add_work: [
          {
            operator_id: 'writer',
            instruction: 'Continue despite stale supervisor references.',
            input_ids: [],
            replaces: 'missing-replacement-target',
          },
        ],
      });

      const result = await reconcileAgentGraphSchedule({
        topology: topology(),
        controlStore: store,
        executor,
        stopController,
        newId: nextId(),
        maxNewActivations: 1,
        observeGraph: () => observation.read(),
        renderPrompt: ({ work }) => work.instruction,
      });

      assert.equal(result.status, 'reconciled');
      assert.deepEqual(
        result.stops.map((stop) => [stop.targetId, stop.status]),
        [
          ['missing-replacement-target', 'ignored_unknown'],
          ['typo-target', 'ignored_unknown'],
        ],
      );
      assert.equal(result.failures.length, 0);
      assert.equal(result.dispatches.length, 1);
      assert.equal(executor.backendInvocations, 1);
    } finally {
      store.close();
    }
  });
});

const GRAPH_ID = 'graph-schedule';

function topology(): AgentGraphTraceTopology {
  return {
    graphId: GRAPH_ID,
    operators: [{ operatorId: 'writer', sessionId: 'session-writer' }],
    edges: [],
  };
}

async function commitSchedule(
  store: AgentGraphScheduleControlStore,
  toolCallId: string,
  input: UpdateAgentGraphToolInput,
): Promise<AgentGraphScheduleUpdateRequest> {
  const request = compileAgentGraphScheduleUpdate({
    graphId: GRAPH_ID,
    input,
    context: {
      sessionId: 'session-main',
      runId: 'run-main',
      turnId: 'turn-main',
      toolCallId,
    },
  });
  await store.commitAgentGraphScheduleUpdate(request);
  return request;
}

class MemoryGraphObservation {
  private readonly activations = new Map<
    string,
    {
      sessionId: string;
      status: 'running' | 'completed' | 'cancelled';
    }
  >();

  setActivation(
    sessionId: string,
    activationId: string,
    status: 'running' | 'completed' | 'cancelled',
  ): void {
    this.activations.set(activationId, { sessionId, status });
  }

  stopSession(sessionId: string): void {
    for (const [activationId, activation] of this.activations) {
      if (activation.sessionId === sessionId && activation.status === 'running') {
        this.activations.set(activationId, { ...activation, status: 'cancelled' });
      }
    }
  }

  async read(): Promise<AgentGraphSupervisorObservation> {
    const activationEntries = [...this.activations]
      .filter(([, activation]) => activation.sessionId === 'session-writer')
      .map(([activationId, activation]) => [
        activationId,
        {
          activationId,
          agentRunId: activationId,
          status: activation.status,
          recordCount: 1,
          firstEventTime: 1,
          lastEventTime: 2,
          lastRecordId: `record-${activationId}`,
          ...(activation.status === 'running'
            ? {}
            : { terminalRecordId: `record-${activationId}` }),
        },
      ]);
    const current = activationEntries.at(-1)?.[0] as string | undefined;
    return structuredClone({
      projection: {
        graphId: GRAPH_ID,
        operators: [{ operatorId: 'writer', sessionId: 'session-writer' }],
        ignoredPartialEvents: 0,
        records: [
          {
            schemaVersion: 1,
            recordId: 'record-input',
            graphId: GRAPH_ID,
            operatorId: 'writer',
            activationId: 'run-input',
            sessionId: 'session-writer',
            agentRunId: 'run-input',
            eventTime: 1,
            orderKey: {
              runCreatedAt: 1,
              operatorId: 'writer',
              runId: 'run-input',
              committedEventOrdinal: 0,
              runtimeEventId: 'event-input',
            },
            type: 'agent_runtime_event',
            facets: ['message'],
            supervisorSignals: [],
            source: {
              kind: 'runtime_event',
              runtimeEventId: 'event-input',
              sessionId: 'session-writer',
              runId: 'run-input',
              turnId: 'turn-input',
              ts: 1,
            },
          },
        ],
        supervisorMetaStream: [],
        state: {
          graphId: GRAPH_ID,
          appliedRecordIds: ['record-input'],
          operators: current
            ? {
                writer: {
                  operatorId: 'writer',
                  sessionId: 'session-writer',
                  status: this.activations.get(current)!.status,
                  currentActivationId: current,
                  activations: Object.fromEntries(activationEntries),
                },
              }
            : {},
        },
      },
      readiness: {
        schemaVersion: 1,
        graphId: GRAPH_ID,
        topologyFingerprint: `sha256:${'a'.repeat(64)}`,
        trace: {
          schemaVersion: 1,
          graphId: GRAPH_ID,
          topologyFingerprint: `sha256:${'a'.repeat(64)}`,
          topologicalOrder: ['writer'],
          rootOperatorIds: ['writer'],
          sinkOperatorIds: ['writer'],
          recordIds: ['record-input'],
          operators: {},
          edges: {},
          routes: [],
        },
        readiness: {},
        supervisorView: [],
      },
      claims: [],
    } as AgentGraphSupervisorObservation);
  }
}

class MemoryScheduleExecutor implements AgentGraphIntentExecutor {
  backendInvocations = 0;
  private readonly results = new Map<
    string,
    Awaited<ReturnType<AgentGraphIntentExecutor['runClaimedAgentGraphIntent']>>
  >();

  constructor(
    private readonly claims: AgentGraphIntentClaimStore,
    private readonly observation: MemoryGraphObservation,
    private readonly status: 'running' | 'completed' = 'completed',
  ) {}

  async runClaimedAgentGraphIntent(
    input: Parameters<AgentGraphIntentExecutor['runClaimedAgentGraphIntent']>[0],
  ): ReturnType<AgentGraphIntentExecutor['runClaimedAgentGraphIntent']> {
    const existing = this.results.get(input.intentId);
    if (existing) return existing;
    if (input.admitExecution && (await input.admitExecution()) === 'cancelled') {
      throw new Error('schedule execution cancelled before runtime admission');
    }
    const claim = await this.claims.readAgentGraphIntentClaim(input.graphId, input.intentId);
    if (!claim) throw new Error('missing schedule claim');
    this.backendInvocations += 1;
    this.observation.setActivation(claim.targetSessionId, claim.targetRunId, this.status);
    await input.onReady?.({
      claimId: claim.claimId,
      graphId: claim.graphId,
      intentId: claim.intentId,
      operatorId: claim.targetOperatorId,
      childSessionId: claim.targetSessionId,
      turnId: claim.targetTurnId,
      runId: claim.targetRunId,
      agentId: 'local-read',
      agentName: 'Local Read',
    });
    const result = {
      claimId: claim.claimId,
      graphId: claim.graphId,
      intentId: claim.intentId,
      operatorId: claim.targetOperatorId,
      childSessionId: claim.targetSessionId,
      turnId: claim.targetTurnId,
      runId: claim.targetRunId,
      agentId: 'local-read',
      agentName: 'Local Read',
      profile: 'local_read',
      status: this.status,
      permissionMode: 'explore' as const,
      summary: this.status,
      artifactIds: [],
      startedAt: 1,
      completedAt: 2,
      durationMs: 1,
      eventCount: 1,
    };
    this.results.set(input.intentId, result);
    return result;
  }
}

class MemoryStopController {
  readonly calls: Array<{ sessionId: string; source: 'graph_supervisor' }> = [];

  constructor(private readonly observation: MemoryGraphObservation) {}

  async stopSession(sessionId: string, input: { source: 'graph_supervisor' }): Promise<void> {
    this.calls.push({ sessionId, source: input.source });
    this.observation.stopSession(sessionId);
  }
}

function nextId(): () => string {
  let value = 0;
  return () => `schedule-id-${++value}`;
}

function nextNumber(start: number): () => number {
  let value = start;
  return () => value++;
}

function deferred<T>(): {
  promise: Promise<T>;
  resolve(value: T): void;
} {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((next) => {
    resolve = next;
  });
  return { promise, resolve };
}
