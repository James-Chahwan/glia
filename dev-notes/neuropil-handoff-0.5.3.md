# neuropil handoff - glia 0.5.3, the ranking release (0.5.1 -> 0.5.3)

**neuropil skipped 0.5.2** (James, 2026-10-18: *Don't notify neuropil it's so far behind visual 3d stuff
takes much more free time not contracting for work but on the other not committed it can go to 0.5.3*),
so this handoff takes neuropil from glia 0.5.1 to 0.5.3. It carries `dev-notes/neuropil-handoff-0.5.2.md`
forward (section 3 says, section by section, what of it still applies and what 0.5.3 supersedes) and
adds 0.5.3's own changes. Read it in the neuropil session and act on neuropil only.

**Written at glia HEAD `5639697b` (the W15 close-out), before the bump.** The version bump, the
`v0.5.3` tag and the publish are James's release steps after these docs; every pin below is the pin to
take **once James tags `v0.5.3`**. At HEAD the workspace still reads 0.5.2.

The fourth glia -> neuropil handoff, after `neuropil-handoff-0.5.0.md`, `-0.5.1.md` and `-0.5.2.md`.
neuropil is glia's in-process Rust consumer: it path-depends on six glia crates and calls
`generate_one` at startup (`crates/neuropil-app/src/state.rs:443`) and in `bin/tier-audit.rs:21`, and
`activate` / `ActivationConfig` in `render/graph.rs:345`. 0.5.3 is the ranking release: 43 packets in
`dev-notes/leap-053-packets.json` (plan: `dev-notes/next-leap-0.5.3.md`), waves W0-W16 since `v0.5.2`
(`9837b1ff`). It ports measured rankings into glia's ranked answers (blast radius, diff-impact,
review, pack, a new `find --files`) and adds a per-repo calibration step; every ranking is query-time,
so **no 0.5.3 packet changes graph content**. Breaks were allowed when a packet declared them, and
consumers pin the release exactly.

Sources, in order of authority:
- a run of glia's compile check, `scripts/check-neuropil.sh`, against this glia tree at HEAD
  `5639697b`, on a copy of neuropil with the 0.5.0 handoff's section 2 edits applied, run 2026-10-24 by
  this packet (EZ.2). Section 1 has the output verbatim. Every compile claim comes from that run;
- each 0.5.3 packet's landed commit body (its `Declared breaks` paragraph). Where a body differs from
  its spec, the body wins, and where a spec note is wrong about neuropil this doc says so (section 5);
- the 0.5.2 handoff for everything 0.5.2 changed.

Every neuropil `file:line` was read read-only in neuropil's working tree: HEAD `b140d1e` with the same
50 dirty paths as at the 0.5.0, 0.5.1 and 0.5.2 handoffs (`git status --porcelain` 50 lines before and
after the check). Paths are relative to `crates/neuropil-app/src/` unless they start with `crates/` or
name a root file. Packet ids are glossed in words; the hash after an id is the glia commit that landed
it.

---

## 1. The compile check at this commit

A copy of neuropil with the 0.5.0 handoff's section 2 edits, checked with the unmodified script (the
0.5.2 handoff's recipe, unchanged). The copy and its target lived under `~/.cache/glia-EZ.2/npcheck/`
and were deleted afterwards; nothing under neuropil or glia was written.

```
C=~/.cache/glia-EZ.2/npcheck
cp -p ../neuropil/Cargo.toml ../neuropil/Cargo.lock ../neuropil/rust-toolchain.toml "$C/np/"
rsync -a --exclude=target/ ../neuropil/crates/ "$C/np/crates/"
python3 dev-notes/rename-0.5.0.py --apply --quiet --root "$C/np"     # non-git root: every file
sed -i '/^glia-projection-text = /s/"driver"/"research"/' "$C/np/Cargo.toml"
NEUROPIL_DIR="$C/np" GLIA_CONSUMER_CACHE="$C/cc" bash scripts/check-neuropil.sh   # cold target
```

```
[rename-0.5.0] rewrote 165 token(s) in 50 file(s) under /home/ivy/.cache/glia-EZ.2/npcheck/np (0 unknown token(s) left)
[neuropil-check] note - neuropil differs from the baseline's (b140d1ede58ff6e77e8e7326a6a4cf21e822959e dirty=50 src=b690b6b4f6db -> unknown src=73b04787b8a9): NEW/FIXED may be neuropil's own edits, not the leap's
[neuropil-check] ok - new=0 fixed=0 preexisting=0 in 124.3s against glia 0.5.2+pd640454f26e4ce00 (neuropil unknown, lock drift +40 -26)
```

