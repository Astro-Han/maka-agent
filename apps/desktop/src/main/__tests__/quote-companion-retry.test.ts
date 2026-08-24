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
import { parseHTML } from 'linkedom';
import { act, createElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import type { SessionEvent } from '@maka/core/events';
import type { SessionChangedEvent, SessionSummary, TurnRecord } from '@maka/core/session';
import {
  createFakeWorkbarServices,
  useQuoteCompanion,
  WorkbarServicesProvider,
  type WorkbarServices,
} from '../../renderer/features/workbar/testing.js';

const originalGlobals = {
  document: globalThis.document,
  window: globalThis.window,
  HTMLElement: globalThis.HTMLElement,
  HTMLIFrameElement: globalThis.HTMLIFrameElement,
  Event: globalThis.Event,
  Node: globalThis.Node,
  IS_REACT_ACT_ENVIRONMENT: (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean })
    .IS_REACT_ACT_ENVIRONMENT,
};

let mountedRoot: Root | undefined;
const SOURCE_SESSION = session('source-session');

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((settle) => {
    resolve = settle;
  });
  return { promise, resolve };
}

afterEach(async () => {
  if (mountedRoot) {
    await act(async () => {
      mountedRoot?.unmount();
      await Promise.resolve();
    });
  }
  mountedRoot = undefined;
  Object.assign(globalThis, originalGlobals);
});

test('retries a busy Side Conversation at the newest settled boundary and clears its banner', async () => {
  const parsed = parseHTML('<html><body><div id="root"></div></body></html>');
  const { document, window } = parsed;
  Object.assign(globalThis, {
    document,
    window,
    HTMLElement: window.HTMLElement,
    HTMLIFrameElement: window.HTMLIFrameElement ?? class HTMLIFrameElement {},
    Event: window.Event,
    Node: window.Node,
    IS_REACT_ACT_ENVIRONMENT: true,
  });

  let listCount = 0;
  let sessionChange: ((event: SessionChangedEvent) => void) | undefined;
  let releaseRetry: (() => void) | undefined;
  const branchInputs: Array<{ sourceTurnId: string; copyId: string }> = [];
  const defaults = createFakeWorkbarServices();
  const services: WorkbarServices = {
    ...defaults,
    sideChat: {
      ...defaults.sideChat,
      listTurns: async () => {
        listCount += 1;
        return listCount === 1
          ? [settledTurn('turn-before-busy')]
          : [settledTurn('turn-before-busy'), settledTurn('turn-after-busy')];
      },
      branchFromTurn: async (_sessionId, input) => {
        branchInputs.push({ sourceTurnId: input.sourceTurnId, copyId: input.copyId });
        if (branchInputs.length === 1) {
          return { ok: false as const, reason: 'session_busy' as const };
        }
        await new Promise<void>((resolve) => {
          releaseRetry = resolve;
        });
        return { ok: true as const, session: session('side-conversation') };
      },
      subscribeSessionChanges: (handler) => {
        sessionChange = handler;
        return () => {
          if (sessionChange === handler) sessionChange = undefined;
        };
      },
    },
  };
  const container = document.querySelector('#root');
  assert.ok(container);
  const root = createRoot(container);
  mountedRoot = root;

  await act(async () => {
    root.render(
      createElement(WorkbarServicesProvider, {
        services,
        children: createElement(QuoteCompanionProbe),
      }),
    );
    await Promise.resolve();
  });
  await waitUntil(() => branchInputs.length === 1 && sessionChange !== undefined);
  assert.match(container.textContent, /main conversation or a linked task is still running/i);
  const probe = container.firstElementChild;
  assert.ok(probe);

  await act(async () => {
    sessionChange?.({
      reason: 'turn-status-change',
      sessionId: 'source-session',
      turnId: 'turn-after-busy',
      ts: Date.now(),
    });
    await Promise.resolve();
  });
  await waitUntil(() => branchInputs.length === 2 && releaseRetry !== undefined);
  assert.equal(probe.getAttribute('data-preparing'), 'false');
  assert.match(container.textContent, /main conversation or a linked task is still running/i);

  await act(async () => {
    releaseRetry?.();
    await Promise.resolve();
  });
  await waitUntil(
    () => probe.getAttribute('data-companion-id') === 'side-conversation',
    () =>
      `branch inputs: ${JSON.stringify(branchInputs)}; companion: ${probe.getAttribute('data-companion-id')}; error: ${probe.getAttribute('data-error')}`,
  );

  assert.deepEqual(
    branchInputs.map(({ sourceTurnId }) => sourceTurnId),
    ['turn-before-busy', 'turn-after-busy'],
  );
  assert.notEqual(branchInputs[0]?.copyId, branchInputs[1]?.copyId);
  assert.equal(probe.getAttribute('data-error'), '');
});

