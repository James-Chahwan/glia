You are IMPLEMENTING exactly one work-packet of glia 0.5.3 (the ranking release), in the Rust engine at /home/ivy/Code/glia. Work in that tree directly. Other agents are implementing DIFFERENT packets in the SAME tree concurrently, on disjoint files.

THE RELEASE. dev-notes/next-leap-0.5.3.md is the agreed scope (items 1-11, the gate and the hygiene, all confirmed by James 2026-10-23, plus the round-3 scope notes); the evidence is dev-notes/ranking-scoreboard.md and the round reports it lists (dev-notes/ranking-eval-*.md), all produced by engine/examples/rank_eval.rs. dev-notes/leap-053-packets.json holds the packet specs: wave 0 E0 (the module slots), then EA structural ranking (the PPR index and its cache, the candidate file space, co-change first, PPR d0.85 + sum pooling, BFS, in blast-radius / diff-impact / review / pack), EB intent (commit messages in the history snapshot, BM25 text ranking, the `[ids]` vocabulary, typed id routing IDR, LX fusion, `review --title`, `find --files`), EC per-repo calibrated selection (the `.glia/calibration-snapshot/` record, the leak-free online replay, `glia history calibrate`, selection in every ranked answer), EG the rank gate (rank_eval's ANS rankers, `--gate-check`, bench/rank-gate/ + scripts/rank-gate.sh in the close-out, its coverage of every item) plus the DE.5 removal and the wheels CI fix, EZ the release docs + handoffs. dev-notes/leap-053-corrections.json holds corrections that OVERRIDE a spec wherever they disagree. The 0.5.0 / 0.5.1 / 0.5.2 packet files are landed history. This is WAVE {{WAVE}} of W0..W{{LAST}}. CLAUDE.md is architecture; where it disagrees with the layout below, the layout below is newer.

{{BASELINE}}

=== WHAT THIS RELEASE IS ===
Each ranking packet PORTS a mechanism the 2026-10-18..19 experiments measured in engine/examples/rank_eval.rs into the engine's answers. The harness is the specification: when your packet names a harness function (`file_pool`, `bfs_key`, `better`, `lex2`, `Ppr::power`, `tokens`, `r3_select`, `branch_labels`, ...) read it in engine/examples/rank_eval.rs and reproduce its arithmetic, including float accumulation order and tie rules, unless your packet says otherwise. A paraphrase of a measured rule was the commonest defect the spec run's adversarial verifiers found (83 defects over the four groups): never "simplify" one.
- ONE file space: `glia_engine::rank::FileIndex` (EA.3) is rank_eval's `index_graph` candidate rule; co-change, text, IDR and the calibration replay all index files through it, and commit files map through `HistoryFile.p` with NO rename fold (the harness's `git log --no-renames`).
- Rankings are QUERY-TIME. Anything computed from history is a snapshot step (`glia history sync`, `glia history calibrate`), never inside a build, never in a .gmap.
- Ties go by (repo label, path) / candidate index. Never a NodeId, never HashMap order: a HashMap may be looked up, never iterated into an output.
- The repo is PUBLIC. Never write Kina domain specifics (law / regime / spec-number text, its id vocabulary, its file paths) into a packet's code, test, harness change, commit message or doc; private repos appear only as aggregate numbers, as the round reports do. rank_eval reads the specifics from a private config (`--private` / GLIA_RANK_PRIVATE, landed in 563385dd); never pass it from the gate, never add to the harness.

=== THE RANK GATE ===
After EG.3 lands, `bash scripts/rank-gate.sh check` is part of every close-out of a wave that touches a ranked path, and every ranking packet's acceptance names it: its last line must be `[rank-gate] check: ok - ...`. A `FAILED (<k> drops: ..)` line is your packet's failure, not noise: the harness is deterministic, so a rerun cannot clear it. Never edit bench/rank-gate/baseline/ by hand; only `baseline` (a declared re-baseline), `baseline-add` (a packet that adds a gated ranker, EG.6 / EG.7) and the close-out's `ratchet` write it. The gate's work dir is `$HOME/.cache/glia-rank-gate` (GLIA_RANK_GATE_DIR): a fetch there is shared by every agent; never delete it. rank_eval's defaults write rank_eval*.json / *.md into the cwd: always pass `--out/--md/--r1md/--r3md` under your own cache dir, and check `git status --short` is clean of them.

=== BREAKS ARE ALLOWED WHEN DECLARED ===
0.5.3 is pinned exactly by its consumers, so it may break contracts. Nothing is pushed, tagged or published by a packet — James is the release gate. Breaks are ALLOWED when your packet declares them:
- Make exactly the breaks your packet's `breaking` block declares, and move every in-repo consumer (tests, fixture key.json, callers, snapshots) in the SAME commit.
- A break your packet does NOT declare is a defect: stop, report it in `surprises`, and do not ship it. A ranking change that a consumer sees (row order, a new key, a kwarg) is a break.
- Out-of-repo consumers (repo-graph wrapper, neuropil, Engram) are NOT yours to edit unless your files_touched names their paths. List what they must change in your result's `breaking` field — that list becomes the 0.5.3 handoff docs (EZ.2).
- Any pyo3 or CLI surface change also updates the matching snapshot files (py/api_surface/<module>.txt, cli/surface/<command>.txt) and, for a CLI command or flag, its README `## CLI` usage line (cli/tests/readme_surface.rs). A surface change without them fails the gate.

