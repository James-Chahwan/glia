# lua-support.md — Lua parser + the plugin-family bridge (spec)

Seeded 2026-10-27 against the glia tree at `929d99ee+` (0.5.3 released; working tree
clean apart from the repo-graph MCP block in CLAUDE.md). Three packets, L1 → L2 → L3,
each landing green alone. Proving target: the **maki** repo
(`/home/brian/Projects/Personal/AI/Projects/maki` — Rust host + 153 Lua plugin files,
read-only for this work; it already carries a `.glia/` build of its Rust crates).

## 0. How to use this spec (fresh context)

- **Use the repo-graph MCP first** (`orient` / `find` / `impact` / `read`) for any
  structural question in *glia*; grep only what `orient`'s blind spots flag. The MCP
  answers from an installed older build (glia-py 0.5.1) and **lags the tree** — every
  line number below was verified against the current tree; re-verify by grep where a
  symbol moved.
- This doc inlines the constraints you need. Read `CLAUDE.md` only for the sections it
  points to; read `CODE_RULES.md` for the general code conventions.
- Do not re-survey maki — §5 records the survey (counts, anchors, require forms).
- Never push, tag, or publish. James is the release gate.

## 1. Locked ground rules (inline — do not re-derive)

- **Registry ids are allocated only in `code-domain/src/lib.rs`** (the `RESERVED`
  blocks, `node_kind` / `edge_category` / `cell_type`), in the **same commit as the
  emitter**, and never renumbered. Verified free: **node 50, edge 38, cell 26**.
  This spec spends node 50 and node 51 (L1/L2) and edge 38 (L3). Cell 26 stays free.
  `code-domain/src/lib.rs:4734` asserts `node_kind::name(NodeKindId(50)) == "UNKNOWN"`
  — a next-free-boundary pin; when you allocate 50, move that pin to the new boundary
  in the same commit.
- **Runtime discovery is preferred; static tables are not.** A repo's plugin
  families come from `.glia/plugin_families.toml` (written by
  `glia plugin-families <repo>` via `--write`; the default dumps it to stdout and
  `--write --force` overwrites) and are read first by the build; without it the
  build infers them from the tree.
  The one static table, `code_domain::plugin_families::SEED_FAMILIES`
  (`seed_families()`), exists only to SERVE that generic discovery: the seed
  architectures recognition matches an inferred repo against, the `[[plugin_family]]`
  template `glia plugin-families` renders, and the manifest basenames the walk
  admits. Maki is the one shipped seed; a new architecture is one seed row plus
  fixtures, never new code, and nothing in the parser, router or resolver reads a
  host name or a table — every consumer takes the repo's `PluginFamilies` value
  (or, for the resolver, the facts the graph itself carries) as a parameter.
- **Parsers extract; the graph resolves.** Parsers emit raw nodes, edges between nodes
  they minted, and `UnresolvedRef`s — never a resolved cross-file edge they had to
  look up another file for.
- Qnames use `::` as the separator; ids come from `NodeId::from_parts(GRAPH_TYPE,
  repo, kind, qname)`. Every edge carries exactly one EVIDENCE cell (emitter, rule,
  file, 0-based row).
- **`fired_on` marker with every lever** (stderr, grep-able). If you can't grep proof
  it ran, it's dead code.
- No `unwrap()` / `panic!()` in non-test code. Determinism: rows sort by qname or
  (repo label, path) / candidate index — a HashMap may be looked up, never iterated
  into an output.
- **Gates:** `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --
  -D warnings`, full `cargo test --workspace` at close-out. The rank gate
  (`scripts/rank-gate.sh check`) is **not** required here (no ranked answer path is
  touched); run it only if you edit `engine/src/rank` / blast-radius code.
- Adding a parser crate moves `PARSER_STAMP` (parsers are hashed roots) — expected;
  the parse cache invalidates itself. No version bump, no crate published.
- **Surface:** this spec adds the `glia plugin-families <repo>` CLI command (and
  edits the README CLI block) → regenerate `cli/surface/plugin-families.txt` in the
  same commit (done: the snapshot gains the `--write` flag). No pyo3 function is added;
  if one ever is, regenerate the matching `py/api_surface/` entry in the same commit.

## 2. Packet L1 — the Lua parser crate + its graph builder

**Creates:** `parsers/code/lua/`, `code-domain/src/plugin_families.rs` (the
plugin-family model — the runtime-discovery seam, below). **Edits:** root
`Cargo.toml` (members), `engine/Cargo.toml`, `engine/src/extract.rs` (`ParseContext`
and the two `lua` arms), `graph/src/build.rs` (`build_lua`, the families
parameter), `graph/src/imports.rs` (the require rule), `engine/src/build/lang_build.rs`
(builder arm + `GroupCtx.plugin_families`), `code-domain/src/lib.rs` (one
`pub mod plugin_families;` line).

### Grammar

`tree-sitter-lua` **0.5.0** (tree-sitter-grammars, MIT, current as of 2026-02) —
`tree_sitter_lua::LANGUAGE` is a `tree_sitter_language::LanguageFn`, used exactly like
terraform's HCL grammar: `tree_sitter::Language = tree_sitter_lua::LANGUAGE.into()`
with `tree-sitter = "0.26"` (the workspace's version — matches
`parsers/code/terraform/Cargo.toml`).

### Crate shape — template: `parsers/code/terraform` (864 lines, the smallest full one)

