# Terminal-Bench 2.1 — DeepSeek V4 Flash: Maka vs Codex vs Claude Code vs Reasonix

This report compares four agent harnesses around the same DeepSeek V4 Flash model on all 89 Terminal-Bench 2.1 tasks. Every arm ran inside the task container under the same executor, the same budget, and the same package mirror, so the only variable that moves between arms is the harness itself.

**Run id:** `deepseek-v4-flash-4arm-tbench-2.1-full-v1`

**Local artifacts (git-excluded):** `~/.maka/eval/runs/deepseek-v4-flash-4arm-tbench-2.1-full-v1/`

**Metric:** end-to-end pass@1 by the official task verifier

**Status:** `completed_with_gaps` — 353/356 cells model-scored; the three unscored cells are Reasonix infrastructure and plumbing failures that survived two retry rounds

**Per-task outcomes:** [`terminal-bench-2.1-deepseek-v4-flash-four-arm.csv`](./terminal-bench-2.1-deepseek-v4-flash-four-arm.csv)

## TL;DR

- **Codex passed 70/89 (78.65%), Maka 65/89 (73.03%), Claude Code 57/89 (64.04%), Reasonix 55/89 (61.80%).**
- **The Codex–Maka gap is not statistically significant.** Of 19 discordant pairs, Codex won 12 and Maka 7; an exact two-sided McNemar test gives **p = 0.359**. Only two of the six pairwise comparisons reach significance: Codex over Claude Code (p = 0.0072) and Codex over Reasonix (p = 0.0026).
- **An independent earlier run agrees, including on the non-significance.** Over the same 89 tasks it scored Codex 67, Maka 60, Claude Code 57, with Codex–Maka at p = 0.210. Codex is the one arm configured identically in both runs, and **19% of its tasks changed outcome between them while its net score moved by 3** — that is the noise floor any five-task claim has to clear.
- **Maka has the lowest cost per pass at $0.03215**, ahead of Codex ($0.03339), Reasonix ($0.04051), and Claude Code ($0.04800).
- On the 60-task subset where no arm exhausted its budget, the ordering inverts at the top: **Claude Code 90.00%, Codex 88.33%, Maka 86.67%, Reasonix 75.00%.** Claude Code has the highest conditional pass rate and the worst overall score, because it exhausted its budget on 24/89 tasks (26.97%) — more than double any other arm.
- Three of the four arms are close in solution quality once they finish. The headline spread is driven mostly by whether an arm finishes inside the budget, not by whether it can solve the task.

## Experiment

| Parameter | Value |
| --- | --- |
| Benchmark | Terminal-Bench 2.1, all 89 tasks |
| Model | DeepSeek V4 Flash, one provider account, one proxy per cell |
| Arms | Maka, Codex, Claude Code, Reasonix |
| Placement | All four arms execute inside the task container |
| Concurrency | 6 task groups, 24 concurrent cells |
| Budget | 900 s per cell |
| Package mirror | Ubuntu apt hosts redirected to a regional mirror for every arm |
| Retries | Two rounds, restricted to cells adjudicated as infrastructure failures |

Redirecting apt is a variance-control measure, not a performance aid. The same command took 16 s or 300 s from the same host depending on which upstream path it landed on, and that noise is charged against whichever agent happened to need more packages. The mirror address enters the manifest fingerprint, so runs with and without it are distinct experiments.

## Results

End-to-end pass@1 is the primary result. Budget-exhausted cells remain scored failures in its denominator.

| Arm | Pass@1 | Passed / evaluated | Budget exhausted | Unscored |
| --- | ---: | ---: | ---: | ---: |
| Codex | **78.65%** | 70 / 89 | 9 | 0 |
| Maka | **73.03%** | 65 / 89 | 11 | 0 |
| Claude Code | **64.04%** | 57 / 89 | 24 | 0 |
| Reasonix | **61.80%** | 55 / 89 | 13 | 3 |

Reasonix is the only arm with unscored cells: two infrastructure failures and one plumbing failure survived both retry rounds. Its pass@1 is reported over all 89 tasks; over the 86 it actually completed the rate is 63.95%. The other three arms recorded zero infrastructure failures.

## Pairwise significance

Each comparison uses the exact two-sided McNemar test over discordant task pairs, treating the 89 benchmark tasks as the paired units.

| Comparison | A-only passes | B-only passes | Discordant | p |
| --- | ---: | ---: | ---: | ---: |
| Codex vs Reasonix | 19 | 4 | 23 | **0.0026** |
| Codex vs Claude Code | 17 | 4 | 21 | **0.0072** |
| Maka vs Reasonix | 17 | 7 | 24 | 0.0639 |
| Maka vs Claude Code | 13 | 5 | 18 | 0.0963 |
| Maka vs Codex | 7 | 12 | 19 | 0.3593 |
| Claude Code vs Reasonix | 15 | 13 | 28 | 0.8506 |

Only the two Codex comparisons clear the 0.05 threshold. **The five-task Codex lead over Maka is within what this suite's noise can produce**: the paired outcomes are 58 shared passes, 7 Maka-only, 12 Codex-only, and 12 shared failures. Maka's advantages over Claude Code and Reasonix point in a consistent direction but do not reach significance either.

This is evidence about one frozen run over the fixed 89-task Terminal-Bench 2.1 suite. It is not proof of a universal ordering on other task distributions, other models, or repeated runs.

## Diagnostic decomposition

These metrics diagnose where the observed gaps appear. Neither replaces end-to-end pass@1.

| Diagnostic | Maka | Codex | Claude Code | Reasonix |
| --- | ---: | ---: | ---: | ---: |
| Non-budget conditional pass rate | 52/60 (86.67%) | 53/60 (88.33%) | **54/60 (90.00%)** | 45/60 (75.00%) |
| Budget exhaustion rate | 11/89 (12.36%) | 9/89 (10.11%) | **24/89 (26.97%)** | 13/89 (14.61%) |

