You are IMPLEMENTING exactly one work-packet in the glia Rust engine at /home/ivy/Code/glia. Work in that tree directly. Other agents are implementing DIFFERENT packets in the SAME tree concurrently, on disjoint files.

PROGRAMME. dev-notes/wave-plan-2026-09-16.md is the plan; dev-notes/wave-packets.json holds the packet specs; dev-notes/packet-corrections.json holds corrections that OVERRIDE a spec wherever they disagree. This is WAVE {{WAVE}} of {{WAVES}}. CLAUDE.md is current — it documents the post-split module layout and the locked-id rule.

{{BASELINE}}

=== USE THE GRAPH FIRST (standing user rule) ===
The repo-graph MCP server indexes THIS repo and is fresh. Load its tools with ToolSearch (`select:mcp__repo-graph__find,mcp__repo-graph__impact,mcp__repo-graph__trace,mcp__repo-graph__read,mcp__repo-graph__orient`) and use them BEFORE grepping for any structural question: `find <symbol>` to locate a function by name (it returns path:line), `impact <node>` for what your change affects, `trace` for a flow. Grep is the fallback, not the default. Report in your result which graph queries you ran. If a symbol you expect is missing from the graph, that is itself a finding — say so.

=== COMMITTED ARTEFACTS ARE NOT YOURS ===
`bench/substrate-gap/results-latest.json`, `COVERAGE.md` and `legacy-latest.json` are committed and guarded by `matrix.py --check` / `run.py --check`. DO NOT regenerate or commit them — never run `matrix.py --emit` or `run.py --emit`. Grading them before the end-of-wave wheel rebuild freezes cells at the wrong level (wave 3 did exactly this; `--check` then reported DRIFT 2). They are regenerated once, after the rebuild, at end of wave.

=== THE TREE HAS MOVED UNDER YOUR SPEC ===
Your packet's entry_points cite line numbers from BEFORE wave 0, and five waves have landed since. NEVER trust a line number in your spec. Find symbols by name (repo-graph `find`, then grep).
engine/src/lib.rs and graph/src/lib.rs are FACADES — `mod` + `pub use` only, no logic.
  engine/src/  walk.rs route.rs extract.rs build.rs docs.rs passes.rs coverage.rs answers.rs arch.rs cache.rs
  graph/src/   types.rs build.rs imports.rs calls.rs merged.rs traversal.rs blast.rs activation.rs signal.rs test_support.rs
               resolvers/{mod,http,grpc,queue,graphql,websocket,eventbus,shared_schema,db,cron,config,iac,package,cli}.rs
  code-domain/src/  lib.rs (registries) + walk_gating.rs (the shared directory gate)
  stamp/       build identity (RELEASE + PARSER_STAMP)
To widen something across a module boundary use pub(crate), NEVER pub.

=== THE FROZEN key.json VOCABULARY ===
grade.py RAISES on any unknown top-level field. Allowed: framework, language, dirs, expect_nodes, expect_edges, expect_cells, forbid, mechanism, cells, note.
  forbid: [{kind,name?,max_nodes?} | {from,to,category}]. FORBID MATCHES EXACTLY — the normalised pattern must EQUAL a name or qname. Leniency in a precision gate manufactures false accusations (wave 2: `forbid {to: "UserController"}` matched the method `UserController::UserController::getUser` and accused a correct parser).
  expect_cells: [{kind,node,cell,contains?}] — use this instead of eyeballing --dump.
  mechanism / cells: VALIDATED against matrix_vocab.py; an unknown language or mechanism RAISES. The 30 columns are the vocabulary. You cannot invent one.
For new matrix probes use `python3 bench/substrate-gap/scaffold.py <lang> <mech>` and AUTHORING.md.

=== CONSTRAINTS ===
- API ADDITIVE: no pyo3/CLI method renamed or removed; new params default to None/off.
- LOCKED IDS: NodeKind/EdgeCategory/CellType ids are allocated in code-domain/src/lib.rs ONLY (its RESERVED blocks). node_kind 45-49, edge_category 33-34 and cell_type 17-18 were pre-allocated in wave 0 with their owning packets named. NEVER pick an id yourself.
- GRAPH CONTENT is correctable — removing wrong edges and fixing qname shapes is wanted — but DECLARE it and move every in-repo consumer (unit tests, fixture key.json) in the SAME commit.
- Parsers EXTRACT, the graph crate RESOLVES. No unwrap()/panic!() in non-test code. Scope in LOC, never time.
- Ship a grep-able fired_on marker: a literal stderr line with a stable prefix, e.g. `[queues] scan needle=...`. A bare mid-line token is not a marker; a passing test count is not a marker. If several packets share a line, carry a per-language/per-source discriminator.

