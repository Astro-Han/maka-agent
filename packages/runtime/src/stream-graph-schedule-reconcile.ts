import type {
  AgentGraphIntentClaim,
  AgentGraphIntentClaimResult,
} from '@maka/core/agent-graph-control';
import {
  AgentGraphScheduleRevisionConflictError,
  type AgentGraphScheduleControlStore,
} from '@maka/core/agent-graph-schedule';
import type { SessionEvent } from '@maka/core/events';
import { claimAgentGraphRunnableIntent } from './stream-graph-admission.js';
import type {
  AgentGraphDispatchedActivation,
  AgentGraphIntentExecutor,
  AgentGraphSupervisorObserver,
  AgentGraphSupervisorObservation,
} from './stream-graph-dispatch.js';
import type { AgentGraphRecord } from './stream-graph-projection.js';
import type { AgentGraphRunnableIntent } from './stream-graph-readiness.js';
import {
  projectAgentGraphSchedule,
  type AgentGraphScheduleProjection,
  type AgentGraphScheduleWorkView,
} from './stream-graph-supervisor-tools.js';
import type { AgentGraphTraceTopology } from './stream-graph-trace.js';
import { stableHash } from './request-shape.js';

const MAX_RECONCILIATION_ATTEMPTS = 8;
const SCHEDULE_INTENT_SCHEMA_VERSION = 1 as const;

export interface AgentGraphScheduleStopController {
  stopSession(sessionId: string, input: { source: 'graph_supervisor' }): Promise<void>;
}

export interface RenderAgentGraphScheduledWorkPromptInput {
  work: AgentGraphScheduleWorkView;
  inputRecords: AgentGraphRecord[];
}

export interface ReconcileAgentGraphScheduleInput {
  topology: AgentGraphTraceTopology;
  controlStore: AgentGraphScheduleControlStore;
  executor: AgentGraphIntentExecutor;
  stopController: AgentGraphScheduleStopController;
  newId: () => string;
  maxNewActivations: number;
  observeGraph(): Promise<AgentGraphSupervisorObservation>;
  renderPrompt(input: RenderAgentGraphScheduledWorkPromptInput): string | Promise<string>;
  abortSignal?: AbortSignal;
  supervisor?: AgentGraphSupervisorObserver;
}

export interface AgentGraphScheduleStopResult {
  targetId: string;
  reason: string;
  status: 'stopped' | 'already_terminal' | 'cancelled_before_runtime' | 'ignored_unknown';
  sessionId?: string;
  activationId?: string;
}

export interface AgentGraphScheduleDeferredWork {
  work: AgentGraphScheduleWorkView;
  reason: 'agent_topology_required' | 'input_not_committed' | 'graph_closed' | 'activation_limit';
  missingInputIds?: string[];
}

export interface AgentGraphScheduleReconciliationFailure {
  phase: 'schedule' | 'stop' | 'render' | 'dispatch';
  error: unknown;
  work?: AgentGraphScheduleWorkView;
  intent?: AgentGraphRunnableIntent;
  targetId?: string;
}

export interface AgentGraphScheduleReconciliationResult {
  status: 'reconciled' | 'waiting' | 'limit_reached' | 'failed' | 'cancelled' | 'stale';
  newActivationCount: number;
  observedExistingActivationCount: number;
  dispatches: AgentGraphDispatchedActivation[];
  stops: AgentGraphScheduleStopResult[];
  deferredWork: AgentGraphScheduleDeferredWork[];
  failures: AgentGraphScheduleReconciliationFailure[];
  schedule: AgentGraphScheduleProjection;
  observation: AgentGraphSupervisorObservation;
}

interface ScheduleSnapshot {
  schedule: AgentGraphScheduleProjection;
  observation: AgentGraphSupervisorObservation;
  claims: AgentGraphIntentClaim[];
}

interface PreparedWork {
  work: AgentGraphScheduleWorkView;
  intent: AgentGraphRunnableIntent;
  prompt: string;
}

