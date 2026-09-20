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



import assert from 'node:assert/strict';
import test from 'node:test';
const SESSION = "public-plugin-session";
import { createWorkHubRuntime } from '../workhub-runtime.js';

const scope = { hostId: 'host', targetEpoch: 'epoch' };
function fixture() {
  let current = true;
  const stops: unknown[] = [];
  const client = {
    queryTurn: async () => ({ sessionId: SESSION, turnId: 'turn', runId: 'run', status: 'running' as const }),
    stopTurn: async (input: unknown) => { stops.push(input); },
  } as unknown as ReturnType<Parameters<typeof createWorkHubRuntime>[0]['client']>;
  const deps: Parameters<typeof createWorkHubRuntime>[0] = {
    isCurrent: () => current,
    client: () => client,
  };
  return { deps, client, stops, retire: () => { current = false; }, runtime: createWorkHubRuntime(deps) };
}


test('takeover stops the exact old turn and run even after selecting another Host', async () => {
  const f = fixture();
  f.retire();
  await f.runtime.interrupt(scope, SESSION, 'turn');
  assert.deepEqual(f.stops, [{ sessionId: SESSION, turnId: 'turn', runId: 'run' }]);
  await assert.rejects(f.runtime.assertTurn(scope, SESSION, 'turn'), /Runtime Host changed/);
});

test('a changed turn identity cannot become the takeover target', async () => {
  const f = fixture();
  f.client.queryTurn = async () => ({ sessionId: SESSION, turnId: 'new-turn', runId: 'new-run', status: 'running' });
  await assert.rejects(f.runtime.interrupt(scope, SESSION, 'turn'), /turn identity changed/);
  assert.deepEqual(f.stops, []);
});
