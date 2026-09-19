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
| `usage_gate.py` | before launching a leap wave: `python3 usage_gate.py --start W<N>` prints GO / HOLD / UNKNOWN from the account's 5-hour usage (exit 0 / 2 / 3) and records the launch reading; `closeout.py --leap` records the end reading, so the gate learns what a wave costs. HOLD at >= 80% or when usage + an average wave passes 100%, unless the window resets within an hour. Data comes from `~/.claude/rate-limits.json`, written by the status line script on every render |

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
