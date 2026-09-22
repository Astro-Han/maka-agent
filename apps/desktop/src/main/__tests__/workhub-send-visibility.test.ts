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
import { afterEach, test } from 'node:test';
import { act, createElement } from 'react';
import { LocaleProvider } from '@maka/ui';
import { deferred } from '@maka/core/test-only/async-primitives';
import { RemoteError } from '@maka-agent/plugin-sdk/client';
import { RuntimeHostRequestInterruptedError } from '@maka/runtime-host/client';
import { registerRuntimeHostSessionExecutionIpc, type RuntimeHostSessionExecutionIpcDeps } from '../runtime-host-session-execution-ipc-main.js';
import type { IpcHandler } from '../ipc-reconnect-policy.js';
import type { DesktopSessionStopResult } from '../../preload/bridge-contract.js';
import type { AttachmentRef } from '@maka/core/events';
import type { StoredMessage } from '@maka/core/session';
import { coordinationCommands, useWorkHubController, type WorkHubContinuation, type CoordinationSessionServices as WorkHubServices, type WorkHubTranscriptSnapshot } from '@maka/workhub/controller';
import { cleanupFakeDom, installReactRenderer } from './fake-dom.js';

afterEach(cleanupFakeDom);

async function mountController(failFirstRead = false, overrides: Partial<WorkHubServices> = {}, continuation?: WorkHubContinuation) {
  let openCount = 0;
  let onPhase!: (phase: 'pending' | 'ready') => void;
  const { root } = installReactRenderer();
  let controller!: ReturnType<typeof useWorkHubController>;
  let publish!: (snapshot: WorkHubTranscriptSnapshot) => void;
  let onExecution: Parameters<WorkHubServices['observe']>[4];
  let observe!: Parameters<WorkHubServices['observe']>[1];
  let earlierLoads = 0;
  let admission = deferred<{ turnId: string }>();
  const requests: Array<Parameters<WorkHubServices['answer']>[1]> = [];
  const accepted = new Map<string, import('@maka-agent/plugin-sdk/host').ExecutionReceipt>();
  const cancellations: string[] = [];
  let rootTurn: { turnId: string; runId: string; status: 'running' | 'cancelled' | 'completed' } | undefined;
  const queueMutations: unknown[][] = [];
  const steers: Array<Parameters<WorkHubServices['enqueueMessage']>> = [];
  let steerResult: Awaited<ReturnType<WorkHubServices['enqueueMessage']>> = 'admitted';
  let onSteer: ((input: Parameters<WorkHubServices['enqueueMessage']>) => void) | undefined;
  const interrupts: Array<{ sessionId: string; turnId: string; runId: string }> = [];
  let stopRetractions: string[] = [];
  const handlers = new Map<string, IpcHandler>();
  const ipc = { handle: (channel: string, handler: IpcHandler) => { handlers.set(channel, handler); } };
  registerRuntimeHostSessionExecutionIpc({
    observer: { snapshot: async () => ({ rootTurn }) },
    beforeStop: async () => {},
    emitSessionsChanged: () => {},
    client: { interruptTurn: async (input: typeof interrupts[number]) => {
      interrupts.push({ sessionId: input.sessionId, turnId: input.turnId, runId: input.runId });
      rootTurn!.status = 'cancelled';
      projectExecution();
      return { retracted: stopRetractions.map((messageId) => ({ messageId })) };
    } },
  } as unknown as RuntimeHostSessionExecutionIpcDeps, ipc);
  const commands = coordinationCommands({
    signal: new AbortController().signal,
    remote: {
      method: (name: string) => async (input: Parameters<WorkHubServices['answer']>[1]) => {
        const receipt = accepted.get(input.operationId);
        if (name === 'answer-receipt') return receipt ? {
          receipt, progress: rootTurn?.turnId === receipt.invocation.turn_id && rootTurn.status !== 'running'
            ? { state: 'ended', outcome: { kind: 'completed' } } : { state: 'running' },
        } : null;
        if (name === 'answer-cancel') {
          assert.ok(receipt);
          cancellations.push(input.operationId);
          if (rootTurn?.turnId === receipt.invocation.turn_id) { rootTurn.status = 'cancelled'; projectExecution(); }
          return { receipt, progress: { state: 'ended', outcome: { kind: 'cancelled', source: 'test' } } };
        }
        assert.equal(name, 'answer');
        requests.push(input);
        try {
          const { turnId } = await admission.promise;
          const receipt = { invocation: { session_id: 'workhub:coordination', turn_id: turnId, run_id: 'run:' + turnId, invocation_id: 'inv:' + turnId }, messageId: 'user:' + turnId, contentDigest: 'digest' };
          accepted.set(input.operationId, receipt);
          return receipt;
        }
        catch (error) {
          if (error instanceof RuntimeHostRequestInterruptedError) throw error;
          throw new RemoteError('invalid_request', String((error as Error).message));
        }
      },
    } as unknown as Parameters<typeof coordinationCommands>[0]['remote'],
  }, (_sessionId, refs) => [...refs], 'workhub:coordination');
  const invoke = (channel: string, ...args: unknown[]) => handlers.get(channel)!({} as Parameters<IpcHandler>[0], ...args);

  const sessionId = JSON.stringify(['host-1', 'workhub-coordination']);
  function projectExecution(available = true) {
    onExecution?.({ type: 'host_execution', available,
      rootTurn: rootTurn ? { ...rootTurn, sessionId,
        ...(rootTurn.status === 'running' ? { status: 'running' as const } : { status: rootTurn.status, terminalEventId: 'terminal', abortSource: 'user_stop' }) } : null });
  }
  const services = {
    getSession: async () => ({ id: sessionId, runningTurnIds: [] }),
    listSessions: async () => [],
    modelChoices: async () => [],
    subscribeAvailability: () => () => {},
    subscribeSessions: () => () => {},
    observe: (_id: string, handler: typeof observe, _onError: unknown, phase: typeof onPhase, execution: typeof onExecution) => { observe = handler; onPhase = phase; onExecution = execution; return () => {}; },
    openTranscript: async (_id: string, handler: typeof publish) => {
      openCount++;
      if (failFirstRead && openCount === 1) throw new Error('transient initial read failure');
      publish = handler;
      handler({ messages: [], ready: true, hasOlder: false });
      return {
        observationChanged: () => {},
        loadEarlier: async () => { earlierLoads += 1; },
        close: async () => {},
      };
    },
    retractQueueEntry: async (...input: Parameters<WorkHubServices['retractQueueEntry']>) => { queueMutations.push(['retract', ...input]); },
    promoteQueueEntry: async (...input: Parameters<WorkHubServices['promoteQueueEntry']>) => { queueMutations.push(['promote', ...input]); },
    updateQueueEntry: async (...input: Parameters<WorkHubServices['updateQueueEntry']>) => { queueMutations.push(['update', ...input]); },
    reorderQueueEntries: async (...input: Parameters<WorkHubServices['reorderQueueEntries']>) => { queueMutations.push(['reorder', ...input]); },
    enqueueMessage: async (...input: Parameters<WorkHubServices['enqueueMessage']>) => { steers.push(input); onSteer?.(input); return steerResult; },
    listActiveInteractions: async () => [],
    subscribeActiveInteractions: () => () => {},
    respondToUserForm: async () => {},
    respondToUserQuestion: async () => {},
    answer: commands.answer,
    cancelAnswer: commands.cancelAnswer,
    stop: async (target: string, turnId: string) => {
      const result = await invoke('sessions:stop', target, { source: 'stop_button', expectedTurnId: turnId }) as DesktopSessionStopResult;
      return result?.kind === 'interrupted' ? result.retractedMessageIds : undefined;
    },
    ...overrides,
  } as unknown as WorkHubServices;
  let submissions = 0;
  function Probe() { controller = useWorkHubController(sessionId, services, () => { submissions++; }, continuation); return null; }
  await act(async () => {
    root.render(createElement(LocaleProvider, { locale: 'en', children:
      createElement(Probe),
    }));
  });
  assert.equal(controller.sessionId, sessionId);
  return {
    get submissions() { return submissions; },
    get controller() { return controller; }, get openCount() { return openCount; },
    reconnect() { onPhase('pending'); onPhase('ready'); },
    complete(turnId: string) { rootTurn = { turnId, runId: `run:${turnId}`, status: 'completed' }; projectExecution(); },
    onSteer(handler: typeof onSteer) { onSteer = handler; },
    queueMutations, steers, setSteerResult(value: typeof steerResult) { steerResult = value; },
    setStopRetractions(ids: string[]) { stopRetractions = ids; },
    sessionId, requests, accepted, cancellations, get admission() { return admission; }, interrupts,
    resetAdmission() { admission = deferred<{ turnId: string }>(); },
    admit(turnId: string) { rootTurn = { turnId, runId: `run:${turnId}`, status: 'running' }; projectExecution(); },
    loseObservation() { projectExecution(false); },
    get earlierLoads() { return earlierLoads; },
    emit(event: Parameters<typeof observe>[0]) { observe(event); },
    publish(messages: StoredMessage[]) { publish({ messages, ready: true, hasOlder: false }); },
  };
}

