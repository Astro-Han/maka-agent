# Terminal-Bench 2.1 — Maka vs OpenCode with GLM-5.2 Max

Paired harness A/B on the frozen 89-task Terminal-Bench 2.1 suite, with Z.ai Coding Plan `glm-5.2` at `max` reasoning effort on both arms. This report records two clean replications against the same detached Maka subject plus one earlier accepted round, separates repeated-task evidence from task-round counts, and reports the token tradeoff behind the Pass@1 result.

**Primary evidence:** two complete replications at Maka `ee7e4ba5d2323f31be5ab3ec4b4a5fbc847f9a83`
**Supporting evidence:** one earlier replacement-policy aggregate with a disclosed subject-provenance caveat
**Metric:** Pass@1 by the task's Harbor verifier
**Coverage:** 89/89 paired tasks in every round; 534/534 adopted task-arm cells
**Per-task outcomes and tokens:** [`terminal-bench-2.1-maka-vs-opencode-glm-5.2-max.csv`](./terminal-bench-2.1-maka-vs-opencode-glm-5.2-max.csv)

## TL;DR

- **Maka has the higher Pass@1.** Across the two immutable-subject replications, Maka passes **103/178 task-rounds (57.9%)** and OpenCode **85/178 (47.8%)**, a **+10.1 pp** difference. Maka leads in both rounds, by 14 and 4 tasks.
- **The earlier round supports the same direction but is not an identical-subject replication.** Including it gives Maka **162/267 (60.7%)** and OpenCode **136/267 (50.9%)**, a **+9.7 pp** difference; Maka leads all three rounds.
- **The advantage survives a task-level repeated-measure check.** Over the two clean replications, 24 tasks favor Maka by pass count, 10 favor OpenCode, and 55 tie (two-sided exact sign test, `p = 0.0243`). Across all three rounds the split is 28 / 11 / 50 (`p = 0.00948`). These are post-hoc checks on this frozen suite, not claims about 267 independent tasks.
- **OpenCode is substantially more token-efficient.** Across all three rounds, Maka records **211.60M** adopted tokens versus **122.15M** for OpenCode (**1.73×**). Median per-cell usage is nearly tied, but Maka's p90, p95, and maximum are much larger. The aggregate disadvantage is a heavy-tail problem rather than a typical-cell problem.
- **The product conclusion is a tradeoff, not total dominance.** Maka buys a higher success rate at materially higher and less predictable token use. Security and system-administration tasks drive much of the quality gain; OpenCode is slightly ahead on software-engineering tasks.

## Results

### Primary: two clean replications

Both replications execute the tracked-clean detached Maka checkout at `ee7e4ba5d2323f31be5ab3ec4b4a5fbc847f9a83`, the same frozen task source, and pinned OpenCode 1.17.18.

| Round | Maka | OpenCode | Difference | Both pass / Maka only / OpenCode only / both fail |
| --- | ---: | ---: | ---: | ---: |
| New round 1 | 56/89 (62.9%) | 42/89 (47.2%) | +14 tasks (+15.7 pp) | 39 / 17 / 3 / 30 |
| New round 2 | 47/89 (52.8%) | 43/89 (48.3%) | +4 tasks (+4.5 pp) | 33 / 14 / 10 / 32 |
| **Combined task-rounds** | **103/178 (57.9%)** | **85/178 (47.8%)** | **+18 (+10.1 pp)** | **72 / 31 / 13 / 62** |

The ordinary exact McNemar calculation over the 178 task-round pairs is `p = 0.00956`. Because the same 89 tasks appear twice, those 178 pairs are repeated measurements, not independent benchmark items. A task-level robustness check first compares each task's two-round pass count: 24 tasks favor Maka, 10 favor OpenCode, and 55 tie; the two-sided exact sign-test result is `p = 0.0243`.

### Supporting historical round and three-round aggregate

