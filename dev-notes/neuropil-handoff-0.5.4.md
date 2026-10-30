# neuropil handoff - glia 0.5.4 (0.5.1 -> 0.5.4)

**Span (ruling HANDOFF-SPAN, James 2026-10-24): 0.5.1 -> 0.5.4.** neuropil skipped 0.5.2 and never applied
`dev-notes/neuropil-handoff-0.5.3.md` (which carried 0.5.2 forward), so this handoff takes neuropil from
glia 0.5.1 to 0.5.4: it carries the 0.5.3 handoff forward (section 3) and adds 0.5.4's changes. Read it in
the neuropil session and act on neuropil only.

**Written at glia HEAD `9bd4d3f0` (after the W26 close-out), before the bump.** The version bump, the
`v0.5.4` tag and the publish are James's release steps after these docs; every pin below is the pin to take
**once James tags `v0.5.4`**. At HEAD the workspace reads 0.5.3.

The fifth glia -> neuropil handoff, after `neuropil-handoff-0.5.0.md`, `-0.5.1.md`, `-0.5.2.md` and
`-0.5.3.md`. neuropil path-depends on six glia crates and calls `generate_one` at startup
(`crates/neuropil-app/src/state.rs:443`) and in `bin/tier-audit.rs:21`, and `activate` /
`ActivationConfig` in `render/graph.rs:345`. 0.5.4 shipped tracks T0, T1, T2, T3, T5, TS, T10, T11, TH and
TZ: 121 landed packets in waves W0-W26 of `dev-notes/leap-054-packets.json` since `v0.5.3` (`929d99ee`):
the precision group (phantom nodes and edges removed in every language), one tier vocabulary,
embeddings as a sidecar, compact answers, org-internal package routing, boundary health. **Not shipped
(ruling OPTION-3, James 2026-10-30):** T4, T6 (the goal layer), T7, T8 (incremental resolution,
multi-repo watch), T9 (build-target graphs). The public slots `glia_engine::goal`, `glia_engine::clarify`,
`glia_engine::targets` and `glia_code_domain::goal` ship **empty, reserved for 0.5.5**: nothing to adopt.

Sources, in order of authority: section 1's compile check (every compile claim comes from it); each 0.5.4
packet's landed result (its `breaking` text) and commit body; the routed paragraphs in
`dev-notes/leap-054-corrections.json`; the 0.5.3 handoff for everything 0.5.2 and 0.5.3 changed. Every
neuropil `file:line` was read read-only in neuropil's working tree: HEAD `b140d1e` with the same 50 dirty
paths as at every handoff since 0.5.0 (`git status --porcelain` 50 lines before and after the check).
Paths are relative to `crates/neuropil-app/src/` unless they start with `crates/` or name a root file.
Packet ids are glossed in words; the hash after an id is the glia commit that landed it.

---

## 1. The compile check at this commit

