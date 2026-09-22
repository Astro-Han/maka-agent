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

type Read<T> = { kind: 'pending' } | { kind: 'end' } | { kind: 'item'; item: T };
interface Driver<T> {
  next(): Promise<Read<T>>;
  close(): Promise<void>;
}

/** One consumer, one outstanding pull; return/abort never waits for a quiet producer. */
export function remoteStream<T>(
  open: (signal: AbortSignal) => Promise<Driver<T>>,
  signals: readonly AbortSignal[],
): AsyncIterableIterator<T> {
  let terminal: { kind: 'end' } | { kind: 'abort'; reason: unknown } | undefined;
  let reading = false;
  let opening: Promise<Driver<T>> | undefined;
  let driver: Driver<T> | undefined;
  let cleanup: Promise<void> | undefined;
  const lifetime = new AbortController();
  const interruptible = <V>(work: Promise<V>): Promise<V | undefined> =>
    new Promise((resolve, reject) => {
      const abort = () => {
        lifetime.signal.removeEventListener('abort', abort);
        resolve(undefined);
      };
      lifetime.signal.addEventListener('abort', abort, { once: true });
      work.then(
        (value) => {
          lifetime.signal.removeEventListener('abort', abort);
          resolve(value);
        },
        (error) => {
          lifetime.signal.removeEventListener('abort', abort);
          reject(error);
        },
      );
      if (lifetime.signal.aborted) abort();
    });
  const done = (): IteratorReturnResult<undefined> => ({ done: true, value: undefined });
  const close = () => {
    if (driver && !cleanup) {
      cleanup = Promise.resolve().then(() => driver!.close());
      // A late open may settle after its caller has left. The Host document
      // still owns the resource and fences unconfirmed backend cleanup.
      void cleanup.catch(() => {});
    }
    return cleanup;
  };
  const listeners = signals.map((signal) => ({
    signal,
    abort: () => stop({ kind: 'abort', reason: signal.reason }),
  }));
  function stop(result: NonNullable<typeof terminal>, ended = false): void {
    if (terminal) return;
    terminal = result;
    for (const { signal, abort } of listeners) signal.removeEventListener('abort', abort);
    lifetime.abort();
    if (!ended) close();
  }
  const result = () => {
    if (terminal?.kind === 'abort') throw terminal.reason;
    return done();
  };
  for (const { signal, abort } of listeners) {
    if (signal.aborted) {
      abort();
      break;
    }
    signal.addEventListener('abort', abort, { once: true });
  }
  return {
    [Symbol.asyncIterator]() {
      return this;
    },
    async next() {
      if (terminal) return result();
      if (reading) throw new Error('Client Remote stream already has an active pull');
      reading = true;
      try {
        opening ??= open(lifetime.signal).then((opened) => {
          driver = opened;
          if (terminal) close();
          return opened;
        });
        await interruptible(opening);
        if (terminal) return result();
        for (;;) {
          const read = await interruptible(driver!.next());
          if (terminal) return result();
          if (!read) throw new Error('Unexpected Remote stream result');
          if (read.kind === 'pending') continue;
          if (read.kind === 'end') {
            stop({ kind: 'end' }, true);
            return done();
          }
          return { done: false, value: read.item };
        }
      } catch (error) {
        if (terminal) return result();
        stop({ kind: 'end' });
        throw error;
      } finally {
        reading = false;
      }
    },
    async return() {
      stop({ kind: 'end' });
      await cleanup;
      return done();
    },
    async throw(error?: unknown) {
      stop({ kind: 'end' });
      await cleanup;
      throw error;
    },
  };
}