The conditional denominator excludes any task where *any* arm exhausted its budget, leaving the same 60 tasks for all four. It is not an "unlimited-time pass rate": the remaining tasks still ran under their original 900 s budgets, and the excluded set is not random — it is enriched for hard tasks.

The two rows tell different stories. On the conditional subset, Maka, Codex, and Claude Code sit within 3.33 percentage points of each other, and the ordering of the top three reverses relative to the headline. What separates them in the headline is the second row: Claude Code exhausts its budget on more than a quarter of the suite, over twice the rate of Codex.

These observations decompose the gap; this run does not establish their causes.

## Outcome-normalized economics

Cost per pass normalizes recorded usage by successful benchmark outcomes. It includes spending on scored failures.

| Arm | Total cost | Passed | Cost per pass |
| --- | ---: | ---: | ---: |
| Maka | $2.0895 | 65 | **$0.03215** |
| Codex | $2.3376 | 70 | $0.03339 |
| Reasonix | $2.2281 | 55 | $0.04051 |
| Claude Code | $2.7363 | 57 | $0.04800 |

Maka is the cheapest per pass despite ranking second on pass@1. Its margin over Codex is 3.7%; the gap to Claude Code is 49%. No equivalence test was performed, so these are descriptive point estimates from a single run.

Usage was metered by a per-cell host proxy that parses the provider's SSE stream. Clients that close the connection after receiving the terminal event are common — 40% of Reasonix requests end this way — and their usage is retained. Across the run, 2 of 412 Reasonix requests are the only ones without recorded usage.

## Per-request shape

The four harnesses differ sharply in how they spend a task budget.

All four columns come from the host proxy's per-request telemetry, so they are mutually comparable.

| Arm | Requests per cell | Output tokens per request | Reasoning tokens per request | Seconds per request |
| --- | ---: | ---: | ---: | ---: |
| Codex | 44.3 | 781 | 543 | 9.1 |
| Reasonix | 41.6 | 1,000 | 745 | 11.5 |
| Maka | 37.4 | 1,064 | 778 | 10.7 |
| Claude Code | 32.8 | 1,708 | not itemized | 18.2 |

Codex takes the most steps and makes each one the smallest; Claude Code takes the fewest and makes each one the largest and slowest. Maka sits between them, spending 1.43× more reasoning per request than Codex. Anthropic's protocol does not itemize reasoning tokens, so Claude Code's reasoning column is empty rather than zero.

Under a hard deadline this shape has consequences. On `write-compressor`, Maka spent 890 s of its 900 s budget inside the model with only 6 s in tools and exhausted the budget after 20 requests; Codex finished the same task in 540 s across 66 requests. Denser steps leave less room to recover from a wrong turn.

The same shape points in the direction of the economic result: Maka's median cell duration is 333 s against Codex's 412 s, and its cost per pass is 3.7% lower. This run does not establish that the step shape causes the cost difference.

## Cross-run stability

An earlier three-arm run over the same 89 tasks and the same model (`deepseek-v4-flash-3arm-tbench-2.1-full-v7`) provides one independent repetition for Codex, Maka, and Claude Code. It predates this branch: Maka ran host-side, no package mirror was applied, and its Claude Code arm lost 17 cells to infrastructure failures. Only the Codex arm is configured identically in both runs.

| Arm | Three-arm run | Four-arm run | Tasks flipping outcome |
| --- | ---: | ---: | ---: |
| Codex | 67/89 (75.28%) | 70/89 (78.65%) | 17 (19.10%) |
| Maka | 60/89 (67.42%) | 65/89 (73.03%) | 19 (21.35%) |
| Claude Code | 57/89 (64.04%) | 57/89 (64.04%) | 18 (20.22%) |

The Codex row is the interpretable one, and it is the most useful number in this report. Its configuration did not change and it recorded no infrastructure failures in either run, yet **19% of tasks changed outcome between the two runs while the net score moved by 3**. Roughly a fifth of this suite is decided by run-to-run variation rather than by any property of the harness under test.

That is the scale against which the five-task Codex–Maka gap has to be read, and it agrees with the significance tests: the same comparison in the three-arm run gives 15 Codex-only against 8 Maka-only, **p = 0.210** — again not significant, again in Codex's favour. Two independent full runs put Codex nominally ahead of Maka and neither can distinguish them.

The Maka and Claude Code rows cannot be read the same way. Maka's placement and package sourcing both changed between the runs, so its +5 confounds those changes with variance. Claude Code's identical headline conceals a different denominator: 17 of its three-arm cells never scored.

## Limitations

- Two runs of this suite exist, but only the Codex arm is configured identically across both; the comparison in this report rests on one run of the four-arm configuration. Given the 19% cross-run flip rate measured above, differences of this size should not be treated as settled by either run alone.
- The 900 s budget is a material parameter, not a neutral one. It binds hardest on Claude Code (26.97% exhaustion) and would likely change the ordering if relaxed.
- Reasonix contributes three unscored cells, so its comparisons rest on 86 shared tasks rather than 89.
- Cost per pass uses provider list pricing applied to metered usage, not billed invoices.
- Pairwise McNemar tests are reported without multiple-comparison correction. Applying a Bonferroni correction across the six comparisons moves the threshold to 0.0083; both Codex results survive it (0.0026 and 0.0072) and no other comparison approaches it.
- Per-request telemetry and the harness token summary disagree on Codex's total reasoning volume (2.14M versus 1.57M tokens); the two agree for Maka. The cause was not identified, so this report makes no cross-source total-volume claim and compares per-request figures only within the telemetry source.
