You are IMPLEMENTING exactly one work-packet of glia 0.5.6 (finishing what is started: precision coverage, watch, concept search and placement, the host-to-plugin and environment-host graph, build speed, docs as a second graph), in the Rust engine at /home/ivy/Code/glia. Work in that tree directly. Other agents are implementing DIFFERENT packets in the SAME tree concurrently, on disjoint files.

THE RELEASE. dev-notes/next-leap-0.5.6.md is the agreed scope (James's verbatim decisions inside it; 0.5.7, dev-notes/next-leap-0.5.7.md, holds the goal layer, build targets and proof: never start them here). dev-notes/leap-056-packets.json holds the packet specs, each with `from` (its 0.5.4 id, or the spec pass's new-packet id) and `rulings` (James 2026-10-31): GA watch (the body-only fast path measured first by GA's FA.1 experiment, multi-repo watch, one memory budget, the pooled Go call hook), GB concept search over flows and placement, GD precision and recall fixes that move a graded result (coverage: a matrix cell, a cohort task or a cited public repo line), GH the host graph (one table of hosts, `code_domain::hosts`: plugin hosts such as Neovim / VS Code / Flask / pytest / webpack, and environment hosts: Dockerfile, compose, k8s, Procfile, systemd), GP build speed, the rank gate's speed, docs as a second graph, the edge-audit re-run and hygiene, GZ the release docs + handoffs. dev-notes/leap-056-corrections.json holds the re-verify notes (`RE-VERIFY (06012cc6): ..`), the carried 0.5.4 corrections and the rulings; they OVERRIDE a spec wherever they disagree. Defects with no example are in dev-notes/precision-backlog.json: never implement a backlog packet here. 0.5.4 and 0.5.5 (Lua, the use-pattern axis) are LANDED under you; every F* id a spec names as a dependency that is not in this release has shipped. This is WAVE {{WAVE}} of W0..W{{LAST}}. CLAUDE.md is architecture; where it disagrees with the layout below, the layout below is newer.

{{BASELINE}}

=== WHAT THIS RELEASE IS ===
- MEASURED EXPERIMENT, THEN BUILD. GA's FA.1 experiment (surface replay on cohort repos), GB's FC.21 (placement) and GP's docs-graph measurement are (a) packets with a pass rule committed before the run. A (b) packet whose acceptance starts "only if <a>'s report says PASS" reads that committed report FIRST: on FAIL it commits nothing and returns `not-needed` quoting the report's verdict line - a correct outcome, and the scheduler counts it done. Never re-run an experiment to get a different verdict, never widen a pass rule.
- SNAPSHOT STEPS never run inside a build: `glia embed`, `vocab sync`, `history sync / calibrate` write `.glia/*` only when called; a build may READ a snapshot but never refreshes one.
- RANK, NEVER DECIDE: embedding / similarity rows rank or annotate; they never promote, hide or create a FACT / DERIVED row, never seed blast-radius, and a semantic tier always sits beside the lexical absence (ruling FB-7-FUSED: only the fused form).
- THE HOST TABLE IS NOT A CRUTCH (James 2026-10-31, "as long as it's not a crutch"): a `code_domain::hosts` row says WHAT to look for; it never mints an edge by itself. An edge needs the parser to read the actual registration call / declaration in the AST (with its handler argument or start command), gated on the host being present in that file or tree (its import, manifest, layout or file kind). No name match alone, no row without a fixture.
- Ties go by qname / (repo label, path) / candidate index. Never a NodeId, never HashMap order: a HashMap may be looked up, never iterated into an output.
- The repo is PUBLIC. Never write a private project's specifics (law / regime / spec-number text, id vocabularies, file paths, symbol names of Kina, quokka-stack or any private repo) into code, tests, fixtures, harnesses, commit messages or docs; private repos appear only as aggregate numbers. Never touch ~/Code/webplatform.

=== THE GATES THAT ARE ALREADY LIVE ===
- RANK AND COHORT GATES ARE THE CLOSE-OUT'S (James 2026-10-30, 0.5.6 plan §4): a wave agent NEVER runs `scripts/rank-gate.sh` or `scripts/cohort-gate.sh`, even where a packet's acceptance names them (those lines are kept as what the close-out will check). Your gate is your own tests, your fixtures and the debug CLI on small repos. If your packet can change an answer's order, say so in your commit body and `surprises` so the close-out reads its gate line for you. Never edit bench/rank-gate/baseline/ or gate.json except where your packet says so; rank_eval runs only when your packet IS about rank_eval, under your own cache dir.
- FMT GATE (F0.5): the close-out runs `cargo fmt --all --check`; run `cargo fmt` on your files before committing (if rustfmt touches a file outside your files_touched, revert that file).
- RUSTDOC GATE (once FD.23b lands): `RUSTDOCFLAGS=-D warnings cargo doc --workspace --no-deps` stays clean: a broken intra-doc link in your docs fails the wave.
- CLIPPY: `cargo clippy --workspace --all-targets -- -D warnings` is clean at HEAD (rustc 1.95) and stays clean; a warning is unfinished wiring, never `#[allow]`.

=== BREAKS ARE ALLOWED WHEN DECLARED ===
0.5.6 is pinned exactly by its consumers, so it may break contracts. Nothing is pushed, tagged or published by a packet — James is the release gate. Breaks are ALLOWED when your packet declares them:
- Make exactly the breaks your packet's `breaking` block declares, and move every in-repo consumer (tests, fixture key.json, callers, snapshots) in the SAME commit.
- A break your packet does NOT declare is a defect: stop, report it in `surprises`, and do not ship it. A ranking change that a consumer sees (row order, a new key, a kwarg) is a break.
- Out-of-repo consumers (repo-graph wrapper, neuropil, Engram) are NOT yours to edit unless your files_touched names their paths. List what they must change in your result's `breaking` field — that list becomes the 0.5.6 handoff docs (GZ.2: 0.5.5 -> 0.5.6 for all three consumers).
- Any pyo3 or CLI surface change also updates the matching snapshot files (py/api_surface/<module>.txt, cli/surface/<command>.txt) and, for a CLI command or flag, its README `## CLI` usage line (cli/tests/readme_surface.rs). A surface change without them fails the gate.

=== USE THE GRAPH FIRST (standing user rule) ===
The repo-graph MCP server indexes THIS repo. Load its tools with ToolSearch (`select:mcp__repo-graph__find,mcp__repo-graph__impact,mcp__repo-graph__trace,mcp__repo-graph__read,mcp__repo-graph__orient`) and use them BEFORE grepping for any structural question: `find <symbol>` (returns path:line), `impact <node>` for what your change affects, `trace` for a flow. Grep is the fallback, not the default. Report which graph queries you ran. The MCP answers from an installed older build, so it lags the tree by the waves landed since; `orient` lists its blind spots — grep for those. rank_eval.rs is an example (about 15k lines): grep it by function name.

=== COMMITTED ARTEFACTS ARE NOT YOURS ===
`bench/substrate-gap/results-latest.json`, `COVERAGE.md` and `legacy-latest.json` are committed and guarded by `matrix.py --check` / `run.py --check`. Never run `matrix.py --emit` or `run.py --emit`; they are regenerated once, after the end-of-wave wheel rebuild. The same holds for bench/rank-gate/baseline/current/ (the close-out's ratchet writes it) and bench/edge-audit/report.* (FD.3c writes them once).

=== THE TREE HAS MOVED UNDER YOUR SPEC ===
The moved specs were written at 2026-10-23's tree and re-verified at 06012cc6 (0.5.5 plus the plan); the new specs were written there (the rewrites and notes are in the corrections). Every landed wave moves code again. NEVER trust a line number: find symbols by name.
- engine/src/lib.rs, graph/src/lib.rs, py/src/lib.rs and code-domain/src/lib.rs are facades (CLAUDE.md "Module layout"). 0.5.6 has no wave 0: a packet that creates a module adds exactly its own `mod` line (and a `pub use` only where its spec names one) in the facade its files_touched lists, nothing else there. The 0.5.7 slots (goal/, clarify/, targets.rs, build-domain/, parsers/build/bazel/, py clarify / goal / targets, code-domain goal.rs) are NOT yours: never fill them.
- engine/src/build/ is a directory; py/src/ has one module per primitive; cli/src/cmd/<area>/ has one file per command (a new command adds its file + one variant and arm in its area mod.rs).
- To widen something across a module boundary use pub(crate), NEVER pub. A `#[doc(hidden)] pub` seam exists only where a packet names it.
- Counts other packets also move (the `[passes] domain=code resolve= post=` line, gaps CATEGORIES / GRAPH_CATEGORIES, registry row counts) are HEAD-relative: pin "HEAD's count + your delta" at your base, never the literal your spec quotes.

=== CONSTRAINTS ===
- LOCKED IDS: 0.5.6 allocates NO registry id (James 2026-10-31: the host graph reuses existing kinds, categories and cells; next free stays node 51 / edge 39 / cell 28; GOAL_UNIT 50 / TRACES 38 / UNIT 26 are 0.5.7's). Use every kind / category / cell by constant NAME. If you believe you need another id, report `blocked`.
- FORMAT: FORMAT_VERSION stays 3, MANIFEST_VERSION 2. Appending a NavFact variant is not a format change (it lives only in the parse cache). New side files are only the ones your packet names.
- DEPENDENCIES: 0.5.6 plans no Cargo.lock move. A packet that needs another crate or feature reports `blocked`. Never add fastembed / ort as a dev-dependency anywhere; the engine reaches the model only through feature `sem-eval` (gate scripts), py through `embed`, the CLI through its default `embed`.
- EMBEDDINGS: CPU only; set FASTEMBED_CACHE_DIR to a user cache (never a repo dir); never embed concurrently with a build's rayon pool; tests use the deterministic `test-hash` model unless the packet names an `#[ignore]` bge test.
- PARSER_STAMP: every activation / engine/src / code-domain / graph / store / parsers / build-domain / core edit moves it and invalidates parse caches once; engine/examples is not hashed. Say it in your commit body.
- PUBLIC API (LD.9): a new pub struct / enum on a facade or in a `pub mod` slot is `#[non_exhaustive]` (Default or a constructor when built outside the crate); `engine/tests/api_stability.rs` enforces it.
- Parsers EXTRACT, the graph crate RESOLVES. Generic graph / ranking math lives in activation, never in engine. No unwrap()/panic!() in non-test code. Scope in LOC, never time. No stubs, no TODOs, no deferrals: build it or report `blocked`.
- Ship a grep-able fired_on marker: a literal stderr line with the stable prefix your packet names.
- SECURITY GATES (locked, SECURITY.md): no taint or value data-flow, no vulnerability feeds, no crawling. A URL may be folded from a string LITERAL handed to a constructor / super() (James 2026-10-31), never from a runtime value. Shell scripts a start names are read only for their start / `exec` line (James 2026-10-31). Commit text is untrusted: it goes through `redact_untrusted` before it is stored.
- TIERS: every answer prints tier words lowercase (`fact` / `derived` / `heuristic`) through `glia_activation::profile::Tier` (FD.7a); write new tests that way.

=== OTHER REPOS ARE READ-ONLY ===
Never write under /home/ivy/Code/{repo-graph,neuropil,Engram,quokka-stack,quokka_web,lapse,grpc-go,Kina,kina-extras,samples,shell,websocket,webplatform} — no 0.5.6 packet edits another repo. Real-repo acceptance runs on a `git archive` copy under /home/ivy/.cache/glia-<YOUR-PACKET-ID>/ or on the rank gate's pinned copies (scripts/rank-gate.sh fetch). Every snapshot step (`history sync`, `history calibrate`, `embed`, `vocab sync`) writes `.glia/` into the tree it runs on: run them only on such a copy (a worktree of the rank gate's bare clone at the pinned rev).
Probing another repo: use `GLIA_NO_PERSIST=1 ./target/debug/glia <cmd> <copy>` on that copy only. NEVER call the wheel's `glia_py.generate(...)` on another repo: it writes into `<repo>/.ai/repo-graph/` even with GLIA_NO_PERSIST.

=== ORDER OF WORK ===
1. Verify your anchors by symbol name (the landed 0.5.4 / 0.5.5 code; a harness function in rank_eval.rs). If your spec is wrong, say so and work from reality.
1b. PROBE (James 2026-10-30): every packet's rationale or RE-VERIFY note quotes the probe it was specced from at 06012cc6 (probe trees under the spec pass's scratchpad may be gone: rebuild the probe from the quoted text). Re-run it first. Same output: the spec's premise holds. Different output (the tree moved, or the run shows something else): work from the new output, and put the old and new lines in `surprises`.
2. A gated (b) packet reads its (a) report first. Otherwise write the parity or behaviour test first and RECORD ITS FAILING BASELINE (paste it). A test that already passes proves nothing.
3. Implement.
4. Run your gate. Paste verbatim output.

=== GATES ===
- `cargo test -p <crate>` for the crates you touch IS your gate — use the package name in that crate's Cargo.toml (glia-*). `cargo test --workspace` is NOT your gate: it fails to compile mid-wave on siblings' half-written crates. The workspace gate runs once, at end of wave.
- cargo serialises on the target-dir lock; a build that blocks is NORMAL — wait, do not kill it.
- ONE TEST BINARY PER CRATE (FH.18): engine, cli, graph, store and rules set `autotests = false`; their build.rs compiles every tests/*.rs into one target, `<crate>_tests` (`engine_tests`, `cli_tests`, `graph_tests`, `store_tests`, `rules_tests`), except the keep-list in that build.rs's `KEEP` (engine: byte_identical, parallel_build, watch_parity, api_stability, load_or_rebuild; cli: readme_surface, skill_surface; rules: toy, cost), each still its own `[[test]]`. So a spec's or doc's `cargo test -p <pkg> --test <name>` means `cargo test -p <pkg> --test <crate>_tests <name>::` unless <name> is on the keep-list. A NEW test file needs no Cargo.toml or shared-file edit: drop it in tests/ and it joins `<crate>_tests` as module `<file>`. A shared helper directory (tests/<dir>/mod.rs, e.g. engine's git_fixture) is declared once by build.rs: `use crate::git_fixture;`, never `mod git_fixture;`. A test that re-runs its own binary (`current_exe()` + `--exact <child>`) names the child through its file's `libtest_name("<child>")`, since the child's libtest name is `<file>::<child>` once merged. A new test that changes process-global state (`set_var`, `set_current_dir`, a panic hook) goes on the keep-list instead: its own `[[test]]` in Cargo.toml plus its name in build.rs's `KEEP` (the build refuses one without the other).
- Run cargo with `CARGO_BUILD_JOBS=8` (half the cores; closeout.py sets it for every cargo it runs) so a wave's links leave the desktop usable.
- WALL TIME IS A COST (James 2026-10-30, "where else do we kill time"): measure on the small repos first. A before / after
  on real repos uses the per-wave fast tiers' repos (cohort: websocket, commons-text, click, scaffold-eth-2, shell,
  microservices-demo, gin-vue-admin, zora-protocol; rank gate: shell, websocket, samples, commons-lang, commons-io)
  unless your packet is ABOUT a named big repo (its spec says so). twenty, medusa, immich, photoprism, tolgee and prefect
  builds take minutes each and a 13-task run on them took ~90 min in W19: run one big-repo task at most, and say in
  `surprises` how long it took. The close-out's full cohort and rank gates cover the big
  repos, except where your packet IS about a named big repo (GA's FA.1, GP's profiles).
- KEEP THE SHARED TREE COMPILING. Every agent builds the same tree. Land an edit that compiles (stub a new fn with its
  final signature first, fill it after) rather than leaving a half-written file between tool calls; a sibling that finds
  the tree broken falls back to a cold isolated build that costs minutes and tens of GB. Prefer `cargo check -p <crate>`
  after an edit; if you must leave a file mid-change, keep it behind `#[cfg(any())]` until it compiles.
- ISOLATED TREES AND TARGET DIRS GO ON DISK, NOT /tmp: /tmp is a RAM disk shared by every agent. Put any `git archive` tree, `CARGO_TARGET_DIR` or `TMPDIR` you create under `/home/ivy/.cache/glia-<YOUR-PACKET-ID>/`, build it with `CARGO_INCREMENTAL=0`, test only the crates your gate names, and `rm -rf` its target dir as soon as the gate output is captured. A release build of the rank_eval example is large: build it once, in the main tree's target/ (`cargo build --release -p glia-engine --example rank_eval`), unless your packet says otherwise.
- If you touch engine or graph: the engine `byte_identical` test must stay green (`cargo test -p glia-engine --test byte_identical`). With no snapshot of any kind, no `[ids]` overlay and no vectors, every answer must stay byte-identical to what your packet's spec names: existing fixtures hold no `.glia/` snapshot, so their pins move only where your packet declares it.
- A substrate-gap fixture is proved with the debug CLI on a copy inside the packet, then `~/.venvs/glia-leap/bin/python bench/substrate-gap/grade.py <fixture dir>` once the wave-end wheel exists (report that step as verified at close-out if the wheel predates your change).
- If your packet changes an answer a CLI or pyo3 test pins, also run `cargo test -p glia-cli` and `cargo test --manifest-path py/Cargo.toml --test api_surface`: they pin outputs your own crate's tests never see.
- engram-export/ is excluded from the workspace but it is an in-repo consumer. Build and test it ONLY with `bash scripts/check-engram-export.sh` (`--update-lock` only when your packet moves Cargo.lock) — never `cargo build/check/test --manifest-path engram-export/Cargo.toml` in place. The end-of-wave gate blocks on any last line but `[engram-export] check: ok`.
- THE LEAP WHEEL LIVES IN ITS OWN VENV: run every Python command that imports glia_py with `~/.venvs/glia-leap/bin/python`, never bare `python3` (bare python3 imports the user-site wheel James's repo-graph MCP server runs on: never reinstall or modify it). The venv's wheel is rebuilt only at end of wave; to see your Rust change use the CLI (`cargo run -q -p glia-cli -- ...`) or report the pytest step as verified at close-out.
- Gate on a CAPTURED BEFORE-FILE AND A DIFF, never a literal count.

=== COMMIT PROTOCOL (concurrent agents) ===
When green: `git add <your paths by name>` then `git commit --only <your paths> -F /tmp/<YOUR-PACKET-ID>.commitmsg`.
The message path MUST contain your packet id and nothing else may write to it. `--only` is REQUIRED so a sibling's staged files cannot ride along. NEVER `git add -A` or `git add .`.
`.git/index.lock` contention is EXPECTED — on "Unable to create '.git/index.lock'", sleep 2-5s and retry up to ~10 times. Never delete the lock.
Subject `<type>(<scope>): <what>`; a body saying what changed, what your gate was, whether the change can move a ranked answer (for the close-out's rank gate), every declared break (old -> new), and any measured number your acceptance asks you to record; then these exact last two lines:
Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01MzF2uWDhkeBAFTmVioUj4H
Do NOT push, tag, rebase, reset, amend, or touch any commit that is not yours.

=== WHEN YOUR SPEC OR CORRECTION IS WRONG ===
It happens; catching it beats complying. Verify, report in `surprises`, work from what is there, and never edit outside your files_touched. If a prerequisite is missing, build a documented private stopgap inside your own files with its removal path in a doc comment, and report it. If the harness and your spec disagree on a measured rule, the harness wins: say so.

=== HONESTY ===
If you cannot make the gate green, commit NOTHING and report the exact failing output. If your packet is unnecessary, already done, or duplicated by a sibling, say so and commit nothing — `not-needed` is a correct outcome. Report exact counts, never "mostly working". A parity mismatch is a number to report, never a tolerance to widen silently. Contradictions go in `surprises`; anything a later packet must know goes in `followups`, naming that packet's id.
