import type {
  AgentGraphIntentClaim,
  AgentGraphScheduleControlStore,
  AgentGraphScheduleUpdate,
  AgentGraphOperatorProvision,
  AgentRunStore,
  RuntimeEventStore,
  SessionHeader,
} from '@maka/core';
import { decodeAgentGraphIntentClaim } from '@maka/core';
import type { MakaTool } from './tool-runtime.js';
import type { SessionManager } from './session-manager.js';
import {
  readCommittedAgentGraphProjection,
  type AgentGraphRecord,
} from './stream-graph-projection.js';
import {
  buildAgentGraphReadinessSnapshot,
  type AgentGraphReadinessPolicy,
} from './stream-graph-readiness.js';
import type {
  AgentGraphSupervisorObservation,
  AgentGraphSupervisorObserver,
} from './stream-graph-dispatch.js';
import {
  reconcileAgentGraphSchedule,
  type AgentGraphScheduleReconciliationResult,
  type RenderAgentGraphScheduledWorkPromptInput,
} from './stream-graph-schedule-reconcile.js';
import { buildAgentGraphSupervisorTools } from './stream-graph-supervisor-tools.js';
import type { AgentGraphTraceTopology } from './stream-graph-trace.js';
import { stableHash } from './request-shape.js';

const DEFAULT_MAX_NEW_ACTIVATIONS = 8;

export interface AgentGraphCoordinatorSessionStore {
  listForRecovery(): Promise<SessionHeader[]>;
  readHeader(sessionId: string): Promise<SessionHeader>;
}

export interface AgentGraphCoordinatorRuntime {
  provisionAgentGraphOperator: SessionManager['provisionAgentGraphOperator'];
  runClaimedAgentGraphIntent: SessionManager['runClaimedAgentGraphIntent'];
  stopSession: SessionManager['stopSession'];
}

export interface AgentGraphCoordinatorInput {
  sessionStore: AgentGraphCoordinatorSessionStore;
  runStore: Pick<AgentRunStore, 'listSessionRuns'>;
  runtimeEventStore: Pick<RuntimeEventStore, 'readImmutableRuntimeEvents'>;
  controlStore: AgentGraphScheduleControlStore;
  runtime: AgentGraphCoordinatorRuntime;
  newId: () => string;
  maxNewActivations?: number;
  resolvePolicies?(
    topology: AgentGraphTraceTopology,
    observation: Pick<AgentGraphSupervisorObservation, 'projection' | 'claims'>,
  ): readonly AgentGraphReadinessPolicy[] | Promise<readonly AgentGraphReadinessPolicy[]>;
  renderPrompt?(
    input: RenderAgentGraphScheduledWorkPromptInput,
  ): string | Promise<string>;
  supervisor?: AgentGraphSupervisorObserver;
  onReconciliation?(
    rootSessionId: string,
    result: AgentGraphScheduleReconciliationResult,
  ): void | Promise<void>;
  onError?(rootSessionId: string, error: unknown): void | Promise<void>;
}

interface GraphDriver {
  rootSessionId: string;
  graphId: string;
  requested: boolean;
  paused: boolean;
  closed: boolean;
  abortController?: AbortController;
  task?: Promise<void>;
  lastResult?: AgentGraphScheduleReconciliationResult;
  lastError?: unknown;
}

/**
 * Process-local execution authority for Session-backed agent graphs.
 *
 * Durable schedule/topology/claim rows and Runtime facts remain the recovery
 * authority. This coordinator owns only single-flight wakeups and cancellation
 * handles, so recreating it after a process restart is safe.
 */
export class AgentGraphCoordinator {
  readonly #input: AgentGraphCoordinatorInput;
  readonly #drivers = new Map<string, GraphDriver>();
  #closed = false;

  constructor(input: AgentGraphCoordinatorInput) {
    const maxNewActivations = input.maxNewActivations ?? DEFAULT_MAX_NEW_ACTIVATIONS;
    if (!Number.isSafeInteger(maxNewActivations) || maxNewActivations < 0) {
      throw new Error('Agent graph coordinator activation limit must be a non-negative integer');
    }
    this.#input = { ...input, maxNewActivations };
  }

