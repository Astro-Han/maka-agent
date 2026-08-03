import assert from 'node:assert/strict';
import { describe, it } from 'node:test';
import type { SandboxBoundaryRequestEvent, SessionEventStreamSnapshot, SessionSummary } from '@maka/core';
import { armLiveTurn } from '@maka/ui';
import { settledSessionTransientIds } from '../../renderer/settled-session-transients.js';
import {
  clearAppShellSessionUiStateForSession,
  createAppShellSessionUiStateController,
  createInitialAppShellSessionUiState,
  type AppShellSessionUiState,
} from '../../renderer/app-shell-session-ui-state.js';

function boundaryRequest(requestId: string): SandboxBoundaryRequestEvent {
  return {
    type: 'sandbox_boundary_request',
    id: `event-${requestId}`,
    turnId: 'turn-1',
    ts: 1,
    requestId,
    toolUseId: `tool-${requestId}`,
    justification: 'Read an external file.',
    expansion: {
      filesystem: {
        entries: [{ path: '/outside/file', access: 'read', scope: 'exact' }],
      },
    },
  };
}

function healthSnapshot(sessionId: string): SessionEventStreamSnapshot {
  return { sessionId, status: 'connected', subscribedAt: 1, checkedAt: 1 };
}

function seededState(): AppShellSessionUiState {
  return {
    ...createInitialAppShellSessionUiState(),
    messageLoadErrorBySession: { drop: 'failed', keep: 'still failed' },
    messageRetryPendingBySession: { drop: true, keep: true },
    stopPendingBySession: { drop: true, keep: true },
    liveTurnBySession: { drop: armLiveTurn('turn-drop'), keep: armLiveTurn('turn-keep') },
    interactionBySession: {
      drop: [boundaryRequest('drop')],
      keep: [boundaryRequest('keep')],
    },
    pendingPermissionModeBySession: { drop: true, keep: true },
    pendingSessionModelBySession: { drop: true, keep: true },
  };
}

describe('app shell session UI state controller', () => {
  it('selects background terminal sessions without cutting off the active handoff', () => {
    const sessions = [
      { id: 'running', status: 'running' },
      { id: 'background', status: 'active' },
      { id: 'active', status: 'active' },
    ] as SessionSummary[];
    const background = { ...armLiveTurn('turn-background'), terminal: true as const };
    const active = { ...armLiveTurn('turn-active'), terminal: true as const };

    assert.deepEqual(settledSessionTransientIds({
      activeId: 'active',
      sessions,
      liveTurnBySession: { background, active },
    }), ['background']);
  });

  it('clears one session from every per-session UI map without touching other sessions', () => {
    const next = clearAppShellSessionUiStateForSession(seededState(), 'drop');

    assert.deepEqual(Object.keys(next.messageLoadErrorBySession), ['keep']);
    assert.deepEqual(Object.keys(next.messageRetryPendingBySession), ['keep']);
    assert.deepEqual(Object.keys(next.stopPendingBySession), ['keep']);
    assert.deepEqual(Object.keys(next.liveTurnBySession), ['keep']);
    assert.deepEqual(Object.keys(next.interactionBySession), ['keep']);
    assert.deepEqual(Object.keys(next.pendingPermissionModeBySession), ['keep']);
    assert.deepEqual(Object.keys(next.pendingSessionModelBySession), ['keep']);
  });

  it('keeps state identity for no-op map updates and only replaces the selected map', () => {
    const controller = createAppShellSessionUiStateController();
    const state = controller.getState();
    controller.setMessageLoadErrorBySession((current) => current);
    assert.equal(controller.getState(), state);

    controller.setMessageLoadErrorBySession((current) => ({ ...current, session: 'failed' }));
    const next = controller.getState();

    assert.notEqual(next, state);
    assert.deepEqual(next.messageLoadErrorBySession, { session: 'failed' });
    assert.equal(next.stopPendingBySession, state.stopPendingBySession);
    assert.equal(next.liveTurnBySession, state.liveTurnBySession);
  });

  it('records event-stream health without notifying render subscribers', () => {
    let notifications = 0;
    const controller = createAppShellSessionUiStateController(undefined, () => {
      notifications += 1;
    });
    const snapshot = healthSnapshot('session');

    controller.setSessionEventHealthBySession((current) => ({ ...current, session: snapshot }));

    assert.equal(controller.sessionEventHealthBySessionRef.current.session, snapshot);
    assert.equal(notifications, 0, 'stream health has no render consumer, so it must not force one');

    controller.setMessageLoadErrorBySession((current) => ({ ...current, session: 'failed' }));

    assert.equal(notifications, 1, 'maps that are rendered still notify');
  });

  it('drops event-stream health along with the rest of a cleared session', () => {
    const controller = createAppShellSessionUiStateController();
    controller.setSessionEventHealthBySession(() => ({
      drop: healthSnapshot('drop'),
      keep: healthSnapshot('keep'),
    }));

    controller.clearSessionUiState('drop');

    assert.deepEqual(Object.keys(controller.sessionEventHealthBySessionRef.current), ['keep']);
  });

  it('keeps the synchronous live-turn ref aligned with reducer updates', () => {
    const controller = createAppShellSessionUiStateController();
    const projection = armLiveTurn('turn-1');
    controller.setLiveTurnBySession((current) => ({ ...current, session: projection }));
    assert.equal(controller.liveTurnBySessionRef.current.session, projection);
  });
});
