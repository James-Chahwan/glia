# neuropil handoff - glia 0.5.1 (the catch-up leap, 2026-10-01)

The second glia -> neuropil handoff, after `neuropil-handoff-0.5.0.md`. neuropil is glia's in-process
Rust consumer: it path-depends on six glia crates and calls `generate_one` at startup
(`crates/neuropil-app/src/state.rs:443`). Read this in the neuropil session and act on neuropil only.
0.5.1 is the "catch-up leap": 155 packets in `dev-notes/leap-051-packets.json`, landed on glia's local
`main` in waves W0-W11 since v0.5.0 (`2170ff8`). Breaks were allowed when a packet declared them, and
**consumers pin the release exactly**.

Sources, in order of authority:
- a run of glia's compile check, `scripts/check-neuropil.sh`, against glia HEAD `491c2ad` (after wave
  W10; BUILD_STAMP `0.5.0+p056f1c5bba957392`), on a copy of neuropil with the 0.5.0 handoff's section 2
  edits applied. Section 1 has the output verbatim. Every compile claim comes from that run. HEAD has
  since moved to `2794ec5` (README and a CLI test only, CZ.1), which touches no crate neuropil links;
- each packet's own `breaking` report from the 0.5.1 workflow journals (W0-W10, plus the CB.23 re-run
  whose result landed). Where an agent's report differs from the spec, the report wins, and where a
  report is wrong about neuropil this doc says so (section 5);
- the specs' `breaking` blocks in `dev-notes/leap-051-packets.json`.

Every neuropil `file:line` below was read read-only in neuropil's working tree: HEAD `b140d1e` with the
same 50 dirty paths as at the 0.5.0 handoff. Paths are relative to `crates/neuropil-app/src/` unless they
start with `crates/`, `docs/`, `tools/` or name a root file. Packet ids are glossed in words; the commit
after each id is the glia commit that landed it.

---

## 0. TL;DR

1. **The 0.5.0 handoff is still unapplied.** `Cargo.toml:112-117` still names `repo-graph-*` crates
   and the `driver` feature, so neuropil does not resolve against glia 0.5.0 or 0.5.1. Apply that
   handoff's sections 2 and 3 first; 0.5.1 stacks on top.
2. **0.5.1 adds no compile break.** With the 0.5.0 section 2 edits applied (the rename script plus
   `driver` -> `research`), the check reports **new=0 errors** under `--all-targets` against the 0.5.1
   code at glia HEAD `491c2ad` (section 1; the version field still read 0.5.0 there). No 0.5.1 API change reaches a type neuropil builds: it constructs no `CodeNav`,
   `SymbolTable`, `DomainTables`, `RepoMeta`, `BuildOptions` or `PassReport` literal (grep).
3. **Pin exactly:** `glia-engine = { path = "../glia/engine", version = "=0.5.1" }`. Only glia-engine
   carries the release version; the other five crates keep their own crate versions (section 2).