`CARGO_BUILD_JOBS=8 bash scripts/check-neuropil.sh`, run as-is against `../neuropil` and this glia tree
(HEAD `9bd4d3f0`) by this packet (FZ.2) on 2026-10-30. The script copies neuropil into
`~/.cache/glia-consumer-check/neuropil/`, applies the 0.5.0 rename and the `driver` -> `research` feature edit
to the copy when neuropil still holds `repo-graph-*` names (the 0.5.3 handoff's recipe, now in the script),
and never writes under neuropil. Its output, verbatim:

```
Cargo.toml: 6
crates/neuropil-app/Cargo.toml: 6
crates/neuropil-app/src/annotations.rs: 1
crates/neuropil-app/src/app_runner.rs: 2
crates/neuropil-app/src/bin/tier-audit.rs: 7
crates/neuropil-app/src/cluster_naming.rs: 1
crates/neuropil-app/src/code_viewer.rs: 18
crates/neuropil-app/src/command_palette.rs: 1
crates/neuropil-app/src/context_menu.rs: 3
crates/neuropil-app/src/domain_zone.rs: 2
crates/neuropil-app/src/entry_tools_card.rs: 2
crates/neuropil-app/src/eva_chrome.rs: 1
crates/neuropil-app/src/external_tool.rs: 4
crates/neuropil-app/src/file_tree.rs: 7
crates/neuropil-app/src/flocking_layout.rs: 2
crates/neuropil-app/src/flow.rs: 2
crates/neuropil-app/src/flow_replay.rs: 2
crates/neuropil-app/src/flow_source.rs: 1
crates/neuropil-app/src/flow_step.rs: 1
crates/neuropil-app/src/graph_search.rs: 1
crates/neuropil-app/src/heartbeat_pill.rs: 1
crates/neuropil-app/src/hud.rs: 6
crates/neuropil-app/src/inject.rs: 3
crates/neuropil-app/src/inject_panel.rs: 2
crates/neuropil-app/src/input.rs: 1
crates/neuropil-app/src/layout/mod.rs: 1
crates/neuropil-app/src/layout/tier_stack.rs: 6
crates/neuropil-app/src/live_node.rs: 5
crates/neuropil-app/src/monitor_flow.rs: 1
crates/neuropil-app/src/openapi_catalog.rs: 1
crates/neuropil-app/src/pill_card.rs: 3
crates/neuropil-app/src/polish_burst.rs: 2
crates/neuropil-app/src/position.rs: 3
crates/neuropil-app/src/render/badges.rs: 1
crates/neuropil-app/src/render/clusters.rs: 1
crates/neuropil-app/src/render/edge_bundle.rs: 1
crates/neuropil-app/src/render/edges.rs: 3
crates/neuropil-app/src/render/flow_edges.rs: 2
crates/neuropil-app/src/render/flow_nodes.rs: 1
crates/neuropil-app/src/render/graph.rs: 9
crates/neuropil-app/src/render/node_material.rs: 1
crates/neuropil-app/src/rubber_band.rs: 2
crates/neuropil-app/src/saved_requests.rs: 10
crates/neuropil-app/src/service_hex.rs: 3
crates/neuropil-app/src/silos.rs: 2
crates/neuropil-app/src/state.rs: 7
crates/neuropil-app/src/test_harness/methods.rs: 14
crates/neuropil-app/src/tier_editor.rs: 1
crates/neuropil-app/src/type_drag.rs: 1
crates/neuropil-app/src/view_state.rs: 1
[rename-0.5.0] rewrote 165 token(s) in 50 file(s) under /home/ivy/.cache/glia-consumer-check/neuropil/src (0 unknown token(s) left)
[neuropil-check] note - neuropil still uses the pre-0.5.0 names; checked a renamed copy
[neuropil-check] ok - new=0 fixed=0 preexisting=0 in 192.6s against glia 0.5.3+p53276b0bd7c90122 (neuropil b140d1e, lock drift +43 -26)
```

- **`new=0`**: neuropil compiles against the 0.5.4 tree once the 0.5.0 handoff's section 2 edits are in
  (`errors.txt` empty, 0 errors across every neuropil target and the glia crates they pull in).
- The rename count (165 tokens in 50 files) is every earlier run's own, so the copy is the same neuropil
  every check since 0.5.0 saw.
- The build stamp `0.5.3+p53276b0bd7c90122` is this tree's (the version field reads 0.5.3 before the bump).
- **Lock drift +43 / -26** (0.5.3: +40 / -26). The three new entries are glia path crates glia-engine now
  depends on: `glia-embed`, `glia-build-domain`, `glia-parser-bazel` (F0.2 `50af6405`). **No fastembed, ort
  or tokenizers entry enters neuropil's lock**: glia-engine reaches the model only through its `sem-eval`
  feature, which neuropil does not enable. glia's own workspace lock gained about 200 entries (F0.2).
- The `note` line is expected until neuropil renames; then the rename step finds nothing and is skipped.

## 2. Cargo: the pin

| where | 0.5.0 handoff asked | 0.5.4 adds (replaces the 0.5.3 handoff's `=0.5.3`) |
|---|---|---|
| `Cargo.toml:112-117` | `repo-graph-<x> = { path = "../glia/<dir>" }` -> `glia-<x> = { path = ... }`; projection-text feature `driver` -> `research` | `version = "=0.5.4"` on all six: `glia-core`, `glia-code-domain`, `glia-graph`, `glia-engine`, `glia-activation`, `glia-projection-text` |
| `crates/neuropil-app/Cargo.toml:86-94` | `repo-graph-<x>.workspace = true` -> `glia-<x>.workspace = true` | nothing |
| `crates/**/*.rs` | 165 `repo_graph_*` path tokens -> `glia_*` | nothing |

A `version` on a path dependency must match the crate's own, so check glia out at the `v0.5.4` tag before
adding it: at `9bd4d3f0` the crates read 0.5.3 and `=0.5.4` fails resolution.

**Stamps.** PARSER_STAMP / BUILD_STAMP moved at every packet that edited a hashed source and once more by
design (F0.6 `fed4d59b`, ruling STAMP-LOCK: the stamp counts the hashed crates' dependency closure, so one
commit gives one stamp in every checkout). The first `generate_one` after the upgrade reparses, and a
`.glia/graph/` layout written by another glia build rebuilds once.

## 3. The 0.5.3 handoff, carried forward

| 0.5.3 handoff section | at 0.5.4 |
|---|---|
| 1 The compile check | superseded by section 1 here |
| 2 The pin | superseded by section 2 here (`=0.5.4`) |
| 3 The 0.5.2 handoff, carried forward | **stands**: apply `dev-notes/neuropil-handoff-0.5.2.md` as that table says (its re-keys, SAME_AS rendering, the DT tweaks) |
| 4 0.5.3 engine API changes | stands (all 0 hits in `crates/`) |
| 5 Corrections | stands |
| 6 Behaviour changes | stands, except "Graph content: none": 0.5.4 changes content (sections 5-6 here) |
| 7 Optional primitives (`WatchSession::rank_cache()`, `RankCache`, `find_files`, `calibrate`) | stands; `FindFilesOptions` gains `semantic` (FB.7a) and `CalibrateOptions` gains `semantic` (FB.7d), both `None` by default |
| 8 Re-test | stands, run once for 0.5.2-0.5.4 together (section 8 here) |
| 9 Done vs pending | superseded by section 9 here |

## 4. 0.5.4 engine API changes, against neuropil

Every row is on an item neuropil never names: a grep of neuropil's `crates/` (`*.rs`) finds 0 hits for
`IdentityTier`, `profile::Tier`, `value::Tier`, `why::`, `edge_tier`, `GliaConfig`, `SECTIONS`, `CATEGORIES`,
`FindOptions`, `BlastOptions`, `DEFAULT_ANSWER_ROWS`, `tie_cmp`, `pick_primary`, `IdentityView`,
`walk_members`, `ResolvePool`, `build_go_with`, `ImportBinding`, `VectorRow`, `FastEmbedder`, `WriteThrough`,
`CellTarget`, `extract_cli_invocation_nodes`, `extract_data_entity`, `scan_migration_dsl`,
`extract_yaml_env_defs`. (neuropil's own `Tier` / `TIER_HEIGHT` in `layout/tier_stack.rs` are its layout
tiers, not glia's.) neuropil's glia imports are `generate_one`, `MergedGraph`, `activate`,
`ActivationConfig`, `NodeId` / `NodeKindId` and the three registries, none of which moved.

| # | packet (commit) | crate | change | neuropil |
|---|---|---|---|---|
| 1 | F0.1 registry ids (`a27aa00e`) | glia-code-domain | `node_kind::GOAL_UNIT` = 50, `edge_category::TRACES` = 38, `cell_type::UNIT` = 26, `cell_type::IMPORT_BINDINGS` = 27. No build emits GOAL_UNIT / TRACES / UNIT (their track did not ship); IMPORT_BINDINGS is on every MODULE with an external import (FA.21) | the HUD tables (section 7) |
| 2 | FD.7a one Tier (`8a0405dc`) | glia-activation, glia-rules, glia-engine | `profile::IdentityTier` renamed `Tier` (gains `Default` = Fact, `ALL`, `as_str` / `upper` / `level`, `parse`, `weakest`, `best`, `Display`); `glia_rules::value::Tier` is that type (`name()` -> `as_str()`); `why::edge_tier` returns `(Tier, Option<String>)` | 0 hits |
| 3 | FD.7c tier consts folded (`278d8734`) | glia-engine | removed the per-module tier consts of tests_for, gaps, flags (TIER_FACT / TIER_DERIVED / TIER_HEURISTIC), hubs, cycles, communities, splits, duplicate_flows and hotspots (the list is in the commit body): use `Tier::Fact.as_str()` | 0 hits |
| 4 | FB.12a `[[convention]]` (`abeef44d`) | glia-code-domain | `glia_config::SECTIONS` `[&str; 13]` -> `[&str; 14]` (`"convention"` last), `section_counts()` 14 pairs; **`GliaConfig` gains `convention`** (a struct literal outside code-domain breaks); a CONV entry with a `kind` other than `"convention"` is refused | 0 hits |
| 5 | FB.13c (`080dfb0d`) | glia-engine | `gaps::CATEGORIES` `[&str; 13]` -> `[&str; 14]` (`named_not_modelled`); `FindOptions.vocab_roots`; `Absence.named_in` | 0 hits |
| 6 | FH.8 / FH.8b / FH.8c compact answers (`f92a1ad1`, `0f6ce15d`, `ad730731`) | glia-engine | `BlastOptions`' `top_k` cuts seeds too; new `BlastOptions` field `offset`, `BlastRadius.{truncated, areas, blind}`, `answer_json`, `DEFAULT_OVERVIEW_ROWS` (25), `DEFAULT_ANSWER_SEEDS` (25); **`DEFAULT_ANSWER_ROWS` removed**. `BlastOptions::default()` still cuts nothing | 0 hits |
| 7 | FD.22 checkout-independent ties (`e43ea02a`) | glia-graph | new `MergedGraph::tie_cmp` and the method `pick_primary_tied`; ties follow (qname, kind, graph index), never NodeId | additive |
| 8 | FA.21 / FA.22 / FA.23 (`bff59e64`, `31385a5d`, `3df0ee91`) | glia-code-domain, glia-graph | `ImportBinding`, `read_import_bindings`, `import_default_binding`; `OrgPackageResolver` (`[passes] domain=code resolve=18 post=9 finalize=2`, was 17) | additive |
| 9 | FD.81 (`ebf41c7b`), FH.21 (`4225bfba`) | glia-graph | `glia_graph::build_go_with(repo, parses, go_module_dirs)` (`build_go` unchanged); `ResolvePool`, `with_resolve_pool` | additive |
| 10 | FD.28 (`dd84ce64`) | glia-engine | `IdentityView::walk_members`, `in_walk_class` | additive |
| 11 | FB.4 / FB.5a / FB.7a (`c98fffcf`, `9bd0430d`, `77141f1a`) | glia-engine, glia-store | `semantic::{EmbedRecipe, DEFAULT_RECIPE (t2), VectorIndex, SemanticSource, embed_repo, EmbedOptions, EmbedReport, ..}`; `glia_store::{write_vector_rows, VectorWriteMode}`; `code_domain::profile::EMBED_KINDS` | additive |
| 12 | FB.3 vectors are query-time (`94106c1f`) | glia-engine, glia-store | a build no longer reads `.glia/vectors.jsonl` into VECTOR cells; `WriteThrough::SidecarOnly` | `generate_one` returns no VECTOR cell |
| 13 | FD.4a / FD.4b / FD.76 (`784193cf`, `056f6843`, `834bee60`) | glia-code-extractors (not a neuropil dependency) | `extract_cli_invocation_nodes`, `extract_data_entity_nodes` / `extract_data_entity_access`, `scan_migration_dsl` and `config::extract_yaml_env_defs` gain a `path` argument | none |
| 14 | FH.2a health (`f092a5ce`) | glia-engine | new slot `glia_engine::health` (`health_with`, `boundary_health`, `Health`, `MechanismHealth`, ..) | optional (section 8) |
| 15 | F0.3 slots (`ec42f78e`) | glia-engine, glia-code-domain | `goal`, `clarify`, `targets`, `semantic` public modules; the first three are empty (reserved for 0.5.5) | none |

## 5. What re-keys

0.5.4 removes and re-keys nodes. Persisted view state keyed by qname (hidden nodes, bookmarks, anchored
annotations) orphans for a removed node and must be re-set once for a re-keyed one. Every packet whose
landed break is `node_id` or `qname_shape`:

| packet (commit) | node ids gone or new | qnames that change |
|---|---|---|
| FD.4a self-graph hygiene, cron / CLI (`784193cf`) | CRON_JOB from k8s CronJob text outside yaml / yml / json and from a non-schedule Celery `"schedule":`; CLI_INVOCATION from Rust / Python literals and comments | none |
| FD.4b data entities in .rs / .py (`056f6843`) | DATA_ENTITY from a SQL / Cypher literal not passed to a call (const / let / field / list), nested literals, comments, migration-DSL needles in literals | none |
| FD.4c Rust routes (`9888e2b8`) | ROUTEs read from literals, raw strings, char literals or comments | none |
| FD.4d WebSocket needles (`53cc6574`) | `ws_client:ws` from a bare `io(` in a file binding its own `io` (glia's own `ws_client:ws @engine`); a non-Ratchet `->route(` WS_HANDLER | Ratchet double-quoted paths `ws:/ws/$x` -> `ws:/ws/{x}` |
| FD.5a guard: Ruby, PHP, Dart, Elixir (`bb9e08d7`) | nodes read from comments and non-interpolated strings | none |
| FD.5b guard: compiled and JS-family languages (`76c74d49`) | the same for Go, TS / JS, Java, Kotlin, Scala, C#, Swift, C / C++; `clap::Command::new` is no CLI_INVOCATION | none |
| FD.31 manifests (`0f599ac3`) | **new**: `package:maven:<g>:<a>`, `package:pub:<n>`, `package:hex:<n>`, `package:nuget:<n>`; synthetic MODULEs `<dir>::pom.xml`, `<dir>::build.gradle(.kts)`, `<dir>::<Name>.csproj`; nothing removed | none |
| FD.76 yaml env (`834bee60`) | CONFIG_KEY `config:env:<k>` defined only by a pubspec / GitHub Actions / GitLab CI `environment:` block | none |
| FD.77 TOML array tables (`6542bb7a`) | PACKAGE_DEP `package:cargo:name` / `path` / `harness` .., `package:pypi:name` / `url` / `priority` from array-of-tables keys | none |
| FD.78 directory symlinks (`43cb576d`) | every node under an in-repo directory link, a second link to an outside directory or a link cycle | a file under a link is now read once, at its target's path (e.g. `web::<x>::..` mirrors go) |

Beside them, FH.3b (`c7236c39`) re-keys a NestJS `@nestjs/swagger` op under a symbolic controller or verb
prefix to the folded path (its DOC_SECTION qname and NodeId move), and FH.6 (`312d1e00`) moves Vue POSITION
rows to the file's real lines and adds `.svelte` / `.astro` nodes.

## 6. Graph content: edges

Edges only (nodes and qnames unchanged), no code change: FD.6 (`9298185e`, Rust / Python test-item
statement literals, string-embedded GraphQL literals, Rust data-source needles that are not paths); FD.73
(`98e0dfd8`, JNI descriptor and longer-word data-source edges); FD.74 (`e00d1263`, file-write / `query_gql` /
Rust tests-dir statement literals); FD.75 (`e7ddaee7`, env writes and deletes, clientless Vault `.read(`);
FD.79 (`d22aeaad`, doclink: lone-word, registry-name and placeholder mentions and out-of-tree test
candidates; glia's own tree 830 -> 344 DOCUMENTS edges); FD.80 (`836d5170`, gRPC HANDLED_BY to bootstraps
gone, rule `register_impl` added); FD.81 (`ebf41c7b`, Go implicit IMPLEMENTS across go.mod modules); FH.14b
(`f046fceb`), FH.15 (`362018a5`); additions from FA.22 / FA.23 (cross-repo IMPORTS / CALLS in merged layouts,
rules `org_import` / `org_call` / `org_ref`), FH.4a / FH.4b / FH.17 (`0a4bc52c`, `6dee2449`, `f3829dba`:
endpoints read from a call's shape and inferred HTTP wrappers; an ENDPOINT_HIT may end `"test_site":true`),
FH.5, FH.16, FH.19, FH.20 (TS workspace imports, bundler aliases, NestJS handlers, type aliases as
INTERFACE), FH.23 (`abi_partial` EVM pairs) and FD.65 (`bf325da4`, DECISION cells on sub-scoped decisions).
The full per-language table is section 4 of `dev-notes/repo-graph-handoff-0.5.4.md`.

`visual_regression compare` drifts for all of these on a repo that has them; on neuropil's own tree expect
fewer DOCUMENTS edges (FD.79) and fewer data-entity / CLI phantoms from Rust literals (FD.4a, FD.4b, FD.6).

## 7. The HUD arms

`hud.rs:1552` `cell_label` names cell types 1-14 and falls back to `"CELL"`; `hud.rs:1516`
`edge_category_name` names categories 1-31 and falls back to `"?"`; `bin/tier-audit.rs:144` `kind_name`
falls back to `"OTHER"`. Ids neuropil shows as `CELL` / `?` / `OTHER` today, all registered before
or in 0.5.4:

- cells: `ORIGIN` 15, `IMPORTS` 16, `MESSAGE_TYPE` 17, `RPC_PACKAGE` 18, `DOC_TAGS` 19, `ROLE` 20,
  `EVIDENCE` 21, `SCHEMA_FIELDS` 22, `COVERAGE` 23, `ENTRYPOINT` 24, `ACCESS_MODE` 25, and 0.5.4's `UNIT` 26
  and `IMPORT_BINDINGS` 27;
- edge categories: `IMPLEMENTS` 32, `SHARES_DATA_SOURCE` 33, `RPC_CALLS` 34, `NAVIGATES_TO` 35,
  `CO_CHANGES` 36, `SAME_AS` 37 (0.5.2) and 0.5.4's `TRACES` 38;
- node kinds: 0.5.4's `GOAL_UNIT` 50, among the ones `kind_name` already lacks.

Optional arms; the one-line fix that never goes stale again is the registry's own name function:
glia-code-domain's `cell_type::name`, `edge_category::name` (lowercase it for the HUD) and
`node_kind::name`. The SHARES_DEPENDENCY arm (`hud.rs:1548`) still cannot fire (no emitter since 0.5.2).

## 8. Optional: new in-process primitives

All by module path. The 0.5.2 and 0.5.3 handoffs' section 7 primitives stand.

| primitive | fits in neuropil | packet (commit) |
|---|---|---|
| `glia_engine::health::health_with(&merged, &roots, &opts)` / `boundary_health(&merged)` | a HUD panel saying per mechanism (http, grpc, queue, ws, ..) whether the graph can see it: BLIND / DEGRADED / OK with its measure, and the repo's role | FH.2a (`f092a5ce`), FH.26 (`1cf4bcd9`: test-site producers counted apart) |
| `glia_code_domain::read_import_bindings(payload)` (an IMPORT_BINDINGS cell's JSON text) | per MODULE, the external imports it binds (spec, name, alias, row), for an imports overlay | FA.21 (`bff59e64`) |
| `VectorIndex` (glia-engine's `semantic` module) over `.glia/vectors.jsonl` | a "similar nodes" search, once `glia embed` has run on the repo (a snapshot step; the model reaches neuropil only if it enables `glia-embed/fastembed`) | FB.4 (`c98fffcf`) |

## 9. Re-test (neuropil's G24 protocol)

After the 0.5.0 handoff's sections 2-3 and the pin, once for 0.5.2-0.5.4 together:

```
cargo check --workspace --all-targets && cargo test --workspace
cargo run -p neuropil-app -- .                              # boot: [parallel] and [timing] lines on stderr
cargo run -p neuropil-app --features test-harness -- .      # in a second shell, then:
python3 tools/test_smoke.py
python3 tools/visual_regression.py compare
```

- `visual_regression compare` drifts as the 0.5.2 handoff's section 8 says, plus sections 5-6 here.
- Hidden nodes, bookmarks and anchored annotations on the re-key families (0.5.2's, then section 5 here)
  need re-setting once.
- From glia, `bash scripts/check-neuropil.sh` should print `new=0` as in section 1; once neuropil has
  renamed, without the `note` line.

## 10. Done vs pending

- **Done on the glia side:** 0.5.3 (tagged `v0.5.3`) and 0.5.4's W0-W26, each close-out green
  (`cargo test --workspace` 7050 passing at W26). This packet ran section 1: `new=0`.
- **Pending, James:** the 0.5.3 -> 0.5.4 bump and the `v0.5.4` tag; then `cargo install --path cli --locked`
  from the tag so the `glia` on PATH matches.
- **Pending, neuropil session:** the 0.5.0 handoff's sections 2, 3 and 6; the 0.5.2 and 0.5.3 handoffs as
  section 3 lists; this doc's section 2 pin, section 7 (optional) and section 9.
- **Pending, glia (not owned by a packet):** `scripts/check-neuropil.sh` still checks a renamed copy until
  neuropil renames; the copy recipe (NEUROPIL_DIR, the rename, the feature edit) stands and is applied by
  the script.
- **Followups the landed packets named for neuropil:** F0.1 (the HUD's cell table, section 7), F0.2 / F0.6
  (the lock and the stamp, sections 1-2), FA.21 (IMPORT_BINDINGS on MODULEs), FA.22 (cross edges in merged
  layouts), FB.13c (`named_in`, gaps `named_not_modelled`, if neuropil ever reads them), FD.4a-FD.5b,
  FD.31, FD.76-FD.78 (section 5), FD.6, FD.73-FD.75, FD.79-FD.81, FD.65, FH.4b, FH.6 (sections 5-6). No
  other landed break names neuropil.
- **Checked, no neuropil change:** F0.3-F0.5, F0.7 (slots, rustfmt), FB.1-FB.3, FB.5a-FB.8, FB.11-FB.14 (the
  embedding stack and the pyo3 / CLI surfaces), FD.2, FD.3a-FD.3d (the audit), FD.22-FD.28, FD.34 / FD.34b
  (engram-export), FD.69 (check markdown), FD.7a-FD.7d (section 4), FE.1-FE.8 (harness, CLI wheel, skills),
  FH.1a-FH.26's answer and bench packets (repo-graph's handoff lists the answer breaks).

## Release-docs addendum (FZ.1, 1ac10500)

- README.md is install / quickstart / CLI only. The long prose moved to docs/overview.md, the release history to docs/history.md and the license rationale to docs/license.md. A link into an old README section now points at one of those files.
- PARSER_STAMP moves once more: FZ.1 gave the 16 reserved slot files a `//!` line in hashed roots, so every parse cache re-parses once after the upgrade. No graph content changes.
