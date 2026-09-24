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

export interface HostedRuntimeInitialization {
  readonly incognito: true;
  readonly proxyUrl?: string;
}

/** Configure a new root before any Host can recover or admit work. */
export async function initializeNativeRuntimeHost(
  executable: string,
  rootPath: string,
  initialization?: HostedRuntimeInitialization,
  signal?: AbortSignal,
): Promise<void> {
  signal?.throwIfAborted();
  const settings = initialization === undefined ? undefined : initialSettings(initialization);
  const args = ['host', 'init', '--root', rootPath];
  if (settings !== undefined) args.push('--settings-stdin');
  const output = await new Promise<string>((resolve, reject) => {
    const child = execFile(
      executable,
      args,
      {
        windowsHide: true,
        timeout: 30_000,
        maxBuffer: 4096,
        signal,
        encoding: 'utf8',
      },
      (error, stdout) => {
        // Child stderr can contain arbitrary data. Report only a fixed local cause.
        if (error) reject(new Error('Native State Root initialization failed'));
        else resolve(stdout);
      },
    );
    child.stdin?.on('error', () => {});
    child.stdin?.end(settings === undefined ? undefined : JSON.stringify(settings));
  });
  let result: unknown;
  try {
    result = JSON.parse(output);
  } catch {
    throw new Error('Invalid native State Root identity');
  }
  if (
    typeof result !== 'object' ||
    result === null ||
    !('rootId' in result) ||
    typeof result.rootId !== 'string' ||
    !/^[a-f0-9]{64}$/.test(result.rootId)
  ) {
    throw new Error('Invalid native State Root identity');
  }
}

function initialSettings(input: HostedRuntimeInitialization) {
  if (input.incognito !== true) throw new Error('Invalid hosted initialization');
  const privacy = { incognitoActive: true };
  if (input.proxyUrl === undefined) return { privacy };
  try {
    const proxy = new URL(input.proxyUrl);
    if (
      proxy.protocol !== 'http:' ||
      !proxy.hostname ||
      proxy.search ||
      proxy.hash ||
      (proxy.pathname !== '' && proxy.pathname !== '/')
    )
      throw new Error();
    const username = decodeURIComponent(proxy.username);
    const password = decodeURIComponent(proxy.password);
    const authenticated = Boolean(username || password);
    if (authenticated && !password) throw new Error();
    return {
      privacy,
      networkProxy: {
        enabled: true,
        protocol: 'http',
        host: proxy.hostname.replace(/^\[|\]$/g, ''),
        port: Number(proxy.port || 80),
        authEnabled: authenticated,
        username,
        bypassList: [],
        autoBypassDomains: [],
      },
      ...(authenticated ? { proxyPassword: password } : {}),
    };
  } catch {
    throw new Error('Invalid hosted HTTP proxy');
  }
}