test('does not restart foreground setup when the source Session object refreshes', async () => {
  const parsed = parseHTML('<html><body><div id="root"></div></body></html>');
  const { document, window } = parsed;
  Object.assign(globalThis, {
    document,
    window,
    HTMLElement: window.HTMLElement,
    HTMLIFrameElement: window.HTMLIFrameElement ?? class HTMLIFrameElement {},
    Event: window.Event,
    Node: window.Node,
    IS_REACT_ACT_ENVIRONMENT: true,
  });

  let branchCount = 0;
  const defaults = createFakeWorkbarServices();
  const services: WorkbarServices = {
    ...defaults,
    sideChat: {
      ...defaults.sideChat,
      listTurns: async () => [settledTurn('settled-turn')],
      branchFromTurn: async () => {
        branchCount += 1;
        if (branchCount === 1) {
          return { ok: false as const, reason: 'session_busy' as const };
        }
        return await new Promise<never>(() => undefined);
      },
    },
  };
  const container = document.querySelector('#root');
  assert.ok(container);
  const root = createRoot(container);
  mountedRoot = root;

  const render = (sourceSession: SessionSummary) =>
    root.render(
      createElement(WorkbarServicesProvider, {
        services,
        children: createElement(QuoteCompanionProbe, { sourceSession }),
      }),
    );

  await act(async () => {
    render(session('source-session'));
    await Promise.resolve();
  });
  const probe = container.firstElementChild;
  assert.ok(probe);
  await waitUntil(
    () => branchCount === 1 && probe.getAttribute('data-preparing') === 'false',
  );

  await act(async () => {
    render(session('source-session'));
    await Promise.resolve();
  });

  assert.equal(branchCount, 1);
  assert.equal(probe.getAttribute('data-preparing'), 'false');
});

