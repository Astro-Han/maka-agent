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

/** Observation ends at one monotonic deadline. Expiry does not undo effects. */
export class NativeHostWaitError extends Error {
  constructor(readonly phase: string, readonly reason: 'deadline' | 'stalled' | 'aborted') {
    super(`Native Host ${phase}: ${reason}; outcome may still be pending. Refresh status to confirm, retry recovery, or switch Host. No rollback was requested.`);
    this.name = 'NativeHostWaitError';
  }
}

export class NativeHostBudget {
  readonly deadline: number;
  constructor(timeoutMs = 120_000, readonly signal?: AbortSignal) {
    this.deadline = performance.now() + timeoutMs;
  }

  remaining(phase: string, limit = Number.POSITIVE_INFINITY): number {
    this.signal?.throwIfAborted();
    const remaining = Math.min(limit, this.deadline - performance.now());
    if (remaining <= 0) throw new NativeHostWaitError(phase, 'deadline');
    return Math.max(1, Math.ceil(remaining));
  }

  withSignal(signal: AbortSignal): NativeHostBudget {
    return new NativeHostBudget(Math.max(0, this.deadline - performance.now()),
      this.signal ? AbortSignal.any([this.signal, signal]) : signal);
  }

  async wait<T>(operation: Promise<T>, phase: string, limit?: number): Promise<T> {
    // Attach handlers even when the budget has expired: a detached failure is
    // still observed, while its resource owner retains responsibility for cleanup.
    void operation.catch(() => undefined);
    const remaining = this.remaining(phase, limit);
    let timer: ReturnType<typeof setTimeout> | undefined;
    let aborted: (() => void) | undefined;
    try {
      return await Promise.race([
        operation,
        new Promise<never>((_, reject) => {
          timer = setTimeout(() => reject(new NativeHostWaitError(phase, 'deadline')), remaining);
          aborted = () => reject(this.signal?.reason ?? new NativeHostWaitError(phase, 'aborted'));
          this.signal?.addEventListener('abort', aborted, { once: true });
          if (this.signal?.aborted) aborted();
        }),
      ]);
    } finally {
      clearTimeout(timer);
      if (aborted) this.signal?.removeEventListener('abort', aborted);
    }
  }
}
