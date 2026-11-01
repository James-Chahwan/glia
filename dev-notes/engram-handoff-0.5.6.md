# engram handoff - glia 0.5.5 -> 0.5.6

For: Engram (engram-export, `--writer engram`). **Pin: `=0.5.6`** (the engram-export exporter built at the
`v0.5.6` tag) / `glia-py==0.5.6`. Apply after the 0.5.4 and 0.5.5 handoffs (`dev-notes/engram-handoff-0.5.4.md`,
`dev-notes/engram-handoff-0.5.5.md`).

**Status: written at glia HEAD `d3a264bf` (after wave W12 of 0.5.6), before the bump.** The version bump and the
`v0.5.6` tag are James's release steps after these docs; at HEAD the workspace and `engram-export/Cargo.toml:3`
read `0.5.5`. Engram was read read-only at HEAD `e6ae2bf` (5 dirty paths), the commit the 0.5.4 handoff read.
Act on Engram only.

**What reaches Engram: graph content only.** `engram-export/` is unchanged since `v0.5.5`
(`git diff --stat v0.5.5..HEAD -- engram-export` is empty), `GMAP_FORMAT_VERSION` stays 6, glia's
`FORMAT_VERSION` 3 / `MANIFEST_VERSION` 2 stand, no registry id is allocated (next free node 51 / edge 39 /
cell 28), and no 0.5.6 packet edits an Engram file. The exporter check passes at this commit:

```
$ CARGO_BUILD_JOBS=8 bash scripts/check-engram-export.sh
[engram-export] check: ok - build + 101 tests passed against engram-core GMAP_FORMAT_VERSION=6
```

0.5.6 finishes started work (plan `dev-notes/next-leap-0.5.6.md`, packets `dev-notes/leap-056-packets.json`,
waves W0-W13): GA watch, GB concept search over flows, GD per-language precision and recall, GH the host graph
(plugin hosts and environment hosts), GP build speed and hygiene. Sources, in order of authority: the code at
`d3a264bf`; each packet's landed result (its `breaking` text) and commit body; the `HANDOFF FROM ..` paragraphs
routed to GZ.2 in `dev-notes/leap-056-corrections.json`. The hash after a packet id is the glia commit that
landed it.

---

## 1. TL;DR

1. **Pin `=0.5.6`**: build the exporter at the `v0.5.6` tag once James tags it; `glia-export-engram -V` then
   prints `glia-export-engram 0.5.6 (build 0.5.6+p<stamp>)`. PARSER_STAMP / BUILD_STAMP moved, so
   `<out>.meta.json`'s stamps move and the first export reparses.
2. **No Engram code change is required.** Every change is graph content: facts keyed by the identity hint of a
   re-keyed or removed node orphan, and new facts appear (sections 2-3). `--since` pairs re-keyed endpoints and
   renamed symbols as before (DE.3 / DE.4), so a `--since` chain across the upgrade reports them as renames
   where it can and as add + remove where it cannot.
3. **Edge order inside one fact list changes once** (GA.2 `af92cd26`): every graph's intra edges are stored in
   canonical (from qname, from kind, to qname, to kind), category, confidence, cells order. engram-export's
   per-fact edge lists change order only; contents are the same.
4. **Two provenance moves** (section 4): a nested `build/` / `out/` source directory is no longer a
   `build_output` REGION but real nodes (GD.25 `190d347d`); `.yarn/` is a `vendored` REGION (a11ec18d).
   `crates/engram-live/src/gmap_sync.rs` `default_drop_provenance` drops both region kinds at seed, so the first
   change adds facts and the second drops nothing that was there before.

## 2. Re-keyed and removed facts (they orphan)