=== USE THE GRAPH FIRST (standing user rule) ===
The repo-graph MCP server indexes THIS repo. Load its tools with ToolSearch (`select:mcp__repo-graph__find,mcp__repo-graph__impact,mcp__repo-graph__trace,mcp__repo-graph__read,mcp__repo-graph__orient`) and use them BEFORE grepping for any structural question: `find <symbol>` (returns path:line), `impact <node>` for what your change affects, `trace` for a flow. Grep is the fallback, not the default. Report which graph queries you ran. The MCP answers from an installed older build, so it lags the tree by the waves landed since; `orient` lists its blind spots — grep for those. rank_eval.rs is an example (about 14k lines): grep it by function name.

=== COMMITTED ARTEFACTS ARE NOT YOURS ===
`bench/substrate-gap/results-latest.json`, `COVERAGE.md` and `legacy-latest.json` are committed and guarded by `matrix.py --check` / `run.py --check`. Never run `matrix.py --emit` or `run.py --emit`; they are regenerated once, after the end-of-wave wheel rebuild. The same holds for bench/rank-gate/baseline/current/ (the close-out's ratchet writes it).

=== THE TREE HAS MOVED UNDER YOUR SPEC ===
Specs were verified at b3fc2ac7; 563385dd (the Kina scrub of rank_eval.rs) landed on top before W0, and every landed wave moves code again. NEVER trust a line number: find symbols by name.
- engine/src/lib.rs and graph/src/lib.rs are FACADES (CLAUDE.md "Module layout"): module declarations and re-exports, no logic. E0 declared 0.5.3's slots: activation `algo::fuse` (EB.2), engine `rank/` (EA: you add `mod <file>;` + `pub use` lines to engine/src/rank/mod.rs), `intent/` (E0.2 already declared text / ids / defs / paths / idr: fill your file, never edit intent/mod.rs) and `calibrate/` (EC: add your `pub mod` / `pub use` lines to engine/src/calibrate/mod.rs). Reach them by module path (`glia_engine::rank::RankCache`); never flatten into the crate root. Only E0.* packets edit a facade.
- engine/src/build/ is a directory; py/src/ has one module per primitive; cli/src/cmd/<area>/ has one file per command.
- To widen something across a module boundary use pub(crate), NEVER pub. A `#[doc(hidden)] pub` seam exists only where a packet names it (the rank_eval harness is an example of the engine crate and uses public paths only).

=== CONSTRAINTS ===
- LOCKED IDS: 0.5.3 allocates NO registry id (node_kind / edge_category / cell_type unchanged; next free 50 / 38 / 26). Use every kind / category / cell by constant NAME. If you believe you need an id, report `blocked`.
- FORMAT: FORMAT_VERSION stays 3, MANIFEST_VERSION 2; no packet changes the .gmap layout. The two declared on-disk changes are snapshot sidecars: the history snapshot's version 2 + messages.jsonl (EB.1) and `.glia/calibration-snapshot/` (EC.1).
- DEPENDENCIES: no crate is added and the workspace Cargo.lock does not move in 0.5.3. engram-export/Cargo.lock moves once, in EG.4. A packet that needs a crate reports `blocked`.
- PARSER_STAMP: every activation / engine/src / code-domain / snapshots edit moves it and invalidates parse caches once; engine/examples is not hashed. That is expected; say it in your commit body when your packet moves it.
- PUBLIC API (LD.9): a new pub struct / enum on the engine or graph facade or in a `pub mod` slot is `#[non_exhaustive]` (derive `Default` or give a constructor when a caller outside the crate builds one: py, cli and the rank_eval example are outside the engine crate); `engine/tests/api_stability.rs` enforces it.
- Parsers EXTRACT, the graph crate RESOLVES. Generic graph / ranking math lives in activation (algo::fuse, PprIndex), never in engine. No unwrap()/panic!() in non-test code. Scope in LOC, never time. No stubs, no TODOs, no deferrals: if your packet needs it, build it or report `blocked`.
- Ship a grep-able fired_on marker: a literal stderr line with the stable prefix your packet names (`[rank]`, `[rank-cache]`, `[intent] text`, `[idr]`, `[calibrate]`, `[select]`, `[rank-gate]` ...).
- SECURITY GATES (locked): no taint or value data-flow, no vulnerability feeds, no crawling. Commit messages and branch labels are untrusted text: they go through the A13.7 redaction (`redact_untrusted`) before they are stored, and trailers (author identities) are dropped (EB.1).

=== OTHER REPOS ARE READ-ONLY ===
Never write under /home/ivy/Code/{repo-graph,neuropil,Engram,quokka-stack,quokka_web,lapse,grpc-go,Kina,kina-extras,samples,shell,websocket} — no 0.5.3 packet edits another repo. Real-repo acceptance runs on a `git archive` copy under /home/ivy/.cache/glia-<YOUR-PACKET-ID>/ or on the rank gate's pinned copies (scripts/rank-gate.sh fetch). A `glia history sync` / `glia history calibrate` writes `.glia/` into the tree it runs on: run them only on such a copy (EC.5 says how: a worktree of the gate's bare clone at the pinned rev).
Probing another repo: use `GLIA_NO_PERSIST=1 ./target/debug/glia <cmd> <copy>` on that copy only. NEVER call the wheel's `glia_py.generate(...)` on another repo: it writes into `<repo>/.ai/repo-graph/` even with GLIA_NO_PERSIST.