test('WorkHub model and thinking selection share versioned saves and reject stale reads', async () => {
  type Session = Awaited<ReturnType<WorkHubServices['getSession']>>;
  const initial = {
    id: JSON.stringify(['host-1', 'workhub-coordination']),
    revision: 1, model: 'A', llmConnectionId: 'connection', llmConnectionSlug: 'provider',
    runningTurnIds: [],
  } as unknown as Session;
  let snapshot = initial;
  let failSave = false;
  let notify!: () => void;
  let notifyModels!: () => void;
  type Choices = Awaited<ReturnType<WorkHubServices['modelChoices']>>;
  const oldModels = deferred<Choices>();
  const currentModels = deferred<Choices>();
  let modelReads = 0;
  let nextRead: Promise<Session> | undefined;
  const requests: Array<Parameters<WorkHubServices['configureModel']>[1]> = [];
  const h = await mountController(false, {
    modelChoices: () => (++modelReads === 1 ? oldModels.promise : currentModels.promise),
    subscribeAvailability: (handler) => { notifyModels = handler; return () => {}; },
    getSession: async () => {
      const read = nextRead;
      nextRead = undefined;
      return read ?? snapshot;
    },
    subscribeSessions: (handler) => { notify = handler; return () => {}; },
    configureModel: async (_id, input) => {
      requests.push(input);
      if (failSave) throw new Error('configuration failed');
      snapshot = { ...snapshot, model: input.target.model.model, thinkingLevel: input.target.thinkingLevel ?? undefined, revision: snapshot.revision + 1 };
      return { kind: 'committed', session: snapshot } as unknown as Awaited<ReturnType<WorkHubServices['configureModel']>>;
    },
  });
  await act(async () => {
    notifyModels();
    currentModels.resolve([{ connectionId: 'connection', connectionSlug: 'provider',
      providerType: 'openai', providerLabel: 'OpenAI', model: 'B', label: 'B',
      isDefault: true, thinkingLevels: ['high'] }]);
  });
  assert.equal(h.controller.choices[0]?.model, 'B');
  await act(async () => { oldModels.resolve([]); });
  assert.equal(h.controller.choices[0]?.model, 'B', 'an old catalog read cannot erase refreshed choices');
  const staleRead = deferred<Session>();
  nextRead = staleRead.promise;
  await act(async () => { notify(); });
  const confirmation = deferred<Session>();
  nextRead = confirmation.promise;
  let settled = false;
  let change!: Promise<void>;
  await act(async () => {
    change = h.controller.changeModel({ llmConnectionId: 'connection', llmConnectionSlug: 'provider', model: 'B' });
    void change.then(() => { settled = true; });
  });
  assert.equal(settled, false, 'the wheel must remain pending until the saved session is available');
  assert.equal(h.controller.configuringModel, true);
  await act(async () => { await h.controller.changeThinkingLevel('high'); });
  assert.equal(requests.length, 1, 'model and thinking saves cannot overlap');
  await act(async () => { confirmation.resolve(snapshot); await change; });
  assert.equal(h.controller.session?.model, 'B');
  assert.equal(h.controller.session?.revision, 2);
  await act(async () => { staleRead.resolve(initial); });
  assert.equal(h.controller.session?.model, 'B', 'a late background snapshot cannot roll back a successful pick');
  await act(async () => {
    await h.controller.changeModel({ llmConnectionId: 'connection', llmConnectionSlug: 'provider', model: 'C' });
  });
  assert.equal(requests[1]?.expectedRevision, 2, 'the next pick uses the committed revision');
  assert.equal(h.controller.session?.model, 'C');
  await act(async () => { await h.controller.changeThinkingLevel('high'); });
  assert.equal(h.controller.session?.thinkingLevel, 'high');
  assert.equal(requests.at(-1)?.expectedRevision, 3);
  assert.equal(requests.at(-1)?.target.model.model, 'C', 'thinking changes preserve model identity');
  failSave = true;
  await act(async () => { await h.controller.changeThinkingLevel('low'); });
  assert.equal(h.controller.session?.thinkingLevel, 'high', 'failed writes retain the saved level');
  assert.equal(h.controller.error, 'configuration failed');
  assert.equal(h.controller.configuringModel, false);
  failSave = false;
  await act(async () => { await h.controller.changeThinkingLevel(undefined); });
  assert.equal(requests.at(-1)?.target.thinkingLevel, undefined, 'default explicitly clears the stored override');
  assert.equal(h.controller.session?.thinkingLevel, undefined);
  await act(async () => { await h.controller.changeThinkingLevel('high'); });
  await act(async () => { await h.controller.changeModel({ llmConnectionId: 'connection', llmConnectionSlug: 'provider', model: 'D' }); });
  assert.equal(h.controller.session?.thinkingLevel, undefined, 'changing models clears the old model level');
  const count = requests.length;
  await act(async () => { h.admit('busy-turn'); });
  await act(async () => { await h.controller.changeThinkingLevel('high'); });
  assert.equal(requests.length, count, 'running turns cannot change their thinking level');
});

