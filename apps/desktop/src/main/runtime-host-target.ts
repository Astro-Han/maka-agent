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

export type RuntimeHostTargetIdentity =
  | { readonly platform: 'linux'; readonly architecture: 'x64' | 'arm64'; readonly glibcVersion: string }
  | { readonly platform: 'darwin' | 'win32'; readonly architecture: 'x64' | 'arm64' };

/** OS tools only; shared by SSH preflight and WSL's actual Linux environment. */
export function posixRuntimeHostTargetProbe(marker: string): string {
  requireMarker(marker);
  return [
    'maka_os=$(uname -s) || exit 1',
    'maka_arch=$(uname -m) || exit 1',
    'maka_libc=',
    'case "$maka_os" in Linux) maka_libc=$(getconf GNU_LIBC_VERSION 2>/dev/null) || maka_libc=unknown ;; Darwin) if [ "$(sysctl -in sysctl.proc_translated 2>/dev/null)" = 1 ]; then maka_arch=arm64; fi ;; *) exit 127 ;; esac',
    // Split the marker so shell diagnostics echoing this command cannot be
    // mistaken for a result. Keep the command single-line for csh/tcsh SSH shells.
    `printf '%s%s%s:%s:%s\\n' '__MAKA_RUNTIME_HOST_TARGET_' '${marker.slice('__MAKA_RUNTIME_HOST_TARGET_'.length)}' "$maka_os" "$maka_arch" "$maka_libc"`,
  ].join('; ');
}

export function windowsRuntimeHostTargetProbe(marker: string): string {
  requireMarker(marker);
  const script = [
    "$ErrorActionPreference = 'Stop'",
    "$architecture = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()",
    `[Console]::WriteLine('${marker}Windows:' + $architecture + ':')`,
  ].join('; ');
  return `powershell.exe -NoLogo -NoProfile -NonInteractive -EncodedCommand ${Buffer.from(script, 'utf16le').toString('base64')}`;
}

export function decodeRuntimeHostTarget(value: string): RuntimeHostTargetIdentity {
  const [os, cpu, libc, ...extra] = value.split(':');
  if (extra.length || libc === undefined) throw new Error('Invalid Runtime Host target result');
  const architecture = cpu === 'x86_64' || cpu === 'X64' ? 'x64'
    : cpu === 'aarch64' || cpu === 'arm64' || cpu === 'Arm64' ? 'arm64' : undefined;
  if (!architecture) throw new Error(`Unsupported Runtime Host architecture: ${cpu}`);
  if (os === 'Linux') {
    const version = /^glibc ([0-9]+\.[0-9]+)$/u.exec(libc)?.[1];
    if (!version) throw new Error('Native Linux Runtime Host requires GNU libc; target libc could not be verified');
    return { platform: 'linux', architecture, glibcVersion: version };
  }
  if ((os === 'Darwin' || os === 'Windows') && libc === '') {
    return { platform: os === 'Darwin' ? 'darwin' : 'win32', architecture };
  }
  throw new Error(`Unsupported Runtime Host operating system: ${os}`);
}

function requireMarker(marker: string): void {
  if (!/^__MAKA_RUNTIME_HOST_TARGET_[0-9a-f]+__$/u.test(marker)) {
    throw new Error('Invalid Runtime Host target marker');
  }
}