type ScheduleDispatchOutcome =
  | {
      status: 'fulfilled';
      dispatch: AgentGraphDispatchedActivation;
    }
  | {
      status: 'rejected';
      failure: AgentGraphScheduleReconciliationFailure;
      admission?: AgentGraphIntentClaimResult;
    }
  | {
      status: 'stale';
    };

/**
 * Applies durable supervisor schedule intent to existing graph operators.
 *
 * Schedule revision and new intent admission are linearized by the control
 * store. Existing claims remain recoverable after finish; unclaimed work does
 * not cross terminal closure. New catalog-agent nodes deliberately remain a
 * separate dynamic-topology concern.
 */
export async function reconcileAgentGraphSchedule(
  input: ReconcileAgentGraphScheduleInput,
): Promise<AgentGraphScheduleReconciliationResult> {
  if (!Number.isSafeInteger(input.maxNewActivations) || input.maxNewActivations < 0) {
    throw new Error('Agent graph maxNewActivations must be a non-negative safe integer');
  }
  if (input.topology.operators.length === 0) {
    throw new Error('Agent graph schedule reconciliation requires at least one operator');
  }

  const processedIntentIds = new Set<string>();
  const dispatches: AgentGraphDispatchedActivation[] = [];
  const stops: AgentGraphScheduleStopResult[] = [];
  const failures: AgentGraphScheduleReconciliationFailure[] = [];
  let newActivationCount = 0;
  let observedExistingActivationCount = 0;
  let snapshot = await readScheduleSnapshot(input);

  for (let attempt = 0; attempt < MAX_RECONCILIATION_ATTEMPTS; attempt += 1) {
    if (input.abortSignal?.aborted) {
      return reconciliationResult(
        'cancelled',
        newActivationCount,
        observedExistingActivationCount,
        dispatches,
        stops,
        [],
        failures,
        snapshot,
      );
    }

    const stopWave = await applyScheduleStops(input, snapshot);
    stops.push(...stopWave.stops);
    failures.push(...stopWave.failures);
    if (failures.length > 0) {
      snapshot = await readScheduleSnapshot(input);
      return reconciliationResult(
        input.abortSignal?.aborted ? 'cancelled' : 'failed',
        newActivationCount,
        observedExistingActivationCount,
        dispatches,
        stops,
        [],
        failures,
        snapshot,
      );
    }

    const claimsByIntent = new Map(snapshot.claims.map((claim) => [claim.intentId, claim]));
    const committedRecords = new Map(
      snapshot.observation.projection.records.map((record) => [record.recordId, record]),
    );
    const deferredWork: AgentGraphScheduleDeferredWork[] = [];
    const candidates: Array<{
      work: AgentGraphScheduleWorkView;
      intent: AgentGraphRunnableIntent;
      existing: boolean;
    }> = [];

    for (const work of orderedRequestedWork(snapshot.schedule)) {
      if (work.target.kind === 'agent') {
        deferredWork.push({
          work,
          reason: snapshot.schedule.closed ? 'graph_closed' : 'agent_topology_required',
        });
        continue;
      }
      let intent: AgentGraphRunnableIntent;
      try {
        intent = scheduledWorkIntent(input.topology, snapshot.observation, work);
      } catch (error) {
        failures.push({ phase: 'schedule', work, error });
        continue;
      }
      if (processedIntentIds.has(intent.intentId)) continue;
      const existing = claimsByIntent.has(intent.intentId);
      if (snapshot.schedule.closed && !existing) {
        deferredWork.push({ work, reason: 'graph_closed' });
        continue;
      }
      const missingInputIds = work.inputIds.filter((recordId) => !committedRecords.has(recordId));
      if (missingInputIds.length > 0) {
        deferredWork.push({
          work,
          reason: 'input_not_committed',
          missingInputIds,
        });
        continue;
      }
      candidates.push({ work, intent, existing });
    }

    if (failures.length > 0) {
      snapshot = await readScheduleSnapshot(input);
      return reconciliationResult(
        'failed',
        newActivationCount,
        observedExistingActivationCount,
        dispatches,
        stops,
        deferredWork,
        failures,
        snapshot,
      );
    }

    const selected: typeof candidates = [];
    for (const candidate of candidates) {
      if (
        candidate.existing ||
        newActivationCount + selected.filter((item) => !item.existing).length <
          input.maxNewActivations
      ) {
        selected.push(candidate);
      } else {
        deferredWork.push({ work: candidate.work, reason: 'activation_limit' });
      }
    }

    const rendered = await Promise.allSettled(
      selected.map(async ({ work, intent }): Promise<PreparedWork> => {
        const prompt = await input.renderPrompt({
          work: clonePlain(work),
          inputRecords: work.inputIds.map((recordId) =>
            clonePlain(committedRecords.get(recordId)!),
          ),
        });
        if (!prompt.trim()) {
          throw new Error(`Agent graph scheduled work ${work.workId} rendered an empty prompt`);
        }
        return { work, intent, prompt };
      }),
    );
    const prepared: PreparedWork[] = [];
    rendered.forEach((result, index) => {
      if (result.status === 'fulfilled') {
        prepared.push(result.value);
      } else {
        failures.push({
          phase: 'render',
          work: selected[index]!.work,
          intent: selected[index]!.intent,
          error: result.reason,
        });
      }
    });
    if (failures.length > 0) {
      snapshot = await readScheduleSnapshot(input);
      return reconciliationResult(
        'failed',
        newActivationCount,
        observedExistingActivationCount,
        dispatches,
        stops,
        deferredWork,
        failures,
        snapshot,
      );
    }

    const outcomes = await Promise.all(
      prepared.map((work) => dispatchScheduledWork(input, work, snapshot.schedule.revision)),
    );
    let stale = false;
    for (const outcome of outcomes) {
      if (outcome.status === 'stale') {
        stale = true;
        continue;
      }
      if (outcome.status === 'fulfilled') {
        dispatches.push(outcome.dispatch);
        processedIntentIds.add(outcome.dispatch.intent.intentId);
        if (outcome.dispatch.claimCreated) newActivationCount += 1;
        else observedExistingActivationCount += 1;
        continue;
      }
      failures.push(outcome.failure);
      if (outcome.admission) {
        processedIntentIds.add(outcome.failure.intent!.intentId);
        if (outcome.admission.created) newActivationCount += 1;
        else observedExistingActivationCount += 1;
      }
    }

    const nextSnapshot = await readScheduleSnapshot(input);
    if (stale || nextSnapshot.schedule.revision !== snapshot.schedule.revision) {
      snapshot = nextSnapshot;
      continue;
    }
    snapshot = nextSnapshot;
    if (failures.length > 0) {
      return reconciliationResult(
        input.abortSignal?.aborted ? 'cancelled' : 'failed',
        newActivationCount,
        observedExistingActivationCount,
        dispatches,
        stops,
        deferredWork,
        failures,
        snapshot,
      );
    }
    const status = input.abortSignal?.aborted
      ? 'cancelled'
      : deferredWork.some((item) => item.reason === 'activation_limit')
        ? 'limit_reached'
        : deferredWork.some(
              (item) =>
                item.reason === 'agent_topology_required' || item.reason === 'input_not_committed',
            )
          ? 'waiting'
          : 'reconciled';
    return reconciliationResult(
      status,
      newActivationCount,
      observedExistingActivationCount,
      dispatches,
      stops,
      deferredWork,
      failures,
      snapshot,
    );
  }

  snapshot = await readScheduleSnapshot(input);
  return reconciliationResult(
    'stale',
    newActivationCount,
    observedExistingActivationCount,
    dispatches,
    stops,
    [],
    failures,
    snapshot,
  );
}

