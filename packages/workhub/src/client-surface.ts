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

import type { WorkHubRootProps } from './surface.js';
import type { coordinationCommands } from './client-session.js';

/** Retirement closes new adapter calls; returned subscription cleanups remain usable.
 * These are Client lifecycle guards, not Host execution authority. */
export function bindSurface(
  ports: Pick<WorkHubRootProps, 'sessions' | 'native' | 'attachments' | 'contextUsage'>,
  commands: ReturnType<typeof coordinationCommands>,
  signal: AbortSignal,
): Pick<WorkHubRootProps, 'sessions' | 'native' | 'attachments' | 'contextUsage'> {
  const files = methods(ports.attachments, signal);
  return {
    sessions: methods(ports.sessions, signal, commands),
    native: {
      presentation: methods(ports.native.presentation, signal),
      control: methods(ports.native.control, signal),
      bindBrowserSession(sessionId) {
        if (sessionId !== null) signal.throwIfAborted();
        ports.native.bindBrowserSession(sessionId);
      },
    },
    attachments: {
      staging: methods(files.staging, signal),
      read: files.read,
      prepare: files.prepare,
      copy: ports.attachments.copy,
      formatError: ports.attachments.formatError,
    },
    contextUsage: methods(ports.contextUsage, signal),
  };
}

function methods<T extends object>(api: T, signal: AbortSignal, overrides?: Partial<T>): T {
  const cache = new Map<PropertyKey, { source: unknown; call: (...args: unknown[]) => unknown }>();
  // Electron bridges are frozen. A separate target permits wrapping methods
  // without violating the source object's non-configurable property invariants.
  return new Proxy(Object.create(api) as T, {
    get(_target, key) {
      const owner = overrides && Object.hasOwn(overrides, key) ? overrides : api;
      const source = Reflect.get(owner, key, owner);
      if (typeof source !== 'function') return source;
      let entry = cache.get(key);
      if (!entry || entry.source !== source) {
        entry = {
          source,
          call(...args) {
            signal.throwIfAborted();
            return Reflect.apply(source, owner, args);
          },
        };
        cache.set(key, entry);
      }
      return entry.call;
    },
  });
}
