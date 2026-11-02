# wave-runner

Resumable tooling for the review-2026-09-15 §1–§3 programme
(`dev-notes/wave-plan-2026-09-16.md`). It lived in `/tmp` for waves 1–5 and was
lost at a session boundary, so it is committed now.

| file | role |
|---|---|
| `schedule.py` | deterministic file-disjoint scheduler. `--verify` asserts it reproduces the waves that already landed — run it first |
| `gen_wave.py N out.js` | renders wave N's Workflow script; refuses a non-disjoint wave |
| `shared_brief.md` | the brief every implementing agent gets — the accumulated rules from waves 0–5 |
| `baseline.json` | the MEASURED state after the last wave; `gen_wave.py` renders it into the brief |

## Running a wave

1. `python3 schedule.py --verify` — must print `VERIFIED`.
2. Pull the previous wave's `followups` that name a packet in this wave; add them to
   `dev-notes/packet-corrections.json` and commit.
3. `python3 gen_wave.py N <scratch>/waveN.js`, then run it as a Workflow.

## End of wave (in this order — every step has bitten us)

1. Check commits and a clean tree. Read each commit's file list against its subject.
2. `cargo test --workspace` and `cargo test -p glia-engine --test byte_identical`.
3. Rebuild the wheel: `cargo clean -p glia-engine -p glia-py`,
   `maturin build -m py/Cargo.toml --release`, `pip install --force-reinstall --no-deps
   target/wheels/glia_py-*.whl` (the newest), then confirm `find <crates> -name '*.rs'
   -newer <installed glia_py .so>` prints nothing, `py/check_api_surface.py` prints
   `wheel OK` and `bench/message-contracts/check.sh` passes.
   `grade.py` reads the INSTALLED wheel — before this, the matrix is not a verdict.
4. `python3 bench/substrate-gap/run.py --no-log`, then `matrix.py --emit` and `--check`,
   `test_matrix.py`, `test_grade.py`. Commit the regenerated artefacts.
5. Update `baseline.json` and add the wave's packets to `LANDED` in `schedule.py`.

## The 0.5.0 leap (`--leap`)

The same tools run the leap (`dev-notes/next-leap-0.5.0.md` §7) with `--leap`:

| file | role |
|---|---|
| `leap_schedule.py` | reads `dev-notes/leap-packets.json`; Batch C claims come from the re-verified `files_touched_now`; applies the dependency patch, the wave-0 split remaps and the `exclusive` list; W0 is the serial split wave. `--verify`, `--stats`, `--wave N`, `--json` |
| `shared_brief_leap.md` | the brief for the breaking release (declared breaks allowed, other repos read-only, L0.1 ids) |
| `gen_wave.py --leap N out.js` | renders leap wave N; Batch C packets get their standing correction + re-verification + leap correction; W0 and single-packet waves render sequential |
| `closeout.py --leap N <run-id>` | end of leap wave; folds followups into `dev-notes/leap-corrections.json`; runs `scripts/check-engram-export.sh` once LG.13 created it; after the wheel rebuild, `py/check_api_surface.py` and `bench/message-contracts/check.sh` against the installed `glia_py` (LD.11b); `--plan-only` prints the wave |
| `usage_gate.py` | before launching a leap wave: `python3 usage_gate.py --start W<N>` prints GO / HOLD / UNKNOWN from the account's 5-hour usage (exit 0 / 2 / 3) and records the launch reading; `closeout.py --leap` records the end reading, so the gate learns what a wave costs. HOLD at >= 80% or when usage + an average wave passes 100%, unless the window resets within an hour. Data comes from `~/.claude/rate-limits.json`, written by the status line script on every render; it also HOLDs with under 60 GB free on /home (`--min-free-gb`) |
| `watchdog.py` | anything we started that is stuck (over 5 min old, under 1% CPU), in disk wait (D), or a /home under 60 GB free; one report, exit 1 when something is flagged. Run it when a background job passes ~2x its expected wall time and before a long run. Every close-out also drops `target/debug/incremental` and prints target/ size and free space |

### Gate reuse (HP.1, 0.5.7)

