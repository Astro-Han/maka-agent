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
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { shellRunUpdate } from '../../packages/runtime/src/shell-run-tool-result.ts';
import { canonicalRuntimeResources } from '../../packages/runtime-host/src/server/runtime-resource-projection.ts';

function originalRecord(record) {
  const raw = record.output;
  const output =
    raw.mode === 'pipes'
      ? {
          mode: 'pipes',
          stdout: raw.stdout,
          stderr: raw.stderr,
          ...(raw.latest_stream === null ? {} : { latestStream: raw.latest_stream }),
          stdoutTruncated: raw.stdout_truncated,
          stderrTruncated: raw.stderr_truncated,
          redacted: false,
        }
      : {
          mode: 'pty',
          screen: raw.screen.screen,
          scrollback: raw.screen.scrollback,
          ...(raw.screen.lastAlternateScreen === undefined
            ? {}
            : { lastAlternateScreen: raw.screen.lastAlternateScreen }),
          ...raw.screen.size,
          cursor: raw.screen.cursor,
          alternateScreen: raw.screen.alternateScreen,
          truncated: raw.screen.truncated,
          redacted: false,
        };
  assert.equal(record.state.kind, 'terminal');
  assert.equal(record.state.outcome.kind, 'completed');
  return {
    shellRunId: record.id,
    sessionId: record.sessionId,
    sourceTurnId: record.sourceTurnId,
    sourceToolCallId: record.sourceToolCallId,
    cwd: record.cwd,
    command: record.command,
    startedAt: record.startedAt,
    updatedAt: record.updatedAt,
    completedAt: record.state.completed_at,
    status: 'completed',
    exitCode: 0,
    revision: record.revision,
    output,
  };
}

export async function verifyResourceQueries(connection, workspace) {
  const query = (input) =>
    connection.request('runtime.resource.query', { sessionId: 'shell-bypass', ...input }, 10000);
  const records = JSON.parse(await readFile(join(workspace, 'resource-records.json'), 'utf8'));
  const expected = canonicalRuntimeResources(
    records.map((record) => shellRunUpdate(originalRecord(record))),
  );
  const resources = [];
  let page = await query({ kind: 'list_start' });
  const revision = page.revision;
  let pages = 0;
  for (;;) {
    assert.equal(page.kind, 'page');
    assert.equal(page.revision, revision);
    assert.ok(Buffer.byteLength(JSON.stringify(page)) <= 52 * 1024);
    assert.ok(page.resources.length <= 64);
    resources.push(...page.resources);
    pages += 1;
    if (page.nextCursor === null) break;
    page = await query({ kind: 'list_continue', revision, cursor: page.nextCursor });
  }
  assert.ok(pages > 1);
  assert.deepEqual(
    resources.filter((entry) => entry.result.ref.includes('/resource-')),
    expected,
  );
  const ref = 'maka://runtime/background-tasks/resource-002';
  const single = await query({ kind: 'get', ref });
  assert.equal(single.kind, 'resource');
  assert.deepEqual(
    single.resource,
    expected.find((entry) => entry.result.ref === ref),
  );
  assert.notEqual(single.revision, revision, 'get hashes only its own projected scope');
  assert.deepEqual(await query({ kind: 'get', ref }), single);
  const missing = await query({ kind: 'get', ref: 'maka://runtime/background-tasks/missing' });
  assert.equal(missing.resource, null);
  const empty = await connection.request('runtime.resource.query', {
    kind: 'list_start',
    sessionId: 'shell-readonly',
  });
  assert.deepEqual(empty.resources, []);
  assert.equal(empty.revision, missing.revision);
  const changed = await query({
    kind: 'list_continue',
    revision: 'sha256:' + '0'.repeat(64),
    cursor: 'not-numeric',
  });
  assert.deepEqual(changed, {
    kind: 'revision_changed',
    expected: 'sha256:' + '0'.repeat(64),
    actual: revision,
  });
  await assert.rejects(query({ kind: 'list_continue', revision, cursor: '0' }), {
    code: 'invalid_request',
  });
  await assert.rejects(query({ kind: 'get', ref: 'maka://unsupported' }), {
    code: 'invalid_request',
  });
  await assert.rejects(
    connection.request('runtime.resource.query', {
      kind: 'list_start',
      sessionId: 'missing-session',
    }),
    { code: 'not_found' },
  );
}