test('keeps Side Conversation events owned by the Host-admitted turn across an admission race', async () => {
  const parsed = parseHTML('<html><body><div id="root"></div></body></html>');
  const { document, window } = parsed;
  Object.assign(globalThis, {
    document,
    window,
    HTMLElement: window.HTMLElement,
    HTMLIFrameElement: window.HTMLIFrameElement ?? class HTMLIFrameElement {},
    Event: window.Event,
    Node: window.Node,
    IS_REACT_ACT_ENVIRONMENT: true,
  });

  let eventHandler: ((event: SessionEvent) => void) | undefined;
  let send: ((text: string) => Promise<boolean>) | undefined;
  const pendingSend = deferred<{ ok: true; turnId: string }>();
  const defaults = createFakeWorkbarServices();
  const services: WorkbarServices = {
    ...defaults,
    sideChat: {
      ...defaults.sideChat,
      listTurns: async () => [settledTurn('source-turn')],
      branchFromTurn: async () => ({ ok: true as const, session: session('side-conversation') }),
      subscribeEvents: (_sessionId, handler, onSeeded) => {
        eventHandler = handler;
        onSeeded?.();
        return () => undefined;
      },
      send: async () => pendingSend.promise,
    },
  };
  const container = document.querySelector('#root');
  assert.ok(container);
  const root = createRoot(container);
  mountedRoot = root;

  await act(async () => {
    root.render(
      createElement(WorkbarServicesProvider, {
        services,
        children: createElement(QuoteCompanionOwnershipProbe, {
          onSend: (value) => {
            send = value;
          },
        }),
      }),
    );
    await Promise.resolve();
  });
  await waitUntil(() => container.firstElementChild?.getAttribute('data-companion-id') === 'side-conversation');
  assert.ok(send);
  assert.ok(eventHandler);

  let sendResult: Promise<boolean> | undefined;
  await act(async () => {
    sendResult = send?.('new prompt');
    await Promise.resolve();
  });

  await act(async () => {
    eventHandler?.({
      type: 'complete',
      id: 'late-old-terminal',
      turnId: 'old-turn',
      ts: 1,
      stopReason: 'end_turn',
    });
    await Promise.resolve();
  });
  assert.equal(container.firstElementChild?.getAttribute('data-processing'), 'true');

  await act(async () => {
    eventHandler?.({
      type: 'text_delta',
      id: 'new-text-before-response',
      messageId: 'assistant-message',
      turnId: 'host-admitted-turn',
      ts: 2,
      text: 'answer',
    });
    await Promise.resolve();
  });
  assert.equal(container.firstElementChild?.getAttribute('data-processing'), 'true');

  await act(async () => {
    pendingSend.resolve({ ok: true, turnId: 'host-admitted-turn' });
    assert.equal(await sendResult, true);
    await Promise.resolve();
  });

  const probe = container.firstElementChild;
  assert.ok(probe);
  assert.equal(probe.getAttribute('data-live-turn-id'), 'host-admitted-turn');
  assert.equal(probe.getAttribute('data-live-text'), 'answer');
  assert.equal(probe.getAttribute('data-streaming'), 'true');
  assert.equal(probe.getAttribute('data-processing'), 'false');
});

test('binds a busy-raced Side Conversation send through its Host-admitted message identity', async () => {
  const parsed = parseHTML('<html><body><div id="root"></div></body></html>');
  const { document, window } = parsed;
  Object.assign(globalThis, {
    document,
    window,
    HTMLElement: window.HTMLElement,
    HTMLIFrameElement: window.HTMLIFrameElement ?? class HTMLIFrameElement {},
    Event: window.Event,
    Node: window.Node,
    IS_REACT_ACT_ENVIRONMENT: true,
  });

  let eventHandler: ((event: SessionEvent) => void) | undefined;
  let send: ((text: string) => Promise<boolean>) | undefined;
  const pendingSend = deferred<{ ok: true; steered: true; turnId: string; messageId: string }>();
  const defaults = createFakeWorkbarServices();
  const services: WorkbarServices = {
    ...defaults,
    sideChat: {
      ...defaults.sideChat,
      listTurns: async () => [settledTurn('source-turn')],
      branchFromTurn: async () => ({ ok: true as const, session: session('side-conversation') }),
      subscribeEvents: (_sessionId, handler, onSeeded) => {
        eventHandler = handler;
        onSeeded?.();
        return () => undefined;
      },
      send: async () => pendingSend.promise,
    },
  };
  const container = document.querySelector('#root');
  assert.ok(container);
  const root = createRoot(container);
  mountedRoot = root;

  await act(async () => {
    root.render(
      createElement(WorkbarServicesProvider, {
        services,
        children: createElement(QuoteCompanionOwnershipProbe, {
          onSend: (value) => {
            send = value;
          },
        }),
      }),
    );
    await Promise.resolve();
  });
  await waitUntil(() => container.firstElementChild?.getAttribute('data-companion-id') === 'side-conversation');
  assert.ok(send);
  assert.ok(eventHandler);

  let sendResult: Promise<boolean> | undefined;
  await act(async () => {
    sendResult = send?.('steer the active turn');
    await Promise.resolve();
  });
  await act(async () => {
    eventHandler?.({
      type: 'complete',
      id: 'late-old-terminal',
      turnId: 'old-turn',
      ts: 1,
      stopReason: 'end_turn',
    });
    eventHandler?.({
      type: 'queue_update',
      id: 'accepted-queue',
      turnId: 'host-active-turn',
      ts: 2,
      queueRevision: 1,
      steering: ['steer the active turn'],
      followup: [],
      steeringEntries: [
        {
          entryId: 'accepted-entry',
          messageId: 'accepted-message',
          content: { text: 'steer the active turn' },
          placement: 'current_turn',
          state: 'queued',
        },
      ],
      followupEntries: [],
    });
    await Promise.resolve();
  });
  assert.equal(container.firstElementChild?.getAttribute('data-processing'), 'true');
  assert.notEqual(
    container.firstElementChild?.getAttribute('data-live-turn-id'),
    'host-active-turn',
  );

  await act(async () => {
    pendingSend.resolve({
      ok: true,
      steered: true,
      turnId: 'requested-turn-is-not-the-owner',
      messageId: 'accepted-message',
    });
    assert.equal(await sendResult, true);
    await Promise.resolve();
  });
  assert.notEqual(
    container.firstElementChild?.getAttribute('data-live-turn-id'),
    'host-active-turn',
  );
  await act(async () => {
    eventHandler?.({
      type: 'steering_message',
      id: 'accepted-steering-message',
      messageId: 'accepted-message',
      turnId: 'host-active-turn',
      ts: 2.5,
      content: { text: 'steer the active turn' },
    });
    eventHandler?.({
      type: 'text_delta',
      id: 'accepted-text',
      messageId: 'assistant-message',
      turnId: 'host-active-turn',
      ts: 3,
      text: 'answer after steering',
    });
    await Promise.resolve();
  });

  const probe = container.firstElementChild;
  assert.ok(probe);
  assert.equal(probe.getAttribute('data-live-turn-id'), 'host-active-turn');
  assert.equal(probe.getAttribute('data-live-text'), 'answer after steering');
  assert.equal(probe.getAttribute('data-streaming'), 'true');
  assert.equal(probe.getAttribute('data-processing'), 'false');
});

