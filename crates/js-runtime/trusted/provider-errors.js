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

const codes = new Set([
  'context_length_exceeded',
  'model_context_window_exceeded',
  'request_too_large',
]);
const record = (value) => value !== null && typeof value === 'object' && !Array.isArray(value);

// Called only for SDK provider failures, never for local emit/normalization errors.
export function isContextOverflow(error, kind) {
  if (!record(error) || error.name === 'AbortError') return false;
  if (error instanceof Error && error.name !== 'AI_APICallError') return false;
  const candidates = [];
  const collect = (value) => {
    if (!record(value)) return;
    candidates.push(value);
    if (record(value.error)) candidates.push(value.error);
    if (value.type === 'response.failed' && record(value.response?.error)) {
      candidates.push(value.response.error);
    }
  };
  collect(error);
  collect(error.data);
  if (typeof error.responseBody === 'string' && error.responseBody.length <= 64 * 1024) {
    try {
      collect(JSON.parse(error.responseBody));
    } catch {
      /* No structured evidence. */
    }
  }
  if (candidates.some((value) => codes.has(value.code) || codes.has(value.type))) return true;
  if (kind !== 'anthropic' || error.statusCode !== 400) return false;
  return candidates.some((value) => {
    if (value.type !== 'invalid_request_error' || typeof value.message !== 'string') return false;
    const match = /^prompt is too long: ([0-9]{1,16}) tokens > ([0-9]{1,16}) maximum$/.exec(
      value.message,
    );
    if (!match) return false;
    const input = Number(match[1]),
      maximum = Number(match[2]);
    return (
      Number.isSafeInteger(input) && Number.isSafeInteger(maximum) && maximum > 0 && input > maximum
    );
  });
}

export function observesOutput(part) {
  if (
    part.providerMetadata != null &&
    (!record(part.providerMetadata) || Object.keys(part.providerMetadata).length > 0)
  )
    return true;
  if (part.type === 'stream-start' || part.type === 'raw') return false;
  if (part.type === 'response-metadata') {
    return Object.keys(part).some(
      (key) => !['type', 'id', 'modelId', 'timestamp', 'providerMetadata'].includes(key),
    );
  }
  return true;
}

// SDK throws and error parts converge before V8's exception display loses codes.
// Local emit failures stay outside these catches, preserving cancellation/limits.
export async function forwardProviderStream(open, normalize, emit, kind) {
  let observedOutput = false;
  const failed = async (error) => {
    if (!isContextOverflow(error, kind)) throw error;
    await emit({ type: 'error', error: { kind: 'context_overflow', observedOutput } });
  };
  let result;
  try {
    result = await open();
  } catch (error) {
    return failed(error);
  }
  const iterator = result.stream[Symbol.asyncIterator]();
  try {
    while (true) {
      let next;
      try {
        next = await iterator.next();
      } catch (error) {
        return await failed(error);
      }
      if (next.done) return;
      const raw = next.value;
      if (raw.type === 'error') return await failed(raw.error);
      observedOutput ||= observesOutput(raw);
      const part = normalize(raw);
      if (!part) continue;
      if (part.type === 'response-metadata' && part.timestamp instanceof Date) {
        part.timestamp = part.timestamp.toISOString();
      }
      await emit(part);
    }
  } finally {
    // Match for-await cleanup without replacing a known failure with cancel noise.
    await iterator.return?.().catch(() => {});
  }
}
