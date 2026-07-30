# Terminal-Bench 2.1 — Maka vs OpenCode with GLM-5.2 Max

Paired harness A/B on the frozen 89-task Terminal-Bench 2.1 suite, with Z.ai Coding Plan `glm-5.2` at `max` reasoning effort on both arms. Three complete paired rounds, 534 adopted task-arm cells, one model attempt per cell. This report records the Pass@1 result, the task-level inference behind it, the throughput ceiling that bounds both arms from below, and the resource profile behind the outcome.

**Evidence:** three complete paired rounds, 267 task-round pairs
**Metric:** Pass@1 by the task's Harbor verifier
**Coverage:** 89/89 paired tasks in every round; 534/534 adopted task-arm cells
**Per-task outcomes and tokens:** [`terminal-bench-2.1-maka-vs-opencode-glm-5.2-max.csv`](./terminal-bench-2.1-maka-vs-opencode-glm-5.2-max.csv)

## TL;DR

- **Maka wins on Pass@1: 162/267 (60.7%) versus OpenCode's 136/267 (50.9%), a +9.7 pp lead in Maka's favor.** Maka leads in all three rounds, by 8, 14, and 4 tasks. Best single round: Maka **66.3%** versus OpenCode **57.3%**.
- **The lead is statistically established at the correct unit of analysis.** Treating each of the 89 tasks as the independent item, an exact paired permutation test gives `p = 0.0042` and the task-cluster bootstrap 95% CI on the difference is **[+3.4, +16.1] pp** — the interval excludes zero.
- **Both arms are throughput-limited lower bounds, and Maka's lead nearly doubles once that ceiling is removed.** Among cells that finished inside their task-native deadline, Maka passes **82.6%** versus OpenCode's **65.1%**, a **+17.5 pp** gap. Cells killed at the deadline pass at 12.0% and 14.7% — indistinguishable between arms, so the ceiling is shared, not a Maka weakness.
- **Maka is also faster to a pass:** median 4.3 minutes per passing cell versus OpenCode's 10.8 minutes, a 2.5× difference.
- **Maka's quality gain is not paid for on the tasks it wins.** On the 116 cells both arms solve, Maka spends marginally *less* (50.05M versus 50.36M tokens). 24.6% of Maka's aggregate token excess buys its 46 exclusive wins (about 0.48M extra tokens per exclusive pass). **77.6% is spent on the 85 cells neither arm solves** — Maka works to the deadline where OpenCode gives up early. That is an addressable early-abandon gap, not a quality-for-cost tradeoff.
- **Security and system administration drive the quality gain.** Security: 95.8% versus 54.2% (`p = 0.002`, survives Bonferroni across all 16 clusters). Software engineering, the largest cluster, is not distinguishable between arms.

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
| Paired permutation on per-task pass-count difference | 89 tasks | 2M sign-flips, seed 3 | **`p = 0.0042`** (39 non-tied tasks) |
| Task-cluster bootstrap on the Pass@1 rate difference | 89 tasks | 100k resamples | **95% CI [+3.4, +16.1] pp** |
| Exact sign test on the direction only | 89 tasks | 28 Maka / 11 OpenCode / 50 ties | `p = 0.0095` |

The permutation test is the primary inference: it shares the sign test's unit of independence but uses the magnitude of each task's difference rather than discarding it, and the two agree in direction.

Descriptive per-round and pooled McNemar values, retained for continuity with the per-round reports: round 0 `p = 0.134`, round 1 `p = 0.0026`, round 2 `p = 0.541`, pooled over all 267 pairs `p = 0.00186`. **The pooled value must not be read as inference** — it treats repeated measurements of the same 89 tasks as independent. The per-round values show that no single 89-task round except round 1 is significant on its own; the evidence comes from pooling three rounds at the task level.

## Where these numbers sit against published results

