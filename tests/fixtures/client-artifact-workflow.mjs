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
import { createHash } from 'node:crypto';

const digest = (bytes) => 'sha256:' + createHash('sha256').update(bytes).digest('hex');
const id = (session, upload) =>
  'attachment-' +
  createHash('sha256')
    .update(session + '\0' + upload)
    .digest('hex')
    .slice(0, 32);
const content = Buffer.from('Maka 😀\n'.repeat(9000));
const metadata = (uploadId, bytes, overrides = {}) => ({
  kind: 'begin',
  sessionId: 'session',
  uploadId,
  name: ' ../file?.txt ',
  mimeType: 'text/plain',
  totalBytes: bytes.length,
  contentSha256: digest(bytes),
  ...overrides,
});
const request = (connection, operation, input) => connection.request(operation, input, 3000);
const ingest = (connection, input) => request(connection, 'artifact.ingest', input);
const query = (connection, kind, artifactId) =>
  request(connection, 'artifact.query', {
    kind,
    sessionId: 'session',
    ...(artifactId === undefined ? {} : { artifactId }),
  });
const reject = (promise, code) => assert.rejects(promise, (error) => error.code === code);
const command = (kind, uploadId) => ({ kind, sessionId: 'session', uploadId });

async function upload(connection, uploadId, bytes, overrides) {
  await ingest(connection, metadata(uploadId, bytes, overrides));
  for (let offset = 0; offset < bytes.length; offset += 48 * 1024) {
    await ingest(connection, {
      ...command('chunk', uploadId),
      offset,
      chunkBase64: bytes.subarray(offset, offset + 48 * 1024).toString('base64'),
    });
  }
  return ingest(connection, command('commit', uploadId));
}
async function readAll(connection, artifactId) {
  const chunks = [];
  let offset = 0;
  do {
    const chunk = await request(connection, 'artifact.query', {
      kind: 'read_chunk',
      sessionId: 'session',
      artifactId,
      offset,
    });
    assert.equal(chunk.offset, offset);
    const bytes = Buffer.from(chunk.chunkBase64, 'base64');
    assert(bytes.length <= 32 * 1024);
    chunks.push(bytes);
    offset = chunk.nextOffset;
  } while (offset !== null);
  return Buffer.concat(chunks);
}
async function pages(connection) {
  const items = [];
  let page = await query(connection, 'list_start');
  const revision = page.revision;
  do {
    assert.equal(page.kind, 'page');
    assert.equal(page.revision, revision);
    assert(Buffer.byteLength(JSON.stringify(page)) <= 48 * 1024);
    assert(page.artifacts.length <= 128);
    items.push(...page.artifacts);
    if (page.nextCursor === null) break;
    page = await request(connection, 'artifact.query', {
      kind: 'list_continue',
      sessionId: 'session',
      revision,
      cursor: page.nextCursor,
    });
  } while (true);
  assert.equal(new Set(items.map((item) => item.id)).size, items.length);
  return { items, revision };
}