```toml
[package]
name = "glia-parser-lua"
version.workspace = true
edition = "2024"
publish = false

[dependencies]
glia-core = { path = "../../../core" }
glia-code-domain = { path = "../../../code-domain" }
tree-sitter = "0.26"
tree-sitter-lua = "0.5"
```

Entry point, signature verbatim (terraform's, `parsers/code/terraform/src/lib.rs:28`):

```rust
pub fn parse_file(
    source: &str,
    file_rel_path: &str,
    module_qname: &str,
    repo: RepoId,
) -> Result<FileParse, ParseError>
```

Re-export the code-domain surface from the crate root exactly as terraform does
(`CallSite, CodeNav, FileParse, GRAPH_TYPE, ImportStmt, ImportTarget, ParseError,
UnresolvedRef, cell_type, edge_category, node_kind`). The MODULE node is minted from
`module_qname` (`NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MODULE, module_qname)`).

### The plugin-family source of truth (runtime discovery, 2026-10-29 rework)

`code-domain/src/plugin_families.rs` holds the model, and it is owned data end to
end — no global state, no `Box::leak`, no cwd reads, no file writes from a build:

```rust
pub struct PluginFamily {                       // one plugin architecture
    pub name: String,                           // "maki" — the qname owner segment
    pub manifest: String,                       // "plugin.toml" — routed manifest basename
    pub calls: Vec<RegistrationCall>,           // one per registration API (L4's call families)
    pub lib_roots: Vec<String>,                 // ["lib"]
}
pub struct PluginFamilies { /* owned rows, in tie order */ }

pub fn for_repo(repo_root: &Path) -> PluginFamilies; // declared file, else inference
pub fn read_declared(repo_root: &Path) -> Option<PluginFamilies>;  // .glia/plugin_families.toml
pub fn infer_families(repo_root: &Path) -> PluginFamilies;         // seed match over the tree
pub fn walk_manifest_names(repo_root: &Path) -> Vec<String>;       // the walk's admission list
pub fn declaration_template() -> String;                           // what `glia plugin-families` writes
pub fn manifest_family(path, &[PluginFamily]) -> Option<&PluginFamily>;
pub fn call_for(dotted, &[PluginFamily]) -> Option<(&PluginFamily, &RegistrationCall)>;
pub fn mints_tool(dotted, &PluginFamily) -> bool;
```

- **The declared file wins.** `.glia/plugin_families.toml` is written by
  `glia plugin-families <repo>` (`--write`; the default dumps it to stdout) and
  read by `for_repo`; a family declared there beats anything inferred.
- **Discovery infers next.** A seed architecture's manifest basename beside
  Lua sources infers that family (its calls and the lib roots the Lua sources
  sit under, depth-capped scan). An undeclared, unmatchable repo has none, and
  nothing plugin-shaped is minted.
- **The static `seed_families()` serves discovery only**: recognition, the
  declaration template, and the walk's admission union. It is never a consumer's
  lookup table.
- **Consumers take the value as a parameter**: the Lua parser
  (`parse_file(.., families)`), the L2 route branch
  (`families.manifest_family(path)`), `build_lua` /
  `resolve_lua_requires` (the lib-root walk), the walk gate
  (`walk_manifest_names`). The L3 resolver reads NO table at all — the
  `NavFact::PluginManifest` facts the route records carry the family and
  manifest basename into the graph.
- Matching is basename / dotted-prefix exact — no globs, no config files,
  family order is the tie order, so results are deterministic.
- The parse context: families ride `engine::extract::ParseContext` (beside the
  Go module set), so a family change discards the parse cache exactly as a
  go.mod edit does, and the watch session full-rebuilds with reason
  `plugin_families`.
- The file is graph-shaping source, and this is verified: `stamp/build.rs:29` hashes
  `HASHED_ROOTS = ["core", "code-domain", "graph", "engine", "store", "parsers",
  "activation"]` — every source file under `code-domain/` feeds `PARSER_STAMP`, so a
  row change moves the stamp and invalidates parse caches exactly as a parser change
  does. No stamp change needed.

### Engine wiring (three exact spots)

1. `engine/src/extract.rs:18` `detect_language` — add one arm in the match:
   `"lua" => Some("lua"),` (after the solidity / terraform rows). The walk then admits
   `.lua` automatically; no walk change.
2. `engine/src/extract.rs:138` `parse_one_as` — add the dispatch arm beside the
   existing per-language rows (`:182–186`):
   `"lua" => glia_parser_lua::parse_file(source, path, &module_qname, repo)…`
   (full crate path, no `use` needed — the elixir/solidity arms set the pattern).
3. MODULE naming: **no special case**. The default `path_to_qname`
   (`engine/src/extract.rs:724`: directories `::`-joined, extension dropped) names
   `plugins/question/init.lua` → `plugins::question::init`. The `c_cpp`
   full-file-name special case does not apply.

`build_group("lua")` needs no code — the default arm of `build_group`
(`engine/src/extract.rs:75`) returns the tag itself, so Lua is its own build group.
Add the builder arm in `engine/src/build/lang_build.rs` (the group→builder match at
`:316–338`): `"lua" => glia_graph::build_lua(repo, parses),`.

### What the parser extracts (and only this)

Confirm exact tree-sitter-lua node names from `tree_sitter_lua::NODE_TYPES` before
writing the walk (grammar node names below are the grammar's documented shapes;
one read, then done).

| Lua source | Extracted | Notes |
|---|---|---|
| `local function f()` / `function f()` | `FUNCTION` node | qname `<module>::f` |
| `function T.f()` / `function T.m()` | `FUNCTION` node | qname `<module>::T.f` — a table field, modelled flat |
| `function T:m()` (colon) | `METHOD` node | qname `<module>::T.m`; the implicit `self` parameter is not a node |
| `local T = {}` + field assigns | nothing extra | the table only becomes bindable through the names above |
| `require("x")` / `require("a.b")` (string-literal argument) | `UnresolvedRef` IMPORTS ref | spec text verbatim; a non-literal argument is ignored (counted in the marker) |
| plain identifier calls `f(...)` | `UnresolvedRef` CALLS ref (bare) | resolved by `build_lua` in-file, else left bare |
| `T.f(...)`, `T:m(...)`, `alias.f(...)` | `UnresolvedRef` CALLS ref with qualifier text | same |
| `maki.api.register_tool({ name = "x", execute = function(...) ... end, ... })` — any call whose dotted callee text starts a shipped host's `register_prefixes` | **registration** (below) — minting a `PLUGIN_TOOL` is the row's `tool_calls` gate | prefixes come from the repo's families; no family name is a literal in the parser |

**Registration extraction** (the bridge's Lua-side endpoint). For each call whose
dotted callee starts a prefix of some shipped host row (`host_for_callee` returns the
row; its `name` becomes the owner label): the callee's suffix after the matched
prefix decides the kind — a suffix in the row's `tool_calls` is a **tool
registration** (below); any other suffix (maki: `register_options`,
`register_prompt_hint`, `register_permission_rule`) is an **auxiliary
registration** (lua-support ruling 3): it mints nothing and counts `aux=` in the
marker, and its table fields still walk, their calls attributing to the call
site's owner. For a tool registration, walk the first table-constructor
argument. A field keyed `name` whose value is a
string literal names the registration; a field whose value is an anonymous
`function_definition` is a handler closure. Mint per named registration:

- one **`PLUGIN_TOOL` node** (kind id **50**, allocated in this packet), qname
  `plugin_tool:<name> @<host>/<dir>` — the LB.8 owner-segment convention
  (`graphql_resolver:getUser @services/users`), so the resolver's `build_kind_index`
  keys it by the bare name and the L3 `same_dir` rule reads host and plugin dir off
  the owner tail. `<host>` is the matched host row's `name`; `<dir>` is the basename
  of the registering file's directory (maki: `question` for
  `plugins/question/init.lua`).
- one `FUNCTION` node per handler closure, qname `<module>::<tool-name>.<key>`
  (`plugins::question::init::question.execute`) — deterministic from the table key,
  so two parses of one file agree.
- one `HANDLED_BY` edge (edge id **9**, reused) `PLUGIN_TOOL →` each handler
  `FUNCTION`, evidence emitter `extractor:lua`, rule `register_handler`, at the
  registration call's row. This is same-file, AST-known — extraction, not resolution.
- A tool registration with no `name` string field mints nothing and counts in the
  marker's `unnamed=` (often a dynamic `name = spec.name` in a registration loop —
  data-driven names stay unminted; the spec forbids guessing) — its table fields
  still walk. An auxiliary registration counts `aux=` even when its table carries a
  literal `name`: the gate is the callee kind, so an options-for-X API can never
  mint a bogus tool (ruling 3).

