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
import { parseArgs } from 'node:util';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { join, resolve } from 'node:path';

const root = fileURLToPath(new URL('../../', import.meta.url));

export function rustTarget(platform = process.platform, arch = process.arch) {
  const cpu = { x64: 'x86_64', arm64: 'aarch64' }[arch];
  const os = { darwin: 'apple-darwin', linux: 'unknown-linux-gnu', win32: 'pc-windows-msvc' }[
    platform
  ];
  if (!cpu || !os) throw new Error(`Unsupported Maka target: ${platform}/${arch}`);
  return `${cpu}-${os}`;
}

export async function buildCli({ release = false, target } = {}) {
  const args = ['build', '--locked', '-p', 'maka-cli', '--target-dir', join(root, 'target')];
  if (release) args.push('--release');
  if (target) args.push('--target', target);
  await new Promise((resolveBuild, reject) => {
    const child = spawn('cargo', args, { cwd: root, stdio: 'inherit', windowsHide: true });
    child.once('error', reject);
    child.once('exit', (code, signal) => {
      if (code === 0) resolveBuild();
      else reject(new Error(`Maka build failed: ${signal ?? code}`));
    });
  });
  const windows = target ? target.includes('-windows-') : process.platform === 'win32';
  return join(
    root,
    'target',
    ...(target ? [target] : []),
    release ? 'release' : 'debug',
    windows ? 'maka.exe' : 'maka',
  );
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { values } = parseArgs({
    options: { release: { type: 'boolean' }, target: { type: 'string' } },
  });
  console.log(await buildCli(values));
}
