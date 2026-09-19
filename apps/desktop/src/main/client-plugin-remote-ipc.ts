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

import { HOST_OPERATION_SPECS, type PluginRemoteInput, type PluginRemoteResult } from '@maka/runtime-host/protocol';
import type { IpcMain, IpcMainInvokeEvent, WebContents } from 'electron';
import { isAbsolute } from 'node:path';
import type { PluginClientQueryInput } from '@maka/runtime-host/protocol';

interface DocumentOwner {
  readonly nonce: string;
  readonly documents: Set<string>;
  readonly pending: Set<Promise<unknown>>;
  pendingOpens: number;
  closed: boolean;
  dispose(): void;
}

/** Renderer documents own resources even though windows share a Host connection. */
export function registerClientPluginRemoteIpc(input: {
  readonly ipcMain: Pick<IpcMain, 'handle'>;
  readonly client: {
    request(operation: 'plugin.remote', input: PluginRemoteInput, timeoutMs?: number): Promise<PluginRemoteResult>;
  };
  readonly ownsRenderer: (contents: WebContents) => boolean;
  readonly report: (error: unknown) => void;
  readonly files?: {
    validate(input: PluginClientQueryInput): Promise<void>;
    pick(): Promise<string | null>;
    open(path: string): Promise<void>;
  };
}): () => Promise<void> {
  const owners = new Map<WebContents, DocumentOwner>();
  const draining = new Set<Promise<unknown>>();
  let closed = false;
  const request = (value: PluginRemoteInput) => input.client.request('plugin.remote', value, 40_000);
  const closeDocument = (document: string) => input.client.request(
    'plugin.remote', { kind: 'close_document', document }, 10_000,
  ).catch(input.report);
  const track = <T>(set: Set<Promise<unknown>>, work: Promise<T>): Promise<T> => {
    set.add(work);
    void work.then(() => set.delete(work), () => set.delete(work));
    return work;
  };
  const ownerFor = (event: IpcMainInvokeEvent, nonce: unknown): DocumentOwner => {
    const sender = event.sender;
    if (closed || sender.isDestroyed() || !input.ownsRenderer(sender) ||
      !event.senderFrame || event.senderFrame.frameToken !== sender.mainFrame.frameToken ||
      typeof nonce !== 'string' || !/^[0-9a-f-]{36}$/i.test(nonce))
      throw new Error('Remote requires a live Desktop document');
    const previous = owners.get(sender);
    if (previous?.nonce === nonce) return previous;
    previous?.dispose();
    const state: DocumentOwner = {
      nonce, documents: new Set(), pending: new Set(), pendingOpens: 0, closed: false,
      dispose() {
        if (state.closed) return;
        state.closed = true;
        if (owners.get(sender) === state) owners.delete(sender);
        sender.removeListener('did-start-navigation', navigation);
        sender.removeListener('render-process-gone', dispose);
        sender.removeListener('destroyed', dispose);
        for (const document of state.documents) track(draining, closeDocument(document));
        state.documents.clear();
        // Accepted opens keep ownership until their late handles are closed.
        for (const pending of state.pending) track(draining, pending);
      },
    };
    const dispose = () => state.dispose();
    const navigation = (_event: unknown, _url: string, inPlace: boolean, mainFrame: boolean) => {
      if (mainFrame && !inPlace) dispose();
    };
    sender.on('did-start-navigation', navigation);
    sender.on('render-process-gone', dispose);
    sender.on('destroyed', dispose);
    owners.set(sender, state);
    return state;
  };

  input.ipcMain.handle('plugins:remote', async (event, nonce: unknown, raw: unknown): Promise<PluginRemoteResult> => {
    const value = HOST_OPERATION_SPECS['plugin.remote'].decodeInput(raw);
    const owner = ownerFor(event, nonce);
    if (value.kind === 'open_document') {
      if (owner.documents.size + owner.pendingOpens >= 32) throw new Error('Remote document limit exceeded');
      owner.pendingOpens++;
      return track(owner.pending, (async () => {
        try {
          const result = await request(value);
          if (result.kind !== 'document') throw new Error('Unexpected Remote document result');
          if (owner.closed) {
            await closeDocument(result.document);
            throw new Error('Remote document retired during open');
          }
          owner.documents.add(result.document);
          return result;
        } finally { owner.pendingOpens--; }
      })());
    }
    if ('document' in value && !owner.documents.has(value.document))
      throw new Error('Remote document does not belong to this Renderer');
    // Commands are never automatically replayed after connection loss.
    if (value.kind === 'close_document') owner.documents.delete(value.document);
    return track(owner.pending, request(value));
  });

  input.ipcMain.handle('plugins:files', async (event, nonce: unknown, rawClient: unknown, raw: unknown) => {
    const owner = ownerFor(event, nonce);
    const files = input.files;
    if (!files) throw new Error('Desktop-local files are unavailable for this Host');
    if (!rawClient || typeof rawClient !== 'object' || !raw || typeof raw !== 'object')
      throw new Error('Invalid Client file request');
    const client = rawClient as Record<string, unknown>;
    const action = raw as Record<string, unknown>;
    const binding = HOST_OPERATION_SPECS['plugin.client.query'].decodeInput({
      kind:'bundle', entryId:client.entryId, activation:client.activation, clientDigest:client.clientDigest, offset:0,
    });
    if (action.kind !== 'pick' && action.kind !== 'open') throw new Error('Unknown Client file action');
    if (action.kind === 'open' && (typeof action.path !== 'string' || !isAbsolute(action.path)
      || action.path.length > 32768 || action.path.includes('\0'))) throw new Error('Invalid local path');
    await files.validate(binding);
    if (owner.closed) throw new Error('Client document has retired');
    if (action.kind === 'open') { await files.open(action.path as string); return null; }
    const path = await files.pick();
    if (owner.closed) throw new Error('Client document retired during file selection');
    return path;
  });

  return async () => {
    closed = true;
    for (const owner of owners.values()) owner.dispose();
    await Promise.allSettled([...draining]);
  };
}