test('WorkHub stops presenting execution on observation loss while retaining the Stop target', async () => {
  const h = await mountController();
  await act(async () => { h.admit('running-turn'); });
  assert.equal(h.controller.activeTurn?.turnId, 'running-turn');
  await act(async () => { h.loseObservation(); });
  assert.equal(h.controller.activeTurn, undefined);
  assert.equal(h.controller.busy, true);
  await act(async () => { await h.controller.stop(); });
  assert.deepEqual(h.interrupts, [{ sessionId: h.sessionId, turnId: 'running-turn', runId: 'run:running-turn' }]);
});


test('Host receipts replace optimistic identities without duplicating already-published messages', async () => {
  for (const publicationFirst of [false, true]) {
    const h = await mountController();
    let sending!: Promise<boolean>;
    await act(async () => { sending = h.controller.send('original payload', []); });
    const input = h.requests[0]!;
    assert.equal(h.controller.liveTurn?.turnId, input.operationId);
    assert.equal(h.controller.transientMessages[0]?.id, input.operationId);
    assert.equal(h.controller.pendingTurnId, undefined, 'a local operation is not a Host Turn');
    const turnId = 'host-assigned';
    const publish = () => h.publish([{ type: 'user', id: 'user:' + turnId, turnId, text: input.text, ts: 1 }]);
    if (publicationFirst) await act(publish);
    await act(async () => { h.admit(turnId); h.admission.resolve({ turnId }); await sending; });
    assert.equal(h.controller.pendingTurnId, turnId);
    assert.equal(h.controller.liveTurn?.turnId, turnId);
    if (!publicationFirst) {
      assert.equal(h.controller.transientMessages[0]?.id, 'user:' + turnId);
      await act(publish);
    }
    assert.deepEqual(h.controller.transientMessages, []);
    cleanupFakeDom();
  }
});