=== ORDER OF WORK ===
1. Verify your anchors by symbol name, in the engine AND in rank_eval.rs. If your spec is wrong, say so and work from reality.
2. Write the parity or behaviour test first and RECORD ITS FAILING BASELINE (paste it). A test that already passes proves nothing.
3. Implement.
4. Run your gate. Paste verbatim output.

=== GATES ===
- `cargo test -p <crate>` for the crates you touch IS your gate — use the package name in that crate's Cargo.toml (glia-*). `cargo test --workspace` is NOT your gate: it fails to compile mid-wave on siblings' half-written crates. The workspace gate runs once, at end of wave.
- cargo serialises on the target-dir lock; a build that blocks is NORMAL — wait, do not kill it.
- ISOLATED TREES AND TARGET DIRS GO ON DISK, NOT /tmp: /tmp is a RAM disk shared by every agent. Put any `git archive` tree, `CARGO_TARGET_DIR` or `TMPDIR` you create under `/home/ivy/.cache/glia-<YOUR-PACKET-ID>/`, build it with `CARGO_INCREMENTAL=0`, test only the crates your gate names, and `rm -rf` its target dir as soon as the gate output is captured. A release build of the rank_eval example is large: build it once, in the main tree's target/ (`cargo build --release -p glia-engine --example rank_eval`), unless your packet says otherwise.
- If you touch engine or graph: the engine `byte_identical` test must stay green (`cargo test -p glia-engine --test byte_identical`). With no history snapshot and no calibration record, every answer must stay byte-identical to the order your packet's spec names (usually 0.5.2's or the previous packet's): the fixtures of the existing tests hold no `.glia/` snapshot, so their pins move only where your packet declares it.
- If your packet changes an answer a CLI or pyo3 test pins, also run `cargo test -p glia-cli` and `cargo test --manifest-path py/Cargo.toml --test api_surface`: they pin outputs your own crate's tests never see.
- engram-export/ is excluded from the workspace but it is an in-repo consumer. Build and test it ONLY with `bash scripts/check-engram-export.sh` (EG.4 with `--update-lock`) — never `cargo build/check/test --manifest-path engram-export/Cargo.toml` in place. The end-of-wave gate blocks on any last line but `[engram-export] check: ok`.
- THE LEAP WHEEL LIVES IN ITS OWN VENV: run every Python command that imports glia_py with `~/.venvs/glia-leap/bin/python`, never bare `python3` (bare python3 imports the user-site wheel James's repo-graph MCP server runs on: never reinstall or modify it). The venv's wheel is rebuilt only at end of wave; to see your Rust change use the CLI (`cargo run -q -p glia-cli -- ...`) or report the pytest step as verified at close-out.
- Gate on a CAPTURED BEFORE-FILE AND A DIFF, never a literal count.

=== COMMIT PROTOCOL (concurrent agents) ===
When green: `git add <your paths by name>` then `git commit --only <your paths> -F /tmp/<YOUR-PACKET-ID>.commitmsg`.
The message path MUST contain your packet id and nothing else may write to it. `--only` is REQUIRED so a sibling's staged files cannot ride along. NEVER `git add -A` or `git add .`.
`.git/index.lock` contention is EXPECTED — on "Unable to create '.git/index.lock'", sleep 2-5s and retry up to ~10 times. Never delete the lock.
Subject `<type>(<scope>): <what>`; a body saying what changed, what the gate was (the `[rank-gate]` last line when your packet names it), every declared break (old -> new), and any measured number your acceptance asks you to record; then these exact last two lines:
Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01HeSHno5K3wK4b3U5tYBKt6
Do NOT push, tag, rebase, reset, amend, or touch any commit that is not yours.

=== WHEN YOUR SPEC OR CORRECTION IS WRONG ===
It happens; catching it beats complying. Verify, report in `surprises`, work from what is there, and never edit outside your files_touched. If a prerequisite is missing, build a documented private stopgap inside your own files with its removal path in a doc comment, and report it. If the harness and your spec disagree on a measured rule, the harness wins: say so.

=== HONESTY ===
If you cannot make the gate green, commit NOTHING and report the exact failing output. If your packet is unnecessary, already done, or duplicated by a sibling, say so and commit nothing — `not-needed` is a correct outcome. Report exact counts, never "mostly working". A parity mismatch is a number to report, never a tolerance to widen silently. Contradictions go in `surprises`; anything a later packet must know goes in `followups`, naming that packet's id.
