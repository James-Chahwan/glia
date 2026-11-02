# The build-timing rule (HP.3, 0.5.7)

Committed before the first baseline. `scripts/build-timing.sh check` applies it, `selftest` pins it, and
nothing else may judge a timing result. A change to this file is a declared change of the rule, never a
way to make a check pass.

## What is measured

Per repo of `bench/build-timing/repos.json` (pins read from `bench/cohort/repos.json`), `runs` cold builds
(`rm -rf <copy>/.glia`, then `glia build <copy> --no-incremental --out <scratch>`, `GLIA_THREADS` unset,
TMPDIR off /tmp), interleaved by round (every repo once, then every repo again). Per build:

- `wall_ms`: the process's wall time;
- `cpu_ms`: user + system CPU of the process (`os.wait4`'s rusage);
- `rss_mb`: its peak resident set (`ru_maxrss`);
- the phases, every field of the committed `[timing]` lines, in ms: `walk`, `parse`, `const_scan`,
  `grafts`, `language_build` (each summed over the build's `[timing] repo=` lines), `resolve`, `post`,
  `finalize`, `total` (the `[timing] build` line), `persist` (the `[timing] persist` line's ms),
  `spans`, `encode`, `write` (its `spans_ms` / `encode_ms` / `write_ms`): 13 phases.

A repo's result is the **median** of each value over its runs, each value on its own.

## Load guard

Before the first build and before every build, the 1-minute load average must be under
**0.5 x the core count**. The script waits for it (polling) for at most **120 s**; past that the check
prints `check: skipped - load <l> on <n> cores` and exits 3. A skip measured nothing: it is never a pass.

## Machine

Every result carries the machine fingerprint (`<cores> cores, <cpuinfo model name>`). A check against a
baseline of another fingerprint prints `check: skipped - machine differs (<base> vs <new>)` and exits 3.

## The check, per repo, median against baseline

A **regression** (any one fails the check, exit 1):

| value | regression when |
|---|---|
| `cpu_ms` | new > base x **1.20** |
| `wall_ms` | new > base x **1.30** and new > base + **1000** ms |
| each phase | new > base x **1.50** and new > base + **300** ms |
| `rss_mb` | new > base x **1.25** |
| the repo | in the baseline, absent from the run (a run measures every baseline repo, or names `--only`) |

A **gain** (listed, never a pass condition, never ratcheted automatically): a phase, `cpu_ms` or `rss_mb`
below base x **0.80**.

`baseline` rewrites `bench/build-timing/baseline.json` only when run by hand or by a perf packet, a
declared re-baseline; no close-out writes it.

## Lines and exit codes

    [build-timing] run label=<l> run=<i> wall_ms=<w> cpu_ms=<c> rss_mb=<m> load=<l>      one per build
    [build-timing] check: ok - <r> repos, <p> phases, <g> gains, 0 regressions          exit 0
    [build-timing] check: FAILED (<n> regressions: <label>/<value> <base>-><new>, ..)    exit 1
    [build-timing] check: FAILED (<reason>)                                             exit 2
    [build-timing] check: skipped - <reason>                                            exit 3

`<p>` counts the phase comparisons made (13 per repo). A `--only` check ends ` (partial: only <l,..>)`.

## In the close-out

`dev-notes/wave-runner/closeout.py` runs `check` after the cohort gate (the `timing` gate of its record).
Per wave a FAILED or skipped check is **advisory**: printed with ` (advisory)`, recorded, never blocking.
At the release's last close-out it **gates**: a FAILED check, and any skip (load, machine, an explicit
`GLIA_BUILD_TIMING_SKIP`), fails the close-out; the box must be quiet.
