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

export interface HttpRequest {
  url: string;
  method?: 'GET' | 'HEAD' | 'POST' | 'PUT' | 'PATCH' | 'DELETE' | 'OPTIONS';
  /** Repeated fields are allowed. Framing and proxy headers belong to Host. */
  headers?: readonly (readonly [name: string, value: string])[];
  /** UTF-8 strings or exact bytes, up to 1 MiB. */
  body?: string | Uint8Array | readonly number[];
}
export interface HttpResponse {
  readonly status: number;
  readonly url: string;
  /** Exact field bytes, including duplicate fields. */
  readonly headers: readonly (readonly [name: string, value: Uint8Array])[];
  /** One pending reader; at most 16 KiB per chunk. null is end of body. */
  next(): Promise<Uint8Array | null>;
  /** Idempotent. Cancels local I/O, not a server's accepted side effects. */
  close(): Promise<void>;
}
export interface Http {
  /**
   * Invocation-bound; requires admitted and current Bypass permission.
   * Uses Host proxy settings. No retries or redirects; HTTP error statuses are
   * returned normally. Cancels on call completion or plugin retirement.
   * A connection/read stall fails after 15/60 seconds respectively.
   */
  request(request: HttpRequest): Promise<HttpResponse>;
}
