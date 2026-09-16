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

export type NativeRuntimeHostOperator =
  | { readonly kind: 'local'; readonly executable: string }
  | { readonly kind: 'ssh'; readonly destination: string; readonly sshPort?: number;
      readonly operator: RuntimeHostNativeOperatorCommand }
  | { readonly kind: 'wsl'; readonly distribution: string;
      readonly operator: RuntimeHostNativeOperatorCommand<'posix'> };

/** Keep draining after an observation timeout; a lost response is not rollback. */
export function runNativeRuntimeHostCommand(
  target: NativeRuntimeHostOperator,
  args: readonly string[],
  readOnly = false,
): Promise<string> {
  const command = invocation(target, args);
  return new Promise((resolve, reject) => {
    const child = spawn(command.executable, command.args, {
      windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'],
    });
    const output: Buffer[] = [];
    let bytes = 0;
    let errorText = '';
    // Bound the caller's wait without killing an in-flight durable mutation.
    // Its listeners keep draining; later activation checks native authority.
    const timer = setTimeout(() => {
      if (readOnly) child.kill();
      reject(new Error(readOnly
        ? 'Native Host query timed out.'
        : 'Native Host operation timed out; its outcome is unknown.'));
    }, readOnly ? 15_000 : 120_000);
    child.stdout.on('data', (chunk: Buffer) => {
      bytes += chunk.length;
      if (bytes <= 256 * 1024) output.push(chunk);
    });
    child.stderr.setEncoding('utf8');
    child.stderr.on('data', (chunk: string) => { errorText = (errorText + chunk).slice(-8192); });
    child.once('error', (error) => {
      clearTimeout(timer);
      reject(error);
    });
    child.once('close', (code) => {
      clearTimeout(timer);
      if (code !== 0 || bytes > 256 * 1024) {
        reject(new Error(`Native Host operation was not confirmed; refresh status before retrying. ${errorText.trim()}`));
      } else resolve(Buffer.concat(output).toString('utf8'));
    });
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
