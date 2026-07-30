# Terminal-Bench 2.1 — Maka vs OpenCode with GLM-5.2 Max

Paired harness A/B on the frozen 89-task Terminal-Bench 2.1 suite, with Z.ai Coding Plan `glm-5.2` at `max` reasoning effort on both arms. Three complete paired rounds, 534 adopted task-arm cells, one model attempt per cell. This report records the Pass@1 result, the task-level inference behind it, how the agent timeout affects the comparison, and the resource profile behind the outcome.

**Evidence:** three complete paired rounds, 267 task-round pairs
**Metric:** Pass@1 by the task's Harbor verifier
**Coverage:** 89/89 paired tasks in every round; 534/534 adopted task-arm cells
**Per-task outcomes and tokens:** [`terminal-bench-2.1-maka-vs-opencode-glm-5.2-max.csv`](./terminal-bench-2.1-maka-vs-opencode-glm-5.2-max.csv)

## TL;DR

- **Maka wins on Pass@1: 162/267 (60.7%) versus OpenCode's 136/267 (50.9%), a +9.7 pp lead in Maka's favor.** Maka leads in all three rounds, by 8, 14, and 4 tasks. Best single round: Maka **66.3%** versus OpenCode **57.3%**.
- **The lead is statistically established at the correct unit of analysis.** Treating each of the 89 tasks as the independent item, an exact paired permutation test gives `p = 0.00426` and the task-cluster bootstrap 95% CI on the difference is **[+3.4, +16.1] pp** — the interval excludes zero.
- **The lead is not an artifact of the agent timeout, but its magnitude is partly contingent on it.** Among cells that finished inside their native timeout, Maka leads 79.8% to 73.6% (+6.2 pp). However, OpenCode is terminated at its deadline more often than Maka (119 versus 80 cells), and it hit the deadline in 35 of the 46 cells Maka won exclusively — so a longer agent timeout could narrow the headline gap.
- **Maka reaches its passes faster,** median 4.3 minutes versus OpenCode's 10.8, measured over the 107 Maka and 136 OpenCode passing cells that carry a recorded duration. 55 of Maka's 162 passes have no recorded duration and are excluded; see the duration-completeness caveat.
- **Maka's quality gain is not paid for on the tasks it wins.** On the 116 cells both arms solve, Maka spends marginally *less* (50.05M versus 50.36M tokens). 24.6% of Maka's aggregate token excess buys its 46 exclusive wins (about 0.48M extra tokens per exclusive pass). **77.6% is spent on the 85 cells neither arm solves**, where both arms run equally long and hit their deadlines at equal rates — the excess is iteration density, not persistence.
- **Security and system administration show the largest gaps.** Security 95.8% versus 54.2%, system administration 63.0% versus 29.6%. At the task unit neither reaches significance after correcting for the 16 clusters examined; both are at the strongest signal their cluster size can produce.

## Results

Each round executes the same frozen task source, the same model at the same reasoning effort, and pinned OpenCode 1.17.18. Rounds 1 and 2 execute the tracked-clean detached Maka checkout at `ee7e4ba5d2323f31be5ab3ec4b4a5fbc847f9a83`; round 0 is the same commit in all but two of its 178 cells (see below).

| Round | Maka | OpenCode | Difference | Both pass / Maka only / OpenCode only / both fail |
| --- | ---: | ---: | ---: | ---: |
| Round 0 | 59/89 (66.3%) | 51/89 (57.3%) | +8 tasks (+9.0 pp) | 44 / 15 / 7 / 23 |
| Round 1 | 56/89 (62.9%) | 42/89 (47.2%) | +14 tasks (+15.7 pp) | 39 / 17 / 3 / 30 |
| Round 2 | 47/89 (52.8%) | 43/89 (48.3%) | +4 tasks (+4.5 pp) | 33 / 14 / 10 / 32 |
| **All three rounds** | **162/267 (60.7%)** | **136/267 (50.9%)** | **+26 (+9.7 pp)** | **116 / 46 / 20 / 85** |

The direction is stable across all three rounds; the magnitude varies from +4 to +14 tasks per round.

