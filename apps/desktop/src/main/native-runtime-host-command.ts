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


import { spawn } from 'node:child_process';
import {
  normalizeRuntimeHostSshDestination,
  normalizeRuntimeHostWslDistribution,
  resolveSystemRuntimeHostWslExecutable,
  runtimeHostSshOperatorRemoteCommand,
} from '@maka/runtime-host/client';
import { runtimeHostOperatorInvocation, type RuntimeHostNativeOperatorCommand } from '@maka/runtime-host/operator';
import { NativeHostBudget, NativeHostWaitError } from './native-runtime-host-operation.js';
import { nativeHostProgressSchema } from '../shared/native-runtime-host-management.js';

export type NativeRuntimeHostOperator =
  | { readonly kind: 'local'; readonly executable: string }
  | { readonly kind: 'ssh'; readonly destination: string; readonly sshPort?: number;
      readonly operator: RuntimeHostNativeOperatorCommand }
  | { readonly kind: 'wsl'; readonly distribution: string;
      readonly operator: RuntimeHostNativeOperatorCommand<'posix'> };

export class NativeHostBusyError extends Error {
  readonly code = 'EAGAIN';
  constructor() { super('Another Host operation still holds the executor. No authority was taken; refresh status or retry later.'); }
}

export class NativeHostCommandUnconfirmedError extends Error {}

/** The path comes from an authenticated deployment receipt, never renderer input. */
export function nativeOperatorAt(target: NativeRuntimeHostOperator, executable: string): NativeRuntimeHostOperator {
  switch (target.kind) {
    case 'local': return { ...target, executable };
    case 'ssh': return { ...target, operator: { ...target.operator, executablePath: executable } };
    case 'wsl': return { ...target, operator: { ...target.operator, executablePath: executable } };
  }
}