  /**
   * Return the supervisor-only tools for an ordinary root Session.
   *
   * Child Sessions are graph operators and never receive this control surface.
   */
  async toolsForSession(rootSessionId: string): Promise<MakaTool[]> {
    await this.#assertRootSupervisor(rootSessionId);
    const driver = this.#driver(rootSessionId);
    return buildAgentGraphSupervisorTools({
      graphId: driver.graphId,
      scheduleStore: this.#input.controlStore,
      observeGraph: () => this.observe(rootSessionId),
      authorizeScheduleUpdate: (request) => {
        if (
          request.graphId !== driver.graphId ||
          request.source.sessionId !== rootSessionId
        ) {
          throw new Error(
            `Agent graph schedule update is not authorized for root Session ${rootSessionId}`,
          );
        }
      },
      onScheduleUpdateCommitted: (update) => {
        this.#assertScheduleOwnedByRoot(update, rootSessionId, driver.graphId);
        this.wake(rootSessionId);
      },
    });
  }

  /** Wake reconciliation without making the caller part of the data path. */
  wake(rootSessionId: string): void {
    if (this.#closed) return;
    const driver = this.#driver(rootSessionId);
    driver.paused = false;
    driver.requested = true;
    if (!driver.task) {
      driver.task = this.#drive(driver).finally(() => {
        driver.task = undefined;
        if (driver.requested && !driver.closed && !this.#closed) this.wake(rootSessionId);
      });
    }
  }

  /** Reconcile now and surface any host-level failure to explicit callers. */
  async reconcile(rootSessionId: string): Promise<AgentGraphScheduleReconciliationResult> {
    await this.#assertRootSupervisor(rootSessionId);
    const driver = this.#driver(rootSessionId);
    driver.lastError = undefined;
    this.wake(rootSessionId);
    await this.waitForIdle(rootSessionId);
    if (driver.lastError !== undefined) throw driver.lastError;
    if (!driver.lastResult) {
      throw new Error(`Agent graph ${driver.graphId} produced no reconciliation result`);
    }
    return driver.lastResult;
  }

  async waitForIdle(rootSessionId: string): Promise<void> {
    const driver = this.#driver(rootSessionId);
    while (driver.task) await driver.task;
  }