**Round-0 subject sensitivity.** Two of round 0's 178 cells — `make-doom-for-mips` (Maka fail) and `video-processing` (Maka pass) — were supplied by replacement runs whose wrapper fingerprint postdates `aff71e5cf fix(headless): export real Maka steps in Harbor trajectories (#1323)`. That commit changes post-run trajectory hydration, artifact download, reported metric fields, and relative-to-absolute path normalization; it does not touch the model, prompt, tool surface, execution loop, timeout policy, task source, or verifier, and the path normalization is a no-op for the absolute paths this benchmark uses. Two sensitivity checks confirm the conclusion does not depend on those cells: dropping both pairs gives +9.43 pp (`p = 0.0058`), and adversarially forcing both to Maka failures gives +9.36 pp (`p = 0.0058`).

## Inference

The 89 tasks are frozen and repeat in every round, so the 267 task-round pairs are repeated measurements, not 267 independent benchmark items. All inference below uses the task as the unit and collapses each task's three rounds into one paired difference.

| Test | Unit | Statistic | Result |
| --- | --- | --- | --- |
| Paired permutation on per-task pass-count difference | 89 tasks | exact enumeration over all 2^39 sign assignments | **`p = 0.00426`** (39 non-tied tasks) |
| Task-cluster bootstrap on the Pass@1 rate difference | 89 tasks | 100k resamples | **95% CI [+3.4, +16.1] pp** |
| Exact sign test on the direction only | 89 tasks | 28 Maka / 11 OpenCode / 50 ties | `p = 0.0095` |

The permutation test is the primary inference: it shares the sign test's unit of independence but uses the magnitude of each task's difference rather than discarding it, and the two agree in direction.

Descriptive per-round and pooled McNemar values, retained for continuity with the per-round reports: round 0 `p = 0.134`, round 1 `p = 0.0026`, round 2 `p = 0.541`, pooled over all 267 pairs `p = 0.00186`. **The pooled value must not be read as inference** — it treats repeated measurements of the same 89 tasks as independent. The per-round values show that no single 89-task round except round 1 is significant on its own; the evidence comes from pooling three rounds at the task level.

## Where these numbers sit against published results

Z.ai's model card reports two distinct Terminal-Bench 2.1 figures for `glm-5.2`, each with its own harness footnote ([model card](https://huggingface.co/zai-org/GLM-5.2/blob/main/README.md), accessed 2026-07-30):

| Vendor figure | Harness and conditions per the model card's footnote |
| --- | --- |
| **81.0** — "Terminal Bench 2.1 (Terminus-2)" | Terminus-2 framework, `parser=json`, **`timeout=4h`**, `temperature=1.0`, `top_p=1.0`, `max_new_tokens=48k`, `max_episodes=500`, 256K context, 4 CPUs / 8 GB RAM |
| **82.7** — "Terminal Bench 2.1 (Best Reported Harness)" | Claude Code 2.1.167, `temperature=1.0, top_p=0.95, max_new_tokens=131072` via a transparent proxy bypassing the 64k CLI cap, **wall-clock time limits removed**, **scores averaged over 5 runs** |

The GLM-5 repository's "81.0 vs. 62.0 on Terminal-Bench 2.1" comparison against GLM-5.1 refers to the Terminus-2 figure, not the 82.7 best-harness figure ([repository](https://github.com/zai-org/GLM-5), accessed 2026-07-30).

