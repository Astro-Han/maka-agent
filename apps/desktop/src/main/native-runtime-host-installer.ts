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

import { randomUUID } from 'node:crypto';
import { posix, win32 } from 'node:path';
import { runtimeHostSshOperatorRemoteCommand } from '@maka/runtime-host/client';
import type { RuntimeHostNativeOperatorCommand } from '@maka/runtime-host/operator';
import {
  decodeNativeArtifact,
  decodeNativeSetup,
  nativeArtifactOperator,
  nativeImportArguments,
  nativeSetupArguments,
  NATIVE_ARTIFACT_PREFIX,
  NATIVE_SETUP_PREFIX,
  type NativeRuntimeHostPackage,
  type NativeRuntimeHostSetup,
} from './native-runtime-host-setup.js';

const STAGE_PREFIX = '__MAKA_NATIVE_HOST_STAGE__';
const CLEAN_PREFIX = '__MAKA_NATIVE_HOST_CLEAN__';

export interface NativeSetupCommand<T> {
  readonly command: string;
  readonly prefix: string;
  readonly decode: (line: string) => T;
  readonly signal?: AbortSignal;
  readonly timeoutMs: number;
}
export interface NativeSetupTransport {
  readonly platform: 'posix' | 'win32';
  execute<T>(command: NativeSetupCommand<T>): Promise<T>;
  upload(source: string, destination: string, signal?: AbortSignal): Promise<void>;
}
export interface NativeSetupInput {
  readonly package: NativeRuntimeHostPackage;
  /** Explicit native root for trusted operator callers; never forwarded from renderer input. */
  readonly rootPath?: string;
  readonly principalId?: string;
  readonly projectDirectoryRoots?: readonly { readonly label: string; readonly path: string }[];
  readonly signal?: AbortSignal;
}
export interface NativeSetupResult {
  readonly receipt: NativeRuntimeHostSetup;
  readonly operator: RuntimeHostNativeOperatorCommand;
}

/** Transfer and verify code before starting any State Root mutation. */
export async function prepareNativeRuntimeHost(
  transport: NativeSetupTransport,
  input: Pick<NativeSetupInput, 'package' | 'signal'>,
): Promise<RuntimeHostNativeOperatorCommand> {
  input.signal?.throwIfAborted();
  const paths = transport.platform === 'win32' ? win32 : posix;
  const name = `maka-native-${randomUUID().replaceAll('-', '')}`;
  const stage = await transport.execute({
    command: stageCommand(transport.platform, name),
    prefix: STAGE_PREFIX,
    timeoutMs: 30_000,
    signal: input.signal,
    decode: (line) => {
      const path = line.slice(STAGE_PREFIX.length).trimEnd();
      if (
        !paths.isAbsolute(path) ||
        paths.basename(path) !== name ||
        /[\u0000-\u001f\u007f]/u.test(path)
      ) {
        throw new Error('Native setup returned an invalid staging path');
      }
      return path;
    },
  });
  let artifact;
  try {
    const source = paths.join(stage, 'package');
    await transport.upload(input.package.artifact.directory, source, input.signal);
    const bootstrap: RuntimeHostNativeOperatorCommand = {
      kind: 'native',
      platform: transport.platform,
      executablePath: paths.join(
        source,
        'bin',
        transport.platform === 'win32' ? 'maka.exe' : 'maka',
      ),
    };
    const importCommand = runtimeHostSshOperatorRemoteCommand(
      bootstrap,
      nativeImportArguments(input.package, source),
    );
    artifact = await transport.execute({
      // A Windows downloader cannot preserve POSIX executable mode through SCP.
      command:
        transport.platform === 'win32'
          ? importCommand
          : 'sh -c ' +
            quoteNativePosix(
              `chmod u+x ${quoteNativePosix(bootstrap.executablePath)} && ${importCommand}`,
            ),
      prefix: NATIVE_ARTIFACT_PREFIX,
      timeoutMs: 190_000,
      signal: input.signal,
      decode: (line) =>
        decodeNativeArtifact(
          JSON.parse(line.slice(NATIVE_ARTIFACT_PREFIX.length)),
          input.package.artifact,
          transport.platform,
        ),
    });
    if (artifact.integrity !== input.package.artifact.integrity) {
      throw new Error('Transferred native artifact identity changed');
    }
  } finally {
    // This exact nonce directory is owned by this attempt. Do not bind cleanup
    // to the cancelled caller, or let a cleanup failure look like a rollback.
    await transport.execute({
      command: cleanupCommand(transport.platform, stage),
      prefix: CLEAN_PREFIX,
      timeoutMs: 30_000,
      decode: (line) => {
        if (line.slice(CLEAN_PREFIX.length).trim() !== 'ok')
          throw new Error('Native setup cleanup was not confirmed');
        return true;
      },
    });
  }
  input.signal?.throwIfAborted();
  return nativeArtifactOperator(artifact);
}

