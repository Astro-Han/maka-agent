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
import { catalog, header, modelId, updateDeclaration } from './client-model-overrides-catalog.mjs';

export async function verifyFactsConnectionTests(request, fixture, row, pin, observe) {
  const run = () => request('connection.test.run', { connectionId: row.connectionId, modelId });
  const windowGate = fixture.expect({
    path: '/v1/responses',
    model: modelId,
    probe: true,
    hold: true,
  });
  const windowTest = run();
  await windowGate.wait();
  const finalPin = { ...pin, contextWindow: pin.contextWindow + 1000 };
  await updateDeclaration(observe, row, finalPin);
  const duringWindow = await catalog(observe);
  windowGate.release();
  const verified = await windowTest;
  assert.equal(verified.kind, 'committed');
  assert.equal(verified.test.kind, 'verified');
  assert.equal(verified.test.modelId, modelId);
  const tested = await catalog(request);
  const retained = header(tested, row.connectionId).lastTest;
  assert.equal(retained.status, 'verified');

  const protocolGate = fixture.expect({
    path: '/v1/responses',
    model: modelId,
    probe: true,
    hold: true,
  });
  const protocolTest = run();
  await protocolGate.wait();
  await updateDeclaration(observe, row, { ...finalPin, apiProtocol: 'openai-chat' });
  const hidden = await catalog(observe);
  assert.equal(
    Object.hasOwn(header(hidden, row.connectionId), 'lastTest'),
    false,
    'query hides an old test when relevant apiProtocol pins changed',
  );
  protocolGate.release();
  const superseded = await protocolTest;
  assert.deepEqual(superseded, { kind: 'superseded', changed: ['connection'] });
  const afterSuperseded = await catalog(request);
  assert.deepEqual(
    afterSuperseded,
    hidden,
    'superseded completion must not persist an old probe result',
  );
  await updateDeclaration(observe, row, finalPin);
  const restored = await catalog(request);
  assert.equal(
    header(restored, row.connectionId).lastTest,
    undefined,
    'restoring an old protocol does not resurrect invalidated verification',
  );
  fixture.check();
  return { pin: finalPin, duringWindow, verified, retained, hidden, superseded, restored };
}
