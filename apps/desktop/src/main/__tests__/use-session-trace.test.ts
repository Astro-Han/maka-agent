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

import { strict as assert } from 'node:assert';
import { TRACE_REFRESH_DEBOUNCE_MS } from '@maka/ui/context-usage';
import { afterEach, describe, it } from 'node:test';
import { act, createElement } from 'react';
import {
  SESSION_TRACE_SCHEMA_VERSION,
  type SessionTrace,
} from '@maka/core/session-trace';
import type { SessionEvent } from '@maka/core/events';
import type { Result } from '@maka/core/result';
import { cleanupFakeDom, installReactRenderer } from './fake-dom.js';
import {
  createFakeWorkbarServices,
  useSessionTrace,
  type SessionTracePage,
  type WorkbarServices,
} from '../../renderer/features/workbar/testing.js';

/**
 * The hook whose doc comment once described a subscription it did not have.
 * These render it for real, because that drift is invisible to every other
 * kind of test.
 */
const COPY = { loadFailed: 'failed', locale: 'en' } as const;

function trace(sessionId: string): SessionTrace {
  return {
    schemaVersion: SESSION_TRACE_SCHEMA_VERSION,
    sessionId,
    turns: [],
    coverage: {
      modelCalls: 'none',
      turnsMissingModelCalls: [],
      turnsWithFewerModelCallsThanSteps: [],
      unreadableRecords: 0,
      oversizedRuns: 0,
    },
  };
}

interface TraceHarness {
  services: WorkbarServices;
  reads: string[];
  traceRequests: Array<{ sessionId: string; cursor?: string }>;
  contextReads: string[];
  emit: (event: SessionEvent) => void;
  subscriptions: number;
  unsubscribes: number;
}

function createTraceHarness(
  options: {
    tracePages?: SessionTracePage[];
    trace?: (
      sessionId: string,
      cursor?: string,
    ) => Promise<Result<SessionTracePage>>;
  } = {},
): TraceHarness {
  const handlers = new Set<(event: SessionEvent) => void>();
  const harness: TraceHarness = {
    services: undefined as never,
    reads: [],
    traceRequests: [],
    contextReads: [],
    emit: (event) => {
      for (const handler of [...handlers]) handler(event);
    },
    subscriptions: 0,
    unsubscribes: 0,
  };
  const services = createFakeWorkbarServices({
    inspector: {
      trace: async (
        sessionId: string,
        cursor?: string,
      ): Promise<Result<SessionTracePage>> => {
        harness.reads.push(sessionId);
        harness.traceRequests.push({ sessionId, ...(cursor ? { cursor } : {}) });
        if (options.trace) return options.trace(sessionId, cursor);
        return {
          ok: true,
          data: options.tracePages?.shift() ?? {
            trace: trace(sessionId),
            nextCursor: null,
          },
        };
      },
      // The hook reads the context snapshot on the same signal (#2323). It
      // is counted separately: the assertions below are about how often the
      // TRACE is re-read, and an enrichment read must not move them.
      context: async (sessionId: string) => {
        harness.contextReads.push(sessionId);
        return {
          ok: true as const,
          data: {
            status: 'unavailable' as const,
            reason: 'no_completed_request' as const,
          },
        };
      },
      subscribeSessionEvents: (
        _sessionId: string,
        handler: (event: SessionEvent) => void,
      ) => {
        harness.subscriptions += 1;
        handlers.add(handler);
        return () => {
          harness.unsubscribes += 1;
          handlers.delete(handler);
        };
      },
    },
  });
  harness.services = services;
  return harness;
}

function event(type: SessionEvent['type']): SessionEvent {
  return { type, id: `${type}-1`, turnId: 'turn-1', ts: 1 } as SessionEvent;
}

function Probe(props: {
  services: WorkbarServices;
  sessionId?: string;
  active: boolean;
  onSnapshot?: (trace: SessionTrace | undefined) => void;
  onHookSnapshot?: (snapshot: ReturnType<typeof useSessionTrace>) => void;
}) {
  const snapshot = useSessionTrace(props.sessionId, props.active, COPY, props.services.inspector);
  props.onSnapshot?.(snapshot.trace);
  props.onHookSnapshot?.(snapshot);
  return null;
}

