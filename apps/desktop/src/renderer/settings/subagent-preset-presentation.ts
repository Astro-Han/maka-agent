import {
  connectionEnabledModelIds,
  type LlmConnection,
  type SubagentPreset,
} from '@maka/core';
import type { StatusTone } from './settings-status-badge.js';

export type SubagentPresetAvailabilityKind =
  | 'available'
  | 'disabled'
  | 'missing_connection'
  | 'connection_disabled'
  | 'model_disabled';

export type SubagentPresetAvailability = {
  kind: SubagentPresetAvailabilityKind;
  tone: StatusTone;
};

export function subagentPresetAvailability(
  preset: SubagentPreset,
  connections: readonly LlmConnection[],
): SubagentPresetAvailability {
  if (!preset.enabled) return { kind: 'disabled', tone: 'neutral' };
  const connection = connections.find((candidate) => candidate.slug === preset.connectionSlug);
  if (!connection) return { kind: 'missing_connection', tone: 'destructive' };
  if (!connection.enabled) return { kind: 'connection_disabled', tone: 'warning' };
  if (!connectionEnabledModelIds(connection).includes(preset.model)) {
    return { kind: 'model_disabled', tone: 'warning' };
  }
  return { kind: 'available', tone: 'success' };
}

/**
 * Typing the display name fills the id — until the user takes the id over.
 *
 * The rule lives here rather than inside the editor because it is state math,
 * not rendering: `idWasEdited` is the whole contract ("the id is derived until
 * the user says otherwise, and an existing preset's id is never derived"), and
 * an editor that quietly stops honouring it looks identical on screen.
 */
export function nextSubagentDraftForName<Draft extends { name: string; id: string }>(
  draft: Draft,
  name: string,
  idWasEdited: boolean,
  existingIds: ReadonlySet<string>,
): Draft {
  if (idWasEdited) return { ...draft, name };
  return { ...draft, name, id: suggestSubagentPresetId(name, existingIds) };
}

export function suggestSubagentPresetId(
  name: string,
  existingIds: ReadonlySet<string>,
): string {
  const normalized = name
    .normalize('NFKD')
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 96);
  const base = normalized || 'subagent';
  if (!existingIds.has(base)) return base;
  for (let suffix = 2; suffix < 10_000; suffix += 1) {
    const candidate = `${base}-${suffix}`;
    if (!existingIds.has(candidate)) return candidate;
  }
  return `${base}-${Date.now()}`;
}
