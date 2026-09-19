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
import type { IpcHandler } from '../ipc-reconnect-policy.js';
import { registerRuntimeHostAttachmentIngestIpc } from '../runtime-host-artifacts-ipc-main.js';

test('attachment ingestion checks the explicit Session before reading files and preserves structured rejections', async () => {
  const handlers = new Map<string, IpcHandler>();
  let available = true;
  registerRuntimeHostAttachmentIngestIpc({
    client: { async getSession(id) { assert.equal(id, 'target'); return available ? {} as never : null; },
      async ingestAttachment() { throw new Error('No file may be uploaded'); } },
    ipcMain: {
      handle(channel, handler) {
        handlers.set(channel, handler);
      },
    },
    attachmentIngest: {
      approvals: {} as never,
      stat: async () => ({ size: 0 }),
    },
  });

  const prepareAttachments = handlers.get('attachments:prepare');
  assert.ok(prepareAttachments);
  const result = await prepareAttachments(
    { sender: { id: 7 } } as Parameters<IpcHandler>[0],
    'target',
    Array.from({ length: 9 }, () => ({})),
  );
  assert.deepEqual(result, { ok: false, code: 'count_limit' });
  available = false;
  await assert.rejects(prepareAttachments({ sender: { id: 7 } } as Parameters<IpcHandler>[0], 'target', []), /Session is unavailable/);
});
