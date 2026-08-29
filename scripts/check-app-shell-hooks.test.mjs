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

import assert from 'node:assert/strict';
import test from 'node:test';
import {
  compareToInventory,
  countHooks,
  readRenderBody,
  stripNonCode,
} from './check-app-shell-hooks.mjs';

const SHELL = [
  'export function AppShell() {',
  '  return <AppShellContent />;',
  '}',
  '',
  'function AppShellContent({ prop }: Props) {',
  '  const [value, setValue] = useState(0);',
  '  const derived = useMemo(() => value, [value]);',
  '  const chat = useShellChatModel({ value });',
  '  return <div>{derived}</div>;',
  '}',
  '',
  'function AfterTheShell() {',
  '  useSomethingElse();',
  '}',
  '',
].join('\n');

test('the render body stops at the component that follows it', () => {
  const body = readRenderBody(SHELL, 'AppShellContent');
  assert.match(body, /useShellChatModel/);
  assert.doesNotMatch(body, /useSomethingElse/);
  assert.doesNotMatch(body, /AppShell\(\)/);
});

test('an unknown component is reported rather than guessed at', () => {
  assert.equal(readRenderBody(SHELL, 'NoSuchComponent'), null);
});

test('prose naming a hook is not a call site', () => {
  const body = [
    '// The model itself lives in useShellChatModel (a pure derivation).',
    '/* See useGoalController () for the other half. */',
    "const label = 'useToast ()';",
    '  const chat = useShellChatModel({});',
  ].join('\n');
  assert.deepEqual(countHooks(body), { useShellChatModel: 1 });
});

test('an escaped quote does not swallow the rest of the file', () => {
  const body = ["const label = 'it\\'s here';", 'useToast();'].join('\n');
  assert.deepEqual(countHooks(body), { useToast: 1 });
});

test('a URL is not mistaken for a line comment', () => {
  assert.match(stripNonCode('const url = ok; // https://example.com\nuseToast();'), /useToast/);
});

test('purely derived hooks do not widen a scope, so they are not counted', () => {
  const counts = countHooks(readRenderBody(SHELL, 'AppShellContent'));
  assert.deepEqual(counts, { useState: 1, useShellChatModel: 1 });
});

test('a new hook in the render body fails the gate', () => {
  const { added } = compareToInventory({ useState: 1, useBrandNew: 1 }, { useState: 1 });
  assert.deepEqual(added, ['useBrandNew']);
});

test('a second call site of an inventoried hook fails the gate', () => {
  const { grown } = compareToInventory({ useState: 2 }, { useState: 1 });
  assert.deepEqual(grown, [{ name: 'useState', budget: 1, count: 2 }]);
});

test('a migrated hook fails until its entry is deleted, so the gate converges', () => {
  const { stale } = compareToInventory({}, { useComposerMentions: 1 });
  assert.deepEqual(stale, [{ name: 'useComposerMentions', budget: 1, count: 0 }]);
});

test('the committed inventory matches the shell as it stands', async () => {
  const { readFile } = await import('node:fs/promises');
  const source = await readFile(
    new URL('../apps/desktop/src/renderer/app-shell.tsx', import.meta.url),
    'utf8',
  );
  const body = readRenderBody(source, 'AppShellContent');
  assert.notEqual(body, null, 'AppShellContent must still be a top-level function declaration');
  const { added, grown, stale } = compareToInventory(countHooks(body));
  assert.deepEqual({ added, grown, stale }, { added: [], grown: [], stale: [] });
});
