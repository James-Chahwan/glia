# Build timings, 0.5.6 -> 0.5.7 (HP.3)

`scripts/build-timing.sh` (HP.3) times cold builds of four big public repos and checks them against
`bench/build-timing/baseline.json` under `bench/build-timing/RULE.md`, which was committed before the first
baseline. Three cold builds of each repo, interleaved by round. Each result is the median of the three runs:
wall, user + system CPU, peak RSS, and the 13 phases of the committed `[timing]` lines. This note records
0.5.6 against 0.5.7 at W1, the code the baseline was measured on. The release close-out's `timing` gate
compares the 0.5.7 release candidate with this baseline (followup for HZ.1, below).

## How it was measured

- **Machine**: `16 cores, AMD Ryzen 7 3700X 8-Core Processor` (the baseline's fingerprint).
- **Load**: before each build, the 1-minute load was 0.32-6.20. The builds themselves raise it: twenty keeps
  11-13 cores busy for its first 6 s. Every run was under the guard's 8.0, so no wall in this note was
  inflated by load the way GP.1's runs 1-4 were.
- **Repos** (pins from `bench/cohort/repos.json`): twenty `8fd53a835fd5`, photoprism `70e20b5e033f`,
  prefect `fa93ba256927` and tolgee `43e6654422b1`. Each was `git archive`d once from the cohort's bare
  clones.
- **Command**: `glia build <copy> --no-incremental --out <scratch>`, with `GLIA_THREADS` unset and TMPDIR
  on /home.
- **Binaries**:
  - **W1 (baseline)**: the W1 close-out's cohort-gate binary, `target/release/glia` with default features,
    code `cd3e04a3`, build `0.5.6+pe220807d11422cb2`. `baseline` was run at 18:33 and `check` at 18:34 on
    the same binary.
  - **v0.5.6 PyPI**: the published `glia-cli` 0.5.6 wheel's binary (build `0.5.6+pcb4dec7684bf4a4b`).
    It was built by CI in manylinux_2_28 with `--no-default-features --features embed`.
  - **v0.5.6 local** and **W1 local**: `git archive v0.5.6` and `git archive cd3e04a3`, each built here
    with `cargo build --release --locked -p glia-cli --no-default-features` in one target dir. The two
    builds share every dependency, so only glia's own code differs between them.

The probe from the spec still holds at HEAD. v0.5.6's prefect build was quoted at wall 3.78 s, CPU 22.0 s
and RSS 585 MB. One W1 run here measured wall 3.68 s, CPU 22.3 s and RSS 577 MB.

## The acceptance pair (baseline, then check at the same HEAD, same binary)

    [build-timing] baseline: ok - wrote bench/build-timing/baseline.json (4 repos, 3 runs, 16 cores, AMD Ryzen 7 3700X 8-Core Processor)
    [build-timing] check: ok - 4 repos, 52 phases, 0 gains, 0 regressions

| repo | baseline wall per run (s) | check wall per run (s) | baseline median -> check median |
|---|---|---|---|
| twenty | 15.91, 15.75, 15.78 | 16.03, 15.83, 16.31 | 15.78 -> 16.03 (+1.6%) |
| photoprism | 2.98, 2.93, 2.95 | 2.93, 2.92, 2.95 | 2.95 -> 2.93 |
| prefect | 3.29, 3.65, 3.29 | 3.38, 3.29, 3.65 | 3.29 -> 3.38 (+2.8%) |
| tolgee | 2.36, 2.36, 2.20 | 2.22, 2.20, 2.22 | 2.36 -> 2.22 |

Between the two runs, CPU moved by at most 1.4% and RSS by at most 1.9%. No phase moved by more than 280 ms
(twenty's parse, 6264 -> 6542 ms; the phase floor is +300 ms). Every value stayed far inside the rule.

## Per repo: medians

Ratios in the first table:
- **avg cores** = CPU / wall.
- **core use** = CPU / 16 / wall. This is the serial-share reading: 1.0 would mean every core was busy for
  the whole build.

Phases are in ms.

### twenty (TypeScript)

| binary | wall s | CPU s | avg cores | core use | peak RSS MB | slowest pass |
|---|---:|---:|---:|---:|---:|---|
| GP.1 0.5.6 W0 tree (load 10-13) | 15.32-16.71 | - | - | - | - | - |
| v0.5.6 PyPI | 15.71 | 81.5 | 5.2 | 0.32 | 1815 | tag_synthetic_provenance 456 ms |
| v0.5.6 local | 16.05 | 83.2 | 5.2 | 0.32 | 1771 | tag_synthetic_provenance 477 ms |
| W1 local | 16.02 | 83.6 | 5.2 | 0.33 | 1838 | tag_synthetic_provenance 465 ms |
| W1 (baseline) | 15.78 | 83.4 | 5.3 | 0.33 | 1793 | tag_synthetic_provenance 474 ms |

| binary | walk | parse | const_scan | grafts | language_build | resolve | post | finalize | total | persist | spans | encode | write |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| GP.1 (runs 5 / 6) | 793 / 792 | 6125 / 5605 | 151 / 90 | 1808 / 1539 | 2809 / 2483 | 727 / 670 | 896 / 813 | 135 / 123 | - | 2625 / 2609 | - | - | - |
| v0.5.6 PyPI | 852 | 6360 | 74 | 1074 | 3669 | 827 | 812 | 147 | 13899 | 1283 | 590 | 382 | 151 |
| W1 (baseline) | 867 | 6264 | 79 | 1105 | 3728 | 836 | 866 | 158 | 13970 | 1292 | 584 | 392 | 150 |

### photoprism (Go + Vue)

| binary | wall s | CPU s | avg cores | core use | peak RSS MB | slowest pass |
|---|---:|---:|---:|---:|---:|---|
| GP.1 0.5.6 W0 tree (load 10-13) | 5.94-6.24 | - | - | - | - | - |
| v0.5.6 PyPI | 2.90 | 19.8 | 6.8 | 0.43 | 467 | tag_synthetic_provenance 50 ms |
| v0.5.6 local | 2.92 | 20.1 | 6.9 | 0.43 | 466 | tag_synthetic_provenance 51 ms |
| W1 local | 2.92 | 20.0 | 6.9 | 0.43 | 460 | tag_synthetic_provenance 51 ms |
| W1 (baseline) | 2.95 | 20.0 | 6.8 | 0.42 | 468 | tag_synthetic_provenance 53 ms |

| binary | walk | parse | const_scan | grafts | language_build | resolve | post | finalize | total | persist | spans | encode | write |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| GP.1 (runs 5 / 6) | 76 / 80 | 1360 / 1199 | 14 / 13 | 252 / 241 | 3587 / 3500 | 33 / 30 | 204 / 183 | 34 / 33 | - | 516 / 496 | - | - | - |
| v0.5.6 PyPI | 86 | 1018 | 9 | 102 | 937 | 65 | 159 | 35 | 2441 | 298 | 104 | 90 | 66 |
| W1 (baseline) | 88 | 1031 | 10 | 106 | 950 | 66 | 167 | 34 | 2490 | 308 | 106 | 89 | 69 |

### prefect (Python + TypeScript)

| binary | wall s | CPU s | avg cores | core use | peak RSS MB | slowest pass |
|---|---:|---:|---:|---:|---:|---|
| spec probe, v0.5.6 (load 1.3) | 3.78 | 22.0 | 5.8 | 0.36 | 585 | org_packages 109 ms |
| v0.5.6 PyPI | 3.26 | 22.1 | 6.8 | 0.42 | 585 | org_packages 111 ms |
| v0.5.6 local | 3.38 | 22.3 | 6.6 | 0.41 | 576 | org_packages 110 ms |
| W1 local | 3.36 | 22.5 | 6.7 | 0.42 | 577 | org_packages 110 ms |
| W1 (baseline) | 3.29 | 22.3 | 6.8 | 0.42 | 579 | org_packages 110 ms |

| binary | walk | parse | const_scan | grafts | language_build | resolve | post | finalize | total | persist | spans | encode | write |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| v0.5.6 PyPI | 94 | 1281 | 9 | 204 | 610 | 279 | 222 | 32 | 2733 | 384 | 126 | 128 | 72 |
| W1 (baseline) | 91 | 1261 | 10 | 215 | 622 | 281 | 240 | 34 | 2764 | 379 | 127 | 124 | 71 |

### tolgee (Kotlin / Java + TypeScript)

| binary | wall s | CPU s | avg cores | core use | peak RSS MB | slowest pass |
|---|---:|---:|---:|---:|---:|---|
| GP.1 0.5.6 W0 tree (load 10-13) | 4.27-4.34 | - | - | - | - | - |
| v0.5.6 PyPI | 2.03 | 11.9 | 5.9 | 0.37 | 368 | tag_synthetic_provenance 81 ms |
| v0.5.6 local | 2.21 | 14.5 | 6.5 | 0.41 | 363 | tag_synthetic_provenance 84 ms |
| W1 local | 2.20 | 14.6 | 6.6 | 0.42 | 364 | tag_synthetic_provenance 81 ms |
| W1 (baseline) | 2.36 | 14.4 | 6.1 | 0.38 | 370 | tag_synthetic_provenance 82 ms |

| binary | walk | parse | const_scan | grafts | language_build | resolve | post | finalize | total | persist | spans | encode | write |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| GP.1 (runs 5 / 6) | 90 / 97 | 1042 / 1066 | 10 / 10 | 196 / 197 | 2082 / 1996 | 63 / 59 | 187 / 181 | 23 / 22 | - | 518 / 521 | - | - | - |
| v0.5.6 PyPI | 106 | 708 | 8 | 186 | 323 | 108 | 163 | 22 | 1632 | 286 | 98 | 82 | 70 |
| v0.5.6 local | - | 876 | - | - | 331 | - | - | - | 1819 | 288 | - | - | - |
| W1 (baseline) | 107 | 1011 | 9 | 189 | 326 | 110 | 172 | 23 | 1960 | 288 | 99 | 81 | 71 |

## What the numbers say

- **0.5.6 -> 0.5.7 W1: no change.** `compare` of v0.5.6 local against W1 local (one target dir, the same
  dependencies, glia's own code the only difference) reads `check: ok - 4 repos, 52 phases, 0 gains,
  0 regressions`. W0 and W1 (HT.1's id blocks, HF.5's Rails nesting, HF.3's MCP host and the rest) moved
  no measured build.
- **GP.2-GP.4 held.** These are GP.1's 0.5.6 W0 tree (before GP.2-GP.4) against today's binaries. GP.1 ran
  at load 10-13, so its walls are an upper bound.

  | repo | wall | language_build | persist |
  |---|---|---|---|
  | photoprism | 5.9-6.2 s -> 2.9 s | 3.5 s -> 0.95 s (GP.3's pooled Go front) | 0.5 s -> 0.3 s (GP.4) |
  | tolgee | 4.3 s -> 2.2-2.4 s | 2.0 s -> 0.33 s (GP.3's pooled JVM front) | - |
  | twenty | not faster | rose, 2.5-2.8 s -> 3.7 s | halved, 2.6 s -> 1.3 s (GP.4) |

  twenty's wall did not drop because its parse (6.3 s) and the serial tail dominate. Its core use stays at
  0.32 (5.2 cores of 16): twenty is the most serial build of the four.
- **The PyPI binary is not the local build.** On tolgee the published 0.5.6 wheel parses in 708 ms and
  11.9 s CPU. The same tag built here parses in 876 ms and 14.5 s CPU (+22% CPU, 0.5.6 code both times).
  The other three repos agree within 2.5% CPU. So the gap comes from how the binary is built (CI's
  manylinux toolchain and its C compiler for the tree-sitter grammars, against this box's), not from glia's
  code. A baseline is therefore per machine **and** per build recipe. The close-out always measures
  `cargo build --release -p glia-cli` on this box, the recipe the baseline was taken with. Comparing it
  against a PyPI binary would fail the rule's CPU line on tolgee (11.9 -> 14.4 s is x1.21) with no code
  change behind it.
- **Slowest pass**: `tag_synthetic_provenance` (twenty 474 ms, tolgee 82 ms, photoprism 53 ms) and
  `org_packages` (prefect 110 ms). Neither is near the phase floor.


## The 0.5.7 release candidate (the release close-out's `timing` gate)

The release close-out (W13, glia f2f5a7551fd8, the tree the tag is cut from but for docs and the
cohort re-baseline) ran the gate against the W1 baseline: `[build-timing] check: ok - 4 repos, 52 phases, 0 gains,
0 regressions`. Medians, W1 baseline -> RC (same machine, same recipe):

| repo | wall ms | cpu ms | rss MB | glia total ms |
|---|---|---|---|---|
| twenty | 15784.9 -> 16246.4 | 83379.7 -> 85153.9 | 1793.0 -> 1871.1 | 13969.5 -> 14418.0 |
| photoprism | 2946.3 -> 2954.8 | 19954.5 -> 20353.8 | 468.3 -> 475.6 | 2489.7 -> 2492.5 |
| prefect | 3293.4 -> 3321.2 | 22322.1 -> 23166.0 | 579.1 -> 581.8 | 2764.4 -> 2785.0 |
| tolgee | 2358.9 -> 2316.5 | 14430.1 -> 14678.6 | 370.1 -> 379.8 | 1960.0 -> 1926.7 |

Every 0.5.7 packet that changed graph content landed between the two rows (HF, HT, HC, HQ); none moved a build
phase past the gate's thresholds.

## Followups

- Done at the release close-out: the 0.5.7 RC row is the section above.
