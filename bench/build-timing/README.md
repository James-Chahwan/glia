# Build timings (HP.3, 0.5.7)

Cold-build timings of four big public repos, kept as a committed baseline and checked by every close-out, so a
later packet cannot bring back a serial build front unseen. 0.5.6's GP.2-GP.4 sped up the TypeScript, Go and JVM
fronts and persist; GP.1 measured them once with throwaway timers (`dev-notes/build-profile-0.5.6.md`), and
nothing held those gains. This holds them.

- `RULE.md` - the check, committed before the first baseline: what is measured, the load guard, the machine
  fingerprint, the thresholds. Only `scripts/build-timing.sh` applies it.
- `repos.json` - the repos and the runs per repo: twenty (TypeScript), photoprism (Go + Vue), prefect
  (Python + TypeScript) and tolgee (Kotlin / Java + TypeScript, the only one that exercises GP.3's pooled JVM
  front). Only labels: the pins are read from `bench/cohort/repos.json` (one source of truth), which must mark
  each row `public: true` with a GitHub https url and a 40-hex sha.
- `baseline.json` - the medians the check compares with, and the machine they were measured on: per repo
  `median` (`wall_ms`, `cpu_ms`, `rss_mb`, `slowest_pass` and the 13 `[timing]` phases) and the per-run
  `wall_ms_runs` / `cpu_ms_runs` / `rss_mb_runs`. Written only by `build-timing.sh baseline`, by hand or by a
  perf packet (a declared re-baseline); a close-out never writes it.

## Running it

    bash scripts/build-timing.sh check            # build glia (release), measure, compare with baseline.json
    bash scripts/build-timing.sh check --glia <bin>
    bash scripts/build-timing.sh run              # measure only, into ${GLIA_BUILD_TIMING_DIR:-~/.cache/glia-build-timing}/run/
    bash scripts/build-timing.sh compare <base.json> <new.json>   # two summaries, read-only
    bash scripts/build-timing.sh baseline         # a declared re-baseline
    bash scripts/build-timing.sh selftest         # the rule and the load guard on seeded results (no glia)

Each pin is `git archive`d once from the cohort's bare clone (`${GLIA_COHORT_DIR:-~/.cache/glia-cohort}`) into
`~/.cache/glia-build-timing/<label>@<sha12>` (twenty's copy is ~40k files: keep it off /tmp). Each build deletes
the copy's `.glia/`, waits for a quiet box, and runs `glia build <copy> --no-incremental --out <scratch>` with
`GLIA_THREADS` unset and TMPDIR on /home, under `os.wait4` for user + system CPU and peak RSS. A check costs
about 75 s of builds on the 16-core baseline machine (twenty ~16 s, prefect ~3.3 s, photoprism ~2.9 s, tolgee
~2.3 s, three rounds) plus the archive copies once.

The 1-minute load must be under half the cores before the run and before each build (at most 120 s of waiting),
else `check: skipped - load ..` (exit 3). The baseline is per machine: on another fingerprint the check prints
`check: skipped - machine differs ..` (exit 3).

## In the close-out

`dev-notes/wave-runner/closeout.py` runs `check` after the cohort gate, on the cohort gate's binary, as its
recorded `timing` gate (HP.1's reuse: the CLI's code, this dir, the script, the four pins, rustc and the machine
fingerprint key it), and adds its seconds to the `[closeout] timing` line as `build_timing=`. Per wave a FAILED
or skipped check is printed with ` (advisory)` and blocks nothing; at the release's last close-out it gates, and
any skip fails it. The 0.5.6 -> 0.5.7 numbers are in `dev-notes/build-timing-0.5.7.md`.