**Non-goals for the parser (explicitly):** metatables / `setmetatable` OOP, `pcall`
indirection (recorded as ordinary calls to their named target when literal, per the
table above), `coroutine` flows, string-keyed dynamic dispatch, `dofile`/`load`,
Neovim's `vim.*`, LuaJIT FFI. Anything not in the table above is ignored.

**Marker** (per parsed file, on the parser's tally line the engine already aggregates
per language group — mirror how terraform reports; the line shape):

```
[lua] functions=<f> methods=<m> requires=<r> registrations=<g> aux=<x> unnamed=<u> file=<p>
```

### `build_lua` (graph/src/build.rs) — intra-repo resolution

Model on `build_ruby` (`graph/src/build.rs:615`) for shape; `build_elixir`
(`:521`) shows the `extra_hook` seam if needed (it is not, for v1).

- MODULE `DEFINES` FUNCTION/METHOD edges (DEFINES = 1) for every symbol above.
- **Require resolution** — a Lua arm in `graph/src/imports.rs` (beside the Go / TS
  target rules). For an IMPORTS ref with spec `s` from file `p`:
  1. **same-dir**: `<dir(p)>/<s>.lua` (bare `require("helper")` beside its requirer).
  2. **lib path** (table-driven): dotted `a.b.c` → `a/b/c.lua`. Walk the ancestor
     directories of `p`, nearest first, ending at the repo root; for each ancestor
     `A`, try in order: `<A>/<L>/<spec>.lua` for every shipped host's `lib_roots`
     (table order), then `<A>/<spec>.lua`. This binds maki's
     `require("maki.tool_view")` → `plugins/lib/maki/tool_view.lua` (requirer
     `plugins/question/question_form.lua`, ancestor `plugins/`, lib root `lib`).
  3. First existing `.lua` wins; the candidate order above is total, so no tie;
     none binds → the ref stays unresolved (bare), never guessed.
  Bound pairs emit IMPORTS edges (3), evidence rule `require_path`, emitter
  `graph:imports`; count and print once per graph:
  `[imports] lua requires bound=<b> (same_dir=<s> lib=<l>) ambiguous=<a>`.