  /**
   * Rebuild every durable, non-archived root graph that has schedule intent.
   *
   * Empty ordinary Sessions are skipped; no separate in-memory registry is
   * required for restart recovery.
   */
  async recover(): Promise<string[]> {
    const recovered: string[] = [];
    for (const header of await this.#input.sessionStore.listForRecovery()) {
      if (header.subagentParent || header.isArchived || header.status === 'archived') continue;
      const graphId = agentGraphIdForRootSession(header.id);
      const updates = await this.#input.controlStore.listAgentGraphScheduleUpdates(graphId);
      if (updates.length === 0) continue;
      updates.forEach((update) => this.#assertScheduleOwnedByRoot(update, header.id, graphId));
      await this.reconcile(header.id);
      recovered.push(header.id);
    }
    return recovered;
  }

  async observe(rootSessionId: string): Promise<AgentGraphSupervisorObservation> {
    await this.#assertRootSupervisor(rootSessionId);
    const graphId = agentGraphIdForRootSession(rootSessionId);
    const topology = await this.#readTopology(graphId);
    return this.#observeTopology(topology);
  }

  async #observeTopology(
    topology: AgentGraphTraceTopology,
  ): Promise<AgentGraphSupervisorObservation> {
    const graphId = topology.graphId;
    const [projection, listedClaims] = await Promise.all([
      readCommittedAgentGraphProjection({
        graphId,
        operators: topology.operators,
        runStore: this.#input.runStore,
        runtimeEventStore: this.#input.runtimeEventStore,
      }),
      this.#input.controlStore.listAgentGraphIntentClaims(graphId),
    ]);
    const claims = listedClaims
      .map(decodeAgentGraphIntentClaim)
      .sort((a, b) => a.intentId.localeCompare(b.intentId) || a.claimId.localeCompare(b.claimId));
    assertUniqueClaims(graphId, claims);
    const policies =
      (await this.#input.resolvePolicies?.(topology, { projection, claims })) ?? [];
    return {
      projection,
      readiness: buildAgentGraphReadinessSnapshot({
        topology,
        records: projection.records,
        policies,
      }),
      claims,
    };
  }

  /**
   * Stop current graph execution. Durable schedule facts are retained, so a
   * later supervisor update can wake the same graph again.
   */
  async stop(rootSessionId: string): Promise<void> {
    await this.#assertRootSupervisor(rootSessionId);
    const driver = this.#driver(rootSessionId);
    driver.paused = true;
    driver.requested = false;
    driver.abortController?.abort();
    const topology = await this.#readTopology(driver.graphId);
    const stopped = await Promise.allSettled(
      topology.operators.map((operator) =>
        this.#input.runtime.stopSession(operator.sessionId, { source: 'graph_supervisor' }),
      ),
    );
    await this.waitForIdle(rootSessionId);
    const failure = stopped.find(
      (result): result is PromiseRejectedResult => result.status === 'rejected',
    );
    if (failure) throw failure.reason;
  }

  async close(): Promise<void> {
    if (this.#closed) return;
    this.#closed = true;
    for (const driver of this.#drivers.values()) {
      driver.closed = true;
      driver.abortController?.abort();
    }
    await Promise.allSettled(
      [...this.#drivers.values()].map(async (driver) => {
        const topology = await this.#readTopology(driver.graphId);
        await Promise.allSettled(
          topology.operators.map((operator) =>
            this.#input.runtime.stopSession(operator.sessionId, {
              source: 'graph_supervisor',
            }),
          ),
        );
        if (driver.task) await driver.task;
      }),
    );
  }

  async #drive(driver: GraphDriver): Promise<void> {
    while (driver.requested && !driver.paused && !driver.closed && !this.#closed) {
      driver.requested = false;
      driver.lastError = undefined;
      const abortController = new AbortController();
      driver.abortController = abortController;
      try {
        const result = await this.#reconcileOnce(driver, abortController.signal);
        driver.lastResult = result;
        await notify(this.#input.onReconciliation, driver.rootSessionId, result);
      } catch (error) {
        driver.lastError = error;
        await notify(this.#input.onError, driver.rootSessionId, error);
      } finally {
        if (driver.abortController === abortController) driver.abortController = undefined;
      }
    }
  }

  async #reconcileOnce(
    driver: GraphDriver,
    abortSignal: AbortSignal,
  ): Promise<AgentGraphScheduleReconciliationResult> {
    await this.#assertRootSupervisor(driver.rootSessionId);
    const updates = await this.#input.controlStore.listAgentGraphScheduleUpdates(driver.graphId);
    updates.forEach((update) =>
      this.#assertScheduleOwnedByRoot(update, driver.rootSessionId, driver.graphId),
    );
    return reconcileAgentGraphSchedule({
      topology: { graphId: driver.graphId, operators: [], edges: [] },
      controlStore: this.#input.controlStore,
      executor: this.#input.runtime,
      stopController: this.#input.runtime,
      provisionOperator: (input) => this.#input.runtime.provisionAgentGraphOperator(input),
      newId: this.#input.newId,
      maxNewActivations: this.#input.maxNewActivations!,
      observeGraph: (topology) => this.#observeTopology(topology),
      renderPrompt: this.#input.renderPrompt ?? renderDefaultScheduledWorkPrompt,
      abortSignal,
      supervisor: {
        onObservation: this.#input.supervisor?.onObservation,
        onActivationReady: this.#input.supervisor?.onActivationReady,
        onRuntimeEvent: (event) => {
          if (!driver.paused) driver.requested = true;
          void notify(this.#input.supervisor?.onRuntimeEvent, event);
        },
      },
    });
  }

  async #readTopology(graphId: string): Promise<AgentGraphTraceTopology> {
    return topologyFromProvisions(
      graphId,
      await this.#input.controlStore.listAgentGraphOperatorProvisions(graphId),
    );
  }

  async #assertRootSupervisor(rootSessionId: string): Promise<SessionHeader> {
    if (this.#closed) throw new Error('Agent graph coordinator is closed');
    const header = await this.#input.sessionStore.readHeader(rootSessionId);
    if (header.id !== rootSessionId) {
      throw new Error(`Session store returned ${header.id}, expected ${rootSessionId}`);
    }
    if (header.subagentParent) {
      throw new Error('Agent graph supervisor tools are available only to root Sessions');
    }
    if (header.isArchived || header.status === 'archived') {
      throw new Error('Archived Sessions cannot supervise an agent graph');
    }
    return header;
  }

  #assertScheduleOwnedByRoot(
    update: AgentGraphScheduleUpdate,
    rootSessionId: string,
    graphId: string,
  ): void {
    if (update.graphId !== graphId || update.source.sessionId !== rootSessionId) {
      throw new Error(
        `Agent graph schedule ${update.updateId} is not owned by root Session ${rootSessionId}`,
      );
    }
  }

  #driver(rootSessionId: string): GraphDriver {
    const graphId = agentGraphIdForRootSession(rootSessionId);
    const existing = this.#drivers.get(graphId);
    if (existing) {
      if (existing.rootSessionId !== rootSessionId) {
        throw new Error(`Agent graph ${graphId} is already bound to another root Session`);
      }
      return existing;
    }
    const created: GraphDriver = {
      rootSessionId,
      graphId,
      requested: false,
      paused: false,
      closed: false,
    };
    this.#drivers.set(graphId, created);
    return created;
  }
}

