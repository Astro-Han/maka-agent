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

import { execFile, spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { promisify } from 'node:util';
import {
  connectOrSpawnRuntimeHostWithDependencies,
  createRuntimeHostCandidateLaunchBarrierWithDependencies,
  type RuntimeHostCandidateLaunchBarrier,
  type RuntimeHostCandidateLaunchBarrierDependencies,
} from '@maka/runtime-host/client';

const run = promisify(execFile);
type Launch = RuntimeHostCandidateLaunchBarrierDependencies['launchCandidate'];
type Exit = {
  code: number | null;
  signal: NodeJS.Signals | null;
  stderr: string;
  stderrTruncated: boolean;
};

export async function initializeNativeRuntimeHost(
  executable: string,
  rootPath: string,
  signal?: AbortSignal,
): Promise<void> {
  await run(executable, ['host', 'init', '--root', rootPath], {
    maxBuffer: 4096,
    timeout: 20_000,
    windowsHide: true,
    signal,
  });
}

/** Only the process launcher changes; discovery, election and client protocol stay shared. */
export function createNativeRuntimeHostCandidateLaunchBarrier(
  executable: string,
): RuntimeHostCandidateLaunchBarrier {
  return createRuntimeHostCandidateLaunchBarrierWithDependencies({
    retireTimeoutMs: 1000,
    launchCandidate: (input) => launchCandidate(executable, input),
    async connect(input, launchCandidate) {
      await initializeNativeRuntimeHost(executable, input.rootPath, input.signal);
      return connectOrSpawnRuntimeHostWithDependencies(input, {
        launchCandidate,
        random: Math.random,
      });
    },
  });
}

function launchCandidate(executable: string, input: Parameters<Launch>[0]): ReturnType<Launch> {
  if (input.inheritableAuthorityLeaseFd !== undefined || input.managedLaunchClaim !== undefined) {
    throw new Error('Native Host does not yet support updater leases or managed deployments');
  }
  const startupAttemptId = randomUUID();
  const args = [
    'host', 'candidate',
    '--root', input.rootPath,
    '--expected-root-id', input.expectedRootId,
    '--startup-attempt-id', startupAttemptId,
    '--owner-stdin',
  ];
  for (const [name, value] of [
    ['generation', input.generation],
    ['initial-connection-timeout-ms', input.initialConnectionTimeoutMs],
    ['idle-grace-ms', input.idleGraceMs],
    ['handshake-timeout-ms', input.handshakeTimeoutMs],
  ] as const) {
    if (value !== undefined) args.push(`--${name}`, String(value));
  }
  // No Node IPC: Windows libuv IPC is framed, not a portable JSON pipe.
  // The registration PID is the real Host PID, with no wrapper process.
  const child = spawn(executable, args, {
    stdio: ['pipe', 'ignore', 'pipe'],
    windowsHide: true,
    env: { ...process.env, ...input.env },
  });
  child.stdin.on('error', () => {
    // A Host that has already exited can close the owner pipe before release.
  });
  let stderr: Buffer<ArrayBufferLike> = Buffer.alloc(0);
  let stderrTruncated = false;
  child.stderr.on('data', (chunk: Buffer) => {
    const combined = Buffer.concat([stderr, chunk]);
    stderrTruncated ||= combined.length > 4096;
    stderr = Buffer.from(combined.subarray(-4096));
  });
  const exited = new Promise<Exit>((resolve) => {
    child.once('close', (code, signal) => {
      resolve({ code, signal, stderr: stderr.toString('utf8'), stderrTruncated });
      try {
        input.onExit?.({ pid: child.pid, code, signal });
      } catch {
        // Diagnostics must not affect settlement.
      }
    });
  });
  const spawned = new Promise<number>((resolve, reject) => {
    child.once('error', reject);
    child.once('spawn', () => {
      if (child.pid === undefined) reject(new Error('Native Host did not receive a process id'));
      else resolve(child.pid);
    });
  });
  let released = false;
  return {
    spawned: spawned.then((pid) => {
      child.unref();
      (child.stdin as typeof child.stdin & { unref?: () => void }).unref?.();
      (child.stderr as typeof child.stderr & { unref?: () => void }).unref?.();
      return {
        pid,
        startupAttemptId,
        exited,
        startupFailure: exited.then(({ code }) =>
          code === 70 ? { reason: 'internal_startup_failure' as const, startupAttemptId } : undefined,
        ),
        releaseToEnvironment() {
          if (released) return;
          released = true;
          child.stdin.end('{"kind":"runtime-host-launch-owner-release"}\n');
        },
        async settle(timeoutMs) {
          child.stdin.end();
          const result = await within(exited, timeoutMs);
          // Recovery may start committed work before discovery becomes visible.
          // EOF requests drain; a deadline cannot authorize killing its effects.
          if (result === undefined) {
            // The barrier must retain this attempt and keep the current owner
            // running until the candidate can no longer become a late winner.
            throw new Error('Native Runtime Host candidate has not finished draining');
          }
          return result.code === 0 && result.signal === null;
        },
      };
    }),
  };
}

async function within<T>(operation: Promise<T>, timeoutMs: number): Promise<T | undefined> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      operation,
      new Promise<undefined>((resolve) => {
        timer = setTimeout(() => resolve(undefined), timeoutMs);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}