A single unverified third-party report describes an unsuccessful attempt to reproduce the Terminus-2 number — Harbor v0.16.1, GLM-5.2-FP8 served with sglang on H200, `reasoning_effort: max`, `agent_timeout_multiplier: 16.0`, `max_turns: 500` — stating "I can only get 62~64 score" ([zai-org/GLM-5#100](https://github.com/zai-org/GLM-5/issues/100), accessed 2026-07-30). It has no published ledger and remains an open help request, so it is context, not a validated reproduction, and this report does not position its own numbers against it.

Neither vendor figure is a comparison target for this A/B, and the agent time budget cannot be compared against either one from what is published.

Harbor resolves an agent timeout as `min(base_sec, max_timeout_sec) × multiplier`, where `base_sec` is the task's own declared `[agent] timeout_sec` (Harbor 0.13.2, `trial/trial.py`). `max_timeout_sec` can therefore only *lower* the budget, never raise it. **This makes the Terminus-2 footnote's `timeout=4h` ambiguous**: read as a `max_timeout_sec` cap it never binds, because no task in this suite declares a native limit above 200 minutes, and the resulting budget equals this run's; read as an override or multiplier it is four hours per task, far above native. The footnote does not disambiguate, and this report does not guess. The best-harness figure is unambiguous on this point — its footnote states wall-clock limits were removed entirely.

For the same reason, `max_episodes=500` cannot be assumed to bind either. In Terminus-2 an episode is one iteration of the agent loop, the parameter is the deprecated spelling of `max_turns`, and its default is 1,000,000 — setting it emits `max_turns (f.k.a. max_episodes) artificially limited to N. Consider removing this limit for better task completion.` Whether 500 turns constrains a given agent depends on its iteration rate; no cell in this run exceeded 230 steps.

What can be stated without interpreting the footnote concerns this run alone: **44.6% of OpenCode's cells and 39.2% of Maka's cells with a recorded duration are terminated by the clock**, and for 27 and 25 of the 89 tasks respectively, *every* recorded cell of that arm hits the limit. Absolute Pass@1 here is measured under the benchmark's native limits with a large fraction of cells cut off mid-work, so it is not comparable to published figures in either direction. This report makes no estimate of what either arm would score under a different time budget; that would require rerunning the suite.

**Leaderboard conformance, including this run's own deviation.** Harbor's leaderboard validator (`leaderboard/static_validation.py`) rejects a submission that sets `agent_timeout_multiplier`, `verifier_timeout_multiplier`, `agent_setup_timeout_multiplier`, `environment_build_timeout_multiplier`, `agent.override_timeout_sec`, `agent.override_setup_timeout_sec`, `verifier.override_timeout_sec`, or any environment resource override, and requires `timeout_multiplier == 1.0`. This run's **agent** timing conforms: all 178 job configs set `max_timeout_sec` to exactly the task's own `timeout_sec`, a no-op cap, with `timeout_multiplier = 1` and no agent override or multiplier. It does however set `verifier.override_timeout_sec = 3720` in all 178 configs, which that validator rejects. That override grants verification time rather than agent time and is applied identically to both arms, so it does not bias this comparison, but **this run is not a conformant leaderboard submission and its absolute scores should not be read as one**. The vendor's 82.7 configuration, which removes wall-clock limits, would likewise not conform.

## Agent timeout and its effect on the comparison

Every Terminal-Bench 2.1 task carries a native agent timeout, declared as `[agent] timeout_sec` in the task definition. Across the 89 tasks these span 10 to 200 minutes; 48 tasks sit at 15 minutes and 17 at 30. This run applies them at ×1: every job config caps the agent at exactly the task's own declared limit, with no multiplier or override. A cell that reaches its timeout is terminated, but termination is **not** automatic failure: the verifier still runs against whatever state exists, and terminated cells do sometimes pass.

Classifying every cell against its own task's declared timeout — the ground truth, available for all 89 tasks — gives:

| Cell group | Maka | OpenCode |
| --- | ---: | ---: |
| Finished inside its native timeout | 99/124 (79.8%) | 109/148 (73.6%) |
| Terminated at its native timeout | 8/80 (10.0%) | 27/119 (22.7%) |
| Duration not recorded | 63 cells (55 passes) | 0 cells |

Three conclusions, one of which cuts against the headline:

1. **The lead is not an artifact of differential timeouts.** Restricted to cells that finished inside their own timeout, Maka still leads, 79.8% to 73.6% (+6.2 pp). Note this is *smaller* than the headline +9.7 pp, not larger.
2. **The timeout binds OpenCode harder than Maka.** OpenCode is terminated in 119 cells against Maka's 80, and it converts terminated cells into passes more often (22.7% versus 10.0%, Fisher exact `p = 0.023`).
3. **Part of Maka's exclusive-win margin coincides with OpenCode running out of clock.** In 35 of the 46 cells Maka won exclusively, OpenCode was terminated at its deadline. A more generous agent timeout — the vendor's own Terminus-2 configuration uses `timeout=4h` — could therefore narrow the headline gap. This report cannot estimate by how much.

Two limits apply to everything in this section:

- **Duration completeness is one-sided.** 63 of Maka's 267 cells (23.6%) carry `durationMs = 0` and `steps = 0` placeholders — adopted cells whose timing metrics were never hydrated, 56 from primary runs and 7 from replacements, spread evenly across rounds (23 / 20 / 20). OpenCode has none. 55 of those 63 are passes, so **34% of Maka's passes are absent from every duration statistic in this report**, including the speed comparison above. Recovering them from the run artifacts would be required before treating any timing figure as complete.
- **"Finished inside its timeout" is a post-treatment condition.** Grouping by whether a cell completed conditions on a variable each agent influences: an agent that stops early lands in the finished group and depresses its own conditional pass rate, while an agent that runs to the deadline moves its failures out of that group and raises it. The comparison is descriptive, not a causal or counterfactual estimate of behavior under a longer timeout.

Separately, the `errorClass` asymmetry must not be read as a capability difference: Maka's 105 failures are labelled 72 `budget_exhausted`, 27 `verification_failed`, and 6 `max_tokens`, while all 131 OpenCode failures are labelled `verification_failed` because that adapter does not surface budget exhaustion at all.

## Where the difference appears

Clusters with at least 8 tasks are shown individually; the 11 clusters holding 1–5 tasks each are pooled because per-cluster differences at that size are noise. The `p` column applies the same task-level exact sign-flip test used for the headline, on each cluster's per-task three-round pass-count differences. Pooling a cluster's three rounds into a task-round McNemar would repeat exactly the pseudo-replication the Inference section rejects, so it is not done here.

| Task cluster | Tasks | Maka | OpenCode | Difference | Task-level `p` | Smallest `p` this cluster could produce |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Security | 8 | 23/24 (95.8%) | 13/24 (54.2%) | **+41.7 pp** | `0.0625` | `0.0625` |
| System administration | 9 | 17/27 (63.0%) | 8/27 (29.6%) | +33.3 pp | `0.0156` | `0.0156` |
| Data science | 8 | 17/24 (70.8%) | 13/24 (54.2%) | +16.7 pp | `0.313` | `0.0625` |
| Scientific computing | 8 | 9/24 (37.5%) | 7/24 (29.2%) | +8.3 pp | `1.00` | `1.00` |
| Software engineering | 26 | 39/78 (50.0%) | 43/78 (55.1%) | −5.1 pp | `0.398` | `0.0039` |
| Small clusters (11 clusters, 1–5 tasks each) | 30 | 57/90 (63.3%) | 52/90 (57.8%) | +5.6 pp | — | — |

**No cluster survives correction for the 16 clusters examined**, and none can: security and system administration are already at the smallest `p` their task counts allow, because every non-tied task in each favors Maka. The right reading is that both show the strongest directional signal their size permits while remaining underpowered, not that either is established. Software engineering, the largest cluster at 26 tasks, is **not distinguishable between the arms** (`p = 0.398`); the −5.1 pp figure should not be read as an OpenCode advantage.

Across the 89 tasks, 25 are passed by both arms in all three rounds and 16 are passed by neither arm in any round. Maka's net gain is concentrated rather than universal: 28 tasks favor Maka by three-round pass count, 11 favor OpenCode, and 50 tie.

## Token economy

The account-plan manifest records observed USD cost as `$0`; that is an accounting profile, not a claim that the run was free. Token totals are the comparable resource measure. `total` is the controller's adopted input-plus-output total; cached input is included, and reasoning is reported within the provider's output accounting rather than added again.

Maka records **211.60M** adopted tokens against OpenCode's **122.15M**, an excess of **+89.45M (1.73×)**. Attributing that excess by paired outcome shows it is not the price of Maka's wins:

| Paired outcome | Cells | Maka | OpenCode | Excess | Share of total excess | Excess per cell |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Both arms pass | 116 | 50.05M | 50.36M | **−0.31M** | −0.3% | −0.003M |
| Maka only passes | 46 | 40.43M | 18.45M | +21.98M | 24.6% | +0.478M |
| OpenCode only passes | 20 | 8.88M | 10.52M | −1.64M | −1.8% | −0.082M |
| **Neither arm passes** | 85 | 112.24M | 42.81M | **+69.43M** | **77.6%** | **+0.817M** |
| **Total** | 267 | 211.60M | 122.15M | +89.45M | 100% | +0.335M |

- **On the tasks both arms solve, Maka is marginally cheaper.** The claim that Maka costs more does not hold on the shared-solvable set.
- **Buying exclusive wins is the smaller and more defensible component.** 46 exclusive Maka passes cost about 0.48M extra tokens each.
- **The dominant term is spend on cells neither arm solves.** 77.6% of the excess lands on the 85 both-fail cells, at about 0.82M extra tokens per cell. The mechanism is *not* that Maka persists while OpenCode gives up: on those cells the two arms run for the same wall-clock time (median 14.5 versus 15.0 minutes) and are terminated at their native timeouts at the same rate (59 versus 57 cells). What differs is how many steps fit in that time: Maka takes **3,383 against OpenCode's 1,548** (median 32 versus 18). Per step the two arms ask the model to generate near-identical amounts — 817 versus 757 output tokens at the median — so decode work per step is comparable and the model produces tokens at the same rate for both. Maka simply completes each step faster (median 22.8 versus 56.6 seconds) and therefore runs more of them before the clock stops. Prompt size per step is not matched, and runs the other way: OpenCode carries roughly twice Maka's input per step at the median (15.4K versus 7.2K). Whether a step budget would cut the waste on unsolved cells without costing exclusive wins is untested here.

These 85 cells are also not "tasks nobody solves": only 48 of them come from the 16 tasks that never pass in any round, while the other 37 come from 25 tasks that do pass in at least one round.

Per-cell distribution and price weighting:

| Adopted usage | Maka | OpenCode | Maka / OpenCode |
| --- | ---: | ---: | ---: |
| All three rounds, raw total | 211.60M | 122.15M | 1.73× |
| Cached share of input tokens | 94.88% | 93.91% | — |
| Cached share of total tokens | 90.43% | 90.33% | — |
| Per-cell p50 | 231K | 239K | 0.97× |
| Per-cell p90 | 1.73M | 1.12M | 1.54× |
| Per-cell p95 | 3.07M | 1.78M | 1.73× |
| Per-cell maximum | 14.39M | 5.05M | 2.85× |

Two accounting caveats that both work against Maka, stated rather than left implicit:

- **The raw 1.73× understates the economic gap.** About 90% of both arms' totals is cached input, which is typically priced near 10% of uncached input, while Maka's output-token ratio is 2.12×. Reweighting cache reads to 0.1× and output to 3–4× of uncached input — a price *shape* assumption, not Z.ai's published rates, hence a range — puts the ratio at **1.84×–1.88×** and the cost per accepted pass at +54% to +57% (raw per-pass is 1.31M versus 0.90M, or +45%).
- **The pooled 1.73× is the most favorable framing available.** Per round the ratio worsens monotonically: **1.34× → 1.90× → 2.12×**. Pooling all three rounds dilutes the two immutable-subject rounds, where the ratio is 2.01×.

The typical cell is not the problem: Maka's per-cell median is slightly *below* OpenCode's. The tail is, and it is concentrated in the both-fail quadrant. The five largest cells are `install-windows-3.11` round 2 (Maka 14.39M, OpenCode 0.35M, both fail), `install-windows-3.11` round 1 (12.34M vs 1.18M, both fail), `mteb-leaderboard` round 0 (10.72M vs 1.83M, both pass), `make-doom-for-mips` round 1 (9.32M vs 0.17M, both fail), and `mailman` round 2 (8.85M vs 1.39M, both fail).

Maka's context-budget tool-result pruning was enabled in all rounds and its cached share of input slightly exceeds OpenCode's, so the data do not support a claim that pruning was absent or ineffective. Attributing the tail to a specific internal policy requires trace-level ablation, which this report does not attempt.

## Setup

| Dimension | Value |
| --- | --- |
| Benchmark | Terminal-Bench 2.1 revision `d49e28f1e4ddd13d289e85a5f312a66750951932`, 89 frozen tasks |
| Task-source fingerprint | `sha256:456826aa4c47ed309716c964c96d2a3acc998764ebc84f3e8449c807d74bd4e7` |
| Model | Z.ai Coding Plan `glm-5.2`, reasoning effort `max` |
| Maka subject | `ee7e4ba5d2323f31be5ab3ec4b4a5fbc847f9a83` (rounds 1–2 in full; round 0 in 176 of 178 cells) |
| Maka adapter | `maka_agent:MakaAgent`, host runtime with task-container tools |
| OpenCode adapter | `opencode_agent:MakaOpenCodeAgent`, pinned OpenCode 1.17.18 |
| OpenCode host-toolchain fingerprint | `sha256:9c2ba35763bb5fb59c16ddcfd036eb172dea2776f494a347f2dd209ff7564649` |
| Composed harness toolchain fingerprint | `sha256:297d922233b1611271120393e25d2c239902dbf502bb147cc6a9a01ba481cd9f` |
| Prompt | Empty external system prompt on both arms |
| Attempt policy | One model attempt per cell; only attempts without a complete verifier result are replacement-eligible |
| Timeout | Task-native agent timeout ×1 plus 900-second outer setup/teardown grace |
| Execution | Four task pairs concurrent, both arms parallel in rounds 1–2 |
| Pricing profile | Z.ai Coding Plan account plan, `$0` observed-price placeholder with token totals retained |

The comparison is between two product-agent harnesses sharing a model, task, verifier, reasoning effort, timeout policy, and external prompt. Their internal instructions, tool surfaces, execution loops, and context policies are intentionally not identical.

## Outcome accounting

Every round has exactly one adopted scored result for each of 89 tasks × two arms. The analysis locks the first complete verifier pass/fail for a task-arm. A replacement may fill an infrastructure or persistence gap only when no complete verifier result exists; a later sampled outcome cannot replace an earlier scored result.

| Scope | Recorded attempts | Adopted cells | Replacement attempts | Adopted replacements | Unscored replacements | Discarded scored companions |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Round 0 | 206 | 178 | 28 | 8 | 11 | 9 |
| Round 1 | 236 | 178 | 58 | 29 | 12 | 17 |
| Round 2 | 220 | 178 | 42 | 5 | 12 | 25 |
| **All rounds** | **662** | **534** | **128** | **42** | **35** | **51** |

All recorded attempts use 421.74M tokens. The adopted outcomes use 333.74M; 87.99M belongs to unscored or later-discarded attempts and is excluded from the arm comparison. The adjacent CSV contains only the adopted cells.

Round 2 exposed a controller-classification edge case: eight Maka primary cells completed their verifier with reward 0, then failed while persisting a runtime event with `RuntimeEvent is not losslessly serializable`. The same-attempt verifier failures are locked as Pass@1=0 rather than redrawn. The reconciliation requires all of the following evidence: Maka arm, complete failed verifier attempts with reward 0, generic post-run error status, and the exact trace-write failure in that trial's run header. Thirteen attempts match the evidence; eight are the first adopted results, while five later attempts are discarded by first-result locking.

Once `build-pov-ray` / OpenCode supplied the last missing result in round 2, five still-running companion jobs were intentionally terminated as redundant. Their `infra_failed` rows and the replacement run's failed background journal are retained but not adopted.

## Runs and provenance

| Round | Run family |
| --- | --- |
| Round 0 | `glm-5.2-maka-vs-opencode-tbench-2.1-canary5-main-ee7e4ba5d-20260727-v1`, `...remaining87-2x4-main-ee7e4ba5d-20260728-v1`, and five replacement runs |
| Round 1 | `glm-5.2-maka-vs-opencode-tbench-2.1-prefx-replication-r2-ee7e4ba5d-20260728-v1` and replacements 1–6 |
| Round 2 | `glm-5.2-maka-vs-opencode-tbench-2.1-prefx-replication-r3-ee7e4ba5d-20260729-v1` and replacements 1–2 |

Rounds 1 and 2, and every result adopted into them, execute the detached subject at `ee7e4ba5d`. During round 1 that detached worktree path was deleted externally and restored at the same commit; only the resulting unscored infrastructure-loss window was replaced. Replacement 1 also recorded incorrectly composed wrapper fingerprint strings, but direct source identity, frozen task fingerprint, and pinned OpenCode checksums match. No scored result from the affected window was redrawn.

Round 0's two-cell subject caveat and its sensitivity checks are stated in Results.

No HTTP 429 event was observed in the recorded round-0/round-1 network evidence or the round-2 preflight probes. There is no remaining unscored adopted cell. Oracle registry annotations were unavailable; official task verifiers remain the Pass@1 authority.

## Caveats

- The suite contains the same 89 tasks in every round. Task-round totals are operational observations; all inference uses the task as the unit, and the pooled McNemar value is descriptive only.
- Three repetitions reduce single-run noise but do not estimate generalization beyond this frozen benchmark. Per-round differences range from +4 to +14 tasks.
- Round 0 carries a two-cell subject-provenance caveat; both sensitivity checks leave the conclusion unchanged.
- The run applies each task's native agent timeout at ×1. That timeout binds OpenCode more often than Maka, and OpenCode was terminated at its deadline in 35 of the 46 cells Maka won exclusively, so the headline margin is partly contingent on this policy. The timeout section's conditional pass rates additionally group on a post-treatment variable and are descriptive only.
- 63 of Maka's 267 cells carry unhydrated `durationMs = 0` / `steps = 0` placeholders and OpenCode has none, so every duration statistic in this report omits 34% of Maka's passes. Timing figures should be treated as provisional until those metrics are recovered.
- The comparison is a product-agent harness A/B, not an ablation of one internal policy. Cluster and token-attribution differences are descriptive, not causal attribution.
- No task cluster reaches significance at the task unit after correcting for the 16 clusters examined; the cluster table is a description of where the differences sit, not a set of established sub-results.
- The published GLM-5.2 Terminal-Bench 2.1 figures cannot be placed on a common time budget with this run: the Terminus-2 footnote's `timeout=4h` is ambiguous under Harbor's timeout resolution, and the best-harness figure removed wall-clock limits entirely. With 39–45% of cells here terminated by the clock, absolute scores are not comparable to published figures in either direction, and this report makes no estimate of what either arm would score under a different budget.
- This run sets `verifier.override_timeout_sec = 3720`, a field Harbor's leaderboard validator rejects. It is applied identically to both arms and grants verification rather than agent time, so it does not bias the comparison, but this run is not a conformant leaderboard submission.
- Replacement and reconciliation decisions are auditable but add operational complexity. The first-valid lock prevents score shopping; all later scored companions remain in the attempt ledgers.
- Account-plan `$0` is not a spend estimate, and the 1.84×–1.88× weighted range rests on a price-shape assumption rather than Z.ai's published rates.
- Raw traces, provider payloads, verifier output, and local analysis ledgers remain git-excluded. This report commits only aggregate prose and a redaction-minimal per-task CSV.

## Integrity

SHA-256 of the frozen local evidence and committed CSV:

| Source | SHA-256 |
| --- | --- |
| Round 0 summary | `5f2692be9c8156ecb09a85708045a39ce224032ee0128e8076c879673d871674` |
| Round 0 adopted cells | `e2e6e623f0ff0f451b1c860615831af930566dc55fdf44ab0d800eb112bda3f8` |
| Round 1 summary | `cf5cf6980cd6ac54669dac9247ce3636a70f2b94acb3bd8e28cc93388be63cc7` |
| Round 1 adopted cells | `bb1d0b8f4b9597ce657160165a5fa0ff2ea32ebef8b219d53168f65cad1da593` |
| Round 2 summary | `8ca269a57e972a6a795b4cc948accd5964567a0911b418e56869bdf4b7854a29` |
| Round 2 adopted cells | `022e9d3b477a7ec121a8b797f8b62c7681adc0cfb5199bd7f725541990872ad5` |
| Round 2 reconciliation ledger | `72331e08a1f1f7520766c45a933722cd3d116879b908f65b80f9e90772360fd0` |
| Committed per-task CSV | `5db04fa7f9cb30d352822a2a524ba8861497f4b42f25fe4ddf52f7d73d428c18` |

Every statistic in this report is recomputable from the three adopted-cells ledgers above, plus the frozen task definitions for timeouts and categories. Definitions used: the permutation test enumerates all 2^39 sign assignments of the 39 non-tied tasks' three-round pass-count differences, exactly rather than by sampling; the bootstrap resamples the 89 tasks 100,000 times with seed 11; a cell counts as terminated at its timeout when it has a recorded duration reaching 95% of its own task's `[agent] timeout_sec`; cells with `durationMs = 0` are excluded from every duration-based figure rather than assigned to a group; cluster `p` values use the same task-level exact sign-flip procedure as the headline.

## Artifact pointers

Local raw artifacts are git-excluded under `.agents-workspace-data/harness-ab/terminal-bench-2.1/`. The authoritative composed analyses are:

- `round0-analysis/`
- `glm-5.2-maka-vs-opencode-tbench-2.1-prefx-replication-r2-ee7e4ba5d-20260728-v1/analysis/`
- `glm-5.2-maka-vs-opencode-tbench-2.1-prefx-replication-r3-ee7e4ba5d-20260729-v1/analysis/`
- `prefx-replication-r2-r3-analysis/`
- `prefx-round0-r2-r3-analysis/`