4. **Persisted state re-keys once for the nodes whose qname moved in 0.5.1**, chiefly mounted Go routes
   (quokka's `POST /activity @turps` -> `POST /api/protected/activity @turps`), C++ out-of-line
   members, verb-named events and a few DOC_SECTIONs (section 3). NodeIds follow the qname, so
   annotations anchored to those nodes lose their anchor. Every other NodeId is unchanged: the RepoId
   scheme did not move in 0.5.1.
5. **The `.gmap` format 3 and CODE-as-span changes do not reach neuropil**: it builds in memory with
   `generate_one` and never loads a layout (section 5 corrects the packets that said otherwise).
6. **Reinstall the `glia` binary from the v0.5.1 tag** with neuropil's move, as the 0.5.0 handoff asked
   for 0.5.0. The hooks prompt (`hooks_install_prompt.rs`) installs hooks that run `glia build .`; a
   0.5.0 binary there writes a format-2 layout, which every 0.5.1 reader of `.glia/graph` (the repo-graph
   MCP server, `glia` itself) rebuilds on its next load.

Bump order:
1. glia is bumped and tagged by James (`[workspace.package].version` and `py/pyproject.toml` to 0.5.1
   together; both read 0.5.0 at HEAD).
2. In neuropil: apply the 0.5.0 handoff sections 2-3, add the pin (section 2 here), run
   `cargo check --workspace --all-targets`, then re-test (section 7).

## 1. The compile check at this commit

A copy of neuropil with the 0.5.0 handoff's section 2 edits, checked with the unmodified script. The
copy and the cache lived in a scratch directory; nothing under neuropil or glia was written.

```
C=<scratch>/npcheck
cp -p ../neuropil/Cargo.toml ../neuropil/Cargo.lock ../neuropil/rust-toolchain.toml "$C/np/"
rsync -a --exclude=target/ ../neuropil/crates/ "$C/np/crates/"
python3 dev-notes/rename-0.5.0.py --apply --quiet --root "$C/np"     # non-git root: every file
sed -i '/^glia-projection-text = /s/"driver"/"research"/' "$C/np/Cargo.toml"
cp -a ~/.cache/glia-consumer-check/neuropil "$C/cc/"                 # warm target (optional)
NEUROPIL_DIR="$C/np" GLIA_CONSUMER_CACHE="$C/cc" bash scripts/check-neuropil.sh
```

```
[rename-0.5.0] rewrote 165 token(s) in 50 file(s) under <scratch>/npcheck/np (0 unknown token(s) left)
[neuropil-check] note - neuropil differs from the baseline's (b140d1ede58ff6e77e8e7326a6a4cf21e822959e dirty=50 src=b690b6b4f6db -> unknown src=73b04787b8a9): NEW/FIXED may be neuropil's own edits, not the leap's
[neuropil-check] ok - new=0 fixed=0 preexisting=0 in 60.2s against glia 0.5.0+p056f1c5bba957392 (neuropil unknown, lock drift +39 -26)
```

- The rename count (165 tokens in 50 files) and the source hash `73b04787b8a9` are the same as in the
  0.5.0 handoff's run, so the copy is the same neuropil the 0.5.0 check saw.
- The `note` line is expected: the copy is not a git checkout.
- The check's parsed error list (`errors.txt`) is empty: 0 errors across every neuropil target and the
  glia crates they pull in.
- **Lock drift +39 / -26**, one more than 0.5.0's +38: the 26 `repo-graph-*` path packages become
  `glia-*`; glia-store, glia-parser-kotlin, glia-stamp and glia-doc enter; the registry crates are the
  0.5.0 eight (`rayon`, `rayon-core`, `crossbeam-deque`, `crossbeam-epoch`, `ignore`, `globset`, `bstr`,
  `tree-sitter-kotlin-ng`) plus **`lz4_flex 0.11.6`** (C0.7, the one 0.5.1 dependency commit,
  `457eba5`). The engine also gained `blake3` and `toml_edit`; neuropil's lock already has versions that
  satisfy them (`blake3 1.8.5`, `toml_edit 0.25.11`, `toml_write 0.1.2`), so they add no line. The first
  build needs `lz4_flex` from crates.io or the local registry cache.

`check-neuropil.sh` still copies neuropil verbatim, so run as-is it reports only the cargo resolution
error until neuropil renames. The `NEUROPIL_DIR` recipe above stands in for it, as it did for 0.5.0.

## 2. Cargo: the pin

| where | 0.5.0 handoff asked | 0.5.1 adds |
|---|---|---|
| `Cargo.toml:112-117` | `repo-graph-<x> = { path = "../glia/<dir>" }` -> `glia-<x> = { path = ... }`; projection-text feature `driver` -> `research` | `glia-engine = { path = "../glia/engine", version = "=0.5.1" }` |
| `crates/neuropil-app/Cargo.toml:86-94` | `repo-graph-<x>.workspace = true` -> `glia-<x>.workspace = true` | nothing |
| `crates/**/*.rs` | 165 `repo_graph_*` path tokens -> `glia_*` (run the rename with `git add -N` on the 11 untracked `.rs` files first; 0.5.0 handoff section 2) | nothing |

**Why only glia-engine takes `=0.5.1`.** A `version` on a path dependency must match the crate's own
version, and only the crates that inherit `version.workspace = true` carry the release number: of the
six neuropil uses, that is glia-engine. The others carry their own versions, which 0.5.1 did not change:
glia-core 0.4.1, glia-code-domain 0.4.3, glia-graph 0.4.3, glia-activation 0.4.6, glia-projection-text
0.4.5 (read from each `Cargo.toml` at HEAD). `version = "=0.5.1"` on any of those fails resolution. All
six come from one checkout, so the engine pin fixes the release for all of them: cargo refuses the
build unless `../glia` is at a 0.5.1 workspace version. Until James tags, glia HEAD still reads 0.5.0,
so add the pin after the tag. (Whether to move the five crates onto the workspace version, so every glia
dep can carry `=0.5.1`, is James's call; it is not part of 0.5.1.)

## 3. What re-keys: qname-keyed state and NodeId anchors

`NodeId = xxhash(graph_type, repo, kind, qname)`. 0.5.1 did not touch the RepoId, so only nodes whose
kind or qname moved get a new id. neuropil keys state by qname in `view_state.rs` (`hidden_nodes`,
`hex_collapsed`, `extra_qname_prefixes`, `hide_node`), the session `bookmarks` table
(`crates/neuropil-session/src/lib.rs:196`) and `bookmarks.rs`, and by NodeId in
`annotations.rs:45`, `:55` (`.neuropil/annotations.json`) and `flow_replay.rs:35-47` (the 0.5.0
handoff's sections 4.1-4.2). For the nodes below, those entries stop matching once; an anchored
annotation falls back to its stored `world_pos` (`annotations.rs:279-282`).

| family | packet (commit) | old -> new | measured |
|---|---|---|---|
| Go routes on a mounted group | CB.23 Go parser emits router mounts (`e072dce`, the W8 re-run), CB.20 build-time mount pass (`af2f094`), CB.11 routes on a struct field (`8b3d8fd`) | a ROUTE registered on a router-typed parameter or a struct field whose mount the build resolves gains the prefix: `<METHOD> <local>[ @owner]` -> `<METHOD> <prefix><local>[ @owner]`. A function mounted N times yields N ROUTEs. Unmounted registrations keep their qname byte for byte. | quokka-stack: 58 route qnames change (`POST /activity @turps` -> `POST /api/protected/activity @turps`; `GET /user/2fa @turps` -> `GET /api/user/2fa` + `GET /api/protected/user/2fa`). Kina: 87 (`GET /stats @backend` -> `GET /admin/stats @backend`, every `/api` route likewise). |
| C++ members | CB.1 `.hh/.hxx/.inl/.ipp/.tpp` routed to the C++ parser (`84448eb`), CB.19 nested types, templates, unions (`70286f0`) | an out-of-line member whose class is declared only in a newly routed header, or a member of a nested type, goes FUNCTION `<file>::...::Q::m` -> METHOD `<scope>::Q::m` (kind and qname both change) | fixtures only; no C++ repo is known to be tracked in neuropil |
| events | CB.3a verb-named events removed (`444848b`), CB.3b constant-keyed events fold to their literal (`5a968a7`) | `event_emit:Subject.next` / `emit` / `publish` ... and `event_handle:on` / `subscribe` / `@OnEvent` ... vanish; a constant-keyed site is `event_emit:<literal>` when resolvable, else `event_emit:<Const.Path>` | quokka's `event_emit:Subject.next` goes; no constant-keyed site in quokka, lapse, Kina or neuropil |
| doc sections | CE.4a fence-aware chunking and slug dedupe (`111bd1b`) | a `#` line inside a code fence is no longer a DOC_SECTION (it folds into its enclosing section); the 2nd+ section with a repeated slug gets `<...>::<slug>-N` | Kina README: 14 sections go; quokka `.ai/WORKFLOW.md`: 4; glia README 10, `docs/onboarding.md` 13 |
| contract ops | CB.2 contract YAML sniffing (`2ff7f49`) | a yaml whose `openapi:` / `swagger:` / `asyncapi:` key has no version value loses its `contract::...` ops; an alphabetical swaggo `swagger.yaml` gains them | the twins share one NodeId (see the note below) |

**Shared NodeIds across per-language graphs (by design, no action).** swaggo writes `docs/swagger.json` and `docs/swagger.yaml` side by side; CB.2 now reads the yaml too, and by the LB.12 identity rule both name their ops `contract::<dirs>::swagger::<op>`, so the twins are ONE node. `MergedGraph` keeps one instance per per-language graph that holds it ("one id can sit in several per-language graphs", `graph/src/merged.rs`), which is how shared synthetic nodes (`config:env:*`, `data_entity:*`, `data_source:*`) have always been stored, 0.5.0 included. Answers dedupe by id (`glia find` on quokka returns one `contract::turps::docs::swagger::GET:/healthz` row, checked 2026-10-01), and `GliaGraph::load` already collapses them (`state.rs:522`, `:533-534`). Only code that walks `merged.graphs[].nodes` directly sees an instance per graph; key by id there.

Additive only (new qnames, no existing one moves): Dart constructors, factories, operators and enum
constants (CB.9, `ec5c6ea`), Dart extension containers `<module>::extension<T>` (CB.17, `205861c`;
qnames with `<` `>`, which `service_hex.rs`'s `::` split handles), Swift `init` / `deinit` / `subscript`
/ computed properties (CB.10, `90d1076`), TS arrow-function class fields as METHODs (CG.1, `8bff4f9`),
Python GraphQL field resolvers (CB.14, `a2a619e`), Kotlin Ktor-client ENDPOINTs (CA.6b, `d739141`), tRPC
server-side callers (CB.13, `636cc57`), Go inferred collection entities `data_entity:nosql:<name>`
(CA.4, `a1e05a5`; quokka gains 12).

To make qname state move-proof, the 0.5.0 handoff's section 4.2 still applies: store
`glia_graph::identity::Identity::hint()` beside the qname and rebind a miss through
`IdentityIndex::build(&merged).rebind(qname, kind, hint)`.

## 4. Behaviour changes with no required code change

### 4.1 Rendering and views

- **Route readers** (0.5.0 handoff section 3.2: `flow.rs:333`, `live_node.rs:196`, `:210`,
  `domain_zone.rs:825`, `:832`, `code_viewer.rs:676`, `layout/tier_stack.rs:405`,
  `context_menu.rs:356`, `openapi_catalog.rs:363`). The shape is still `<METHOD> <path>[ @owner]`; only
  the path of a mounted Go route is longer. `openapi_catalog.rs:433` `merge_entries` keys glia routes and
  spec-file routes by `(method, path)`, so once the owner is split off (the 0.5.0 fix), a mounted Go
  route now matches the full path an OpenAPI file declares: expect `n_both` to rise and `n_glia` /
  `n_file` to fall for Go backends.
- **C++ kind change.** The re-parented members are METHODs, so `layout/tier_stack.rs:181`
  `kind_to_sub`, `file_tree.rs:245` `kind_priority` and `hud.rs` glyphs place them with their class.
- **Shared namespaces** (CB.15, `0742baf`): a C# / PHP namespace PACKAGE opened by several files now
  hangs under its first file (it hung under the last). Its qname and NodeId are unchanged; file-tree and
  tier views move it.
- **Spans** (CB.4, `84a3581`): a decorated TS / Angular / Vue / React method's POSITION starts at its
  first decorator. `position.rs` reads it as before; code-viewer ranges and the Inject gold-set overlap
  at `inject_panel.rs:441-446` grow by the decorator lines. POSITION rows are still 0-based.
- **DOC_SECTION text** (CG.3, `dd191fa`): a markdown section's CODE cell holds the whole section, up to
  64 KiB, not its first 500 bytes. `code_viewer.rs:633` shows the first 40 lines, so a doc node shows
  more text.
- **Cells on channel clients** (CB.21, `016775d`; CB.24, `99502f9`): WS_CLIENT and GRPC_CLIENT nodes
  carry `ENDPOINT_HIT` `{"via":"ws"|"grpc"[,"host":..]}`, GRAPHQL_OPERATION / RPC_CALL may carry
  `{"via":"graphql"|"rpc","hosts":[..]}`. `hud.rs:1401-1428` and `code_viewer.rs:643` print Json
  payloads raw; neuropil parses no ENDPOINT_HIT field.
- **ORIGIN provenance** gains `inferred:wrapper` (CA.4), `external` on third-party ENDPOINTs (CG.4b,
  `0a23e66`) and `test_fixture` on more paths (CG.2a, `5004a5a`: e2e / cypress / fixtures / testdata /
  `__mocks__` / `test_*.py` ...). neuropil reads no ORIGIN.

### 4.2 Edges

- Go: many more CALLS (CA.1 closures, `efdf872`; CA.2a / CA.2b typed receivers, `43260b9`, `7788f90`);
  fewer IMPLEMENTS (CA.3b, `c8f2257`: Kina 132 -> 84 type-level) and Go method-level IMPLEMENTS go
  Strong -> Medium, so any confidence styling shows them as medium; HANDLED_BY from routes to
  receiver-method handlers (CA.5a, `22c3d82`: Kina 36 -> 89 of 90 routes). `flow.rs` animations over
  quokka / Kina backends get deeper.
- Swift implicit self (CB.18, `008f2b8`) and C++ class scope (CB.25, `b78c622`): a bare call inside a
  member binds the member, not a same-named free function (the old edge goes).
- Host narrowing (CB.21, CB.24): arch and flow views lose a ws / graphql / rpc link that fanned out to
  every same-path service, and gain a gRPC link the dial host resolves.
- External HTTP endpoints (CG.4b): an ENDPOINT whose every site dials a public, unconfigured host no
  longer pairs with a same-path route.
- Feature-flag reads (CC.7a, `74f9402`): READS_CONFIG into `config:flag:<key>` starts at the reading
  function, not the module (`hud.rs` `reads_config` rows).
- Dart constructor bodies (CB.9): CALLS start at the constructor METHOD, not the CLASS; markers inside
  constructors hang on the METHOD (USES / HANDLED_BY), not the MODULE (CONTAINS).
- The doc linker (CG.3): more DOCUMENTS edges (quokka 28 -> 179).

### 4.3 The build

- **TS on the engine pool** (CA.7, `8585e0c`): the TS family's graph build runs on the rayon pool as
  its last item instead of on the calling thread; output is byte-identical. A panicking TS build is now
  quiet like every other language (the hook no longer prints it).
- **Per-phase timers** (CA.9, `b0a91d9`): every build prints `[timing] repo=...` per repo and one
  `[timing] build ...` line to stderr. neuropil could show them in `perf_overlay.rs`.
- More new stderr lines at boot: `[provenance] ...`, `[go-mounts] ...`, `[http] mount-segment folds: ...`,
  `[ts-fields] ...`, `[event-const] ...` (only when something folds), `[client-hosts] ...`,
  `[http-external] ...`. neuropil parses none.
- **Cargo.lock moved, so PARSER_STAMP moved.** neuropil keeps no parse cache (`generate_one` is
  uncached), so nothing rebuilds on its side.

## 5. Corrections to the packets' neuropil notes

- **CD.7b (`4fdf434`, FORMAT_VERSION 2 -> 3) and CD.7c (`5611813`, CODE cells as spans)** say neuropil
  "reads layouts in-process ... rebuilds on first load" and that `code_viewer.rs` "shows the span JSON
  when its layout's sources are missing". neuropil loads no layout: `state.rs:443` and
  `bin/tier-audit.rs:21` call `generate_one`, whose CODE cells are always text. Nothing changes today.
  It matters only if neuropil adopts a warm start (0.5.0 handoff section 5): through
  `glia_engine::persist::load_or_rebuild(dir, Some(repo), true)` a span that no longer resolves makes the
  layout rebuild, so CODE still arrives as text; only a direct `persist::load_layout` hands back
  `{"code_span":{"file","start","end","xxh64"}}`, and then `code_viewer.rs:643` should decode it with
  `glia_code_domain::code_span::CodeSpan::from_payload` and slice the file with `CodeSpan::slice`.
- **CB.3b** says "Engram identity_hint follows the qname"; that is an Engram note, and it is not quite
  right (the Engram handoff explains). For neuropil the qname rule in section 3 is what matters.
- **CD.5b (`815ef80`)** notes neuropil builds no `RepoMeta`: correct (grep).

## 6. Optional: new in-process primitives

All by module path (glia's facade rule: new 0.5.1 primitives are module slots, not flat re-exports).
Each returns located rows with 1-based lines.

| primitive | fits in neuropil | packet (commit) |
|---|---|---|
| `glia_engine::communities::communities(&merged, &repo_labels, &CommunityArgs::default())` (`repo_labels` is `GenerateResult.repo_labels`) over `glia_activation::algo::community::{leiden, label_propagation}` | `flocking_layout.rs` clusters by qname prefix today ("~30 for quokka with prefix splitting", `:43`); seeded Leiden gives structural clusters with labels, entries, services and inter-community links | CD.1a-CD.1d (`86cbe89`, `6aab27d`, `f03c321`, `9edef48`) |
| `glia_engine::pack::{pack, pack_ids}` over `glia_projection_text::ladder` | Inject / chat context: a context pack sized to a token budget, each node at full / preview / outline / qname fidelity, with a manifest of what was dropped | CC.4a-CC.4b (`7a34e5c`, `62f1a4c`) |
| `glia_engine::hubs::hubs(&merged, &repo_labels, &HubArgs)`, `glia_engine::hotspots::hotspots(&merged, &HotspotArgs)` (churn x centrality; the `centrality` activation preset) | a heat overlay on the tier stack; hubs flag utilities, orchestrators and cross-service connectors | CD.4b (`16f75ba`), CC.10a (`9456731`) |
| `glia_engine::splits::splits` | `service_hex.rs`: proposed service boundaries with the cut edges located | CD.2b / CD.2c (`de96625`, `4bad2c7`) |
| `glia_engine::duplicate_flows` | flag entry flows that reach the same set | CD.4e (`12114b8`) |
| `glia_engine::timeline::{build_timeline, edge_history, as_of}` | a time-travel scrubber: per-edge `[valid_from, invalid_at)` over the last N first-parent revs | CD.5c (`d50617e`) |
| `glia_engine::cochange::cochange`, `glia_engine::flags::flags`, `glia_engine::review::review_vs_rev`, `glia_engine::contract_breaks::contract_breaks_vs_rev` | co-change suggestions, stale feature flags, a PR review panel, contract breaks vs a rev | CC.11a co-change suggestions (`7b93c52`), CC.7b stale flags (`43e07bf`), CC.6a review (`b492e71`), CC.8a contract breaks (`c3b503e`) |
| `glia_engine::shared_cache::{wanted, import_entries, install_layout}` + `glia cache pull --layout` | a cold start from a pulled layout or pulled parse entries instead of a full build | CE.2a-CE.2d, the shared / remote cache (`624e9e9`, `8ba4621`, `f094679`, `d919a05`) |

The Inject scene items from the 0.5.0 handoff section 5 (`ActivationPlan` with synth hooks) are
unchanged.

## 7. Re-test (neuropil's G24 protocol)

Run in neuropil after the 0.5.0 handoff's sections 2-3 and the pin:

```
cargo check --workspace --all-targets && cargo test --workspace
cargo run -p neuropil-app -- .                              # boot: [parallel] and [timing] lines on stderr
cargo run -p neuropil-app --features test-harness -- .      # in a second shell, then:
python3 tools/test_smoke.py
python3 tools/visual_regression.py compare
```

- Expect `visual_regression compare` to drift on scenes that draw quokka / Kina backends (mounted Go
  routes, more Go CALLS, fewer IMPLEMENTS) and decorated TS methods. Review the diffs, then re-record.
- Hidden nodes, bookmarks and anchored annotations on the section 3 families need re-setting once.
- From glia, `bash scripts/check-neuropil.sh` should then report `new=0` plus the `note` that neuropil
  moved from the baseline's sha.

## 8. Done vs pending

- **Done on the glia side:** W0-W10 (153 packets, all green) and W11's CZ.1 (`2794ec5`, README). The
  section 1 run shows neuropil compiling against them once the 0.5.0 section 2 edits are in.
- **Still running:** CZ.2 (release docs 2/2: CLAUDE.md and the glia skill); docs only, and it names no
  neuropil item.
- **Pending, neuropil session:** the 0.5.0 handoff sections 2, 3 and 6, then this doc's section 2 pin and
  section 7.
- **Pending, James:** the 0.5.1 bump and tag, then `cargo install --path cli --locked` from the tag so
  the `glia` on PATH matches.
- **Pending, glia (not owned by a packet):**
  `check-neuropil.sh` still copies neuropil verbatim (the 0.5.0 handoff's open item).
- **Checked, no neuropil change:** C0.1-C0.7 (slots, the dependency commit), CA.2a / CA.3a / CB.6
  (`CodeNav` gained `return_types`, `method_sigs`, `nav_facts`; neuropil has no `CodeNav` literal),
  CB.15's `SymbolTable.home_module` (no literal), CD.1c's `DomainTables.community_weights` (neuropil
  builds none), CA.9's `PassReport` fields, CD.5b's `RepoMeta.rev`, CE.3b's `BuildOptions.overlay_text`
  (`#[non_exhaustive]`), CC.3 / CE.1d / CE.1e (evidence tiers and the `scip` stage: neuropil keys nothing
  on evidence), and every CF matrix probe (bench test data only).
