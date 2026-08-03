import type { ShellRunUpdate, StoredMessage } from '@maka/core';
import type { LiveTurnProjection } from './live-turn-projection.js';
import {
  applyShellRunOverlayEntry,
  foldShellRunUpdates,
  materializeTurns,
  overlayLiveTurn,
  projectTurnTools,
  type ShellRunOverlayEntry,
  type ToolActivityItem,
  type TurnViewModel,
} from './materialize.js';

/**
 * Incremental transcript projection (issue #2030).
 *
 * The runtime event stream is already incremental — every delta names the turn
 * it belongs to — but flattening it into a `messages` snapshot throws that away,
 * and the old composition tried to guess it back downstream with `memo`'s
 * reference comparison over three chained pure derivations. Two of those
 * derivations were not idempotent, so the guess was wrong on every token for
 * any session with background-command history.
 *
 * This layer owns the derived state instead of re-deriving it:
 *
 *  - it remembers the settled turns and reuses the previous object for any turn
 *    a message refresh did not actually change;
 *  - it remembers the result of applying each shell-run update to each tool, so
 *    a durable revision that permanently leads the persisted `tool_result`
 *    snapshot is merged once rather than on every render;
 *  - it reports `affectedTurnIds` — the turns whose object identity moved — so
 *    "what changed" is an answer this layer gives rather than one the renderer
 *    infers.
 *
 * Contract (mirrors `parseMarkdownIncremental`'s in `@astryxdesign/core`):
 * a turn keeps its object identity unless its projected value changed, and
 * projecting the same inputs twice returns the same result object, so calling
 * this during render stays safe under double-invocation.
 */
export interface TranscriptProjectionInput {
  /** Resets the owned state when it changes, so no state crosses sessions. */
  sessionId?: string;
  messages: readonly StoredMessage[];
  liveTurn?: LiveTurnProjection;
  shellRunUpdates?: readonly ShellRunUpdate[];
}

export interface TranscriptProjectionResult {
  turns: readonly TurnViewModel[];
  /**
   * Turn ids whose projected value moved in this step: added turns, turns whose
   * content changed, and turns that disappeared. A semantically identical
   * update yields an empty set. The first projection reports every turn.
   *
   * Derived from this layer's own output, never from the turn an event names.
   * A ShellRun result recorded under one turn folds into the Bash that owns its
   * `ref`, which can live in an earlier turn (`foldShellRunToolActivities`), so
   * a WriteStdin in turn-2 affects both turn-2 and turn-1. Only the projection
   * knows that ownership graph.
   */
  affectedTurnIds: ReadonlySet<string>;
}

export interface TranscriptProjection {
  project(input: TranscriptProjectionInput): TranscriptProjectionResult;
  reset(): void;
}

const NO_UPDATES: readonly ShellRunUpdate[] = [];
const NO_TURNS: readonly TurnViewModel[] = [];

interface AppliedOverlay {
  tool: ToolActivityItem;
  entry: ShellRunOverlayEntry;
  out: ToolActivityItem;
}