function tracePage(
  sessionId: string,
  runId: string,
  nextCursor: string | null,
): SessionTracePage {
  const startedAt = Number(runId.replace(/\D/g, ''));
  return {
    trace: {
      ...trace(sessionId),
      turns: [
        {
          turnId: `turn-${runId}`,
          runId,
          startedAt,
          endedAt: startedAt,
          durationMs: 0,
          steps: [],
        },
      ],
    },
    nextCursor,
  };
}

async function flushRefresh(): Promise<void> {
  // The coalescer's real timer, plus a margin for the read it starts. Derived
  // from the constant so the two cannot drift apart.
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, TRACE_REFRESH_DEBOUNCE_MS + 50));
  });
}

describe('useSessionTrace', () => {
  afterEach(() => {
    cleanupFakeDom();
    delete (globalThis as { window?: unknown }).window;
  });

  it('subscribes only while the panel is active, and unsubscribes when it hides', async () => {
    const { root } = installReactRenderer();
    const harness = createTraceHarness();

    await act(async () => {
      root.render(createElement(Probe, { services: harness.services, sessionId: 'session-1', active: false }));
    });
    assert.equal(harness.subscriptions, 0, 'a hidden panel subscribes to nothing');
    assert.deepEqual(harness.reads, [], 'and reads nothing');

    await act(async () => {
      root.render(createElement(Probe, { services: harness.services, sessionId: 'session-1', active: true }));
    });
    assert.equal(harness.subscriptions, 1);
    assert.deepEqual(harness.reads, ['session-1']);

    await act(async () => {
      root.render(createElement(Probe, { services: harness.services, sessionId: 'session-1', active: false }));
    });
    assert.equal(harness.unsubscribes, 1, 'hiding releases the subscription');
  });

  it('re-reads once for a burst of ledger-changing events', async () => {
    const { root } = installReactRenderer();
    const harness = createTraceHarness();
    await act(async () => {
      root.render(createElement(Probe, { services: harness.services, sessionId: 'session-1', active: true }));
    });
    assert.equal(harness.reads.length, 1, 'the activation read');

    await act(async () => {
      harness.emit(event('tool_result'));
      harness.emit(event('token_usage'));
      harness.emit(event('complete'));
    });
    await flushRefresh();

    assert.equal(harness.reads.length, 2, 'a closing burst is one re-read, not three');
  });

  it('does not re-read for streaming deltas', async () => {
    const { root } = installReactRenderer();
    const harness = createTraceHarness();
    await act(async () => {
      root.render(createElement(Probe, { services: harness.services, sessionId: 'session-1', active: true }));
    });

    await act(async () => {
      for (let index = 0; index < 20; index += 1) harness.emit(event('text_delta'));
    });
    await flushRefresh();

    assert.equal(harness.reads.length, 1, 'a streaming turn must not re-project per delta');
  });

  it('never reads after the panel hides, even for an event already in flight', async () => {
    const { root } = installReactRenderer();
    const harness = createTraceHarness();
    await act(async () => {
      root.render(createElement(Probe, { services: harness.services, sessionId: 'session-1', active: true }));
    });

    await act(async () => {
      harness.emit(event('complete'));
    });
    await act(async () => {
      root.render(createElement(Probe, { services: harness.services, sessionId: 'session-1', active: false }));
    });
    await flushRefresh();

    assert.equal(harness.reads.length, 1, 'the scheduled read dies with the panel');
  });

  it('keeps the previous timeline across hide and re-activation', () => {
    // Switching tabs away and back is not new information about the session, so
    // blanking the timeline would read as the panel forgetting. Nothing pinned
    // this before, so reverting it passed the suite unchanged.
    const { root } = installReactRenderer();
    const harness = createTraceHarness();
    const seen: Array<SessionTrace | undefined> = [];
    const render = async (active: boolean) => {
      await act(async () => {
        root.render(
          createElement(Probe, {
            services: harness.services,
            sessionId: 'session-1',
            active,
            onSnapshot: (snapshotTrace) => seen.push(snapshotTrace),
          }),
        );
      });
    };

    return (async () => {
      await render(true);
      assert.equal(seen.at(-1)?.sessionId, 'session-1', 'the first read populates it');

      await render(false);
      await render(true);

      // Every frame of the re-activation read still carries the previous trace:
      // no render in between saw `undefined`.
      // Once a trace has arrived, no later render may go back to nothing —
      // renders before it are the initial mount and its loading frame.
      const firstTrace = seen.findIndex((snapshotTrace) => snapshotTrace !== undefined);
      assert.notEqual(firstTrace, -1, 'a trace arrived at all');
      assert.equal(
        seen.slice(firstTrace).every((snapshotTrace) => snapshotTrace !== undefined),
        true,
        'the timeline never blanks once it has content',
      );
      assert.equal(harness.reads.length, 2, 're-activation still re-reads');
    })();
  });

  it('rebuilds the requested page depth from the newest runs after a refresh', async () => {
    const { root } = installReactRenderer();
    const harness = createTraceHarness({
      tracePages: [
        tracePage('session-1', 'run-3', 'cursor-3'),
        tracePage('session-1', 'run-2', 'cursor-2'),
        tracePage('session-1', 'run-5', 'cursor-5'),
        tracePage('session-1', 'run-4', 'cursor-4'),
      ],
    });
    let snapshot: ReturnType<typeof useSessionTrace> | undefined;
    await act(async () => {
      root.render(
        createElement(Probe, {
          services: harness.services,
          sessionId: 'session-1',
          active: true,
          onHookSnapshot: (value) => {
            snapshot = value;
          },
        }),
      );
    });

    await act(async () => snapshot?.loadEarlier());
    assert.deepEqual(
      snapshot?.trace?.turns.map((turn) => turn.runId),
      ['run-2', 'run-3'],
    );

    await act(async () => harness.emit(event('complete')));
    await flushRefresh();

    assert.deepEqual(
      snapshot?.trace?.turns.map((turn) => turn.runId),
      ['run-4', 'run-5'],
    );
    assert.deepEqual(
      harness.traceRequests.map((request) => request.cursor),
      [undefined, 'cursor-3', undefined, 'cursor-5'],
    );
  });

  it('collapses all loaded earlier pages after reaching the oldest page', async () => {
    const { root } = installReactRenderer();
    const harness = createTraceHarness({
      tracePages: [
        tracePage('session-1', 'run-3', 'cursor-3'),
        tracePage('session-1', 'run-2', null),
      ],
    });
    let snapshot: ReturnType<typeof useSessionTrace> | undefined;
    await act(async () => {
      root.render(
        createElement(Probe, {
          services: harness.services,
          sessionId: 'session-1',
          active: true,
          onHookSnapshot: (value) => {
            snapshot = value;
          },
        }),
      );
    });

    assert.equal(snapshot?.canHideEarlier, false);
    await act(async () => snapshot?.loadEarlier());
    assert.equal(snapshot?.canHideEarlier, true);
    assert.deepEqual(
      snapshot?.trace?.turns.map((turn) => turn.runId),
      ['run-2', 'run-3'],
    );

    await act(async () => snapshot?.hideEarlier());
    assert.equal(snapshot?.canHideEarlier, false);
    assert.equal(snapshot?.nextCursor, 'cursor-3');
    assert.deepEqual(
      snapshot?.trace?.turns.map((turn) => turn.runId),
      ['run-3'],
    );
  });

  it('rebuilds the loaded window when a run is inserted behind an unchanged head cursor', async () => {
    const { root } = installReactRenderer();
    const harness = createTraceHarness({
      tracePages: [
        tracePage('session-1', 'run-z', 'cursor-z'),
        tracePage('session-1', 'run-m', null),
        // The head page and its cursor are unchanged, but run-n now sorts
        // between the two pages that were already loaded.
        tracePage('session-1', 'run-z', 'cursor-z'),
        tracePage('session-1', 'run-n', 'cursor-n'),
      ],
    });
    let snapshot: ReturnType<typeof useSessionTrace> | undefined;
    await act(async () => {
      root.render(
        createElement(Probe, {
          services: harness.services,
          sessionId: 'session-1',
          active: true,
          onHookSnapshot: (value) => {
            snapshot = value;
          },
        }),
      );
    });

    await act(async () => snapshot?.loadEarlier());
    assert.deepEqual(
      snapshot?.trace?.turns.map((turn) => turn.runId),
      ['run-m', 'run-z'],
    );

    await act(async () => harness.emit(event('complete')));
    await flushRefresh();

    assert.deepEqual(
      snapshot?.trace?.turns.map((turn) => turn.runId),
      ['run-n', 'run-z'],
      'an unchanged head cursor cannot justify reusing a stale older page',
    );
    assert.equal(snapshot?.nextCursor, 'cursor-n');
  });

  it('keeps the requested page depth when a head refresh supersedes load-earlier', async () => {
    const { root } = installReactRenderer();
    let resolveEarlierPage: ((result: Result<SessionTracePage>) => void) | undefined;
    const earlierPage = new Promise<Result<SessionTracePage>>((resolve) => {
      resolveEarlierPage = resolve;
    });
    let startReads = 0;
    const harness = createTraceHarness({
      trace: async (sessionId, cursor) => {
        if (cursor === undefined) {
          startReads += 1;
          if (startReads === 1) {
            return { ok: true, data: tracePage(sessionId, 'run-3', 'cursor-3') };
          }
          return { ok: true, data: tracePage(sessionId, 'run-4', 'cursor-4') };
        }
        if (cursor === 'cursor-3') return earlierPage;
        if (cursor === 'cursor-4') {
          return { ok: true, data: tracePage(sessionId, 'run-3', 'cursor-3') };
        }
        throw new Error(`unexpected cursor ${cursor}`);
      },
    });
    let snapshot: ReturnType<typeof useSessionTrace> | undefined;
    await act(async () => {
      root.render(
        createElement(Probe, {
          services: harness.services,
          sessionId: 'session-1',
          active: true,
          onHookSnapshot: (value) => {
            snapshot = value;
          },
        }),
      );
    });

    await act(async () => snapshot?.loadEarlier());
    assert.equal(snapshot?.loadingEarlier, true);
    await act(async () => harness.emit(event('complete')));
    await flushRefresh();
    await act(async () => {
      resolveEarlierPage?.({ ok: true, data: tracePage('session-1', 'run-2', 'cursor-2') });
    });

    assert.deepEqual(
      snapshot?.trace?.turns.map((turn) => turn.runId),
      ['run-3', 'run-4'],
    );
    assert.equal(snapshot?.loadingEarlier, false);
    assert.deepEqual(
      harness.traceRequests.map((request) => request.cursor),
      [undefined, 'cursor-3', undefined, 'cursor-4'],
    );
  });

  it('settles loading when load-earlier supersedes an in-flight retry', async () => {
    const { root } = installReactRenderer();
    let resolveRetry: ((result: Result<SessionTracePage>) => void) | undefined;
    const retryPage = new Promise<Result<SessionTracePage>>((resolve) => {
      resolveRetry = resolve;
    });
    let headReads = 0;
    const harness = createTraceHarness({
      trace: async (sessionId, cursor) => {
        if (cursor === undefined) {
          headReads += 1;
          return headReads === 1
            ? { ok: true, data: tracePage(sessionId, 'run-3', 'cursor-3') }
            : retryPage;
        }
        if (cursor === 'cursor-3') {
          return { ok: true, data: tracePage(sessionId, 'run-2', 'cursor-2') };
        }
        throw new Error(`unexpected cursor ${cursor}`);
      },
    });
    let snapshot: ReturnType<typeof useSessionTrace> | undefined;
    await act(async () => {
      root.render(
        createElement(Probe, {
          services: harness.services,
          sessionId: 'session-1',
          active: true,
          onHookSnapshot: (value) => {
            snapshot = value;
          },
        }),
      );
    });

    await act(async () => {
      snapshot?.retry();
      snapshot?.loadEarlier();
      await Promise.resolve();
    });

    assert.equal(snapshot?.loading, false);
    assert.equal(snapshot?.loadingEarlier, false);
    assert.deepEqual(
      snapshot?.trace?.turns.map((turn) => turn.runId),
      ['run-2', 'run-3'],
    );

    await act(async () => {
      resolveRetry?.({ ok: true, data: tracePage('session-1', 'run-4', 'cursor-4') });
    });
    assert.deepEqual(
      snapshot?.trace?.turns.map((turn) => turn.runId),
      ['run-2', 'run-3'],
    );
  });


});
