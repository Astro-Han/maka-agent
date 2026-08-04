import type { PlanReminder, PlanReminderStatus } from '@maka/core';

/**
 * Lifecycle tone for a reminder itself: paused is the only state that asks for
 * attention, completed is spent, everything else is simply live.
 *
 * Shared by the list page and the inspector so one reminder never reads as two
 * different severities depending on where it is shown.
 */
export function planReminderStatusDotVariant(
  status: PlanReminderStatus,
): 'accent' | 'warning' | 'neutral' {
  if (status === 'paused') return 'warning';
  if (status === 'completed') return 'neutral';
  return 'accent';
}

/**
 * Run-history status tone. triggered = it fired (informational accent, not a
 * health signal), blocked = intentionally skipped (warning), failed =
 * delivery error. Exception-only: no success green for a plain "it ran"
 * record.
 */
export function planRunStatusDotVariant(
  status: NonNullable<PlanReminder['lastRun']>['status'],
): 'accent' | 'warning' | 'error' {
  if (status === 'blocked') return 'warning';
  if (status === 'failed') return 'error';
  return 'accent';
}
