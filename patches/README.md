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

- The inline `anchor-name` and `aria-describedby` describe the **element**. They are bookkept against element identity and skipped entirely when the trigger has not moved. This is the entire cost, and the patch takes it to zero.
- `interactionRef` installs event listeners that close over the current props (`isEnabled`, `delay`, `hideDelay`, `focusTrigger`, `onOpenChange`). `removeEventListener` matches only the exact function that was added, so these are bound and released through React's own per-render effect cleanup: a single dependency-array-free effect ends with

  ```js
  const detach = tooltip.interactionRef;
  detach(target);
  return () => {
    detach(null);
  };
  ```

  React runs the cleanup a render created *before* the next render's effect body, so `detach` is always the very closure that added the listeners. That is the whole correctness argument, and it is React's guarantee rather than bookkeeping this patch has to maintain.

Upstream was correct only by accident, and in both directions at once: because `tooltip.ref` changed every render, every render happened to rescan the DOM (correct, and the whole cost) and happened to rebind listeners with fresh closures (correct, and — measured below — nearly free). Keying both halves on the element drops the cost but freezes the listener closure, so `isEnabled={false}` stops suppressing hover and the cleanup calls `removeEventListener` with function identities that never match, leaking listeners onto a trigger the tooltip does not own. Keying both on the callback keeps the listeners right and keeps the cost. Only splitting them gets both.

Four smaller corrections ride along, all consequences of the same "key it on the right thing" idea. Each one is a way the element-keyed half is *not* simply idempotent, and each is the reason a future rewrite of this patch must not be talked out of it:

- Detach removes **only this tooltip's own id** from `aria-describedby`, instead of restoring the snapshot taken at attach time. A snapshot is stale the moment anything else writes to the attribute: two tooltips sharing one trigger, or the application updating it while the tooltip is mounted. Restoring it silently un-describes the other writer, or resurrects a value the application has already replaced.
- **`aria-describedby` is a value invariant, not an element-identity one.** An element-keyed attach that early-returns whenever the element has not moved will skip a trigger whose `aria-describedby` the application rewrote from its own props — React writes the new value, the tooltip's own id is gone, and nothing ever puts it back, so the description silently breaks and does not self-heal. The effect therefore re-reads the attribute on the unchanged-element path and re-asserts its id when it is missing. Reading produces no write, so a steady state where nobody else touches the attribute still costs zero writes.
- **Detach must not route through `useLayer`'s ref.** `layer.ref(null)` removes the anchor name from whatever element `useLayer` *itself* last held (`useLayer.js:215`), which is not necessarily the element this tooltip attached to. When element children are replaced by text children, the text-only `<span>`'s JSX ref has already pointed `useLayer` at the span by the time the layout effect runs, so `layer.ref(null)` would strip the anchor name off the element that just acquired it. The patch adds and removes the anchor name directly through `addAnchorName`/`removeAnchorName` with `tooltip.anchorId`, which is the half of `layer.ref` that actually belongs to this component. `useLayer`'s `triggerRef` is used for nothing else.
- The effect resolving the trigger runs with no dependency array and returns immediately unless the element it resolves differs from the one it holds. That makes `TooltipProps`' documented "Children refs are preserved" true for a child that is replaced (`<button>` → `<a>`) or arrives late (`null` → `<button>`), which the upstream dependency arrays never handled.

**Why the listeners are allowed to rebind every render.** With attachment split this way, `interactionRef` still changes identity on every render (it transitively depends on `useLayer`'s per-render layer object), so the listeners are unbound and rebound every render — exactly what upstream did. The argument that this is affordable does not rest on a benchmark: `addEventListener` and `removeEventListener` touch no style or layout state, so they cannot dirty the cascade no matter how many of them run, while `anchor-name` is an inline style write that always does. A one-off Chromium measurement against a DOM shaped like the real transcript (22 history turns, 6 tooltip'd buttons each, 110 frames ≈ 2420 turn re-renders) put the rebinds at ~11ms of mutation and 0.0ms of style+layout per streamed answer, against 7.9ms of mutation plus 21.2ms of style+layout for the attribute writes they replace. That measurement was taken once and its script is not in this repo, so treat the numbers as an illustration and the invariant above as the reason.

Buying listener-identity stability instead would mean also patching `useLayer` (memoizing its context ref and returned object) to save something that costs no layout at all — and its blast radius is wider than the tooltip. Everything that calls `useLayer` directly would change effect-firing timing: `Carousel`, `Tokenizer`, `ContextMenu`, `useHoverCard` and `usePopover` (unused here), but also `DropdownMenu/DropdownMenuSubMenu.js` and `hooks/useKeyboardHint.js` — and `useKeyboardHint` backs `TabList`, `Toolbar` and `SegmentedControl`, which this repo references 19, 16 and 20 times respectively, with `DropdownMenu` at 34. Not touching `useLayer` is what keeps the patch's reach to the one component that was measured.

**Why `Popover.js` is left alone.** `usePopover` has the same defect: it calls `useLayer` and returns a fresh object literal per render, and `Popover`'s two attach effects depend on `popover` as a whole (`Popover.js:222,251`). Nothing in the CDP profile points at it — the measured writes are all tooltip triggers in the transcript, and `DropdownMenu`/`Popover` are not on the streaming hot path — so fixing it would widen the patch surface without evidence.

Both the published `dist/` files (what Node and the bundler load) and their `src/` counterparts (what the `source` export condition points at) carry the change. Nothing in this repo resolves the `source` condition — Vite sets no `resolve.conditions` and the tests load `dist` — so the `src/` half is defensive only; what actually stops the two from drifting is `patch-package --error-on-fail` in the root postinstall.

Delete the patch once an `@astryxdesign/core` release after `0.2.0` attaches its tooltip by element, re-asserts `aria-describedby` when the attribute is rewritten under it, and removes the anchor name from the element it attached to rather than the one `useLayer` currently holds. The procedure: bump the dependency, delete this patch file, run `npm --workspace @maka/ui run build && node --test "packages/ui/dist/**/tooltip-anchor-stability.test.js"`, and keep it deleted only if all 16 cases stay green. `patch-package --error-on-fail` in the root postinstall is the backstop if the patch is kept and stops applying.

The guard is `packages/ui/src/__tests__/tooltip-anchor-stability.test.tsx`, which renders the real `Tooltip` into a DOM that records attribute writes, inline-style writes, and `(type, handler)` listener pairs. It pins the cost and the correctness together, because each is the half the other's fix breaks: no write when the trigger has not moved, correct hand-off when it has (including element children becoming text children), the full hover/focus listener set actually bound, a prop change taking effect after mount, `aria-describedby` re-asserted when the application overwrites it, and an external trigger left exactly as it was found when the tooltip unmounts.

That DOM is deliberately partial, and the gaps are recorded next to `class FakeElement` in the test: no `showPopover`/`hidePopover` (so `useLayer.show()` always takes the Safari<17 `style.display` fallback and the native popover and `toggle` paths are unreached), a no-op `document.addEventListener` (so Escape-to-dismiss is unreachable), a `matches(':focus-visible')` that always answers true, and no `tabIndex` property (so a text-only `<span>` binds 3 of the 5 listeners a browser would). None of them touch what this patch changes — which element is written to, and which closure removes the listeners.
