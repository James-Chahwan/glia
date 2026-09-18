You are IMPLEMENTING exactly one work-packet of the glia 0.5.0 leap, in the Rust engine at /home/ivy/Code/glia. Work in that tree directly. Other agents are implementing DIFFERENT packets in the SAME tree concurrently, on disjoint files.

THE LEAP. dev-notes/next-leap-0.5.0.md is the agreed scope; dev-notes/leap-packets.json holds the packet specs (leap packets L0 / LA..LG, plus the Batch C rows with their re-verification); dev-notes/leap-corrections.json holds corrections that OVERRIDE a spec wherever they disagree. This is WAVE {{WAVE}} of W0..W{{LAST}}. CLAUDE.md is architecture; where it disagrees with the layout below, the layout below is newer.

{{BASELINE}}

=== THIS IS THE BREAKING RELEASE ===
0.5.0 ships every contract break at once, moving glia, repo-graph and Engram together. Nothing is pushed, tagged or published until the bump — James is the release gate. Breaks are ALLOWED when your packet declares them:
- Make exactly the breaks your packet's `breaking` block declares, and move every in-repo consumer (tests, fixture key.json, callers, snapshots) in the SAME commit.
- A break your packet does NOT declare is a defect: stop, report it in `surprises`, and do not ship it.
- Out-of-repo consumers (repo-graph wrapper, neuropil, Engram) are NOT yours to edit unless your files_touched names their paths. List what they must change in your result's `breaking` field — that list becomes the handoff docs (LG.5, LG.14).
- After LG.6a lands, any pyo3 or CLI surface change also updates the matching snapshot files (py/api_surface/<module>.txt, cli/surface/<command>.txt). A surface change without its snapshot fails the gate.

=== USE THE GRAPH FIRST (standing user rule) ===
The repo-graph MCP server indexes THIS repo. Load its tools with ToolSearch (`select:mcp__repo-graph__find,mcp__repo-graph__impact,mcp__repo-graph__trace,mcp__repo-graph__read,mcp__repo-graph__orient`) and use them BEFORE grepping for any structural question: `find <symbol>` (returns path:line), `impact <node>` for what your change affects, `trace` for a flow. Grep is the fallback, not the default. Report which graph queries you ran. Known graph blind spots until their packets land: Rust full-crate-path calls (LA.1), calls inside macro arguments (LA.2), inline `mod x {}` and enum variants (LA.3) — grep for those.

=== COMMITTED ARTEFACTS ARE NOT YOURS ===
`bench/substrate-gap/results-latest.json`, `COVERAGE.md` and `legacy-latest.json` are committed and guarded by `matrix.py --check` / `run.py --check`. Never run `matrix.py --emit` or `run.py --emit`; they are regenerated once, after the end-of-wave wheel rebuild.

=== THE TREE HAS MOVED UNDER YOUR SPEC ===
Specs were verified at 7a4883b. Wave 0 then split the hot files (L0.2-L0.6) and every landed wave moves code again. NEVER trust a line number: find symbols by name.
- engine/src/lib.rs and graph/src/lib.rs are FACADES: glob re-exports plus one public module slot per leap primitive (engine: delta, persist, profile, trace, find, why, effects, cycles, ...; graph: roles, rust_paths, nav, identity, cells, ...). Fill your slot; call it from cli/py by module path. Only LD.11a edits engine/src/lib.rs.
- engine/src/build/ is a directory (mod, assemble, grafts, rpc_needles, lang_build, resolvers); py/src/ has one module per primitive; cli/src/cmd/ has one file per command; store/src/lib.rs glob re-exports its modules after LC.5a.
- To widen something across a module boundary use pub(crate), NEVER pub.

=== THE FROZEN key.json VOCABULARY ===
grade.py RAISES on any unknown top-level field. Allowed: framework, language, dirs, expect_nodes, expect_edges, expect_cells, forbid, mechanism, cells, note.
  forbid: [{kind,name?,max_nodes?} | {from,to,category}]. FORBID MATCHES EXACTLY — the normalised pattern must EQUAL a name or qname.
  expect_cells: [{kind,node,cell,contains?}].
  mechanism / cells: VALIDATED against matrix_vocab.py; an unknown language or mechanism RAISES.
For new matrix probes use `python3 bench/substrate-gap/scaffold.py <lang> <mech>` and AUTHORING.md.

=== CONSTRAINTS ===
- LOCKED IDS: NodeKind / EdgeCategory / CellType ids are allocated in code-domain/src/lib.rs ONLY, by L0.1 (edge_category 35 NAVIGATES_TO, 36 CO_CHANGES; cell_type 19 DOC_TAGS, 20 ROLE, 21 EVIDENCE, 22 SCHEMA_FIELDS, 23 COVERAGE, 24 ENTRYPOINT, 25 ACCESS_MODE). Refer to them by constant NAME. NEVER pick, renumber or reuse an id.
- FORMAT: only LC.1 bumps FORMAT_VERSION / MANIFEST_VERSION.
- Parsers EXTRACT, the graph crate RESOLVES. No unwrap()/panic!() in non-test code. Scope in LOC, never time. No stubs, no TODOs, no deferrals: if your packet needs it, build it or report `blocked`.
- Ship a grep-able fired_on marker: a literal stderr line with a stable prefix, e.g. `[queues] scan needle=...`, with a per-language / per-source discriminator if shared.
- SECURITY GATES (locked): no taint or value data-flow, no CVE / vulnerability-feed joins, no mass-corpus crawling, no lists of routes reaching data without auth. Untrusted text (CI logs, .env, config) goes through the A13.7 redaction before it is stored.

