import type { LiveTurnProjection } from '@maka/ui';
import type { TurnPhase } from './model-wait-state.js';
import { hasInFlightToolActivity } from './session-event-health.js';

/**
 * The low-entropy reading of a live turn: everything the shell derives from the
 * active projection EXCEPT the streamed content itself.
 *
 * #1985: a text delta changes the projection on every token, but it changes
 * none of these values once a turn is under way. The shell (and through it the
 * sidebar, the composer, and every non-chat surface) subscribes to this
 * snapshot, so a stream only re-renders the chat transcript — the one surface
 * that genuinely reads the growing text.
 *
 * Keep this free of buffers, arrays, and maps. Anything whose identity changes
 * per delta belongs to the chat surface, not here.
 */
export interface LiveTurnSnapshot {
  /** Turn phase, or undefined when no turn is in flight (incl. a settled one). */
  phase: TurnPhase | undefined;
  /** Whether the active text step has emitted anything yet. */
  hasStreamingText: boolean;
  /** Whether that text step is closed. */
  streamingTextComplete: boolean;
  /** Step id of the settled answer, once complete — the handoff key. */
  streamingMessageId: string | undefined;
  /** Whether the active reasoning step has emitted anything yet. */
  hasThinkingText: boolean;
  /** Whether the turn has any tool activity at all. */
  hasLiveTools: boolean;
  /** Whether any tool is still pending / running / awaiting permission. */
  hasInFlightTools: boolean;
}

const NO_LIVE_TURN: LiveTurnSnapshot = {
  phase: undefined,
  hasStreamingText: false,
  streamingTextComplete: false,
  streamingMessageId: undefined,
  hasThinkingText: false,
  hasLiveTools: false,
  hasInFlightTools: false,
};

export function deriveLiveTurnSnapshot(projection: LiveTurnProjection | undefined): LiveTurnSnapshot {
  if (!projection) return NO_LIVE_TURN;
  const steps = projection.steps;
  const textStep = findLast(steps, (step) => Boolean(step.text));
  const thinkingStep = findLast(steps, (step) => Boolean(step.thinking));
  const tools = steps.flatMap((step) => step.tools);
  const streamingTextComplete = textStep?.text?.complete === true;
  return {
    phase: projection.terminal ? undefined : projection.phase,
    hasStreamingText: (textStep?.text?.text.length ?? 0) > 0,
    streamingTextComplete,
    streamingMessageId: streamingTextComplete ? textStep?.stepId : undefined,
    hasThinkingText: (thinkingStep?.thinking?.text.length ?? 0) > 0,
    hasLiveTools: tools.length > 0,
    hasInFlightTools: hasInFlightToolActivity(tools),
  };
}

export function liveTurnSnapshotsEqual(a: LiveTurnSnapshot, b: LiveTurnSnapshot): boolean {
  return (
    a.phase === b.phase &&
    a.hasStreamingText === b.hasStreamingText &&
    a.streamingTextComplete === b.streamingTextComplete &&
    a.streamingMessageId === b.streamingMessageId &&
    a.hasThinkingText === b.hasThinkingText &&
    a.hasLiveTools === b.hasLiveTools &&
    a.hasInFlightTools === b.hasInFlightTools
  );
}

function findLast<T>(items: readonly T[], predicate: (item: T) => boolean): T | undefined {
  for (let index = items.length - 1; index >= 0; index -= 1) {
    const item = items[index];
    if (item !== undefined && predicate(item)) return item;
  }
  return undefined;
}
