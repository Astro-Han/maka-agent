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
export default async function activate(ctx) {
  /** @type {import('../../../../packages/plugin-sdk/src/host.js').RemoteCaller | undefined} */
  let previousDatabase;
  const database = async (
    /** @type {import('../../../../packages/plugin-sdk/src/host.js').Json} */ input,
    /** @type {import('../../../../packages/plugin-sdk/src/host.js').RemoteCaller} */ caller,
  ) => {
    const request =
      /** @type {import('../../../../packages/plugin-sdk/src/host.js').DatabaseRead} */ (
        /** @type {unknown} */ (input)
      );
    if (previousDatabase) {
      try {
        await previousDatabase.views.queryDatabase(request);
        throw new Error('completed Remote call retained database authority');
      } catch (error) {
        if (error.code !== 'revoked') throw error;
      }
    }
    const tables = await caller.views.queryDatabase(request);
    previousDatabase = caller;
    return tables.map((table) => ({ ...table }));
  };
  await ctx.remote.method('database', database, { access: 'host_paths' });
  await ctx.remote.method(
    'database-summary',
    async (input, caller) => {
      const tables = await database(input, caller);
      return tables
        .flatMap((table) => table.rows)
        .reduce(
          (size, row) =>
            size +
            row.reduce((sum, cell) => sum + (cell.kind === 'text' ? cell.value.length : 0), 0),
          0,
        );
    },
    { access: 'host_paths' },
  );
  await ctx.remote.method('denied-database', database);
  await ctx.remote.method('uncertain', () => {
    /** @type {import('../../../../packages/plugin-sdk/src/host.js').RemoteFailure} */
    const failure = Object.assign(new Error('publication result needs recovery'), {
      code: /** @type {const} */ ('outcome_unknown'),
    });
    throw failure;
  });
  await ctx.remote.stream('uncertain-stream', () => ({
    next() {
      throw Object.assign(new Error('stream operation result needs recovery'), {
        code: 'outcome_unknown',
      });
    },
    cancel() {},
    close() {},
  }));
  /** @type {import('../../../../packages/plugin-sdk/src/host.js').RemoteCaller | undefined} */
  let previous;
  /** @type {import('../../../../packages/plugin-sdk/src/host.js').ReadDirectory | undefined} */
  let previousFiles;
  const workspace = async (
    /** @type {import('../../../../packages/plugin-sdk/src/host.js').Json} */ input,
    /** @type {import('../../../../packages/plugin-sdk/src/host.js').RemoteCaller} */ caller,
  ) => {
    if (typeof input !== 'string') throw new Error('expected workspace path');
    /** @type {import('../../../../packages/plugin-sdk/src/host.js').WorkspaceViewInput} */
    const request = {
      workspace: { kind: 'host_path', path: input },
      sandboxMode: 'read-only',
      collaborationMode: 'agent',
    };
    if (previous) {
      try {
        await previous.views.workspace(request);
        throw new Error('completed Remote call retained authority');
      } catch (error) {
        if (error.code !== 'revoked') throw error;
      }
    }
    if (previousFiles) {
      try {
        await previousFiles.list();
        throw new Error('completed Remote call retained filesystem authority');
      } catch (error) {
        if (error.code !== 'revoked') throw error;
      }
    }
    const view = await caller.views.workspace(request);
    await view.files.list({ limit: 1 });
    try {
      await view.files.read({ path: '../outside' });
      throw new Error('Remote read escaped its workspace');
    } catch (error) {
      if (error.code !== 'invalid') throw error;
    }
    previous = caller;
    previousFiles = view.files;
    return { cwd: view.workspace.hostCwd };
  };
  await ctx.remote.method('workspace', workspace, { access: 'host_paths' });
  await ctx.remote.method('denied-workspace', workspace);
  await ctx.remote.stream(
    'workspace-stream',
    (input, caller) => ({
      next: async () => ({ done: false, value: await workspace(input, caller) }),
      cancel() {},
      close() {},
    }),
    { access: 'host_paths' },
  );
  const state = { generation: 0, opening: 0, active: 0, stopped: 0 };
  let echo = await ctx.remote.method('echo', (input, caller) => ({
    input,
    client: caller.clientInstanceId,
    session: caller.sessionId,
    generation: state.generation,
  }));
  await ctx.remote.method('replace', async () => {
    await echo.close();
    state.generation++;
    echo = await ctx.remote.method('echo', (input) => ({ input, generation: state.generation }));
    return true;
  });
  await ctx.remote.method('stats', () => ({ ...state }));
  await ctx.remote.stream('events', async (input, caller) => {
    state.opening++;
    if (input === 'late') await caller.signal.wait();
    state.opening--;
    state.active++;
    let cancelled = false;
    let index = 0;
    return {
      async next() {
        if (index++ === 0) return { done: false, value: null };
        await caller.signal.wait();
        return { done: true, value: undefined };
      },
      cancel() {
        if (!cancelled) state.stopped++;
        cancelled = true;
      },
      close() {
        if (!cancelled) throw new Error('close must follow cancellation');
        state.active--;
      },
    };
  });
}
