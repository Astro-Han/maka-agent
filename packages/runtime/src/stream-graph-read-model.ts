import type {
  AgentGraphIntentClaim,
  AgentGraphOperatorProvision,
  AgentGraphScheduleUpdate,
} from '@maka/core';
import type { AgentGraphSupervisorObservation } from './stream-graph-dispatch.js';
import type {
  AgentGraphActivationStatus,
  AgentGraphRecord,
  AgentGraphRecordFacet,
  AgentGraphSupervisorSignal,
} from './stream-graph-projection.js';
import type { AgentGraphReadinessWait } from './stream-graph-readiness.js';
import {
  projectAgentGraphSchedule,
  type AgentGraphScheduleFinishView,
  type AgentGraphScheduleProjection,
  type AgentGraphScheduleWorkView,
  type AgentGraphStoppedTargetView,
} from './stream-graph-supervisor-tools.js';
import { stableHash } from './request-shape.js';

export const AGENT_GRAPH_CLIENT_SNAPSHOT_SCHEMA_VERSION = 1 as const;

const MAX_VISIBLE_OPERATORS = 256;
const MAX_VISIBLE_EDGES = 512;
const MAX_VISIBLE_WORK = 256;
const MAX_VISIBLE_STOPPED_TARGETS = 128;
const MAX_RECENT_CONTROL_DECISIONS = 32;
const MAX_VISIBLE_CLAIMS = 256;
const MAX_RECENT_ACTIVITY = 64;
const MAX_TERMINAL_HISTORY = 64;
const MAX_OPERATOR_INSPECTION_ACTIVATIONS = 64;
const MAX_OPERATOR_INSPECTION_RECORDS = 128;
const MAX_INSTRUCTION_PREVIEW_CHARS = 500;

export type AgentGraphClientOperatorStatus =
  | 'not_started'
  | 'waiting'
  | 'runnable'
  | 'running'
  | 'blocked'
  | AgentGraphActivationStatus;

export type AgentGraphClientStatus =
  | 'empty'
  | 'active'
  | 'waiting'
  | 'stopped'
  | 'failed'
  | 'completed';

export interface AgentGraphClientRunRef {
  sessionId: string;
  agentRunId: string;
  turnId?: string;
}

export interface AgentGraphClientOperator {
  operatorId: string;
  childSessionId: string;
  provisionId: string;
  agentId: string;
  provisionedAt: number;
  status: AgentGraphClientOperatorStatus;
  inboundEdgeIds: string[];
  outboundEdgeIds: string[];
  scheduledWorkIds: string[];
  readiness: Array<{
    readinessId: string;
    policyKind: 'map' | 'all_settled';
    status: 'waiting' | 'runnable';
    waitingFor: AgentGraphReadinessWait[];
  }>;
  currentActivation?: {
    activationId: string;
    status: AgentGraphActivationStatus;
    recordCount: number;
    firstEventTime: number;
    lastEventTime: number;
    terminalRecordId?: string;
    run: AgentGraphClientRunRef;
  };
}

export interface AgentGraphClientEdge {
  edgeId: string;
  fromOperatorId: string;
  toOperatorId: string;
}

export interface AgentGraphClientScheduledWork {
  workId: string;
  target:
    | { kind: 'agent'; agentId: string }
    | { kind: 'operator'; operatorId: string };
  inputIds: string[];
  replaces?: string;
  status: AgentGraphScheduleWorkView['status'];
  instructionPreview: string;
  instructionTruncated: boolean;
  revision: number;
  committedAt: number;
}

export interface AgentGraphClientStoppedTarget {
  targetId: string;
  reason: string;
  revision: number;
  committedAt: number;
}

export interface AgentGraphClientFinish {
  resultIds: string[];
  reason: string;
  revision: number;
  committedAt: number;
}

export interface AgentGraphClientControlDecision {
  updateId: string;
  revision: number;
  committedAt: number;
  source: {
    sessionId: string;
    agentRunId: string;
    turnId: string;
    toolCallId: string;
  };
  addedWorkIds: string[];
  stoppedTargetIds: string[];
  selectedResultIds: string[];
}

export interface AgentGraphClientClaimRef {
  claimId: string;
  intentId: string;
  operatorId: string;
  childSessionId: string;
  run: AgentGraphClientRunRef;
  claimedAt: number;
}

