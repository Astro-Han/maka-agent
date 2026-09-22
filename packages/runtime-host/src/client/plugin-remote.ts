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

import {
  RemoteError,
  type ClientIdentity,
  type ClientRemote,
  type RemoteFailure,
} from '@maka-agent/plugin-sdk/client';
import type { Json } from '@maka-agent/plugin-sdk/host';
import type {
  PluginRemoteInput,
  PluginRemoteResult,
  PluginRemoteBinding,
  PluginRemoteTarget,
} from '../protocol/plugin-remote.js';
import { remoteStream } from './plugin-remote-stream.js';

/** One document per consumer, lazily allocated and never retargeted. */
export function createPluginRemote(
  transport: (
    input: PluginRemoteInput,
  ) => Promise<PluginRemoteResult | RemoteFailure | { kind: 'connection_retired' }>,
  identity: ClientIdentity | { readonly packageId: string },
  signal: AbortSignal,
): { api: ClientRemote; close(): Promise<void> } {
  const request = async (input: PluginRemoteInput) => {
    const result = await transport(input);
    if (result.kind === 'remote_error') throw new RemoteError(result.code, result.message);
    return result;
  };
  let document: Promise<string> | undefined;
  let closing: Promise<void> | undefined;
  const lifetime = new AbortController();
  const assertLive = () => {
    signal.throwIfAborted();
    if (closing) throw new Error('Client Remote is closed');
  };
  const getDocument = () => {
    assertLive();
    document ??= request({ kind: 'open_document' }).then((result) => {
      if (result.kind !== 'document') throw new Error('Unexpected Remote document result');
      return result.document;
    });
    return document;
  };
  const close = (): Promise<void> => {
    if (closing) return closing;
    signal.removeEventListener('abort', retired);
    closing = (async () => {
      if (!document) return;
      let id: string;
      try {
        id = await document;
      } catch {
        return;
      }
      const result = await request({ kind: 'close_document', document: id });
      // Connection retirement revokes this local lease. Host owns draining
      // the old connection and fences backend instances if cleanup fails.
      if (result.kind !== 'closed' && result.kind !== 'connection_retired')
        throw new Error('Remote cleanup was not confirmed');
    })();
    lifetime.abort(signal.aborted ? signal.reason : new Error('Client Remote is closed'));
    return closing;
  };
  const retired = () => {
    void close().catch(() => {});
  };
  signal.addEventListener('abort', retired, { once: true });
  if (signal.aborted) retired();

  const bind = (name: string, sessionId: string | undefined, handler: 'method' | 'stream') => {
    const binding: PluginRemoteBinding = {
      ...('packageId' in identity ? { packageId: identity.packageId } : { client: identity }),
      method: name,
      sessionId: sessionId ?? null,
    };
    let target: Promise<PluginRemoteTarget> | undefined;
    return async () => {
      assertLive();
      target ??= request({ kind: 'bind', binding }).then((result) => {
        if (result.kind !== 'bound' || result.handler !== handler)
          throw new Error('Remote handler kind mismatch');
        return result.target;
      });
      const [destination, owner] = await Promise.all([target, getDocument()]);
      assertLive();
      return { binding, target: destination, document: owner };
    };
  };
  const api: ClientRemote = {
    method<I extends Json, O extends Json>(name: string, sessionId?: string) {
      const bound = bind(name, sessionId, 'method');
      return async (input: I): Promise<O> => {
        const result = await request({ kind: 'call', ...(await bound()), input });
        assertLive();
        if (result.kind !== 'value') throw new Error('Unexpected Remote method result');
        return result.value as O;
      };
    },
    stream<I extends Json, O extends Json>(name: string, sessionId?: string) {
      const bound = bind(name, sessionId, 'stream');
      return (input: I, cancellation?: AbortSignal): AsyncIterable<O> =>
        remoteStream(
          async (pull) => {
            pull.throwIfAborted();
            const origin = await bound();
            pull.throwIfAborted();
            const opened = await request({ kind: 'open', ...origin, input });
            if (opened.kind !== 'opened') throw new Error('Unexpected Remote stream result');
            const handle = { document: origin.document, stream: opened.stream };
            return {
              async close() {
                if (closing) return;
                const result = await request({ kind: 'close', ...handle });
                if (result.kind !== 'closed' && result.kind !== 'connection_retired')
                  throw new Error('Remote stream cleanup was not confirmed');
              },
              async next() {
                assertLive();
                cancellation?.throwIfAborted();
                const result = await request({ kind: 'next', ...handle });
                if (result.kind === 'pending' || result.kind === 'end') return result;
                if (result.kind !== 'item') throw new Error('Unexpected Remote stream item');
                return { kind: 'item' as const, item: result.item as O };
              },
            };
          },
          cancellation ? [signal, lifetime.signal, cancellation] : [signal, lifetime.signal],
        );
    },
  };
  return { api, close };
}
