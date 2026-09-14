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

const MAX_RESPONSE_BYTES = 8 * 1024 * 1024;

export class ProviderResponseLimitError extends Error {
  name = 'ProviderResponseLimitError';
}

// Count decoded transport bytes before TextDecoder/JSON/SSE parsers can retain
// an unbounded line or response body. This is a per-response parsing boundary,
// not an OS memory limit or a total-output cap on a long-running model stream.
export async function boundedFetch(input, init, fetch = globalThis.fetch) {
  const response = await fetch(input, init);
  if (!response.body) return response;
  const sse =
    response.ok &&
    response.headers.get('content-type')?.split(';', 1)[0].trim().toLowerCase() ===
      'text/event-stream';
  const length = Number(response.headers.get('content-length'));
  const failure = () =>
    new ProviderResponseLimitError(
      sse ? 'provider SSE record exceeds 8 MiB' : 'provider response body exceeds 8 MiB',
    );
  if (!sse && Number.isFinite(length) && length > MAX_RESPONSE_BYTES) {
    await response.body.cancel();
    throw failure();
  }
  let bytes = 0;
  let emptyLine = true;
  let carriageReturn = false;
  const count = (length) => {
    bytes += length;
    if (bytes > MAX_RESPONSE_BYTES) throw failure();
  };
  const endLine = () => {
    if (emptyLine) bytes = 0;
    emptyLine = true;
  };
  const body = response.body.pipeThrough(
    new TransformStream({
      transform(chunk, controller) {
        if (!sse) {
          count(chunk.byteLength);
        } else {
          for (const byte of chunk) {
            // CRLF is one line ending, including when split across reads.
            // A lone CR also ends a line; do not reset on every data line,
            // because a single event may contain many data: lines.
            if (carriageReturn) {
              carriageReturn = false;
              if (byte === 10) {
                count(1);
                endLine();
                continue;
              }
              endLine();
            }
            count(1);
            if (byte === 13) carriageReturn = true;
            else if (byte === 10) endLine();
            else emptyLine = false;
          }
        }
        controller.enqueue(chunk);
      },
    }),
  );
  return new Response(body, {
    status: response.status,
    statusText: response.statusText,
    headers: response.headers,
  });
}
