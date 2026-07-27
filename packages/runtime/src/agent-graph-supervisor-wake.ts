import type { StoredMessage, UserMessageInput } from '@maka/core';
import type { SessionActivityLease, SessionActivityRegistry } from './goal-turn-lifecycle.js';
import type { AgentGraphClientSnapshot } from './stream-graph-read-model.js';
import type { AgentGraphScheduleReconciliationResult } from './stream-graph-schedule-reconcile.js';

export interface AgentGraphSupervisorWakeInput {
  activityRegistry: SessionActivityRegistry;
  readMessages(sessionId: string): Promise<StoredMessage[]>;
  readSnapshot(rootSessionId: string): Promise<AgentGraphClientSnapshot>;
  startTurn(
    sessionId: string,
    input: UserMessageInput,
    activity: SessionActivityLease,
  ): Promise<unknown>;
  newId(): string;
  onError?(rootSessionId: string, error: unknown): void | Promise<void>;
}

/**
 * Host-side control-plane bridge from graph quiescence back to the root
 * supervisor Agent.
 *
 * The graph coordinator remains independent from Agent turns. This bridge
 * waits for the current root turn to settle, then starts a fresh, visibly
 * host-authored Graph turn. The durable message origin is the idempotency
 * authority across concurrent callbacks and process recovery.
 */
export class AgentGraphSupervisorWakeCoordinator {
  readonly #input: AgentGraphSupervisorWakeInput;
  readonly #tasks = new Set<Promise<void>>();
  readonly #pendingWakeIds = new Set<string>();
  #closed = false;

  constructor(input: AgentGraphSupervisorWakeInput) {
    this.#input = input;
  }

  notify(rootSessionId: string, result: AgentGraphScheduleReconciliationResult): void {
    if (this.#closed || !isSupervisorMilestone(result)) return;
    const task = this.#wake(rootSessionId, result).catch((error) =>
      notifyError(this.#input.onError, rootSessionId, error),
    );
    this.#tasks.add(task);
    void task.finally(() => this.#tasks.delete(task));
  }

  async waitForIdle(): Promise<void> {
    while (this.#tasks.size > 0) await Promise.all([...this.#tasks]);
  }

  async close(): Promise<void> {
    this.#closed = true;
    await this.waitForIdle();
  }

  async #wake(
    rootSessionId: string,
    result: AgentGraphScheduleReconciliationResult,
  ): Promise<void> {
    const snapshot = await this.#input.readSnapshot(rootSessionId);
    if (snapshot.closed || snapshot.scheduleRevision === 0) return;
    const wakeId = `${snapshot.graphId}:${snapshot.snapshotVersion}`;
    if (this.#pendingWakeIds.has(wakeId)) return;
    this.#pendingWakeIds.add(wakeId);
    try {
      if (await hasPersistedWake(this.#input, rootSessionId, wakeId)) return;
      const activity = await this.#input.activityRegistry.acquire(rootSessionId);
      try {
        // A competing callback or a recovered turn may have committed while
        // this wake waited behind the user's original supervisor turn.
        if (await hasPersistedWake(this.#input, rootSessionId, wakeId)) return;
        await this.#input.startTurn(
          rootSessionId,
          {
            turnId: this.#input.newId(),
            text: renderAgentGraphSupervisorWakePrompt(snapshot, result),
            displayText: 'Agent graph reached a supervisor checkpoint.',
            turnOrchestration: { mode: 'graph', source: 'host_api' },
            origin: {
              kind: 'agent_graph',
              graphId: snapshot.graphId,
              wakeId,
            },
          },
          activity,
        );
      } finally {
        // The Desktop stream owns and normally releases this lease. release()
        // is idempotent, so this also covers failures before streaming starts.
        activity.release();
      }
    } finally {
      this.#pendingWakeIds.delete(wakeId);
    }
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

async function hasPersistedWake(
  input: AgentGraphSupervisorWakeInput,
  rootSessionId: string,
  wakeId: string,
): Promise<boolean> {
  return (await input.readMessages(rootSessionId)).some(
    (message) =>
      message.type === 'user' &&
      message.origin?.kind === 'agent_graph' &&
      message.origin.wakeId === wakeId,
  );
}

function renderAgentGraphSupervisorWakePrompt(
  snapshot: AgentGraphClientSnapshot,
  result: AgentGraphScheduleReconciliationResult,
): string {
  return [
    '<agent-graph-supervisor-checkpoint>',
    `Graph ${snapshot.graphId} reached a durable supervisor checkpoint.`,
    `Reconciliation status: ${result.status}. Snapshot: ${snapshot.snapshotVersion}.`,
    'Inspect the graph with view_agent_graph. Use agent_output for child results when needed.',
    'Then either schedule the next work with update_agent_graph or finish the graph with the selected result record IDs.',
    'Report the useful outcome to the user when the graph is complete.',
    '</agent-graph-supervisor-checkpoint>',
  ].join('\n');
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