- The rename count (165 tokens in 50 files) and the source hash `73b04787b8a9` are the earlier runs'
  own, so the copy is the same neuropil every check since 0.5.0 saw.
- `errors.txt` is empty: 0 errors across every neuropil target and the glia crates they pull in
  (neuropil's `rust-toolchain.toml` toolchain; cargo `Finished ... in 2m 03s`).
- The build stamp `0.5.2+pd640454f26e4ce00` is the W15 close-out's (the version field reads 0.5.2
  before the bump; the same stamp as the leap wheel built at that close-out).
- **Lock drift +40 / -26**, the same as at 0.5.2: the 26 `repo-graph-*` path packages become `glia-*`;
  glia-store, glia-parser-kotlin, glia-stamp, glia-doc and glia-rules enter; the registry crates are
  0.5.1's nine. **0.5.3 adds no crate** (its workspace `Cargo.lock` did not move).
- The `note` line is expected: the copy is not a git checkout.

`check-neuropil.sh` still copies neuropil verbatim, so run as-is it reports only the cargo resolution
error until neuropil renames; the recipe above stands in for it, as at 0.5.0-0.5.2.

## 2. Cargo: the pin

| where | 0.5.0 handoff asked | 0.5.3 adds (replaces 0.5.2's `=0.5.2`) |
|---|---|---|
| `Cargo.toml:112-117` | `repo-graph-<x> = { path = "../glia/<dir>" }` -> `glia-<x> = { path = ... }`; projection-text feature `driver` -> `research` | `version = "=0.5.3"` on all six: `glia-core`, `glia-code-domain`, `glia-graph`, `glia-engine`, `glia-activation`, `glia-projection-text` (e.g. `glia-engine = { path = "../glia/engine", version = "=0.5.3" }`) |
| `crates/neuropil-app/Cargo.toml:86-94` | `repo-graph-<x>.workspace = true` -> `glia-<x>.workspace = true` | nothing |
| `crates/**/*.rs` | 165 `repo_graph_*` path tokens -> `glia_*` (0.5.0 handoff section 2) | nothing |

All six crates read `version.workspace = true`. A `version` on a path dependency must match the
crate's own version, so check out glia at the `v0.5.3` tag before adding it: at HEAD `5639697b` (and
any commit before the release commit) the crates read 0.5.2, and `=0.5.3` fails resolution there.

## 3. The 0.5.2 handoff, carried forward

`dev-notes/neuropil-handoff-0.5.2.md` was never applied (neuropil skipped 0.5.2). Apply it with this
one; its sections stand as follows.

