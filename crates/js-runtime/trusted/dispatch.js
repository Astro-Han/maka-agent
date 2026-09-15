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

(() => {
  const requests = new Map();
  const terminals = new Map();
  globalThis.makaTrusted = {
    async model(id, request) {
      const controller = new AbortController();
      requests.set(id, controller);
      try {
        await MakaProvider.stream(
          request,
          (value) => Deno.core.ops.op_model_emit(id, value),
          controller.signal,
          id,
        );
      } finally {
        await Deno.core.ops.op_http_close(id);
        requests.delete(id);
      }
    },
    cancel(id) {
      requests.get(id)?.abort();
    },
    create(id, size) {
      terminals.set(id, createTerminal(size));
    },
    async terminal(id, operation, argument) {
      const terminal = terminals.get(id);
      if (!terminal) throw new Error('terminal parser closed');
      let result = await terminal[operation](argument);
      if (operation === 'write') result = { replies: result, screen: terminal.snapshot() };
      else if (operation === 'resize') result = terminal.snapshot();
      const json = JSON.stringify(result ?? null);
      if (Deno.core.byteLength(json) > 2 * 1024 * 1024)
        throw new Error('terminal snapshot budget exceeded');
      return json;
    },
    dispose(id) {
      const terminal = terminals.get(id);
      terminals.delete(id);
      terminal?.dispose();
    },
  };
})();