test('waits for Side Conversation observation readiness before sending', async () => {
  const parsed = parseHTML('<html><body><div id="root"></div></body></html>');
  const { document, window } = parsed;
  Object.assign(globalThis, {
    document,
    window,
    HTMLElement: window.HTMLElement,
    HTMLIFrameElement: window.HTMLIFrameElement ?? class HTMLIFrameElement {},
    Event: window.Event,
    Node: window.Node,
    IS_REACT_ACT_ENVIRONMENT: true,
  });

  let send: ((text: string) => Promise<boolean>) | undefined;
  let sendCalls = 0;
  let markSeeded: (() => void) | undefined;
  const defaults = createFakeWorkbarServices();
  const services: WorkbarServices = {
    ...defaults,
    sideChat: {
      ...defaults.sideChat,
      listTurns: async () => [settledTurn('source-turn')],
      branchFromTurn: async () => ({ ok: true as const, session: session('side-conversation') }),
      subscribeEvents: (_sessionId, _handler, onSeeded) => {
        markSeeded = onSeeded;
        return () => undefined;
      },
      send: async () => {
        sendCalls += 1;
        return { ok: true as const, turnId: 'seeded-turn' };
      },
    },
  };
  const container = document.querySelector('#root');
  assert.ok(container);
  const root = createRoot(container);
  mountedRoot = root;

  await act(async () => {
    root.render(
      createElement(WorkbarServicesProvider, {
        services,
        children: createElement(QuoteCompanionOwnershipProbe, {
          onSend: (value) => {
            send = value;
          },
        }),
      }),
    );
    await Promise.resolve();
  });
  await waitUntil(() => container.firstElementChild?.getAttribute('data-companion-id') === 'side-conversation');
  assert.ok(send);

  let sendResult: Promise<boolean> | undefined;
  await act(async () => {
    sendResult = send?.('wait for the observer');
    await Promise.resolve();
  });
  assert.equal(sendCalls, 0);

  await act(async () => {
    markSeeded?.();
    await Promise.resolve();
  });
  await waitUntil(() => sendCalls === 1);
  assert.equal(await sendResult, true);
});

