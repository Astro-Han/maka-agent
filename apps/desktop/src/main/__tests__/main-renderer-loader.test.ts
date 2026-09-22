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

import assert from 'node:assert/strict';
import test from 'node:test';
import { loadMainRenderer, resolveMainRendererEntry } from '../main-renderer-loader.js';

test('stalled document loads stop at their budget or when the window closes', async () => {
  let stops = 0;
  let finish!: () => void;
  const pending = new Promise<void>((resolve) => { finish = resolve; });
  const window = {
    loadURL: () => pending,
    loadFile: () => pending,
    webContents: { stop: () => { stops++; }, isDestroyed: () => false },
  };
  const entry = resolveMainRendererEntry('/app/main', 'http://127.0.0.1:1234');
  await assert.rejects(loadMainRenderer(window, entry, undefined, { timeoutMs: 1 }), /loading timed out/);
  assert.equal(stops, 1);

  const cancellation = new AbortController();
  const aborted = loadMainRenderer(window, entry, undefined, { signal: cancellation.signal });
  cancellation.abort(new Error('window closed'));
  await assert.rejects(aborted, /window closed/);
  assert.equal(stops, 2);
  finish();
  await loadMainRenderer(window, entry);
  assert.equal(stops, 2);
});
