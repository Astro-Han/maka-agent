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

export default async function (ctx) {
  await ctx.modelAdapters.register('example.protocol', (lifetime) => {
    let socket;
    let confirmed = false;
    let step = 0;
    return {
      confirm(value) {
        if (!value.prompt.some((message) => message.role === 'tool'))
          throw new Error('settlement missing');
        confirmed = true;
      },
      async stream(request, call) {
        if (request.provider.apiKey !== 'local-recovery-fixture')
          throw new Error('resolved credentials missing');
        const response = await call.transport.request({
          url: request.provider.baseUrl + '/chat/completions',
          method: 'POST',
          body: JSON.stringify({ model: request.provider.model, messages: [] }),
        });
        if (response.status !== 200) throw new Error('HTTP failed');
        while ((await response.next()) !== null) {}
        socket ??= await call.transport.connect({ url: '__WEBSOCKET__' });
        await call.transport.send(socket, { kind: 'text', data: 'step:' + step });
        const echo = await call.transport.receive(socket);
        if (echo?.data !== 'step:' + step) throw new Error('socket continuation failed');
        await call.progress();
        const usage = {
          input_tokens: 3,
          output_tokens: 5,
          cache_read_tokens: null,
          cache_write_tokens: null,
          reasoning_tokens: 2,
        };
        if (step++ === 0) {
          if (!request.tools.some((tool) => tool.name === 'tool_search'))
            throw new Error('request tool catalog missing');
          await call.emit({
            kind: 'tool_call',
            data: {
              id: 'raw|plugin:1',
              name: 'tool_search',
              input: { query: 'Read' },
              provider_executed: false,
              provider_options: null,
            },
          });
          await call.emit({
            kind: 'finished',
            data: { reason: 'tool-calls', usage, provider_options: null },
          });
        } else {
          if (!confirmed || lifetime !== 'conversation')
            throw new Error('canonical confirmation missing');
          await call.emit({
            kind: 'part_started',
            data: { id: 'text', text_kind: 'text', provider_options: null },
          });
          await call.emit({
            kind: 'part_delta',
            data: { id: 'text', text: 'external adapter complete', provider_options: null },
          });
          await call.emit({ kind: 'part_finished', data: { id: 'text', provider_options: null } });
          await call.emit({
            kind: 'finished',
            data: { reason: 'stop', usage, provider_options: null },
          });
        }
      },
    };
  });
}