- **Call binding** — file-local only: a bare CALLS ref binds the in-file FUNCTION of
  that name (nearest enclosing scope wins; ties impossible — Lua locals shadow);
  a qualified `T.f` binds when `T` is (a) an in-file module table with a minted
  `T.f`, or (b) the alias of a same-file `local T = require("spec")` whose require
  resolved — then any minted `X.f` of the target MODULE binds. Everything else stays
  bare. Emitter `graph:calls`, rules `local_call` / `module_table` / `require_alias`.
  No cross-file guessing, no metatable dispatch, no globals.

### Tests (L1)

- In-crate units (terraform's `#[cfg(test)]` style): one source sample per table row,
  asserting node kinds, qnames, ref counts.
- Host-genericity units: the registration scan takes the host row's prefix list as a
  parameter; a test-constructed `PluginFamily { name: "toy", manifest: "host.toml",
  calls: vec![..], lib_roots: vec![] }` proves a non-maki family mints
  `PLUGIN_TOOL` nodes and that the maki row is data, not code.
- `engine/tests/lua_plugin.rs` over a committed fixture `engine/tests/data/lua-plugin/`
  (see §6 for the tree): builds the fixture and asserts — MODULE count, the require
  IMPORTS pair (`helper` same-dir, `maki.tool_view` lib-path), the registration's
  `PLUGIN_TOOL` + two handler FUNCTIONs + `HANDLED_BY` edges, and zero unresolved
  CALLS from the bound sample.
- Registry boundary pin: `code-domain/src/lib.rs:4734` asserts the *next free* node id
  is `UNKNOWN`. Each packet that allocates an id moves the pin in the same commit:
  L1 allocates 50 (`PLUGIN_TOOL`) → the pin asserts 51; L2 allocates 51 (`PLUGIN`) →
  the pin asserts 52. Never delete the pin — moving it is the allocation's proof.

**Acceptance L1:** `cargo test -p glia-parser-lua`, the `glia-code-domain`
host-table units, and `cargo test -p glia-engine --test lua_plugin` green; `glia
build` on the maki checkout parses all 153 Lua files (`[lua]` lines present, zero
parse errors); clippy + fmt clean.

## 3. Packet L2 — route `plugin.toml` (the manifest side)

**Creates:** `parsers/code/extractors/src/plugin_manifest.rs`. **Edits:**
`parsers/code/extractors/src/lib.rs` (one `mod` line + re-export),
`engine/src/route.rs` (one branch), code-domain (kind **51** `PLUGIN`, allocated here).

### Design facts that fix the shape

- maki's `plugin.toml` carries **no plugin name** (verified: `plugins/question/plugin.toml`
  is `[permissions]` only) — the loader keys plugins by **directory**
  (`maki-lua/src/loader.rs:740` `load_package(name, dir, …)`; discovery is a runtime
  `fs::read_dir`, `loader.rs:246`). So the manifest's identity is its **parent
  directory's basename**, and the graph mints the node from the path, not from TOML.
- `plugin.toml` is deliberately **not** added to `packages::is_manifest_path`
  (`extractors/src/packages.rs:90`) — that branch runs the package-dependency +
  manifest-CLI machinery, the wrong shape. It gets its own route branch, mirroring the
  Dockerfile branch's shape (`engine/src/route.rs:795–805`: `synthetic_parse` with the
  extractor's nodes/edges/nav).

### The extractor

`extractors/src/plugin_manifest.rs`, one public fn in the `packages`-extractor style:

```rust
pub fn extract_for_path(source: &str, path: &str, module_id: NodeId, repo: RepoId) -> ExtractOut
```

(`ExtractOut` = the nodes/edges/nav triple the neighbouring extractors return; mirror
`iac.rs`'s output struct.) Behaviour:

- Fires only when the basename matches one of the repo's families' `manifest`,
  via `families.manifest_family(path) -> Option<&PluginFamily>` (maki: a
  `<dir>/plugin.toml`).
- Mints one **`PLUGIN` node** (kind 51), qname
  `plugin:<basename-of-parent-dir> @<host>` — the matched row's `name` as the LB.8
  owner segment, so two hosts' plugins can never collide on a NodeId — confidence
  Strong, with a POSITION cell and a CODE cell holding the manifest text
  (the synthetic-parse file-cell plumbing does this — mirror `is_manifest_path`'s
  branch, `engine/src/route.rs:808`, for how `module_id` / `repo` flow).
- Does **not** parse permissions into structure (§7 ruling 4); counts
  `[permissions]`-section keys only for the marker.
- Route branch order: before the `is_manifest_path` branch (a `plugin.toml` must never
  fall into the package machinery), and the branch is a `Routed::NonCode`-shaped
  synthetic parse exactly like Dockerfile's.
- **CORRECTED during L2 (2026-10-27), reworked 2026-10-29:** routing alone is
  not enough — the walk's `is_bypass_path` (`engine/src/walk.rs`) is the real
  extension gate, and it never read `.toml`, so `plugin.toml` was silently
  skipped until an admission line was added. The admission is now
  family-data-driven without the walk reading the families itself:
  `plan_walk` computes `plugin_families::walk_manifest_names(root)` once (the
  declared file's manifests, else the seed table's) and threads it through
  `walk_dir`, so a future architecture's manifest is admitted by its seed row,
  never by a walk.rs edit. The route branch decides with the repo's actual
  `PluginFamilies`; a file admitted but matched by no family costs one small
  read and routes nowhere. The store's freshness scan needs no mirror (it
  marks stale on any non-ignored file).