test('uncertain admission keeps its operation across replacement and Stop waits for the exact receipt', async () => {
  const continuation: WorkHubContinuation = {};
  const first = await mountController(false, {}, continuation);
  let sending!: Promise<boolean>;
  await act(async () => { sending = first.controller.send('original payload', []); });
  const original = first.requests[0]!;
  await act(async () => { await first.controller.stop(); });
  assert.deepEqual(first.interrupts, []);
  assert.deepEqual(first.cancellations, [], 'there is no accepted operation to control yet');
  cleanupFakeDom();
  const next = await mountController(false, {}, continuation);
  await act(async () => { next.reconnect(); });
  assert.deepEqual(next.requests, [original], 'an absent receipt retries the same operation, including after restart');
  await act(async () => {
    first.admission.resolve({ turnId: 'host-assigned' });
    await sending;
  });
  assert.equal(continuation.answer?.stop, true, 'a retired Client cannot clear its successor intent');
  await act(async () => {
    next.admit('unrelated');
    next.admission.resolve({ turnId: 'host-assigned' });
  });
  assert.deepEqual(next.cancellations, [original.operationId]);
  assert.deepEqual(next.interrupts, [], 'Stop cannot transfer to whichever Turn is now visible');
  assert.equal(continuation.answer, undefined);
  assert.equal(next.controller.stopPending, false);
});

test('rejected input keeps retry identity, while lost replies recover without another user submission', async () => {
  const h = await mountController();
  let sending!: Promise<boolean>;
  await act(async () => { sending = h.controller.send('original', []); });
  const original = h.requests[0]!;
  await act(async () => { h.admission.reject(new Error('rejected')); assert.equal(await sending, false); });
  assert.equal(h.controller.busy, false);
  assert.equal(h.controller.turnStates[original.operationId], 'failed');
  h.resetAdmission();
  await act(async () => { sending = h.controller.send('original', []); });
  assert.deepEqual(h.requests.at(-1), original);
  await act(async () => {
    h.admission.reject(new RuntimeHostRequestInterruptedError('plugin.remote', 'command', 'dispatched', 'connection_lost'));
    assert.equal(await sending, true);
  });
  assert.equal(h.controller.busy, true);
  assert.equal(await h.controller.send('different', []), false, 'unknown acceptance blocks a new submission');
  const accepted = { invocation: { session_id: 'workhub:coordination', turn_id: 'canonical', run_id: 'run', invocation_id: 'inv' }, messageId: 'user:canonical', contentDigest: 'digest' };
  h.accepted.set(original.operationId, accepted);
  const submissions = h.submissions;
  await act(async () => { h.complete('canonical'); h.reconnect(); });
  assert.equal(h.submissions, submissions);
  assert.equal(h.controller.busy, false);
  assert.equal(h.controller.pendingTurnId, 'canonical');
  assert.deepEqual(h.cancellations, []);
});

