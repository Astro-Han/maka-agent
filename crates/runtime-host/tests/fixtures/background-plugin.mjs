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
  await ctx.remote.method('network', async (input) => {
    const intent = parseIntent(input);
    return ctx
      .withAuthorization(intent.grant, async (call, _grant, boundary) => {
        if (boundary.kind !== 'workspace' || boundary.sandboxMode !== 'read-only')
          throw new Error('HTTP consent changed filesystem isolation');
        for (const forbidden of [
          () => call.files.entries.write({ path: 'network-must-not-write', bytes: [1] }),
          () => call.processes.spawn({ executable: '__PROTOCOL_EXECUTABLE__', args: [], env: {} }),
        ]) {
          try {
            await forbidden();
            throw new Error('HTTP consent became file or process authority');
          } catch (error) {
            if (error.code !== 'revoked') throw error;
          }
        }
        const response = await call.http.request({ url: '__RESOURCE_URL__' });
        let body = '';
        const decoder = new TextDecoder();
        for (let chunk = await response.next(); chunk !== null; chunk = await response.next())
          body += decoder.decode(chunk, { stream: true });
        await response.close();
        if (response.status !== 200 || body !== 'authorized')
          throw new Error('Authorized HTTP response lost');
        return true;
      })
      .catch((error) => {
        if (error.code === 'revoked') return false;
        throw error;
      });
  });
  await ctx.remote.method('history', async (_input, caller) =>
    caller.views.authorize(
      {
        operationId: '12d11b66-69c2-42f9-a7ab-cc023092215f',
        title: 'Read profile history',
        target: { kind: 'profile' },
        capabilities: ['read_history'],
      },
      async (call) => {
        const catalog = await call.history.list({ includeArchived: true });
        if (!catalog.entries.some((entry) => entry.session.sessionId === 'background-session'))
          throw new Error('History catalog omitted its source');
        /** @type {Parameters<typeof call.history.read>[0]} */
        const input = { sessionId: 'background-session' };
        let text = '';
        for (;;) {
          const page = await call.history.read(input);
          input.through = page.through;
          if (page.kind === 'preparing') continue;
          text += page.chunks.map((chunk) => chunk.text).join('\n');
          if (!page.next) return text;
          input.cursor = page.next;
        }
      },
    ),
  );
  await ctx.remote.method('notify', async (input) => {
    const intent = parseIntent(input);
    return ctx.withAuthorization(intent.grant, async (call, grant, boundary) => {
      if (boundary.kind !== 'profile') throw new Error('Background boundary observation changed');
      if (grant.id !== intent.grant || grant.request.target.kind !== 'profile')
        throw new Error('Host returned a different grant observation');
      const catalog = await call.sessions.list();
      if (!catalog.entries.some((entry) => entry.session.sessionId === 'background-session'))
        throw new Error('Profile catalog did not include authorized Session metadata');
      try {
        await call.history.read({ sessionId: 'background-session' });
        throw new Error('metadata-only consent became history authority');
      } catch (error) {
        if (error.code !== 'revoked') throw error;
      }
      try {
        await call.executions.open();
        throw new Error('metadata consent became execution authority');
      } catch (error) {
        if (error.code !== 'revoked') throw error;
      }
      await call.clients.notify({
        id: intent.operation,
        title: 'Public plugin notification',
        body: 'No Session or Agent identity is fabricated',
        destination: { kind: 'local' },
      });
      return true;
    });
  });
  await ctx.remote.method('clients', async (input, caller) => {
    if (!input || typeof input !== 'object' || Array.isArray(input))
      throw new Error('invalid Client request');
    /** @param {import('../../../../packages/plugin-sdk/src/host.js').ResourceContext} call */
    const exercise = async (call) => {
      try {
        await call.sessions.list();
        throw new Error('Client consent became catalog read authority');
      } catch (error) {
        if (error.code !== 'revoked') throw error;
      }
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
        managed: true,
        settings: {
          target: { kind: 'executor', executorId: 'example.background' },
          sandboxMode: 'read-only',
          approvalPolicy: { kind: 'on-request' },
          collaborationMode: 'agent',
          behavior: 'default',
        },
      };
      try {
        await commands.createRoot({
          ...request,
          settings: { ...request.settings, sandboxMode: 'danger-full-access' },
        });
        throw new Error('workspace grant widened');
      } catch (error) {
        if (error.code !== 'revoked') throw error;
      }
      const root = await commands.createRoot(request);
      const session = await commands.session(root.sessionId);
      const configured = await commands.configure({
        sessionId: root.sessionId,
        expectedRevision: session.revision,
        target: session.target,
      });
      if (configured.kind !== 'committed' || configured.session.sessionId !== root.sessionId)
        throw new Error('managed Session configuration did not commit');
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
      const originalInput = await commands.input(receipt.invocation);
      if (originalInput?.text !== 'Independent root work')
        throw new Error('original input changed');
      await commands.stop(receipt.invocation);
      const activity = await commands.activity(root.sessionId);
      if (activity.execution && activity.execution.invocation.session_id !== root.sessionId)
        throw new Error('activity escaped the authorized Session');
      if (
        (await commands.artifact({
          operationId: 'workspace-work',
          artifactId: 'missing-artifact',
          offset: 0,
          limit: 4096,
        })) !== null
      )
        throw new Error('missing artifact was fabricated');
      try {
        await commands.activity('not-authorized');
        throw new Error('foreign Session activity was disclosed');
      } catch (error) {
        if (error.code !== 'revoked') throw error;
      }
      const view = await commands.session(root.sessionId);
      if (view.sandboxMode !== 'read-only' || view.target.kind !== 'executor')
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
        capabilities: [
          'processes',
          'network',
          'read_files',
          'write_files',
          'models',
          'executions',
          'read_sessions',
        ],
      },
      async (call) => {
        await exercise(call);
        const catalog = await call.sessions.list();
        if (
          catalog.entries.length !== 1 ||
          catalog.entries[0].session.sessionId !== 'background-session'
        )
          throw new Error('Session catalog escaped its authorized scope');
        const commands = await call.executions.open();
        try {
          /** @type {import('../../../../packages/plugin-sdk/src/host.js').CreateChild} */
          const request = {
            operationId: 'remote-child',
            parentSessionId: 'background-session',
            name: 'Authorized child',
            target: { kind: 'executor', executorId: 'example.background' },
          };
          if (await commands.restoreChild(request))
            throw new Error('missing child must not be provisioned by restore');
          const child = await commands.createChild(request);
          const recovered = await call.executions.open();
          try {
            const restored = await recovered.restoreChild(request);
            if (restored?.sessionId !== child.sessionId)
              throw new Error('child restore changed its identity');
            await recovered.session(child.sessionId);
          } finally {
            await recovered.close();
          }
          const view = await commands.session(child.sessionId);
          if (
            view.target.kind !== 'executor' ||
            view.target.executorId !== 'example.background' ||
            view.sandboxMode !== 'danger-full-access' ||
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
    async (request, call) => {
      const catalog = await call.sessions.list();
      if (
        catalog.entries.length !== 1 ||
        catalog.entries[0].session.sessionId !== request.invocation.session_id
      )
        throw new Error('Agent catalog escaped its own Session');
      const commands = await call.executions.open();
      try {
        const operationId = `question-${request.invocation.turn_id}`;
        const offered = await commands.offerInteraction({
          operationId,
          invocation: request.invocation,
          prompt: {
            kind: 'form',
            message: 'Public plugin input',
            fields: [{ kind: 'boolean', name: 'confirmed', label: 'Continue?', required: true }],
          },
        });
        if (
          offered.request.kind !== 'form' ||
          offered.request.requester.name !== 'example.background'
        )
          throw new Error('Host did not stamp the authenticated requester');
        if ((await commands.interaction(operationId))?.requestId !== offered.requestId)
          throw new Error('interaction lost its stable identity');
        const closed = await commands.closeInteraction(operationId);
        const outcome = await commands.waitInteraction(operationId);
        if (
          closed.outcome?.kind !== 'closure' ||
          outcome.kind !== 'closure' ||
          outcome.reason !== 'producer_cancelled'
        )
          throw new Error('interaction withdrawal did not settle canonically');
        const queuedInput = {
          operationId: `followup-${request.invocation.turn_id}`,
          messageId: `queued-${request.invocation.turn_id}`,
          invocation: request.invocation,
          content: { text: 'Must not execute' },
          placement: /** @type {const} */ ('next_turn'),
        };
        const queued = await commands.enqueue(queuedInput);
        if ((await commands.enqueue(queuedInput)).messageId !== queued.messageId)
          throw new Error('queue retry lost its receipt');
        if ((await commands.message(queuedInput.operationId)).state.state !== 'pending')
          throw new Error('queued message did not remain pending');
        if (
          (
            await commands.readMessage({
              sessionId: request.invocation.session_id,
              messageId: queued.messageId,
            })
          )?.state !== 'pending'
        )
          throw new Error('message observation lost exact identity');
        if ((await commands.retract(queuedInput.operationId)).state.state !== 'cancelled')
          throw new Error('queued message was not retracted');
        return { status: 'completed', text: 'Accepted background work' };
      } finally {
        await commands.close();
      }
    },
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
  await call.files.entries.createDirectory('staging');
  await call.files.entries.write({ path: 'staging/next', bytes: [0, 255, 42] });
  await call.files.entries.write({ path: 'staging/existing', bytes: [7] });
  try {
    await call.files.entries.rename('staging/next', 'staging/existing');
    throw new Error('publication overwrote a concurrent file');
  } catch (error) {
    if (error.code !== 'conflict') throw error;
  }
  await call.files.entries.rename('staging/next', 'staging/published');
  const binary = await call.files.entries.read({ path: 'staging/published' });
  const metadata = await call.files.entries.stat('staging/published');
  if (binary.bytes[1] !== 255 || metadata.size !== 3 || metadata.kind !== 'file')
    throw new Error('authorized directory operations lost file data');
  await call.files.entries.remove('staging/published');
  await call.files.entries.remove('staging/existing');
  await call.files.entries.remove('staging');
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