export function agentGraphIdForRootSession(rootSessionId: string): string {
  const normalized = rootSessionId.trim();
  if (!normalized || normalized !== rootSessionId) {
    throw new Error('Agent graph root Session id must be a non-empty canonical identity');
  }
  const suffix = stableHash({
    schemaVersion: 1,
    rootSessionId,
  }).slice('sha256:'.length, 'sha256:'.length + 32);
  return `agent_graph_${suffix}`;
}

export function topologyFromProvisions(
  graphId: string,
  provisions: readonly AgentGraphOperatorProvision[],
): AgentGraphTraceTopology {
  const operators = new Map<string, { operatorId: string; sessionId: string }>();
  const sessions = new Map<string, string>();
  const edges = new Map<
    string,
    { edgeId: string; fromOperatorId: string; toOperatorId: string }
  >();
  for (const provision of [...provisions].sort(
    (a, b) => a.provisionedAt - b.provisionedAt || a.provisionId.localeCompare(b.provisionId),
  )) {
    if (provision.graphId !== graphId) {
      throw new Error(`Graph provision ${provision.provisionId} belongs to ${provision.graphId}`);
    }
    const existingOperator = operators.get(provision.operatorId);
    if (
      existingOperator &&
      existingOperator.sessionId !== provision.targetSessionId
    ) {
      throw new Error(`Graph operator ${provision.operatorId} has conflicting Session bindings`);
    }
    const existingSession = sessions.get(provision.targetSessionId);
    if (existingSession && existingSession !== provision.operatorId) {
      throw new Error(`Graph Session ${provision.targetSessionId} has multiple operators`);
    }
    operators.set(provision.operatorId, {
      operatorId: provision.operatorId,
      sessionId: provision.targetSessionId,
    });
    sessions.set(provision.targetSessionId, provision.operatorId);
    for (const edge of provision.edges) {
      const existingEdge = edges.get(edge.edgeId);
      if (
        existingEdge &&
        (existingEdge.fromOperatorId !== edge.fromOperatorId ||
          existingEdge.toOperatorId !== edge.toOperatorId)
      ) {
        throw new Error(`Graph edge ${edge.edgeId} has conflicting endpoints`);
      }
      edges.set(edge.edgeId, { ...edge });
    }
  }
  return {
    graphId,
    operators: [...operators.values()].sort((a, b) => a.operatorId.localeCompare(b.operatorId)),
    edges: [...edges.values()].sort((a, b) => a.edgeId.localeCompare(b.edgeId)),
  };
}

function renderDefaultScheduledWorkPrompt(
  input: RenderAgentGraphScheduledWorkPromptInput,
): string {
  if (input.inputRecords.length === 0) return input.work.instruction;
  const references = input.inputRecords.map((record) => graphRecordReference(record));
  return `${input.work.instruction}\n\nCommitted graph input references:\n${JSON.stringify(references, null, 2)}`;
}

function graphRecordReference(record: AgentGraphRecord): object {
  return {
    recordId: record.recordId,
    operatorId: record.operatorId,
    activationId: record.activationId,
    facets: [...record.facets],
    source: { ...record.source },
  };
}

function assertUniqueClaims(graphId: string, claims: readonly AgentGraphIntentClaim[]): void {
  const intentIds = new Set<string>();
  for (const claim of claims) {
    if (claim.graphId !== graphId) {
      throw new Error(`Graph claim ${claim.claimId} belongs to ${claim.graphId}`);
    }
    if (intentIds.has(claim.intentId)) {
      throw new Error(`Graph ${graphId} contains duplicate claim intent ${claim.intentId}`);
    }
    intentIds.add(claim.intentId);
  }
}

function notify<T extends unknown[]>(
  callback: ((...args: T) => void | Promise<void>) | undefined,
  ...args: T
): Promise<void> {
  if (!callback) return Promise.resolve();
  return Promise.resolve()
    .then(() => callback(...args))
    .then(
      () => {},
      () => {},
    );
}
