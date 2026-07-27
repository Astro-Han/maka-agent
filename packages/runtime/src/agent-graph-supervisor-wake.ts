import {
  AGENT_GRAPH_SUPERVISOR_WAKE_SCHEMA_VERSION,
  type AgentGraphSupervisorWakeRecord,
  type AgentGraphSupervisorWakeStore,
  type UserMessageInput,
} from '@maka/core';
import type {
  GoalTurnOutcome,
  SessionActivityLease,
  SessionActivityRegistry,
} from './goal-turn-lifecycle.js';
import type { AgentGraphClientSnapshot } from './stream-graph-read-model.js';
import type { AgentGraphScheduleReconciliationResult } from './stream-graph-schedule-reconcile.js';

const DEFAULT_MAX_DELIVERY_ATTEMPTS = 3;

export interface AgentGraphSupervisorWakeInput {
  activityRegistry: SessionActivityRegistry;
  wakeStore: AgentGraphSupervisorWakeStore;
  readSnapshot(rootSessionId: string): Promise<AgentGraphClientSnapshot>;
  startTurn(
    sessionId: string,
    input: UserMessageInput,
    activity: SessionActivityLease,
  ): Promise<GoalTurnOutcome>;
  newId(): string;
  maxDeliveryAttempts?: number;
  onError?(rootSessionId: string, error: unknown): void | Promise<void>;
}

/**
 * Host-side control-plane bridge from graph quiescence back to the root
 * supervisor Agent.
 *
 * SQLite owns wake admission and delivery state. A persisted prompt or Run is
 * only an attempt: the wake becomes delivered after the host observes a
 * completed root turn. Interrupted attempts remain retryable across callbacks
 * and process recovery.
 */
export class AgentGraphSupervisorWakeCoordinator {
  readonly #input: AgentGraphSupervisorWakeInput;
  readonly #tasks = new Set<Promise<void>>();
  readonly #pendingWakeIds = new Set<string>();
  readonly #abortController = new AbortController();
  readonly #maxDeliveryAttempts: number;
  #closed = false;