| Round | Maka | OpenCode | Difference | Both pass / Maka only / OpenCode only / both fail |
| --- | ---: | ---: | ---: | ---: |
| Historical round 0 | 59/89 (66.3%) | 51/89 (57.3%) | +8 tasks (+9.0 pp) | 44 / 15 / 7 / 23 |
| New round 1 | 56/89 (62.9%) | 42/89 (47.2%) | +14 tasks (+15.7 pp) | 39 / 17 / 3 / 30 |
| New round 2 | 47/89 (52.8%) | 43/89 (48.3%) | +4 tasks (+4.5 pp) | 33 / 14 / 10 / 32 |
| **Combined task-rounds** | **162/267 (60.7%)** | **136/267 (50.9%)** | **+26 (+9.7 pp)** | **116 / 46 / 20 / 85** |

The task-round McNemar value is `p = 0.00186`, shown for descriptive continuity with the per-round reports. It must not be read as if 267 different tasks were sampled. Collapsing the repetitions to one direction per task gives 28 tasks favoring Maka, 11 favoring OpenCode, and 50 ties; the two-sided exact sign-test result is `p = 0.00948`.

The round-to-round gap varies from +4 to +14 tasks. The direction is stable, but a single 89-task run does not reliably estimate the size of the advantage.

## Where the difference appears

The cluster counts below pool three task-round observations per task and are descriptive; no correction for multiple cluster comparisons is applied.

| Task cluster | Task-rounds | Maka | OpenCode | Difference |
| --- | ---: | ---: | ---: | ---: |
| Security | 24 | 23/24 (95.8%) | 13/24 (54.2%) | +41.7 pp |
| System administration | 27 | 17/27 (63.0%) | 8/27 (29.6%) | +33.3 pp |
| Debugging | 15 | 11/15 (73.3%) | 6/15 (40.0%) | +33.3 pp |
| Data science | 24 | 17/24 (70.8%) | 13/24 (54.2%) | +16.7 pp |
| Software engineering | 78 | 39/78 (50.0%) | 43/78 (55.1%) | −5.1 pp |
| Mathematics | 12 | 5/12 (41.7%) | 8/12 (66.7%) | −25.0 pp |
| Model training | 12 | 7/12 (58.3%) | 8/12 (66.7%) | −8.3 pp |

Across the 89 tasks, 25 are passed by both arms in all three rounds and 16 are passed by neither arm in any round. Maka's net gain is concentrated rather than universal: 28 tasks favor Maka by three-round pass count, 11 favor OpenCode, and half the suite ties.

## Token economy

The account-plan manifest records observed USD cost as `$0`; that is an accounting profile, not a claim that the run was free or that the arms had equal economic cost. Token totals are the comparable resource measure. `total` is the controller's adopted input-plus-output total; cached input is included, and reasoning is reported within the provider's output accounting rather than added again.

| Adopted usage | Maka | OpenCode | Maka / OpenCode |
| --- | ---: | ---: | ---: |
| Two clean replications | 144.04M | 71.60M | 2.01× |
| All three rounds | 211.60M | 122.15M | 1.73× |
| All-three cached-input share | 94.88% | 93.91% | — |
| All-three per-cell p50 | 231K | 239K | 0.97× |
| All-three per-cell p90 | 1.73M | 1.12M | 1.54× |
| All-three per-cell p95 | 3.07M | 1.78M | 1.73× |
| All-three per-cell maximum | 14.39M | 5.05M | 2.85× |

The p50 is slightly lower for Maka, so the typical cell does not explain the 89.45M-token aggregate difference. The tail does:

1. `install-windows-3.11`, new round 2: Maka 14.39M, OpenCode 0.35M.
2. `install-windows-3.11`, new round 1: Maka 12.34M, OpenCode 1.18M.
3. `mteb-leaderboard`, historical round 0: Maka 10.72M, OpenCode 1.83M.
4. `make-doom-for-mips`, new round 1: Maka 9.32M, OpenCode 0.17M.
5. `mailman`, new round 2: Maka 8.85M, OpenCode 1.39M.

Dividing all adopted tokens by accepted passes gives about 1.31M tokens per Maka pass and 0.90M per OpenCode pass, a coarse 45% Maka penalty. This normalization charges failed attempts to successful ones and is not a price estimate, but it captures the observed quality-versus-resource tradeoff.

