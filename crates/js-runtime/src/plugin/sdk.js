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

(namespace, key) => {
  const activate = namespace.default;
  if (typeof activate !== 'function') {
    throw new TypeError('A Host plugin must export a default activation function');
  }
  const callbacks = new Map();
  const registrations = [];
  const cleanups = [];
  const jobs = [];
  const active = new Set();
  const invocations = new Map();
  const streams = new Map();
  let streamSequence = 0;
  const closeStream = async (handle) => {
    const stream = streams.get(handle);
    if (!stream) return;
    stream.closing ??= (async () => {
      stream.cancel();
      await stream.pending?.catch(() => {});
      try {
        await stream.value.close();
      } finally {
        streams.delete(handle);
        invocations.delete(handle);
      }
    })();
    return stream.closing;
  };
  const processes = (authority) => {
    const open = (handle) =>
      Object.freeze({
        id: handle,
        write: (data) =>
          host('process.write', {
            authority,
            handle,
            bytes: Array.from(typeof data === 'string' ? new TextEncoder().encode(data) : data),
          }),
        endInput: () => host('process.endInput', { authority, handle }),
        next: async () => {
          const chunk = await host('process.next', { authority, handle });
          return chunk === null ? null : { ...chunk, bytes: Uint8Array.from(chunk.bytes) };
        },
        wait: () => host('process.wait', { authority, handle }),
        close: () => host('process.close', { handle }),
      });
    return Object.freeze({
      spawn: async (command) => open(await host('process.spawn', { command, authority })),
      open,
    });
  };
  const getService = async (name, authority) => {
    const handle = await host('service.get', { name });
    if (handle === null) return undefined;
    return Object.freeze({
      call: (input) => host('service.call', { handle, input, authority }),
      close: () => host('service.release', { handle }),
    });
  };
  const terminals = (authority) => {
    const open = (handle) =>
      Object.freeze({
        id: handle,
        write: (text, size) => host('terminal.control', { authority, handle, text, size }),
        resize: (size) => host('terminal.control', { authority, handle, size }),
        next: () => host('terminal.next', { authority, handle }),
        wait: () => host('terminal.wait', { authority, handle }),
        close: () => host('terminal.close', { handle }),
      });
    return Object.freeze({
      spawn: async (command, size = { cols: 80, rows: 24 }) =>
        open(await host('terminal.spawn', { authority, command, size })),
      open,
    });
  };
  const files = (authority) =>
    Object.freeze(
      Object.fromEntries(
        ['read', 'write', 'edit', 'glob', 'grep', 'patch'].map((kind) => [
          kind,
          (input) => host('files.invoke', { authority, operation: { kind, input } }),
        ]),
      ),
    );
  const http = (authority) =>
    Object.freeze({
      request: async ({ body, ...request }) => {
        const head = await host('http.request', {
          method: 'GET',
          ...request,
          authority,
          body: Array.from(
            typeof body === 'string' ? new TextEncoder().encode(body) : (body ?? []),
          ),
        });
        const { handle, ...metadata } = head;
        return Object.freeze({
          ...metadata,
          headers: head.headers.map(([name, value]) => [name, Uint8Array.from(value)]),
          next: async () => {
            const bytes = await host('http.next', { authority, handle });
            return bytes === null ? null : Uint8Array.from(bytes);
          },
          close: () => host('http.close', { handle }),
        });
      },
    });
  const tracked = async (task) => {
    const promise = Promise.resolve().then(task);
    active.add(promise);
    try {
      return await promise;
    } finally {
      active.delete(promise);
    }
  };
  let next = 0;
  let phase = 'new';
  let signalStop;
  const stopping = new Promise((resolve) => {
    signalStop = resolve;
  });
  const signal = Object.freeze({
    get aborted() {
      return phase === 'retired';
    },
    wait: () => stopping,
    throwIfAborted() {
      if (this.aborted) throw new Error('Plugin retired');
    },
  });
  const host = async (method, input) => {
    const reply = await Deno.core.ops.op_maka_plugin(key, method, input);
    if (reply.ok) return reply.value;
    throw Object.assign(new Error(reply.error.message), { code: reply.error.code });
  };
  const callback = (fn) => {
    if (typeof fn !== 'function' || callbacks.size >= 256 || next >= 0xffff_ffff) {
      throw new TypeError('Invalid callback or plugin callback limit exceeded');
    }
    const id = ++next;
    callbacks.set(id, fn);
    return id;
  };
  const register = async (kind, definition, fn) => {
    if (
      !['loading', 'prepared', 'active'].includes(phase) ||
      (phase === 'loading' && registrations.length >= 128)
    ) {
      throw new Error('Invalid contribution registration phase or capacity');
    }
    const descriptor = { ...definition, kind, callback: callback(fn) };
    let handle;
    if (phase === 'loading') {
      registrations.push(descriptor);
    } else {
      try {
        handle = await host('contribution.publish', [descriptor]);
      } catch (error) {
        callbacks.delete(descriptor.callback);
        throw error;
      }
    }
    let closing;
    return Object.freeze({
      close() {
        closing ??= (async () => {
          if (phase === 'loading') {
            const index = registrations.indexOf(descriptor);
            if (index >= 0) registrations.splice(index, 1);
            callbacks.delete(descriptor.callback);
          } else if (phase !== 'retired') {
            if (handle) await host('contribution.release', { handle });
            else await host('contribution.withdraw', { kind, name: descriptor.name });
          }
        })();
        return closing;
      },
    });
  };
  const text = (value) => (typeof value === 'function' ? value : () => value);
  return Object.freeze({
    async activate(identity, config) {
      if (phase !== 'new') throw new Error('Plugin already initialized');
      phase = 'loading';
      const context = Object.freeze({
        identity: Object.freeze(identity),
        signal,
        tools: Object.freeze({
          register: (definition, invoke) => register('tool', definition, invoke),
        }),
        executors: Object.freeze({
          register: (definition, execute) => register('executor', definition, execute),
        }),
        remote: Object.freeze({
          method: (name, invoke) => register('remote_method', { name }, invoke),
          stream: (name, open) =>
            register('remote_stream', { name }, async (input, call) => {
              if (streams.size >= 32) throw new Error('Client stream capacity exceeded');
              const handle = 'stream-' + ++streamSequence;
              let stopped = false;
              let stop;
              const cancelled = new Promise((resolve) => {
                stop = resolve;
              });
              const stream = {
                value: undefined,
                pending: undefined,
                closing: undefined,
                cancelledValue: false,
                cancel() {
                  stopped = true;
                  stop();
                  if (stream.value && !stream.cancelledValue) {
                    stream.cancelledValue = true;
                    stream.value.cancel();
                  }
                },
              };
              const streamSignal = Object.freeze({
                get aborted() {
                  return stopped || call.signal.aborted;
                },
                wait: () => Promise.race([cancelled, call.signal.wait()]),
                throwIfAborted() {
                  if (this.aborted) throw new Error('Client stream cancelled');
                },
              });
              streams.set(handle, stream);
              invocations.set(handle, stream.cancel);
              try {
                stream.value = await open(input, Object.freeze({ ...call, signal: streamSignal }));
                if (
                  !stream.value ||
                  typeof stream.value.next !== 'function' ||
                  typeof stream.value.cancel !== 'function' ||
                  typeof stream.value.close !== 'function'
                ) {
                  throw new Error('Invalid Client stream');
                }
                if (stopped) stream.cancel();
                return handle;
              } catch (error) {
                streams.delete(handle);
                invocations.delete(handle);
                throw error;
              }
            }),
        }),
        prompt: Object.freeze({
          section: (definition) => {
            const { text: value, ...metadata } = definition;
            return register('section', metadata, text(value));
          },
          variable: (name, value) => register('variable', { name }, text(value)),
          context: (definition) => {
            const { text: value, ...metadata } = definition;
            return register('context', metadata, text(value));
          },
        }),
        services: Object.freeze({
          provide: async (name, handler) => {
            const id = callback(handler);
            let handle;
            try {
              handle = await host('service.provide', { name, callback: id });
            } catch (error) {
              callbacks.delete(id);
              throw error;
            }
            let closing;
            return Object.freeze({
              close() {
                closing ??= host('contribution.release', { handle });
                return closing;
              },
            });
          },
          get: (name) => getService(name),
        }),
        storage: Object.freeze({
          read: (key) => host('storage.read', { key }),
          batch: (mutations) => host('storage.batch', { mutations }),
        }),
        credentials: Object.freeze({
          read: (key) => host('credentials.read', { key }),
          write: (input) => host('credentials.write', input),
        }),
        executions: Object.freeze({
          submit: (input) => host('execution.submit', input),
          createChild: (input) => host('execution.createChild', input),
          workspacePatch: (operationId) => host('execution.workspacePatch', { operationId }),
          query: (operationId) => host('execution.query', { operationId }),
          cancel: (operationId) => host('execution.cancel', { operationId }),
          events: (input) => host('execution.events', input),
          event: (input) => host('execution.event', input),
        }),
        sleep: (milliseconds) => host('clock.sleep', { milliseconds }),
        effect(dispose) {
          if (phase === 'retired' || typeof dispose !== 'function' || cleanups.length >= 128) {
            throw new Error('Invalid or retired cleanup registration');
          }
          cleanups.push(dispose);
        },
        run(task) {
          if (phase !== 'loading' || typeof task !== 'function' || jobs.length >= 64) {
            throw new Error('Business tasks must be staged during activation');
          }
          jobs.push(task);
        },
      });
      const dispose = await activate(context, config);
      if (phase !== 'loading') throw new Error('Plugin retired during initialization');
      if (dispose !== undefined) context.effect(dispose);
      phase = 'prepared';
      return registrations;
    },
    async invoke(id, input, call, invocation) {
      if (phase === 'retired') throw new Error('Plugin retired');
      const fn = callbacks.get(id);
      if (!fn) throw new Error('Plugin callback no longer exists');
      let abort;
      let aborted = false;
      const cancelled = new Promise((resolve) => {
        abort = resolve;
      });
      const cancel = () => {
        aborted = true;
        abort();
      };
      if (invocation !== undefined) invocations.set(invocation, cancel);
      const callSignal = Object.freeze({
        get aborted() {
          return aborted || signal.aborted;
        },
        wait: () => Promise.race([cancelled, stopping]),
        throwIfAborted() {
          if (this.aborted) throw new Error('Call cancelled');
        },
      });
      try {
        const context = { ...call, signal: callSignal };
        if (call?.authority) {
          context.services = Object.freeze({ get: (name) => getService(name, call.authority) });
          context.processes = processes(call.authority);
          context.terminals = terminals(call.authority);
          context.http = http(call.authority);
          context.files = files(call.authority);
          context.llm = Object.freeze({
            generate: (input) => host('llm.generate', { authority: call.authority, input }),
          });
          context.clients = Object.freeze({
            tools: () => host('clients.tools', { authority: call.authority }),
            call: (input) => host('clients.call', { authority: call.authority, call: input }),
          });
        }
        if (call?.executor) {
          context.emit = (output) => host('executor.emit', { handle: call.executor, output });
        }
        return await tracked(() => fn(input, Object.freeze(context)));
      } finally {
        if (invocation !== undefined) invocations.delete(invocation);
      }
    },
    cancel(invocation) {
      invocations.get(invocation)?.();
    },
    async streamNext(handle) {
      const stream = streams.get(handle);
      if (!stream?.value || stream.closing || stream.pending)
        throw new Error('Client stream is closed or busy');
      const pending = tracked(() => stream.value.next());
      stream.pending = pending;
      try {
        const result = await pending;
        if (
          !result ||
          (result.done !== undefined && typeof result.done !== 'boolean') ||
          (!result.done && result.value === undefined)
        ) {
          throw new Error('Client stream returned an invalid iterator result');
        }
        return result.done ? { kind: 'end' } : { kind: 'item', value: result.value };
      } finally {
        stream.pending = undefined;
      }
    },
    streamClose: closeStream,
    release(callback) {
      callbacks.delete(callback);
    },
    async effective() {
      if (phase !== 'prepared') throw new Error('Invalid effective transition');
      phase = 'active';
      const running = jobs.map((task) => tracked(task));
      jobs.length = 0;
      await Promise.all(running);
    },
    async retire() {
      phase = 'retired';
      signalStop();
      for (const stream of streams.values()) stream.cancel();
      await Promise.allSettled([...active]);
    },
    async dispose() {
      phase = 'retired';
      signalStop();
      const errors = [];
      for (const handle of streams.keys()) {
        try {
          await closeStream(handle);
        } catch (error) {
          errors.push(String(error));
        }
      }
      while (cleanups.length) {
        try {
          await cleanups.pop()();
        } catch (error) {
          errors.push(String(error));
        }
      }
      callbacks.clear();
      registrations.length = 0;
      if (errors.length) throw new Error(errors.join('; '));
    },
  });
};
