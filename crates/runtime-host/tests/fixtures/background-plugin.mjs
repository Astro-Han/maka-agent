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

/** @param {import('../../../../packages/plugin-sdk/src/host.js').HostContext} ctx */
export default async function (ctx) {
  await ctx.remote.method('clients', async (input, caller) => {
    if (!input || typeof input !== 'object' || Array.isArray(input))
      throw new Error('invalid Client request');
    /** @param {import('../../../../packages/plugin-sdk/src/host.js').ResourceContext} call */
    const exercise = async (call) => {
      const tools = await call.clients.tools();
      if (tools.length !== 1 || tools[0].name !== 'mcp__desktop__inspect')
        throw new Error('Client catalog escaped its authorizing Client');
      return call.clients.call({ name: tools[0].name, input: {} });
    };
    if ('grant' in input && typeof input.grant === 'string')
      return ctx.withAuthorization(input.grant, exercise);
    if (!('operationId' in input) || typeof input.operationId !== 'string')
      throw new Error('invalid Client operation');
    return caller.views.authorize(
      {
        operationId: input.operationId,
        title: 'Call my Client',
        target: { kind: 'session', sessionId: 'background-session' },
        capabilities: ['client_capabilities'],
      },
      exercise,
    );
  });
  await ctx.remote.method('root', async (input) => {
    const intent = parseIntent(input);
    const commands = await ctx.executions.restore(intent.grant);
    try {
      /** @type {Parameters<typeof commands.createRoot>[0]} */
      const request = {
        operationId: intent.operation,
        name: 'Authorized root',
        settings: {
          target: { kind: 'executor', executorId: 'example.background' },
          permissionMode: 'explore',
          toolMode: 'direct',
          collaborationMode: 'agent',
          behavior: 'default',
        },
      };
      try {
        await commands.createRoot({
          ...request,
          settings: { ...request.settings, permissionMode: 'bypass' },
        });
        throw new Error('workspace grant widened');
      } catch (error) {
        if (error.code !== 'revoked') throw error;
      }
      const root = await commands.createRoot(request);
      const replay = await commands.createRoot(request);
      if (replay.sessionId !== root.sessionId)
        throw new Error('root creation replay changed identity');
      try {
        await commands.createRoot({ ...request, name: 'Changed retry' });
        throw new Error('root creation accepted a changed proposal');
      } catch (error) {
        if (error.code !== 'conflict') throw error;
      }
      const receipt = await commands.submit({
        operationId: 'workspace-work',
        sessionId: root.sessionId,
        content: { text: 'Independent root work' },
      });
      while ((await commands.query('workspace-work')).progress.state !== 'ended')
        await ctx.sleep(10);
      const view = await commands.session(root.sessionId);
      if (view.permissionMode !== 'explore' || view.target.kind !== 'executor')
        throw new Error('root settings changed');
      return JSON.parse(JSON.stringify({ root, receipt }));
    } finally {
      await commands.close();
    }
  });
  await ctx.remote.method('resources', async (input, caller) => {
    if (
      !input ||
      typeof input !== 'object' ||
      !('operationId' in input) ||
      typeof input.operationId !== 'string'
    )
      throw new Error('invalid operation');
    const borrowed = await caller.views.authorize(
      {
        operationId: input.operationId,
        title: 'Use current Remote authority',
        target: { kind: 'session', sessionId: 'background-session' },
        capabilities: ['processes', 'network', 'read_files', 'write_files', 'models', 'executions'],
      },
      async (call) => {
        await exercise(call);
        const commands = await call.executions.open();
        try {
          const child = await commands.createChild({
            operationId: 'remote-child',
            parentSessionId: 'background-session',
            name: 'Authorized child',
            target: { kind: 'executor', executorId: 'example.background' },
          });
          const view = await commands.session(child.sessionId);
          if (
            view.target.kind !== 'executor' ||
            view.target.executorId !== 'example.background' ||
            view.permissionMode !== 'bypass' ||
            view.sessionId !== child.sessionId
          )
            throw new Error('authorized Session projection changed');
          try {
            await commands.session('not-authorized');
            throw new Error('Session query escaped its authority');
          } catch (error) {
            if (error.code !== 'revoked') throw error;
          }
          await commands.submit({
            operationId: 'remote-execution',
            sessionId: child.sessionId,
            content: { text: 'Accepted independent work' },
          });
          while ((await commands.query('remote-execution')).progress.state !== 'ended')
            await ctx.sleep(10);
          return commands;
        } catch (error) {
          await commands.close();
          throw error;
        }
      },
    );
    try {
      await borrowed.submit({
        operationId: 'stale-remote',
        sessionId: 'background-session',
        content: { text: 'Must not start' },
      });
      throw new Error('closed Remote authority was reused');
    } catch (error) {
      if (error.code !== 'revoked') throw error;
    } finally {
      await borrowed.close();
    }
    return true;
  });
  await ctx.executors.register(
    { name: 'example.background', displayName: 'Background acceptance', capabilities: {} },
    async () => ({ status: 'completed', text: 'Accepted background work' }),
  );
  let record = await ctx.storage.read('intent');
  let pending = record?.data.kind === 'present';
  let wake = () => {};
  /** @type {import('../../../../packages/plugin-sdk/src/host.js').Json} */
  let state = null;
  await ctx.remote.method('queue', async (input) => {
    const intent = parseIntent(input);
    [record] = await ctx.storage.batch([
      {
        key: 'intent',
        expectedRevision: record?.revision ?? null,
        data: { kind: 'present', value: intent },
      },
    ]);
    pending = true;
    state = null;
    wake();
    return true;
  });
  await ctx.remote.method('state', () => state);
  ctx.run(async () => {
    /** @type {Awaited<ReturnType<typeof ctx.executions.restore>> | undefined} */
    let commands;
    let grant;
    try {
      while (!ctx.signal.aborted) {
        if (!pending) {
          await Promise.race([
            new Promise((resolve) => {
              wake = () => resolve(undefined);
            }),
            ctx.signal.wait(),
          ]);
          continue;
        }
        pending = false;
        const intent = parseIntent(record?.data.kind === 'present' ? record.data.value : null);
        try {
          if (!commands || grant !== intent.grant) {
            await commands?.close();
            commands = await ctx.executions.restore(intent.grant);
            grant = intent.grant;
          }
          await ctx.withAuthorization(intent.grant, async (call) => {
            if (call.source.kind !== 'background') throw new Error('background source lost');
            await exercise(call);
          });
          const receipt = await commands.submit({
            operationId: intent.operation,
            sessionId: 'background-session',
            content: { text: 'Run authorized work' },
          });
          let result = await commands.query(intent.operation);
          while (result.progress.state !== 'ended' && !ctx.signal.aborted) {
            await ctx.sleep(10);
            result = await commands.query(intent.operation);
          }
          state = JSON.parse(JSON.stringify({ receipt, progress: result.progress }));
        } catch (error) {
          state = { error: error.code ?? 'failed' };
        }
      }
    } finally {
      await commands?.close();
    }
  });
}

