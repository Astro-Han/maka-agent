#!/usr/bin/env node
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

/**
 * Convergence gate for #4109: which hooks are still called in the render body
 * of `AppShellContent`.
 *
 * In React the position in the tree IS the scope of the state. A hook called
 * in the shell's render body has the whole tree as its scope; the same hook
 * called in a provider has its readers as its scope. `AppShellContent` today
 * carries 536 hooks above the entire tree, so a session switch produces ~19
 * commits, ~14 of them full-tree renders, of which 96-99% is recoverable work.
 *
 * The fix is to move call sites, one feature at a time. That is slow, and a
 * checklist cannot tell whether it is progressing — extracting a feature into
 * its own slice does not change the scope of any state, and Session Navigation
 * proves it: a complete `model / controller / ui / ports` slice whose
 * controller is still invoked from this render body.
 *
 * A call site does have a definition of done, so this gate counts them. The
 * inventory below is an upper bound that may only shrink: a migration deletes
 * an entry and its own diff shows the gate converging. Adding a hook here is
 * not forbidden, but it cannot be done silently.
 *
 * Deliberately NOT a `--write` mode. Regenerating the inventory on demand
 * would let a new hook be accepted by rerunning a command, and the friction of
 * editing it by hand is the point.
 *
 * Purely derived hooks (`useMemo`, `useCallback`, `useRef`, `useId`) are
 * ignored: they hold no state and subscribe to nothing, so they cannot widen
 * the scope of a render. Everything else counts, including React's own
 * `useState` / `useEffect`, because a custom hook is only a name for them.
 *
 * Run: npm run check:app-shell-hooks
 * Fix: move the call site into the feature's provider, then delete its entry.
 */
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const root = join(fileURLToPath(new URL('..', import.meta.url)));
const shellFile = 'apps/desktop/src/renderer/app-shell.tsx';
const component = 'AppShellContent';

/** Hooks that hold no state and subscribe to nothing. */
const DERIVED_HOOKS = new Set(['useMemo', 'useCallback', 'useRef', 'useId']);

/**
 * Hooks still called in `AppShellContent`'s render body, with the number of
 * call sites. This inventory may only shrink. Each entry is one scope that is
 * still the whole tree rather than its readers.
 */
const ALLOWED = {
  useActiveExecutionBoundary: 1,
  useActiveSessionEvents: 1,
  useAppShellBootstrapSubscriptions: 1,
  useAppShellComposerQuotes: 1,
  useAppShellHostEffects: 1,
  useAppShellNavRefSync: 1,
  useAppShellPersistenceEffects: 1,
  useAppShellProjectContext: 1,
  useAppShellSessionUiReads: 1,
  useAppShellSessionWorkspace: 1,
  useAppShellTurnPresentation: 1,
  useCommandPalette: 1,
  useComposerAttachments: 1,
  useEffect: 14,
  useGoalController: 1,
  useKeyboardHelp: 1,
  useKeyedPendingRegistry: 3,
  useLayoutEffect: 2,
  useModuleHubController: 1,
  useOnboardingSnapshot: 1,
  usePlanModeState: 1,
  useSessionEventHealthPolling: 1,
  useSessionNavigationController: 1,
  useSettingsModal: 1,
  useShellAppearance: 1,
  useShellChatModel: 1,
  useShellConnections: 3,
  useShellLiveTurn: 1,
  useShellMemoryPill: 1,
  useShellResume: 1,
  useShellRunUpdates: 1,
  useShellSearch: 1,
  useSkillPrompt: 1,
  useStableActions: 7,
  useState: 7,
  useTaskEntryController: 1,
  useTaskSubmissionReadiness: 1,
  useToast: 1,
  useWorkbarController: 1,
};

/**
 * The render body of a top-level function declaration. Its closing brace is
 * the first `}` in column zero after the declaration, which is cheaper and
 * more predictable here than balancing braces through TSX and template
 * literals.
 */
export function readRenderBody(source, componentName) {
  const start = source.indexOf(`\nfunction ${componentName}(`);
  if (start === -1) return null;
  const end = source.indexOf('\n}\n', start);
  return end === -1 ? null : source.slice(start, end);
}

/**
 * Blanks out comments and string bodies, so that prose naming a hook — which
 * the shell's comments do, at length — is not counted as a call site. Lengths
 * are preserved only incidentally; nothing downstream reads offsets.
 */
export function stripNonCode(source) {
  let out = '';
  let index = 0;
  while (index < source.length) {
    const char = source[index];
    const next = source[index + 1];
    if (char === '/' && next === '/') {
      const end = source.indexOf('\n', index);
      index = end === -1 ? source.length : end;
      continue;
    }
    if (char === '/' && next === '*') {
      const end = source.indexOf('*/', index + 2);
      index = end === -1 ? source.length : end + 2;
      continue;
    }
    if (char === "'" || char === '"' || char === '`') {
      index += 1;
      while (index < source.length && source[index] !== char) {
        index += source[index] === '\\' ? 2 : 1;
      }
      index += 1;
      continue;
    }
    out += char;
    index += 1;
  }
  return out;
}

export function countHooks(body) {
  const counts = {};
  for (const match of stripNonCode(body).matchAll(/\buse[A-Z][A-Za-z0-9]*(?=\s*\()/g)) {
    const name = match[0];
    if (DERIVED_HOOKS.has(name)) continue;
    counts[name] = (counts[name] ?? 0) + 1;
  }
  return counts;
}

export function compareToInventory(counts, allowed = ALLOWED) {
  const added = [];
  const grown = [];
  const stale = [];
  for (const [name, count] of Object.entries(counts)) {
    const budget = allowed[name];
    if (budget === undefined) added.push(name);
    else if (count > budget) grown.push({ name, budget, count });
  }
  for (const [name, budget] of Object.entries(allowed)) {
    const count = counts[name] ?? 0;
    if (count < budget) stale.push({ name, budget, count });
  }
  return { added, grown, stale };
}

function main() {
  const source = readFileSync(join(root, shellFile), 'utf8');
  const body = readRenderBody(source, component);
  if (body === null) {
    console.error(`${shellFile}: could not find the render body of ${component}`);
    process.exit(1);
  }

  const counts = countHooks(body);
  const { added, grown, stale } = compareToInventory(counts);
  const total = Object.values(counts).reduce((sum, n) => sum + n, 0);

  if (added.length === 0 && grown.length === 0 && stale.length === 0) {
    console.log(
      `app-shell hook scope: ok (${Object.keys(counts).length} hooks, ${total} call sites in ${component})`,
    );
    return;
  }

  for (const name of added) {
    console.error(
      `${shellFile}: ${name} is a new hook in ${component}'s render body.\n` +
        '  Its state would be scoped to the whole tree. Call it from the feature\n' +
        '  provider instead; add it to the inventory only if it genuinely belongs\n' +
        '  to the shell, and say why in the pull request (#4109).',
    );
  }
  for (const { name, budget, count } of grown) {
    console.error(
      `${shellFile}: ${name} has ${count} call sites in ${component}, inventory allows ${budget}.`,
    );
  }
  for (const { name, budget, count } of stale) {
    console.error(
      `${shellFile}: ${name} is down to ${count} call sites (inventory says ${budget}).\n` +
        `  The gate converged — ${count === 0 ? 'delete its entry' : `lower it to ${count}`} in scripts/check-app-shell-hooks.mjs.`,
    );
  }
  process.exit(1);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) main();