test('WorkHub steering keeps the current Turn and Stop authority and reconciles only its own durable message', async () => {
  const h = await mountController();
  let sent!: Promise<boolean>;
  await act(async () => { sent = h.controller.send('original request', []); });
  const turnId = 'host-assigned';
  await act(async () => h.admit(turnId));
  await act(async () => { h.admission.resolve({ turnId }); await sent; });
  const attachments: AttachmentRef[] = [{ kind: 'doc', name: 'brief.txt', mimeType: 'text/plain', bytes: 4, ref: { kind: 'workspace_file', relativePath: 'brief.txt' } }];
  await act(async () => { assert.equal(await h.controller.send('change direction', attachments, 'steer'), true); });
  assert.equal(h.requests.length, 1, 'steering must not start or queue another answer');
  assert.equal(h.controller.liveTurn?.turnId, turnId);
  assert.deepEqual(h.steers[0]!.slice(2), ['change direction', attachments, 'current_turn', turnId]);
  const messageId = h.steers[0]![1];
  const original: StoredMessage = { type: 'user', id: 'user:' + turnId, turnId, text: 'original request', ts: 1 };
  await act(() => h.publish([original]));
  assert.deepEqual(h.controller.transientMessages.map((message) => message.id), [messageId],
    'an admitted response and unrelated transcript evidence must keep the submission visible');
  await act(() => h.emit({ type: 'steering_message', id: 'steer-observation', turnId, messageId, ts: 2, content: { text: 'change direction', attachments } }));
  assert.deepEqual(h.controller.transientMessages, [], 'live steering must not duplicate its admission placeholder while the transcript lags');
  await act(() => h.publish([original, { type: 'user', id: messageId, turnId, text: 'change direction', attachments, ts: 2 }]));
  assert.deepEqual(h.controller.transientMessages, []);
  await act(async () => { await h.controller.stop(); });
  assert.equal(h.interrupts[0]?.turnId, turnId);
});

test('uncertain steering retains its identity across Turn completion and rejection preserves the active Turn', async () => {
  const h = await mountController();
  await act(() => { h.admit('active-turn'); h.emit({ type: 'text_delta', id: 'live', turnId: 'active-turn', messageId: 'answer', ts: 1, text: 'Working' }); });
  h.setSteerResult('rejected');
  await act(async () => { assert.equal(await h.controller.send('change direction', [], 'steer'), false); });
  assert.equal(h.controller.liveTurn?.turnId, 'active-turn');
  assert.equal(h.controller.busy, true);
  assert.equal(h.controller.transientMessages.length, 0);
  h.setSteerResult('unknown');
  await act(async () => { assert.equal(await h.controller.send('change direction', [], 'steer'), false); });
  const messageId = h.steers[1]![1];
  await act(async () => { assert.equal(await h.controller.send('change direction', []), false); });
  assert.match(h.controller.error!, /Cmd\/Ctrl\+Enter/);
  assert.equal(h.steers.length, 2, 'Enter cannot silently replay uncertain steering or duplicate it');
  for (const [text, attachments] of [
    ['edited direction', []],
    ['change direction', [{ kind: 'doc', name: 'new.txt', mimeType: 'text/plain', bytes: 1,
      ref: { kind: 'workspace_file', relativePath: 'new.txt' } }]],
  ] as [string, AttachmentRef[]][]) {
    await act(async () => { assert.equal(await h.controller.send(text, attachments, 'steer'), false); });
  }
  assert.equal(h.steers.length, 2, 'edited text or attachments cannot overwrite an unknown attempt');
  assert.deepEqual(h.controller.transientMessages.map((message) => message.id), [messageId]);
  await act(() => { h.complete('active-turn'); h.emit({ type: 'complete', id: 'done', turnId: 'active-turn', ts: 2, stopReason: 'end_turn' }); });
  h.setSteerResult('admitted');
  await act(async () => { assert.equal(await h.controller.send('change direction', [], 'steer'), true); });
  assert.equal(h.steers[2]![1], messageId);
  assert.equal(h.requests.length, 0, 'retry must recover the steering receipt even after its Turn ends');
});


test('Host retraction resolves an uncertain WorkHub attempt before the next draft is sent', async () => {
  const h = await mountController();
  await act(() => { h.admit('active-turn'); h.emit({ type: 'text_delta', id: 'live', turnId: 'active-turn', messageId: 'answer', ts: 1, text: 'Working' }); });
  h.setSteerResult('unknown');
  await act(async () => { assert.equal(await h.controller.send('old direction', [], 'steer'), false); });
  const messageId = h.steers[0]![1];
  await act(() => h.emit({ type: 'message_admission', id: 'retracted', turnId: 'active-turn', messageId, ts: 2, outcome: 'retracted' }));
  assert.equal(h.controller.transientMessages.length, 0);
  h.setSteerResult('admitted');
  await act(async () => { assert.equal(await h.controller.send('new direction', [], 'steer'), true); });
  assert.notEqual(h.steers[1]![1], messageId);
});