export interface AgentGraphClientActivity {
  recordId: string;
  operatorId: string;
  activationId: string;
  eventTime: number;
  facets: AgentGraphRecordFacet[];
  signals: AgentGraphSupervisorSignal[];
  run: AgentGraphClientRunRef;
}

export interface AgentGraphClientTerminalHistoryPage {
  records: AgentGraphClientActivity[];
  nextCursor?: string;
}

export interface AgentGraphClientSnapshot {
  schemaVersion: typeof AGENT_GRAPH_CLIENT_SNAPSHOT_SCHEMA_VERSION;
  rootSessionId: string;
  graphId: string;
  snapshotVersion: string;
  status: AgentGraphClientStatus;
  scheduleRevision: number;
  topologyFingerprint: string;
  closed: boolean;
  latestEventTime?: number;
  operators: AgentGraphClientOperator[];
  edges: AgentGraphClientEdge[];
  work: AgentGraphClientScheduledWork[];
  stoppedTargets: AgentGraphClientStoppedTarget[];
  finish?: AgentGraphClientFinish;
  claims: AgentGraphClientClaimRef[];
  recentControlDecisions: AgentGraphClientControlDecision[];
  recentActivity: AgentGraphClientActivity[];
  terminalHistory: AgentGraphClientTerminalHistoryPage;
  omitted: {
    operators: number;
    edges: number;
    work: number;
    stoppedTargets: number;
    claims: number;
    controlDecisions: number;
    recentActivity: number;
  };
}

export interface AgentGraphOperatorInspection {
  schemaVersion: typeof AGENT_GRAPH_CLIENT_SNAPSHOT_SCHEMA_VERSION;
  rootSessionId: string;
  graphId: string;
  snapshotVersion: string;
  operator: AgentGraphClientOperator;
  inboundEdges: AgentGraphClientEdge[];
  outboundEdges: AgentGraphClientEdge[];
  work: AgentGraphClientScheduledWork[];
  claims: AgentGraphClientClaimRef[];
  activations: Array<{
    activationId: string;
    status: AgentGraphActivationStatus;
    recordCount: number;
    firstEventTime: number;
    lastEventTime: number;
    lastRecordId: string;
    terminalRecordId?: string;
    run: AgentGraphClientRunRef;
  }>;
  recentRecords: AgentGraphClientActivity[];
  omittedActivationCount: number;
  omittedRecordCount: number;
}

export interface BuildAgentGraphClientReadModelInput {
  rootSessionId: string;
  graphId: string;
  provisions: readonly AgentGraphOperatorProvision[];
  scheduleUpdates: readonly AgentGraphScheduleUpdate[];
  observation: AgentGraphSupervisorObservation;
}

export interface AgentGraphClientSnapshotOptions {
  terminalCursor?: string;
}

interface BuiltReadModel {
  snapshotVersion: string;
  schedule: AgentGraphScheduleProjection;
  operators: AgentGraphClientOperator[];
  edges: AgentGraphClientEdge[];
  work: AgentGraphClientScheduledWork[];
  stoppedTargets: AgentGraphClientStoppedTarget[];
  finish?: AgentGraphClientFinish;
  claims: AgentGraphClientClaimRef[];
  recentControlDecisions: AgentGraphClientControlDecision[];
  activity: AgentGraphClientActivity[];
}

/**
 * Durable, bounded graph-facing projection for untrusted presentation clients.
 *
 * Payloads remain in Session/Runtime stores. This surface carries only
 * identities, lifecycle state, wait reasons, and bounded instruction previews.
 */
