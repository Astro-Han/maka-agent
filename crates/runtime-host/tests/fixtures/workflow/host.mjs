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

/** @param {import('../../../../../packages/plugin-sdk/src/host.js').HostContext} ctx */
export default async function activate(ctx) {
  const executor = ctx.identity.packageId;
  await ctx.executors.register(
    { name: executor, displayName: 'Public workflow', capabilities: {} },
    async (_request, call) => {
      await call.emit({ type: 'output_delta', text: 'Public plugin execution completed.' });
      return { status: 'completed', text: 'Public plugin execution completed.' };
    },
  );
  let intent = await ctx.storage.read('intent');
  let dirty = intent?.data.kind === 'present';
  /** @type {() => void} */
  let wake = () => {};
  /** @type {import('../../../../../packages/plugin-sdk/src/host.js').Json} */
  let state = null;
  await ctx.remote.method('queue', async (value) => {
    const input = parse(value);
    [intent] = await ctx.storage.batch([
      {
        key: 'intent',
        expectedRevision: intent?.revision ?? null,
        data: { kind: 'present', value: { grant: input.grant, operation: input.operation } },
      },
    ]);
    dirty = true;
    wake();
    return true;
  });
  await ctx.remote.method('state', () => state);
  ctx.run(async () => {
    while (!ctx.signal.aborted) {
      if (!dirty) {
        await Promise.race([
          new Promise((resolve) => {
            wake = () => resolve(undefined);
          }),
          ctx.signal.wait(),
        ]);
        continue;
      }
      dirty = false;
      if (intent?.data.kind !== 'present') continue;
      const input = parse(intent.data.value);
      let commands;
      try {
        commands = await ctx.executions.restore(input.grant);
        const root = await commands.createRoot({
          operationId: 'workspace',
          name: 'Public workflow',
          managed: true,
          settings: {
            target: { kind: 'executor', executorId: executor },
            sandboxMode: 'read-only',
            approvalPolicy: { kind: 'on-request' },
            toolMode: 'direct',
            collaborationMode: 'agent',
            behavior: 'default',
          },
        });
        const receipt = await commands.submit({
          operationId: input.operation,
          sessionId: root.sessionId,
          content: { text: 'Exercise public authorization, persistence and execution.' },
        });
        let observation = await commands.query(input.operation);
        while (observation.progress.state !== 'ended' && !ctx.signal.aborted) {
          await ctx.sleep(25);
          observation = await commands.query(input.operation);
        }
        state = JSON.parse(
          JSON.stringify({ grant: input.grant, receipt, progress: observation.progress }),
        );
      } catch (error) {
        state = { error: error.code ?? 'failed', message: String(error) };
      } finally {
        await commands?.close();
      }
    }
  });
}

/** @param {unknown} value */
function parse(value) {
  if (
    !value ||
    typeof value !== 'object' ||
    !('grant' in value) ||
    !('operation' in value) ||
    typeof value.grant !== 'string' ||
    typeof value.operation !== 'string'
  )
    throw new Error('Expected authorization and operation identities');
  return { grant: value.grant, operation: value.operation };
}