`closeout.py` keys every recorded gate (tests + byte_identical, fmt, clippy, rustdoc, rank, cohort, wheel,
bench) by a digest of exactly what the gate reads at HEAD: sha256 over the `<path> <blob>` lines of its input
set from one `git ls-tree -r -z --full-tree HEAD`, then its env lines. The sets are derived, never listed by
hand: CODE(root) is the dirs of the workspace members in `root`'s dependency closure (`cargo metadata --locked
--offline --all-features`: normal + build deps, the root's own dev-deps) + Cargo.toml + Cargo.lock + every
tracked path an `include!` / `include_str!` / `include_bytes!` in those dirs names. tests = the whole tree but
`dev-notes/` and every `bench/` dir except substrate-gap (minus its outputs), doc-link and edge-audit; fmt =
every tracked `*.rs` + rustfmt.toml; clippy / rustdoc = CODE of every member; rank = CODE(glia-engine) +
rank-gate.sh + snapshots.json + gate.json without `accepted` (its judge: `baseline/**` + `accepted`); cohort =
CODE(glia-cli) + cohort-gate.sh + bench/cohort (minus results/) + bench/sibyl + the python modules they import
(its judge: `results/baseline/**`); wheel = CODE(glia-py) + CODE(glia-cli) + bench/message-contracts; bench =
bench/substrate-gap (minus its outputs) + the installed glia_py `.so`. Every tracked path under a gate's
GP.12 `GATE_PATHS` is in its digest. Env lines: closeout.py's own sha256 (every gate), rustc / cargo /
rustfmt / clippy / maturin / the leap python versions, the untracked `.cargo/config.toml`, the fastembed cache
listing (rank). engram and `rank-gate.sh coverage` always run.

- **Record**: `~/.cache/glia-closeout/gate-record.json` (`GLIA_CLOSEOUT_RECORD` overrides), logs in
  `logs/<gate>-<digest12>.log` beside it; written after each gate, so a later gate's failure keeps an earlier
  gate's green. A gate reuses the newest green entry at HEAD's digest whose tier covers the need (a full entry
  serves a fast-tier need, never the reverse): `[closeout] reuse <gate> from <commit12> (wave <w>, <tier>,
  digest <12>)`, then its recorded lines with ` (reused <commit12>)`. Else `[closeout] run <gate> (<reason>)`:
  `forced`, `no record`, `dirty tree`, `inputs moved: <n> paths (..)`, `env moved: <what>`, `need full,
  record partial`, `last result FAILED`. A wheel reuse also needs the installed `.so` and `build_stamp()` to be
  the recorded ones; a bench reuse needs HEAD's bench outputs to be the ones its run wrote.
- **Forcing**: `GLIA_CLOSEOUT_FRESH=all` (or `rank,cohort`, ..) or `--fresh <gates>`.
- **Mid-release**: the touched-path skips are gone: a gate whose inputs moved runs, whoever moved them (an
  out-of-packet fix between waves included); with no record at all the old skip applies once, `(no record yet)`.
- **The release set**: the last close-out needs every recorded gate fresh-full or reused-full at HEAD's
  digest, `[closeout] set tree=<12> fresh=<a> reused=<b> full=<k>/<n>` and one `<gate>=fresh|reused <c>` line each.
- **Ratchets** run only after a FRESH, FULL, green check in the same close-out: a reused gate prints
  `ratchet: skipped - reused from <c>; GLIA_CLOSEOUT_FRESH=<gate> to ratchet`.
- **A failed close-out** writes no baseline.json, LANDED or next wave, and puts the bench outputs back.
- **TMPDIR**: every gate runs with `$GLIA_CLOSEOUT_TMP` or `~/.cache/glia-tmp` unless the caller's TMPDIR is
  outside /tmp (`[closeout] tmpdir=<dir> free_gb=<f>`); a log showing `Disk quota exceeded` / `No space left on
  device` (os error 122 / 28) marks the result an environment failure, never green.
- **Read-only**: `closeout.py --digests` prints `[closeout] digest <gate> measure= judge= files= env=` per gate;
  `--plan-only` adds each gate's `plan reuse` / `plan run` decision. Tests: `python3 -m unittest
  dev-notes/wave-runner/test_closeout.py`.

### Two wheels side by side (from LD.11b, W33)

The Python package is `glia-py` / `import glia_py` from W33 on (dist and module
renamed from the 0.4.x repo-graph names by `dev-notes/rename-0.5.0.py --python`); the
wheel is `target/wheels/glia_py-<ver>-cp311-abi3-*.whl`.

- **Install `glia-py` ALONGSIDE the old wheel; never uninstall the old one.** The
  repo-graph MCP server every agent queries imports the old module name from the
  user-site install (`/usr/bin/python3`), which the leap never touches. It keeps
  running that pre-leap build until the repo-graph session moves to `glia-py` (its
  LG.5 handoff), so the graph answers agents get lag the tree until then.
- `closeout.py --leap` installs `glia_py` into `~/.venvs/glia-leap`, which from W33 holds
  both modules (the old one frozen at W32). `grade.py`, `run.py`, `matrix.py`,
  `py/check_api_surface.py`, `py/tests/surface/*.py`, `bench/message-contracts/check.sh`
  (`PYTHON=`) and `closeout.py`'s stale / stamp checks import `glia_py`; an import of
  the old name would silently grade the frozen W32 build.
- The user-site `python3` has only the old wheel, so it cannot run the bench scripts
  from W33 on — use the venv's python.
- PyPI: the first `glia-py` upload needs a pending trusted publisher for project
  `glia-py` (the `release` job's comment in `.github/workflows/wheels-py.yml` names the
  settings page); the old dist stays at 0.4.18 unless James decides otherwise.