export function buildAgentGraphClientSnapshot(
  input: BuildAgentGraphClientReadModelInput,
  options: AgentGraphClientSnapshotOptions = {},
): AgentGraphClientSnapshot {
  const model = buildReadModel(input);
  const visibleOperators = boundOperators(model.operators);
  const visibleOperatorIds = new Set(visibleOperators.map((operator) => operator.operatorId));
  const candidateEdges = model.edges.filter(
    (edge) =>
      visibleOperatorIds.has(edge.fromOperatorId) &&
      visibleOperatorIds.has(edge.toOperatorId),
  );
  const edges = candidateEdges.slice(0, MAX_VISIBLE_EDGES);
  const work = boundWork(model.work);
  const stoppedTargets = model.stoppedTargets.slice(-MAX_VISIBLE_STOPPED_TARGETS);
  const claims = model.claims.slice(-MAX_VISIBLE_CLAIMS);
  const recentControlDecisions = model.recentControlDecisions.slice(
    -MAX_RECENT_CONTROL_DECISIONS,
  );
  const recentActivity = model.activity.slice(-MAX_RECENT_ACTIVITY);
  return {
    schemaVersion: AGENT_GRAPH_CLIENT_SNAPSHOT_SCHEMA_VERSION,
    rootSessionId: input.rootSessionId,
    graphId: input.graphId,
    snapshotVersion: model.snapshotVersion,
    status: graphStatus(model.schedule, model.operators),
    scheduleRevision: model.schedule.revision,
    topologyFingerprint: input.observation.readiness.topologyFingerprint,
    closed: model.schedule.closed,
    ...(input.observation.projection.state.latestEventTime !== undefined
      ? { latestEventTime: input.observation.projection.state.latestEventTime }
      : {}),
    operators: visibleOperators,
    edges,
    work,
    stoppedTargets,
    ...(model.finish ? { finish: model.finish } : {}),
    claims,
    recentControlDecisions,
    recentActivity,
    terminalHistory: terminalHistoryPage(input.graphId, model.activity, options.terminalCursor),
    omitted: {
      operators: model.operators.length - visibleOperators.length,
      edges: model.edges.length - edges.length,
      work: model.work.length - work.length,
      stoppedTargets: model.stoppedTargets.length - stoppedTargets.length,
      claims: model.claims.length - claims.length,
      controlDecisions:
        model.recentControlDecisions.length - recentControlDecisions.length,
      recentActivity: model.activity.length - recentActivity.length,
    },
  };
}

export function inspectAgentGraphOperator(
  input: BuildAgentGraphClientReadModelInput,
  operatorId: string,
): AgentGraphOperatorInspection {
  const expectedOperatorId = requireIdentity(operatorId, 'operator id');
  const model = buildReadModel(input);
  const operator = model.operators.find(
    (candidate) => candidate.operatorId === expectedOperatorId,
  );
  if (!operator) {
    throw new Error(`Agent graph operator ${expectedOperatorId} was not found`);
  }
  const runtimeState = input.observation.projection.state.operators[expectedOperatorId];
  const allActivations = runtimeState
    ? Object.values(runtimeState.activations).sort(
        (a, b) =>
          a.firstEventTime - b.firstEventTime ||
          compareIdentity(a.activationId, b.activationId),
      )
    : [];
  const visibleActivations = allActivations.slice(-MAX_OPERATOR_INSPECTION_ACTIVATIONS);
  const allRecords = model.activity.filter(
    (record) => record.operatorId === expectedOperatorId,
  );
  const recentRecords = allRecords.slice(-MAX_OPERATOR_INSPECTION_RECORDS);
  const claimByRunId = new Map(
    model.claims
      .filter((claim) => claim.operatorId === expectedOperatorId)
      .map((claim) => [claim.run.agentRunId, claim]),
  );
  return {
    schemaVersion: AGENT_GRAPH_CLIENT_SNAPSHOT_SCHEMA_VERSION,
    rootSessionId: input.rootSessionId,
    graphId: input.graphId,
    snapshotVersion: model.snapshotVersion,
    operator,
    inboundEdges: model.edges.filter(
      (edge) => edge.toOperatorId === expectedOperatorId,
    ),
    outboundEdges: model.edges.filter(
      (edge) => edge.fromOperatorId === expectedOperatorId,
    ),
    work: model.work.filter((work) =>
      operator.scheduledWorkIds.includes(work.workId),
    ),
    claims: model.claims.filter(
      (claim) => claim.operatorId === expectedOperatorId,
    ),
    activations: visibleActivations.map((activation) => ({
      activationId: activation.activationId,
      status: activation.status,
      recordCount: activation.recordCount,
      firstEventTime: activation.firstEventTime,
      lastEventTime: activation.lastEventTime,
      lastRecordId: activation.lastRecordId,
      ...(activation.terminalRecordId
        ? { terminalRecordId: activation.terminalRecordId }
        : {}),
      run: runRefForActivation(
        operator.childSessionId,
        activation.agentRunId,
        claimByRunId.get(activation.agentRunId),
        allRecords,
      ),
    })),
    recentRecords,
    omittedActivationCount: allActivations.length - visibleActivations.length,
    omittedRecordCount: allRecords.length - recentRecords.length,
  };
}