export async function verifyArtifacts(connection, open, reopened) {
  const original = metadata('durable-upload', content);
  const artifactId = id('session', original.uploadId);
  if (reopened) {
    const receipt = await ingest(connection, original);
    assert.equal(receipt.kind, 'committed');
    assert.equal(receipt.attachment.ref.relativePath, artifactId);
    assert.deepEqual(await ingest(connection, command('commit', original.uploadId)), receipt);
    assert.deepEqual(await readAll(connection, artifactId), content);
    const page = await pages(connection);
    assert.equal(page.items.length, 132);
    await reject(
      request(connection, 'artifact.delete', {
        sessionId: 'session',
        artifactId: 'protected-evidence',
      }),
      'operation_conflict',
    );
    await reject(ingest(connection, command('commit', 'uncommitted')), 'not_found');
    return;
  }
  const sibling = await open();
  const opened = await ingest(connection, original);
  assert.deepEqual(opened, { kind: 'upload_opened', uploadId: original.uploadId, nextOffset: 0 });
  const empty = await query(connection, 'list_start');
  assert.deepEqual(empty.artifacts, []);
  assert.equal((await query(connection, 'get', artifactId)).artifact, null);
  await reject(ingest(sibling, original), 'operation_conflict');
  await reject(ingest(sibling, command('commit', original.uploadId)), 'not_found');
  const first = {
    ...command('chunk', original.uploadId),
    offset: 0,
    chunkBase64: content.subarray(0, 48 * 1024).toString('base64'),
  };
  assert.equal((await ingest(connection, first)).nextOffset, 48 * 1024);
  assert.equal((await ingest(connection, first)).nextOffset, 48 * 1024);
  assert.equal((await ingest(connection, original)).nextOffset, 48 * 1024);
  await ingest(sibling, command('abort', original.uploadId));
  await reject(ingest(connection, { ...first, offset: 1 }), 'operation_conflict');
  await reject(ingest(connection, command('commit', original.uploadId)), 'operation_conflict');
  for (let offset = 48 * 1024; offset < content.length; offset += 48 * 1024) {
    await ingest(connection, {
      ...first,
      offset,
      chunkBase64: content.subarray(offset, offset + 48 * 1024).toString('base64'),
    });
  }
  const receipt = await ingest(connection, command('commit', original.uploadId));
  assert.deepEqual(receipt.attachment, {
    kind: 'other',
    name: 'file-.txt',
    mimeType: 'text/plain',
    bytes: content.length,
    ref: { kind: 'session_file', sessionId: 'session', relativePath: artifactId },
  });
  assert.deepEqual(await ingest(sibling, original), receipt);
  assert.deepEqual(await ingest(sibling, command('commit', original.uploadId)), receipt);
  await reject(
    ingest(sibling, { ...original, contentSha256: digest(Buffer.from('other')) }),
    'operation_conflict',
  );
  assert.deepEqual(await readAll(connection, artifactId), content);
  const eof = await request(connection, 'artifact.query', {
    kind: 'read_chunk',
    sessionId: 'session',
    artifactId,
    offset: content.length,
  });
  assert.equal(eof.chunkBase64, '');
  assert.equal(eof.nextOffset, null);
  await reject(
    request(connection, 'artifact.query', {
      kind: 'read_chunk',
      sessionId: 'session',
      artifactId,
      offset: content.length + 1,
    }),
    'invalid_request',
  );
  assert.deepEqual((await query(connection, 'read_text', artifactId)).preview, {
    ok: false,
    reason: 'too_large',
  });
  assert.equal(
    (
      await request(connection, 'artifact.query', {
        kind: 'get',
        sessionId: 'other',
        artifactId,
      })
    ).artifact,
    null,
  );
  await reject(
    request(connection, 'artifact.delete', { sessionId: 'other', artifactId }),
    'not_found',
  );

  const wrong = metadata('digest-mismatch', Buffer.from('bad'), {
    contentSha256: digest(Buffer.from('yes')),
  });
  await ingest(connection, wrong);
  await ingest(connection, {
    ...command('chunk', wrong.uploadId),
    offset: 0,
    chunkBase64: Buffer.from('bad').toString('base64'),
  });
  await reject(ingest(connection, command('commit', wrong.uploadId)), 'operation_conflict');
  await reject(ingest(connection, command('commit', wrong.uploadId)), 'not_found');
  assert.equal((await query(connection, 'get', id('session', wrong.uploadId))).artifact, null);
  const nul = await upload(connection, 'escaped-preview', Buffer.alloc(12000));
  assert.deepEqual(
    (await query(connection, 'read_text', nul.attachment.ref.relativePath)).preview,
    { ok: false, reason: 'too_large' },
  );
  assert.deepEqual(
    (await query(connection, 'read_binary', nul.attachment.ref.relativePath)).preview,
    { ok: false, reason: 'unsupported_mime' },
  );
  const png = await upload(connection, 'sniffed-preview', Buffer.from('89504e470d0a1a0a', 'hex'), {
    mimeType: 'text/plain',
  });
  assert.equal(
    (await query(connection, 'read_binary', png.attachment.ref.relativePath)).preview.mimeType,
    'image/png',
  );
  const stale = await query(connection, 'list_start');
  for (let n = 0; n < 129; n++) {
    await upload(connection, 'page-' + n, Buffer.alloc(0), { name: '😀'.repeat(60) });
  }
  assert.equal(
    (
      await request(connection, 'artifact.query', {
        kind: 'list_continue',
        sessionId: 'session',
        revision: stale.revision,
        cursor: 'invalid',
      })
    ).kind,
    'revision_changed',
  );
  const page = await pages(connection);
  assert.equal(page.items.length, 132);
  await reject(
    request(connection, 'artifact.query', {
      kind: 'list_continue',
      sessionId: 'session',
      revision: page.revision,
      cursor: '01',
    }),
    'invalid_request',
  );
  await request(connection, 'artifact.delete', {
    sessionId: 'session',
    artifactId: nul.attachment.ref.relativePath,
  });
  assert.equal((await query(connection, 'get', nul.attachment.ref.relativePath)).artifact, null);
  assert.notEqual((await query(connection, 'list_start')).revision, page.revision);

  const disconnect = await open();
  await ingest(disconnect, metadata('disconnect', Buffer.from('x')));
  await disconnect.close();
  // Await the observable connection lifetime, not a timing assumption.
  while ((await connection.status(3000)).connections !== 2)
    await new Promise((resolve) => setTimeout(resolve, 5));
  assert.equal(
    (await ingest(sibling, metadata('disconnect', Buffer.from('x')))).kind,
    'upload_opened',
  );
  await ingest(sibling, command('abort', 'disconnect'));
  for (let n = 0; n < 16; n++) await ingest(connection, metadata('slot-' + n, Buffer.alloc(0)));
  await reject(ingest(connection, metadata('overflow', Buffer.alloc(0))), 'operation_conflict');
  for (let n = 0; n < 16; n++) await ingest(connection, command('abort', 'slot-' + n));
  await ingest(connection, metadata('uncommitted', Buffer.from('not published')));
}
