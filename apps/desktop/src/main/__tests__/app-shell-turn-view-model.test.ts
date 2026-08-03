import { strict as assert } from 'node:assert';
import { describe, it } from 'node:test';
import { SANDBOX_BOUNDARY_RESTART_CLOSURE_CLASS } from '@maka/core';
import type { StoredMessage } from '@maka/core';
import { deriveAppShellTurnViewModel } from '../../renderer/app-shell-turn-view-model.js';
import { latestInterruptedResumeTurnId } from '../../renderer/interrupted-resume.js';

function derive(messages: StoredMessage[]) {
  return deriveAppShellTurnViewModel({
    activeId: 'session-1',
    messages,
    pendingTurnActions: new Set(),
    uiLocale: 'zh',
    pendingKeyOf: (sessionId, turnId, actionId) => `${sessionId}:${turnId}:${actionId}`,
  });
}

function failedToolMessages(input: {
  sandboxDenied: boolean;
  includeOrdinaryFailure?: boolean;
  errorClass?: string;
}): StoredMessage[] {
  const turnId = 'turn-1';
  const messages: StoredMessage[] = [
    { type: 'user', id: 'user-1', turnId, ts: 1, text: 'run it' },
    { type: 'tool_call', id: 'tool-1', turnId, ts: 2, toolName: 'Grep', args: {} },
    {
      type: 'tool_result',
      id: 'result-1',
      turnId,
      ts: 3,
      toolUseId: 'tool-1',
      isError: true,
      content: {
        kind: 'text',
        text: 'Operation not permitted',
        ...(input.sandboxDenied
          ? { sandboxDenial: { likely: true as const, backend: 'macos-seatbelt' as const } }
          : {}),
      },
    },
  ];
  if (input.includeOrdinaryFailure) {
    messages.push(
      { type: 'tool_call', id: 'tool-2', turnId, ts: 4, toolName: 'Read', args: {} },
      {
        type: 'tool_result',
        id: 'result-2',
        turnId,
        ts: 5,
        toolUseId: 'tool-2',
        isError: true,
        content: { kind: 'text', text: 'File not found' },
      },
    );
  }
  messages.push({
    type: 'turn_state',
    id: 'state-1',
    turnId,
    ts: 6,
    status: 'failed',
    ...(input.errorClass ? { errorClass: input.errorClass } : {}),
    partialOutputRetained: true,
  });
  return messages;
}

describe('deriveAppShellTurnViewModel interrupted recovery', () => {
  it('offers safe resume only for the latest app-restarted failed turn', () => {
    const turnId = latestInterruptedResumeTurnId([
      { turnId: 'turn-1', status: 'failed', errorClass: 'app_restarted' },
    ]);

    assert.equal(turnId, 'turn-1');
  });

  it('removes the action after a later turn completes', () => {
    const turnId = latestInterruptedResumeTurnId([
      { turnId: 'turn-1', status: 'failed', errorClass: 'app_restarted' },
      { turnId: 'turn-2', status: 'completed' },
    ]);

    assert.equal(turnId, undefined);
  });

  it('explains a turn whose sandbox boundary request the restart closed (#1612)', () => {
    const viewModel = derive([
      { type: 'user', id: 'user-1', turnId: 'turn-1', ts: 1, text: 'build it' },
      {
        type: 'turn_state',
        id: 'state-1',
        turnId: 'turn-1',
        ts: 2,
        status: 'failed',
        errorClass: SANDBOX_BOUNDARY_RESTART_CLOSURE_CLASS,
        partialOutputRetained: false,
      },
    ]);

    assert.match(viewModel.turnFailedReasonLabels['turn-1'] ?? '', /「允许访问工作区以外的内容」请求已按拒绝关闭/);
    assert.match(viewModel.turnFailedRecoveryLabels['turn-1'] ?? '', /重试本轮/);
    // Answering is impossible after the restart, so this turn must not be
    // offered as a safe-resume candidate the way a plain restart is.
    assert.equal(viewModel.resumeCandidateTurnId, undefined);
  });
});