export async function installNativeRuntimeHost(
  transport: NativeSetupTransport,
  input: NativeSetupInput,
  onCommit: () => void,
): Promise<NativeSetupResult> {
  const operator = await prepareNativeRuntimeHost(transport, input);
  // After this point cancellation cannot promise to undo installation. Finish
  // reading its bounded receipt, then let the existing pairing journal take over.
  onCommit();
  const receipt = await transport.execute({
    command: runtimeHostSshOperatorRemoteCommand(operator, nativeSetupArguments(input)),
    prefix: NATIVE_SETUP_PREFIX,
    timeoutMs: 120_000,
    decode: (line) => {
      try {
        return decodeNativeSetup(
          JSON.parse(line.slice(NATIVE_SETUP_PREFIX.length)),
          Boolean(input.principalId),
        );
      } catch {
        throw new Error('Native setup returned an invalid receipt');
      }
    },
  });
  return { receipt, operator };
}

export function quoteNativePosix(value: string): string {
  return "'" + value.replaceAll("'", "'\"'\"'") + "'";
}
function powershell(script: string): string {
  return (
    'powershell.exe -NoLogo -NoProfile -NonInteractive -EncodedCommand ' +
    Buffer.from(script, 'utf16le').toString('base64')
  );
}
function quotePowerShell(value: string): string {
  return "'" + value.replaceAll("'", "''") + "'";
}

function stageCommand(platform: 'posix' | 'win32', name: string): string {
  if (platform === 'win32')
    return powershell(
      [
        "$ErrorActionPreference='Stop'",
        `$path=[IO.Path]::Combine([IO.Path]::GetTempPath(),'${name}')`,
        'if([IO.Directory]::Exists($path)){throw "Native stage already exists"}',
        '$sid=[Security.Principal.WindowsIdentity]::GetCurrent().User',
        '$acl=New-Object Security.AccessControl.DirectorySecurity',
        '$acl.SetOwner($sid)',
        '$acl.SetAccessRuleProtection($true,$false)',
        "$rule=New-Object Security.AccessControl.FileSystemAccessRule($sid,'FullControl','ContainerInherit,ObjectInherit','None','Allow')",
        '$acl.AddAccessRule($rule)',
        '[void][IO.Directory]::CreateDirectory($path,$acl)',
        `[Console]::WriteLine('${STAGE_PREFIX}'+$path)`,
      ].join('; '),
    );
  // The random, fixed basename allows exact cleanup without trusting an
  // arbitrary path returned by a remote shell or expanding a broad variable.
  const path = '/tmp/' + name;
  return (
    'sh -c ' +
    quoteNativePosix(
      `umask 077; mkdir ${quoteNativePosix(path)} || exit 1; printf '%s\\n' ${quoteNativePosix(STAGE_PREFIX + path)}`,
    )
  );
}
function cleanupCommand(platform: 'posix' | 'win32', path: string): string {
  if (platform === 'win32')
    return powershell(
      `$ErrorActionPreference='Stop'; Remove-Item -LiteralPath ${quotePowerShell(path)} -Recurse -Force; [Console]::WriteLine('${CLEAN_PREFIX}ok')`,
    );
  return (
    'sh -c ' +
    quoteNativePosix(`rm -rf -- ${quoteNativePosix(path)} && printf '%s\\n' '${CLEAN_PREFIX}ok'`)
  );
}
