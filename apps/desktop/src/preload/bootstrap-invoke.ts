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

import { ipcRenderer } from 'electron';

const EARLY_CHANNELS = new Set([
  'app:bootstrapReady',
  'window:notifyRendererReady',
  'window:quit',
  'diagnostics:takePreviousMainProcessInterruption',
  'diagnostics:copyPreviousMainProcessInterruption',
]);

// Registration is independent of Host connectivity. A failed boot rejects
// queued calls; it must not dispatch mutations to a partly registered Host.
const bootReady = Promise.resolve(ipcRenderer.invoke('app:bootstrapReady'));
void bootReady.catch(() => undefined);

async function waitForBootstrap(): Promise<void> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    await Promise.race([
      bootReady,
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error('Desktop startup timed out; retry when startup completes')), 15_000);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

export const invokeWhenReady: typeof ipcRenderer.invoke = async (channel, ...args) => {
  if (!EARLY_CHANNELS.has(channel)) await waitForBootstrap();
  return ipcRenderer.invoke(channel, ...args);
};

export const sendWhenReady: typeof ipcRenderer.send = (channel, ...args) => {
  if (EARLY_CHANNELS.has(channel)) {
    ipcRenderer.send(channel, ...args);
    return;
  }
  void waitForBootstrap().then(() => ipcRenderer.send(channel, ...args)).catch((error: unknown) => {
    console.error('[startup] IPC send failed:', channel, error);
  });
};