function buildReadModel(input: BuildAgentGraphClientReadModelInput): BuiltReadModel {
  requireIdentity(input.rootSessionId, 'root Session id');
  requireIdentity(input.graphId, 'graph id');
  assertObservation(input.graphId, input.observation);
  const schedule = projectAgentGraphSchedule(input.graphId, input.scheduleUpdates);
  const provisionByOperator = provisionsByOperator(input.graphId, input.provisions);
  const edges = uniqueEdges(input.graphId, input.provisions);
  const claims = clientClaims(input.graphId, input.observation.claims);
  const activity = input.observation.projection.records.map(clientActivity);
  const work = schedule.work.map(clientWork);
  const operators = input.observation.projection.operators.map((binding) => {
    const provision = provisionByOperator.get(binding.operatorId);
    if (!provision || provision.targetSessionId !== binding.sessionId) {
      throw new Error(
        `Agent graph operator ${binding.operatorId} has no matching durable provision`,
      );
    }
    const readiness = input.observation.readiness.supervisorView
      .filter((entry) => entry.operatorId === binding.operatorId)
      .map((entry) => ({
        readinessId: entry.readinessId,
        policyKind: entry.policyKind,
        status: entry.status,
        waitingFor: entry.waitingFor.map(cloneWait),
      }));
    const state = input.observation.projection.state.operators[binding.operatorId];
    const currentActivation = state?.activations[state.currentActivationId];
    const currentClaim = currentActivation
      ? claims.find(
          (claim) =>
            claim.operatorId === binding.operatorId &&
            claim.run.agentRunId === currentActivation.agentRunId,
        )
      : undefined;
    const operatorRecords = activity.filter(
      (record) => record.operatorId === binding.operatorId,
    );
    const currentRecord = currentActivation
      ? [...operatorRecords]
          .reverse()
          .find((record) => record.activationId === currentActivation.activationId)
      : undefined;
    return {
      operatorId: binding.operatorId,
      childSessionId: binding.sessionId,
      provisionId: provision.provisionId,
      agentId: provision.agentId,
      provisionedAt: provision.provisionedAt,
      status: operatorStatus(state?.status, readiness, currentRecord),
      inboundEdgeIds: edges
        .filter((edge) => edge.toOperatorId === binding.operatorId)
        .map((edge) => edge.edgeId),
      outboundEdgeIds: edges
        .filter((edge) => edge.fromOperatorId === binding.operatorId)
        .map((edge) => edge.edgeId),
      scheduledWorkIds: work
        .filter(
          (entry) =>
            entry.workId === provision.workId ||
            (entry.target.kind === 'operator' &&
              entry.target.operatorId === binding.operatorId),
        )
        .map((entry) => entry.workId),
      readiness,
      ...(currentActivation
        ? {
            currentActivation: {
              activationId: currentActivation.activationId,
              status: currentActivation.status,
              recordCount: currentActivation.recordCount,
              firstEventTime: currentActivation.firstEventTime,
              lastEventTime: currentActivation.lastEventTime,
              ...(currentActivation.terminalRecordId
                ? { terminalRecordId: currentActivation.terminalRecordId }
                : {}),
              run: runRefForActivation(
                binding.sessionId,
                currentActivation.agentRunId,
                currentClaim,
                operatorRecords,
              ),
            },
          }
        : {}),
    } satisfies AgentGraphClientOperator;
  });
  const stoppedTargets = schedule.stoppedTargets.map(clientStoppedTarget);
  const finish = schedule.finish ? clientFinish(schedule.finish) : undefined;
  const recentControlDecisions = input.scheduleUpdates
    .slice()
    .sort((a, b) => a.revision - b.revision)
    .map((update) => ({
      updateId: update.updateId,
      revision: update.revision,
      committedAt: update.committedAt,
      source: {
        sessionId: update.source.sessionId,
        agentRunId: update.source.runId,
        turnId: update.source.turnId,
        toolCallId: update.source.toolCallId,
      },
      addedWorkIds: update.addWork.map((entry) => entry.workId),
      stoppedTargetIds: update.stop.map((entry) => entry.targetId),
      selectedResultIds: update.finish ? [...update.finish.resultIds] : [],
    }));
  const snapshotVersion = stableHash({
    schemaVersion: AGENT_GRAPH_CLIENT_SNAPSHOT_SCHEMA_VERSION,
    rootSessionId: input.rootSessionId,
    graphId: input.graphId,
    scheduleRevision: schedule.revision,
    topologyFingerprint: input.observation.readiness.topologyFingerprint,
    provisionCount: input.provisions.length,
    latestProvisionId: [...input.provisions]
      .sort(
        (a, b) =>
          a.provisionedAt - b.provisionedAt ||
          compareIdentity(a.provisionId, b.provisionId),
      )
      .at(-1)?.provisionId,
    claimCount: claims.length,
    latestClaimId: claims.at(-1)?.claimId,
    recordCount: activity.length,
    latestRecordId: activity.at(-1)?.recordId,
  });
  return {
    snapshotVersion,
    schedule,
    operators,
    edges,
    work,
    stoppedTargets,
    ...(finish ? { finish } : {}),
    claims,
    recentControlDecisions,
    activity,
  };
}

