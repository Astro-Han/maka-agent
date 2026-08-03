# patches

`scripts/apply-dependency-patches.mjs` applies everything here during the root `postinstall`, with `--error-on-fail` so a patch that no longer applies blocks the install instead of silently disappearing. It skips when `patch-package` itself is absent, which happens under `npm ci --workspace <name>` and `npm ci --omit=dev`; those trees are not what ships, and an unpatched one fails the regression test below.

After bumping a patched dependency, re-run `npx patch-package <name>` so the patch filename tracks the installed version.

Each patch needs a reason to exist and a condition under which it can be deleted.

## `@ai-sdk/provider-utils`: every alias a tool-call delta carries must agree

Fixes [#1967](https://github.com/maka-agent/maka-agent/issues/1967) and [#1976](https://github.com/maka-agent/maka-agent/issues/1976). One modelling defect in `StreamingToolCallTracker`: `tool_calls[].index` is an association label a gateway may omit, repeat, or number freely, and the tracker used it as the storage slot, the identity, and the ordering at once. The class even declares `private toolCalls: TrackedToolCall[]` while indexing it by that external number, so the type hid that the array was sparse.

Three symptoms, one cause:

- **index as a storage slot** — `index: 1` leaves an empty slot at 0, and `flush()` walks the array with `for...of`, which does not skip holes, and dereferences `undefined.hasFinished`. Crashed the turn (#1967).
- **index as identity** — two calls both labelled `index: 0` (the Ollama shape, [vercel/ai#14277](https://github.com/vercel/ai/pull/14277); 45 registry entries use the `openai-compatible` adapter and all 45 are `status: 'ready'`, including `ollama` and `ollama-cloud`) collided in one slot. `processExistingToolCall` only appends `function.arguments`, so arguments concatenated into invalid JSON and the second call's `id` and `name` were dropped; when the concatenation stayed valid JSON the second call vanished with no error (#1976).
- **index as a required field** — a call omitting `index` across chunks threw `Expected 'id' to be a string.`, because `?? this.toolCalls.length` picked a fresh slot per continuation (#1976).

### The rule

`id` is no more trustworthy than `index` — gateways repeat it across distinct calls and send `''` in place of omitting it — so promoting it to sole authority just reproduces the defect with the fields swapped. Two review rounds were spent discovering that empirically.

So the tracker keys on nothing. Records live in a dense array in creation order; `byId` and `byIndex` are alias maps. A delta continues an existing call only when **every** identifying field it carries agrees with that call — `id` (with `''` read as absent) and `function.name`. Which candidate is even considered comes from the strongest alias present: `index` first, then `id`, then — only when exactly one call is open — that call.

That last fallback is why a delta with no alias at all cannot be misfiled: with several calls open it is undecidable, so it falls through and fails on the missing `function.name` rather than guessing. Requiring `name` to agree is what closes the sharpest shape, a reused index *and* a repeated id, where the name is the only discriminator left.

An `id` that cannot address a call — absent, empty, or already taken — is replaced with a generated one. Upstream already reaches for `_generateId()` when finalizing but threw on an absent id long before that could fire; minting makes that branch live, and makes two calls sharing a wire id separately *addressable* rather than merely separately stored. That matters concretely: `tool-runtime.ts` derives `operationId` from the tool call id, so a duplicate collides and the second `commitToolPrepared` is rejected *after* the first tool's side effects have run, while an empty id throws out of `runtime-commit-sink.ts` before the call is recorded at all.

`flush()` emits in index order only when every call supplied an index **and** those indices are unique. A repeated index no longer orders anything, and a missing one turns the comparator into `NaN` comparisons that scramble rather than preserve.

### Known boundary

One shape still merges, and it is genuinely undecidable: a reused index carrying the same `id` *and* the same `name`, which is indistinguishable from one call streamed as two argument fragments. Two more fail loudly instead of merging — a reused index whose new call has not sent its `name` yet (`@ai-sdk/openai-compatible` forwards on first `name` and lets later deltas on that index bypass its buffer, so the tracker sees an unnamed new identity), and a reused index with *interleaved* continuations. A gateway that interleaves several calls' fragments without numbering them emits a stream nobody can parse; sequential reuse, the shape Ollama produces, is handled.

### Lifecycle

Not upstreamed, by decision — treat this as a long-lived vendored correction, not something waiting on a release. It rewrites the internals of one self-contained class without touching either method other code calls (`processDelta`, `flush`), and `toolCalls` is private. `patch-package` hard-fails on any upstream edit to that class's text, so a dependency bump surfaces as a blocked install in the trees that patch at all — note `scripts/apply-dependency-patches.mjs` deliberately skips when `patch-package` is unresolvable (`npm ci --workspace X`, `npm ci --omit=dev`), and those trees are covered only by the guard below. The patch touches `dist/index.js` alone, because `package.json` `exports` resolves there and nothing compiles `src/`; an earlier version also patched `src/` as a readable record, which only doubled the conflict surface and immediately drifted.

**Delete it when the guard still holds without it.** Remove the patch, reinstall, and run `packages/runtime/src/__tests__/model-factory-tool-call-index.test.ts`. Read the result by property, not by count: three cases assert that an undecidable shape fails, and an upstream that genuinely fixed the layers below would make those shapes *succeed* — turning the guard red for a better implementation. The patch is unnecessary once, on both the `openai` and `openai-compatible` paths, no case merges two calls' arguments into one input, drops a call, emits a duplicate or empty tool call id, or crashes. `assertInputsSelfContained` and `assertToolCallIdsUsable` are those properties, and every multi-call case asserts both; the exact-output assertions pin today's behaviour rather than defining correctness forever. Related upstream reports, for context only: [vercel/ai#18333](https://github.com/vercel/ai/issues/18333), [vercel/ai#14277](https://github.com/vercel/ai/pull/14277), [vercel/ai#15879](https://github.com/vercel/ai/pull/15879).