async function readScheduleSnapshot(
  input: ReconcileAgentGraphScheduleInput,
): Promise<ScheduleSnapshot> {
  const [updates, observation, claims] = await Promise.all([
    input.controlStore.listAgentGraphScheduleUpdates(input.topology.graphId),
    input.observeGraph(),
    input.controlStore.listAgentGraphIntentClaims(input.topology.graphId),
  ]);
  assertGraphObservation(input.topology.graphId, observation);
  notifySupervisor(input.supervisor?.onObservation, observation);
  return {
    schedule: projectAgentGraphSchedule(input.topology.graphId, updates),
    observation,
    claims,
  };
}

async function applyScheduleStops(
  input: ReconcileAgentGraphScheduleInput,
  snapshot: ScheduleSnapshot,
): Promise<{
  stops: AgentGraphScheduleStopResult[];
  failures: AgentGraphScheduleReconciliationFailure[];
}> {
  const requests = new Map<string, string>();
  for (const stopped of snapshot.schedule.stoppedTargets) {
    requests.set(stopped.targetId, stopped.reason);
  }
  for (const work of snapshot.schedule.work) {
    if (work.replaces && !requests.has(work.replaces)) {
      requests.set(work.replaces, `Superseded by graph work ${work.workId}`);
    }
  }
  if (requests.size === 0) return { stops: [], failures: [] };

  const workById = new Map(snapshot.schedule.work.map((work) => [work.workId, work]));
  const claimsByIntent = new Map(snapshot.claims.map((claim) => [claim.intentId, claim]));
  const activationTargets = graphActivationTargets(snapshot.observation);
  const immediate: AgentGraphScheduleStopResult[] = [];
  const targetsBySession = new Map<
    string,
    Array<{ targetId: string; reason: string; activationId: string }>
  >();
  const failures: AgentGraphScheduleReconciliationFailure[] = [];

  for (const [targetId, reason] of requests) {
    const work = workById.get(targetId);
    const claim = work
      ? claimsByIntent.get(scheduledWorkIntentId(snapshot.schedule.graphId, work.workId))
      : undefined;
    const activation = claim
      ? activationTargets.get(claim.targetRunId)
      : activationTargets.get(targetId);
    if (activation && isTerminalActivationStatus(activation.status)) {
      immediate.push({
        targetId,
        reason,
        status: 'already_terminal',
        sessionId: activation.sessionId,
        activationId: activation.activationId,
      });
      continue;
    }
    if (work && claim) {
      try {
        const cancellation = await input.controlStore.cancelAgentGraphIntentExecution(
          snapshot.schedule.graphId,
          claim.intentId,
          reason,
        );
        if (cancellation.previousState !== 'executing' && !activation) {
          immediate.push({
            targetId,
            reason,
            status: 'cancelled_before_runtime',
            sessionId: claim.targetSessionId,
            activationId: claim.targetRunId,
          });
          continue;
        }
      } catch (error) {
        failures.push({ phase: 'stop', targetId, error });
        continue;
      }
    } else if (work) {
      immediate.push({ targetId, reason, status: 'cancelled_before_runtime' });
      continue;
    } else if (!activation) {
      immediate.push({ targetId, reason, status: 'ignored_unknown' });
      continue;
    }
    const sessionId = activation?.sessionId ?? claim!.targetSessionId;
    const activationId = activation?.activationId ?? claim!.targetRunId;
    const sessionTargets = targetsBySession.get(sessionId) ?? [];
    sessionTargets.push({
      targetId,
      reason,
      activationId,
    });
    targetsBySession.set(sessionId, sessionTargets);
  }

  const settled = await Promise.allSettled(
    [...targetsBySession].map(async ([sessionId, targets]) => {
      await input.stopController.stopSession(sessionId, { source: 'graph_supervisor' });
      return targets.map(
        (target): AgentGraphScheduleStopResult => ({
          targetId: target.targetId,
          reason: target.reason,
          status: 'stopped',
          sessionId,
          activationId: target.activationId,
        }),
      );
    }),
  );
  settled.forEach((result, index) => {
    if (result.status === 'fulfilled') {
      immediate.push(...result.value);
      return;
    }
    const [sessionId, targets] = [...targetsBySession][index]!;
    targets.forEach((target) => {
      failures.push({
        phase: 'stop',
        targetId: target.targetId,
        error: new Error(
          `Failed to stop graph supervisor target ${target.targetId} in ${sessionId}`,
          { cause: result.reason },
        ),
      });
    });
  });
  return {
    stops: immediate.sort((a, b) => compareIdentity(a.targetId, b.targetId)),
    failures,
  };
}

