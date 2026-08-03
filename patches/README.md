# patches

`scripts/apply-dependency-patches.mjs` applies everything here during the root `postinstall`, with `--error-on-fail` so a patch that no longer applies blocks the install instead of silently disappearing. It skips when `patch-package` itself is absent, which happens under `npm ci --workspace <name>` and `npm ci --omit=dev`; those trees are not what ships, and an unpatched one fails the regression test below.

After bumping a patched dependency, re-run `npx patch-package <name>` so the patch filename tracks the installed version.

Each patch needs a reason to exist and a condition under which it can be deleted.

## `@ai-sdk/provider-utils`: a streamed tool-call index is an identifier, not an array slot

Fixes [#1967](https://github.com/maka-agent/maka-agent/issues/1967): a tool call through an OpenAI-compatible gateway that labels `tool_calls[].index` with a non-zero-starting or non-contiguous number crashes the whole turn.

`StreamingToolCallTracker` stores tool calls at `this.toolCalls[index]`, so a gateway reporting `index: 1` leaves an empty slot at 0. `flush()` then walks the array with `for...of`, which does not skip holes the way `forEach` would, and dereferences `undefined.hasFinished`. The patch skips empty slots.

That is the crash, not the whole defect class. `index` says which tool call a delta belongs to, but identity really lives in `toolCallDelta.id`, and the tracker keys off neither consistently. Two cases remain broken exactly as they are upstream, unchanged by this patch:

- **A reused index across two distinct calls** (`id: a` then `id: b`, both at `index: 0`, the Ollama shape in [vercel/ai#14277](https://github.com/vercel/ai/pull/14277)) silently merges them into one call with concatenated, invalid JSON arguments and drops the second `id`.
- **A call whose deltas omit `index`** and span more than one chunk throws `Expected 'id' to be a string.`, because the `?? this.toolCalls.length` fallback picks a fresh slot each time.

The crash this patch fixes is reported upstream in [vercel/ai#18333](https://github.com/vercel/ai/issues/18333); the reused-index case is [vercel/ai#14277](https://github.com/vercel/ai/pull/14277) and the missing-index case [vercel/ai#15879](https://github.com/vercel/ai/pull/15879). Fixing those means rekeying the tracker on `id`, which is upstream's call to make, not something to widen this patch into.

Delete the patch once `@ai-sdk/provider-utils` stops crashing on sparse indices itself. The guard is `packages/runtime/src/__tests__/model-factory-tool-call-index.test.ts`, which asserts behaviour through the real provider stack and stays green either way.

## `@astryxdesign/core`: a tooltip's trigger attachment is per-element state, not per-render state

Part of [#2030](https://github.com/maka-agent/maka-agent/issues/2030): a streamed answer dropped the app from 118fps to 22fps, and one contributing layer is that every re-render rewrote DOM attributes on tooltip triggers that had not changed. Measured with CDP over a single answer against a real session: 44608 inline-`style` + `aria-describedby` writes on footer buttons belonging to *history*, messages nothing in that turn touched. With the patch the same session measures 0.

`Tooltip` attached its trigger from two layout effects that listed `tooltip.ref` as a dependency and never listed the element they actually read (`anchorRef.current`, or `wrapper.firstElementChild`). `useTooltip` composes `tooltip.ref` from `useLayer`'s context ref, which is a fresh inline closure on every render, so both effects tore down and re-ran on every render of every tooltip on the page. Each run unconditionally executes `removeAttribute`/`setAttribute('aria-describedby')` and `removeAnchorName`/`addAnchorName` — and the latter pair writes inline `style.anchor-name`, which invalidates style. Nothing about the tooltip had changed; the callback identity churn alone was the trigger, which makes the cost O(every tooltip trigger on the page) per unrelated parent re-render.

The fix is to key attachment on what each piece of state is actually about. A tooltip puts two different kinds of state on its trigger:

- `positionRef` (inline `anchor-name`) and `aria-describedby` describe the **element**. Applying them twice to the same element is a no-op, so they are keyed on element identity and skipped when nothing moved. This is the entire cost, and the patch takes it to zero.
- `interactionRef` installs event listeners that close over the current props (`isEnabled`, `delay`, `hideDelay`, `focusTrigger`, `onOpenChange`). `removeEventListener` matches only the exact function that was added, so these are keyed on the **closure** that installed them, and are removed through that same closure.

Upstream was correct only by accident, and in both directions at once: because `tooltip.ref` changed every render, every render happened to rescan the DOM (correct, and the whole cost) and happened to rebind listeners with fresh closures (correct, and — measured below — nearly free). Keying both halves on the element drops the cost but freezes the listener closure, so `isEnabled={false}` stops suppressing hover and the unmount cleanup calls `removeEventListener` with function identities that never match, leaking listeners onto a trigger the tooltip does not own. Keying both on the callback keeps the listeners right and keeps the cost. Only splitting them gets both.

Two smaller corrections ride along, both consequences of the same "key it on the right thing" idea:

- Detach removes **only this tooltip's own id** from `aria-describedby`, instead of restoring the snapshot taken at attach time. A snapshot is stale the moment anything else writes to the attribute: two tooltips sharing one trigger, or the application updating it while the tooltip is mounted. Restoring it silently un-describes the other writer.
- The effect resolving the trigger runs with no dependency array and returns immediately unless the element it resolves differs from the one it holds. That makes `TooltipProps`' documented "Children refs are preserved" true for a child that is replaced (`<button>` → `<a>`) or arrives late (`null` → `<button>`), which the upstream dependency arrays never handled.

**Why the listeners are allowed to rebind every render.** With attachment split this way, `interactionRef` still changes identity on every render (it transitively depends on `useLayer`'s per-render layer object), so the listeners are unbound and rebound every render — exactly what upstream did. Measured in Chromium against a DOM shaped like the real transcript (22 history turns, 6 tooltip'd buttons each, 110 frames ≈ 2420 turn re-renders):

| per streamed answer | operations | mutation | style + layout | total |
|---|---|---|---|---|
| rebind listeners (what this patch does) | 145200 | 11.0ms | **0.0ms** | 11.0ms |
| rewrite `anchor-name` + `aria-describedby` (unpatched) | 58080 | 7.9ms | **21.2ms** | 29.1ms |

`addEventListener`/`removeEventListener` touch no style or layout state, so 145200 of them cost ~11ms spread across an entire answer — about 4.6µs per turn re-render, well under one frame, and they never dirty the cascade. Half as many attribute writes cost nearly three times as much, almost all of it style recalc. That is why stabilizing the listener identity is not worth buying: it would mean also patching `useLayer` (memoizing its context ref and returned object), which changes effect-firing timing for `Carousel`, `Tokenizer`, and `ContextMenu` — components this repo does not use and does not test — to save ~11ms per answer that costs no layout at all.

**Why `Popover.js` is left alone.** `usePopover` has the same defect: it calls `useLayer` and returns a fresh object literal per render, and `Popover`'s two attach effects depend on `popover` as a whole (`Popover.js:222,251`). Nothing in the CDP profile points at it — the measured writes are all tooltip triggers in the transcript, and `DropdownMenu`/`Popover` are not on the streaming hot path — so fixing it would widen the patch surface without evidence. It belongs in the upstream report, not in this patch.

Both the published `dist/` files (what Node and the bundler load) and their `src/` counterparts (what the `source` export condition points at) carry the change. Nothing in this repo resolves the `source` condition — Vite sets no `resolve.conditions` and the tests load `dist` — so the `src/` half is defensive only; what actually stops the two from drifting is `patch-package --error-on-fail` in the root postinstall.

Not reported upstream yet: there is no issue or PR on `@astryxdesign/core` for this. Delete the patch once a release after `0.2.0` attaches its tooltip by element and detaches its listeners through the closure that bound them. The guard is `packages/ui/src/__tests__/tooltip-anchor-stability.test.tsx`, which renders the real `Tooltip` into a DOM that records attribute writes, inline-style writes, and `(type, handler)` listener pairs. It pins the cost and the correctness together, because each is the half the other's fix breaks: no write when the trigger has not moved, correct hand-off when it has, the full hover/focus listener set actually bound, a prop change taking effect after mount, and an external trigger left exactly as it was found when the tooltip unmounts.
