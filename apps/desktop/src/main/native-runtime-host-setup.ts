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

import { execFile } from 'node:child_process';
import { createHash } from 'node:crypto';
import { lstat, readFile } from 'node:fs/promises';
import { join, posix, win32 } from 'node:path';
import { promisify } from 'node:util';
import { z } from 'zod';
import {
  isProductReleaseVersion,
  type RuntimeHostNativeOperatorCommand,
} from '@maka/runtime-host/operator';
import {
  nativeRuntimeHostDeploymentSchema,
  nativeRuntimeHostIdentitySchema,
} from '../shared/native-runtime-host-deployment.js';
import type { RuntimeHostTargetIdentity } from './runtime-host-target.js';

const run = promisify(execFile);
export const NATIVE_ARTIFACT_PREFIX = '__MAKA_NATIVE_HOST_ARTIFACT__';
export const NATIVE_SETUP_PREFIX = '__MAKA_NATIVE_HOST_SETUP__';
const path = z
  .string()
  .min(1)
  .max(32_768)
  .refine((value) => !/[\u0000-\u001f\u007f]/u.test(value));
const artifactSchema = z
  .object({
    target: z.enum(['darwin-arm64', 'darwin-x64', 'linux-arm64-gnu', 'linux-x64-gnu', 'win32-x64']),
    version: z.string().min(1).max(128),
    directory: path,
    executable: path,
    serviceExecutable: path.optional(),
    integrity: z.string().regex(/^sha512-[A-Za-z0-9+/]{86}==$/u),
  })
  .strict();
export type NativeRuntimeHostArtifact = z.infer<typeof artifactSchema>;
export interface NativeRuntimeHostPackage {
  readonly artifact: NativeRuntimeHostArtifact;
  readonly receiptSha256: string;
}
const setupSchema = z
  .object({
    deployment: nativeRuntimeHostDeploymentSchema,
    host: nativeRuntimeHostIdentitySchema,
    pairing: z
      .object({
        rootId: z.string().regex(/^[a-f0-9]{64}$/u),
        credentialId: z.string().min(1).max(128),
        credential: z
          .string()
          .min(1)
          .max(16 * 1024),
      })
      .strict()
      .optional(),
  })
  .strict();
export type NativeRuntimeHostSetup = z.infer<typeof setupSchema>;

export function nativeRuntimeHostTarget(
  identity: RuntimeHostTargetIdentity,
): NativeRuntimeHostArtifact['target'] {
  if (identity.platform === 'win32') {
    if (identity.architecture !== 'x64')
      throw new Error('Native Windows CLI is currently available for x64 only');
    return 'win32-x64';
  }
  if (identity.platform === 'linux')
    return identity.architecture === 'arm64' ? 'linux-arm64-gnu' : 'linux-x64-gnu';
  return identity.architecture === 'arm64' ? 'darwin-arm64' : 'darwin-x64';
}

export function decodeNativeArtifact(
  value: unknown,
  expected: Pick<NativeRuntimeHostArtifact, 'target' | 'version'>,
  platform: 'posix' | 'win32',
): NativeRuntimeHostArtifact {
  const artifact = artifactSchema.parse(value);
  const paths = platform === 'win32' ? win32 : posix;
  const windowsTarget = artifact.target === 'win32-x64';
  if (
    artifact.target !== expected.target ||
    artifact.version !== expected.version ||
    !paths.isAbsolute(artifact.directory) ||
    artifact.executable !==
      paths.join(artifact.directory, 'bin', windowsTarget ? 'maka.exe' : 'maka') ||
    artifact.serviceExecutable !==
      (windowsTarget ? paths.join(artifact.directory, 'bin', 'maka-service.exe') : undefined)
  ) {
    throw new Error('Native CLI artifact does not match its requested target or layout');
  }
  return artifact;
}

export function decodeNativeSetup(value: unknown, pairing: boolean): NativeRuntimeHostSetup {
  // Do not forward schema diagnostics: a malformed receipt can contain secrets.
  const parsed = setupSchema.safeParse(value);
  if (!parsed.success) throw new Error('Native setup returned an invalid receipt');
  const result = parsed.data;
  if (
    result.deployment.admission !== undefined ||
    Boolean(result.pairing) !== pairing ||
    (result.pairing && result.pairing.rootId !== result.deployment.rootId)
  ) {
    throw new Error('Native setup returned an inconsistent receipt');
  }
  return result;
}

export function nativeArtifactOperator(
  artifact: NativeRuntimeHostArtifact,
): RuntimeHostNativeOperatorCommand {
  return {
    kind: 'native',
    platform: artifact.target === 'win32-x64' ? 'win32' : 'posix',
    executablePath: artifact.executable,
  };
}

export function nativeImportArguments(pkg: NativeRuntimeHostPackage, source: string): string[] {
  return [
    'fetch',
    '--target',
    pkg.artifact.target,
    '--version',
    pkg.artifact.version,
    '--directory',
    source,
    '--receipt-sha256',
    pkg.receiptSha256,
    '--framed',
  ];
}

export function nativeSetupArguments(input: {
  readonly rootPath?: string;
  readonly principalId?: string;
  readonly projectDirectoryRoots?: readonly { readonly label: string; readonly path: string }[];
}): string[] {
  return [
    'setup',
    '--framed',
    ...(input.rootPath ? ['--root', input.rootPath] : []),
    ...(input.principalId ? ['--principal', input.principalId] : []),
    ...(input.projectDirectoryRoots?.length === 0
      ? ['--no-project-roots']
      : (input.projectDirectoryRoots ?? []).flatMap((root) => [
          '--project-root-json',
          JSON.stringify(root),
        ])),
  ];
}

/** Runs the bundled verifier locally; never executes a foreign-target binary. */
export async function resolveNativeRuntimeHostPackage(input: {
  readonly executable: string;
  readonly cache: string;
  readonly version: string;
  readonly identity: RuntimeHostTargetIdentity;
  /** Development only: cache directories previously populated by host fetch. */
  readonly sourceCache?: string;
  readonly signal?: AbortSignal;
}): Promise<NativeRuntimeHostPackage> {
  if (!isProductReleaseVersion(input.version))
    throw new Error('Native CLI requires an exact release version');
  const target = nativeRuntimeHostTarget(input.identity);
  const args = [
    'host',
    'fetch',
    '--target',
    target,
    '--version',
    input.version,
    '--cache',
    input.cache,
  ];
  if (input.sourceCache) {
    const directory = join(input.sourceCache, `${target}@${input.version}`);
    args.push('--directory', directory, '--receipt-sha256', await receiptDigest(directory));
  }
  const result = await run(input.executable, args, {
    timeout: 190_000,
    maxBuffer: 256 * 1024,
    windowsHide: true,
    signal: input.signal,
  });
  const artifact = decodeNativeArtifact(
    JSON.parse(result.stdout),
    { target, version: input.version },
    process.platform === 'win32' ? 'win32' : 'posix',
  );
  return { artifact, receiptSha256: await receiptDigest(artifact.directory) };
}

async function receiptDigest(directory: string): Promise<string> {
  const file = join(directory, 'receipt.json');
  const metadata = await lstat(file);
  if (!metadata.isFile() || metadata.size > 16 * 1024)
    throw new Error('Invalid native package receipt');
  const bytes = await readFile(file);
  if (bytes.length > 16 * 1024) throw new Error('Invalid native package receipt');
  return createHash('sha256').update(bytes).digest('hex');
}
