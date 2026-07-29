import type { ExecutionBoundary } from '@maka/core';
import { useCallback, useEffect, useRef, useState } from 'react';

/**
 * A boundary snapshot together with the session it was read for, so a snapshot
 * can never be shown against a different session.
 */
export interface ActiveExecutionBoundarySnapshot {
  sessionId: string;
  boundary: ExecutionBoundary;
}

/**
 * The boundary belonging to `activeSessionId`, or `undefined` while none has
 * been read for it yet. Fails closed on session switches without needing an
 * explicit clear: a snapshot for another session simply does not match.
 */
export function activeExecutionBoundaryOf(
  snapshot: ActiveExecutionBoundarySnapshot | undefined,
  activeSessionId: string | undefined,
): ExecutionBoundary | undefined {
  if (!activeSessionId || snapshot?.sessionId !== activeSessionId) return undefined;
  return snapshot.boundary;
}

/**
 * The desktop's read model for the active session's execution boundary — the
 * one place that decides when the renderer's copy of the boundary is stale.
 *
 * The boundary is main-process authority, so an Effect synchronising with it is
 * the right tool; what this hook must never become is a mirror of renderer
 * state. It therefore keeps exactly one fact (the last snapshot read from main)
 * and re-reads on the two events that can change it: the active session
 * changing, and a caller reporting that a boundary decision settled (#1611).
 *
 * Before #1611 the surface displayed every managed boundary as Auto, so a stale
 * snapshot was invisible. Now that the label reports what the session may
 * actually do, staleness would be an active false statement about permissions:
 * a read-only session that has just been granted write access would keep
 * showing "read only". Approving an expansion only bumps the boundary's
 * revision — no session field changes — so nothing else here can notice it.
 */
export function useActiveExecutionBoundary(
  activeSessionId: string | undefined,
  /** Re-read when the session's stored permission mode changes under us. */
  permissionMode: string | undefined,
): {
  boundary: ExecutionBoundary | undefined;
  /** Report that this session's boundary may have changed; re-reads authority. */
  reload(sessionId: string): void;
} {
  const [snapshot, setSnapshot] = useState<ActiveExecutionBoundarySnapshot | undefined>();
  const [reloadNonce, setReloadNonce] = useState(0);
  const activeSessionIdRef = useRef(activeSessionId);
  useEffect(() => {
    activeSessionIdRef.current = activeSessionId;
  }, [activeSessionId]);

  useEffect(() => {
    if (!activeSessionId) return;
    let cancelled = false;
    void window.maka.sessions
      .readExecutionBoundary(activeSessionId)
      .then((boundary) => {
        if (!cancelled) setSnapshot({ sessionId: activeSessionId, boundary });
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [activeSessionId, permissionMode, reloadNonce]);

  const reload = useCallback((sessionId: string) => {
    // Only the active session is read here, so a decision settled on any other
    // session has nothing to refresh.
    if (activeSessionIdRef.current !== sessionId) return;
    setReloadNonce((nonce) => nonce + 1);
  }, []);

  return { boundary: activeExecutionBoundaryOf(snapshot, activeSessionId), reload };
}
