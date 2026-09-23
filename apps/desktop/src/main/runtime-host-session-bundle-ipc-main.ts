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

import type { IpcMain } from 'electron';
import type { SessionChangedReason } from '@maka/core/session';
import { RuntimeHostOperationError } from '@maka/runtime-host/client';
import type { WorkspaceTarget } from '@maka/runtime-host/protocol';
import type {
  SessionBundleExportIpcResult,
  SessionBundleFailure,
  SessionBundleFailureReason,
  SessionBundleImportIpcResult,
  SessionBundlePreviewIpcResult,
} from '../preload/bridge-contract.js';

/**
 * Settings › Import/export tasks — moving a Session between installations.
 *
 * The file is picked here and the work happens in the Runtime Host, because the
 * Host holds the Storage Root authority for its whole lifetime and the lock
 * that grants it is an election: it refuses a second exclusive hold even from
 * the process already holding one. Doing this in the desktop process would
 * require stopping the Host the user is looking at.
 */
type SessionBundleClient = {
  previewSessionBundle(sessionId: string): Promise<{
    readonly sessionCount: number; readonly subtreeDigest: string;
  }>;
  exportSessionBundle(input: {
    readonly sessionId: string;
    readonly destination: string;
    readonly expectedSubtreeDigest?: string;
  }): Promise<{ readonly sessionCount: number; readonly compressedBytes: number }>;
  importSessionBundle(input: {
    readonly source: string;
    readonly workspace: WorkspaceTarget;
  }): Promise<{ readonly sessionCount: number; readonly artifactFiles: number }>;
};

interface DialogController {
  showSaveDialog(options: {
    title?: string;
    defaultPath?: string;
    filters?: Array<{ name: string; extensions: string[] }>;
  }): Promise<{ canceled: boolean; filePath?: string }>;
  showOpenDialog(options: {
    title?: string;
    properties?: string[];
    filters?: Array<{ name: string; extensions: string[] }>;
  }): Promise<{ canceled: boolean; filePaths: string[] }>;
}

export interface RuntimeHostSessionBundleIpcDeps {
  readonly client: SessionBundleClient;
  readonly mainWindowController: DialogController;
  readonly emitSessionsChanged: (reason: SessionChangedReason, sessionId?: string) => void;
}

const BUNDLE_EXTENSION = 'maka-session';

export { bundleFileName as __bundleFileNameForTests };

export function registerRuntimeHostSessionBundleIpc(
  deps: RuntimeHostSessionBundleIpcDeps,
  ipcMain: { handle(channel: string, listener: Parameters<IpcMain['handle']>[1]): void },
): void {
  ipcMain.handle('session-bundle:preview', async (_event, sessionId: string) => {
    try {
      return { ok: true, ...await deps.client.previewSessionBundle(sessionId) } satisfies SessionBundlePreviewIpcResult;
    } catch (error) {
      return failure(error) satisfies SessionBundlePreviewIpcResult;
    }
  });
  ipcMain.handle('session-bundle:export', async (_event, ...args: unknown[]) => {
    const [sessionId, suggestedName, expectedSubtreeDigest] = args as [
      string,
      string,
      string,
    ];
    const picked = await deps.mainWindowController.showSaveDialog({
      defaultPath: `${bundleFileName(suggestedName)}.${BUNDLE_EXTENSION}`,
      filters: [{ name: 'Maka session', extensions: [BUNDLE_EXTENSION] }],
    });
    if (picked.canceled || !picked.filePath) {
      return { ok: false, reason: 'canceled' } satisfies SessionBundleExportIpcResult;
    }
    try {
      const result = await deps.client.exportSessionBundle({
        sessionId,
        destination: picked.filePath,
        expectedSubtreeDigest,
      });
      return {
        ok: true,
        sessionCount: result.sessionCount,
        path: picked.filePath,
      } satisfies SessionBundleExportIpcResult;
    } catch (error) {
      return failure(error) satisfies SessionBundleExportIpcResult;
    }
  });

  ipcMain.handle('session-bundle:import', async () => {
    const picked = await deps.mainWindowController.showOpenDialog({
      properties: ['openFile'],
      filters: [{ name: 'Maka session', extensions: [BUNDLE_EXTENSION] }],
    });
    const source = picked.filePaths[0];
    if (picked.canceled || !source) {
      return { ok: false, reason: 'canceled' } satisfies SessionBundleImportIpcResult;
    }
    try {
      const destination = await deps.mainWindowController.showOpenDialog({
        title: 'Workspace for imported sessions',
        properties: ['openDirectory'],
      });
      const path = destination.filePaths[0];
      if (destination.canceled || !path) {
        return { ok: false, reason: 'canceled' } satisfies SessionBundleImportIpcResult;
      }
      const result = await deps.client.importSessionBundle({
        source,
        workspace: { kind: 'host_path', path },
      });
      // The Host republishes the catalog, but the desktop shell keeps its own
      // list and only reads again when told.
      // No ids: the result is a count, because a list that grows with the
      // subtree can outgrow a frame after the Sessions are already committed.
      // Unscoped is what tells the shell to read its catalog again.
      deps.emitSessionsChanged('created');
      return { ok: true, sessionCount: result.sessionCount } satisfies SessionBundleImportIpcResult;
    } catch (error) {
      return failure(error) satisfies SessionBundleImportIpcResult;
    }
  });
}

/**
 * A task name is written by a person, and this one becomes a proposed filename.
 *
 * Separators would move the dialog somewhere the name does not say, a leading
 * dot proposes a hidden file, and a long name proposes one the filesystem may
 * refuse. The dialog is still where the path is decided -- this only decides
 * what it opens holding.
 */
function bundleFileName(name: unknown): string {
  const proposed = (typeof name === 'string' ? name : '')
    .replace(/[\u0000-\u001f/\\:*?"<>|]/g, ' ')
    .replace(/\s+/g, ' ')
    .replace(/^[.\s]+|[.\s]+$/g, '')
    .slice(0, 80)
    .trim();
  return proposed.length > 0 ? proposed : 'maka-session';
}

function failure(error: unknown): SessionBundleFailure {
  const reason = classify(error);
  if (reason !== 'failed') return { ok: false, reason };
  // Nothing downstream can act on `failed`, so the message is all the user has.
  // It also reaches the main-process log, because a reason code that says only
  // "no" is how a cause gets lost.
  console.error('[session-bundle] unclassified failure:', error);
  return {
    ok: false,
    reason,
    ...(error instanceof Error && error.message ? { detail: error.message } : {}),
  };
}

function classify(error: unknown): SessionBundleFailureReason {
  if (!(error instanceof RuntimeHostOperationError)) return 'failed';
  switch (error.code) {
    case 'not_found':
    case 'session_busy':
    case 'operation_conflict':
    case 'source_unreadable':
    case 'candidate_set_stale':
      return error.code;
    default:
      return 'failed';
  }
}