**Marker:** `[plugin-manifest] family=<name> plugin=<dir> perms_keys=<n> path=<p>`,
per file, silent with zero nodes minted.

**Tests (L2):** extractor units (name-from-dir, `[permissions]` count, non-`plugin.toml`
basename ignored) + one route-level test asserting the branch taken and the minted
qname (mirror the `manifest_cli` route tests at `engine/src/route.rs:2031–2065`).

**Acceptance L2:** extractor + route tests green; `glia build` on maki mints 35
`plugin:*` nodes (35 `plugin.toml` files — re-count at run time, §5); boundary pin now
asserts 52.

## 4. Packet L3 — the pairing resolver (`resolver:plugin`)

**Creates:** `graph/src/resolvers/plugin.rs`. **Edits:** `graph/src/resolvers/mod.rs`
(two lines), `engine/src/profile.rs` (one `resolver!` row), code-domain
(edge **38** `LOADS`, allocated here), `code-domain/src/profile.rs` (two CODE_TABLES rows).

### Registration (canonical recipe)

- `graph/src/resolvers/mod.rs`: `mod plugin;` + `pub use plugin::PluginResolver;`
- `engine/src/profile.rs` `CODE_PASSES`: `resolver!("plugin", PluginResolver)` after
  the `iac` row (`:181`), before the `models` PassSpec (`:187`) — a Resolve-stage pass.
- Trait (`graph/src/resolvers/mod.rs`, `CrossGraphResolver { fn resolve(&self, merged:
  &mut MergedGraph) }`); use the shared `RuleTally` for the marker and evidence cells
  (`rule_evidence("plugin", rule)` → emitter `resolver:plugin`).

### Rules (in this fixed order — the tally's order is the marker's)

