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
      const caller = await scoped.call({ inspect: true });
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
    text: 'JavaScript plugin acceptance',
  });
  ctx.tools.register(
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
    async (/** @type {{wait?:boolean}} */ input, call) => {
      try {
        const denied = await call.terminals.spawn(command);
        await denied.close();
        throw new Error('Ask Session gained unrestricted terminal access');
      } catch (error) {
        if (error.code !== 'invalid' || !error.message.includes('not authorized')) throw error;
      }
      try {
        const denied = await call.processes.spawn(command);
        await denied.close();
        throw new Error('Ask Session gained unrestricted process access');
      } catch (error) {
        if (error.code !== 'invalid' || !error.message.includes('not authorized')) throw error;
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
      await ctx.storage.batch([
        {
          key: 'count',
          expectedRevision: before?.revision ?? null,
          data: { kind: 'present', value: count },
        },
      ]);
      return value;
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
