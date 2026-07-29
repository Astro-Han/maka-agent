import assert from 'node:assert/strict';
import { describe, it } from 'node:test';
import { createReadOnlyPermissionProfile, createWorkspaceWritePermissionProfile } from '@maka/core';
import type { SessionEvent } from '@maka/core';

import { createAppShellSessionEventHandlers } from '../../renderer/app-shell-session-events.js';
import { activeExecutionBoundaryOf } from '../../renderer/use-active-execution-boundary.js';
import { deriveDesktopExecutionBoundarySurface } from '../../renderer/desktop-execution-boundary-surface.js';

const readOnly = {
  kind: 'managed',
  profile: createReadOnlyPermissionProfile(),
  revision: 0,
} as const;
const widened = {
  kind: 'managed',
  profile: createWorkspaceWritePermissionProfile(),
  revision: 1,
} as const;

describe('Active execution boundary read model', () => {
  it('never shows one session the boundary read for another', () => {
    const snapshot = { sessionId: 'session-a', boundary: readOnly };

    assert.equal(activeExecutionBoundaryOf(snapshot, 'session-a'), readOnly);
    // Switching sessions falls closed until the new session's boundary is read,
    // rather than briefly attributing the old session's permissions to it.
    assert.equal(activeExecutionBoundaryOf(snapshot, 'session-b'), undefined);
    assert.equal(activeExecutionBoundaryOf(snapshot, undefined), undefined);
    assert.equal(activeExecutionBoundaryOf(undefined, 'session-a'), undefined);
  });

  it('a stale snapshot would misreport permissions the user just granted (#1611)', () => {
    // Why the reload below has to exist: the two boundaries differ only in
    // revision + profile, and they drive different labels.
    assert.equal(
      deriveDesktopExecutionBoundarySurface('session-a', readOnly, 'ask').permissionMode,
      'explore',
    );
    assert.equal(
      deriveDesktopExecutionBoundarySurface('session-a', widened, 'ask').permissionMode,
      'ask',
    );
  });
});

describe('Boundary decisions notify the read model', () => {
  function handlersWithRecorder() {
    const boundaryChanges: string[] = [];
    const handlers = createAppShellSessionEventHandlers({
      uiLocale: 'zh',
      activeIdRef: { current: 'session-a' },
      liveTurnBySessionRef: { current: {} },
      refreshMessages: async () => true,
      refreshSessions: async () => [],
      setLiveTurnBySession: () => {},
      setInteractionBySession: () => {},
      onExecutionBoundaryChanged: (sessionId) => boundaryChanges.push(sessionId),
      showModelSetupToast: () => {},
      toastApi: { error: () => {} },
    });
    return { handlers, boundaryChanges };
  }

  it('re-reads authority when a boundary decision is acknowledged', () => {
    const { handlers, boundaryChanges } = handlersWithRecorder();

    handlers.handleEvent('session-a', {
      type: 'sandbox_boundary_decision_ack',
      id: 'event-ack',
      turnId: 'turn-1',
      ts: 1,
      requestId: 'request-1',
      toolUseId: 'tool-1',
      decision: 'allow',
      status: 'approved',
      revision: 1,
    } satisfies SessionEvent);

    // Approving an expansion moves only the boundary's revision: no session
    // field changes, so without this signal the surface would keep rendering
    // the permissions the session had before the user granted more.
    assert.deepEqual(boundaryChanges, ['session-a']);
  });

  it('does not re-read on events that cannot move a boundary', () => {
    const { handlers, boundaryChanges } = handlersWithRecorder();

    handlers.handleEvent('session-a', {
      type: 'sandbox_boundary_request',
      id: 'event-request',
      turnId: 'turn-1',
      ts: 1,
      requestId: 'request-1',
      toolUseId: 'tool-1',
      justification: 'write outside the workspace',
      expansion: {
        filesystem: { entries: [{ path: '/outside', access: 'write', scope: 'subtree' }] },
      },
    } satisfies SessionEvent);

    assert.deepEqual(boundaryChanges, []);
  });
});
