import assert from 'node:assert/strict';
import { describe, it } from 'node:test';
import {
  createDangerFullAccessPermissionProfile,
  createReadOnlyPermissionProfile,
  createWorkspaceWritePermissionProfile,
} from '@maka/core';

import { deriveDesktopExecutionBoundarySurface } from '../../renderer/desktop-execution-boundary-surface.js';

describe('Desktop execution boundary surface', () => {
  it('projects managed and bypass boundaries to their user-facing controls', () => {
    assert.deepEqual(
      deriveDesktopExecutionBoundarySurface(
        'managed',
        {
          kind: 'managed',
          profile: createWorkspaceWritePermissionProfile(),
          revision: 0,
        },
        'bypass',
      ),
      {
        permissionMode: 'ask',
        localInteractionAvailable: true,
      },
    );
    assert.deepEqual(
      deriveDesktopExecutionBoundarySurface(
        'bypass',
        { kind: 'bypass', revision: 1 },
        'ask',
      ),
      {
        permissionMode: 'bypass',
        localInteractionAvailable: true,
      },
    );
  });

  it('shows a read-only managed boundary as read-only instead of Auto (#1611)', () => {
    assert.deepEqual(
      deriveDesktopExecutionBoundarySurface(
        'read-only',
        {
          kind: 'managed',
          profile: createReadOnlyPermissionProfile(),
          revision: 0,
        },
        'ask',
      ),
      {
        permissionMode: 'explore',
        localInteractionAvailable: true,
      },
    );
    // Once an approved expansion grants a write, the session is no longer
    // read-only and must stop claiming to be.
    assert.deepEqual(
      deriveDesktopExecutionBoundarySurface(
        'expanded',
        {
          kind: 'managed',
          profile: {
            ...createReadOnlyPermissionProfile(),
            fileSystem: {
              kind: 'restricted',
              entries: [
                { kind: 'special', access: 'read', special: ':workspace_roots' },
                { kind: 'path', access: 'write', path: '/workspace/out', match: 'subtree' },
              ],
            },
          },
          revision: 2,
        },
        'ask',
      ),
      {
        permissionMode: 'ask',
        localInteractionAvailable: true,
      },
    );
    assert.deepEqual(
      deriveDesktopExecutionBoundarySurface(
        'danger-full-access',
        {
          kind: 'managed',
          profile: createDangerFullAccessPermissionProfile(),
          revision: 0,
        },
        'ask',
      ),
      {
        permissionMode: 'ask',
        localInteractionAvailable: true,
      },
    );
  });

  it('fails closed while authority is loading and keeps External history non-interactive', () => {
    assert.deepEqual(
      deriveDesktopExecutionBoundarySurface('loading', undefined, 'ask'),
      {
        permissionMode: undefined,
        localInteractionAvailable: false,
      },
    );
    assert.deepEqual(
      deriveDesktopExecutionBoundarySurface(
        'external',
        { kind: 'external', revision: 0 },
        'ask',
      ),
      {
        permissionMode: undefined,
        localInteractionAvailable: false,
      },
    );
  });
});
