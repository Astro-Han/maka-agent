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

import type { PermissionMode } from '@maka/core/permission';
import { createPluginRemote, type RuntimeHostConnection } from '@maka/runtime-host/client';
import type { WorkspaceTarget } from '@maka/runtime-host/protocol';
import type { InvocableSkillEntry } from '@maka/runtime/skill-invocation';

/** CLI presentation is a consumer of the Skills plugin, not a Host operation. */
export async function readRuntimeHostSkills(
  connection: RuntimeHostConnection,
  workspace: WorkspaceTarget,
  permissionMode: PermissionMode,
): Promise<InvocableSkillEntry[]> {
  const snapshot = await connection.request('plugin.client.query', { kind: 'snapshot' });
  if (snapshot.kind !== 'snapshot') throw new Error('Unexpected plugin catalog result');
  let entries = snapshot;
  let plugin = entries.entries.find((entry) => entry.extensionId === 'maka.skills');
  while (!plugin && entries.nextCursor) {
    const next = await connection.request('plugin.client.query', {
      kind: 'snapshot',
      cursor: entries.nextCursor,
    });
    if (next.kind !== 'snapshot' || next.revision !== snapshot.revision)
      throw new Error('Plugin catalog changed during Skills lookup');
    entries = next;
    plugin = entries.entries.find((entry) => entry.extensionId === 'maka.skills');
  }
  if (!plugin) return [];
  const { entryId, extensionId, activation, contentDigest, clientDigest } = plugin;
  const remote = createPluginRemote(
    (input) => connection.request('plugin.remote', input),
    { entryId, extensionId, activation, contentDigest, clientDigest },
    AbortSignal.timeout(10_000),
  );
  type Page = { revision: string; cursor: string };
  type Request = { kind: 'invocable'; page: Page | null };
  type Result =
    | {
        kind: 'page';
        revision: string;
        items: { ref: string; id: string; name: string; description: string }[];
        nextCursor: string | null;
      }
    | { kind: 'revision_changed'; expectedRevision: string; actualRevision: string };
  type Input = ({ projectId: string } | { path: string }) & {
    permissionMode: PermissionMode;
    collaborationMode: 'agent';
    request: Request;
  };
  const target =
    workspace.kind === 'project' ? { projectId: workspace.projectId } : { path: workspace.path };
  const call = remote.api.method<Input, Result>(
    workspace.kind === 'project' ? 'project-request' : 'path-request',
  );
  try {
    const items: InvocableSkillEntry[] = [];
    const cursors = new Set<string>();
    let page: Page | null = null;
    do {
      const result = await call({
        ...target,
        permissionMode,
        collaborationMode: 'agent',
        request: { kind: 'invocable', page },
      });
      if (result.kind !== 'page') throw new Error('Skills changed during pagination');
      items.push(...result.items);
      if (items.length > 4096) throw new Error('Skill suggestions exceed the catalog limit');
      page = result.nextCursor ? { revision: result.revision, cursor: result.nextCursor } : null;
      if (page && cursors.has(page.cursor)) throw new Error('Skill catalog repeated a cursor');
      if (page) cursors.add(page.cursor);
    } while (page);
    return items;
  } finally {
    await remote.close();
  }
}