async function dispatchScheduledWork(
  input: ReconcileAgentGraphScheduleInput,
  prepared: PreparedWork,
  expectedRevision: number,
): Promise<ScheduleDispatchOutcome> {
  let admission: AgentGraphIntentClaimResult | undefined;
  try {
    if (input.abortSignal?.aborted) {
      throw new Error('Agent graph scheduled work was cancelled before admission');
    }
    admission = await claimAgentGraphRunnableIntent({
      intent: prepared.intent,
      store: {
        claimAgentGraphIntent: (request) =>
          input.controlStore.claimAgentGraphIntentAtScheduleRevision(request, expectedRevision),
      },
      newId: input.newId,
      executionInput: { prompt: prepared.prompt },
    });
    const result = await input.executor.runClaimedAgentGraphIntent({
      claimStore: input.controlStore,
      graphId: prepared.intent.graphId,
      intentId: prepared.intent.intentId,
      prompt: prepared.prompt,
      async admitExecution() {
        const transition =
          await input.controlStore.beginAgentGraphIntentExecutionAtScheduleRevision(
            prepared.intent.graphId,
            prepared.intent.intentId,
            expectedRevision,
          );
        return transition.state === 'cancelled' ? 'cancelled' : 'executing';
      },
      ...(input.abortSignal ? { abortSignal: input.abortSignal } : {}),
      onReady(runtime) {
        notifySupervisor(input.supervisor?.onActivationReady, {
          intent: prepared.intent,
          claim: admission!.claim,
          runtime,
        });
      },
      onEvent(event: SessionEvent) {
        notifySupervisor(input.supervisor?.onRuntimeEvent, {
          intent: prepared.intent,
          claim: admission!.claim,
          event,
        });
      },
    });
    return {
      status: 'fulfilled',
      dispatch: {
        intent: prepared.intent,
        claim: admission.claim,
        claimCreated: admission.created,
        result,
      },
    };
  } catch (error) {
    if (error instanceof AgentGraphScheduleRevisionConflictError) {
      return { status: 'stale' };
    }
    return {
      status: 'rejected',
      failure: {
        phase: 'dispatch',
        work: prepared.work,
        intent: prepared.intent,
        error,
      },
      ...(admission ? { admission } : {}),
    };
  }
}