test('releases a send waiting for observation when the Side Conversation is disposed', async () => {
  const parsed = parseHTML('<html><body><div id="root"></div></body></html>');
  const { document, window } = parsed;
  Object.assign(globalThis, {
    document,
    window,
    HTMLElement: window.HTMLElement,
    HTMLIFrameElement: window.HTMLIFrameElement ?? class HTMLIFrameElement {},
    Event: window.Event,
    Node: window.Node,
    IS_REACT_ACT_ENVIRONMENT: true,
  });

  let send: ((text: string) => Promise<boolean>) | undefined;
  let sendCalls = 0;
  let unsubscribed = false;
  const defaults = createFakeWorkbarServices();
  const services: WorkbarServices = {
    ...defaults,
    sideChat: {
      ...defaults.sideChat,
      listTurns: async () => [settledTurn('source-turn')],
      branchFromTurn: async () => ({ ok: true as const, session: session('side-conversation') }),
      subscribeEvents: () => () => {
        unsubscribed = true;
      },
      send: async () => {
        sendCalls += 1;
        return { ok: true as const, turnId: 'disposed-turn' };
      },
    },
  };
  const container = document.querySelector('#root');
  assert.ok(container);
  const root = createRoot(container);
  mountedRoot = root;

  await act(async () => {
    root.render(
      createElement(WorkbarServicesProvider, {
        services,
        children: createElement(QuoteCompanionOwnershipProbe, {
          onSend: (value) => {
            send = value;
          },
        }),
      }),
    );
    await Promise.resolve();
  });
  await waitUntil(() => container.firstElementChild?.getAttribute('data-companion-id') === 'side-conversation');
  assert.ok(send);

  let sendResult: Promise<boolean> | undefined;
  await act(async () => {
    sendResult = send?.('dispose while observing');
    await Promise.resolve();
  });
  await act(async () => {
    root.unmount();
    await Promise.resolve();
  });

  assert.equal(await sendResult, false);
  assert.equal(sendCalls, 0);
  assert.equal(unsubscribed, true);
  mountedRoot = undefined;
});

function QuoteCompanionProbe(props: { sourceSession?: SessionSummary }) {
  const companion = useQuoteCompanion({
    panelId: 'retry-panel',
    pendingQuotes: [],
    sourceSession: props.sourceSession ?? SOURCE_SESSION,
    locale: 'en',
    onQuotesConsumed: () => undefined,
  });
  return createElement('div', {
    'data-error': companion.error ?? '',
    'data-companion-id': companion.companionSession?.id ?? '',
    'data-preparing': String(companion.preparing),
  }, companion.error);
}

function QuoteCompanionOwnershipProbe(props: {
  onSend: (send: (text: string) => Promise<boolean>) => void;
}) {
  const companion = useQuoteCompanion({
    panelId: 'ownership-panel',
    pendingQuotes: [],
    sourceSession: SOURCE_SESSION,
    locale: 'en',
    onQuotesConsumed: () => undefined,
  });
  props.onSend(companion.send);
  return createElement('div', {
    'data-companion-id': companion.companionSession?.id ?? '',
    'data-live-turn-id': companion.liveTurn?.turnId ?? '',
    'data-live-text': companion.liveTurn?.steps.find((step) => step.text)?.text?.text ?? '',
    'data-streaming': String(companion.streaming),
    'data-processing': String(companion.processing),
  });
}

function session(id: string): SessionSummary {
  return {
    id,
    name: id,
    isFlagged: false,
    isArchived: false,
    labels: [],
    hasUnread: false,
    status: 'active',
    backend: 'ai-sdk',
    llmConnectionSlug: 'test',
    connectionLocked: false,
    model: 'test-model',
    permissionMode: 'ask',
  };
}

function settledTurn(turnId: string): TurnRecord {
  return { turnId, status: 'completed', partialOutputRetained: false };
}

async function waitUntil(predicate: () => boolean, diagnostics?: () => string): Promise<void> {
  for (let attempt = 0; attempt < 50; attempt += 1) {
    if (predicate()) return;
    await act(async () => {
      await new Promise<void>((resolve) => setImmediate(resolve));
    });
  }
  assert.fail(
    `Timed out waiting for the Side Conversation state${diagnostics ? ` (${diagnostics()})` : ''}`,
  );
}
