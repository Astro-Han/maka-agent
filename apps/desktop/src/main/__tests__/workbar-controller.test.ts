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

import { deferred } from '@maka/core/test-only/async-primitives';
import { strict as assert } from 'node:assert';
import { afterEach, describe, it } from 'node:test';
import { act, createElement, StrictMode, useLayoutEffect } from 'react';
import type { ShellRunUpdate } from '@maka/core/events';
import type { SessionSummary } from '@maka/core/session';
import { LocaleProvider } from '@maka/ui';
import { cleanupFakeDom, installReactRenderer } from './fake-dom.js';
import {
  createFakeWorkbarServices,
  projectWorkbarPanelsForSession,
  useWorkbarController,
  WorkbarServicesProvider,
  type UseWorkbarControllerInput,
  type WorkbarController,
  type WorkbarServices,
} from '../../renderer/features/workbar/testing.js';

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

function shellUpdate(sessionId: string, ref: string): ShellRunUpdate {
  return {
    sessionId,
    ownership: { kind: 'local' },
    sourceTurnId: 'turn',
    sourceToolCallId: 'tool',
    result: { ref },
  } as ShellRunUpdate;
}
let latestController: WorkbarController | undefined;
let controllerRenderSnapshots: Array<{
  activeId: string | undefined;
  terminalOwnerIds: Array<string | undefined>;
}> = [];

type ControllerProbeInput = UseWorkbarControllerInput & { openOnActivation?: boolean };

function ControllerProbe(props: ControllerProbeInput) {
  const workbar = useWorkbarController(props);
  latestController = workbar;
  const visiblePanels = projectWorkbarPanelsForSession(
    workbar.host.panelsState, workbar.host.activeId,
    new Set(workbar.host.quotes?.map((quote) => `side-chat:${quote.id}`)),
  );
  useLayoutEffect(() => {
    if (props.openOnActivation) workbar.host.onOpenLauncher('right');
  }, [props.activeSession?.id, props.openOnActivation]);
  controllerRenderSnapshots.push({
    activeId: latestController.host.activeId,
    terminalOwnerIds: [
      ...visiblePanels.right.tabs,
      ...visiblePanels.bottom.tabs,
    ]
      .filter((tab) => tab.kind === 'terminal')
      .map((tab) => tab.ownerSessionId),
  });
  return null;
}

function renderController(
  root: ReturnType<typeof installReactRenderer>['root'],
  services: WorkbarServices,
  input: ControllerProbeInput,
  strictMode = false,
) {
  const probe = createElement(
    LocaleProvider,
    {
      locale: 'en',
      children: createElement(
        WorkbarServicesProvider,
        { services },
        createElement(ControllerProbe, input),
      ),
    },
  );
  root.render(
    strictMode
      ? createElement(StrictMode, { children: probe })
      : probe,
  );
}

function controller(): WorkbarController {
  assert.ok(latestController);
  return latestController;
}

function input(
  activeSession: SessionSummary | undefined,
  errors: string[] = [],
): UseWorkbarControllerInput {
  return {
    available: true,
    layoutSessionId: activeSession?.id,
    activeSession,
    projectId: activeSession?.projectId,
    projectAliases: [],
    authoritativeSessionIds: new Set(['a', 'b', ...(activeSession ? [activeSession.id] : [])]),
    shellObscured: false,
    modelChoices: [],
    reportError: (title, description) => errors.push(`${title}: ${description}`),
  };
}

