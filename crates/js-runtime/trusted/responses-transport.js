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

// Rust owns sockets and their Turn-scoped lifetime; the SDK still owns event
// decoding. This adapter presents Responses JSON frames as an SSE fetch body.
export function responsesFetch(httpFetch, requestId) {
  return async (input, init) => {
    if (!Deno.core.ops.op_responses_enabled(requestId)) return httpFetch(input, init);
    const request = new Request(input, init);
    if (request.method !== 'POST' || !new URL(request.url).pathname.endsWith('/responses')) {
      return httpFetch(request);
    }
    const body = await request.clone().json();
    if (body.stream !== true) return httpFetch(request);
    const ops = Deno.core.ops;
    const fallback = await ops.op_responses_start(
      requestId,
      request.url,
      Object.fromEntries(request.headers),
      body,
    );
    if (fallback !== null) {
      request.headers.delete('content-length');
      return httpFetch(new Request(request, { body: JSON.stringify(fallback) }));
    }
    let ended = false;
    const encoder = new TextEncoder();
    return new Response(
      new ReadableStream(
        {
          async pull(controller) {
            if (ended) return;
            try {
              const frame = await ops.op_responses_next(requestId);
              if (frame === null) {
                ended = true;
                controller.close();
              } else {
                controller.enqueue(encoder.encode(`data: ${frame}\n\n`));
              }
            } catch (error) {
              ended = true;
              ops.op_responses_close(requestId);
              controller.error(error);
            }
          },
          cancel() {
            ended = true;
            ops.op_responses_close(requestId);
          },
        },
        { highWaterMark: 0 },
      ),
      { headers: { 'content-type': 'text/event-stream' } },
    );
  };
}
