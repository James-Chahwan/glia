# neuropil handoff - glia 0.5.0 (LG.5b, 2026-09-20)

The first glia -> neuropil handoff. neuropil is glia's in-process Rust consumer: it path-depends
on six glia crates and calls `generate_one` at startup. Read this in the neuropil session and act
on neuropil only. It is the neuropil counterpart of `repo-graph-handoff-0.5.0.md` (LG.5a).
Engram's is LG.14. **0.5.0 breaks the contract on purpose.** glia, repo-graph, Engram and
neuropil move together at the bump, and nothing is pushed before James says go.

Sources, in order of authority:
- two runs of the LG.6d compile check, `scripts/check-neuropil.sh`, at glia HEAD `7412677`
  (after wave W34; BUILD_STAMP `0.4.18+pc3773758c5544fe0`). Both were repeated at `27f77ec`
  with the same counts and stamp: its two newer commits touch only engram-export and plan docs,
  so no crate neuropil links changed. Section 1 has the output verbatim. Every compile claim below
  comes from these runs.
- each packet's landed `breaking` declaration, read from the leap workflow journals;
- the specs (`dev-notes/leap-packets.json`, and Batch C in `dev-notes/wave-packets.json`) plus
  `dev-notes/leap-corrections.json`.

Every neuropil `file:line` below was read read-only in neuropil's working tree: HEAD `b140d1e`
with 50 dirty paths (23 modified, 27 untracked; the same state as LG.6d's baseline). Those
uncommitted edits shift lines, so grep for the symbol. Paths are relative to
`crates/neuropil-app/src/` unless they start with `crates/`, `docs/`, `tools/` or name a root file.

The packet inventory behind section 7 is generated. Re-run it with:

```
python3 dev-notes/leap-handoff/breaking_rows.py dev-notes/leap-packets.json dev-notes/wave-packets.json \
  --corrections dev-notes/leap-corrections.json --consumer /home/ivy/Code/neuropil --alias neuropil \
  --results '~/.claude/projects/-home-ivy-Code-glia/*/subagents/workflows/*/journal.jsonl' --handoff-to LG.5b
```

At this commit it prints `api=24 content=55 additive=26 unnamed_identity=2 not_landed=1
checked_no_change=24 in_scope=133`, which matches LG.5a's sanity run.

---

## 0. TL;DR

1. **neuropil does not build against 0.5.0 unchanged.** cargo stops at dependency resolution,
   because every `repo-graph-*` crate is now `glia-*` (LD.11a). Two mechanical edits (section 2)
   are enough: the rename script, plus the `driver` -> `research` feature rename (LD.12b). With
   them, neuropil checks with **new=0 errors** under `--all-targets`, measured in section 1. No
   other compile fix is needed.
2. **Four hand fixes that compile but behave wrongly** (section 3):
   - the hooks prompt (LG.2);
   - the readers of `route:` qnames, a shape that is gone (LB.4a / LB.5 / LB.11a / LB.11b);
   - kind-based tiering of role-folded components and services (LB.3a);
   - the label chains for the new registry ids.
3. **Persisted state re-keys once** (section 4.1-4.2):
   - **Every NodeId changes once** (LB.1), so node-anchored annotations in
     `.neuropil/annotations.json` and old flow recordings lose their nodes. LB.1's spec said
     nothing orphans. That claim was wrong.
   - The qname-keyed `view_state.json` and session bookmarks orphan entries whose qname moved.
4. **The build runs on a thread pool.** On neuropil's own repo it is 4.1x faster at boot, with
   byte-identical output. `GLIA_THREADS` caps the pool (section 4.4). No code change is needed.
5. **Install the 0.5.0 `glia` binary.** `~/.cargo/bin/glia` is 0.4.13, and it has no
   `install-hooks --pair`, which the new hooks prompt runs.

Bump order:
1. glia is bumped and tagged (James; see LG.5a section 0).
2. In neuropil: apply section 2, run `cargo check --workspace --all-targets`, apply section 3,
   then re-test (section 6).

## 1. The compile check at this commit (LG.6d)

**Run A: neuropil as it is.** Command: `bash scripts/check-neuropil.sh`

```
[neuropil-check] NEW   cargo: error: no matching package named `repo-graph-activation` found
[neuropil-check] FAIL - new=1 fixed=0 preexisting=0 in 0.4s against glia 0.4.18+p? (neuropil b140d1e, lock drift +11 -1)
```

Only a cargo-level error. rustc never runs, so this run cannot show any other break (the LG.6d
handoff predicted this). Its `lock drift` compares the check cache's copy of the lock, which
earlier waves' runs relocked, against neuropil's lock. It is not a change in neuropil. The error names the first renamed crate cargo resolves. After the
rename, the unrenamed `driver` feature fails next (`failed to select a version for
...projection-text`, measured by LD.12b).

**Run B: a copy with the two section 2 edits.** The same unmodified script, pointed at the copy
through `NEUROPIL_DIR`:

```
cd /home/ivy/Code/glia
C=~/.cache/glia-neuropil-renamed          # on disk: never /tmp (tmpfs), never inside neuropil
rm -rf "$C" && mkdir -p "$C/np"
cp -p ../neuropil/Cargo.toml ../neuropil/Cargo.lock ../neuropil/rust-toolchain.toml "$C/np/"
rsync -a --exclude=target/ ../neuropil/crates/ "$C/np/crates/"
python3 dev-notes/rename-0.5.0.py --apply --quiet --root "$C/np"     # non-git root: every file
sed -i '/^glia-projection-text = /s/"driver"/"research"/' "$C/np/Cargo.toml"
mkdir -p "$C/cc" && cp -a --reflink=auto ~/.cache/glia-consumer-check/neuropil "$C/cc/"  # optional warm target
NEUROPIL_DIR="$C/np" GLIA_CONSUMER_CACHE="$C/cc" bash scripts/check-neuropil.sh
```

```
[rename-0.5.0] rewrote 165 token(s) in 50 file(s) under /home/ivy/.cache/glia-LG.5b/recipe/np (0 unknown token(s) left)
[neuropil-check] note - neuropil differs from the baseline's (b140d1ede58ff6e77e8e7326a6a4cf21e822959e dirty=50 src=b690b6b4f6db -> unknown src=73b04787b8a9): NEW/FIXED may be neuropil's own edits, not the leap's
[neuropil-check] ok - new=0 fixed=0 preexisting=0 in 61.9s against glia 0.4.18+pc3773758c5544fe0 (neuropil unknown, lock drift +38 -26)
```

This output is from a run with `C=/home/ivy/.cache/glia-LG.5b/recipe`. The `note` line is
expected: the copy is not a git checkout, so it has no sha. The check covered:
- every neuropil target, including `neuropil`, `tier-audit`, `neuropil-audit`, `neuropil-hookc`,
  `shader-smoke`, `export-demo-gltf`, five libs and every test target;
- all 30 glia crates they pull in.

rustc emitted 102 messages, all warnings in neuropil's own code (unused and dead-code items).
None names a glia item.
**Pre-existing errors: 0.** The committed baseline (`dev-notes/neuropil-check-baseline.txt`,
glia `f6f007e`) has none, so every error at the bump is the leap's, and after section 2 there are
none.

This recipe is the compile check to use until neuropil lands section 2. `check-neuropil.sh` still
copies neuropil verbatim: its manifest rewrite was never extended with the rename (see the
followups of LG.6d, LD.11a and LD.12b). Once neuropil has moved, the plain
`bash scripts/check-neuropil.sh` is the check again.

## 2. Cargo edits and the mechanical rename (LD.11a, LD.12b)

| where | old | new |
|---|---|---|
| `Cargo.toml:112-116` | `repo-graph-{core,code-domain,graph,engine,activation} = { path = "../glia/<dir>" }` | `glia-<x> = { path = ... }` (the directory paths are unchanged) |
| `Cargo.toml:117` | `repo-graph-projection-text = { ..., features = ["driver"] }` | `glia-projection-text = { ..., features = ["research"] }` |
| `crates/neuropil-app/Cargo.toml:86-90, :94` | `repo-graph-<x>.workspace = true` | `glia-<x>.workspace = true` |
| `crates/neuropil-app/Cargo.toml:91-93` (comment) | "driver feature is enabled" | "research feature" |
| `crates/**/*.rs` | 151 `repo_graph_*` path tokens: core 113, code_domain 31, engine 2, graph 2, projection_text 2, activation 1 | `glia_*` |

Run it in neuropil:

```
cd /home/ivy/Code/neuropil
git add -N crates ONBOARDING.md docs/onboarding    # see below: the script walks `git ls-files`
python3 ../glia/dev-notes/rename-0.5.0.py --check --root .     # lists every rewrite; exit 1 = some pending
python3 ../glia/dev-notes/rename-0.5.0.py --apply --root .
sed -i '/^glia-projection-text = /s/"driver"/"research"/' Cargo.toml
grep -rnE 'repo[-_]graph[-_]' Cargo.toml crates        # expect nothing
cargo check --workspace --all-targets
```

**Untracked files are skipped.** In a git checkout the script rewrites only what
`git ls-files` lists. Today that is **138 tokens in 39 tracked files**. There are also 11
untracked `.rs` files holding 27 more tokens: `app_runner`, `cluster_naming`, `entry_tools_card`,
`flow_step`, `heartbeat_pill`, `pill_card`, `saved_requests`, `service_hex`, `silos`,
`tier_editor` and `view_state`. That makes 165 tokens in 50 files across `Cargo.toml` and
`crates/`, the count section 1's copy shows. Skipped, those 11 would keep `repo_graph_*` paths that
no longer resolve. `git add -N` (intent-to-add) makes `git ls-files` list them without staging any
content. Committing them first works too. The untracked `ONBOARDING.md` adds 3 more tokens
(:177-179).

**Cargo.lock** is not touched by the script, and cargo relocks it on the first build. Measured on
the check's copy, the lock goes +38 / -26 packages:
- the 26 `repo-graph-*` path packages become `glia-*`;
- four glia crates enter the graph: `glia-store` (LC.7), `glia-parser-kotlin` (A14.2), and
  `glia-stamp` and `glia-doc`. The last two predate the leap but postdate neuropil's lock;
- eight registry crates are added: `rayon 1.12.0`, `rayon-core 1.13.0` (LG.1a),
  `crossbeam-deque 0.8.8`, `crossbeam-epoch 0.9.21`, `ignore 0.4.32`, `globset 0.4.19`,
  `bstr 1.13.1` and `tree-sitter-kotlin-ng 1.1.0`. The first build needs the crates.io index.

`either` is already in the lock. neuropil's lock still pins `repo-graph-engine 0.4.13`. It is a
path dependency, so cargo rewrites that pin to the checkout's version.

## 3. Hand fixes: compile clean, behaviour wrong

### 3.1 Hooks prompt (LG.2): `hooks_install_prompt.rs`

glia's `install-hooks` (`cli/src/hooks.rs`) now writes these hooks:
- the rebuild hooks `post-commit`, `post-merge` and `post-checkout`, each running `glia build .`,
  which refreshes `<repo>/.glia/graph/`;
- with `--pair <sibling>`, the u151 branch-pair lock:
  - `pre-commit` runs `glia hook pre-commit --pair <sibling>`. It blocks the commit unless the
    sibling has the same branch checked out.
  - `commit-msg` writes `Glia-Pinned-At: <sibling HEAD>`.

Each managed hook starts `#!/bin/sh` then `# glia-install-hooks: managed`. The hooks go in the
directory `git rev-parse --git-path hooks` names, which honours `core.hooksPath` and worktrees.
The `--pair` path is relative to the work-tree top. Escape hatches: `git commit --no-verify`, or
`GLIA_BRANCH_PAIR=skip` (which skips the check but still writes the pin). Both scripts fail open
when `glia` is not on PATH.

| site | change |
|---|---|
| `:29` `GLIA_HOOK_MARKER = "# glia post-commit"` | `"# glia-install-hooks: managed"`. The old marker never matches, so `needs_install()` is always true today. |
| `:73-82` `needs_install` reads `repo/.git/hooks/post-commit` | Take the dir from `git -C <repo> rev-parse --git-path hooks`, joined onto the repo when relative. Consider requiring the marker in `pre-commit` too. |
| `:103-113` `try_install` runs `glia install-hooks <repo>` | `glia install-hooks <repo> --pair ../glia` |
| `:153` title, `:166-167` modal text `.git/hooks/{post-commit, pre-commit}` | Five hooks: `{post-commit, post-merge, post-checkout, pre-commit, commit-msg}`. The u151 claim is now true with `--pair`. |
| `:1-14` module doc | the same facts |
| `tools/check_branch_pair.sh` | Retire it: `glia hook pre-commit` supersedes it, and its trailer branch can never run from pre-commit. Nothing references it: no hook is installed in neuropil, and a grep outside `target/` finds no reference. |
| `docs/spec-vNext.md:517` u151 "Files: `glia/hooks/pre-commit.sh` extension" | That file never existed. It is `glia install-hooks --pair` + `glia hook pre-commit / commit-msg` (`glia/cli/src/hooks.rs`). |

`try_install` spawns `glia` from PATH. Reinstall it from the bumped checkout
(`cargo install --path ../glia/cli --locked`): the 0.4.13 binary rejects `--pair`.

### 3.2 HTTP qname readers (LB.4a, LB.4c, LB.5, LB.11a, LB.11b, LA.6b)

The 0.5.0 shapes are built by `glia_code_domain::endpoint`:
- ROUTE is `<METHOD> <path>`, with `ANY` for a method-agnostic registration. Since LB.11a / LB.11b,
  **no emitter produces `route:<path>`.**
- ENDPOINT is `endpoint:<METHOD>:<path>`.
- A nav page is `page:<path>`.

Any of them can end in ` @<owner>`, the nested project root the node lives under (LB.4a). Paths
are canonical, with a leading slash (LB.5). Strip the owner with
`glia_code_domain::endpoint::split_owner(q) -> (&str, Option<&str>)` before parsing. Reading the
kind (`ROUTE` / `ENDPOINT`) beats sniffing a prefix.

| site | today | fix |
|---|---|---|
| `flow.rs:333` `synthesize_step_transforms` | builds the snippet from `route:` only | parse `<METHOD> <path>` after `split_owner` |
| `live_node.rs:196`, `:210` | `endpoint:` (the owner leaks into the path) and `route:` (method defaults to GET) | `split_owner`, then parse `<METHOD> <path>` |
| `domain_zone.rs:825`, `:832` | the same two forms | the same |
| `code_viewer.rs:676` `parse_route_qname` | `route:GET /admin` only | `<METHOD> <path>` |
| `layout/tier_stack.rs:405` `group_of` | `starts_with("route:")` | group ROUTE nodes by the first path segment of `<METHOD> <path>` |
| `context_menu.rs:356` probe gate | `endpoint:` / `route:` prefixes (Go, TS, Java and Python routes all fail it) | `meta.kind == ROUTE \|\| meta.kind == ENDPOINT` |
| `openapi_catalog.rs:363` `parse_glia_route_qname` | form C already parses `GET /path` | call `split_owner` first, or paths read `/health @web`. Decide about `ANY`. `page:` stays unparsed: pages are not API routes. |
| `openapi_catalog.rs:508` test `glia_qname_forms` | slash-missing forms (`route:healthz`, `endpoint:GET:auth/login`) | These are legacy only now. Add owner-suffixed and `page:` cases. |

### 3.3 Role-folded kinds (LB.3a, LB.3b, LA.21a)

A component, service, hook, composable, directive, pipe or guard that has a declaration twin is
now that CLASS / FUNCTION, carrying a ROLE cell (cell 20). Kind switches no longer see it. Six
more languages populate roles (LA.21a). The one reader is
`glia_graph::roles::roles_in(Some(node.kind), &node.cells) -> Vec<NodeKindId>`. Compute it once
in `GliaGraph::load` (`state.rs:438`), for example as a `roles` field on `NodeMeta`, and tier from
it at these sites:
- `layout/tier_stack.rs:181` `kind_to_sub` (`:194-199` COMPONENT / SERVICE / HOOK / COMPOSABLE /
  DIRECTIVE / GUARD / PIPE);
- `file_tree.rs:245` `kind_priority` (`:249`);
- `view_state.rs:204` `tier_overrides` (keyed by `NodeKindId`: an override for COMPONENT no
  longer reaches folded nodes);
- `hud.rs` `glyph_for` (the COMPONENT / SERVICE glyphs);
- `bin/tier-audit.rs:160-161`.

### 3.4 Label chains for the new ids (L0.1, LA.6a, LA.8, LF.3b, LF.5b, LF.6c, LA.17)

| site | missing (leap ids) |
|---|---|
| `hud.rs:1516` `edge_category_name` (ends `:1549 else { "?" }`) | NAVIGATES_TO (35, LA.6a) and CO_CHANGES (36, LF.5b) |
| `hud.rs:1552` `cell_label` (ends `:1568 else { "CELL" }`) | node cells DOC_TAGS 19, ROLE 20, SCHEMA_FIELDS 22, COVERAGE 23, ENTRYPOINT 24. EVIDENCE 21 and ACCESS_MODE 25 are **edge** cells (LC.2 / LC.3a / LE.4a) and matter only if neuropil starts showing edge cells. |

A cheaper fix that also covers later ids: make each fallback the registry name, e.g.
`else { edge_category::name(c) }` and `else { cell_type::name(k) }`. Both are
`glia_code_domain` fns that return an upper-case `&'static str`, and both predate the leap.

**CO_CHANGES is a Weak, statistical edge** between modules whose files change together in git
history (LF.5b), not a flow. It appears only in a repo with a
`.glia/history-snapshot/`, which `glia history sync` writes (LF.5a / LF.5b).
`GliaGraph::load` builds `adjacency_out` from every edge (`state.rs:543-545`), so the default flow
animation (`GraphFlowSource`) would walk co-change pairs as if they were calls. Filter category 36
out of it.

Pre-existing gaps, not caused by the leap:
- the edge chain lacks IMPLEMENTS, SHARES_DATA_SOURCE and RPC_CALLS;
- the cell chain lacks ORIGIN, IMPORTS, MESSAGE_TYPE and RPC_PACKAGE;
- the kind chain (`hud.rs`, `else { "?" }` at `:1483`) and `code_viewer.rs:430-439`
  `is_route_like` lack GRPC_SERVER / RPC_PROCEDURE / RPC_CALL (47-49; LA.17 names
  `is_route_like`).

Node kinds did not change in the leap.

### 3.5 Prose that is now false

- `docs/onboarding/02-vocab-and-modes.md:77` (LC.9): the layout lives at `<repo>/.glia/graph/`
  (manifest + shards + parse cache, self-ignoring), written by `glia build .` and the rebuild
  hooks. `glia analyze` only prints.
- `ONBOARDING.md:47` and `docs/onboarding/01-architecture.md:15` say the `repo-graph-*` prefix is
  "intentional and locked" / "Don't rename". The crates are `glia-*` now.
- `view_state.rs:12-14` says NodeIds are not stable across rebuilds. After LB.1 they are stable
  for an unchanged kind + qname (section 4.1).
- `inject.rs:7-10` waits for "pub fn entry points". They exist now (section 5).

## 4. Behaviour changes with no required code change

### 4.1 NodeIds change once (LB.1)

`NodeId = xxhash(graph_type, repo, kind, qname)`. The `RepoId` in it was `xxhash("file://<path as
typed>")`. It is now derived from an identity key:
- `git:<normalised origin url>[/<path in checkout>]`;
- `gitdir:<main checkout dir>[/<rel>]` for a checkout with no remote;
- `dir:<basename>` outside git.

So **every NodeId changes once** at the bump. After that an id survives path re-spellings, clones,
linked worktrees and moved checkouts. A node whose qname or kind moves (section 4.2, LB.3a) also
gets a new id. neuropil persists NodeIds in two places:
- `annotations.rs:45` `Annotation.anchor` and `:55` `AnchorOrPoint::Node`, saved to
  `.neuropil/annotations.json` (`:144`). An anchor that no longer resolves falls back to the
  stored `world_pos` (`:279-282`). Anchored notes stop following their node, silently.
- `flow_replay.rs:35-47` `RecordedFlow` JSONL. Recordings made before the bump replay against ids
  that no longer exist.

The in-memory NodeId maps are rebuilt every session and are unaffected: `saved_requests.rs:189`
`LastResponses.by_node` and `flow_step.rs:98`. `SavedRequest` stores no NodeId.

### 4.2 qname moves: qname-keyed state orphans once

The places neuropil keys state by qname:
- `view_state.rs`: `extra_qname_prefixes` (`:131`), `hidden_nodes` (`:192`), `hex_collapsed`
  (`:210`), `hide_node` (`:316`) and `drain_hide_node_action` (`:356`);
- the session `bookmarks` table `(project_path, qname)`, written by
  `crates/neuropil-session/src/lib.rs:196` `add_bookmark` (listed at `:215`);
- `bookmarks.rs`, which resolves those bookmarks against the loaded graph.

Entries whose qname moved stop matching once. `service_hex.rs` builds its hierarchy from `::`
depth (`:91`, `:129-134`), so the depth-changing moves below re-nest hexes.

| family | packets |
|---|---|
| Java, Scala, PHP and Swift top-level types become package-scoped (`<dir>::Type`, no doubled file segment). They lose one `::` level and share their qname with the file MODULE, so hiding the class also hides the module | LB.2, LB.7a, LB.7b, LB.7c |
| C# file-scoped-namespace types take the namespace scope, and `file` types move under `<file module>::<Type>` | LB.7d, LB.14 |
| C / C++: each file is its own MODULE named by its file name (`src::Widget.h`). Header types take their C++ namespace name, and out-of-line `Q::m` definitions join the header's class | LB.10a, LB.10b, LB.10c |
| MODULEs of non-code files and of same-stem files are named by the full file name (`api::user.proto`) | LB.9a, LB.9b, LB.13 |
| queue, WS, GraphQL, gRPC, RPC and event sides inside a nested project gain ` @<project path>` | LB.8, LB.8b |
| HTTP routes, endpoints and nav pages (section 3.2) | LB.4a, LB.4c, LB.5, LB.11a, LB.11b, LA.6b (nested nav `page:/users` -> `page:/admin/users`), A14.5 (Ktor nested routes) |
| contract ops are scoped by their file (`contract::<file path>::<op>`), and spec-kit contract ops by their feature | LB.12, LE.9a |
| smaller moves | A13.2 (Java DATA_ENTITY gets a flavour prefix), A13.4 (`cli_invoke:` keys normalised), LA.4 (queues in const-topic repos), LA.18a / LA.18c (`ws_client:ws://` and `ws:default` become paths), LA.22b / LA.22c (phantom Feign and Clojure ROUTEs removed), LA.38 (demo-app `graphql_resolver:strawberry.type` / `.mutation` -> `graphql_resolver:Query` / `Mutation`), LF.2d (overlay `[constants]` re-keys endpoints in repos that opt in) |

To make qname state move-proof, store `glia_graph::identity::Identity::hint()` beside the qname
(LB.6), then after a miss call `IdentityIndex::build(&merged).rebind(qname, kind, hint)`. It
returns `Exact` / `Moved { tier }` / `Ambiguous` / `Orphan`. `Moved` is heuristic, and an
`Ambiguous` result must be shown to the user, never applied. This is optional.

### 4.3 Lines and spans

- LD.1's 1-based `line` applies to engine answer records only. neuropil reads POSITION cells
  directly (`position.rs`, `inject_panel.rs:441-446`), and POSITION payloads are still 0-based
  rows. **No change.**
- LG.10a: DOC_SECTION spans shrink to the section's own rows, so the Inject gold-set overlap at
  `inject_panel.rs:446` matches fewer doc nodes. Golden outputs that show doc spans will move.
- LA.37a: top-level Dart functions span their whole body, not just the signature line.
- LA.31 / LA.32a: tRPC nodes and Go routes gain POSITION, so file-keyed views can place them.

### 4.4 Engine thread pool (LG.1a, LG.1b, LG.1c)

`generate_one` now runs the walk's file reads, the per-file route / parse / extract, the
const-table scan, the RPC needle pass and per-language graph builds on a dedicated rayon pool,
`glia-parse-<i>`.

- **Size.** `GLIA_THREADS` sets it, read once per process. Unset or 0 means every core, and the
  value is clamped to 1..=256. `GLIA_THREADS=1` is the old single-threaded path, exactly. Output
  is byte-identical at any size.
- **Measured.** A debug `glia analyze` on neuropil (3890 nodes, 6806 edges) took 5.40 s with
  `GLIA_THREADS=1` and 1.33 s at the default 16, with max RSS 69 -> 94 MB. The JSON was
  byte-identical.
- **For neuropil the default is right.** `main.rs:254` builds the graph synchronously before
  `App::new()`, so Bevy's task pools do not exist yet.
- **The pool lives as long as the process.** Its idle workers stay parked. Each reserves a 16 MiB
  virtual stack; resident memory is unchanged.
- **Later rebuilds.** If neuropil ever rebuilds while rendering (today the watcher only counts
  changes: `watcher.rs:4-7`), cap the pool with `GLIA_THREADS` in the launch environment, or with
  `std::env::set_var` before the first build.
- **stderr.** Each build prints three `[parallel] ...` lines per repo (walk, routed, const-scan).
  Per-file parser markers now arrive in any order. `GLIA_THREADS=1` restores the sequential order.
- **Panic hook.** A parser panic is still caught per file. But glia no longer swaps the
  process-wide panic hook for a no-op during a build (the old `SuppressPanicHook`, which silenced
  every thread). Panics on neuropil's own threads reach its hook again.

### 4.5 Edges carry cells (LC.2, LC.3a-d)

`glia_core::Edge` lost `Copy` and gained `cells: Vec<Cell>`. Every edge now carries one EVIDENCE
cell (JSON: emitter, rule, file, 0-based line), and any edge left without one is stamped. neuropil
compiles unchanged, but:
- `GliaGraph::load` clones every edge, including that string (`state.rs:536-541`). It could hold
  just `(from, to, category)` instead.
- `Edge` equality and hash now include the cells. Compare edges across builds with `Edge::key()`.
- New code builds an edge with `Edge::new(from, to, category, confidence)`, which starts with no
  cells. Struct literals must list `cells`.

The engine and graph result types are `#[non_exhaustive]` (LD.9). neuropil only reads
`GenerateResult.merged` (`state.rs:443-444`, `bin/tier-audit.rs:21-22`), so the new fields
(`repo_labels` / `repo_roots`, LC.7 / LF.1b) cost nothing.

### 4.6 Graph content

Content only: no code change.

| what neuropil's views see | packets |
|---|---|
| fewer phantom nodes: GRAPHQL_* (`graphql_op:request` 1 -> 0), EVENT_* / EVENT_HANDLER (neuropil's 3 HANDLED_BY gone), DATA_ENTITY (neuropil 180 -> 114 distinct, LG.3b; its 106 `graph:*` phantoms -> 0, LA.28) | LA.26, LA.27, LA.29, LA.39, LA.41, LA.42, LA.28, LG.3b |
| more CALLS / USES / IMPORTS edges (Rust paths, Ruby, enums, Flutter: quokka_app CALLS 87 -> 133, TS aliases) | LA.1a, LA.1b, LA.23b, LA.30a, LA.34, LA.36a, A6.8 |
| edge endpoints move from a module to the function: `code_viewer.rs:535-547` "ACCESSED BY" now lists functions; hud `accesses_data` / `reads_config` rows (`hud.rs:1535`, `:1542`); queues gain function neighbours | LE.4a, LE.4b, LE.4c |
| duplicate and wrong-project edges removed: Java INJECTS, monorepo HTTP hops (read by `flow.rs`), coincidence-only WS_CONNECTS, Dart ENDPOINT-sourced CALLS | A7.2, LB.4b, LA.18b, LA.37b |
| new node families and containment: Kotlin symbols (arch `languages` gains "kotlin"), Rust inline modules, split-file Go methods under their struct, extra DOC_SECTIONs from ADRs, blast-carry SHARES_DATA_ENTITY, more TS heritage | A14.1, A14.2, LA.3, LA.23d, LF.4b, A13.1, A13.13, A6.3 |
| cells | VECTOR cells up to 64 KiB on nodes (`hud.rs` / `code_viewer.rs` already match `CellPayload::Bytes`; LF.1a). ORIGIN provenance gains `"excluded"` (LF.3a; neuropil reads no ORIGIN). REGION nodes under `.glia/` disappear in repos that ignore it (LF.1d). New coverage caveat text (A14.6). |

## 5. Optional: new in-process primitives

- **Inject scene** (`inject.rs:17-18`, `:90-106`): the two manual synth calls, plus the research
  passes the header waits for, can run as one
  `glia_activation::plan::ActivationPlan::new(ActivationConfig::default())` with `.synth(&..)`
  hooks, then `.synthesize(&g, &mut ActivatedView::from_ranked(..))`.
  - `glia_projection_text::hooks::AccessPathSynth` is ungated (LD.12b).
  - `hooks::CallsiteArgflowSynth` needs `research` (LD.12b).
  - So do `research::key_symbols::KeySymbolsSynth` (LD.12d) and
    `research::derived_notes::DerivedNotesSynth` (LD.12e).
  - `composition::synth_paths` / `render_cells` and `synth_callsite_argflow::run` stay public, so
    this change is optional.
- **Warm start.** `glia_engine::persist::load_or_rebuild(&default_layout_dir(repo), Some(repo),
  true)` loads the `<repo>/.glia/graph/` the rebuild hooks keep fresh. It rebuilds only when the
  layout is stale (LC.8). For hot reload, `generate_one_with_cache` reuses a `ParseCache`.
  `state.rs:443` still calls the uncached `generate_one`.
- **Extraction-only build.** `generate_one_opts(path, false,
  &BuildOptions::default().with_overlay(false))` skips `.glia/overlay.toml` inference (LF.2b).
- **CI.** `glia check` returns an exit code (the contract is in `glia check --help`; LE.8).

## 6. Re-test (neuropil's G24 protocol)

G24 comes from the glia-upgrade-spec. That plan file no longer exists, and this is the form glia's
memory recorded. Run these in neuropil after sections 2-3:

```
cargo check --workspace --all-targets && cargo test --workspace
cargo run -p neuropil-app -- .                     # boot: three [parallel] lines per repo on stderr
cargo run -p neuropil-app --features test-harness -- .   # in a second shell, then:
python3 tools/test_smoke.py
python3 tools/visual_regression.py compare
```

- Expect `visual_regression compare` to drift on scenes that draw glia content (sections 4.2 and
  4.6). Review the diffs, then re-record the baselines.
- The hook prompt should appear once, install five hooks with `--pair ../glia`, and not reappear.
- Any committed `.neuropil/annotations.json` or flow recording needs its anchors re-set
  (section 4.1).
- From glia, `bash scripts/check-neuropil.sh` should then report `new=0`, plus the `note` that
  neuropil moved from the baseline's sha.

## 7. Done vs pending

**Done on the glia side.** Every packet below has landed on local glia main (W0-W34), and the
section 1 run shows neuropil compiling once section 2 is applied. The inventory lists each packet
that names neuropil:
- **Section 2 and the compile check:** LD.11a, LD.12b, LD.14a (the lock adds activation to
  code-domain), LC.7 (pulls in glia-store) and LG.6d.
- **Compile clean, checked (section 1):** LC.2, LD.9, LC.7, LF.1b, LA.35a (a new pub field
  `CodeNav.local_types`, and `recv_stats::LANGS` is now `[&str; 8]`; neuropil uses neither),
  LD.10 (`project_name` moved; `cluster_naming.rs:29` has its own), LE.4d, LD.3a, LD.4a, LD.8a,
  LA.20a, LC.3a, LC.3b and LC.10c.
- **Section 3:** LG.2 (3.1); LB.4a, LB.4c, LB.5, LB.11a, LB.11b and LA.6b (3.2); LB.3a, LB.3b
  and LA.21a (3.3); L0.1, LA.6a, LA.8, LA.17, LF.3b, LF.5b and LF.6c (3.4); LC.9 (3.5).
- **Section 4:** LB.1 (4.1); the section 4.2 table; LG.10a, LA.37a, LA.31 and LA.32a (4.3);
  LG.1a, LG.1b and LG.1c (4.4); LC.2 (4.5); the section 4.6 table.
- **Section 5 (optional):** LB.6, LD.12d, LD.12e, LF.2b and LE.8.
- **Specced but not landed:** LA.43 (not-needed).
- **Checked, no change** (24): A7.3, L0.2, L0.3, L0.4, LA.25a, LC.1, LC.4, LC.5b, LC.8, LC.10a,
  LC.10b, LD.1, LD.2, LD.5, LD.6, LD.7a, LD.7b, LD.11b, LD.12a, LD.12c, LD.13, LD.14b, LD.15a
  and LE.1a.

**Pending:**
- **The neuropil session:** sections 2, 3 and 6. Section 4.1's re-anchoring is a one-time user
  step.
- **James:** the glia 0.5.0 bump and tag, then `cargo install --path cli` so `glia` on PATH
  accepts `--pair`.
- **glia, unowned:** `scripts/check-neuropil.sh` still copies neuropil verbatim, so before
  neuropil moves it reports only the cargo resolution error. Section 1's `NEUROPIL_DIR` recipe
  stands in for the rewrite that LG.6d, LD.11a and LD.12b asked for.
- **glia, still to land:** LG.14 (the Engram handoff) and the rest of LG (LG.10, engram-export,
  committed in W35 as `68c519e`). None of them names neuropil.