function scheduledWorkIntent(
  topology: AgentGraphTraceTopology,
  observation: AgentGraphSupervisorObservation,
  work: AgentGraphScheduleWorkView,
): AgentGraphRunnableIntent {
  if (work.target.kind !== 'operator') {
    throw new Error(`Graph work ${work.workId} requires a new agent topology node`);
  }
  const operatorId = work.target.operatorId;
  const topologyBinding = topology.operators.find((operator) => operator.operatorId === operatorId);
  const observedBinding = observation.projection.operators.find(
    (operator) => operator.operatorId === operatorId,
  );
  if (!topologyBinding || !observedBinding) {
    throw new Error(`Graph work ${work.workId} references unknown operator ${operatorId}`);
  }
  if (topologyBinding.sessionId !== observedBinding.sessionId) {
    throw new Error(`Graph operator ${operatorId} changed session identity during reconciliation`);
  }
  const policyFingerprint = stableHash({
    schemaVersion: SCHEDULE_INTENT_SCHEMA_VERSION,
    kind: 'supervisor',
    graphId: topology.graphId,
    workId: work.workId,
    target: work.target,
    inputIds: work.inputIds,
    ...(work.replaces ? { replaces: work.replaces } : {}),
  });
  const readinessContextFingerprint = stableHash({
    schemaVersion: SCHEDULE_INTENT_SCHEMA_VERSION,
    graphId: topology.graphId,
    workId: work.workId,
    operatorId,
    targetSessionId: topologyBinding.sessionId,
    inputIds: work.inputIds,
  });
  return {
    schemaVersion: 1,
    intentId: scheduledWorkIntentId(topology.graphId, work.workId),
    graphId: topology.graphId,
    readinessContextFingerprint,
    policyFingerprint,
    readinessId: work.workId,
    operatorId,
    targetSessionId: topologyBinding.sessionId,
    policyKind: 'supervisor',
    triggerRouteIds: [],
    triggerRecordIds: [...work.inputIds],
  };
}

