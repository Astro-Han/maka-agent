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
import { clientPluginRemote } from '../../apps/desktop/src/renderer/platform/desktop/client-plugin-remote.ts';

export async function pluginRemote(connection, extensionId) {
  const page = await connection.request('plugin.client.query', { kind: 'snapshot' });
  const descriptor = page.entries.find((entry) => entry.extensionId === extensionId);
  assert(descriptor, `${extensionId} must publish its Client`);
  const { entryId, activation, contentDigest, clientDigest } = descriptor;
  const lifetime = new AbortController();
  const remote = clientPluginRemote(
    (_host, _epoch, request) => connection.request('plugin.remote', request),
    { profileId: 'test', hostId: 'test' },
    'original-connection',
  )({ entryId, extensionId, activation, contentDigest, clientDigest }, lifetime.signal);
  return {
    descriptor,
    method(name, sessionId) {
      return remote.api.method(name, sessionId);
    },
    close: remote.close,
  };
}