function operatorStatus(
  runtimeStatus: AgentGraphActivationStatus | undefined,
  readiness: AgentGraphClientOperator['readiness'],
  currentRecord: AgentGraphClientActivity | undefined,
): AgentGraphClientOperatorStatus {
  if (runtimeStatus === 'running') {
    return currentRecord?.signals.some((signal) => signal.kind === 'attention')
      ? 'blocked'
      : 'running';
  }
  if (runtimeStatus) return runtimeStatus;
  if (readiness.some((entry) => entry.status === 'runnable')) return 'runnable';
  if (readiness.length > 0) return 'waiting';
  return 'not_started';
}

function graphStatus(
  schedule: AgentGraphScheduleProjection,
  operators: readonly AgentGraphClientOperator[],
): AgentGraphClientStatus {
  if (schedule.closed) return 'completed';
  if (operators.length === 0 && schedule.work.length === 0) return 'empty';
  if (
    operators.some((operator) =>
      ['running', 'blocked', 'runnable'].includes(operator.status),
    )
  ) {
    return 'active';
  }
  const activeWork = schedule.work.filter((work) => work.status === 'requested');
  if (activeWork.length === 0 && schedule.stoppedTargets.length > 0) return 'stopped';
  if (
    operators.length > 0 &&
    operators.every((operator) =>
      ['failed', 'aborted', 'cancelled'].includes(operator.status),
    )
  ) {
    return 'failed';
  }
  return 'waiting';
}

function boundOperators(
  operators: readonly AgentGraphClientOperator[],
): AgentGraphClientOperator[] {
  const live = operators.filter(
    (operator) =>
      !['completed', 'failed', 'aborted', 'cancelled'].includes(operator.status),
  );
  const terminal = operators
    .filter((operator) =>
      ['completed', 'failed', 'aborted', 'cancelled'].includes(operator.status),
    )
    .sort(
      (a, b) =>
        (a.currentActivation?.lastEventTime ?? a.provisionedAt) -
          (b.currentActivation?.lastEventTime ?? b.provisionedAt) ||
        compareIdentity(a.operatorId, b.operatorId),
    );
  if (live.length >= MAX_VISIBLE_OPERATORS) {
    return live.slice(0, MAX_VISIBLE_OPERATORS);
  }
  return [...live, ...terminal.slice(-(MAX_VISIBLE_OPERATORS - live.length))];
}

function boundWork(
  work: readonly AgentGraphClientScheduledWork[],
): AgentGraphClientScheduledWork[] {
  const live = work.filter((entry) => entry.status === 'requested');
  const terminal = work.filter((entry) => entry.status !== 'requested');
  if (live.length >= MAX_VISIBLE_WORK) return live.slice(0, MAX_VISIBLE_WORK);
  return [...live, ...terminal.slice(-(MAX_VISIBLE_WORK - live.length))];
}