/** @param {import('../../../../packages/plugin-sdk/src/host.js').ResourceContext} call */
async function exercise(call) {
  await call.files.write({ path: 'authorized.txt', content: 'first\nsecond\nthird\n' });
  const page = await call.files.read({ path: 'authorized.txt', offset: 1, limit: 1 });
  if (!('content' in page) || page.content.trim() !== 'second')
    throw new Error('resource file pagination changed');
  const image = await call.files.read({ path: 'proof.png' });
  if (!('bytes' in image) || !(image.bytes instanceof Uint8Array) || image.bytes.length < 20)
    throw new Error('resource image lost its byte output');
  try {
    await call.files.write({ path: '../outside', content: 'must not be written' });
    throw new Error('authorization escaped its workspace');
  } catch (error) {
    if (error.code !== 'invalid') throw error;
  }
  const command = {
    executable: '__PROTOCOL_EXECUTABLE__',
    args: [
      '--exact',
      'javascript_plugins::external_shared_and_dedicated_plugins_route_services_persist_data_and_drain_on_disable',
      '--nocapture',
    ],
    env: { MAKA_PLUGIN_PROTOCOL_TEST_CHILD: '1' },
  };
  const process = await call.processes.spawn(command);
  await process.write('authorized\n');
  let output = '';
  const decoder = new TextDecoder();
  while (!output.includes('protocol:authorized')) {
    const chunk = await process.next();
    if (!chunk) throw new Error('background process closed before reply');
    output += decoder.decode(chunk.bytes, { stream: true });
  }
  // Leave the process running: the public call lifetime must drain it.
  const terminal = await call.terminals.spawn({
    ...command,
    env: { ...command.env, MAKA_PLUGIN_PTY_TEST_CHILD: '1' },
  });
  await terminal.write('quit\n', { cols: 100, rows: 30 });
  const outcome = await terminal.wait();
  if (outcome.kind !== 'completed' && !(outcome.kind === 'exited' && outcome.code === 0))
    throw new Error('background PTY did not complete');
  const response = await call.http.request({ url: '__RESOURCE_URL__' });
  let body = '';
  for (let chunk = await response.next(); chunk !== null; chunk = await response.next()) {
    body += decoder.decode(chunk, { stream: true });
  }
  await response.close();
  if (response.status !== 200 || body !== 'authorized')
    throw new Error('background HTTP lost output');
  const generated = await call.llm.generate({ prompt: 'Auxiliary work', maxOutputTokens: 32 });
  if (generated.text !== 'recovered' || generated.modelId !== 'fixture-model')
    throw new Error('independent model binding lost');
  return true;
}

/** @param {unknown} value */
function parseIntent(value) {
  if (
    !value ||
    typeof value !== 'object' ||
    !('grant' in value) ||
    !('operation' in value) ||
    typeof value.grant !== 'string' ||
    typeof value.operation !== 'string'
  )
    throw new Error('invalid intent');
  return { grant: value.grant, operation: value.operation };
}
