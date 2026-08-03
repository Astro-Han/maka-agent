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

`id` is no more trustworthy than `index` — gateways repeat it across distinct calls and send `''` in place of omitting it — so promoting it to sole authority just reproduces the defect with the fields swapped. Two review rounds were spent discovering that empirically, and a third on the corollary below.

So the tracker keys on nothing. Records live in a dense array in creation order, each carrying the aliases the wire used for it (`index`, and `id` with `''` read as absent) and, separately, the `id` we emit. A delta resolves to the most recently created call that **every** alias it carries agrees with, where an alias the candidate has never seen cannot disagree — it neither matches nor rejects. At least one alias must actually match, so a delta carrying only arguments never resolves this way; when it carries no alias at all and exactly one call is open, it continues that call, and otherwise it falls through and fails on the missing `function.name` rather than being filed against a guess.

Requiring `function.name` to agree is what closes the sharpest shape, a reused index *and* a repeated id, where the name is the only discriminator left.

Keeping the wire's aliases and the emitted id apart is the corollary, and getting it wrong killed turns upstream had survived. An `id` that cannot address a call — absent, empty, or already taken by another call's emitted id — is replaced with a generated one, because `tool-runtime.ts` derives `operationId` from the tool call id, so a duplicate collides and the second `commitToolPrepared` is rejected *after* the first tool's side effects have run, while an empty id throws out of `runtime-commit-sink.ts` before the call is recorded at all. But the wire's `id` stays on the record as an alias: a gateway that repeats one `id` across calls also echoes it on their continuations, and comparing those against the *generated* id — which by construction they never equal — made every such delta unresolvable, so it became a new call with no name and threw. Note this leaves upstream's `toolCall.id ?? this._generateId()` in `finishToolCall` provably dead rather than reachable; minting happens at creation instead.

`flush()` emits in index order only when every call supplied an index **and** those indices are unique. A repeated index no longer orders anything, and a missing one turns the comparator into `NaN` comparisons that scramble rather than preserve.

### Known boundaries

Two shapes fail loudly, which is the intended outcome: a delta whose aliases contradict each other, and a reused index whose new call has not sent its `name` yet (`@ai-sdk/openai-compatible` forwards on first `name` and lets later deltas on that index bypass its buffer, so the tracker sees an unnamed new identity). Closing the second properly means folding that buffer into the tracker so a call can stay pending until its name arrives — a rewrite of both layers.

Three shapes stay silently wrong, and all three are one alias shared by two calls that are both still open, with the continuation carrying only that alias:

- a reused index continued by index alone, with the fragments *interleaved* rather than sequential;
- a duplicated `id` as the only alias on a continuation;
- a reused index carrying the same `id` *and* the same `name`, indistinguishable from one call streamed as two fragments.

The fragment is attributed to the call that most recently claimed the alias. That is required for the sequential shape Ollama actually produces and wrong for an interleaved one, and the tracker cannot tell them apart — a gateway that interleaves fragments while reusing one label emits a stream nobody can demultiplex. What still holds in all three: every call is emitted, once, under its own name and its own usable id. Only the argument text lands on the wrong call. `getAIModel: streamed tool call shapes that cannot be demultiplexed` in the guard pins exactly that much.

### Lifecycle

Not upstreamed, by decision — treat this as a long-lived vendored correction, not something waiting on a release. It rewrites the internals of one self-contained class without touching either method other code calls (`processDelta`, `flush`), and `toolCalls` is private. Any upstream edit that disturbs the patched hunks hard-fails the install rather than silently dropping the fix, so a dependency bump surfaces as a blocked install — but only in trees that patch at all: `scripts/apply-dependency-patches.mjs` deliberately skips when `patch-package` is unresolvable (`npm ci --workspace X`, `npm ci --omit=dev`), and an upstream edit *outside* those hunks applies cleanly and is caught only by the guard below. The patch touches `dist/index.js` alone, because `package.json` `exports` resolves there and nothing compiles `src/`; an earlier version also patched `src/` as a readable record, which only doubled the conflict surface and immediately drifted.

**Delete it when the guard still holds without it.** Remove the patch, reinstall, and run `packages/runtime/src/__tests__/model-factory-tool-call-index.test.ts`. Read the result by property, not by count. The patch is unnecessary once, on both the `openai` and `openai-compatible` paths, no case merges two calls' arguments into one input, drops a call, emits a duplicate or empty tool call id, emits a `tool-input-start`/`-delta`/`-end` sequence that does not match the `tool-call` it precedes, or crashes. `assertInputsSelfContained`, `assertToolCallIdsUsable`, and `assertEventLifecycle` are those properties. Two caveats when reading a red result: four cases assert that an *undecidable* shape fails, and an upstream that genuinely fixed the layers below would make those shapes succeed — turning the guard red for a better implementation. And self-containment is a property of the shapes the guard covers, not of any implementation: the `shapes that cannot be demultiplexed` suite exists because a shared alias makes it unachievable, and it deliberately does not assert it. Related upstream reports, for context only: [vercel/ai#18333](https://github.com/vercel/ai/issues/18333), [vercel/ai#14277](https://github.com/vercel/ai/pull/14277), [vercel/ai#15879](https://github.com/vercel/ai/pull/15879).
