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
  const load = Deno.core.loadExtScript;
  const install = (path, names) => {
    const exports = load(path);
    for (const name of names) {
      Object.defineProperty(globalThis, name, {
        value: exports[name],
        configurable: true,
        writable: true,
      });
    }
  };
  install('ext:deno_web/01_dom_exception.js', ['DOMException']);
  install('ext:deno_web/02_event.js', ['Event', 'EventTarget']);
  install('ext:deno_web/03_abort_signal.js', ['AbortController', 'AbortSignal']);
  install('ext:deno_web/02_timers.js', [
    'setTimeout',
    'clearTimeout',
    'setInterval',
    'clearInterval',
  ]);
  install('ext:deno_web/08_text_encoding.js', [
    'TextEncoder',
    'TextDecoder',
    'TextEncoderStream',
    'TextDecoderStream',
  ]);
  install('ext:deno_web/06_streams.js', [
    'ReadableStream',
    'WritableStream',
    'TransformStream',
    'ByteLengthQueuingStrategy',
    'CountQueuingStrategy',
  ]);
  install('ext:deno_web/00_url.js', ['URL', 'URLSearchParams']);
  install('ext:deno_web/05_base64.js', ['atob', 'btoa']);
  install('ext:deno_web/09_file.js', ['Blob', 'File']);
  install('ext:deno_fetch/20_headers.js', ['Headers']);
  install('ext:deno_fetch/21_formdata.js', ['FormData']);
  install('ext:deno_fetch/23_request.js', ['Request']);
  install('ext:deno_fetch/23_response.js', ['Response']);
  install('ext:deno_fetch/26_fetch.js', ['fetch']);
  install('ext:deno_crypto/00_crypto.js', ['crypto']);
  globalThis.self = globalThis;
  globalThis.window = globalThis;
  globalThis.navigator = { platform: '', userAgent: 'Maka Headless' };
  globalThis.performance = { now: Deno.core.ops.op_timer_now };
})();