function terminalHistoryPage(
  graphId: string,
  activity: readonly AgentGraphClientActivity[],
  cursor: string | undefined,
): AgentGraphClientTerminalHistoryPage {
  const terminal = activity
    .filter((record) => record.signals.some((signal) => signal.kind === 'terminal'))
    .reverse();
  let start = 0;
  if (cursor !== undefined) {
    const decoded = decodeTerminalCursor(cursor);
    if (decoded.graphId !== graphId) {
      throw new Error('Agent graph terminal history cursor belongs to another graph');
    }
    const index = terminal.findIndex((record) => record.recordId === decoded.recordId);
    if (index < 0) {
      throw new Error('Agent graph terminal history cursor is stale or invalid');
    }
    start = index + 1;
  }
  const records = terminal.slice(start, start + MAX_TERMINAL_HISTORY);
  const hasMore = start + records.length < terminal.length;
  return {
    records,
    ...(hasMore && records.length > 0
      ? {
          nextCursor: encodeTerminalCursor(
            graphId,
            records[records.length - 1]!.recordId,
          ),
        }
      : {}),
  };
}

function clientWork(work: AgentGraphScheduleWorkView): AgentGraphClientScheduledWork {
  const instructionTruncated = work.instruction.length > MAX_INSTRUCTION_PREVIEW_CHARS;
  return {
    workId: work.workId,
    target: { ...work.target },
    inputIds: [...work.inputIds],
    ...(work.replaces ? { replaces: work.replaces } : {}),
    status: work.status,
    instructionPreview: instructionTruncated
      ? `${work.instruction.slice(0, MAX_INSTRUCTION_PREVIEW_CHARS)}…`
      : work.instruction,
    instructionTruncated,
    revision: work.revision,
    committedAt: work.committedAt,
  };
}

function clientStoppedTarget(
  stopped: AgentGraphStoppedTargetView,
): AgentGraphClientStoppedTarget {
  return {
    targetId: stopped.targetId,
    reason: stopped.reason,
    revision: stopped.revision,
    committedAt: stopped.committedAt,
  };
}

function clientFinish(finish: AgentGraphScheduleFinishView): AgentGraphClientFinish {
  return {
    resultIds: [...finish.resultIds],
    reason: finish.reason,
    revision: finish.revision,
    committedAt: finish.committedAt,
  };
}

function clientClaims(
  graphId: string,
  claims: readonly AgentGraphIntentClaim[],
): AgentGraphClientClaimRef[] {
  return [...claims]
    .sort(
      (a, b) =>
        a.claimedAt - b.claimedAt || compareIdentity(a.claimId, b.claimId),
    )
    .map((claim) => {
      if (claim.graphId !== graphId) {
        throw new Error(`Agent graph claim ${claim.claimId} belongs to another graph`);
      }
      return {
        claimId: claim.claimId,
        intentId: claim.intentId,
        operatorId: claim.targetOperatorId,
        childSessionId: claim.targetSessionId,
        run: {
          sessionId: claim.targetSessionId,
          agentRunId: claim.targetRunId,
          turnId: claim.targetTurnId,
        },
        claimedAt: claim.claimedAt,
      };
    });
}

function clientActivity(record: AgentGraphRecord): AgentGraphClientActivity {
  return {
    recordId: record.recordId,
    operatorId: record.operatorId,
    activationId: record.activationId,
    eventTime: record.eventTime,
    facets: [...record.facets],
    signals: record.supervisorSignals.map((signal) => ({ ...signal })),
    run: {
      sessionId: record.sessionId,
      agentRunId: record.agentRunId,
      turnId: record.source.turnId,
    },
  };
}

function runRefForActivation(
  sessionId: string,
  agentRunId: string,
  claim: AgentGraphClientClaimRef | undefined,
  records: readonly AgentGraphClientActivity[],
): AgentGraphClientRunRef {
  const turnId =
    claim?.run.turnId ??
    records.find((record) => record.run.agentRunId === agentRunId)?.run.turnId;
  return {
    sessionId,
    agentRunId,
    ...(turnId ? { turnId } : {}),
  };
}

