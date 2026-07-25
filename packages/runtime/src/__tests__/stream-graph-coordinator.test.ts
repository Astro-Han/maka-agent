import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { describe, test } from 'node:test';
import { randomUUID } from 'node:crypto';
import { z } from 'zod';
import {
  createAgentRunStore,
  createRuntimeEventStore,
  createSessionStore,
  createSqliteSessionMetadataStore,
  SQLITE_SESSION_METADATA_DATABASE_NAME,
} from '@maka/storage';
import { FakeBackend } from '../fake-backend.js';
import { BackendRegistry, SessionManager } from '../session-manager.js';
import type { MakaTool, MakaToolContext } from '../tool-runtime.js';
import {
  AgentGraphCoordinator,
  agentGraphIdForRootSession,
  type AgentGraphCoordinatorInput,
} from '../stream-graph-coordinator.js';
import { UPDATE_AGENT_GRAPH_TOOL_NAME } from '../stream-graph-supervisor-tools.js';

describe('host-managed agent graph coordinator', () => {
  test('boots an empty graph from agent work and recovers it without duplicate topology', async () => {
    const root = await mkdtemp(join(tmpdir(), 'maka-graph-coordinator-'));
    const sessionStore = createSessionStore(root);
    const runStore = createAgentRunStore(root);
    const runtimeEventStore = createRuntimeEventStore(root);
    let graphRuntimeHistoryReads = 0;
    const countedRuntimeEventStore = new Proxy(runtimeEventStore, {
      get(target, property) {
        if (property === 'readImmutableRuntimeEvents') {
          return async (
            ...args: Parameters<typeof target.readImmutableRuntimeEvents>
          ) => {
            graphRuntimeHistoryReads += 1;
            return target.readImmutableRuntimeEvents(...args);
          };
        }
        const value = Reflect.get(target, property, target);
        return typeof value === 'function' ? value.bind(target) : value;
      },
    });
    const backends = new BackendRegistry();
    backends.register('fake', (context) => new FakeBackend(context));
    const manager = new SessionManager({
      store: sessionStore,
      runStore,
      runtimeEventStore,
      backends,
      childTools: localReadTools(),
      newId: randomUUID,
      now: Date.now,
    });
    let controlStore:
      | ReturnType<typeof createSqliteSessionMetadataStore>
      | undefined;
    let coordinator: AgentGraphCoordinator | undefined;
    let recovered: AgentGraphCoordinator | undefined;
    let delayedControlStore:
      | ReturnType<typeof createDelayableControlStore>
      | undefined;
    const clientEvents: string[] = [];
    let unsubscribe: (() => void) | undefined;
    try {
      const rootSession = await manager.createSession({
        cwd: root,
        backend: 'fake',
        llmConnectionSlug: 'fake',
        permissionMode: 'ask',
        name: 'Graph supervisor',
      });
      const sourceTurnId = randomUUID();
      for await (const _event of manager.sendMessage(rootSession.id, {
        turnId: sourceTurnId,
        text: 'Prepare graph work.',
      })) {
        // Drain the ordinary root turn so its source AgentRun is durable.
      }
      const sourceRun = (await runStore.listSessionRuns(rootSession.id)).find(
        (run) => run.turnId === sourceTurnId,
      );
      assert.ok(sourceRun);

      controlStore = createSqliteSessionMetadataStore(
        join(root, SQLITE_SESSION_METADATA_DATABASE_NAME),
      );
      delayedControlStore = createDelayableControlStore(controlStore);
      coordinator = createCoordinator({
        sessionStore,
        runStore,
        runtimeEventStore: countedRuntimeEventStore,
        controlStore: delayedControlStore.store,
        manager,
      });
      unsubscribe = coordinator.subscribe(rootSession.id, (event) => {
        assert.equal(event.rootSessionId, rootSession.id);
        clientEvents.push(event.reason);
      });
      const tools = await coordinator.toolsForSession(rootSession.id);
      const update = tools.find((tool) => tool.name === UPDATE_AGENT_GRAPH_TOOL_NAME);
      assert.ok(update);
      const graphId = agentGraphIdForRootSession(rootSession.id);
      await assert.rejects(
        async () =>
          await update.impl(
            {
              add_work: [
                {
                  agent_id: 'local-read',
                  instruction: 'This request must not become durable.',
                  input_ids: [],
                },
              ],
            },
            toolContext('different-root', sourceRun.runId, sourceTurnId),
          ),
        /not authorized/,
      );
      assert.equal(
        (await controlStore.listAgentGraphScheduleUpdates(graphId)).length,
        0,
      );
      await update.impl(
        {
          add_work: [
            {
              agent_id: 'local-read',
              instruction: 'Inspect the repository and report one concrete finding.',
              input_ids: [],
            },
          ],
        },
        toolContext(rootSession.id, sourceRun.runId, sourceTurnId),
      );
      await coordinator.waitForIdle(rootSession.id);

      const firstProvisions =
        await controlStore.listAgentGraphOperatorProvisions(graphId);
      assert.equal(firstProvisions.length, 1);
      assert.equal(firstProvisions[0]?.agentId, 'local-read');
      assert.equal((await controlStore.listAgentGraphIntentClaims(graphId)).length, 1);
      assert.equal((await coordinator.observe(rootSession.id)).projection.operators.length, 1);
      const historyReadsBeforeClient = graphRuntimeHistoryReads;
      const childSessions = await manager.listChildSessions(rootSession.id);
      assert.equal(childSessions.length, 1);
      assert.equal(
        childSessions[0]?.subagentParent?.graph?.operatorId,
        firstProvisions[0]?.operatorId,
      );
      const snapshot = await coordinator.getSnapshot(rootSession.id);
      assert.equal(snapshot.rootSessionId, rootSession.id);
      assert.equal(snapshot.graphId, graphId);
      assert.equal(snapshot.scheduleRevision, 1);
      assert.equal(snapshot.operators.length, 1);
      assert.equal(snapshot.operators[0]?.childSessionId, childSessions[0]?.id);
      assert.equal(snapshot.operators[0]?.agentId, 'local-read');
      assert.equal(snapshot.work[0]?.instructionPreview, 'Inspect the repository and report one concrete finding.');
      assert.match(snapshot.snapshotVersion, /^sha256:[a-f0-9]{64}$/);
      const inspection = await coordinator.inspectOperator(
        rootSession.id,
        firstProvisions[0]!.operatorId,
      );
      assert.equal(inspection.operator.childSessionId, childSessions[0]?.id);
      assert.equal(inspection.claims.length, 1);
      assert.equal(inspection.activations.length, 1);
      assert.equal(
        graphRuntimeHistoryReads,
        historyReadsBeforeClient,
        'client reads must use the materialized SQLite projection',
      );
      await new Promise<void>((resolve) => setImmediate(resolve));
      assert.ok(clientEvents.includes('reconciled'));
      assert.equal(
        clientEvents.filter((reason) => reason === 'runtime_activity').length,
        2,
        'partial text deltas must not cause projection writes or invalidations',
      );
      await assert.rejects(
        coordinator.toolsForSession(childSessions[0]!.id),
        /only to root Sessions/,
      );

      await coordinator.close();
      unsubscribe = undefined;
      coordinator = undefined;
      recovered = createCoordinator({
        sessionStore,
        runStore,
        runtimeEventStore: countedRuntimeEventStore,
        controlStore: delayedControlStore.store,
        manager,
      });
      assert.deepEqual(await recovered.recover(), [rootSession.id]);
      assert.equal(
        (await controlStore.listAgentGraphOperatorProvisions(graphId)).length,
        1,
      );
      assert.equal((await controlStore.listAgentGraphIntentClaims(graphId)).length, 1);
      assert.equal((await manager.listChildSessions(rootSession.id)).length, 1);

      const recoveredTools = await recovered.toolsForSession(rootSession.id);
      const delayedUpdate = recoveredTools.find(
        (tool) => tool.name === UPDATE_AGENT_GRAPH_TOOL_NAME,
      );
      assert.ok(delayedUpdate);
      const gate = delayedControlStore.holdNextCommit();
      const pendingUpdate = Promise.resolve(
        delayedUpdate.impl(
          {
            add_work: [
              {
                agent_id: 'local-read',
                instruction: 'This pre-stop update must remain paused.',
                input_ids: [],
              },
            ],
          },
          toolContext(
            rootSession.id,
            sourceRun.runId,
            sourceTurnId,
            'tool-call-graph-stop-race',
          ),
        ),
      );
      await gate.started;
      try {
        await recovered.stop(rootSession.id);
      } finally {
        gate.release();
      }
      await pendingUpdate;
      await recovered.waitForIdle(rootSession.id);
      assert.equal(
        (await controlStore.listAgentGraphOperatorProvisions(graphId)).length,
        1,
        'a schedule commit authorized before stop must not restart reconciliation',
      );
    } finally {
      unsubscribe?.();
      await coordinator?.close();
      await recovered?.close();
      controlStore?.close();
      await sessionStore.close?.();
      await rm(root, { recursive: true, force: true });
    }
  });

  test('derives stable graph identity from the root Session', async () => {
    const graphId = agentGraphIdForRootSession('root-session');
    assert.match(graphId, /^agent_graph_[a-f0-9]{32}$/);
    assert.equal(graphId, agentGraphIdForRootSession('root-session'));
    assert.notEqual(graphId, agentGraphIdForRootSession('other-root'));
  });

  test('keeps completed graph snapshots readable after the root Session is archived', async () => {
    let projection:
      | {
          schemaVersion: 1;
          graphId: string;
          rootSessionId: string;
          snapshotVersion: string;
          payload: unknown;
          materializedAt: number;
        }
      | undefined;
    const coordinator = new AgentGraphCoordinator({
      sessionStore: {
        listForRecovery: async () => [],
        readHeader: async (sessionId: string) =>
          ({
            id: sessionId,
            status: 'archived',
            isArchived: true,
          }) as never,
      },
      runStore: { listSessionRuns: async () => [] },
      runtimeEventStore: { readImmutableRuntimeEvents: async () => [] },
      controlStore: {
        listAgentGraphOperatorProvisions: async () => [],
        listAgentGraphScheduleUpdates: async () => [],
        listAgentGraphIntentClaims: async () => [],
        listAgentGraphClientClaimAdmissions: async () => [],
        readAgentGraphClientProjection: async () => projection,
        commitAgentGraphClientProjection: async (request: {
          graphId: string;
          rootSessionId: string;
          snapshotVersion: string;
          snapshot: unknown;
        }) => {
          projection = {
            schemaVersion: 1,
            graphId: request.graphId,
            rootSessionId: request.rootSessionId,
            snapshotVersion: request.snapshotVersion,
            payload: request.snapshot,
            materializedAt: 1,
          };
          return projection;
        },
        readAgentGraphClientOperatorProjection: async () => undefined,
        listAgentGraphClientTerminalActivities: async () => ({
          records: [],
          hasMore: false,
        }),
      },
      runtime: {},
      newId: randomUUID,
      rootSessionId: 'root-session',
    } as unknown as AgentGraphCoordinatorInput);
    const snapshot = await coordinator.getSnapshot('root-session');
    assert.equal(snapshot.status, 'empty');
    await assert.rejects(
      coordinator.toolsForSession('root-session'),
      /Archived Sessions cannot supervise/,
    );
    await coordinator.close();
  });

  test('surfaces topology cleanup failures from close', async () => {
    const topologyFailure = new Error('graph topology unavailable');
    const coordinator = new AgentGraphCoordinator({
      sessionStore: {
        listForRecovery: async () => [],
        readHeader: async (sessionId: string) =>
          ({
            id: sessionId,
            status: 'active',
            isArchived: false,
          }) as never,
      },
      runStore: { listSessionRuns: async () => [] },
      runtimeEventStore: { readImmutableRuntimeEvents: async () => [] },
      controlStore: {
        listAgentGraphOperatorProvisions: async () => {
          throw topologyFailure;
        },
      },
      runtime: {},
      newId: randomUUID,
      rootSessionId: 'root-session',
    } as unknown as AgentGraphCoordinatorInput);
    await assert.rejects(
      coordinator.toolsForSession('another-root'),
      /scoped to root Session root-session/,
    );
    await coordinator.toolsForSession('root-session');
    await assert.rejects(
      coordinator.close(),
      (error: unknown) =>
        error === topologyFailure ||
        (error instanceof AggregateError &&
          error.errors.some(
            (failure) =>
              failure === topologyFailure ||
              (failure instanceof AggregateError && failure.errors.includes(topologyFailure)),
      )),
    );
  });

  test('attempts every operator stop and surfaces all close failures', async () => {
    const graphId = agentGraphIdForRootSession('root-session');
    const stopped: string[] = [];
    const coordinator = new AgentGraphCoordinator({
      sessionStore: {
        listForRecovery: async () => [],
        readHeader: async (sessionId: string) =>
          ({
            id: sessionId,
            status: 'active',
            isArchived: false,
          }) as never,
      },
      runStore: { listSessionRuns: async () => [] },
      runtimeEventStore: { readImmutableRuntimeEvents: async () => [] },
      controlStore: {
        listAgentGraphOperatorProvisions: async () =>
          ['a', 'b'].map((suffix, index) => ({
            graphId,
            provisionId: `provision-${suffix}`,
            operatorId: `operator-${suffix}`,
            targetSessionId: `child-${suffix}`,
            provisionedAt: index,
            edges: [],
          })),
      },
      runtime: {
        stopSession: async (sessionId: string) => {
          stopped.push(sessionId);
          throw new Error(`cannot stop ${sessionId}`);
        },
      },
      newId: randomUUID,
      rootSessionId: 'root-session',
    } as unknown as AgentGraphCoordinatorInput);
    await coordinator.toolsForSession('root-session');
    await assert.rejects(
      coordinator.close(),
      (error: unknown) =>
        error instanceof AggregateError &&
        (error.errors.length === 2 ||
          error.errors.some(
            (failure) =>
              failure instanceof AggregateError &&
              failure.errors.length === 2,
          )),
    );
    assert.deepEqual(stopped.sort(), ['child-a', 'child-b']);
  });
});