1. **`same_dir`** (FACT). For each `PLUGIN` node (qname `plugin:<dir> @<host>`), pair
   every `PLUGIN_TOOL` node whose owner tail is exactly `@<host>/<dir>` in the same
   repo — emit `PLUGIN -LOADS-> PLUGIN_TOOL` (LOADS = 38 reused; **ruling 1,
   2026-10-27**: the load chain's second hop — the loader loads the plugin, the
   plugin provides the tool — so the carried walk crosses both hops under one
   name. The alternative weighed was a new REGISTERS category, declined on
   impact: zero id spend and one uniform chain weight beat exact vocabulary;
   the full record is on `edge_category::LOADS`'s doc). Exact match only; a
   `PLUGIN_TOOL` whose owner matches no `PLUGIN` (a registration outside any
   manifest directory) pairs nothing and counts in `ambiguous=`.
2. **`manifest_literal`** (DERIVED). The host side is a runtime directory scan, so the
   honest anchor is the manifest *name* in host source: scan Rust `FUNCTION` /
   `STATE_VAR` / `MODULE` CODE cells of repos that contain at least one `PLUGIN`
   node for the manifest basenames the graph itself carries — the
   `NavFact::PluginManifest` facts the route records (family, basename), read
   back from the graphs, so the resolver consults no table —
   each matching node `-LOADS->` every `PLUGIN` whose owner segment equals that
   family (edge **38** `LOADS`, host → plugin, evidence rule `manifest_literal`). Rationale:
   the loader loads *all* discovered plugins of its host, so loader→every-plugin is
   semantically true, not a guess; the tier is DERIVED because the connection is
   through a runtime scan the graph cannot see. **Ruling 2, 2026-10-27**: anchor
   breadth is accepted — the in-edge direction from a manifest is true impact on
   anything that reads manifests (manifest-validating tests, docs generators), and
   the out-edge noise direction is rare and ranked low; test-stamped exclusion and
   the host-node compression are the recorded escape hatches. The full directional
   analysis and the re-measurement recipe are on `resolver:plugin`'s `manifest_literal`
   doc (`graph/src/resolvers/plugin.rs`).
3. **`include_str`** (FACT). A Rust CODE cell containing
   `include_str!("…<path>.lua")` whose path resolves repo-relative to an existing
   `.lua` MODULE → `IMPORTS` edge (3) from the containing file's MODULE to that
   MODULE. This binds `maki-lua/src/docs_render.rs:21`
   (`include_str!("../../plugins/glob/init.lua")`).

**Marker** (once per resolve, silent with no edges):

```
[plugin] pairs=<p> same_dir=<s> loads=<l> include_str=<i> ambiguous=<a>
```

### CODE_TABLES rows (both one-line table edits, `code-domain/src/profile.rs`)

- `carry_edges` **+ `LOADS`** — so blast radius and trace cross host → plugin
  (a loader change reaches the plugins it loads) **and** plugin → tool (ruling
  1: a manifest change reaches the tools it declares).
- The entry rule's kinds **+ `PLUGIN_TOOL`** — a registration is an entry the host
  invokes at load time, so liveness seeds it and `dead_symbol` can call an
  unregistered tool dead. (`PLUGIN` is not an entry.)

**Tests (L3):** resolver units on synthetic graphs (the `channel_graph` helper in
`resolvers/mod.rs` tests) for all three rules + ambiguity; the L1 fixture extended
with a `src/loader.rs` stub containing the `"plugin.toml"` literal and one
`include_str!` so the engine test asserts a LOADS edge and the include_str IMPORTS.

**Acceptance L3:** resolver tests + `engine/tests/lua_plugin.rs` green; boundary pin
now asserts edge 39 / node 52.

## 5. The maki survey (recorded 2026-10-27 — do not re-survey; re-count only)

- Repo: `/home/brian/Projects/Personal/AI/Projects/maki`. Rust workspace (maki-lua,
  maki-interpreter, maki-pack, …) + `plugins/` = **153 `.lua` files** across 34 tool
  dirs + `plugins/lib/maki/` (the shared Lua API shim: 14 files) +
  `plugins/index/lang/` (34 per-language tree-sitter extractor modules written in Lua).
- Idiom counts (grep, whole `plugins/`): `maki.*` 703 · `require(` 260 — **all
  string-literal** · `local function` 833 · `function T.f()` 1023 · anonymous
  `function(` ≈1105 (test helpers + small callbacks) · `pcall` 32 (all on named
  targets) · `setmetatable` 4 · `coroutine` 4 · `vim.*` 0.
- Host anchors: `maki-lua/src/loader.rs:246` `fs::read_dir` (discovery);
  `loader.rs:740` `load_package(name, dir, …)`; the manifest-name constants
  `maki-lua/src/pack.rs:27` (`PLUGIN_MANIFEST`) and
  `maki-lua/src/plugin_permissions.rs:16` (`MANIFEST_FILE`) — the L3
  `manifest_literal` anchors hit these; `maki-lua/src/docs_render.rs:21`
  `include_str!("../../plugins/glob/init.lua")` — the `include_str` rule's live case.
- `plugin.toml` content: `[permissions]` (+ compatibility keys) — **no name field**.
- Require forms observed: `require("question_form")` (sibling),
  `require("maki.tool_view")` (dotted → `plugins/lib/maki/tool_view.lua`),
  `require("tests.helpers")` (test dirs), `pcall(require, …)` (rare, ignored).
- Registration forms: `maki.api.register_tool` / `register_options` / other
  `register_*` — **54 `maki.api.register*` sites**; `execute` / `restore` handlers are
  anonymous closures that dispatch into named helpers
  (`QuestionHelpers.render_card`, …), so the closure FUNCTION nodes carry real CALLS
  edges into the plugin's own modules.

## 6. Fixture layout (engine/tests/data/lua-plugin/)

```
lua-plugin/
  mytool/plugin.toml          # "[permissions]\nnet_hosts = [\"api.example.com\"]\n"
  mytool/init.lua             # require("helper") + require("maki.tool_view")
                              # + one maki.api.register_tool{name="greet",
                              #   execute=function(...) return helper.greet(..) end,
                              #   restore=function(...) end}
  mytool/helper.lua           # local function greet(name) ... end; return { greet = greet }
  lib/maki/tool_view.lua      # local ToolView = {} ; function ToolView.render() end ; return ToolView
  src/loader.rs               # fn load_all(dir: &Path) { let p = dir.join("plugin.toml"); … }
                              # const EXAMPLE: &str = include_str!("../mytool/init.lua");
                              # (the literal is the manifest_literal anchor; the include_str
                              #  line is the include_str rule's fixture)
```

Assertions (single engine test, `engine/tests/lua_plugin.rs`): 3 Lua MODULEs +
  (CORRECTED 2026-10-27: said 4; the tree this section fixes holds three `.lua`
  files, and the test asserts their three exact qnames.)
1 `plugin:mytool @maki`; `helper` IMPORTS same-dir, `maki.tool_view` IMPORTS lib-path;
`plugin_tool:greet @maki/mytool` with 2 handler FUNCTIONs and 2 HANDLED_BY edges;
`helper.greet` CALLS bound from the closure (require-alias rule); one LOADS edge
loader→`plugin:mytool @maki` (DERIVED); one `include_str` IMPORTS (the loader's
embedded `init.lua`); markers grep-able in the test's captured stderr.

The fixture is deliberately maki-shaped; host-genericity is proven by the
synthetic-row units in the parser and code-domain test suites (a test-constructed
`toy` host row through the registration scan and the table helpers), not by
shipping a second host.

## 7. Rulings requested (defaults proposed — proceed on defaults unless overruled)

1. Kind names **`PLUGIN` (51) / `PLUGIN_TOOL` (50)**. `EXTENSION` / `HOSTED_UNIT` are
   the alternatives; reusing `ROUTE` was rejected (it would leak into the http
   resolver's route index and the test-route predicates).
2. Edge **38 `LOADS`**, host → plugin (impact flows outward from the host, which is
   the blast-radius direction that matters). `EXTENDS` (plugin → host) is the
   alternative and would need the carry/PPR direction re-argued.
3. **The plugin family is the genericity seam** (`code-domain/src/plugin_families.rs`),
   **reworked 2026-10-29 to runtime discovery**: the declared
   `.glia/plugin_families.toml` (written by `glia plugin-families <repo>` via
   `--write`; the default dumps it to stdout, `--write --force` overwrites)
   is primary, inference from the tree next, and the static `seed_families()`
   serves that discovery only (recognition, the declaration template, the
   walk's admission union) — never a consumer lookup. Shipped seeds at v1:
   maki only. A new architecture is one seed row + a require-resolution check
   + fixtures, never new code paths; a family change moves `PARSER_STAMP` (it
   shapes what parses mint) and discards the parse cache through the
   `ParseContext` context key. The matching helpers take `&[PluginFamily]` /
   `&PluginFamilies` as parameters, so tests exercise synthetic families
   without shipping a half-supported one — and without global state (the
   first cut's `Mutex` + `test_set_hosts` global is gone: a build threads the
   families it read; tests pass rows as arguments).
4. `plugin.toml` `[permissions]` stays out of the graph (runtime security config,
   not structure) — counted only in the marker.
5. Plugin identity is the **directory**, with the family name as the qname owner
   (`plugin:<dir> @<family>`). A future family whose manifest declares its name gets
   an optional `name_field` on its row (read the field when present, fall back to the
   directory); not shipped until a family needs it — the maki row omits the field.

## 8. Out of scope (follow-ons, not this spec)

- **The `maki.*` host API as INTERFACE nodes** (reverse direction: Lua → host calls
  resolvable instead of bare external refs). maki-docgen already inventories the API;
  a snapshot step (the `scip import` pattern) could import it. Needs its own packet.
- **Neovim as family 2**: one `plugin_families` row (no manifest convention — the row
  would carry `register_prefixes` like `vim.api.nvim_create_user_command` and empty
  `manifest`) plus a runtimepath require rule — the L1 require resolver's lib-root
  step does not model `rtp`. Spec a row only with fixtures from a real config.
- The coverage-matrix rows for Lua (419-fixture matrix) — a separate grading packet.
- `glia watch` fast-path interactions: none expected (Lua files are ordinary Low
  durability paths once parsed) — verify in the demo, not by argument.

## 9. Full-tree acceptance (all three landed)

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
glia build /home/brian/Projects/Personal/AI/Projects/maki 2>&1 | grep -E '^\[(lua|plugin)'
glia blast-radius /home/brian/Projects/Personal/AI/Projects/maki maki-lua/src/loader.rs --top-files 10
glia trace /home/brian/Projects/Personal/AI/Projects/maki \
  'maki-lua/src/loader.rs::load_packages' 'plugin_tool:question @maki/question'
```

Expected on the maki demo: every Lua file parsed (`[lua]` lines, 0 errors);
35 `plugin:*` nodes; ~54 `plugin_tool:*` nodes paired by `same_dir`; ≥2 `manifest_literal`
LOADS sites; ≥1 `include_str` IMPORTS (`glob`); the blast-radius from the loader
reaches plugin files through the LOADS carry edge; the trace crosses
loader → PLUGIN → PLUGIN_TOOL → handler closure → the named helpers.

## 10. Packet L4 — run-time shape classification and detection (2026-10-27)

**Motivation (the five-ecosystem survey).** Sampled against real docs and
source — neovim (telescope's `plugin/telescope.lua`), WezTerm (plugins doc),
Luanti (`mod.conf` + `core.register_*`), Factorio (`info.json` + `control.lua`),
Hammerspoon (`SPOONS.md`): the registration-call space is a small closed set —
table-keyed (maki), positional (neovim/factorio/wezterm/hammerspoon), the
dotted + positional-name + handler-table hybrid (luanti), receiver-method
(hammerspoon objects; obsidian on the TS side) — while identity varies
(manifests in TOML/INI/JSON, `.spoon`/`plugin/` conventions) and the host side
is in-repo only for maki. A call's SHAPE is a property of the call (readable
without knowing the host); which callees are registrations is the host row's
knowledge. Detection exploits exactly that split.

**What landed:**

- **Row reshaped to call families** (`code-domain/src/plugin_families.rs`,
  renamed from `plugin_hosts.rs` 2026-10-29): `register_prefixes` +
  `tool_calls` became `calls: Vec<RegistrationCall> { prefix, shape, tools,
  handler_name }` — one registration API per family, each naming its `Shape`
  (`Table` shipped; `Positional { name_arg, handler_args, handler_table }`
  new) and its own tool suffixes. Owned data, not `&'static`: an inferred or
  declared family constructs the same shape a seed does.
  `call_for` / `call_of` / `mints_tool` match longest-prefix, ties keep the
  earlier row (the family-order tie rule, pinned by a test after
  `max_by_key` silently inverted it).
- **The positional reader** (`parsers/code/lua`): `mint_registration_positional`
  mints the tool from the string literal at `name_arg`, handlers from the
  anonymous functions at `handler_args` (member name = the family's
  `handler_name`, else `run`) and from a `handler_table` argument's named
  keys; everything else walks with the owner. Gated exactly like the table
  reader (a shipped or configured family; synthetic rows in tests — no
  shipped neovim row without fixtures, per house discipline). The tool and
  handler minting is factored (`mint_tool` / `mint_handler`) so both readers
  share it.
- **Candidate classification** (ungated, per call): a call matching no
  shipped family is classified structurally — a table with a `name` key and
  an anonymous-function field (Table), or a string-literal argument beside an
  anonymous function (Positional) — and recorded as
  `NavFact::Registration { callee, shape: ShapeTag, named }` on the file
  MODULE (`cands=` on the `[lua]` marker). Single-identifier callees stay
  out: a registration API is a namespaced host API, and measured on maki,
  every single-segment positional candidate was a local callback idiom
  (`case("pat", fn)` dispatch). The parse cache carries the facts for free —
  same file, same verdict.
- **The detection report** (`resolver:plugin`, report-only, zero graph
  change): candidates grouped by callee family (the dotted callee minus its
  last segment), each marked with the shipped host that covers it (a family
  covered when a shipped family prefix extends it); families with a single
  candidate summarize; the rest sort calls-descending, ties by family then
  shape, capped at 12. Pure `detection_report` returns the lines; `resolve`
  prints them (map order never reaches output).

```
[plugin-detect] candidates=<c> families=<f> reported=<r> singles=<s>
[plugin-detect] family=<f> shape=<table|positional> calls=<n> named=<m> files=<k> host=<name|->
```

**Tier:** a declared or inferred family is FACT/DERIVED; detection is
HEURISTIC-framed — advisory, never a gate. The report mints nothing: what an
unknown family *means* (is `buf.get_lines` a registration? no) is judgment,
and judgment lives in rows, not in structural sniffing. The report's
`host=` column is read off the graph's own owner segments (2026-10-29), not
off a table.

**Maki after L4:** `candidates=19 families=4 reported=2 singles=2` —
`buf` (14) and `R` (3) are helper idioms (`R.append_section_if_present("…",
fn)`), correctly uncovered (`host=-`); `[plugin]` byte-identical (993 pairs,
26 tools); `[lua-calls]` unchanged (2901). The graph is untouched — the whole
packet is extraction capability plus a report.

**Deferred (the graduation path, its own packet):** the `[[plugin_family]]`
overlay schema (then detection *proposes* through `overlay propose` → `try`
→ `accept`, graduating a sniffed host into checked-in config); advisory
minting for unknown hosts (needs a pseudo-host qname ruling — the callee's
first segment is the leading candidate); the receiver-method shape; shipped
neovim / luanti / factorio rows (each with real fixtures).

**Acceptance (ran):** `cargo test --workspace` 6602 passed (13 lua-parser,
10 resolver-plugin, 8 plugin_hosts); clippy `-D warnings` clean at the pinned
1.92.0; the maki build shows the markers above with `[plugin]` unchanged.

## 11. The 2026-10-29 rework (runtime discovery — record)

James's review of the first implementation ruled: runtime discovery is
preferred, static tables are not; the `.glia` file is created by an explicit
CLI call; any static table in this area serves generic runtime discovery and
must make future plugin architectures easy; leaked memory and the build-time
example-file writer go. What changed:

- **`plugin_hosts.rs` is `plugin_families.rs`** (`PluginFamily`,
  `RegistrationCall`): owned data, no `Mutex` global, no `test_set_hosts`, no
  cwd reads, no writes from a build. The deleted first cut wrote
  `.glia/plugin_hosts.toml` examples and inferred a host named after the cwd —
  both gone.
- **`ParseContext`** (`engine::extract::ParseContext`, sealed with a private
  field for the api_stability gate): the Go module set and the plugin families
  as siblings, threaded through `parse_repo_files` / `route_one` /
  `parse_one_with*` as one value. A future per-repo parse input is a field,
  not another parameter. It is in the parse-cache context key
  (`ParseCache::context_key`) and the shared-cache file key (for `go` and
  `lua` files), so a family change invalidates exactly the parses it shapes;
  the watch session full-rebuilds with reason `plugin_families`.
- **The families reach the resolver through the graph, not a parameter**:
  `NavFact::PluginManifest { family, manifest }` is recorded by the manifest
  route (a fact the fix to `engine::extract::merge_nav` now preserves — it
  dropped `nav_facts` on every merge, which is also the general lesson: a
  `CodeNav` field added anywhere must join both merge paths, the graph
  crate's and the engine's). The detection report's `host=` column reads the
  graphs' PLUGIN / PLUGIN_TOOL owner segments.
- **`glia plugin-families <repo>`** (the `inputs` area) writes the
  declaration (`--write`; `--write --force` overwrites; the default dumps it to
  stdout); a build never writes it.
- **The registry ids (the question of what "the boundary pin moved" means):**
  ids are allocated only in `code-domain/src/lib.rs`'s RESERVED blocks and are
  locked once allocated — they are baked into every `.gmap` on disk, so
  reusing or renumbering one corrupts old stores. This work spent the
  verified-free ids: node **50 `PLUGIN_TOOL`** (L1) and node **51 `PLUGIN`**
  (L2), edge **38 `LOADS`** (L3, reused for both load-chain hops per ruling
  1); cell 26 stays free. `lib.rs`'s boundary test used to assert the NEXT
  free node id (50) decoded as `"UNKNOWN"` — each allocation moved that pin
  forward (now 52), in the same commit as the emitter. Moving it is the
  allocation's proof; deleting it would leave the next id unguarded.
- **The fixture is feature-named** (`tool-registration/`,
  `manifest-permissions/`, `lib/maki/`, `host-loader/`, a README naming the
  source ecosystem per shape — "As employed in maki"), minimal by design, and
  no generated `.glia/` output is committed.
- **Gates at the rework:** `cargo test --workspace` (excluding
  glia-engram-export) green; clippy `-D warnings` clean on every file this
  work touches (five errors remain in `clojure`, `php` and two
  `extractors` files — verified present at HEAD, newer-clippy drift, not this
  branch's); `cargo fmt` applied to every touched file (repo-wide `fmt
  --check` also drifts at HEAD, same cause).

## 12. Current state — as the code stands (the write/dump split)

The last change to this work landed the `glia plugin-families` write surface as a
dump / write split, and this spec now tracks it:

- the default is **dump**: `glia plugin-families <repo>` prints the declaration to
  stdout (the module comment names the location/filename it belongs to,
  `.glia/plugin_families.toml`);
- `--write` is the opt-in **write**; `--write --force` overwrites an existing
  declaration; `--force` alone is refused;
- a build never writes the file, and inference still falls back to the seed.

The clap description, the README CLI block, and `cli/surface/plugin-families.txt`
were updated in the same change (the snapshot gained the `--write` arg).
