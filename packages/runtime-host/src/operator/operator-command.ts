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

import { posix, win32 } from 'node:path';

const PATH_MAX_BYTES = 4 * 1024;

export type RuntimeHostOperatorPlatform = 'posix' | 'win32';

/** A stable operator entrypoint with no target-shell dependency. */
export interface RuntimeHostNodeOperatorCommand<
  Platform extends RuntimeHostOperatorPlatform = RuntimeHostOperatorPlatform,
> {
  readonly kind: 'node';
  readonly platform: Platform;
  readonly nodePath: string;
  readonly modulePath: string;
}

/** The native Maka CLI owns the complete host operator command tree. */
export interface RuntimeHostNativeOperatorCommand<
  Platform extends RuntimeHostOperatorPlatform = RuntimeHostOperatorPlatform,
> {
  readonly kind: 'native';
  readonly platform: Platform;
  readonly executablePath: string;
}

export type RuntimeHostOperatorCommand =
  | RuntimeHostNodeOperatorCommand
  | RuntimeHostNativeOperatorCommand;

export type RuntimeHostPosixOperatorCommand =
  | RuntimeHostNodeOperatorCommand<'posix'>
  | RuntimeHostNativeOperatorCommand<'posix'>;

export function createRuntimeHostOperatorCommand<
  Platform extends RuntimeHostOperatorPlatform,
>(input: {
  readonly platform: Platform;
  readonly nodePath: string;
  readonly modulePath: string;
}): RuntimeHostNodeOperatorCommand<Platform> {
  return Object.freeze({
    kind: 'node',
    platform: input.platform,
    nodePath: requireAbsolutePath(input.nodePath, input.platform, 'operator Node path'),
    modulePath: requireAbsolutePath(input.modulePath, input.platform, 'operator module path'),
  });
}

export function createRuntimeHostNativeOperatorCommand<
  Platform extends RuntimeHostOperatorPlatform,
>(input: {
  readonly platform: Platform;
  readonly executablePath: string;
}): RuntimeHostNativeOperatorCommand<Platform> {
  return Object.freeze({
    kind: 'native',
    platform: input.platform,
    executablePath: requireAbsolutePath(
      input.executablePath,
      input.platform,
      'operator executable',
    ),
  });
}

export function runtimeHostManagedOperatorCommand<Platform extends RuntimeHostOperatorPlatform>(
  deployment: {
    readonly deploymentRoot: string;
    readonly launch: { readonly nodePath: string };
  },
  platform: Platform,
): RuntimeHostNodeOperatorCommand<Platform> {
  return createRuntimeHostOperatorCommand({
    platform,
    nodePath: deployment.launch.nodePath,
    modulePath: runtimeHostManagedOperatorModulePath(deployment.deploymentRoot, platform),
  });
}

export function runtimeHostManagedOperatorModulePath(
  deploymentRoot: string,
  platform: RuntimeHostOperatorPlatform,
): string {
  const paths = platform === 'win32' ? win32 : posix;
  return paths.join(deploymentRoot, 'operator.mjs');
}

export function decodeRuntimeHostOperatorCommand(value: unknown): RuntimeHostOperatorCommand {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error('Runtime Host operator command is invalid');
  }
  const record = value as Record<string, unknown>;
  const keys = Object.keys(record).sort();
  if (record.kind === 'native') {
    const expected = ['executablePath', 'kind', 'platform'];
    if (keys.length !== expected.length || keys.some((key, index) => key !== expected[index])) {
      throw new Error('Runtime Host operator command has unexpected fields');
    }
    if (record.platform !== 'posix' && record.platform !== 'win32') {
      throw new Error('Runtime Host operator platform is invalid');
    }
    return createRuntimeHostNativeOperatorCommand({
      platform: record.platform,
      executablePath: requireString(record.executablePath, 'Runtime Host operator executable'),
    });
  }
  const expected = ['kind', 'modulePath', 'nodePath', 'platform'];
  if (keys.length !== expected.length || keys.some((key, index) => key !== expected[index])) {
    throw new Error('Runtime Host operator command has unexpected fields');
  }
  if (record.kind !== 'node') throw new Error('Runtime Host operator command kind is invalid');
  if (record.platform !== 'posix' && record.platform !== 'win32') {
    throw new Error('Runtime Host operator platform is invalid');
  }
  return createRuntimeHostOperatorCommand({
    platform: record.platform,
    nodePath: requireString(record.nodePath, 'Runtime Host operator Node path'),
    modulePath: requireString(record.modulePath, 'Runtime Host operator module path'),
  });
}

export function decodeRuntimeHostPosixOperatorCommand(
  value: unknown,
): RuntimeHostPosixOperatorCommand {
  const command = decodeRuntimeHostOperatorCommand(value);
  if (command.platform !== 'posix') {
    throw new Error('Runtime Host operator must target POSIX');
  }
  if (command.kind === 'native') {
    return createRuntimeHostNativeOperatorCommand({
      platform: 'posix',
      executablePath: command.executablePath,
    });
  }
  return createRuntimeHostOperatorCommand({
    platform: 'posix',
    nodePath: command.nodePath,
    modulePath: command.modulePath,
  });
}

export function runtimeHostOperatorInvocation(
  command: RuntimeHostOperatorCommand,
  args: readonly string[],
): { readonly executable: string; readonly args: readonly string[] } {
  const normalized = decodeRuntimeHostOperatorCommand(command);
  if (normalized.kind === 'native') {
    return { executable: normalized.executablePath, args: ['host', ...args] };
  }
  return {
    executable: normalized.nodePath,
    args: [normalized.modulePath, ...args],
  };
}

function requireAbsolutePath(
  value: string,
  platform: RuntimeHostOperatorPlatform,
  label: string,
): string {
  if (
    Buffer.byteLength(value, 'utf8') > PATH_MAX_BYTES ||
    /[\u0000-\u001f\u007f]/u.test(value) ||
    !(platform === 'win32' ? win32.isAbsolute(value) : posix.isAbsolute(value))
  ) {
    throw new Error(`Runtime Host ${label} must be an absolute ${platform} path`);
  }
  return value;
}

function requireString(value: unknown, label: string): string {
  if (typeof value !== 'string' || value.length === 0) throw new Error(`${label} is invalid`);
  return value;
}
