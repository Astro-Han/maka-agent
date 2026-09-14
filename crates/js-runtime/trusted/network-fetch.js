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

// No ambient proxy policy: Rust uses the same immutable routing for HTTP and WS.
const transportFailures = new WeakSet();
export const isTransportFailure = (error) => transportFailures.has(error);
function nativeFailure(error) {
  if (error instanceof Error && error.code === 'MAKA_HTTP_TRANSPORT') transportFailures.add(error);
  throw error;
}

export function networkFetch(id) {
  const ops = Deno.core.ops;
  return async (input, init) => {
    const request = new Request(input, init);
    request.signal.throwIfAborted();
    const abort = () => {
      void ops.op_http_close(id);
    };
    request.signal.addEventListener('abort', abort, { once: true });
    const cleanup = () => request.signal.removeEventListener('abort', abort);
    try {
      const body = new Uint8Array(await request.arrayBuffer());
      request.signal.throwIfAborted();
      const response = await ops
        .op_http_start(id, request.method, request.url, Object.fromEntries(request.headers), body)
        .catch(nativeFailure);
      if (!response.hasBody) {
        cleanup();
        await ops.op_http_close(id);
        return new Response(null, response);
      }
      return new Response(
        new ReadableStream(
          {
            async pull(controller) {
              try {
                const chunk = await ops.op_http_chunk(id).catch(nativeFailure);
                if (chunk === null) {
                  cleanup();
                  controller.close();
                } else controller.enqueue(chunk);
              } catch (error) {
                cleanup();
                await ops.op_http_close(id);
                controller.error(error);
              }
            },
            async cancel() {
              cleanup();
              await ops.op_http_close(id);
            },
          },
          { highWaterMark: 0 },
        ),
        response,
      );
    } catch (error) {
      cleanup();
      await ops.op_http_close(id);
      throw error;
    }
  };
}
