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

import { join } from 'node:path';
import { pathToFileURL } from 'node:url';

export interface MainRendererWindow {
  loadFile(path: string): Promise<void>;
  loadURL(url: string): Promise<void>;
  readonly webContents?: { stop(): void; isDestroyed(): boolean };
}

export interface MainRendererEntry {
  readonly filePath: string;
  readonly url: string;
  readonly useDevServer: boolean;
}

export function resolveMainRendererEntry(
  mainModuleDirectory: string,
  viteDevServerUrl: string | undefined,
): MainRendererEntry {
  const rendererEntryPath = join(
    mainModuleDirectory,
    '..',
    '..',
    'dist-renderer',
    'index.html',
  );
  const rendererEntryUrl = viteDevServerUrl ?? pathToFileURL(rendererEntryPath).href;
  return Object.freeze({
    filePath: rendererEntryPath,
    url: rendererEntryUrl,
    useDevServer: !!viteDevServerUrl,
  });
}

export async function loadMainRenderer(
  mainWindow: MainRendererWindow,
  rendererEntry: MainRendererEntry,
  surface?: 'workhub',
  options: { readonly signal?: AbortSignal; readonly timeoutMs?: number } = {},
): Promise<void> {
  options.signal?.throwIfAborted();
  let timer: ReturnType<typeof setTimeout> | undefined;
  let onAbort: (() => void) | undefined;
  const stop = () => {
    try {
      if (!mainWindow.webContents?.isDestroyed()) mainWindow.webContents?.stop();
    } catch { /* The window may have closed while its load was pending. */ }
  };
  const load = async () => {
    if (surface) {
      const url = new URL(rendererEntry.url);
      url.searchParams.set('surface', surface);
      await mainWindow.loadURL(url.href);
      return;
    }
    if (rendererEntry.useDevServer) {
      await mainWindow.loadURL(rendererEntry.url);
    } else {
      await mainWindow.loadFile(rendererEntry.filePath);
    }
  };
  try {
    await Promise.race([
      load(),
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => {
          reject(new Error('Desktop document loading timed out'));
          stop();
        }, options.timeoutMs ?? 15_000);
        onAbort = () => { reject(options.signal!.reason); stop(); };
        options.signal?.addEventListener('abort', onAbort, { once: true });
        if (options.signal?.aborted) onAbort();
      }),
    ]);
  } finally {
    clearTimeout(timer);
    if (onAbort) options.signal?.removeEventListener('abort', onAbort);
  }
}
