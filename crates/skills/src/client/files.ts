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

import type { ClientContext, AuthorizationRequest } from '@maka-agent/plugin-sdk/client';
import type { Call, Rejected } from './model.js';

export async function authorizeUser(context: ClientContext): Promise<string> {
  const status = await context.remote.method<
    { kind: 'status' },
    { target: AuthorizationRequest['target']; grant: string | null; recovery: string | null }
  >('user-authorization')({ kind: 'status' });
  if (status.grant && status.recovery === null) {
    const grant = await context.authorization.query('profile', status.grant);
    if (grant && !grant.revoked) return grant.id;
  }
  const grant = await context.authorization.approve('profile', {
    operationId: crypto.randomUUID(),
    title: 'Manage user Skills / 管理用户技能目录',
    target: status.target,
    capabilities: ['read_files', 'write_files'],
  });
  if (!grant) throw new Error('User Skills authorization was cancelled');
  return grant.id;
}

export async function importSource(context: ClientContext): Promise<boolean> {
  if (!context.localFiles) throw new Error('Desktop-local files are unavailable');
  const sourcePath = await context.localFiles.pick();
  if (sourcePath === null) return false;
  const grant = await authorizeUser(context);
  const result = await context.remote.method<
    { sourcePath: string; grant: string },
    { kind: 'imported' } | Rejected
  >('import-source')({ sourcePath, grant });
  if (result.kind === 'rejected') throw new Error(result.reason);
  return true;
}
export async function recoverUser(context: ClientContext): Promise<void> {
  const grant = await authorizeUser(context);
  await context.remote.method<{ kind: 'recover'; grant: string }, { recovery: string | null }>(
    'user-authorization',
  )({ kind: 'recover', grant });
}
export async function openSource(
  context: ClientContext,
  call: Call,
  ref: string,
  target: 'file' | 'directory',
) {
  if (!context.localFiles) throw new Error('Desktop-local files are unavailable');
  const result = await call({ kind: 'resolve_path', ref, target });
  if (result.kind !== 'resolved')
    throw new Error(result.kind === 'rejected' ? result.reason : 'Source changed');
  await context.localFiles.open(result.path);
}
