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
} from '../stream-graph-coordinator.js';
import { UPDATE_AGENT_GRAPH_TOOL_NAME } from '../stream-graph-supervisor-tools.js';

describe('host-managed agent graph coordinator', () => {
  test('boots an empty graph from agent work and recovers it without duplicate topology', async () => {
    const root = await mkdtemp(join(tmpdir(), 'maka-graph-coordinator-'));
    const sessionStore = createSessionStore(root);
    const runStore = createAgentRunStore(root);
    const runtimeEventStore = createRuntimeEventStore(root);
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
      coordinator = createCoordinator({
        sessionStore,
        runStore,
        runtimeEventStore,
        controlStore,
        manager,
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
      const childSessions = await manager.listChildSessions(rootSession.id);
      assert.equal(childSessions.length, 1);
      assert.equal(
        childSessions[0]?.subagentParent?.graph?.operatorId,
        firstProvisions[0]?.operatorId,
      );
      await assert.rejects(
        coordinator.toolsForSession(childSessions[0]!.id),
        /only to root Sessions/,
      );

      await coordinator.close();
      coordinator = undefined;
      recovered = createCoordinator({
        sessionStore,
        runStore,
        runtimeEventStore,
        controlStore,
        manager,
      });
      assert.deepEqual(await recovered.recover(), [rootSession.id]);
      assert.equal(
        (await controlStore.listAgentGraphOperatorProvisions(graphId)).length,
        1,
      );
      assert.equal((await controlStore.listAgentGraphIntentClaims(graphId)).length, 1);
      assert.equal((await manager.listChildSessions(rootSession.id)).length, 1);
    } finally {
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
): MakaToolContext {
  return {
    sessionId,
    runId,
    turnId,
    toolCallId: 'tool-call-graph-start',
    cwd: '/workspace',
    abortSignal: new AbortController().signal,
    emitOutput() {},
  };
}