/** Keep draining after an observation timeout; a lost response is not rollback. */
export function runNativeRuntimeHostCommand(
  target: NativeRuntimeHostOperator,
  args: readonly string[],
  readOnly = false,
  budget = new NativeHostBudget(readOnly ? 15_000 : 120_000),
  onProgress?: (progress: import('../shared/native-runtime-host-management.js').NativeHostProgress) => void,
  interactive?: (args: readonly string[], timeoutMs: number) => Promise<string>,
): Promise<string> {
  const report = (progress: import('../shared/native-runtime-host-management.js').NativeHostProgress) => {
    try { onProgress?.(progress); } catch { /* Observers do not own the operation. */ }
  };
  const phase = args[0] ?? 'command';
  const available = budget.remaining(phase, readOnly ? 15_000 : phase === 'fetch' ? 150_000 : 60_000);
  // Reserve confirmation time inside, never after, the original deadline.
  const remaining = readOnly || phase === 'fetch' ? available : Math.max(1, available - 2_000);
  const boundedArgs = [...args, '--timeout-ms', String(remaining)];
  if (interactive) {
    report({ phase: phase === 'activate' ? 'activate' : 'stage' });
    return budget.wait(interactive(boundedArgs, remaining), phase, remaining).catch((error) => {
      report({ phase: 'confirmation_pending' });
      throw error;
    });
  }
  const command = invocation(target, boundedArgs);
  const operation = new Promise<string>((resolve, reject) => {
    const child = spawn(command.executable, command.args, {
      windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'],
    });
    const output: Buffer[] = [];
    let bytes = 0;
    let errorText = '';
    let busy = false;
    // Bound the caller's wait without killing an in-flight durable mutation.
    // Its listeners keep draining; later activation checks native authority.
    let progressLine = '';
    const initial = nativeHostProgressSchema.safeParse({ phase });
    report(initial.success ? initial.data : { phase: 'stage' });
    let lastPhase = '';
    let lastCompleted = -1;
    let detached = false;
    let stallTimer: ReturnType<typeof setTimeout> | undefined;
    child.stdout.on('data', (chunk: Buffer) => {
      bytes += chunk.length;
      if (bytes <= 256 * 1024) output.push(chunk);
    });
    child.stderr.setEncoding('utf8');
    child.stderr.on('data', (chunk: string) => {
      errorText = (errorText + chunk).slice(-8192);
      progressLine = (progressLine + chunk).slice(-16_384);
      let newline: number;
      while ((newline = progressLine.indexOf('\n')) >= 0) {
        const line = progressLine.slice(0, newline);
        progressLine = progressLine.slice(newline + 1);
        if (line === 'MAKA_HOST_ERROR {"kind":"busy"}') busy = true;
        if (!line.startsWith('MAKA_HOST_PROGRESS ')) continue;
        try {
          const value = JSON.parse(line.slice('MAKA_HOST_PROGRESS '.length));
          const progress = nativeHostProgressSchema.safeParse(value);
          if (!progress.success) continue;
          if (detached) continue;
          report(progress.data);
          if (value.phase !== lastPhase || (value.completed !== undefined && value.completed > lastCompleted)) {
            lastPhase = value.phase;
            lastCompleted = value.completed ?? -1;
            clearTimeout(stallTimer);
            stallTimer = setTimeout(() => {
              detach();
              reject(new NativeHostWaitError(value.phase, 'stalled'));
            }, value.phase === 'download' ? 20_000 : 60_000);
          }
        } catch { /* Diagnostics never alter a command's outcome. */ }
      }
    });
    // Stop observing without killing a mutation or keeping Desktop alive. Pipes
    // continue draining while this process exists; the CLI owns its durable work.
    const detach = () => {
      detached = true;
      clearTimeout(stallTimer);
      if (readOnly) child.kill();
      child.unref();
      (child.stdout as typeof child.stdout & { unref?: () => void }).unref?.();
      (child.stderr as typeof child.stderr & { unref?: () => void }).unref?.();
    };
    const timer = setTimeout(detach, remaining);
    budget.signal?.addEventListener('abort', detach, { once: true });
    child.once('error', (error) => {
      clearTimeout(timer);
      clearTimeout(stallTimer);
      budget.signal?.removeEventListener('abort', detach);
      reject(error);
    });
    child.once('close', (code) => {
      clearTimeout(timer);
      clearTimeout(stallTimer);
      budget.signal?.removeEventListener('abort', detach);
      if (code !== 0 || bytes > 256 * 1024) {
        reject(busy ? new NativeHostBusyError() : new NativeHostCommandUnconfirmedError(`Native Host operation was not confirmed; refresh status before retrying. ${errorText.trim()}`));
      } else resolve(Buffer.concat(output).toString('utf8'));
    });
  });
  return budget.wait(operation, phase, remaining).catch((error) => {
    if (error instanceof NativeHostWaitError) report({ phase: 'confirmation_pending' });
    throw error;
  });
}

function invocation(target: NativeRuntimeHostOperator, args: readonly string[]) {
  switch (target.kind) {
    case 'local':
      return { executable: target.executable, args: ['host', ...args] };
    case 'wsl': {
      const command = runtimeHostOperatorInvocation(target.operator, args);
      return {
        executable: resolveSystemRuntimeHostWslExecutable(),
        args: ['--distribution', normalizeRuntimeHostWslDistribution(target.distribution),
          '--exec', command.executable, ...command.args],
      };
    }
    case 'ssh': {
      if (target.sshPort !== undefined &&
        (!Number.isSafeInteger(target.sshPort) || target.sshPort < 1 || target.sshPort > 65_535)) {
        throw new Error('Invalid SSH port');
      }
      return {
        executable: 'ssh',
        args: [
          '-T', '-o', 'BatchMode=yes', '-o', 'ControlMaster=no', '-o', 'ControlPath=none',
          '-o', 'ClearAllForwardings=yes', '-o', 'RemoteCommand=none', '-o', 'ConnectTimeout=15',
          '-o', 'ServerAliveInterval=15', '-o', 'ServerAliveCountMax=5',
          ...(target.sshPort === undefined ? [] : ['-p', String(target.sshPort)]),
          normalizeRuntimeHostSshDestination(target.destination),
          runtimeHostSshOperatorRemoteCommand(target.operator, args),
        ],
      };
    }
  }
}
