/*
 * Licensed to the Apache Software Foundation (ASF) under one
 * or more contributor license agreements.  See the NOTICE file
 * distributed with this work for additional information
 * regarding copyright ownership.  The ASF licenses this file
 * to you under the Apache License, Version 2.0 (the
 * "License"); you may not use this file except in compliance
 * with the License.  You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing,
 * software distributed under the License is distributed on an
 * "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
 * KIND, either express or implied.  See the License for the
 * specific language governing permissions and limitations
 * under the License.
 */

/** Footer availability follows settled Turn state, not optimistic transcript content. */

import type { TurnStatus } from '@maka/core/session';

import type { UiLocale } from '@maka/core/ui-locale';
import { getDesktopConversationCopy } from './locales/conversation-copy.js';

export type TurnFooterActionId = 'branch' | 'copy';

export interface TurnFooterAction {
  id: TurnFooterActionId;
  /** Chinese button label. */
  label: string;
  /**
   * Whether the button is enabled for this turn. A disabled button is
   * still rendered (so the user can see what actions exist on the
   * turn) but the click handler is a no-op. UI may also hide
   * disabled actions in compact mode.
   */
  enabled: boolean;
  /**
   * Tooltip explaining why the action is enabled/disabled. Always
   * Chinese; never exposes the raw TurnStatus enum identifier.
   */
  tooltip?: string;
  /** Busy from click until the action settles — UI renders the spinner. */
  pending?: boolean;
}

export interface TurnFooterContext {
  status: TurnStatus;
  /**
   * True when the turn has at least one materialized assistant message
   * with non-empty text. Disables `copy` for empty turns (running
   * turns before the first delta, or aborted with no partial output).
   */
  hasContent: boolean;
  /**
   * Per @kenji review: prevent double-click duplicate sibling turns.
   * The renderer marks an action `pending` from click time until
   * `sessions:changed` (or timeout) clears it; the footer renders that
   * action as disabled + busy with a "正在处理…" tooltip. Other turns
   * / other action types stay clickable.
   */
  pendingActions?: ReadonlySet<TurnFooterActionId>;
  locale: UiLocale;
}

/**
 * Derive the ordered list of footer actions to render for a turn.
 * The order is fixed at the matrix level (branch → copy)
 * so adjacent buttons line up across rows even when some are disabled.
 *
 * @kenji gate: returned `enabled` flags depend only on `TurnStatus`
 * and lineage state; we never sniff the turn text or fall back to
 * optimistic guesses.
 */
export function deriveTurnFooterActions(input: TurnFooterContext): TurnFooterAction[] {
  const { status, hasContent, pendingActions } = input;
  const copyText = getDesktopConversationCopy(input.locale).footer;
  const actionLabel = copyText.labels;
  const isPending = (id: TurnFooterActionId) => pendingActions?.has(id) ?? false;
  const PENDING_TOOLTIP = copyText.pending;

  const branch: TurnFooterAction = isPending('branch')
    ? { id: 'branch', label: actionLabel.branch, enabled: false, tooltip: PENDING_TOOLTIP, pending: true }
    : {
        id: 'branch',
        label: actionLabel.branch,
        enabled: status !== 'running',
        tooltip:
          status === 'running'
            ? copyText.branchRunning
            : status === 'aborted'
            ? copyText.branchAborted
            : copyText.branch,
      };
  const copy: TurnFooterAction = {
    id: 'copy',
    label: actionLabel.copy,
    enabled: hasContent,
    tooltip: hasContent ? copyText.copy : copyText.copyEmpty,
  };

  return [branch, copy];
}