test('steering observed before its admission response renders once and outranks an uncertain receipt', async () => {
  const h = await mountController();
  await act(() => { h.admit('active-turn'); h.emit({ type: 'text_delta', id: 'live', turnId: 'active-turn', messageId: 'answer', ts: 1, text: 'Working' }); });
  h.setSteerResult('unknown');
  h.onSteer(([, messageId, text]) => h.emit({ type: 'steering_message', id: 'consumed', turnId: 'active-turn', messageId, ts: 2, content: { text } }));
  await act(async () => { assert.equal(await h.controller.send('change direction', [], 'steer'), true); });
  assert.deepEqual(h.controller.transientMessages, []);
  assert.equal(h.controller.error, undefined);
  assert.equal(h.controller.liveTurn?.turnId, 'active-turn');
});


test('WorkHub Host queue owns restored, consumed and retracted rows without transient mirrors', async () => {
  const h = await mountController();
  await act(() => { h.admit('active-turn'); h.emit({ type: 'text_delta', id: 'live', turnId: 'active-turn', messageId: 'answer', ts: 1, text: 'Working' }); });
  const entry = { entryId: 'queued', messageId: 'queued', placement: 'current_turn' as const, state: 'queued' as const, content: { text: 'change direction' } };
  const project = (state: 'queued' | 'in_flight') => h.emit({
    type: 'queue_update', id: 'snapshot', turnId: 'active-turn', ts: 2,
    steering: [entry.content.text], followup: [], steeringEntries: [{ ...entry, state }],
  });
  await act(() => project('queued'));
  assert.deepEqual(h.controller.messageQueue.entries, [entry]);
  assert.deepEqual(h.controller.transientMessages, []);
  await act(() => h.emit({ type: 'steering_message', id: 'consumed', turnId: 'active-turn', ts: 3, messageId: entry.messageId, content: entry.content }));
  await act(() => project('in_flight'));
  assert.deepEqual(h.controller.messageQueue.entries, []);
  assert.deepEqual(h.controller.transientMessages, [], 'an in-flight snapshot cannot resurrect consumed steering');
  await act(() => project('queued'));
  await act(async () => { await h.controller.deleteQueuedEntry(entry.entryId); });
  await act(() => h.emit({ type: 'queue_update', id: 'removed', turnId: 'active-turn', ts: 4, steering: [], followup: [], steeringEntries: [] }));
  assert.deepEqual(h.controller.messageQueue.entries, []);
  assert.deepEqual(h.controller.transientMessages, [], 'withdrawal needs no separate admission event');
});

test('WorkHub sends queue edits, withdrawal and both queue orders to the Host and waits for its projection', async () => {
  const h = await mountController();
  const entries = ['first', 'second'].map((id) => ({ entryId: id, messageId: id,
    content: { text: id }, placement: 'current_turn' as const, state: 'queued' as const }));
  await act(() => h.emit({ type: 'queue_update', id: 'queued', turnId: 'active-turn', ts: 2,
    queueRevision: 7, steering: ['first', 'second'], followup: [], steeringEntries: entries }));
  await act(async () => {
    await h.controller.updateQueuedEntry('second', 7, 'edited second');
    await h.controller.reorderQueuedEntries(['second', 'first']);
    await h.controller.deleteQueuedEntry('first');
  });
  assert.deepEqual(h.queueMutations, [
    ['update', h.controller.sessionId, 'second', 7, 'edited second'],
    ['reorder', h.controller.sessionId, ['second', 'first']],
    ['retract', h.controller.sessionId, 'first'],
  ]);
  assert.deepEqual(h.controller.messageQueue.entries.map((entry) => entry.entryId), ['first', 'second']);
  await act(() => h.emit({ type: 'queue_update', id: 'updated', turnId: 'active-turn', ts: 3,
    queueRevision: 10, steering: ['edited second'], followup: [], steeringEntries: [{ ...entries[1]!, content: { text: 'edited second' } }] }));
  assert.deepEqual(h.controller.messageQueue.entries.map((entry) => entry.content.text), ['edited second']);
  assert.deepEqual(h.controller.transientMessages, []);
});


