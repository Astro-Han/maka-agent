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

import { useCallback, useEffect, useRef, useState } from 'react';
import { generalizedErrorMessageForLocale } from '@maka/core/redaction';
import { type UiLocale } from '@maka/core/ui-locale';
import { hasSettledInitialOnboarding } from '@maka/core/onboarding-milestone';
import { useUiLocale, valuesEqual } from '@maka/ui';
import type { OnboardingSnapshot } from '../preload/bridge-contract.js';
import { getOnboardingCopy } from './locales/onboarding-copy.js';

export interface UseOnboardingSnapshotResult {
  snapshot: OnboardingSnapshot | null;
  error: string | null;
  refresh: () => void;

}

export interface UseOnboardingSnapshotDeps {
  /** Fetch the current snapshot. */
  getSnapshot: () => Promise<OnboardingSnapshot>;
  /**
   * Subscribe to invalidation signals. The handler is fired
   * (debounced internally by the caller if needed) whenever an
   * upstream event suggests the snapshot may be stale. Return value
   * is an unsubscribe function.
   */
  subscribeInvalidations: (onInvalidate: () => void) => () => void;
}

/**
 * The core readiness pair may seed only the unfinished first task. Once the
 * guide is settled or workspace history exists, normal Composer preference
 * rules own new-task selection again.
 */
export function getOnboardingActivationCandidate(
  snapshot: Pick<OnboardingSnapshot, 'state' | 'milestones'> | null,
  hasWorkspaceHistory: boolean,
): { llmConnectionSlug: string; model: string } | undefined {
  if (
    snapshot?.state.kind !== 'ready_empty' ||
    hasWorkspaceHistory ||
    hasSettledInitialOnboarding(snapshot.milestones)
  ) {
    return undefined;
  }
  return {
    llmConnectionSlug: snapshot.state.connectionSlug,
    model: snapshot.state.model,
  };
}

export function useOnboardingSnapshotImpl(
  deps: UseOnboardingSnapshotDeps,
): UseOnboardingSnapshotResult {
  const locale = useUiLocale();
  const localeRef = useRef(locale);
  localeRef.current = locale;
  const [snapshot, setSnapshot] = useState<OnboardingSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const pollerRef = useRef<OnboardingSnapshotPoller | null>(null);

  if (pollerRef.current === null) {
    pollerRef.current = createOnboardingSnapshotPoller(deps, {
      onSnapshot: (next) => {
        setSnapshot((previous) => previous && onboardingSnapshotProjectionEqual(previous, next) ? previous : next);
        setError(null);
      },
      onError: (message) => {
        setError(message);
      },
    }, () => localeRef.current);
  }

  useEffect(() => {
    const poller = pollerRef.current!;
    poller.activate();
    void poller.pull();
    const unsubscribe = deps.subscribeInvalidations(() => {
      void poller.pull();
    });
    return () => {
      unsubscribe();
      poller.dispose();
    };
  }, [deps]);

  const refresh = useCallback(() => {
    void pollerRef.current?.pull();
  }, []);


  return {
    snapshot,
    error,
    refresh,
  };
}

/** Serializes invalidations and fences responses across effect lifetimes. */
export interface OnboardingSnapshotPollerCallbacks {
  onSnapshot(snapshot: OnboardingSnapshot): void;
  onError(message: string): void;
}

export interface OnboardingSnapshotPoller {
  /** React effect setup calls this so StrictMode cleanup replay can recover. */
  activate(): void;
  /** Fetch the latest snapshot unless disposed. */
  pull(): Promise<void>;
  /** Stop accepting callbacks. Pending IPC responses become no-ops. */
  dispose(): void;
}

export function createOnboardingSnapshotPoller(
  deps: Pick<UseOnboardingSnapshotDeps, 'getSnapshot'>,
  callbacks: OnboardingSnapshotPollerCallbacks,
  getLocale: () => UiLocale,
): OnboardingSnapshotPoller {
  let inflightTicket = 0;
  let active = true;
  let inflight: Promise<void> | null = null;
  let pullAgain = false;

  function emitSnapshot(snapshot: OnboardingSnapshot): void {
    if (!active) return;
    callbacks.onSnapshot(snapshot);
  }

  function emitError(message: string): void {
    if (!active) return;
    callbacks.onError(message);
  }

  async function runPull(): Promise<void> {
    const ticket = ++inflightTicket;
    try {
      const next = await deps.getSnapshot();
      if (!active || ticket !== inflightTicket) return;
      emitSnapshot(next);
    } catch (err) {
      if (!active || ticket !== inflightTicket) return;
      emitError(onboardingSnapshotErrorMessage(err, getLocale()));
    }
  }

  return {
    activate(): void { active = true; },
    pull(): Promise<void> {
      if (!active) return Promise.resolve();
      if (inflight !== null) {
        pullAgain = true;
        return inflight;
      }
      inflight = (async () => {
        do {
          pullAgain = false;
          await runPull();
        } while (active && pullAgain);
        inflight = null;
      })();
      return inflight;
    },
    dispose(): void {
      active = false;
      inflightTicket += 1;
    },
  };
}

export function onboardingSnapshotErrorMessage(error: unknown, locale: UiLocale): string {
  const fallback = getOnboardingCopy(locale).snapshotErrorFallback;
  return generalizedErrorMessageForLocale(error, fallback, locale);
}

/** Session rows belong to the catalog; every other field participates by default. */
export function onboardingSnapshotProjectionEqual(a: OnboardingSnapshot, b: OnboardingSnapshot): boolean {
  const { sessions: _a, ...left } = a;
  const { sessions: _b, ...right } = b;
  return valuesEqual(left, right);
}

/** Host events invalidate readiness; explicit actions can also request a refresh. */
export function useOnboardingSnapshot(): UseOnboardingSnapshotResult {
  return useOnboardingSnapshotImpl(LIVE_DEPS);
}

const LIVE_DEPS: UseOnboardingSnapshotDeps = {
  getSnapshot: () => window.maka.onboarding.getSnapshot(),
  subscribeInvalidations(onInvalidate) {
    const unsubscribeSessions = window.maka.sessions.subscribeChanges(() => onInvalidate());
    const unsubscribeConnections = window.maka.connections.subscribeEvents(() => onInvalidate());
    return () => {
      unsubscribeSessions();
      unsubscribeConnections();
    };
  },
};
