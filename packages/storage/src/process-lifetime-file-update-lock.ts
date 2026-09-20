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

import {
  openStableNativeLockFile,
  releaseNativeFileLock,
  tryAcquireNativeFileLock,
} from './native-file-lock.js';

const LOCK_POLL_MS = 25;
const LOCK_TIMEOUT_MS = 10_000;
const lockGates = new Map<string, Promise<void>>();

/**
 * The callback may pass the lease fd as an extra child stdio descriptor. The
 * advisory lock then survives a parent crash until that exact child exits.
 */
export async function withProcessLifetimeFileUpdateLock<T>(
  targetPath: string,
  operation: (inheritableLeaseFd: number) => Promise<T>,
  timeoutMs: number = LOCK_TIMEOUT_MS,
): Promise<T> {
  const deadline = Date.now() + timeoutMs;
  const leasePath = `${targetPath}.lease`;
  return runWithLockGate(leasePath, deadline, async () => {
    const lease = await openStableNativeLockFile(leasePath);
    let leased = false;
    try {
      while (!(leased = tryAcquireNativeFileLock(lease))) {
        await waitForLockTurn(leasePath, deadline);
      }
      return await operation(lease.fd);
    } finally {
      if (leased) releaseNativeFileLock(lease);
      await lease.close();
    }
  });
}

async function runWithLockGate<T>(
  lockPath: string,
  deadline: number,
  operation: () => Promise<T>,
): Promise<T> {
  const previous = lockGates.get(lockPath);
  let release!: () => void;
  const current = new Promise<void>((resolve) => {
    release = resolve;
  });
  lockGates.set(lockPath, current);
  try {
    if (previous) await waitForGate(previous, lockPath, deadline);
    return await operation();
  } finally {
    release();
    if (lockGates.get(lockPath) === current) lockGates.delete(lockPath);
  }
}

async function waitForGate(
  previous: Promise<void>,
  lockPath: string,
  deadline: number,
): Promise<void> {
  const remaining = deadline - Date.now();
  if (remaining <= 0) throw lockTimeout(lockPath);
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    await Promise.race([
      previous.catch(() => undefined),
      new Promise<never>((_resolve, reject) => {
        timer = setTimeout(() => reject(lockTimeout(lockPath)), remaining);
      }),
    ]);
  } finally {
    if (timer !== undefined) clearTimeout(timer);
  }
}

async function waitForLockTurn(lockPath: string, deadline: number): Promise<void> {
  if (Date.now() >= deadline) throw lockTimeout(lockPath);
  await new Promise<void>((resolve) => setTimeout(resolve, LOCK_POLL_MS));
}

function lockTimeout(lockPath: string): Error {
  return new Error(`File update is locked by another process (${lockPath})`);
}