test('WorkHub defaults to follow-up and moves each message into its admitted successor Turn', async () => {
  const h = await mountController();
  await act(() => { h.admit('active-turn'); h.emit({ type: 'text_delta', id: 'live', turnId: 'active-turn', messageId: 'answer', ts: 1, text: 'Working' }); });
  const attachments: AttachmentRef[] = [{ kind: 'doc', name: 'brief.txt', mimeType: 'text/plain', bytes: 4, ref: { kind: 'workspace_file', relativePath: 'brief.txt' } }];
  await act(async () => {
    assert.equal(await h.controller.send('first follow-up', attachments), true);
    assert.equal(await h.controller.send('second follow-up', []), true);
  });
  assert.deepEqual(h.steers.map((input) => input.slice(2)), [
    ['first follow-up', attachments, 'next_turn', 'active-turn'], ['second follow-up', [], 'next_turn', 'active-turn'],
  ]);
  assert.equal(h.requests.length, 0);
  assert.equal(h.controller.liveTurn?.turnId, 'active-turn');
  const first = h.steers[0]![1];
  const second = h.steers[1]![1];
  assert.deepEqual(h.controller.transientMessages.map((message) => message.id), [first, second]);
  await act(async () => h.reconnect());
  assert.deepEqual(h.controller.transientMessages.map((message) => message.id), [first, second],
    'disconnecting before canonical evidence cannot hide accepted messages');
  const entries = h.steers.map(([, messageId, text, attachments]) => ({
    entryId: messageId, messageId, content: { text, attachments }, placement: 'next_turn' as const, state: 'queued' as const,
  }));
  await act(() => h.emit({ type: 'queue_update', id: 'queued', turnId: 'active-turn', ts: 2,
    steering: [], followup: entries.map((entry) => entry.content.text), followupEntries: entries }));
  await act(() => h.emit({ type: 'message_admission', id: 'first-admission', turnId: 'successor', messageId: first, ts: 2, outcome: 'admitted' }));
  assert.deepEqual(h.controller.messageQueue.entries.map((entry) => entry.messageId), [second]);
  await act(() => {
    h.admit('successor');
    h.emit({ type: 'text_delta', id: 'successor-output', turnId: 'successor', messageId: 'successor-answer', ts: 3, text: 'Responding to first follow-up' });
  });
  assert.equal(h.controller.liveTurn?.steps[0]?.text?.text, 'Responding to first follow-up');
  assert.deepEqual(h.controller.transientMessages.map(({ id, text, attachments, hostTurnId, transientPlacement, pendingSteering }) =>
    ({ id, text, attachments, hostTurnId, transientPlacement, pendingSteering })), [{
    id: first, text: 'first follow-up', attachments, hostTurnId: 'successor', transientPlacement: 'current_turn', pendingSteering: false,
  }], 'the admitted prompt must accompany its live answer before transcript publication');
  await act(() => h.emit({ type: 'queue_update', id: 'remaining', turnId: 'successor', ts: 3,
    steering: [], followup: ['second follow-up'], followupEntries: entries.slice(1) }));
  assert.equal(h.controller.transientMessages[0]?.id, first, 'later queue snapshots cannot retire an admitted prompt');
  await act(() => h.publish([{ type: 'user', id: first, turnId: 'successor', text: 'first follow-up', attachments, ts: 2 }]));
  assert.deepEqual(h.controller.transientMessages, []);
  assert.deepEqual(h.controller.messageQueue.entries.map((entry) => entry.messageId), [second]);
});

test('restored follow-ups transfer edited Host content once, regardless of transcript arrival order', async () => {
  for (const transcriptFirst of [false, true]) {
    const h = await mountController();
    const attachments: AttachmentRef[] = [{ kind: 'doc', name: 'brief.txt', mimeType: 'text/plain', bytes: 4, ref: { kind: 'workspace_file', relativePath: 'brief.txt' } }];
    const entry = { entryId: 'restored-entry', messageId: 'restored-message', placement: 'next_turn' as const, state: 'queued' as const,
      content: { text: 'model-facing envelope', displayText: 'edited follow-up', attachments } };
    await act(() => {
      h.emit({ type: 'queue_update', id: 'restored', turnId: 'predecessor', ts: 1, steering: [], followup: ['old follow-up'],
        followupEntries: [{ ...entry, content: { text: 'old follow-up' } }] });
      h.emit({ type: 'queue_update', id: 'edited', turnId: 'predecessor', ts: 2, steering: [], followup: [entry.content.text], followupEntries: [entry] });
    });
    const publish = () => h.publish([{ type: 'user', id: entry.messageId, turnId: 'successor', ts: 3, ...entry.content }]);
    const admit = () => h.emit({ type: 'message_admission', id: 'admitted', turnId: 'successor', messageId: entry.messageId, ts: 3, outcome: 'admitted' });
    if (transcriptFirst) await act(publish);
    await act(() => { admit(); admit(); });
    assert.deepEqual(h.controller.messageQueue.entries, []);
    assert.deepEqual(h.controller.transientMessages.map(({ id, text, attachments, hostTurnId }) => ({ id, text, attachments, hostTurnId })),
      transcriptFirst ? [] : [{ id: entry.messageId, text: 'edited follow-up', attachments, hostTurnId: 'successor' }]);
    if (!transcriptFirst) await act(publish);
    await act(admit);
    assert.deepEqual(h.controller.transientMessages, [], 'late admission cannot recreate a published user row');
    cleanupFakeDom();
  }
});

