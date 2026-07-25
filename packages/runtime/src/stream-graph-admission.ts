import type {
  AgentGraphIntentClaimResult,
  AgentGraphIntentClaimStore,
} from '@maka/core/agent-graph-control';
import { AGENT_GRAPH_INTENT_CLAIM_SCHEMA_VERSION } from '@maka/core/agent-graph-control';
import { stableHash } from './request-shape.js';
import type { AgentGraphRunnableIntent } from './stream-graph-readiness.js';

const AGENT_GRAPH_EXECUTION_INPUT_SCHEMA_VERSION = 1 as const;

export interface ClaimAgentGraphRunnableIntentInput {
  intent: AgentGraphRunnableIntent;
  store: Pick<AgentGraphIntentClaimStore, 'claimAgentGraphIntent'>;
  newId: () => string;
  /**
   * Stable execution input resolved before admission.
   *
   * This must be bound before the durable claim is written so a crash after
   * admission but before the Runtime message is durable cannot retry the same
   * intent with different work.
   */
  executionInput: {
    prompt: string;
  };
}

/**
 * Claims a deterministic readiness intent without invoking Agent runtime.
 *
 * The store is the admission authority. Proposed ids are disposable on an
 * idempotent retry: the persisted turn/run identity always wins.
 */
export function claimAgentGraphRunnableIntent(
  input: ClaimAgentGraphRunnableIntentInput,
): Promise<AgentGraphIntentClaimResult> {
  if (!input.executionInput.prompt.trim()) {
    throw new Error('Agent graph execution prompt must not be empty');
  }
  const intentFingerprint = stableHash({
    schemaVersion: AGENT_GRAPH_EXECUTION_INPUT_SCHEMA_VERSION,
    intent: input.intent,
    executionInput: input.executionInput,
  });
  const claimHash = stableHash({
    schemaVersion: AGENT_GRAPH_INTENT_CLAIM_SCHEMA_VERSION,
    graphId: input.intent.graphId,
    intentId: input.intent.intentId,
  });
  return input.store.claimAgentGraphIntent({
    schemaVersion: AGENT_GRAPH_INTENT_CLAIM_SCHEMA_VERSION,
    claimId: `graph_claim_${claimHash.slice('sha256:'.length, 'sha256:'.length + 32)}`,
    graphId: input.intent.graphId,
    intentId: input.intent.intentId,
    intentFingerprint,
    readinessContextFingerprint: input.intent.readinessContextFingerprint,
    targetOperatorId: input.intent.operatorId,
    targetSessionId: input.intent.targetSessionId,
    targetTurnId: input.newId(),
    targetRunId: input.newId(),
  });
}