| 0.5.2 handoff section | at 0.5.3 |
|---|---|
| 0 TL;DR | stands, with two edits: the pin is `=0.5.3` (section 2 here), and the compile check is section 1 here. Items 1 (apply the 0.5.0 handoff's sections 2-3 first), 4 (re-keys), 5 (SAME_AS displays `?`), 6 (optional primitives) and 7 (reinstall the `glia` binary, now from `v0.5.3`) stand |
| 1 The compile check | **superseded** by section 1 here (`new=0` against `5639697b`) |
| 2 Cargo: the pin | **superseded** by section 2 here (`=0.5.2` -> `=0.5.3`) |
| 3 What re-keys | **stands**: 0.5.3 moves no qname and no kind, so the 0.5.2 re-keys (Go constant-prefixed groups, requirements.txt / Pipfile and sbt channel qnames, `${...}` client ENDPOINTs, templated OpenAPI ops, Akka / Vapor ROUTEs, Dart GraphQL ops, Elixir behaviours, named `queue_*` tags) are the only re-keys between 0.5.1 and 0.5.3, together with the 0.5.1 handoff's section 3 |
| 4 Engine API changes | **stands** (0 hits in `crates/` then and now); 0.5.3's are in section 4 here |
| 5 Behaviour changes | **stands**: 5.1 rendering (SAME_AS has no arm in `hud.rs:1516-1550` `edge_category_name`; the SHARES_DEPENDENCY arm at `hud.rs:1548` cannot fire), 5.2 edges, 5.3 the build. 0.5.3 adds none to the graph (section 6) |
| 6 Corrections | stands |
| 7 Optional primitives | **stands, extended**: `WatchSession` gains `rank_cache()` and the ranked answers gain a cache input (section 7 here) |
| 8 Re-test | stands, run once for both releases (section 8 here) |
| 9 Done vs pending | superseded by section 9 here |
| 10 The tweaks (DT.1a-DT.13), 11 the close-out (DT.5h-DT.5l) | **stand**: they are part of the released 0.5.2 and so of 0.5.3; their re-keys (10.2) and behaviour notes apply. Their compile checks (10.1, 11.1) are superseded by section 1 here |

## 4. 0.5.3 engine API changes, against neuropil

Every row is on an item neuropil never names: a grep of neuropil's `crates/` (`*.rs`) finds 0 hits for
each of `BlastOptions`, `BlastRadius`, `PackArgs`, `FindFilesOptions`, `ReviewArgs`, `Answer<`,
`GliaConfig`, `glia_config`, `SECTIONS`, `section_counts`, `IdsConfig`, `HistoryOptions`,
`HistorySummary`, `history_sync`, `HISTORY_VERSION`, `WatchSession`, `RankCache`, `rank_cache`,
`Ranking`, `CalibrateOptions`, `PprIndex`, `activate_indexed`, `ActivationPlan`, `ppr_vector`,
`blast_radius_over`, `closure_config`. neuropil's glia imports are `generate_one`, `MergedGraph`
(`::default`), `activate`, `ActivationConfig` and `NodeId` / `NodeKindId`, none of which moved.

| # | packet (commit) | crate | change | neuropil |
|---|---|---|---|---|
| 1 | EB.4 `[ids]` vocabulary (`44380ddc`) | glia-code-domain | `glia_config::SECTIONS` `[&str; 12]` -> `[&str; 13]` (`"ids"` last), `section_counts()` -> `[(&str, usize); 13]`; **`GliaConfig` gains `ids: IdsConfig`** (not `#[non_exhaustive]`: a struct literal outside code-domain breaks); new items `IdsConfig`, `TrackerDecl`, `IdPattern`, `IdMatch`, `PathGlobs`, `MAX_ID_PATTERNS`, `MAX_ID_PATTERN_CHARS`, `MAX_SHORTHAND_LETTERS`; the `[overlay] loaded ...` stderr line gains `ids=<n>` after `layer=<n>`; a `.glia/overlay.toml` holding `[ids]` now loads (0.5.1 / 0.5.2 ignored such a file whole) | 0 hits; no change |
| 2 | EB.1 history messages (`3fbe3e8f`) | glia-code-domain (`snapshots` module), glia-snapshots | `HISTORY_VERSION` 1 -> 2, `HISTORY_VERSIONS = [1, 2]`; new `HistoryMessage`, `HISTORY_MESSAGES_FILE`, `MESSAGE_BODY_CAP`, `SUBJECT_CAP`, `MESSAGE_TRAILER_PREFIXES`, `write_history_with_messages`, `read_history_messages`, `clean_message`, `clean_message_counted`; `HistoryOptions` gains `pub messages: bool` (default true; not `#[non_exhaustive]`), `HistorySummary` gains `messages`, `redacted`, `trailers_dropped`, `branch_labels` | `HistoryOptions` lives in **glia-snapshots, which neuropil does not depend on** (section 5); 0 hits |
| 3 | EA.5 ranked blast radius (`fe4f23b8`), EC.8 its selection (`096d114b`) | glia-engine, glia-graph | `BlastOptions` gains `ranking` (default H10), `rank_cache` (None), `top_files` (10), `select` (true); `BlastRadius` gains `files` (after `results`) and `selection`; new `glia_graph::blast_radius_over_with` and `closure_config`. All `#[non_exhaustive]`: no literal breaks | 0 hits |
| 4 | EA.6 / EA.6b pack ranking (`d05b956c`, `c6a2179f`), EC.9 (`6b3bfd59`) | glia-engine | `PackArgs` gains `ranking`, `rank_cache`, `select`; `Pack` gains `selection`; with a history snapshot pack lifts each co-changed file's best row (ruling EA-2b) | 0 hits |
| 5 | EA.7 the held cache (`32922d48`) | glia-engine | **`WatchSession::rank_cache() -> Arc<RankCache>`** (new): the cache of the current build, a new `Arc` after every update that changed the graph, the same one after a noop | optional, section 7 |
| 6 | EB.7 review --title (`378e79fb`), EC.11 (`182de4f1`), EC.12 (`6ae02bab`) | glia-engine | `ReviewArgs.title`, `Review.intent` / `selection_intent`, `ReviewIntent` (its `fusion` `&'static str` -> `String` in EC.11), `IntentFile`, `INTENT_FUSION_REVIEW`; all `#[non_exhaustive]` | 0 hits |
| 7 | EB.8 find --files (`8f9074c2`), EC.13 (`8e5bba6c`) | glia-engine | `find::{find_files, find_files_with_ids, FindFilesOptions (+ select), FoundFile, FIND_FILES_FUSION}`; `Answer<T>` gains `selection: Option<Selection>` (`#[non_exhaustive]`; never serialized for other answers) | 0 hits |
| 8 | EA.1 `PprIndex` (`4e0c74ad`) | glia-activation | new `PprIndex`, `activate_indexed`, `ActivationPlan::run_indexed` / `rank_indexed`; `ppr_vector` is now `PprIndex::build(..).vector(..)`, bit-identical (oracle tests); the `GLIA_ACTIVATION_DEBUG=1` line gains a trailing ` index=prebuilt\|built\|none`; an index whose config mismatches prints `[activation] index mismatch mode=<m>` | `activate` (`render/graph.rs:345`) is unchanged and its scores are bit-identical |
| 9 | EB.2 `algo::fuse` (`04315cc3`) | glia-activation | new module: `minmax`, `blend`, `mean`, `lex_order`, `order`, `lx` | additive |
| 10 | EA.2-EA.4, EB.3, EB.5a-c, EB.6, EC.1-EC.5, EC.10, EC.14 | glia-engine | new public slots by module path: `rank::{Ranking, RankBase, RankCache, FileIndex, CochangeIndex, rank_files, order_rows, ..}`, `intent::{text, ids, defs, paths, idr}`, `calibrate::{record, replay, select, bank, sets, step, work, fusions, ..}`; `git_rev::Tree` (re-exported as `intent::defs::Tree`); `CalibrateOptions` gains `vocab: Option<IdVocab>` (EC.14, additive, `Default`) | additive |
| 11 | EG.7 (`b252607e`) | glia-engine | `review::intent_files_with`, `find::find_files_with`, `review::IntentHistory`: `#[doc(hidden)]` seams for the rank gate, **unstable**: never call them | none |

## 5. Corrections to the packets' neuropil notes

- **EZ.2's own spec** said neuropil should check `HistoryOptions` (a struct literal breaks). It cannot:
  `HistoryOptions` is in `glia-snapshots` (`snapshots/src/git_history.rs`), and none of neuropil's six
  glia crates depends on it (glia-engine reads the snapshot through `glia_code_domain::snapshots`; only
  the CLI and pyo3 crates depend on glia-snapshots). The literal-breaking struct neuropil could reach is
  `GliaConfig` (row 1), with 0 hits.
- **EA.7's note** ("WatchSession users get rank_cache(); no call changes") is right; neuropil holds no
  `WatchSession` yet (the 0.5.2 handoff's section 7 offers it).

## 6. Behaviour changes with no required code change

- **Graph content: none.** No 0.5.3 packet changes what a build extracts or resolves: the substrate
  bench stayed at 419 fixtures, matrix 346/482, blind 0 at every wave close-out W0-W15, and the
  engine's `byte_identical` test stayed green. `generate_one`'s `GenerateResult` for a tree is the
  0.5.2 one.
- **PARSER_STAMP moved** (every packet that edited an engine, activation, code-domain or snapshots
  source), so the first build after the upgrade reparses, and a `.glia/graph/` layout written by
  another glia build rebuilds once.
- **`activate` scores are bit-identical** (EA.1's oracle tests); the release's "PPR d0.85 with sum
  pooling" is one ranking (PT*) of the ranked answers, never the `ActivationConfig` default (still
  damping 0.5).
- **`.glia/` gains two per-user snapshot dirs** when someone runs the new steps on a repo neuropil
  shows: `.glia/history-snapshot/` moves to version 2 (`messages.jsonl`, EB.1) on the next `glia
  history sync`, and `glia history calibrate` writes `.glia/calibration-snapshot/` (EC.1, EC.6). The
  build reads the history snapshot as before (its `data_hash` covers `commits.jsonl` and
  `blame.jsonl` only); it never reads the calibration record. A neuropil file watcher that reloads
  on `.glia/` writes sees one more write after a calibration.
- **stderr:** ranked answers print `[rank] ..`, `[rank-cache] ..`, `[rank-files] ..`,
  `[rank-cochange] ..`, `[select] ..`; neuropil calls none of them, and `generate_one` prints no new
  line.

## 7. Optional: new in-process primitives

All by module path (glia's facade rule). The 0.5.2 handoff's section 7 primitives stand
(`WatchSession`, `generate_one_with_cache`, `memory::release_freed_memory`, `identity`, `rules`).

| primitive | fits in neuropil | packet (commit) |
|---|---|---|
| `glia_engine::watch::WatchSession::rank_cache()` -> `Arc<RankCache>`, handed to `BlastOptions` / `PackArgs` (`rank_cache: Some(..)`) | a session-held graph answers blast radius / pack with its PPR, file and co-change indexes built once per update instead of per call: EA.7 measured 561.7 ms for the first `blast_radius` vs 202.5 ms median after on a 1231-commit glia clone (2.8x), all answers equal. With a history snapshot under the repo the order is co-change first | EA.7 (`32922d48`) |
| `glia_engine::rank::RankCache::new(holder, &repo_roots, &repo_labels)` over a `GenerateResult`'s repo roots and labels, in an `Arc` | the same cache without a session; a cache with no roots reads no co-change (`files` rows label repos `repo#<n>`) | EA.2, EA.3 (`ceae5771`, `42215a67`) |
| `glia_engine::find::find_files(&merged, &repo_roots, query, &opts)` -> `Result<Answer<FoundFile>, String>` (`opts` from `FindFilesOptions::default()`, its `rank_cache` set to the held cache) | a search box that answers "which files is this about" for a ticket id or an issue title (typed id routing, then BM25 over past commit messages) | EB.8, EC.13 (`8f9074c2`, `8e5bba6c`) |
| `glia_engine::calibrate::step::calibrate(root, &merged, &CalibrateOptions::default())` -> `Result<CalibrationSummary, String>` | the per-repo calibration `glia history calibrate` runs: ranked answers given the cache then use the repo's measured pick. Minutes on a large history (glia: 133.9 s, peak RSS 717 MB, measured by EZ.2 with the W15 wheel) | EC.5, EC.6 (`87681b1d`, `d64c2d0b`) |

A cache reads the calibration record once (the first ranked answer); a session's new cache after an
update reads it again. Signatures as at HEAD `5639697b`; read them again at the tag (`engine/src/rank/cache.rs`,
`engine/src/find.rs`, `engine/src/calibrate/step.rs`). `FindFilesOptions` and `CalibrateOptions` are
`#[non_exhaustive]` with `Default`: set fields on `default()`.

## 8. Re-test (neuropil's G24 protocol)

Run in neuropil after the 0.5.0 handoff's sections 2-3 and the pin, once for 0.5.2 and 0.5.3 together:

```
cargo check --workspace --all-targets && cargo test --workspace
cargo run -p neuropil-app -- .                              # boot: [parallel] and [timing] lines on stderr
cargo run -p neuropil-app --features test-harness -- .      # in a second shell, then:
python3 tools/test_smoke.py
python3 tools/visual_regression.py compare
```

- `visual_regression compare` drifts as the 0.5.2 handoff's section 8 says (0.5.2's new nodes and
  edges); 0.5.3 adds no drift of its own (no graph change).
- Hidden nodes, bookmarks and anchored annotations on the 0.5.1 and 0.5.2 re-key families need
  re-setting once; 0.5.3 adds none.
- From glia, the section 1 recipe should then print `new=0`, and once neuropil has renamed,
  `bash scripts/check-neuropil.sh` as-is should too, with the `note` that neuropil moved from the
  baseline's sha.

## 9. Done vs pending

- **Done on the glia side:** 0.5.2 (released `v0.5.2`, 2026-10-18) and 0.5.3's W0-W15, each close-out
  green (`cargo test --workspace` 6556 passing at W15). This packet ran section 1: neuropil compiles
  against the 0.5.3 tree once the 0.5.0 section 2 edits are in.
- **Pending, James:** the 0.5.2 -> 0.5.3 bump, the `v0.5.3` tag. Then `cargo install --path cli
  --locked` from the tag, so the `glia` on PATH matches.
- **Pending, neuropil session:** the 0.5.0 handoff sections 2, 3 and 6; the 0.5.2 handoff as section 3
  here lists; this doc's section 2 pin and section 8.
- **Pending, glia (not owned by a packet):** `check-neuropil.sh` still copies neuropil verbatim (the
  0.5.0 handoff's open item).
- **Followups the packets named for neuropil:** EA.7 (`WatchSession::rank_cache()`, section 7) and
  EB.1 (`HistoryOptions.messages`; section 5 explains why it does not reach neuropil). No other
  0.5.3 packet's landed breaks name neuropil.
- **Checked, no neuropil change:** E0.1 / E0.2 (module slots); EA.9, EB.9, EG.1-EG.3, EG.6, EG.7
  (the rank_eval harness and the rank gate); EG.4 (engram-export only); EG.5 (wheels CI); EB.2, EA.1
  beyond row 8 (additive activation API). The 0.5.3 release record (the rank gate, the CI pinning) is
  section 9 of `dev-notes/repo-graph-handoff-0.5.3.md`.