  constructor(input: AgentGraphSupervisorWakeInput) {
    this.#input = input;
    this.#maxDeliveryAttempts = input.maxDeliveryAttempts ?? DEFAULT_MAX_DELIVERY_ATTEMPTS;
    if (!Number.isSafeInteger(this.#maxDeliveryAttempts) || this.#maxDeliveryAttempts < 1) {
      throw new Error('Agent graph supervisor wake attempts must be a positive safe integer');
    }
  }

  notify(rootSessionId: string, result: AgentGraphScheduleReconciliationResult): void {
    if (this.#closed || !isSupervisorMilestone(result)) return;
    const task = this.#wake(rootSessionId, result).catch((error) => {
      if (!this.#closed && !isAbortError(error)) {
        return notifyError(this.#input.onError, rootSessionId, error);
      }
    });
    this.#tasks.add(task);
    void task.finally(() => this.#tasks.delete(task));
  }

  /** Makes crash-interrupted attempts eligible and actively resumes them. */
  async recover(): Promise<number> {
    if (this.#closed) return 0;
    const recovered = await this.#input.wakeStore.recoverAgentGraphSupervisorWakes();
    if (this.#closed) return recovered;
    for (const wake of await this.#input.wakeStore.listRetryableAgentGraphSupervisorWakes()) {
      this.#scheduleRecoveredWake(wake);
    }
    return recovered;
  }

  async waitForIdle(): Promise<void> {
    while (this.#tasks.size > 0) await Promise.all([...this.#tasks]);
  }

  async close(): Promise<void> {
    if (this.#closed) return;
    this.#closed = true;
    this.#abortController.abort();
    await this.waitForIdle();
  }

  async #wake(
    rootSessionId: string,
    result: AgentGraphScheduleReconciliationResult,
  ): Promise<void> {
    const snapshot = await this.#input.readSnapshot(rootSessionId);
    if (this.#closed || snapshot.closed || snapshot.scheduleRevision === 0) return;
    const wakeId = `${snapshot.graphId}:${snapshot.snapshotVersion}`;
    if (this.#pendingWakeIds.has(wakeId)) return;
    this.#pendingWakeIds.add(wakeId);
    try {
      const claimed = await this.#input.wakeStore.claimAgentGraphSupervisorWake({
        schemaVersion: AGENT_GRAPH_SUPERVISOR_WAKE_SCHEMA_VERSION,
        graphId: snapshot.graphId,
        wakeId,
        snapshotVersion: snapshot.snapshotVersion,
        rootSessionId,
      });
      if (this.#closed || claimed.wake.status === 'delivered') return;
      await this.#deliverWake(claimed.wake, snapshot, result);
    } finally {
      this.#pendingWakeIds.delete(wakeId);
    }
  }

  #scheduleRecoveredWake(wake: AgentGraphSupervisorWakeRecord): void {
    if (this.#closed || this.#pendingWakeIds.has(wake.wakeId)) return;
    this.#pendingWakeIds.add(wake.wakeId);
    const task = this.#resumeWake(wake)
      .catch((error) => {
        if (!this.#closed && !isAbortError(error)) {
          return notifyError(this.#input.onError, wake.rootSessionId, error);
        }
      })
      .finally(() => this.#pendingWakeIds.delete(wake.wakeId));
    this.#tasks.add(task);
    void task.finally(() => this.#tasks.delete(task));
  }

  async #resumeWake(wake: AgentGraphSupervisorWakeRecord): Promise<void> {
    const snapshot = await this.#input.readSnapshot(wake.rootSessionId);
    if (
      this.#closed ||
      snapshot.closed ||
      snapshot.graphId !== wake.graphId ||
      snapshot.scheduleRevision === 0
    ) {
      return;
    }
    await this.#deliverWake(wake, snapshot);
  }

  async #deliverWake(
    wake: AgentGraphSupervisorWakeRecord,
    snapshot: AgentGraphClientSnapshot,
    result?: AgentGraphScheduleReconciliationResult,
  ): Promise<void> {
    let lastFailure: string | undefined;
    for (let index = 0; index < this.#maxDeliveryAttempts; index += 1) {
      const activity = await this.#input.activityRegistry.acquire(
        wake.rootSessionId,
        this.#abortController.signal,
      );
      try {
        if (this.#closed) return;
        const attemptId = this.#input.newId();
        const turnId = this.#input.newId();
        const admission = await this.#input.wakeStore.beginAgentGraphSupervisorWakeAttempt({
          graphId: wake.graphId,
          wakeId: wake.wakeId,
          attemptId,
          turnId,
        });
        if (!admission.acquired) return;
        if (this.#closed) {
          await this.#markRetryable(wake.graphId, wake.wakeId, attemptId, 'host_shutdown');
          return;
        }

        try {
          const outcome = await this.#input.startTurn(
            wake.rootSessionId,
            {
              turnId,
              text: renderAgentGraphSupervisorWakePrompt(snapshot, result),
              displayText: 'Agent graph reached a supervisor checkpoint.',
              turnOrchestration: { mode: 'graph', source: 'host_api' },
              origin: {
                kind: 'agent_graph',
                graphId: wake.graphId,
                wakeId: wake.wakeId,
                attemptId,
              },
            },
            activity,
          );
          if (outcome.kind === 'completed') {
            await this.#input.wakeStore.completeAgentGraphSupervisorWakeAttempt({
              graphId: wake.graphId,
              wakeId: wake.wakeId,
              attemptId,
              status: 'delivered',
            });
            return;
          }
          lastFailure = wakeOutcomeFailure(outcome);
          await this.#markRetryable(wake.graphId, wake.wakeId, attemptId, lastFailure);
        } catch (error) {
          lastFailure = errorMessage(error);
          await this.#markRetryable(wake.graphId, wake.wakeId, attemptId, lastFailure);
        }
      } finally {
        activity.release();
      }
      if (this.#closed) return;
    }
    throw new Error(
      `Agent graph supervisor wake was not delivered after ${this.#maxDeliveryAttempts} attempts: ${
        lastFailure ?? 'unknown failure'
      }`,
    );
  }

  async #markRetryable(
    graphId: string,
    wakeId: string,
    attemptId: string,
    failureReason: string,
  ): Promise<void> {
    await this.#input.wakeStore.completeAgentGraphSupervisorWakeAttempt({
      graphId,
      wakeId,
      attemptId,
      status: 'retryable_failed',
      failureReason: failureReason.slice(0, 4_000) || 'unknown failure',
    });
  }
}

function isSupervisorMilestone(result: AgentGraphScheduleReconciliationResult): boolean {
  if (
    result.status === 'cancelled' ||
    result.status === 'stale' ||
    result.status === 'limit_reached'
  ) {
    return false;
  }
  return result.dispatches.length > 0 || result.failures.length > 0;
}

function wakeOutcomeFailure(outcome: Exclude<GoalTurnOutcome, { kind: 'completed' }>): string {
  if (outcome.kind === 'errored' || outcome.kind === 'suspended') {
    return `${outcome.kind}: ${outcome.reason}`;
  }
  return 'aborted';
}

function renderAgentGraphSupervisorWakePrompt(
  snapshot: AgentGraphClientSnapshot,
  result?: AgentGraphScheduleReconciliationResult,
): string {
  return [
    '<agent-graph-supervisor-checkpoint>',
    `Graph ${snapshot.graphId} reached a durable supervisor checkpoint.`,
    `Reconciliation status: ${result?.status ?? 'recovered'}. Snapshot: ${snapshot.snapshotVersion}.`,
    'Inspect the graph with view_agent_graph. Use agent_output for child results when needed.',
    'Then either schedule the next work with update_agent_graph or finish the graph with the selected result record IDs.',
    'Report the useful outcome to the user when the graph is complete.',
    '</agent-graph-supervisor-checkpoint>',
  ].join('\n');
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function isAbortError(error: unknown): boolean {
  return error instanceof Error && error.name === 'AbortError';
}

async function notifyError(
  observer: AgentGraphSupervisorWakeInput['onError'],
  rootSessionId: string,
  error: unknown,
): Promise<void> {
  try {
    await observer?.(rootSessionId, error);
  } catch {
    // Wake diagnostics must not become graph data-path failures.
  }
}