function scheduledWorkIntentId(graphId: string, workId: string): string {
  const hash = stableHash({
    schemaVersion: SCHEDULE_INTENT_SCHEMA_VERSION,
    graphId,
    workId,
  });
  return `graph_intent_${hash.slice('sha256:'.length, 'sha256:'.length + 32)}`;
}

function orderedRequestedWork(
  schedule: AgentGraphScheduleProjection,
): AgentGraphScheduleWorkView[] {
  return schedule.work
    .filter((work) => work.status === 'requested')
    .sort(
      (a, b) =>
        a.revision - b.revision ||
        a.committedAt - b.committedAt ||
        compareIdentity(a.workId, b.workId),
    );
}

function graphActivationTargets(observation: AgentGraphSupervisorObservation): Map<
  string,
  {
    sessionId: string;
    activationId: string;
    status: string;
  }
> {
  const targets = new Map<
    string,
    {
      sessionId: string;
      activationId: string;
      status: string;
    }
  >();
  for (const binding of observation.projection.operators) {
    const state = observation.projection.state.operators[binding.operatorId];
    for (const activation of Object.values(state?.activations ?? {})) {
      if (targets.has(activation.activationId)) {
        throw new Error(
          `Graph activation ${activation.activationId} belongs to multiple operators`,
        );
      }
      targets.set(activation.activationId, {
        sessionId: binding.sessionId,
        activationId: activation.activationId,
        status: activation.status,
      });
    }
  }
  return targets;
}

function isTerminalActivationStatus(status: string): boolean {
  return (
    status === 'completed' || status === 'failed' || status === 'aborted' || status === 'cancelled'
  );
}

function assertGraphObservation(
  graphId: string,
  observation: AgentGraphSupervisorObservation,
): void {
  if (
    observation.projection.graphId !== graphId ||
    observation.readiness.graphId !== graphId ||
    observation.readiness.trace.graphId !== graphId
  ) {
    throw new Error('Agent graph schedule observation belongs to another graph');
  }
}

function reconciliationResult(
  status: AgentGraphScheduleReconciliationResult['status'],
  newActivationCount: number,
  observedExistingActivationCount: number,
  dispatches: readonly AgentGraphDispatchedActivation[],
  stops: readonly AgentGraphScheduleStopResult[],
  deferredWork: readonly AgentGraphScheduleDeferredWork[],
  failures: readonly AgentGraphScheduleReconciliationFailure[],
  snapshot: ScheduleSnapshot,
): AgentGraphScheduleReconciliationResult {
  return {
    status,
    newActivationCount,
    observedExistingActivationCount,
    dispatches: [...dispatches],
    stops: dedupeStops(stops),
    deferredWork: deferredWork.map((item) => clonePlain(item)),
    failures: [...failures],
    schedule: snapshot.schedule,
    observation: snapshot.observation,
  };
}

function dedupeStops(
  stops: readonly AgentGraphScheduleStopResult[],
): AgentGraphScheduleStopResult[] {
  const byTarget = new Map<string, AgentGraphScheduleStopResult>();
  for (const stop of stops) byTarget.set(stop.targetId, stop);
  return [...byTarget.values()].sort((a, b) => compareIdentity(a.targetId, b.targetId));
}

function notifySupervisor<T>(
  observer: ((input: T) => void | Promise<void>) | undefined,
  value: T,
): void {
  if (!observer) return;
  try {
    void Promise.resolve(observer(clonePlain(value))).catch(() => {
      // Presentation-only supervision must not gate reconciliation.
    });
  } catch {
    // Presentation-only supervision must not gate reconciliation.
  }
}

function compareIdentity(a: string, b: string): number {
  return a < b ? -1 : a > b ? 1 : 0;
}

function clonePlain<T>(value: T): T {
  return structuredClone(value);
}