export function createTranscriptProjection(): TranscriptProjection {
  let sessionId: string | undefined;
  let hasProjected = false;

  // Stage inputs, remembered so a stage only reruns when its own input moved.
  let lastMessages: readonly StoredMessage[] | undefined;
  let lastLiveTurn: LiveTurnProjection | undefined;
  let lastUpdates: readonly ShellRunUpdate[] | undefined;

  // Stage outputs.
  let settledTurns: readonly TurnViewModel[] = NO_TURNS;
  let liveTurns: readonly TurnViewModel[] = NO_TURNS;
  // Tracked separately from `lastMessages` because a refresh can leave the
  // settled projection untouched, which must not force the live overlay to run.
  let liveTurnsFrom: readonly TurnViewModel[] | undefined;
  let overlayEntries: ReadonlyMap<string, ShellRunOverlayEntry> = new Map();
  const appliedByToolUseId = new Map<string, AppliedOverlay>();
  let lastResult: TranscriptProjectionResult = { turns: NO_TURNS, affectedTurnIds: new Set() };

  function reset(): void {
    hasProjected = false;
    lastMessages = undefined;
    lastLiveTurn = undefined;
    lastUpdates = undefined;
    settledTurns = NO_TURNS;
    liveTurns = NO_TURNS;
    liveTurnsFrom = undefined;
    overlayEntries = new Map();
    appliedByToolUseId.clear();
    lastResult = { turns: NO_TURNS, affectedTurnIds: new Set() };
  }

  function project(input: TranscriptProjectionInput): TranscriptProjectionResult {
    if (hasProjected && input.sessionId !== sessionId) reset();
    sessionId = input.sessionId;
    const updates = input.shellRunUpdates ?? NO_UPDATES;
    // Compared element-wise, not by array identity: the store publishes a new
    // ShellRunUpdate object for every change it accepts, so this holds even if
    // a caller reuses the array it hands us.
    const updatesMoved = lastUpdates === undefined
      || lastUpdates.length !== updates.length
      || updates.some((update, index) => update !== lastUpdates![index]);

    // Same inputs, same answer — including the same affected set, so a
    // double-invoked render never sees a step it did not cause.
    if (
      hasProjected
      && input.messages === lastMessages
      && input.liveTurn === lastLiveTurn
      && !updatesMoved
    ) {
      return lastResult;
    }

    if (input.messages !== lastMessages) {
      settledTurns = reconcileTurnIdentities(settledTurns, materializeTurns(input.messages));
      lastMessages = input.messages;
    }
    if (liveTurnsFrom !== settledTurns || input.liveTurn !== lastLiveTurn) {
      liveTurns = overlayLiveTurn(settledTurns, input.liveTurn);
      liveTurnsFrom = settledTurns;
      lastLiveTurn = input.liveTurn;
    }
    if (updatesMoved) {
      // Entry identity is made semantic here — an entry object survives a
      // refold iff its value (ref, revision, status, output, ownership) is
      // unchanged. That is what lets the per-tool memo below key on entry
      // identity without keying on the caller's update objects, and it runs
      // only when the store publishes, never per streamed token.
      overlayEntries = reuseEqualEntries(overlayEntries, foldShellRunUpdates(updates));
      lastUpdates = updates;
    }

    // Final identity reconciliation against what we last published: a stage
    // that rewrote a turn without changing its value hands the previous object
    // back, so "affected" means "the value moved", never "a pass ran".
    const overlaid = applyShellRunOverlay(liveTurns);
    const turns = hasProjected ? reconcileTurnIdentities(lastResult.turns, overlaid) : overlaid;
    const affectedTurnIds = diffAffectedTurnIds(hasProjected ? lastResult.turns : NO_TURNS, turns);
    hasProjected = true;
    lastResult = { turns, affectedTurnIds };
    return lastResult;
  }

  /**
   * Apply the folded shell-run updates to the turns' canonical tools, reusing
   * the previously produced tool object whenever neither the tool nor its
   * update moved. Without that memory the merge allocates on every call — a
   * background command's durable revision permanently leads the persisted
   * snapshot, so `changed` never goes false — and the owning turn is rebuilt
   * on every streamed token.
   */
  function applyShellRunOverlay(turns: readonly TurnViewModel[]): readonly TurnViewModel[] {
    if (overlayEntries.size === 0) return turns;
    const projected: ToolActivityItem[] = [];
    for (const turn of turns) {
      for (const tool of turn.tools) {
        const entry = overlayEntries.get(tool.toolUseId);
        if (!entry) {
          projected.push(tool);
          continue;
        }
        const applied = appliedByToolUseId.get(tool.toolUseId);
        if (applied && applied.tool === tool && applied.entry === entry) {
          projected.push(applied.out);
          continue;
        }
        const out = applyShellRunOverlayEntry(tool, entry);
        appliedByToolUseId.set(tool.toolUseId, { tool, entry, out });
        projected.push(out);
      }
    }
    return projectTurnTools(turns, projected);
  }

  return { project, reset };
}

function reuseEqualEntries(
  previous: ReadonlyMap<string, ShellRunOverlayEntry>,
  next: ReadonlyMap<string, ShellRunOverlayEntry>,
): ReadonlyMap<string, ShellRunOverlayEntry> {
  const reconciled = new Map<string, ShellRunOverlayEntry>();
  for (const [toolUseId, entry] of next) {
    const prior = previous.get(toolUseId);
    reconciled.set(toolUseId, prior && valuesEqual(prior, entry) ? prior : entry);
  }
  return reconciled;
}

/**
 * Keep the previous object for every turn whose projected value is unchanged.
 * A message refresh rebuilds the whole snapshot from freshly deserialized IPC
 * rows, so nothing upstream can carry identity — the equality check here is
 * what narrows a refresh to the turns whose messages actually changed.
 */
export function reconcileTurnIdentities(
  previous: readonly TurnViewModel[],
  next: readonly TurnViewModel[],
): readonly TurnViewModel[] {
  if (previous.length === 0) return next;
  const previousById = new Map(previous.map((turn) => [turn.turnId, turn]));
  const reconciled = next.map((turn) => {
    const prior = previousById.get(turn.turnId);
    return prior && valuesEqual(prior, turn) ? prior : turn;
  });
  return reconciled.length === previous.length && reconciled.every((turn, index) => turn === previous[index])
    ? previous
    : reconciled;
}

export function diffAffectedTurnIds(
  previous: readonly TurnViewModel[],
  next: readonly TurnViewModel[],
): ReadonlySet<string> {
  const affected = new Set<string>();
  const previousById = new Map(previous.map((turn) => [turn.turnId, turn]));
  const nextIds = new Set<string>();
  for (const turn of next) {
    nextIds.add(turn.turnId);
    if (previousById.get(turn.turnId) !== turn) affected.add(turn.turnId);
  }
  for (const turn of previous) {
    if (!nextIds.has(turn.turnId)) affected.add(turn.turnId);
  }
  return affected;
}

/**
 * Structural equality over the projected view model. Everything a turn holds is
 * plain JSON-shaped data (`args` and tool results included), so a value walk is
 * the whole comparison — with an identity short circuit that makes the common
 * "only the tail turn moved" case cheap.
 */
function valuesEqual(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (typeof a !== 'object' || typeof b !== 'object' || a === null || b === null) return false;
  if (Array.isArray(a) || Array.isArray(b)) {
    if (!Array.isArray(a) || !Array.isArray(b) || a.length !== b.length) return false;
    return a.every((item, index) => valuesEqual(item, b[index]));
  }
  const left = a as Record<string, unknown>;
  const right = b as Record<string, unknown>;
  const keys = new Set([...Object.keys(left), ...Object.keys(right)]);
  for (const key of keys) {
    if (!valuesEqual(left[key], right[key])) return false;
  }
  return true;
}