function createCoordinator(input: {
  sessionStore: ReturnType<typeof createSessionStore>;
  runStore: ReturnType<typeof createAgentRunStore>;
  runtimeEventStore: ReturnType<typeof createRuntimeEventStore>;
  controlStore: ReturnType<typeof createSqliteSessionMetadataStore>;
  manager: SessionManager;
}): AgentGraphCoordinator {
  return new AgentGraphCoordinator({
    ...input,
    runtime: input.manager,
    newId: randomUUID,
    maxNewActivations: 4,
  });
}

function createDelayableControlStore(
  store: ReturnType<typeof createSqliteSessionMetadataStore>,
): {
  store: ReturnType<typeof createSqliteSessionMetadataStore>;
  holdNextCommit(): { started: Promise<void>; release(): void };
} {
  let gate:
    | {
        started(): void;
        release: Promise<void>;
      }
    | undefined;
  const proxy = new Proxy(store, {
    get(target, property) {
      if (property === 'commitAgentGraphScheduleUpdate') {
        return async (...args: Parameters<typeof target.commitAgentGraphScheduleUpdate>) => {
          const activeGate = gate;
          gate = undefined;
          if (activeGate) {
            activeGate.started();
            await activeGate.release;
          }
          return target.commitAgentGraphScheduleUpdate(...args);
        };
      }
      const value = Reflect.get(target, property, target);
      return typeof value === 'function' ? value.bind(target) : value;
    },
  });
  return {
    store: proxy,
    holdNextCommit() {
      if (gate) throw new Error('A delayed graph schedule commit is already pending');
      let markStarted!: () => void;
      let release!: () => void;
      const started = new Promise<void>((resolve) => {
        markStarted = resolve;
      });
      const released = new Promise<void>((resolve) => {
        release = resolve;
      });
      gate = { started: markStarted, release: released };
      return { started, release };
    },
  };
}

function localReadTools(): MakaTool[] {
  return ['Read', 'Glob', 'Grep'].map((name) => ({
    name,
    displayName: name,
    description: `${name} fixture`,
    parameters: z.object({}).passthrough(),
    permissionRequired: false,
    categoryHint: 'read',
    impl: async () => ({ ok: true }),
  }));
}

function toolContext(
  sessionId: string,
  runId: string,
  turnId: string,
  toolCallId = 'tool-call-graph-start',
): MakaToolContext {
  return {
    sessionId,
    runId,
    turnId,
    toolCallId,
    cwd: '/workspace',
    abortSignal: new AbortController().signal,
    emitOutput() {},
  };
}