describe('useWorkbarController', () => {
  afterEach(() => {
    latestController = undefined;
    controllerRenderSnapshots = [];
    cleanupFakeDom();
    delete (globalThis as { window?: unknown }).window;
  });

  it('preserves an expansion requested before the Host-backed Session arrives', async () => {
    const { root } = installReactRenderer();
    const services = createFakeWorkbarServices();
    await act(async () => renderController(root, services, {
      ...input(undefined), layoutSessionId: 'pending',
    }));
    await act(async () => controller().commands.toggleRight());
    assert.equal(controller().host.rightCollapsed, false);
    assert.equal(controller().host.activeId, undefined);
    await act(async () => renderController(root, services, input(session('pending'))));
    assert.equal(controller().host.activeId, 'pending');
    assert.equal(controller().host.rightCollapsed, false);
  });

  it('keeps right-panel visibility independent across Session navigation', async () => {
    const { root } = installReactRenderer();
    const services = createFakeWorkbarServices();
    const authoritativeSessionIds = new Set(['a', 'b']);
    const show = (id: string | undefined) => renderController(root, services, {
      ...input(id ? session(id) : undefined),
      authoritativeSessionIds,
    });

    await act(async () => show('a'));
    await act(async () => controller().commands.toggleRight());
    assert.equal(controller().host.rightCollapsed, false);
    await act(async () => show(undefined));
    await act(async () => show('b'));
    assert.equal(controller().host.rightCollapsed, true);
    await act(async () => show('a'));
    assert.equal(controller().host.rightCollapsed, false);
  });

  it('keeps an open requested in the activation commit bound to the new Session', async () => {
    const { root } = installReactRenderer();
    const services = createFakeWorkbarServices();
    const authoritativeSessionIds = new Set(['a', 'b']);
    await act(async () => renderController(root, services, {
      ...input(session('a')), authoritativeSessionIds,
    }, true));
    await act(async () => renderController(root, services, {
      ...input(session('b')), authoritativeSessionIds, openOnActivation: true,
    }, true));
    assert.equal(controller().host.rightCollapsed, false);
    await act(async () => renderController(root, services, {
      ...input(session('a')), authoritativeSessionIds,
    }, true));
    assert.equal(controller().host.rightCollapsed, true);
    await act(async () => renderController(root, services, {
      ...input(session('b')), authoritativeSessionIds,
    }, true));
    assert.equal(controller().host.rightCollapsed, false);
  });

  it("preserves visibility while hiding, rather than removing, another Session's Terminal", async () => {
    const { root } = installReactRenderer();
    const defaults = createFakeWorkbarServices();
    const services = createFakeWorkbarServices({
      terminal: {
        ...defaults.terminal,
        start: async (sessionId) => shellUpdate(sessionId, 'terminal-a'),
      },
    });
    const authoritativeSessionIds = new Set(['a', 'b']);
    const show = (id: string) => renderController(root, services, {
      ...input(session(id)),
      authoritativeSessionIds,
    });

    await act(async () => show('b'));
    await act(async () => controller().commands.toggleRight());
    assert.equal(controller().host.rightCollapsed, false);
    await act(async () => show('a'));
    await act(async () => controller().commands.openTool('terminal'));
    assert.equal(controller().host.panelsState.right.tabs.length, 1);
    await act(async () => show('b'));

    assert.equal(controller().host.panelsState.right.tabs.length, 1);
    assert.equal(projectWorkbarPanelsForSession(
      controller().host.panelsState, 'b', new Set(),
    ).right.tabs.length, 0);
    assert.equal(controller().host.rightCollapsed, false);
  });

  it('projects the canonical project and absorbed aliases into the host model', async () => {
    const { root } = installReactRenderer();
    const controllerInput = input(session('a'));
    controllerInput.projectId = 'project-canonical';
    controllerInput.projectAliases = ['project-absorbed'];

    await act(async () =>
      renderController(root, createFakeWorkbarServices(), controllerInput),
    );

    assert.equal(controller().host.projectId, 'project-canonical');
    assert.deepEqual(controller().host.projectAliases, ['project-absorbed']);
  });

  it('routes Client Capability decisions to the active Session', async () => {
    const { root } = installReactRenderer();
    const responses: Array<{ sessionId: string; requestId: string; decision: string }> = [];
    const defaults = createFakeWorkbarServices();
    const services = createFakeWorkbarServices({
      sideChat: {
        ...defaults.sideChat,
        respondToClientCapability: async (sessionId, response) => {
          responses.push({ sessionId, ...response });
        },
      },
    });

    await act(async () => renderController(root, services, input(session('a'))));
    await act(async () =>
      controller().commands.respondToClientCapability({
        requestId: 'capability-1',
        decision: 'allow',
      }),
    );

    assert.deepEqual(responses, [
      { sessionId: 'a', requestId: 'capability-1', decision: 'allow' },
    ]);
  });

  it('keeps the initial Session active after StrictMode replays mount effects', async () => {
    const { root } = installReactRenderer();
    const starts: string[] = [];
    let browserSubscriptions = 0;
    const defaults = createFakeWorkbarServices();
    const services = createFakeWorkbarServices({
      browser: {
        ...defaults.browser,
        subscribeLive: () => {
          browserSubscriptions += 1;
          return () => undefined;
        },
      },
      terminal: {
        ...defaults.terminal,
        start: async (sessionId) => {
          starts.push(sessionId);
          return shellUpdate(sessionId, 'terminal-strict-mode');
        },
      },
    });

    await act(async () =>
      renderController(root, services, input(session('a')), true),
    );
    await act(async () => controller().commands.openTool('terminal'));

    assert.equal(browserSubscriptions, 2);
    assert.deepEqual(starts, ['a']);
    assert.equal(
      controller().host.panelsState.right.tabs.some(
        (tab) => tab.kind === 'terminal',
      ),
      true,
    );
  });

  it('opens registry singletons once and dynamic tools as separate instances', async () => {
    const { root } = installReactRenderer();
    const defaults = createFakeWorkbarServices();
    let terminalOrdinal = 0;
    const services = createFakeWorkbarServices({
      terminal: {
        ...defaults.terminal,
        start: async (sessionId) =>
          shellUpdate(sessionId, `terminal-${++terminalOrdinal}`),
      },
    });

    await act(async () => renderController(root, services, input(session('a'))));
    await act(async () => {
      controller().commands.openTool('review');
      controller().commands.openTool('review');
    });
    await act(async () => controller().commands.openTool('terminal'));
    await act(async () => controller().commands.openTool('terminal'));
    await act(async () => {
      controller().commands.openTool('side-chat');
      controller().commands.openTool('side-chat');
    });

    const tabs = controller().host.panelsState.right.tabs;
    assert.equal(tabs.filter((tab) => tab.kind === 'review').length, 1);
    assert.equal(tabs.filter((tab) => tab.kind === 'terminal').length, 2);
    assert.equal(tabs.filter((tab) => tab.kind === 'side-chat').length, 2);
    assert.deepEqual(
      tabs
        .filter((tab) => tab.kind === 'terminal')
        .map((tab) => tab.ordinal),
      [1, 2],
    );
    assert.deepEqual(
      tabs
        .filter((tab) => tab.kind === 'side-chat')
        .map((tab) => tab.ordinal),
      [1, 2],
    );
  });

  it('retains a Terminal whose start resolves after navigation without revealing it in the new Session', async () => {
    const { root } = installReactRenderer();
    const start = deferred<ShellRunUpdate>();
    const starts: string[] = [];
    const stops: Array<{ sessionId: string; ref: string }> = [];
    const defaults = createFakeWorkbarServices();
    const services = createFakeWorkbarServices({
      terminal: {
        ...defaults.terminal,
        start: (sessionId) => {
          starts.push(sessionId);
          return start.promise;
        },
        stop: async (request) => {
          stops.push(request);
          return null;
        },
      },
    });

    await act(async () => renderController(root, services, input(session('a'))));
    await act(async () => controller().commands.openTool('terminal'));
    assert.deepEqual(starts, ['a']);

    await act(async () => renderController(root, services, input(session('b'))));
    await act(async () => start.resolve(shellUpdate('a', 'terminal-a')));

    assert.deepEqual(stops, []);
    assert.equal(
      controller().host.panelsState.right.tabs.some(
        (tab) => tab.kind === 'terminal',
      ),
      true,
    );
    assert.equal(controller().host.rightCollapsed, true);
    await act(async () => renderController(root, services, input(session('a'))));
    const owned = controller().host.panelsState.right.tabs.find((tab) => tab.kind === 'terminal');
    assert.equal(owned?.ownerSessionId, 'a');
    assert.equal(owned?.resourceRef, 'terminal-a');
  });

  it('stops only explicitly closed Terminals and retains the same resource across navigation', async () => {
    const { root } = installReactRenderer();
    const stops: Array<{ sessionId: string; ref: string }> = [];
    const defaults = createFakeWorkbarServices();
    let ordinal = 0;
    const services = createFakeWorkbarServices({
      terminal: {
        ...defaults.terminal,
        start: async (sessionId) =>
          shellUpdate(sessionId, `terminal-${++ordinal}`),
        stop: async (request) => {
          stops.push(request);
          return null;
        },
      },
    });

    await act(async () => renderController(root, services, input(session('a'))));
    await act(async () => controller().commands.openTool('terminal'));
    const first = controller().host.panelsState.right.tabs.find(
      (tab) => tab.kind === 'terminal',
    );
    assert.ok(first);
    await act(async () => controller().host.onCloseTab('right', first));
    assert.deepEqual(stops, [{ sessionId: 'a', ref: 'terminal-1' }]);

    await act(async () => controller().commands.openTool('terminal'));
    const sessionSwitchSnapshot = controllerRenderSnapshots.length;
    await act(async () => renderController(root, services, input(session('b'))));
    assert.deepEqual(stops, [
      { sessionId: 'a', ref: 'terminal-1' },
    ]);
    assert.equal(
      controllerRenderSnapshots
        .slice(sessionSwitchSnapshot)
        .some(
          (snapshot) =>
            snapshot.activeId === 'b' &&
            snapshot.terminalOwnerIds.includes('a'),
        ),
      false,
    );
    await act(async () => renderController(root, services, input(session('a'))));
    const retained = controller().host.panelsState.right.tabs.find((tab) => tab.kind === 'terminal');
    assert.equal(retained?.resourceRef, 'terminal-2');
    assert.deepEqual(stops, [{ sessionId: 'a', ref: 'terminal-1' }]);
  });

  it('retains a failed Terminal close for visible retry and removes it only after Stop succeeds', async () => {
    const { root } = installReactRenderer();
    const firstStop = deferred<ShellRunUpdate | null>();
    const retryStop = deferred<ShellRunUpdate | null>();
    const stops: Array<{ sessionId: string; ref: string }> = [];
    const defaults = createFakeWorkbarServices();
    const services = createFakeWorkbarServices({
      terminal: {
        ...defaults.terminal,
        start: async (sessionId) => shellUpdate(sessionId, 'terminal-retry'),
        stop: (request) => {
          stops.push(request);
          return stops.length === 1 ? firstStop.promise : retryStop.promise;
        },
      },
    });

    await act(async () => renderController(root, services, input(session('a'))));
    await act(async () => controller().commands.openTool('terminal'));
    const tab = controller().host.panelsState.right.tabs.find(
      (candidate) => candidate.kind === 'terminal',
    );
    assert.ok(tab);
    await act(async () => controller().host.onCloseTab('right', tab));
    await act(async () => controller().host.onCloseTab('right', tab));
    assert.deepEqual(stops, [{ sessionId: 'a', ref: 'terminal-retry' }]);
    assert.ok(controller().host.panelsState.right.tabs.includes(tab));

    await act(async () => {
      firstStop.reject(new Error('Host disconnected'));
      await Promise.resolve();
    });
    await act(async () => renderController(root, services, input(session('b'))));
    assert.equal(stops.length, 1);
    await act(async () => renderController(root, services, input(session('a'))));
    assert.ok(controller().host.panelsState.right.tabs.includes(tab));
    await act(async () => controller().host.onCloseTab('right', tab));
    await act(async () => renderController(root, services, input(session('b'))));
    await act(async () => controller().commands.toggleRight());
    assert.equal(controller().host.rightCollapsed, false);
    await act(async () => retryStop.resolve(null));
    assert.equal(controller().host.panelsState.right.tabs.includes(tab), false);
    assert.equal(controller().host.rightCollapsed, false);
    assert.deepEqual(stops, [
      { sessionId: 'a', ref: 'terminal-retry' },
      { sessionId: 'a', ref: 'terminal-retry' },
    ]);

    await act(async () => root.unmount());
    assert.equal(stops.length, 2);
  });

  it('releases a retired owner’s terminal topology and teardown obligation', async () => {
    const { root } = installReactRenderer();
    const lateStart = deferred<ShellRunUpdate>();
    const stops: string[] = [];
    const defaults = createFakeWorkbarServices();
    const services = createFakeWorkbarServices({ terminal: {
      ...defaults.terminal,
      start: async (id) => id === 'c' ? lateStart.promise : shellUpdate(id, `terminal-${id}`),
      stop: async ({ ref }) => { stops.push(ref); return null; },
    } });
    await act(async () => renderController(root, services, input(session('a'))));
    await act(async () => controller().commands.openTool('terminal'));
    await act(async () => renderController(root, services, input(session('b'))));
    await act(async () => controller().commands.openTool('terminal'));
    // Host admits retirement only after A's resource is terminal. Its catalog
    // removal is authoritative; the renderer does not issue another Stop.
    await act(async () => renderController(root, services, {
      ...input(session('b')), authoritativeSessionIds: new Set(['b']),
    }));
    assert.deepEqual(controller().host.panelsState.right.tabs.map((tab) => tab.ownerSessionId), ['b']);
    await act(async () => renderController(root, services, input(session('c'))));
    await act(async () => controller().commands.openTool('terminal'));
    await act(async () => renderController(root, services, {
      ...input(session('b')), authoritativeSessionIds: new Set(['b']),
    }));
    // A delayed response cannot resurrect a terminal whose owner has retired.
    await act(async () => lateStart.resolve(shellUpdate('c', 'terminal-c')));
    assert.deepEqual(controller().host.panelsState.right.tabs.map((tab) => tab.ownerSessionId), ['b']);
    assert.deepEqual(stops, []);
    await act(async () => root.unmount());
    assert.deepEqual(stops, ['terminal-b']);
  });

  it('owns a resolved Terminal before its tab state commits', async () => {
    const { root } = installReactRenderer();
    const start = deferred<ShellRunUpdate>();
    const stops: Array<{ sessionId: string; ref: string }> = [];
    const defaults = createFakeWorkbarServices();
    const services = createFakeWorkbarServices({
      terminal: {
        ...defaults.terminal,
        start: () => start.promise,
        stop: async (request) => {
          stops.push(request);
          return null;
        },
      },
    });

    await act(async () => renderController(root, services, input(session('a'))));
    await act(async () => controller().commands.openTool('terminal'));
    await act(async () => {
      start.resolve(shellUpdate('a', 'terminal-before-commit'));
      await Promise.resolve();
      root.unmount();
    });

    assert.deepEqual(stops, [
      { sessionId: 'a', ref: 'terminal-before-commit' },
    ]);
  });

  it('stops an opened Terminal exactly once when the controller unmounts', async () => {
    const { root } = installReactRenderer();
    const stops: Array<{ sessionId: string; ref: string }> = [];
    const defaults = createFakeWorkbarServices();
    const services = createFakeWorkbarServices({
      terminal: {
        ...defaults.terminal,
        start: async (sessionId) => shellUpdate(sessionId, 'terminal-unmount'),
        stop: async (request) => {
          stops.push(request);
          return null;
        },
      },
    });

    await act(async () => renderController(root, services, input(session('a'))));
    await act(async () => controller().commands.openTool('terminal'));
    await act(async () => root.unmount());

    assert.deepEqual(stops, [
      { sessionId: 'a', ref: 'terminal-unmount' },
    ]);
  });

  it('reports only a Terminal start failure that still belongs to the active Session', async () => {
    const { root } = installReactRenderer();
    const currentErrors: string[] = [];
    const staleErrors: string[] = [];
    const currentStart = deferred<ShellRunUpdate>();
    const staleStart = deferred<ShellRunUpdate>();
    const defaults = createFakeWorkbarServices();
    let attempt = 0;
    const services = createFakeWorkbarServices({
      terminal: {
        ...defaults.terminal,
        start: () => (++attempt === 1 ? currentStart.promise : staleStart.promise),
      },
    });

    await act(async () =>
      renderController(root, services, input(session('a'), currentErrors)),
    );
    await act(async () => controller().commands.openTool('terminal'));
    await act(async () => currentStart.reject(new Error('current failure')));
    assert.equal(currentErrors.length, 1);

    await act(async () =>
      renderController(root, services, input(session('a'), staleErrors)),
    );
    await act(async () => controller().commands.openTool('terminal'));
    await act(async () =>
      renderController(root, services, input(session('b'), staleErrors)),
    );
    await act(async () => staleStart.reject(new Error('stale failure')));
    assert.deepEqual(staleErrors, []);
  });

  it('keeps Side Chat through collapse, confirms content close, and removes it on source switch', async () => {
    const { root } = installReactRenderer();
    const services = createFakeWorkbarServices();
    await act(async () => renderController(root, services, input(session('a'))));

    await act(async () => controller().commands.openTool('side-chat'));
    const panelId = controller().host.quotes?.[0]?.id;
    assert.ok(panelId);
    await act(async () => controller().commands.toggleRight());
    assert.equal(controller().host.quotes?.some((panel) => panel.id === panelId), true);

    await act(async () => controller().host.onContentStateChange?.(panelId, true));
    const tab = controller().host.panelsState.right.tabs.find(
      (candidate) => candidate.id === `side-chat:${panelId}`,
    );
    assert.ok(tab);
    await act(async () => controller().host.onCloseTab('right', tab));
    assert.equal(controller().host.closeConfirmation.open, true);
    await act(async () => controller().host.closeConfirmation.onCancel());
    assert.equal(
      controller().host.panelsState.right.tabs.some(
        (candidate) => candidate.id === tab.id,
      ),
      true,
    );

    await act(async () => controller().host.onCloseTab('right', tab));
    await act(async () => controller().host.closeConfirmation.onConfirm(false));
    assert.equal(
      controller().host.panelsState.right.tabs.some(
        (candidate) => candidate.id === tab.id,
      ),
      false,
    );

    await act(async () => controller().commands.openTool('side-chat'));
    await act(async () => renderController(root, services, input(session('b'))));
    assert.equal(
      controller().host.panelsState.right.tabs.some(
        (candidate) => candidate.kind === 'side-chat',
      ),
      false,
    );
  });

  it('hides created companion Sessions until cleanup or reconciliation', async () => {
    const { root } = installReactRenderer();
    const services = createFakeWorkbarServices();
    const firstInput = input(session('a'));
    firstInput.authoritativeSessionIds = new Set(['a', 'fork']);
    await act(async () => renderController(root, services, firstInput));

    await act(async () =>
      controller().host.onForkVisibilityChange?.({
        type: 'fork-created',
        sessionId: 'fork',
      }),
    );
    assert.equal(controller().selectors.hiddenSessionIds.has('fork'), true);

    const reconciled = input(session('a'));
    reconciled.authoritativeSessionIds = new Set(['a']);
    await act(async () => renderController(root, services, reconciled));
    assert.equal(controller().selectors.hiddenSessionIds.has('fork'), false);
  });

  it('binds Browser ownership and disposes its live subscription once', async () => {
    const { root } = installReactRenderer();
    const activeSessions: Array<string | null> = [];
    let subscriptions = 0;
    let disposals = 0;
    const defaults = createFakeWorkbarServices();
    const services = createFakeWorkbarServices({
      browser: {
        ...defaults.browser,
        setActiveSession: (sessionId) => activeSessions.push(sessionId),
        subscribeLive: () => {
          subscriptions += 1;
          return () => {
            disposals += 1;
          };
        },
      },
    });

    await act(async () => renderController(root, services, input(session('a'))));
    await act(async () => renderController(root, services, input(session('b'))));
    await act(async () => root.unmount());

    assert.equal(subscriptions, 1);
    assert.equal(disposals, 1);
    assert.deepEqual(activeSessions, ['a', 'b']);
  });
});
