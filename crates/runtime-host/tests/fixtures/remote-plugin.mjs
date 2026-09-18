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
