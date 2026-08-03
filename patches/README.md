# patches

`scripts/apply-dependency-patches.mjs` applies everything here during the root `postinstall`, with `--error-on-fail` so a patch that no longer applies blocks the install instead of silently disappearing. It skips when `patch-package` itself is absent, which happens under `npm ci --workspace <name>` and `npm ci --omit=dev`; those trees are not what ships, and an unpatched one fails the regression test below.

After bumping a patched dependency, re-run `npx patch-package <name>` so the patch filename tracks the installed version.

Each patch needs a reason to exist and a condition under which it can be deleted.

## `@ai-sdk/provider-utils`: a streamed tool call is identified by `index` and `id` together

Fixes [#1967](https://github.com/maka-agent/maka-agent/issues/1967) and [#1976](https://github.com/maka-agent/maka-agent/issues/1976). Both are one modelling defect in `StreamingToolCallTracker`: the streamed `tool_calls[].index` is an association label a gateway may omit, repeat, or number freely, and the tracker used it as the storage slot, the identity, and the ordering all at once. The class even declares `private toolCalls: TrackedToolCall[]` while indexing it by that external number, so the type hid that the array was sparse.

Three symptoms, one cause:

- **index as a storage slot** — a gateway reporting `index: 1` leaves an empty slot at 0, and `flush()` walks the array with `for...of`, which does not skip holes the way `forEach` would, and dereferences `undefined.hasFinished`. This crashed the whole turn (#1967).
- **index as identity** — two distinct calls both labelled `index: 0` (the Ollama shape in [vercel/ai#14277](https://github.com/vercel/ai/pull/14277); 45 registry entries use the `openai-compatible` adapter and all 45 are `status: 'ready'`, including `ollama` and `ollama-cloud`) collided in one slot. `processExistingToolCall` only appends `function.arguments`, so arguments concatenated into invalid JSON and the second call's `id` and `name` were dropped. When the concatenation happened to stay valid JSON the second call vanished with no error at all (#1976).
- **index as a required field** — a call whose deltas omit `index` and span more than one chunk threw `Expected 'id' to be a string.`, because the `?? this.toolCalls.length` fallback picked a fresh slot for each continuation (#1976).

The patch separates the three jobs, and — this is the part that took two review rounds to get right — it does **not** promote `id` to be the new sole authority. `id` is exactly as untrustworthy as `index`: gateways repeat it across distinct calls and send `''` in place of omitting it. Keying on `id` alone reproduced the original defect with the two fields swapped, merging distinct calls into one invalid-JSON input, and killed whole turns when a continuation carried `id: ''`.

So neither field is accepted alone. Records live in a dense array in creation order, with `byId` and `byIndex` as alias maps. `processDelta` normalizes `''` to absent, then:

- **both fields present** — the index selects the slot and the `id` decides whether that slot still holds the same call. Requiring them to agree is what stops a repeated `id` at a different index from absorbing a delta, and a repeated index under a new `id` from merging.
- **`id` only** — look it up; a continuation that omits `index` still belongs to its call.
- **`index` only** — the slot's current occupant, so a reused index routes later continuations to the call that most recently claimed it.
- **neither** — attributable only when exactly one call is open. With several open this is undecidable, and guessing a target is precisely how one call's arguments end up on another; it falls through and fails on the absent `id`.

`flush()` emits in index order when every call supplied one, and in arrival order otherwise. The sort is stable, so calls sharing a repeated index keep arrival order for free.

**Known boundary, not closed.** `@ai-sdk/openai-compatible` keeps its own index-keyed buffer ahead of the tracker and forwards a delta as soon as it has a `name`; later deltas on an already-forwarded index bypass that buffer. So a reused index whose new call has not sent its `name` yet reaches the tracker as an unnamed new identity and the turn fails. Failing is intended — the old behaviour appended those arguments to the previous call — but it is a hard failure, not a recovery. Also undecidable: a reused index with *interleaved* continuations, where the index has stopped carrying any information. A gateway that can interleave several calls' fragments but cannot number them is emitting a stream nobody can parse; sequential reuse, the realistic shape, is handled correctly.

We are not upstreaming this, so treat it as a long-lived vendored correction rather than something waiting on a release. It rewrites the internals of one self-contained class without touching either method other code calls (`processDelta`, `flush`), and `toolCalls` is private — but note `patch-package` hard-fails on any upstream edit to that class's text, so a dependency bump surfaces as a blocked install, not a silent drop. Only the `dist/` half is load-bearing: `package.json` `exports` resolves to `dist/index.js` and nothing compiles `src/`. The `src/` half is kept as the readable record of the same change, so a future fix must land in both.

**Delete it when the guard still holds without it.** Remove the patch, reinstall, and run `packages/runtime/src/__tests__/model-factory-tool-call-index.test.ts`. The question that condition answers is deliberately narrower than "are all 18 cases green": two of them (`fails loudly rather than merging when a reused index delays its name`, `refuses to guess a target for a bare delta while several calls are open`) assert that an undecidable shape fails, and an upstream that genuinely fixed the underlying layers would make those shapes *succeed* — turning the guard red for a better implementation. So read the result by property, not by count: the patch is no longer needed once no case merges two calls' arguments into one input, drops a call, or crashes, on both the `openai` and `openai-compatible` paths. `assertInputsSelfContained` is the property; the exact-output assertions exist to pin today's behaviour, not to define correctness for all time. Related upstream reports, for context only: [vercel/ai#18333](https://github.com/vercel/ai/issues/18333), [vercel/ai#14277](https://github.com/vercel/ai/pull/14277), [vercel/ai#15879](https://github.com/vercel/ai/pull/15879).