=== OTHER REPOS ARE READ-ONLY ===
Never write under /home/ivy/Code/{repo-graph,neuropil,Engram,quokka-stack,lapse} unless your files_touched names the path (only LG.7 / LG.14 edit Engram, after James's explicit OK — Engram is NOT a git repo, so back it up first as your packet says).
Probing another repo: use `GLIA_NO_PERSIST=1 ./target/debug/glia <cmd> <repo>` only. NEVER call the wheel's `repo_graph_py.generate(...)` on another repo: it writes and purges `<repo>/.ai/repo-graph/` even with GLIA_NO_PERSIST (the spec run left stray caches in quokka-stack and neuropil that way).

=== ORDER OF WORK (fixture-first) ===
1. Verify your anchors by symbol name. If your spec is wrong, say so and work from reality.
2. Author fixtures and RECORD THEIR FAILING BASELINE FIRST — run grade.py on each and paste the output before writing the fix. A fixture that already passes proves nothing.
3. Implement.
4. Run your gate. Paste verbatim output.

=== GATES ===
- `cargo test -p <crate>` for the crates you touch IS your gate — use the package name in that crate's Cargo.toml (LD.11a renames repo-graph-* to glia-*). `cargo test --workspace` is NOT your gate: it fails to compile mid-wave on siblings' half-written crates. The workspace gate runs once, at end of wave.
- cargo serialises on the target-dir lock; a build that blocks is NORMAL — wait, do not kill it.
- If you touch engine or graph: the engine `byte_identical` test must stay green (`cargo test -p <engine package> --test byte_identical`).
- engram-export/ is excluded from the workspace but it is an in-repo consumer. If your packet renames a crate, or changes or moves a pub item it uses (generate_one, MergedGraph, RepoGraph / Node / Edge / Cell / CellPayload / RepoId literals, CodeNav::record, the node_kind / edge_category / cell_type consts, core::project_name), fix engram-export in the same commit and list its files in your files_touched. Build and test it ONLY with `bash scripts/check-engram-export.sh` — never `cargo build/check/test --manifest-path engram-export/Cargo.toml` in place, which rewrites the tracked engram-export/Cargo.lock. If your packet changes engram-export's dependency set (a crate rename, a new crate under engine or any crate it pulls in), run the script with --update-lock and commit engram-export/Cargo.lock. Mid-wave a sibling's half-written crate can break the live tree: then point it at a coherent tree with `GLIA_ROOT=<git archive HEAD + your change>`. The end-of-wave gate runs the script and blocks on any last line but `[engram-export] check: ok`.
- grade.py reads the INSTALLED wheel, rebuilt only at end of wave. To see your Rust change use `GLIA_NO_PERSIST=1 cargo run -q -p <cli package> -- analyze <fixture> --format json`; otherwise report the cell unverifiable-this-wave. Do not rebuild the wheel.
- Gate on a CAPTURED BEFORE-FILE AND A DIFF, never a literal count: `python3 bench/substrate-gap/run.py --no-log > /tmp/<pkt>_before.txt` first.

=== COMMIT PROTOCOL (concurrent agents) ===
When green: `git add <your paths by name>` then `git commit --only <your paths> -F /tmp/<YOUR-PACKET-ID>.commitmsg`.
The message path MUST contain your packet id and nothing else may write to it. `--only` is REQUIRED so a sibling's staged files cannot ride along. NEVER `git add -A` or `git add .`.
`.git/index.lock` contention is EXPECTED — on "Unable to create '.git/index.lock'", sleep 2-5s and retry up to ~10 times. Never delete the lock.
Subject `<type>(<scope>): <what>`; a body saying what changed, what the gate was, and every declared break (old -> new); then these exact last two lines:
Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01DeZg4LEQveWWZQ1ZL1bZvd
Do NOT push, tag, rebase, reset, amend, or touch any commit that is not yours.

=== WHEN YOUR SPEC OR CORRECTION IS WRONG ===
It happens; catching it beats complying. Verify, report in `surprises`, work from what is there, and never edit outside your files_touched. If a prerequisite is missing, build a documented private stopgap inside your own files with its removal path in a doc comment, and report it.

=== HONESTY ===
If you cannot make the gate green, commit NOTHING and report the exact failing output. If your packet is unnecessary, already done, or duplicated by a sibling, say so and commit nothing — `not-needed` is a correct outcome. Report exact counts, never "mostly working". Contradictions go in `surprises`; anything a later packet must know goes in `followups`, naming that packet's id.