function provisionsByOperator(
  graphId: string,
  provisions: readonly AgentGraphOperatorProvision[],
): Map<string, AgentGraphOperatorProvision> {
  const result = new Map<string, AgentGraphOperatorProvision>();
  for (const provision of [...provisions].sort(
    (a, b) =>
      a.provisionedAt - b.provisionedAt ||
      compareIdentity(a.provisionId, b.provisionId),
  )) {
    if (provision.graphId !== graphId) {
      throw new Error(`Agent graph provision ${provision.provisionId} belongs to another graph`);
    }
    const existing = result.get(provision.operatorId);
    if (
      existing &&
      (existing.targetSessionId !== provision.targetSessionId ||
        existing.agentId !== provision.agentId)
    ) {
      throw new Error(
        `Agent graph operator ${provision.operatorId} has conflicting durable provisions`,
      );
    }
    result.set(provision.operatorId, provision);
  }
  return result;
}

function uniqueEdges(
  graphId: string,
  provisions: readonly AgentGraphOperatorProvision[],
): AgentGraphClientEdge[] {
  const edges = new Map<string, AgentGraphClientEdge>();
  for (const provision of provisions) {
    if (provision.graphId !== graphId) {
      throw new Error(`Agent graph provision ${provision.provisionId} belongs to another graph`);
    }
    for (const edge of provision.edges) {
      const existing = edges.get(edge.edgeId);
      if (
        existing &&
        (existing.fromOperatorId !== edge.fromOperatorId ||
          existing.toOperatorId !== edge.toOperatorId)
      ) {
        throw new Error(`Agent graph edge ${edge.edgeId} has conflicting endpoints`);
      }
      edges.set(edge.edgeId, { ...edge });
    }
  }
  return [...edges.values()].sort((a, b) => compareIdentity(a.edgeId, b.edgeId));
}

function assertObservation(
  graphId: string,
  observation: AgentGraphSupervisorObservation,
): void {
  if (
    observation.projection.graphId !== graphId ||
    observation.readiness.graphId !== graphId ||
    observation.readiness.trace.graphId !== graphId
  ) {
    throw new Error('Agent graph client observation belongs to another graph');
  }
}

function cloneWait(wait: AgentGraphReadinessWait): AgentGraphReadinessWait {
  return wait.kind === 'input_route'
    ? { ...wait, upstreamOperatorIds: [...wait.upstreamOperatorIds] }
    : { ...wait };
}

function encodeTerminalCursor(graphId: string, recordId: string): string {
  return Buffer.from(
    JSON.stringify({
      schemaVersion: AGENT_GRAPH_CLIENT_SNAPSHOT_SCHEMA_VERSION,
      graphId,
      recordId,
    }),
    'utf8',
  ).toString('base64url');
}

function decodeTerminalCursor(cursor: string): {
  graphId: string;
  recordId: string;
} {
  if (
    typeof cursor !== 'string' ||
    cursor.length === 0 ||
    cursor.length > 2_048 ||
    cursor.trim() !== cursor
  ) {
    throw new Error('Invalid agent graph terminal history cursor');
  }
  try {
    const json = Buffer.from(cursor, 'base64url').toString('utf8');
    const canonical = Buffer.from(json, 'utf8').toString('base64url');
    const value = JSON.parse(json) as Record<string, unknown>;
    if (
      canonical !== cursor ||
      Object.keys(value).sort().join(',') !== 'graphId,recordId,schemaVersion' ||
      value.schemaVersion !== AGENT_GRAPH_CLIENT_SNAPSHOT_SCHEMA_VERSION
    ) {
      throw new Error('invalid cursor envelope');
    }
    return {
      graphId: requireIdentity(value.graphId, 'cursor graph id'),
      recordId: requireIdentity(value.recordId, 'cursor record id'),
    };
  } catch {
    throw new Error('Invalid agent graph terminal history cursor');
  }
}

function requireIdentity(value: unknown, name: string): string {
  if (
    typeof value !== 'string' ||
    value.length === 0 ||
    value.length > 512 ||
    value.trim() !== value ||
    /[\u0000-\u001f\u007f]/.test(value)
  ) {
    throw new Error(`Invalid agent graph ${name}`);
  }
  return value;
}

function compareIdentity(a: string, b: string): number {
  return a < b ? -1 : a > b ? 1 : 0;
}
