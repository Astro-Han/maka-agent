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
  await ctx.tools.register(
    {
      name: 'ClientsBounded',
      description: 'Verify Client Capability ceiling',
      inputSchema: { type: 'object' },
      directOnly: true,
    },
    async (_input, call) => {
      if ((await call.clients.tools()).length !== 0) throw new Error('tool ceiling leaked catalog');
      try {
        await call.clients.call({ name: 'mcp__desktop__inspect', input: {} });
      } catch {
        return { bounded: true };
      }
      throw new Error('tool ceiling widened');
    },
  );
  /** @type {import('../../../../packages/plugin-sdk/src/host.js').ClientCapabilities | undefined} */
  let previous;
  await ctx.executors.register(
    { name: 'example.clients', displayName: 'Client SDK acceptance' },
    async (request, call) => {
      if (previous) {
        let rejected = false;
        try {
          await previous.tools();
        } catch {
          rejected = true;
        }
        if (!rejected) throw new Error('expired caller still has authority');
      }
      previous = call.clients;
      const command = request.content.text;
      const tools = await call.clients.tools();
      if (command === 'bounded') {
        if (tools.length !== 0) throw new Error('tool ceiling leaked catalog');
      } else if (tools.length !== 1 || tools[0].name !== 'mcp__desktop__inspect') {
        throw new Error('lost frozen catalog');
      }
      if (command === 'widen') {
        await ctx.storage.batch([
          { key: 'waiting', expectedRevision: null, data: { kind: 'present', value: true } },
        ]);
        while (!(await ctx.storage.read('continue'))) await ctx.sleep(10);
      }
      let result;
      try {
        result = await call.clients.call({ name: 'mcp__desktop__inspect', input: { command } });
      } catch (error) {
        if (command === 'complete') throw error;
        return { status: 'completed', text: 'denied' };
      }
      if (command !== 'complete') throw new Error('tool ceiling or frozen permission widened');
      if (
        JSON.stringify(result) !==
        JSON.stringify({ content: [], structuredContent: { inspected: true } })
      )
        throw new Error('client result changed');
      if ((await call.clients.tools()).some((tool) => tool.name === 'mcp__late__unexpected'))
        throw new Error('late capability escaped frozen composition');
      return { status: 'completed', text: 'complete' };
    },
  );
}
