/**
 * Map a tool_result onto the UI activity status used by cards/trows.
 * Cancel / abort are user-or-system stops, not tool failures.
 */
import type { ToolResultContent } from './events.js';

export type SettledToolActivityStatus = 'completed' | 'errored' | 'interrupted';

/**
 * A call that has not settled. `pending` is where `tool_start` opens one; it
 * only reaches `running` once output arrives, so a tool that never streams
 * stays `pending` for its whole life and both must read as in flight.
 */
export type InFlightToolActivityStatus = 'pending' | 'running';

/** The whole tool-row status vocabulary, owned here so it is spelled once. */
export type ToolActivityStatus = InFlightToolActivityStatus | SettledToolActivityStatus;

export function isInFlightToolStatus(status: ToolActivityStatus): boolean {
  return status === 'pending' || status === 'running';
}

/** Terminal / shell_run results whose runtime status is explicit cancel. */
export function isCancelledToolResultContent(content: ToolResultContent | undefined): boolean {
  if (!content) return false;
  if (content.kind === 'terminal' || content.kind === 'shell_run') {
    return content.status === 'cancelled';
  }
  if (content.kind === 'agent_swarm') return content.status === 'cancelled';
  return false;
}

/**
 * Derive settled ToolActivityItem.status from tool_result flags + content.
 *
 * `isError` is the call-level contract: a successful observation of a
 * cancelled background task (`StopBackgroundTask` → shell_run cancelled,
 * isError:false) is `completed`, not interrupted. Only failed cancels and
 * aborted explore agents map to `interrupted`.
 */
export function toolResultActivityStatus(
  isError: boolean,
  content: ToolResultContent | undefined,
): SettledToolActivityStatus {
  if (!isError) return 'completed';
  // Failed cancel (user stop / kill) — not a tool failure banner.
  if (isCancelledToolResultContent(content)) return 'interrupted';
  if (content?.kind === 'explore_agent' && content.reason === 'aborted') {
    return 'interrupted';
  }
  return 'errored';
}
