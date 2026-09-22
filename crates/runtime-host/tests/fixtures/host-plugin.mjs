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
  const model = await ctx.models.resolve({
    kind: 'named',
    connectionSlug: 'recovery',
    model: 'fixture-model',
  });
  if (
    !model ||
    model.connection_slug !== 'recovery' ||
    Object.keys(model).sort().join(',') !== 'connection_id,connection_slug,model'
  )
    throw new Error('model lookup leaked configuration or lost its binding');
  if ((await ctx.models.resolve({ kind: 'default' })) !== null)
    throw new Error('model lookup invented an unconfigured default');
  if (await ctx.models.resolve({ kind: 'named', connectionSlug: 'recovery', model: 'not-enabled' }))
    throw new Error('disabled model is selectable');

  /** @type {import('../../../../packages/plugin-sdk/src/host.js').ReadDirectory | undefined} */
  let preparedFiles;
  const revision = await ctx.revision();
  const behaviorRevision = await ctx.revision();
  let invalidated = false;
  await ctx.input.prepare('example.review', async (request) => {
    const basis = await revision.capture();
    preparedFiles = request.workspace;
    const proof = await request.workspace.read({ path: 'native-proof.txt', limit: 6 });
    if (new TextDecoder().decode(proof.bytes) !== 'native' || proof.nextOffset !== 6)
      throw new Error('preparation lost bounded workspace access');
    const selections = request.selections['example.review'];
    if (!selections) return { kind: 'unchanged' };
    if (selections[0] === 'missing')
      return {
        kind: 'blocked',
        message: 'Review document is unavailable',
        receipt: { document: selections[0] },
      };
    if (!invalidated)
      await revision.invalidate(() => {
        invalidated = true;
      });
    return {
      basis,
      kind: 'ready',
      content: {
        ...request.content,
        quotes: [{ text: 'Prepared by an external input provider', label: selections[0] }],
      },
      receipt: { document: selections[0] },
    };
  });
  const marker = 'state.bin';
  try {
    const page = await ctx.data.read({ path: marker });
    if (new TextDecoder().decode(page.bytes) !== '持久状态🦀')
      throw new Error('private file did not survive activation');
  } catch (error) {
    if (error.code !== 'not_found') throw error;
    await ctx.data.write({ path: marker, bytes: new TextEncoder().encode('持久状态🦀') });
  }
  await ctx.data.write({ path: 'temporary', bytes: [0, 255, 42], truncate: true });
  const firstPage = await ctx.data.read({ path: 'temporary', limit: 2 });
  if (firstPage.nextOffset === null) throw new Error('private-file cursor missing');
  const lastPage = await ctx.data.read({ path: 'temporary', offset: firstPage.nextOffset });
  if (
    firstPage.bytes[1] !== 255 ||
    firstPage.nextOffset !== 2 ||
    lastPage.bytes[0] !== 42 ||
    lastPage.nextOffset !== null
  )
    throw new Error('private-file byte pagination is lossy');
  const listing = await ctx.data.list({ limit: 1 });
  const remaining = await ctx.data.list({ after: listing.nextAfter });
  if (listing.nextAfter !== marker || remaining.entries[0]?.name !== 'temporary')
    throw new Error('private-file listing silently truncated');
  await ctx.data.rename('temporary', 'renamed');
  await ctx.data.remove('renamed');
  for (const path of ['../outside', 'C:/outside', 'CON', 'alias.']) {
    try {
      await ctx.data.write({ path, bytes: [] });
      throw new Error('invalid private path accepted');
    } catch (error) {
      if (error.code !== 'invalid') throw error;
    }
  }
  await ctx.behaviors.register('example.behavior', async ({ session }) => {
    if (
      session.sessionId !== 'js-session' ||
      session.behavior !== 'example.behavior' ||
      session.revision < 1
    )
      throw new Error('wrong behavior Session');
    return {
      instructions: 'External behavior instructions',
      basis: await behaviorRevision.capture(),
    };
  });
  let credential = await ctx.credentials.read('test-token');
  if (credential === null) {
    const written = await ctx.credentials.write({
      key: 'test-token',
      expectedRevision: null,
      secret: 'acceptance-secret',
    });
    if (written.kind !== 'written') throw new Error('credential CAS failed');
    credential = await ctx.credentials.read('test-token');
  }
  if (credential?.secret !== 'acceptance-secret')
    throw new Error('credential was lost across activation');
  if ((await ctx.storage.read('test-token')) !== null)
    throw new Error('credential leaked into general storage');
  const echo = await ctx.services.get('example.echo');
  /** @type {import('../../../../packages/plugin-sdk/src/host.js').Service<{inspect:boolean},{invocation:import('../../../../packages/plugin-sdk/src/host.js').Invocation,operationId:string|null}> | undefined} */
  let previousCall;
  /** @type {Map<string, import('../../../../packages/plugin-sdk/src/host.js').Process>} */
  const protocols = new Map();
  /** @type {Map<string, import('../../../../packages/plugin-sdk/src/host.js').Terminal>} */
  const terminals = new Map();
  const command = {
    executable: '__PROTOCOL_EXECUTABLE__',
    args: [
      '--exact',
      'javascript_plugins::external_shared_and_dedicated_plugins_route_services_persist_data_and_drain_on_disable',
      '--nocapture',
    ],
    env: { MAKA_PLUGIN_PROTOCOL_TEST_CHILD: '1' },
  };
  if (!echo) throw new Error('missing service');
  await ctx.executors.register(
    {
      name: 'example.external',
      displayName: 'External acceptance',
      capabilities: { thinking: true, toolActivity: true },
    },
    async (request, context) => {
      if (request.content.quotes?.[0]?.text !== 'Prepared by an external input provider')
        throw new Error('executor lost provider-prepared structured content');
      const session = request.invocation.session_id;
      if (
        session !== 'executor-session' &&
        !request.instructions?.includes('Executor child instructions')
      ) {
        throw new Error('child execution lost its declared instructions');
      }
      let protocol = protocols.get(session);
      if (protocol) {
        try {
          await protocol.write([0]);
          throw new Error('old process caller still authorized');
        } catch (error) {
          if (error.code !== 'revoked') throw error;
        }
        protocol = context.processes.open(protocol.id);
      } else {
        protocol = await context.processes.spawn({ ...command, lifetime: 'instance' });
      }
      protocols.set(session, protocol);
      await protocol.write('ping 測試🦀\n');
      const decoder = new TextDecoder('utf-8', { fatal: true });
      let output = '';
      while (!output.includes('protocol:ping 測試🦀')) {
        const chunk = await protocol.next();
        if (!chunk) throw new Error('protocol closed before reply');
        if (chunk.stream === 'stdout') output += decoder.decode(chunk.bytes, { stream: true });
      }
      // Deliberately leave a call-owned process alive: Host settlement must
      // cancel it and confirm cleanup before committing this invocation's T2.
      await context.processes.spawn(command);
      let terminal = terminals.get(session);
      const ttyCommand = { ...command, env: { ...command.env, MAKA_PLUGIN_PTY_TEST_CHILD: '1' } };
      if (terminal) {
        try {
          await terminal.resize({ cols: 100, rows: 30 });
          throw new Error('old terminal caller still authorized');
        } catch (error) {
          if (error.code !== 'revoked') throw error;
        }
        terminal = context.terminals.open(terminal.id);
      } else {
        terminal = await context.terminals.spawn({ ...ttyCommand, lifetime: 'instance' });
      }
      terminals.set(session, terminal);
      // PTY input is keystrokes: Enter is CR, including on ConPTY. Pipes above
      // carry the protocol's LF-delimited records instead.
      const receipt = await terminal.write('ping 終端🦀\r', { cols: 101, rows: 37 });
      if (
        !receipt.resized ||
        receipt.acceptedBytes !== new TextEncoder().encode('ping 終端🦀\r').length
      )
        throw new Error('terminal lost resize or input receipt');
      let terminalOutput = '';
      while (!terminalOutput.includes('protocol:ping 終端🦀')) {
        const event = await terminal.next();
        if (event.kind === 'closed') throw new Error('terminal closed before reply');
        terminalOutput = event.kind === 'reset' ? event.text : terminalOutput + event.text;
      }
      // An invocation-owned terminal is intentionally left for Host settlement.
      await context.terminals.spawn(ttyCommand);
      const completed = await context.terminals.spawn(ttyCommand);
      await completed.write('quit\r');
      const terminalExit = await completed.wait();
      if (terminalExit.kind !== 'completed')
        throw new Error(`terminal failed to drain: ${JSON.stringify(terminalExit)}`);
      await completed.close();
      const scoped = await context.services.get('example.echo');
      if (!scoped) throw new Error('service retired');
      const caller = await scoped.call({ inspect: true, resources: command });
      await scoped.close();
      if (
        JSON.stringify(caller) !==
        JSON.stringify({ invocation: request.invocation, operationId: null })
      )
        throw new Error('executor authority lost across VMs');
      await context.emit({ type: 'thinking_delta', text: 'External reasoning' });
      await context.emit({ type: 'output_delta', text: 'External partial' });
      await context.emit({
        type: 'tool_start',
        toolCallId: 'external-call',
        name: 'shell',
        input: { command: 'observation only' },
      });
      await context.emit({
        type: 'tool_result',
        toolCallId: 'external-call',
        text: 'External result',
        isError: false,
      });
      if (request.content.text === 'wait') {
        await ctx.storage.batch([
          {
            key: 'executor-waiting',
            expectedRevision: null,
            data: { kind: 'present', value: true },
          },
        ]);
        await context.signal.wait();
      }
      return { status: 'completed', text: 'External answer' };
    },
  );
  ctx.prompt.section({
    name: 'example.prompt',
    complete: true,
    async text(_request, call) {
      if (preparedFiles) {
        try {
          await preparedFiles.read({ path: 'native-proof.txt' });
          throw new Error('preparation view survived its callback');
        } catch (error) {
          if (error.code !== 'revoked') throw error;
        }
      }
      const proof = await call.workspace.read({ path: 'native-proof.txt' });
      if (!new TextDecoder().decode(proof.bytes).includes('native and JS'))
        throw new Error('prompt workspace access failed');
      try {
        await call.workspace.read({ path: '../outside.txt' });
        throw new Error('prompt escaped its workspace');
      } catch (error) {
        if (error.code !== 'invalid') throw error;
      }
      const names = await ctx.inputs.names();
      if (!names.includes('public-notes')) throw new Error('missing public input mount');
      const mountName = (await ctx.inputs.at('public-notes').location()).split(/[\\/]/).at(-1);
      const workspaceName = (await call.workspace.location()).split(/[\\/]/).at(-1);
      if (!mountName || mountName !== workspaceName)
        throw new Error('input location does not identify the mounted workspace');
      const mounted = await ctx.inputs.at('public-notes').read({ path: 'native-proof.txt' });
      if (new TextDecoder().decode(mounted.bytes) !== new TextDecoder().decode(proof.bytes))
        throw new Error('native and JS see different input mounts');
      try {
        await ctx.inputs.at('public-notes').read({ path: 'unshared.txt' });
        throw new Error('input mount ignored its file selection');
      } catch (error) {
        if (error.code !== 'invalid') throw error;
      }
      const preferences = await ctx.preferences.read();
      if (
        Object.keys(preferences).sort().join(',') !==
        'personalization,privacy,revision,workspaceInstructions'
      )
        throw new Error('preferences exposed unrelated Host configuration');
      return `JavaScript plugin acceptance: ${JSON.stringify(preferences)}`;
    },
  });
  ctx.tools.bind(
    [
      {
        name: 'PluginEcho',
        description: 'Call the plugin service and persist its result.',
        inputSchema: {
          type: 'object',
          properties: { wait: { type: 'boolean' } },
          additionalProperties: false,
        },
        directOnly: true,
        semantics: 'finish_turn',
      },
    ],
    async (_request, preparation) => {
      const page = await preparation.workspace.read({ path: 'binding-proof.txt' });
      const frozen = new TextDecoder().decode(page.bytes);
      return {
        context: `binding proof: ${frozen}`,
        async invoke(_name, /** @type {{wait?:boolean}} */ input, call) {
          try {
            await preparation.workspace.read({ path: 'binding-proof.txt' });
            throw new Error('tool binding retained its preparation authority');
          } catch (error) {
            if (error.code !== 'revoked') throw error;
          }

          for (const terminal of [false, true]) {
            const protectedCommand = {
              ...command,
              env: {
                ...command.env,
                MAKA_PLUGIN_SANDBOX_TEST_CHILD: '1',
                ...(terminal ? { MAKA_PLUGIN_PTY_TEST_CHILD: '1' } : {}),
              },
            };
            if (String('__MANAGED_SANDBOX__') === 'supported') {
              const resource = terminal
                ? await call.terminals.spawn(protectedCommand)
                : await call.processes.spawn(protectedCommand);
              try {
                await resource.write(terminal ? 'quit\r' : 'quit\n');
                const outcome = await resource.wait();
                if ('kind' in outcome ? outcome.kind !== 'completed' : !outcome.success)
                  throw new Error('managed plugin process did not enforce metadata protection');
              } finally {
                await resource.close();
              }
            } else {
              try {
                const resource = terminal
                  ? await call.terminals.spawn(protectedCommand)
                  : await call.processes.spawn(protectedCommand);
                await resource.close();
                throw new Error('unsupported isolation executed without a sandbox');
              } catch (error) {
                if (error.code !== 'invalid' || !error.message.includes('sandbox')) throw error;
              }
            }
          }
          if (previousCall) {
            try {
              await previousCall.call({ inspect: true });
              throw new Error('old invocation still authorized');
            } catch (error) {
              if (error.code !== 'revoked') throw error;
            }
            await previousCall.close();
          }
          previousCall = await call.services.get('example.echo');
          if (!previousCall) throw new Error('service retired');
          const caller = await previousCall.call({ inspect: true });
          if (
            caller.invocation.invocation_id !== call.invocation.invocation_id ||
            caller.operationId !== call.operationId
          )
            throw new Error('tool authority lost across VMs');
          if (input.wait) {
            // Abandon the Promise deliberately; retirement must still drain and settle it.
            void call.llm.generate({ prompt: 'cancel this' }).catch(() => {});
            await ctx.storage.batch([
              { key: 'waiting', expectedRevision: null, data: { kind: 'present', value: true } },
            ]);
            await call.signal.wait();
            return { interrupted: true };
          }
          const generated = await call.llm.generate({
            prompt: 'nested prompt',
            system: 'Auxiliary only',
          });
          if (
            generated.text !== 'nested answer' ||
            generated.finishReason !== 'stop' ||
            generated.usage.input_tokens !== 3 ||
            generated.usage.output_tokens !== 5
          )
            throw new Error('nested model result or usage was lost');
          for (let generation = 0; generation < 300; generation++) {
            const dynamic = await ctx.prompt.variable('temporary', () => String(generation));
            await dynamic.close();
          }
          const registration = await ctx.services.provide('temporary.echo', (value) => value);
          const temporary = await ctx.services.get('temporary.echo');
          if (!temporary) throw new Error('missing temporary service');
          if ((await temporary.call('live')) !== 'live') throw new Error('service not callable');
          await registration.close();
          try {
            await temporary.call('stale');
            throw new Error('retired service accepted a stale call');
          } catch (error) {
            if (error.code !== 'revoked') throw error;
          }
          await temporary.close();
          const before = await ctx.storage.read('count');
          const count =
            before?.data.kind === 'present' && typeof before.data.value === 'number'
              ? before.data.value + 1
              : 1;
          const value = await echo.call({ count });
          const priorProof = await ctx.storage.read('bound-proof');
          await ctx.storage.batch([
            {
              key: 'bound-proof',
              expectedRevision: priorProof?.revision ?? null,
              data: { kind: 'present', value: frozen },
            },
          ]);
          await ctx.storage.batch([
            {
              key: 'count',
              expectedRevision: before?.revision ?? null,
              data: { kind: 'present', value: count },
            },
          ]);
          const page = await ctx.storage.scan({ prefix: 'count' });
          if (
            page.nextAfter !== null ||
            page.entries.length !== 1 ||
            page.entries[0].key !== 'count' ||
            page.entries[0].record.data.kind !== 'present' ||
            page.entries[0].record.data.value !== count
          )
            throw new Error('public storage enumeration lost its namespace or record');
          return value;
        },
      };
    },
  );
  ctx.run(async () => {
    try {
      await ctx.sleep(86_400_000);
    } catch (error) {
      if (!ctx.signal.aborted) throw error;
    }
  });
}