describe('deriveAppShellTurnViewModel sandbox denial presentation', () => {
  it('omits the duplicate failed-turn banner for a sandbox-only tool failure', () => {
    const viewModel = derive(failedToolMessages({
      sandboxDenied: true,
    }));

    assert.equal(viewModel.turnFailedReasonLabels['turn-1'], undefined);
    assert.equal(viewModel.turnFailedRecoveryLabels['turn-1'], undefined);
  });

  it('keeps the failed-turn banner for an ordinary tool failure', () => {
    const viewModel = derive(failedToolMessages({
      sandboxDenied: false,
      errorClass: 'tool_failed',
    }));

    assert.equal(typeof viewModel.turnFailedReasonLabels['turn-1'], 'string');
    assert.equal(typeof viewModel.turnFailedRecoveryLabels['turn-1'], 'string');
  });

  it('keeps the failed-turn banner when a sandbox denial and ordinary failure coexist', () => {
    const viewModel = derive(failedToolMessages({
      sandboxDenied: true,
      includeOrdinaryFailure: true,
      errorClass: 'tool_failed',
    }));

    assert.equal(typeof viewModel.turnFailedReasonLabels['turn-1'], 'string');
    assert.equal(typeof viewModel.turnFailedRecoveryLabels['turn-1'], 'string');
  });

  it('keeps the failed-turn banner for a separate turn-level failure', () => {
    const viewModel = derive(failedToolMessages({
      sandboxDenied: true,
      errorClass: 'network',
    }));

    assert.equal(typeof viewModel.turnFailedReasonLabels['turn-1'], 'string');
    assert.equal(typeof viewModel.turnFailedRecoveryLabels['turn-1'], 'string');
  });
});

/**
 * A memoized `TurnView` compares every prop, not just `turn`. The incremental
 * transcript projection (#2030) keeps a turn's object identity across the
 * `refreshMessages` that fires at each step and tool boundary, but these
 * per-turn props are derived beside it from a freshly deserialized `messages`
 * array — so if they are rebuilt every time, the memo fails on the footer and
 * the whole transcript re-renders anyway, and the projection buys nothing.
 */
describe('AppShell turn view model prop identity', () => {
  const transcript: StoredMessage[] = [
    { type: 'user', id: 'user-1', turnId: 'turn-1', ts: 1, text: 'first' },
    { type: 'assistant', id: 'assistant-1', turnId: 'turn-1', ts: 2, text: 'first answer', modelId: 'model-1' },
    { type: 'turn_state', id: 'state-1', turnId: 'turn-1', ts: 3, status: 'completed', partialOutputRetained: false },
    { type: 'user', id: 'user-2', turnId: 'turn-2', ts: 4, text: 'second' },
    { type: 'assistant', id: 'assistant-2', turnId: 'turn-2', ts: 5, text: 'second answer', modelId: 'model-1' },
    { type: 'turn_state', id: 'state-2', turnId: 'turn-2', ts: 6, status: 'completed', partialOutputRetained: false },
  ];

  function deriveWith(messages: StoredMessage[], previous?: ReturnType<typeof derive>) {
    return deriveAppShellTurnViewModel({
      activeId: 'session-1',
      messages,
      pendingTurnActions: new Set(),
      uiLocale: 'zh',
      pendingKeyOf: (sessionId, turnId, actionId) => `${sessionId}:${turnId}:${actionId}`,
      ...(previous ? { previous } : {}),
    });
  }

  it('keeps every per-turn prop identical across a refresh that changed nothing', () => {
    const before = deriveWith(transcript);
    // A refresh re-reads the ledger over IPC: same values, all-new objects.
    const after = deriveWith(structuredClone(transcript), before);

    assert.equal(after, before, 'a refresh that changed nothing must not move the view model at all');
    for (const turnId of ['turn-1', 'turn-2']) {
      assert.equal(
        after.turnFooterActionsByTurn[turnId],
        before.turnFooterActionsByTurn[turnId],
        `${turnId} footer actions must keep identity`,
      );
    }
  });

  it('moves only the turn whose derived props actually changed', () => {
    const before = deriveWith(transcript);
    const after = deriveWith(
      [
        ...structuredClone(transcript),
        { type: 'user', id: 'user-3', turnId: 'turn-3', ts: 7, text: 'third' },
      ],
      before,
    );

    assert.notEqual(after, before);
    assert.equal(after.turnFooterActionsByTurn['turn-1'], before.turnFooterActionsByTurn['turn-1']);
    assert.equal(after.turnFooterActionsByTurn['turn-2'], before.turnFooterActionsByTurn['turn-2']);
    assert.ok(after.turnFooterActionsByTurn['turn-3'], 'the new turn gets its own footer actions');
  });

  it('moves a turn whose footer actions genuinely changed', () => {
    const before = deriveWith(transcript);
    // An answer that lost its text disables `copy` — a real footer change.
    const emptied = structuredClone(transcript);
    emptied[4] = { type: 'assistant', id: 'assistant-2', turnId: 'turn-2', ts: 5, text: '', modelId: 'model-1' };
    const after = deriveWith(emptied, before);

    assert.equal(after.turnFooterActionsByTurn['turn-1'], before.turnFooterActionsByTurn['turn-1']);
    assert.notEqual(after.turnFooterActionsByTurn['turn-2'], before.turnFooterActionsByTurn['turn-2']);
  });
});