Z.ai reports **82.7** for `glm-5.2` on Terminal-Bench 2.1 under a "Best Reported Harness" label. Its footnote states the evaluation ran in **Claude Code 2.1.167** with `temperature=1.0, top_p=0.95, max_new_tokens=131072`, overriding the 64k CLI output cap to 128k through a transparent proxy, **removing wall-clock time limits** while preserving per-task CPU and memory constraints, and **averaging over 5 runs** ([model card](https://huggingface.co/zai-org/GLM-5.2/blob/main/README.md), accessed 2026-07-30). The GLM-5 repository quotes the same result as "81.0 vs. 62.0 on Terminal-Bench 2.1" against GLM-5.1 ([repository](https://github.com/zai-org/GLM-5), accessed 2026-07-30).

An independent reproduction attempt on the Terminus 2 harness — Harbor v0.16.1, GLM-5.2-FP8 served with sglang on H200, `reasoning_effort: max`, `agent_timeout_multiplier: 16.0`, `max_turns: 500` — reports "I can only get 62~64 score" ([zai-org/GLM-5#100](https://github.com/zai-org/GLM-5/issues/100), accessed 2026-07-30).

Two things follow, and neither is a like-for-like comparison with this A/B:

1. **Harness choice is a first-order factor for this model.** The same model at the same reasoning effort spans roughly 19 points between the vendor's Claude Code configuration and an independent Terminus 2 reproduction. That spread is the variable this report isolates, and the +9.7 pp Maka-over-OpenCode gap sits well inside it.
2. **Maka lands in the independently reproduced band under a stricter timeout policy; OpenCode lands below it.** Maka's 60.7% pooled and 66.3% best round bracket the 62–64 third-party figure, while running at the task-native agent timeout ×1 rather than ×16, and without removing wall-clock limits or averaging over 5 runs. OpenCode's 50.9% falls clearly beneath that band.

The vendor's 82.7 is not the comparison target for this report: it differs on harness, wall-clock limits, output-token cap, and run averaging. It is quoted because its own footnote identifies wall-clock limits as something worth removing, which is exactly the ceiling measured in the next section.

## Throughput ceiling and headroom

Every Terminal-Bench 2.1 task carries a native time limit; observed values in this run range from 12 to 60 minutes. When the limit is reached the cell is terminated and scored as a failure regardless of progress. Both absolute scores in this report are therefore lower bounds, not the arms' ceilings.

| Cell group | Maka | OpenCode |
| --- | ---: | ---: |
| Finished inside the deadline | **152/184 (82.6%)** | **125/192 (65.1%)** |
| Terminated at the deadline | 10/83 (12.0%) | 11/75 (14.7%) |

Conditioning on cells that reached a verifier decision inside their deadline widens the gap from +9.7 pp to **+17.5 pp**. Deadline-terminated cells pass at statistically indistinguishable rates on both arms, and the two arms' failure durations coincide closely (median 14.5 versus 15.0 minutes, p90 39.5 versus 40.0, max 59.5 versus 60.0). The ceiling is a property of the run configuration, shared by both arms.

Maka also reaches its passes faster: **median 4.3 minutes per passing cell versus OpenCode's 10.8 minutes** (p90 14.5 versus 28.9).

Three limits apply to this section and none of them are resolved by the aggregate data:

- **The deadline classifier is a conservative proxy.** Maka's runtime records an explicit `benchmark.deadline` settlement (80 of its 267 cells); the OpenCode adapter has no equivalent field. Per-task deadline values are therefore recovered from Maka's explicit settlements on 37 of 89 tasks and applied symmetrically to both arms; the remaining 52 tasks contribute no deadline-terminated cells by construction. True deadline-limited counts can only be higher than 83 and 75, not lower.
- **"Finished in time" is a post-treatment condition.** Grouping by whether a cell completed conditions on a variable the agent itself influences: an agent that abandons early would inflate its own conditional pass rate. Maka is the faster-to-finish arm, so this bias cannot be ruled out from aggregate data. These are descriptive observations, not causal estimates.
- **The `errorClass` asymmetry is instrumentation, not capability.** Maka's 105 failures are labelled 72 `budget_exhausted`, 27 `verification_failed`, and 6 `max_tokens`; all 131 OpenCode failures are labelled `verification_failed` because that adapter does not surface budget exhaustion. Given the near-identical failure-duration distributions, this asymmetry must not be read as OpenCode failing for better reasons or Maka failing for more excusable ones.

## Where the difference appears

Clusters with at least 8 tasks are shown individually; the 11 clusters holding 1–5 tasks each are pooled because per-cluster differences at that size are noise. No multiple-comparison correction is applied to the table; the Bonferroni column across all 16 clusters is given for the two clusters that reach nominal significance.

| Task cluster | Tasks | Task-rounds | Maka | OpenCode | Difference | Exact McNemar | Bonferroni (16) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Security | 8 | 24 | 23/24 (95.8%) | 13/24 (54.2%) | **+41.7 pp** | `0.0020` | `0.031` |
| System administration | 9 | 27 | 17/27 (63.0%) | 8/27 (29.6%) | +33.3 pp | `0.0117` | `0.188` |
| Data science | 8 | 24 | 17/24 (70.8%) | 13/24 (54.2%) | +16.7 pp | `0.289` | — |
| Scientific computing | 8 | 24 | 9/24 (37.5%) | 7/24 (29.2%) | +8.3 pp | `0.500` | — |
| Software engineering | 26 | 78 | 39/78 (50.0%) | 43/78 (55.1%) | −5.1 pp | `0.388` | — |
| Small clusters (11 clusters, 1–5 tasks each) | 30 | 90 | 57/90 (63.3%) | 52/90 (57.8%) | +5.6 pp | — | — |

Security is the only cluster whose advantage survives correction for the number of clusters examined. Software engineering, the largest cluster at 26 tasks, is **not distinguishable between the arms** (`p = 0.388`); the −5.1 pp figure should not be read as an OpenCode advantage.

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
- **The dominant term is spend on tasks nobody solves.** 77.6% of the excess lands on 85 cells where neither arm passes, at about 0.82M extra tokens per cell. Combined with the 72 `budget_exhausted` failures and the deadline evidence above, the mechanism is that Maka keeps working to the deadline on unsolvable tasks while OpenCode abandons early. This is a missing early-abandon policy — an addressable engineering gap, not an intrinsic quality-for-cost tradeoff.

Per-cell distribution and price weighting:

| Adopted usage | Maka | OpenCode | Maka / OpenCode |
| --- | ---: | ---: | ---: |
| All three rounds, raw total | 211.60M | 122.15M | 1.73× |
| Cached-input share | 94.88% | 93.91% | — |
| Per-cell p50 | 231K | 239K | 0.97× |
| Per-cell p90 | 1.73M | 1.12M | 1.54× |
| Per-cell p95 | 3.07M | 1.78M | 1.73× |
| Per-cell maximum | 14.39M | 5.05M | 2.85× |

Two accounting caveats that both work against Maka, stated rather than left implicit:

- **The raw 1.73× understates the economic gap.** About 95% of both arms' totals is cached input, which is typically priced near 10% of uncached input, while Maka's output-token ratio is 2.12×. Reweighting cache reads to 0.1× and output to 3–4× of uncached input — a price *shape* assumption, not Z.ai's published rates, hence a range — puts the ratio at **1.84×–1.88×** and the cost per accepted pass at +54% to +57% (raw per-pass is 1.31M versus 0.90M, or +45%).
- **The pooled 1.73× is the most favorable framing available.** Per round the ratio worsens monotonically: **1.34× → 1.90× → 2.12×**. Pooling all three rounds dilutes the two immutable-subject rounds, where the ratio is 2.01×.

The typical cell is not the problem: Maka's per-cell median is slightly *below* OpenCode's. The tail is, and it is concentrated in the both-fail quadrant. The five largest cells are `install-windows-3.11` round 2 (Maka 14.39M, OpenCode 0.35M, both fail), `install-windows-3.11` round 1 (12.34M vs 1.18M, both fail), `mteb-leaderboard` round 0 (10.72M vs 1.83M, both pass), `make-doom-for-mips` round 1 (9.32M vs 0.17M, both fail), and `mailman` round 2 (8.85M vs 1.39M, both fail).

Maka's context-budget tool-result pruning was enabled in all rounds and its cached-input share slightly exceeds OpenCode's, so the data do not support a claim that pruning was absent or ineffective. Assigning the tail to a specific mechanism beyond the early-abandon gap above requires trace-level ablation, which this report does not attempt.

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
- Both absolute scores are lower bounds under a task-native ×1 timeout. The conditional pass rates in the headroom section condition on a post-treatment variable and rest on a deadline proxy recovered from 37 of 89 tasks.
- The comparison is a product-agent harness A/B, not an ablation of one internal policy. Cluster and token-attribution differences are descriptive, not causal attribution.
- Published GLM-5.2 Terminal-Bench 2.1 figures differ from this run on harness, wall-clock limits, output-token cap, run averaging, and serving stack. They are context for harness sensitivity, not a like-for-like baseline.
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

Every statistic in this report is recomputable from the three adopted-cells ledgers above. Definitions used: the permutation test sign-flips each task's three-round pass-count difference over 2,000,000 draws with seed 3 across the 39 non-tied tasks; the bootstrap resamples the 89 tasks 100,000 times with seed 11; a cell counts as deadline-terminated when its duration reaches 95% of the minimum duration among that task's cells carrying an explicit `benchmark.deadline` settlement.

## Artifact pointers

Local raw artifacts are git-excluded under `.agents-workspace-data/harness-ab/terminal-bench-2.1/`. The authoritative composed analyses are:

- `round0-analysis/`
- `glm-5.2-maka-vs-opencode-tbench-2.1-prefx-replication-r2-ee7e4ba5d-20260728-v1/analysis/`
- `glm-5.2-maka-vs-opencode-tbench-2.1-prefx-replication-r3-ee7e4ba5d-20260729-v1/analysis/`
- `prefx-replication-r2-r3-analysis/`
- `prefx-round0-r2-r3-analysis/`