Maka's context-budget tool-result pruning was enabled in the immutable-subject rounds, and its cached-input share is slightly higher than OpenCode's. The data therefore do not support a broad claim that pruning was absent or universally ineffective. They show that a small set of difficult cells can still sustain long, high-token trajectories. Assigning that tail to overthinking, loop policy, tool-result growth, or another mechanism requires trace-level ablation; this report does not infer one cause from aggregate usage alone.

## Setup

| Dimension | Value |
| --- | --- |
| Benchmark | Terminal-Bench 2.1 revision `d49e28f1e4ddd13d289e85a5f312a66750951932`, 89 frozen tasks |
| Task-source fingerprint | `sha256:456826aa4c47ed309716c964c96d2a3acc998764ebc84f3e8449c807d74bd4e7` |
| Model | Z.ai Coding Plan `glm-5.2`, reasoning effort `max` |
| Immutable Maka subject | `ee7e4ba5d2323f31be5ab3ec4b4a5fbc847f9a83` |
| Maka adapter | `maka_agent:MakaAgent`, host runtime with task-container tools |
| OpenCode adapter | `opencode_agent:MakaOpenCodeAgent`, pinned OpenCode 1.17.18 |
| OpenCode host-toolchain fingerprint | `sha256:9c2ba35763bb5fb59c16ddcfd036eb172dea2776f494a347f2dd209ff7564649` |
| Composed harness toolchain fingerprint | `sha256:297d922233b1611271120393e25d2c239902dbf502bb147cc6a9a01ba481cd9f` |
| Prompt | Empty external system prompt on both arms |
| Attempt policy | One model attempt per cell; only attempts without a complete verifier result are replacement-eligible |
| Timeout | Task-native agent timeout ×1 plus 900-second outer setup/teardown grace |
| Execution | Four task pairs concurrent, both arms parallel in the immutable-subject rounds |
| Pricing profile | Z.ai Coding Plan account plan, `$0` observed-price placeholder with token totals retained |

The comparison is between two product-agent harnesses sharing a model, task, verifier, reasoning effort, timeout policy, and external prompt. Their internal instructions, tool surfaces, execution loops, and context policies are intentionally not identical.

## Outcome accounting

Every round has exactly one adopted scored result for each of 89 tasks × two arms. The analysis locks the first complete verifier pass/fail for a task-arm. A replacement may fill an infrastructure or persistence gap only when no complete verifier result exists; a later sampled outcome cannot replace an earlier scored result.

| Scope | Recorded attempts | Adopted cells | Replacement attempts | Adopted replacements | Unscored replacements | Discarded scored companions |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Historical round 0 | 206 | 178 | 28 | 8 | 11 | 9 |
| New round 1 | 236 | 178 | 58 | 29 | 12 | 17 |
| New round 2 | 220 | 178 | 42 | 5 | 12 | 25 |
| **All rounds** | **662** | **534** | **128** | **42** | **35** | **51** |

All recorded attempts use 421.74M tokens. The adopted outcomes use 333.74M; 87.99M belongs to unscored or later-discarded attempts and is excluded from the headline arm comparison. The adjacent CSV contains only the adopted cells.

New round 2 exposed a controller-classification edge case: eight Maka primary cells completed their verifier with reward 0, then failed while persisting a runtime event with `RuntimeEvent is not losslessly serializable`. The same-attempt verifier failures are locked as Pass@1=0 rather than redrawn. The reconciliation requires all of the following evidence: Maka arm, complete failed verifier attempts with reward 0, generic post-run error status, and the exact trace-write failure in that trial's run header. Thirteen attempts match the evidence; eight are the first adopted results, while five later attempts are discarded by first-result locking.

Once `build-pov-ray` / OpenCode supplied the last missing result in new round 2, five still-running companion jobs were intentionally terminated as redundant. Their `infra_failed` rows and the replacement run's failed background journal are retained but not adopted.

## Runs and provenance

