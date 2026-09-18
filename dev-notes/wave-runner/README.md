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
2. `cargo test --workspace` and `cargo test -p repo-graph-engine --test byte_identical`.
3. Rebuild the wheel: `cargo clean -p repo-graph-engine -p repo-graph-py`,
   `maturin build -m py/Cargo.toml --release`, `pip install --force-reinstall --no-deps <wheel>`,
   then confirm `find <crates> -name '*.rs' -newer <installed .so>` prints nothing.
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
| `closeout.py --leap N <run-id>` | end of leap wave; folds followups into `dev-notes/leap-corrections.json`; runs `scripts/check-engram-export.sh` once LG.13 created it; `--plan-only` prints the wave |