test('withdrawing a queued follow-up never transfers it into the conversation', async () => {
  const h = await mountController();
  const entry = { entryId: 'queued-entry', messageId: 'queued-message', placement: 'next_turn' as const, state: 'queued' as const, content: { text: 'withdraw me' } };
  await act(() => h.emit({ type: 'queue_update', id: 'queued', turnId: 'active', ts: 1, steering: [], followup: ['withdraw me'], followupEntries: [entry] }));
  await act(() => h.emit({ type: 'message_admission', id: 'retracted', turnId: 'active', messageId: entry.messageId, ts: 2, outcome: 'retracted' }));
  assert.deepEqual(h.controller.messageQueue.entries, []);
  assert.deepEqual(h.controller.transientMessages, []);
});

test('an uncertain queued follow-up blocks the next one until its row is observed', async () => {
  const h = await mountController();
  await act(() => { h.admit('active-turn'); h.emit({ type: 'text_delta', id: 'live', turnId: 'active-turn', messageId: 'answer', ts: 1, text: 'Working' }); });
  h.setSteerResult('unknown');
  await act(async () => { assert.equal(await h.controller.send('queued while parked in history', []), false); });
  const messageId = h.steers[0]![1];  await act(async () => { assert.equal(await h.controller.send('a different follow-up', []), false); });
  assert.equal(h.steers.length, 1, 'an unobserved attempt refuses the next follow-up');
  await act(() => h.publish([{ type: 'user', id: messageId, turnId: 'successor', text: 'queued while parked in history', ts: 2 }]));
  h.setSteerResult('admitted');
  await act(async () => { assert.equal(await h.controller.send('a different follow-up', []), true); });
  assert.equal(h.steers.length, 2, 'the observed row releases the guard');
});

test('loading earlier history reaches WorkHub’s transcript', async () => {
  const h = await mountController();
  await h.controller.loadEarlier();
  assert.equal(h.earlierLoads, 1);
});

test('follow-up admission before an uncertain response keeps its successor placement', async () => {
  const h = await mountController();
  await act(() => { h.admit('active-turn'); h.emit({ type: 'text_delta', id: 'live', turnId: 'active-turn', messageId: 'answer', ts: 1, text: 'Working' }); });
  h.setSteerResult('unknown');
  h.onSteer(([, messageId]) => h.emit({ type: 'message_admission', id: 'admitted', turnId: 'successor', messageId, ts: 2, outcome: 'admitted' }));
  await act(async () => { assert.equal(await h.controller.send('next request', []), true); });
  assert.equal(h.controller.transientMessages[0]?.transientPlacement, 'current_turn');
  assert.equal(h.controller.transientMessages[0]?.hostTurnId, 'successor');
  assert.equal(h.controller.error, undefined);
});


test('Stop retires only Host-confirmed queued messages even without retraction events', async () => {
  for (const origin of ['local', 'restored'] as const) {
    const h = await mountController();
    let turnId = 'active-turn';
    if (origin === 'local') {
      let sent!: Promise<boolean>;
      await act(async () => { sent = h.controller.send('original request', []); });
      turnId = 'host-assigned';
      await act(async () => h.admit(turnId));
      await act(async () => { h.admission.resolve({ turnId }); await sent; });
    } else {
      await act(async () => h.admit(turnId));
      await act(() => h.emit({ type: 'text_delta', id: 'live', turnId, messageId: 'answer', ts: 1, text: 'Working' }));
    }
    const entries = ['steering', 'followup', 'retained'].map((messageId) => ({
      messageId, entryId: messageId, content: { text: messageId }, state: 'queued' as const,
      placement: messageId === 'steering' ? 'current_turn' as const : 'next_turn' as const,
    }));
    await act(() => h.emit({ type: 'queue_update', id: 'queued', turnId, ts: 2,
      steering: ['steering'], followup: ['followup', 'retained'],
      steeringEntries: entries.slice(0, 1), followupEntries: entries.slice(1),
    }));
    h.setStopRetractions(['steering', 'followup']);
    await act(async () => { await h.controller.stop(); });
    assert.deepEqual(h.controller.messageQueue.entries.map((entry) => entry.messageId), ['retained']);
    assert.deepEqual(h.controller.transientMessages.filter((message) => message.id !== 'user:' + turnId), []);
    assert.equal(h.controller.stopPending, false);
    cleanupFakeDom();
  }
});
