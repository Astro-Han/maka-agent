import type { ExecutionBoundary, PermissionMode } from '@maka/core';
import { isReadOnlyPermissionProfile } from '@maka/core';

export interface DesktopExecutionBoundarySurface {
  permissionMode: PermissionMode | undefined;
  localInteractionAvailable: boolean;
}

export function deriveDesktopExecutionBoundarySurface(
  activeSessionId: string | undefined,
  boundary: ExecutionBoundary | undefined,
  fallbackMode: PermissionMode,
): DesktopExecutionBoundarySurface {
  if (!activeSessionId) {
    return {
      permissionMode: fallbackMode,
      localInteractionAvailable: true,
    };
  }
  if (!boundary || boundary.kind === 'external') {
    return {
      permissionMode: undefined,
      localInteractionAvailable: false,
    };
  }
  // #1611: a managed boundary carries the whole profile, so a read-only
  // session is a distinct fact from a writable one. Collapsing both to `ask`
  // labelled read-only sessions "Auto" and gave them the Auto hint, which
  // describes permissions they do not have. `explore` is the existing
  // read-only display state; the option list is unchanged, so the user can
  // still switch a read-only session to full access.
  return {
    permissionMode:
      boundary.kind === 'bypass'
        ? 'bypass'
        : isReadOnlyPermissionProfile(boundary.profile)
          ? 'explore'
          : 'ask',
    localInteractionAvailable: true,
  };
}
