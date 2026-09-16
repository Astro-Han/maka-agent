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

// Apache OpenDAL uses cargo-deny for both policy and the TSV inventory:
// https://github.com/apache/opendal/blob/main/scripts/dependencies.py
import { execFileSync } from 'node:child_process';
import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const mode = process.argv[2];
if (!['check', 'generate'].includes(mode) || process.argv.length !== 3) {
  throw new Error('Usage: node scripts/rust/dependencies.mjs <check|generate>');
}
const root = fileURLToPath(new URL('../../', import.meta.url));
const cargo = ['deny', '--locked', '--config', 'deny.toml'];
// Check test/build dependencies too, but omit dev-only crates from the CLI inventory.
execFileSync('cargo', [...cargo, 'check', 'licenses'], { cwd: root, stdio: 'inherit' });
const inventory = execFileSync(
  'cargo',
  [
    ...cargo,
    '--manifest-path',
    'crates/cli/Cargo.toml',
    '--exclude-dev',
    'list',
    '--format',
    'tsv',
  ],
  { cwd: root, encoding: 'utf8', maxBuffer: 4 * 1024 * 1024 },
).replaceAll('\r\n', '\n');
const output = new URL('../../crates/cli/DEPENDENCIES.rust.tsv', import.meta.url);
if (mode === 'generate') {
  writeFileSync(output, inventory);
} else if (readFileSync(output, 'utf8').replaceAll('\r\n', '\n') !== inventory) {
  throw new Error(
    'Rust dependency inventory is stale; run node scripts/rust/dependencies.mjs generate',
  );
}