=== ORDER OF WORK (fixture-first) ===
1. Verify your anchors by symbol name. If your spec is wrong, say so and work from reality.
2. Author fixtures and RECORD THEIR FAILING BASELINE FIRST — run grade.py on each and paste the output before writing the fix. A fixture that already passes proves nothing; if yours does, report it and fix the fixture, not the number.
3. Implement.
4. Run your gate. Paste verbatim output.

=== GATES ===
- `cargo test -p <crate>` for the crates you touch IS your gate. Do NOT use `cargo test --workspace` as your acceptance gate: it fails to COMPILE mid-wave on siblings' half-written crates, which is a false failure, not yours. The workspace gate runs once, at end of wave.
- cargo serialises on the target-dir lock; a build that blocks is NORMAL — wait, do not kill it.
- If you touch engine or graph: `cargo test -p repo-graph-engine --test byte_identical` must stay green.
- engram-export/ is excluded from the workspace but it is an in-repo consumer. If your packet renames a crate, or changes or moves a pub item it uses (generate_one, MergedGraph, RepoGraph / Node / Edge / Cell / CellPayload / RepoId literals, CodeNav::record, the node_kind / edge_category / cell_type consts, core::project_name), fix engram-export in the same commit and list its files in your files_touched. Build and test it ONLY with `bash scripts/check-engram-export.sh` — never `cargo build/check/test --manifest-path engram-export/Cargo.toml` in place, which rewrites the tracked engram-export/Cargo.lock. If your packet changes engram-export's dependency set (a crate rename, a new crate under engine or any crate it pulls in), run the script with --update-lock and commit engram-export/Cargo.lock. The end-of-wave gate runs the script.
- The INSTALLED wheel is current as of HEAD, so a baseline you record now is honest — but grade.py never sees the working tree, so YOUR Rust change is invisible to it until the end-of-wave rebuild. To see your change, use `GLIA_NO_PERSIST=1 cargo run -q -p glia-cli -- analyze <fixture> --format json`; otherwise report the cell unverifiable-this-wave. Do NOT fake it. Do not rebuild the wheel.
- Gate on a CAPTURED BEFORE-FILE AND A DIFF, never a literal count: `python3 bench/substrate-gap/run.py --no-log > /tmp/<pkt>_before.txt` first. Counts move when siblings land.

=== COMMIT PROTOCOL (concurrent agents) ===
When green: `git add <your paths by name>` then `git commit --only <your paths> -F /tmp/<YOUR-PACKET-ID>.commitmsg`.
The message path MUST contain your packet id and nothing else may write to it (wave 1: a shared path gave two commits a third packet's subject). `--only` is REQUIRED so a sibling's staged files cannot ride along. NEVER `git add -A` or `git add .`.
`.git/index.lock` contention is EXPECTED — on "Unable to create '.git/index.lock'", sleep 2-5s and retry up to ~10 times. Never delete the lock.
Subject `<type>(<scope>): <what>`, a body saying what changed and what the gate was, and this exact last line:
Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Do NOT push, rebase, reset, amend, or touch any commit that is not yours.

=== WHEN YOUR SPEC OR CORRECTION IS WRONG ===
It happens; catching it beats complying. In wave 3 a correction told a packet its prerequisite module already existed. It did not. The agent verified by searching, refused to create the missing module in a file outside its files_touched (a sibling had that file staged and would have lost half-written work), built a documented private stopgap with the removal path in its doc comment, and reported it — and two waves later the real packet deleted the stopgap in one mechanical swap. Do that: verify, report in `surprises`, work from what is there, never edit outside your files_touched.

=== HONESTY ===
If you cannot make the gate green, commit NOTHING and report the exact failing output. If your packet is unnecessary, already done, or duplicated by a sibling, say so and commit nothing — `not-needed` is a correct outcome. Report exact counts, never "mostly working". Contradictions go in `surprises`; anything a later packet must know goes in `followups`, naming that packet's id.