| packet | what moves |
|---|---|
| GD.40 Django include mounts (`6b19e684`) | Django ROUTEs gain the leading `/` and include prefixes (`ANY workspaces/<str:slug>/views/` -> `ANY /api/workspaces/<str:slug>/views/`); regex anchors dropped; `ANY <include prefix>` and `PATCH <mock target>` ROUTEs removed |
| GD.41 Flask / FastAPI mounts (`fb5a02d9`) | `POST /filter` -> `POST /artifacts/filter`; blueprint url_prefix routes |
| GD.42 Express / Koa / Hono / NestJS mounts (`beb6fc62`) | `GET /health` -> `GET /api/health`; NestJS `GET /albums @server` -> `GET /api/albums @server` |
| GD.28 gin `.Use(..)` groups (`4242b652`) | `POST /createAuthority` -> `POST /authority/createAuthority` |
| GD.14 Go route residuals (`f5898fcc`) | field-mount routes re-keyed under clause-named owners |
| GD.16 OpenAPI `servers:` at indent 0 / flow style (`681d9b4a`) | `contract::<doc>::GET:/x` -> `contract::<doc>::GET:<base>/x` (prefect-databricks: 32 ops gain `/api`); `contract_op` identity keys move with them |
| GD.19 GraphQL client residuals (`57893857`) | `graphql_op:client.query` / `graphql_op:request` / `graphql_op:useQuery` .. removed where the call holds its own document; Dart anonymous / by-value documents renamed to their op name |
| GD.18 Ruby Bunny (`6115d1f3`) | `queue_producer:unresolved:rabbitmq` / `queue_consumer:unresolved:rabbitmq` -> `queue_producer:<exchange>` / `queue_consumer:<exchange>` |
| GD.11 TS memo / forwardRef consts (`ec4946df`) | STATE_VAR / standalone COMPONENT facts orphan; a FUNCTION of the same qname appears |
| GD.38 TSX grammar (`0c14aec4`), GD.39 JSX (`f32a3248`) | locals the old grammar's recovery hoisted to module-level FUNCTIONs and standalone COMPONENT / HOOK markers are removed; component FUNCTIONs gain POSITION, CODE and edges (twenty: standalone 1202 -> 250) |
| GD.17 Elixir (`b0d29de6`) | a behaviour module that also defines functions changes kind PACKAGE -> INTERFACE (qname kept, hint kind moves) |
| GD.23 Solidity (`c03557db`) | undocumented literal-constant STATE_VARs removed (zora-protocol's `NAME` / `CONTRACT_BASE_ID`); their getter `evm:<C>.<name>` stays, CONTAINed by the file MODULE with no HANDLED_BY |
| GH.13 Dockerfile image identity (`1bf73a3e`) | `infra:image:--platform=$BUILDPLATFORM` removed; generic-dir images renamed (`infra:image:src` -> `infra:image:cartservice`); no image for a FROM naming a stage, `scratch` or an unfilled `$VAR` |

## 3. New facts and edges

- **The host graph** (GH.2-GH.20): CLI_COMMAND `host_cmd:<host>:<name>`, EVENT_HANDLER `host_event:<host>:<event>`,
  CONFIG_KEY `config:<host>:<key>` (Neovim, Maki, WoW, AwesomeWM, WezTerm, LOVE, Flask, pytest, VS Code,
  Obsidian, webpack); nginx conf MODULEs with ROUTE `ANY <path>`; Procfile / systemd / supervisord MODULEs and
  INFRA_RESOURCE `infra:process:<type>@<dir>`, `infra:unit:<stem>`, `infra:program:<name>`; CLI_INVOCATION
  `cli_invoke:<bin>` from Dockerfiles. Edges under the new emitter `resolver:host` (the 19th Resolve pass,
  GH.6 `dfc62ecd`): rules `<directive>` (nginx), `script_file` / `script_call` / `script_eval` /
  `include_str` / `include_bytes` (GH.7 `e8b517f8`), `<host>:<form>` starts such as `docker:script`,
  `compose:module_attr`, `procfile:celery_app`, `compose:pkg_script` (GH.18 `f1017cdc`, GH.19 `9679d051`),
  `<host>:via_script` (GH.19b `47f00873`, located at the script's kept line) and `manifest_bin` (GH.20
  `b91169a6`).
- **ENTRYPOINT cells** with sources `host:<h>`, `shebang`, `script_arg`, `main_guard` (now also TS / JS), plus
  `host` / `trigger` keys on host starts (GH.4, GH.6, GH.8-GH.11, GH.18). A Dockerfile stage is a launch only
  when a compose / k8s file targets it or it is the final stage (James 2026-11-01).
- **Language facts:** TS interface member METHODs (GD.2 `e79a4d6e`), Rust trait METHODs and cross-file
  IMPLEMENTS (GD.5 `39c7f9e0`), Swift protocol requirement METHODs (GD.15 `75893093`), Elixir protocol heads /
  property tests / elixir-grpc / Absinthe (GD.17), `.mjs` / `.cjs` / `.mts` / `.cts` MODULEs (GH.10
  `db342162`; zora-protocol 0 -> 5 modules), PACKAGE_DEP from Cargo dotted / target tables and pyproject /
  Poetry groups (GD.37 `3ea90c58`), Dart / Elixir CONFIG_KEYs and PHP / Dart / Ruby CLI nodes (GD.21
  `326242e2`), TS EVM clients (GD.20 `30af8154`, GD.30 `1bcdcac5`, GD.31 `3bd4d914`), GraphQL concatenated /
  by-value documents and aliased transports (GD.19), TS heritage and class-resource clients (GD.26 `72605887`,
  GD.29 `3ce330f5`).
- **More edges** (no node goes with them): GD.1, GD.3, GD.4, GD.6, GD.8, GD.9, GD.12, GD.13, GD.22, GD.24,
  GD.27, GD.39 (calls, receivers, heritage, JSX and type references in Go / TS / C++ / Dart / Python / Angular);
  JVM org routing (GD.32 `c7a9b3ef`: org_import / org_call / org_ref for Java / Kotlin, SAME_AS manifest_name
  stars keyed by Maven coordinate); more npm / cargo org edges (GD.33 `25775504`); INFRA_REFERENCES image ->
  base and compose service -> image (GH.13, GH.16 `cf8c2a5c`).
- **Removed edges:** false Rust MODULE -IMPLEMENTS-> trait for foreign / reference / blanket impls (GD.5);
  qualified framework heritage, heritage self-loops and PHP IMPORTS of unowned namespaces (GD.7 `4b077825`);
  wrong `tail_unique` IMPORTS (GD.34 `3b025619`); cross-directory TESTS name_match, Go cross-directory TESTS,
  flutter drive hosts (GD.36 `4844ddfe`); WoW `RegisterEvent` / `SetScript` CALLS (GH.3 `d9a4b89e`). A Rust
  `use` in an inline `mod` starts at the inline PACKAGE (GD.35 `aa4dde8d`).

## 4. Cells and provenance in the gmap

- **ORIGIN** gains `coordinate` on Maven / Gradle PROJECTs (`package:maven:<g>:<a>`, GD.32) and `ts_out`
  (`{"tsconfig","dir","out_dir","root_dir"}`) on a PROJECT whose tsconfig declares `outDir` (GH.19b). An npm
  PROJECT's `subpaths` may hold a `*` pattern key (GD.33): skip keys containing `*` when reading it as a
  literal subpath -> file map.
- **CODE cells with a Json payload** on INFRA_RESOURCE nodes (`{"start":..}`, `{"build":..}`,
  `{"script","exec","row"}`; GH.15 `04e5af09`, GH.16, GH.17 `a3c9ed5d`, GH.19b), on package.json MODULEs
  (`{"pkg_scripts":..}`, GH.19) and on manifest CLI_COMMANDs (`{"manifest","file","line","target"}`, GH.20).
  engram-export's `code_cell` (`engram-export/src/lib.rs`) reads CODE only as a Text payload, so these never
  become proposition text; nothing to change.
- **ROLE** COMPONENT on derived Flutter widgets (GD.10 `e427dea5`) and JSX components (GD.39).
- **Regions.** GD.25: a `build/` / `out/` directory not at a repo or project root is walked as source, so its
  `build_output` REGION anchor is replaced by its nodes and facts. a11ec18d: `.yarn/` (Yarn 2+ committed
  tooling) is a `vendored` REGION with no nodes under it. At 0.5.5 its `.cjs` bundles were not parsed; GH.10 parsed
  them inside this release (phantom ROUTEs from Yarn's own registry client) and a11ec18d collapsed the directory.

## 5. Needs no change (checked)

- The exporter, its `--since` diff, the hints side file (`<out>.hints.jsonl`, LB.6 v1 hints) and
  `--vocab`: unchanged code, check green above.
- The rules memory budget (GA.13 `f2c8c3f5`, GA.14 `60fca954`): matters only if Engram sets
  `GLIA_MEMORY_BUDGET_MB` or runs rules near the row budget (the Budget text now says `bindings`; a trip is a
  `ValueError` / exit 2).
- Concept search (GB.1, GB.3; the semantic tier passed its gate, `dev-notes/concept-gate-0.5.6.md`): a reader
  of concept rows must accept tier `semantic`, the `similarity` key and `resolved_by` `concept_semantic`;
  Engram reads none today.
- NavFact gains appended variants (GD.8, GD.18, GD.31, GD.40, GH.6): build-time only, never in a gmap, and the
  exporter matches none.

## 6. Re-test

1. Build the exporter at `v0.5.6`; `glia-export-engram -V`.
2. Export one repo with mounted routes and one with a Dockerfile; seed a scratch store; the replay's facts table
   shows the re-keyed routes as renames under `--since` (or add + remove) and the new `host_*` / `infra:*`
   facts.
3. `crates/engram-live/examples/replay.rs`' `v6 since pairing:` sums: unchanged line format.
