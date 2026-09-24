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
import { spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { access, mkdtemp, readFile, readdir, rm } from 'node:fs/promises';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

test('native Maka shim enrolls through the proxy, settles, and refuses to reinitialize its root', {
  timeout: 60_000,
}, async () => {
  const root = await mkdtemp(join(tmpdir(), 'maka-eval-initialization-'));
  const state = join(root, 'state');
  const artifacts = join(root, 'artifacts');
  const apiKey = 'eval-fixture-api-secret';
  const proxyPassword = 'eval-fixture-proxy-secret';
  const requests: string[] = [];
  const failures: unknown[] = [];
  const proxy = createServer(async (request, response) => {
    try {
      const url = new URL(request.url!);
      assert.equal(url.origin, 'http://provider.invalid');
      assert.equal(
        request.headers['proxy-authorization'],
        'Basic ' + Buffer.from('eval-user:' + proxyPassword).toString('base64'),
      );
      assert.equal(request.headers.authorization, 'Bearer ' + apiKey);
      let raw = '';
      for await (const chunk of request) raw += chunk;
      const body = raw ? JSON.parse(raw) : {};
      requests.push(url.pathname);
      if (url.pathname === '/v1/models') {
        response.setHeader('content-type', 'application/json');
        response.end(
          JSON.stringify({
            object: 'list',
            data: [{ id: 'fixture-model', object: 'model' }],
          }),
        );
        return;
      }
      assert.equal(url.pathname, '/v1/chat/completions');
      assert.equal(body.model, 'fixture-model');
      const usage = { prompt_tokens: 20, completion_tokens: 5, total_tokens: 25 };
      if (body.stream !== true) {
        response.setHeader('content-type', 'application/json');
        response.end(
          JSON.stringify({
            id: 'fixture-title',
            object: 'chat.completion',
            created: 1,
            model: body.model,
            choices: [
              {
                index: 0,
                message: { role: 'assistant', content: 'Verified.' },
                finish_reason: 'stop',
              },
            ],
            usage,
          }),
        );
        return;
      }
      response.writeHead(200, { 'content-type': 'text/event-stream' });
      response.end(
        'data: ' +
          JSON.stringify({
            id: 'fixture-completion',
            object: 'chat.completion.chunk',
            created: 1,
            model: body.model,
            choices: [
              {
                index: 0,
                delta: { role: 'assistant', content: 'Verified.' },
                finish_reason: 'stop',
              },
            ],
            usage,
          }) +
          '\n\ndata: [DONE]\n\n',
      );
    } catch (error) {
      failures.push(error);
      response.writeHead(400).end();
    }
  });
  proxy.on('connect', (request, socket) => {
    failures.push(new Error('Unexpected CONNECT ' + request.url));
    socket.end('HTTP/1.1 403 Forbidden\r\n\r\n');
  });
  await new Promise<void>((resolve) => proxy.listen(0, '127.0.0.1', resolve));
  const address = proxy.address();
  assert.ok(address && typeof address === 'object');
  const executable =
    process.env.MAKA_TEST_CLI ??
    fileURLToPath(
      new URL(
        '../../../../target/debug/' + (process.platform === 'win32' ? 'maka.exe' : 'maka'),
        import.meta.url,
      ),
    );
  const payload = {
    executable,
    rootPath: state,
    artifactRoot: artifacts,
    hostSettlementTimeoutMs: 5_000,
    connection: {
      provider: {
        packageId: 'maka.providers',
        entryId: 'maka.providers',
        scope: 'profile',
        name: 'openai-compatible',
      },
      configuration: { baseUrl: 'http://provider.invalid/v1' },
      authentication: { method: 'api-key', inputEnvironment: 'MAKA_FIXTURE_AUTH' },
    },
    execution: {
      executionId: randomUUID(),
      session: {
        workspace: { kind: 'host_path', path: root },
        modelTarget: { kind: 'explicit', connectionSlug: 'fixture', model: 'fixture-model' },
        sandboxMode: 'danger-full-access',
      },
      content: { text: 'Reply briefly.' },
      maxSteps: 2,
    },
  };
  const run = async () => {
    const child = spawn(
      process.execPath,
      [
        fileURLToPath(new URL('../harbor-maka-subject.js', import.meta.url)),
        Buffer.from(JSON.stringify(payload)).toString('base64url'),
      ],
      {
        env: {
          ...process.env,
          MAKA_FIXTURE_AUTH: JSON.stringify({ apiKey }),
          HTTPS_PROXY: 'http://eval-user:' + proxyPassword + '@127.0.0.1:' + address.port,
          MAKA_EVAL_RESULT_TOKEN: '1'.repeat(32),
        },
        stdio: ['ignore', 'pipe', 'pipe'],
      },
    );
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', (chunk) => {
      stdout += chunk;
    });
    child.stderr.on('data', (chunk) => {
      stderr += chunk;
    });
    const timer = setTimeout(() => child.kill('SIGTERM'), 25_000);
    try {
      const exit = await new Promise<number | null>((resolve, reject) => {
        child.once('exit', resolve);
        child.once('error', reject);
      });
      return { exit, stdout, stderr };
    } finally {
      clearTimeout(timer);
    }
  };
  try {
    await access(executable);
    const completed = await run();
    assert.deepEqual(failures, []);
    assert.equal(completed.exit, 0, completed.stdout + '\n' + completed.stderr);
    assert.ok(requests.includes('/v1/models'));
    assert.ok(requests.includes('/v1/chat/completions'));
    const frame = completed.stdout
      .split('\n')
      .find((line) => line.startsWith('MAKA-EVAL-RESULT-V1 '));
    assert.ok(frame);
    const result = JSON.parse(Buffer.from(frame.split(' ')[4]!, 'base64url').toString());
    assert.equal(result.status, 'completed');
    assert.ok(result.usage.inputTokens > 0);
    const files = await readdir(artifacts);
    assert.ok(files.includes('runtime-rust.sqlite'));
    assert.equal(files.includes('configuration-rust.sqlite'), false);
    for (const file of files) {
      const contents = await readFile(join(artifacts, file));
      assert.equal(contents.includes(Buffer.from(apiKey)), false, file);
      assert.equal(contents.includes(Buffer.from(proxyPassword)), false, file);
    }
    const count = requests.length;
    payload.execution.executionId = randomUUID();
    const duplicate = await run();
    assert.equal(duplicate.exit, 1);
    assert.equal(requests.length, count, 'an existing root must not be reconfigured or reused');
  } finally {
    proxy.closeAllConnections();
    await new Promise<void>((resolve) => proxy.close(() => resolve()));
    await rm(root, { recursive: true, force: true });
  }
});
