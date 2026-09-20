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
  /** @type {import('../../../../packages/plugin-sdk/src/host.js').Files | undefined} */
  let previous;
  /** @param {() => Promise<unknown>} operation */
  const denied = async (operation) => {
    try {
      await operation();
    } catch {
      return;
    }
    throw new Error('forbidden file operation succeeded');
  };
  await ctx.services.provide('example.files', async (_input, call) => {
    if (!call.invocation) throw new Error('missing invocation');
    return call.files.write({ path: 'fault.txt', content: 'effect happened' });
  });
  await ctx.tools.register(
    {
      name: 'FilesForward',
      description: 'Verify forwarded settlement',
      inputSchema: { type: 'object', additionalProperties: false },
      directOnly: true,
      semantics: 'finish_turn',
    },
    async (_input, call) => {
      const service = await call.services.get('example.files');
      if (!service) throw new Error('missing service');
      try {
        await service.call(null);
      } catch {
        /* Cannot hide an uncertain file effect. */
      } finally {
        await service.close();
      }
      return { incorrectlyCompleted: true };
    },
  );
  await ctx.tools.register(
    {
      name: 'FilesBounded',
      description: 'Verify bounded file access',
      inputSchema: { type: 'object', additionalProperties: false },
      directOnly: true,
    },
    async (_input, call) => {
      await denied(() => call.files.write({ path: 'forbidden.txt', content: 'wrong' }));
      await denied(() => call.files.grep({ pattern: 'one' }));
      return { bounded: true };
    },
  );
  await ctx.executors.register(
    { name: 'example.files', displayName: 'Filesystem acceptance' },
    async (request, call) => {
      const old = previous;
      if (old) await denied(() => old.read({ path: 'source.txt' }));
      previous = call.files;
      const page = await call.files.read({ path: 'source.txt', offset: 1, limit: 2 });
      if (!('content' in page) || page.content !== 'one\ntwo') throw new Error('file page damaged');
      const command = request.content.text;
      if (command === 'widen') {
        await ctx.storage.batch([
          { key: 'waiting', expectedRevision: null, data: { kind: 'present', value: true } },
        ]);
        while (!(await ctx.storage.read('continue'))) await ctx.sleep(10);
      }
      if (command === 'restricted' || command === 'widen') {
        await denied(() => call.files.read({ path: '../outside.txt' }));
        await denied(() => call.files.write({ path: 'forbidden.txt', content: 'wrong' }));
      } else {
        await call.files.write({ path: 'result.txt', content: 'before\n' });
        await call.files.edit({ path: 'result.txt', old_string: 'before', new_string: 'after' });
        const page = await call.files.read({ path: 'result.txt' });
        if (!('content' in page) || page.content !== 'after\n')
          throw new Error('write/edit failed');
        await call.files.patch({ type: 'create_file', path: 'created.txt', diff: '+created\n' });
        await call.files.patch({ type: 'delete_file', path: 'created.txt' });
        const grep = await call.files.grep({ pattern: 'match', path: 'many.txt' });
        if (grep.complete || grep.matches.length !== 50) throw new Error('grep completeness lost');
        const glob = await call.files.glob({ pattern: '*.txt' });
        if (!glob.files.includes('result.txt')) throw new Error('glob omitted file');
        const image = await call.files.read({ path: 'image.png' });
        if (!('ref' in image) || image.kind !== 'image' || image.ref.kind !== 'session_file') {
          throw new Error('image was not persisted before SDK delivery');
        }
        // Even a caught/abandoned operation remains owned until durable settlement.
        void call.files.write({ path: 'unawaited.txt', content: 'settled' }).catch(() => {});
      }
      return { status: 'completed', text: command };
    },
  );
}