| Evidence set | Run family |
| --- | --- |
| Historical round 0 | `glm-5.2-maka-vs-opencode-tbench-2.1-canary5-main-ee7e4ba5d-20260727-v1`, `...remaining87-2x4-main-ee7e4ba5d-20260728-v1`, and five replacement runs |
| New round 1 | `glm-5.2-maka-vs-opencode-tbench-2.1-prefx-replication-r2-ee7e4ba5d-20260728-v1` and replacements 1–6 |
| New round 2 | `glm-5.2-maka-vs-opencode-tbench-2.1-prefx-replication-r3-ee7e4ba5d-20260729-v1` and replacements 1–2 |

The two new rounds and every result adopted into them execute the detached subject at `ee7e4ba5d`. During new round 1, that detached worktree path was deleted externally and restored at the same commit; only the resulting unscored infrastructure-loss window was replaced. Replacement 1 also recorded incorrectly composed wrapper fingerprint strings, but direct source identity, frozen task fingerprint, and pinned OpenCode checksums match. No scored result from the affected window was redrawn.

Historical round 0 is supporting evidence, not an immutable-subject replication. Two accepted replacement cells run after `aff71e5cf` changed post-run trajectory hydration/reporting and absolute-path normalization. The model, prompt, tools, timeout policy, task source, and OpenCode toolchain were unchanged, but this subject difference is why the primary conclusion rests on the two clean replications.

No HTTP 429 event was observed in the recorded round-0/new-round-1 network evidence or the new-round-2 preflight probes. There is no remaining unscored adopted cell. Oracle registry annotations were unavailable; official task verifiers remain the Pass@1 authority.

## Caveats

- The suite contains the same 89 tasks in every round. Task-round totals are useful operational observations, not independent-task sample sizes; the task-level sign checks are included to avoid a pseudo-replication claim.
- Three repetitions reduce single-run noise but do not estimate generalization beyond this frozen benchmark.
- Historical round 0 has the subject-provenance caveat above and is supporting rather than primary replication evidence.
- The comparison is a product-agent harness A/B, not an ablation of one internal policy. Cluster and token-tail differences are descriptive, not causal attribution.
- Replacement and reconciliation decisions are auditable but add operational complexity. The first-valid lock prevents score shopping; all later scored companions remain in the attempt ledgers.
- Account-plan `$0` is not a spend estimate. Cached, uncached, output, and reasoning tokens may have different economic weights outside this plan.
- Raw traces, provider payloads, verifier output, and local analysis ledgers remain git-excluded. This report commits only aggregate prose and a redaction-minimal per-task CSV.

## Integrity

SHA-256 of the frozen local evidence and committed CSV:

| Source | SHA-256 |
| --- | --- |
| Historical round summary | `5f2692be9c8156ecb09a85708045a39ce224032ee0128e8076c879673d871674` |
| Historical round adopted cells | `e2e6e623f0ff0f451b1c860615831af930566dc55fdf44ab0d800eb112bda3f8` |
| New round 1 summary | `cf5cf6980cd6ac54669dac9247ce3636a70f2b94acb3bd8e28cc93388be63cc7` |
| New round 1 adopted cells | `bb1d0b8f4b9597ce657160165a5fa0ff2ea32ebef8b219d53168f65cad1da593` |
| New round 2 summary | `8ca269a57e972a6a795b4cc948accd5964567a0911b418e56869bdf4b7854a29` |
| New round 2 adopted cells | `022e9d3b477a7ec121a8b797f8b62c7681adc0cfb5199bd7f725541990872ad5` |
| New round 2 reconciliation ledger | `72331e08a1f1f7520766c45a933722cd3d116879b908f65b80f9e90772360fd0` |
| Committed per-task CSV | `5db04fa7f9cb30d352822a2a524ba8861497f4b42f25fe4ddf52f7d73d428c18` |

## Artifact pointers

Local raw artifacts are git-excluded under `.agents-workspace-data/harness-ab/terminal-bench-2.1/`. The authoritative composed analyses are:

- `round0-analysis/`
- `glm-5.2-maka-vs-opencode-tbench-2.1-prefx-replication-r2-ee7e4ba5d-20260728-v1/analysis/`
- `glm-5.2-maka-vs-opencode-tbench-2.1-prefx-replication-r3-ee7e4ba5d-20260729-v1/analysis/`
- `prefx-replication-r2-r3-analysis/`
- `prefx-round0-r2-r3-analysis/`
