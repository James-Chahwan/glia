# glia

A code graph engine that answers questions about code. It parses one repo or many (17 languages, the common web, RPC, queue and data frameworks across them), joins them into one graph with cross-service edges (an HTTP client to its route, a gRPC stub to its service, a producer to its consumer, two services reading one table or one env var), stores it in a zero-copy file, and answers from it: what calls this, what breaks if I change it, which tests cover it, why is this edge here, what did this diff do, which services talk to which.

Every answer row is located (`file:line`), carries an evidence tier (`fact`: read at a site; `derived`: paired by a resolver or a pass; `heuristic`: a name guess, an overlay, git history, a similarity), and an empty answer says what the graph could not see rather than returning nothing. Every edge records the extractor, the rule and the exact line that produced it.

Rust engine. CLI `glia`, Python wheel `glia-py` (`import glia_py`; formerly `repo-graph-py`, up to 0.4.18), and the MCP server [repo-graph](https://github.com/James-Chahwan/repo-graph), which wraps the wheel for coding agents.

> **Licensed [Glia Software License v0.1](./LICENSE).** PolyForm Noncommercial 1.0.0 + worker-protection overlay. Free for individuals, students, researchers, nonprofits, OSS projects, orgs <500 STEM workers, worker-owned coops, B Corps, unionized workplaces. Commercial license required otherwise. Contact `j.r.chahwan@gmail.com`. Not OSI-approved by design. See [License](#license).

## Install

```
# CLI, no build (Linux / macOS / Windows, x86_64 + arm64)
uvx --from glia-cli glia --version     # run without installing: uvx --from glia-cli glia find <repo> X --json
pipx install glia-cli                  # or install the `glia` command

# CLI from source (Rust 1.95+)
git clone https://github.com/James-Chahwan/glia
cd glia
cargo build --release -p glia-cli
cp target/release/glia ~/.local/bin/

# Python wheel (works for scripts and the MCP server)
pip install glia-py          # abi3 pyo3 wheels for Linux / macOS / Windows (x86_64 + arm64), import glia_py
```

`glia embed` (and `glia_py.embed`) runs a local model on the GPU where one works and falls back to the CPU: Linux x86_64 (glibc >= 2.38 for the GPU worker), Windows x86_64 and macOS arm64 >= 13.4 use the GPU, Linux aarch64 the CPU, and Intel macOS has no embeddings (no ONNX Runtime build exists for it). Devices, fallbacks and speeds: [docs/embeddings.md](./docs/embeddings.md). A build uses one thread per core (`GLIA_THREADS=N` to change it) and is byte-identical at any thread count.

For LLM / MCP usage see [repo-graph](https://github.com/James-Chahwan/repo-graph), which wraps the wheel as an MCP server with six tools: `orient`, `find`, `impact`, `trace`, `read` and `refresh`.

**Use without MCP.** An agent can also call the CLI directly, one `glia <command> <repo> --json` per question. `skills/` holds one family of Claude Code skills that teach this. [`skills/glia/SKILL.md`](./skills/glia/SKILL.md) is the CLI router: it maps each question to its command and says how to read an answer (`file:line` rows, absences, blind spots), with `reference.md` (every field and exit code) and `examples.md` (a worked run of each command) beside it, read on demand. Six task skills, moved from repo-graph in 0.5.4, carry its task steps: [`glia-orient`](./skills/glia-orient/SKILL.md), [`glia-find`](./skills/glia-find/SKILL.md), [`glia-debug`](./skills/glia-debug/SKILL.md), [`glia-impact`](./skills/glia-impact/SKILL.md), [`glia-trace`](./skills/glia-trace/SKILL.md) and [`glia-init`](./skills/glia-init/SKILL.md). Each names three transports, used in this order: the repo-graph MCP tools when they are loaded, the `glia` CLI (or `uvx --from glia-cli glia ..` with nothing installed), else `uvx mcp-repo-graph <verb>`. [`skills/glia-overlay/SKILL.md`](./skills/glia-overlay/SKILL.md) is the model step of the overlay loop: it reads the gaps `glia overlay propose` lists, writes candidate `.glia/overlay.toml` stanzas from the code, measures them with `glia overlay try` and accepts only what the verdict keeps. To install a skill, copy `skills/<name>/` to `~/.claude/skills/<name>/`, or to `<repo>/.claude/skills/<name>/` for one repo. No server stays resident: each call builds the graph in memory and exits. `skills/` follows this repo's [LICENSE](./LICENSE) (the Glia Software License), like the engine. `cli/tests/skill_surface.rs` checks every command and flag the skills name, in the `glia ..` and `uvx --from glia-cli glia ..` forms, against `cli/surface/`; the MCP calls and `uvx mcp-repo-graph` are repo-graph's surface and are not checked here.

## Quickstart

```
uvx --from glia-cli glia health ./my-repo      # can glia see this repo's boundaries? OK / DEGRADED / BLIND per mechanism
uvx --from glia-cli glia find ./my-repo Invoke  # then any question, one command each; add --json to parse
```

Real output, glia 0.5.2 on [grpc-go](https://github.com/grpc/grpc-go) (14,296 nodes, built in about a second):

Real output, glia 0.5.2 on [grpc-go](https://github.com/grpc/grpc-go) (14,296 nodes, built in about a second):

```
$ glia find grpc-go Invoke
| match       | live | kind     | qname                                         | location                |
| exact_name  | ●    | METHOD   | `clientconn::ClientConnInterface::Invoke`     | clientconn.go:573       |
| exact_name  | ●    | METHOD   | `call::ClientConn::Invoke`                    | call.go:29              |
| exact_name  | ●    | METHOD   | `balancer_wrapper::acBalancerWrapper::Invoke` | balancer_wrapper.go:320 |
| exact_name  | ⊘    | FUNCTION | `call::Invoke`                                | call.go:59              |
...

$ glia blast-radius grpc-go call::ClientConn::Invoke --depth 2
| score  | depth | live | via   | kind     | qname                                               | location                  |
| 0.0310 | 1     | ●    | CALLS | FUNCTION | `call::invoke`                                      | call.go:65                |
| 0.0211 | 1     | ⊘    | CALLS | FUNCTION | `call::Invoke`                                      | call.go:59                |
| 0.0197 | 1     | ●    | CALLS | FUNCTION | `call::combine`                                     | call.go:40                |
| 0.0190 | 1     | ●    | CALLS | METHOD   | `test::end2end_test::s::TestMethodFromServerStream` | test/end2end_test.go:4921 |
...

$ glia why grpc-go call::ClientConn::Invoke call::invoke
| category | confidence | tier | emitter           | rule          | site       |
| USES     | strong     | fact | graph:go_packages | func_value    | call.go:35 |
| CALLS    | strong     | fact | graph:calls       | module_symbol | call.go:37 |

$ glia tests-for grpc-go call::invoke
- tests: 10 (fact 0, derived 10, heuristic 0) in 5 files
| # | tier    | test                                              | at                              | via |
| 1 | derived | `...grpclb_test::s::TestGRPCLBStatsUnaryFailedToSend` | balancer/grpclb/grpclb_test.go:1498 | `TestGRPCLBStatsUnaryFailedToSend` -[CALLS]-> `Invoke` -[CALLS]-> `invoke` |
...
```

Optional snapshot steps make some answers sharper; each writes under `<repo>/.glia/` and none runs inside a build: `glia history sync` then `glia history calibrate` (co-change and the repo's own calibrated ranking in `blast-radius`, `diff-impact`, `review`, `pack` and `find --files`), `glia embed` (semantic similarity blended into `find --files`) and `glia vocab sync` (an empty `find` names where an identifier occurs that no node models). Several services in several repos: pass the others with `--with <repo>`.

## What it answers

`●` is reachable from an entry point (a route, a `main`, a test, a consumer); `⊘` is not. The rest of the answer surface, each a CLI command, a pyo3 method and (through repo-graph) an MCP mode:

- **A change:** `delta` (graph diff against a git rev), `diff-impact`, `review` (one PR report: impact, tests to run, every added or removed edge with its tier, rule violations new vs resolved, client / server model breaks), `tests-for`, `contract-breaks` (OpenAPI / proto / Avro / AsyncAPI field evolution against a rev, and the field changes between a frontend model and the backend struct it reads, advisory by default), `cochange` (files that usually change with these), `watch` (keep the build open and stream what each edit did to the graph).
- **Structure:** `trace` and `flows` (entry point to sinks, across services), `serves` (who handles a channel), `implementors`, `arch` (services and the links between them), `identity` (what is one thing across repos: a package, a message, a contract op, a SCIP symbol, a frontend model and the backend struct it mirrors, a vendored copy), `contracts --fields` (where a client's model and the server's, an OpenAPI op or a GraphQL schema disagree, field by field), `communities`, `splits` (where a service could be cut), `hubs`, `hotspots` (churn x centrality), `duplicate-flows`, `cycles`, `timeline` (edges over git history), `pack` (the ranked neighbourhood of a question, fitted to a token budget).
- **Rules and queries:** `check` (forbidden edges and paths, cycles, Datalog rule invariants, declared components and layers; each rule's `min_tier` decides what fails CI), `query` (your own Datalog, or a GQL read query, over the graph: every row located and tiered; [docs/rules.md](./docs/rules.md)), `patterns` (the handler convention a population follows, and who diverges), `effects` (the data, config and external calls a function reaches), `flags` (dead, undefined and stale feature flags), `spec-status` (which declared operations a route implements).
- **What the graph cannot see:** `health` (per boundary mechanism, how much of each side pairs: OK / DEGRADED / BLIND; run it first on an unfamiliar repo), `coverage` (per-language blind spots), `gaps` (orphan endpoints, dead symbols, co-change pairs with no edge, rules this glia can no longer read), and an overlay loop (`overlay propose / try / accept`) that turns a gap into a measured `.glia/overlay.toml` stanza, and judges a rule an agent proposes by the violations it finds on the code as it is.
- **Inputs from outside the source:** git history, test reports and lcov, SCIP indexes, Confluence / Notion / MediaWiki / local docs, a shared parse cache, node embeddings from a local model (`glia embed`) and the repo's identifier vocabulary (`glia vocab sync`). Each is a snapshot written by its own step, read by the next build or at query time; a build never fetches, syncs or embeds.

The full command list with every flag is in [CLI](#cli).

## CLI

Every subcommand and flag of `glia`, written from the committed CLI surface snapshots in `cli/surface/` (LG.6a) and held to them by `cli/tests/readme_surface.rs`: every command has a usage line, every flag a line shows is declared and every flag a command declares is shown, and every `a|b` value set is the snapshot's. `glia <command> --help` has the full text. `--with <repo>` (repeatable) merges more repos in first, so the resolvers pair across them; `--json` prints JSON instead of tables; `--scope` takes a repo-relative path or a project label from `glia projects`. The global `--no-overlay` builds without `.glia/overlay.toml`'s `[[edge]]` stanzas, the extraction-only graph ([docs/overlay.md](./docs/overlay.md)). The answers that collapse identity classes (`find`, `blast-radius`, `trace`, `flows`, `tests-for`, `serves`, `arch`) show a FACT / DERIVED class once, its best member carrying `identity` (`tier`, `rules`, `diverged`, `also_in`: the other repos' copies, located); `--no-collapse` is the raw answer, one row per member, with no identity tag. `glia query` and `glia check`'s `rule` read the Datalog dialect and the GQL subset in [docs/rules.md](./docs/rules.md). Tier words print lowercase on every surface (`fact`, `derived`, `heuristic`), and an answer that read a boundary mechanism `glia health` rates BLIND or DEGRADED carries a `blind` row naming it.

```
# Build, merge, store
glia build <REPO> [--no-incremental] [--out <OUT>]
    Walk a repo and write its graph layout (manifest.json + per-language .gmap
    shards + cross_stack.gmap) to <repo>/.glia/graph/, the directory the MCP
    server and load_from_gmap read. Incremental through the parse cache beside it.
glia analyze <REPO> [--format summary|mermaid|json]
    Walk one repo and print node kinds + cross-graph edges. `mermaid` is the
    service graph (as `glia arch --mermaid`), `json` the full nodes + edges.
glia merge [REPOS]... [--gmap <DIR>]... [--incremental] [--layout <DIR>] [--out <OUT>] [--workspace <FILE>]
    One MergedGraph across N repos, or across pre-built layouts (--gmap, or a
    glia.workspace.json via --workspace) without their sources; the resolvers
    fire across repo boundaries. --layout writes the merged layout, --out JSON.
glia inspect <PATH> [--json]
    What a .gmap file or layout directory holds, every id named from the file's
    own header. Exits 1 on an unreadable or old-format file ("rebuild the graph").
glia install-hooks [REPO] [--command <COMMAND>] [--pair <PAIR>] [--uninstall]
    Opt-in post-commit / post-merge / post-checkout hooks that rerun `glia build .`
    (or --command), written where git reads hooks: core.hooksPath, else the common
    dir, so one install serves every worktree of the repo. --pair <sibling> adds a
    branch-pair lock: pre-commit blocks unless the sibling has the same branch
    checked out, commit-msg pins `Glia-Pinned-At: <sibling HEAD>`. Bypass once with
    `git commit --no-verify` (both hooks) or GLIA_BRANCH_PAIR=skip (the check
    only). Needs git >= 2.13. Never overwrites hooks glia did not write.
glia cache push <REPO> <STORE> [--json] [--key-file <FILE>] [--layout] [--unsigned]
    Upload a built checkout's parse cache to an object store, one signed object per
    content address, skipping what the store holds. STORE is a directory or an
    https:// URL (PUT, `Authorization: Bearer $GLIA_CACHE_TOKEN` when set; plain
    http:// to a loopback host only). The MAC key is GLIA_CACHE_KEY or --key-file;
    --unsigned trusts the store as-is. --layout first uploads the whole .glia/graph/
    layout of a clean checkout whose layout is fresh. See docs/cache.md.
glia cache pull <REPO> <STORE> [--jobs <JOBS>] [--json] [--key-file <FILE>] [--layout] [--unsigned] [--verify <N|all>]
    Fetch the parses this checkout's cache lacks, check each object (size, key,
    MAC), re-parse a sample (--verify; 32 by default when --unsigned) and write
    <repo>/.glia/graph/parse_cache.bin, which the next build reuses. Exits 1 and
    writes nothing when a sample differs from a local parse. --layout first
    installs the finished layout the store holds for this clean checkout, keyed by
    build stamp, repo identity, HEAD tree, .glia inputs, target and walk digest.
    The build itself never touches the network.
glia cache gc <STORE> [--json] [--keep-stamps <KEEP_STAMPS>] [--max-bytes <BYTES>]
    Prune a directory store: keep the newest --keep-stamps build stamps, then drop
    the oldest objects over --max-bytes (K / M / G suffixes).

# The whole stack
glia arch <REPO> [--include-shared] [--json] [--mermaid] [--no-collapse] [--with <WITH>]...
    The services and the cross-service links between them, each with its mechanism
    and channel. One repo keys services by top-level directory; SHARES_*, SAME_AS and
    DOCUMENTS links show only with --include-shared. A SAME_AS identity class is one
    link per service pair at its weakest edge, and two copies of one class count once
    in every other link; --no-collapse shows the raw SAME_AS star.
glia projects <REPO> [--json] [--with <WITH>]...
    The manifest-rooted sub-projects (label, ecosystem, path): the --scope vocabulary.
glia coverage <REPO> [--json] [--with <WITH>]...
    Per-language extraction caveats and edges found, plus the co-change-without-edge
    audit, so a grep fallback is a deliberate choice. Two (*, DOCUMENTS) rows: the
    contract-JSON limits, and which markdown is read at all (the well-known files and
    docs/ trees at the repo and project roots, .ai/, ADR dirs, SDD feature docs) with
    the single-backtick mention rule.
glia gaps <REPO> [--category <CATEGORY>] [--json] [--overlay-delta] [--top-k <TOP_K>] [--with <WITH>]...
    Ranked blind spots (unpaired / ambiguous / unresolved endpoints, uncalled routes,
    tag-only queues, dead symbols (never an implementation of a method something
    calls, which dispatch reaches), co-change without an edge, suspected edges,
    overlay rot: orphaned and redundant rules, and rotted_rule, a [[constraint]] whose
    `rule` this glia can no longer read), each with a stable gap id and the overlay
    section that could repair it. suspected_edge rows (heuristic) print a paste-ready [[edge]] stanza
    after their table ("paste into <repo>/.glia/overlay.toml ..."). --overlay-delta:
    what an overlay edit did, with a keep / review / drop verdict. Exits 0. Not rows:
    a route a WebSocket client connects through, a route only test code registers
    (test files, `[walk] tests`), a GraphQL client's transport endpoint when the
    project's operations pair a resolver on a server with no route at its path (any
    other unpaired transport says `graphql transport; ` first), and as dead symbols a
    used class's lifecycle hooks, framework-called members and constructors, and
    generated code.
glia contracts <REPO> [--breaking-only] [--fields] [--json] [--mismatch-only] [--with <WITH>]...
    Per queue topic, producer vs consumer message type. --fields diffs declared fields
    (schema copies, topics, AsyncAPI channels, OpenAPI vs Pact) and the inferred
    contract models: http_model (each client call site's model against the server
    model of its HTTP exchange, response and request: a field the client reads that
    the server never writes, one it requires that the server may omit, a type
    mismatch), client_contract (a client against the OpenAPI op documenting its route)
    and graphql_selection (an operation's selection against the schema type its root
    field returns). Notes `open` / `union` ride on a verdict; a row that compared
    nothing is unknown, saying why (ambiguous_server, client_type_unresolved, ...).
glia pages <REPO> [--dead-only] [--json] [--with <WITH>]...
    Frontend router pages, the links between them, dead deep links, unlinked pages.
glia flows <REPO> [--depth <DEPTH>] [--feature <FEATURE>] [--features] [--group-by feature|entry] [--json] [--no-collapse] [--out <OUT>] [--scope <SCOPE>] [--with <WITH>]...
    Every entry point's forward flow: reach, mechanisms, whether it crosses a service,
    keyed by the feature word `glia trace` takes. --features / --out group them into
    per-feature records (<feature>.yaml + index.json). A hop through an identity class
    shows its best member; --no-collapse (not with --features / --out) walks the raw graph.
glia spec-status <REPO> [--feature <FEATURE>] [--json] [--status implemented|declared_missing|undeclared] [--with <WITH>]...
    Per feature, the declared API ops (OpenAPI, feature.yaml) a route implements, the
    ones still missing, and routes no op declares. Each implemented row says how it
    paired: exact, server_base (another server the document lists), annotated_base (the
    project's swaggo @BasePath, for a document with no base) or prefix (an inferred strip).
glia cycles <REPO> [--json] [--kind event|import|all] [--scope <SCOPE>] [--with <WITH>]...
    Cross-service event loops with a located witness, call loops, service-level
    possible loops, then module import cycles.
glia communities <REPO> [--json] [--members <MEMBERS>] [--method leiden|lpa] [--resolution <RESOLUTION>] [--scope <SCOPE>] [--seed <SEED>] [--top <TOP>] [--with <WITH>]...
    The graph's communities by seeded Leiden (label propagation above its pair cap,
    or exactly --method), largest first: each with its size, cohesion, node kinds,
    `glia arch` services, effect sinks, located entry points, top members and
    heaviest links to other communities. Heuristic. A report: exits 0.
glia splits <REPO> [--from <FROM> --to <TO>] [--json] [--min-share <MIN_SHARE>] [--parts <PARTS>] [--quotient module|community] [--scope <SCOPE>] [--seed <SEED>] [--with <WITH>]...
    Where a scope splits into services at the least coupling: the module (or
    community) quotient cut by Stoer-Wagner into --parts parts (2..=8) above a
    balance floor, or with --from / --to the minimum cut separating two sides. The
    parts, the cut edges located at their evidence sites, the blockers (data
    entities two parts write, cycles between parts) and each part against
    `glia arch`. A suggestion, never a verdict (heuristic); exits 1 with no cut.
glia hubs <REPO> [--category <CATEGORY>] [--include-tests] [--json] [--min-degree <MIN_DEGREE>] [--scope <SCOPE>] [--top <TOP>] [--with <WITH>]...
    The nodes carrying the most structural load, three ranked lists: fan-in
    (utilities), fan-out (orchestrators) and cross-service connectors, each located
    with its degree, HITS authority / hub scores and label. Counts every carry edge
    but tests, docs and manifest dependencies, or one --category; test nodes only
    with --include-tests. Derived. Exits 1 with no rows.
glia duplicate-flows <REPO> [--depth <DEPTH>] [--include-tests] [--json] [--keep-hubs] [--min-size <MIN_SIZE>] [--scope <SCOPE>] [--threshold <THRESHOLD>] [--with <WITH>]...
    Entry points whose forward flows reach the same nodes: exact groups (derived: an
    aliased route, a v1 / v2 pair never retired) and near groups (heuristic:
    MinHash / LSH, verified at Jaccard >= --threshold, default 0.8), each with the
    nodes it differs by. A report: exits 0, 2 on a --threshold outside (0, 1].
glia hotspots <REPO> [--include-tests] [--json] [--level module|symbol|both] [--min-churn <MIN_CHURN>] [--scope <SCOPE>] [--top <TOP>] [--with <WITH>]...
    Modules and symbols that change often AND sit where much depends on them: git
    churn rank beside PageRank centrality rank (the `centrality` preset), no
    composite score. Reads the history snapshot (`glia history sync`, `--blame` for
    symbols). Heuristic. Exits 1 with no rows (no_history / no_match).
glia identity <REPO> [--diverged-only] [--json] [--limit <N>] [--rule <RULE>] [--tier fact|derived|heuristic] [--with <WITH>]...
    The identity classes: what the build says is one thing across repos (a package, a
    message, a contract op, a SCIP symbol, a client model and its server model or
    GraphQL schema type: fact / derived, collapsed in answers) and the same-as
    candidates (vendored copies: heuristic, never collapsed), by rule and tier,
    each member located, diverged copies with their variants. Exits 0 with classes, 1
    with none (the absence says why), 2 on an error.
glia health <REPO> [--json] [--with <WITH>]...
    Whether glia's answers about this repo can be trusted, before asking one: per
    boundary mechanism (http, grpc, rpc, graphql, queue, ws) the clients, servers and
    pairs, how much of each side is paired and a verdict OK / DEGRADED / BLIND; then
    the code files no parser read, workspace packages imported by name but bound to
    no in-repo module, suspicious endpoint / route qnames and the dead-symbol share.
    The verdict is the worst row; a library is never judged on reach. Run it first on
    an unfamiliar repo. A report: exits 0, 2 on an error.

# One symbol, channel or signal
glia find <REPO> <QUERY> [--files] [--json] [--kind <KIND>]... [--no-collapse] [--no-semantic] [--scope <SCOPE>] [--top-k <TOP_K>] [--with <WITH>]...
    The ranked, located nodes a symbol, qname or fragment names, with the match tier.
    One row per identity class, folded before the --top-k cut. --files ranks one repo's
    files for the query text instead: ids in the text first (through its task / requirement
    rows and trackers), then past commits whose messages match it (glia history sync).
    Ranked by the repo's calibrated pick (glia history calibrate), else ids first, then
    text; the table ends with `ranked by <pick> (..)`, --json adds `selection`. One repo
    (--with exits 2); a query that routes no id and matches no message is an absence
    naming the missing input, exit 0. When the repo holds vectors of the surface model
    (glia embed; GLIA_EMBED_MODEL, default bge-small) the text key blends in their
    similarity, LX[IDR,FUSE[T1s,SEMp:t2]], and the table gains a `sem` column (--json:
    `sem`); --no-semantic (with --files) ranks without them.
glia resolve <REPO> <SIGNAL> [--json] [--kind <KIND>] [--scope <SCOPE>] [--top-k <TOP_K>] [--with <WITH>]...
    A stacktrace, diff, test id or free text -> the ranked, located nodes it names.
glia impact <REPO> <QNAME> [--depth <DEPTH>] [--direction forward|backward|both]
    Reachability walk from one entity: forward, backward or both.
glia blast-radius <REPO> <QNAME>... [--depth <DEPTH>] [--direction forward|backward|both] [--full] [--json] [--live-only] [--no-cochange] [--no-collapse] [--offset <N>] [--ranking ppr|ppr-wide|bfs] [--scope <SCOPE>] [--top-files <N>] [--top-k <K>] [--with <WITH>]...
    Edge-category-aware, PPR-ranked, located closure around one or more seeds, each
    row naming its seed, edge reason and `live` flag; import / contain edges excluded.
    A shared message, contract op or SCIP symbol is one node of the walk, so the radius
    crosses into every repo that shares it. With a history snapshot the rows of files that
    usually change with the seeds' files come first (co-change, then PageRank). `files` /
    `## files` ranks the --top-files (10) most relevant files: co-change, ppr, depth under
    bfs, and the rows each holds. --ranking forces ppr, ppr-wide (damping 0.85, sum-pooled
    per file) or bfs; without it the repo's calibrated pick ranks (glia history calibrate),
    else co-change then ppr, named on one `ranking:` line (`selection` in --json).
    --no-cochange drops the co-change tier. The default answer is an overview: rows cut
    to --top-k (25) after the ranking, seeds to the lesser of --top-k and 25, `truncated`
    counting what was left out, and `areas` / `## areas` mapping every row by sub-project
    or directory (rows, files, best file, the --scope that selects it, the 10 largest);
    `truncated.next` names the largest area's --scope and the next --offset. Read the
    areas first, then ask for one with --scope; --offset <N> skips the first N rows.
    Capped rows are slim: `seed_index` into `seeds`, no `name` repeating the qname's
    tail, a 4-digit `score`. --full keeps every row and field whole. The JSON leaves out
    null, empty and default fields, and a capped answer shows a seed's identity also_in
    past three members as the first three and `also_in_count`.
glia trace <REPO> <FEATURE> [--depth <DEPTH>] [--json] [--max-paths <MAX_PATHS>] [--no-collapse] [--to <TO>] [--with <WITH>]...
    Ranked distinct paths from a feature across service boundaries, each hop with its
    mechanism; --to gives the paths between two nodes. A hop through an identity class
    shows its best member and an `(also in: ..)` cell.
glia why <REPO> <FROM> <TO> [--category <CATEGORY>] [--json] [--with <WITH>]...
    Every edge between two nodes with its emitter, rule, call site, confidence and
    FACT / DERIVED / HEURISTIC tier; with none, a witness path. Exits 1 when not found.
glia implementors <REPO> <QNAME> [--direct] [--json] [--up] [--with <WITH>]...
    Who implements or extends a type, or overrides a method (--up: its supertypes),
    each row tiered by the weakest heritage edge on its path.
glia serves <REPO> <CHANNEL> [--json] [--mechanism auto|http|queue] [--no-collapse] [--with <WITH>]...
    Who serves `METHOD /path` or a queue topic, through the resolver's own matcher. An
    HTTP channel also lists, after the routes, each third-party ENDPOINT the code calls
    on that verb and path (match `external`, `external_hosts` in --json). An empty
    answer is a FACT with the mechanism's caveats and near misses. Servers of one
    identity class fold into the first (`identity`), and a server's handlers of one
    class keep the first (`handler_identity`).
glia effects <REPO> <QNAMES>... [--class <CLASS>]... [--cross-service] [--depth <DEPTH>] [--json] [--scope <SCOPE>] [--with <WITH>]... [--writes-only]
    The effect sinks downstream of the seeds (DB read / write, queue produce, outbound
    HTTP / RPC / WS / GraphQL call, event emit), each with its witness path. A call to a
    third-party host names it (`external: <host>`, `external_hosts` in --json): an HTTP
    ENDPOINT, or a WebSocket, gRPC or GraphQL client marked external.
glia docs-for <REPO> <QNAME> [--json] [--scope <SCOPE>] [--with <WITH>]...
    The doc sections that DOCUMENT a symbol, from the markdown the build reads: the
    well-known files and docs/ trees at the repo root and every project root, .ai/,
    ADR dirs, SDD feature docs (features/<f>/*.md, spec-kit specs/<NNN-slug>/) and
    synced docs (`glia docs sync`).
glia pack <REPO> <QUERY> [--budget <BUDGET>] [--bytes-per-token <BYTES_PER_TOKEN>] [--candidates <CANDIDATES>] [--json] [--no-cochange] [--no-semantic] [--preset repair|review|onboard|centrality] [--ranking ppr|ppr-wide|bfs] [--scope <SCOPE>] [--seeds <SEEDS>] [--with <WITH>]...
    The context for a query packed to a token budget (default 8000), ready to paste:
    the seeds `glia find` matches, then their callers and callees by personalised
    PageRank, each at the most detail the budget buys (full source, a preview, an
    outline line or a bare qname), located, with the links between them. stdout is
    the pack only, a summary goes to stderr; --json is the pack with its manifest.
    With a history snapshot each co-changed file's best candidate moves to the front
    before the cut; --ranking (default ppr) / --no-cochange choose the file ranking.
    Pack never reads the calibration record (ruling EC-9b: `glia history calibrate`
    changes no pack); --json names the ranking in `selection` (`fallback`, reason
    `pack_uncalibrated`, for the default; `explicit` otherwise). When nothing matches
    the query by name and the repo's .glia/vectors.jsonl (glia embed) holds vectors of
    the surface model, the seeds come from the files `find --files` ranks first (the
    blended order), each the file's best match, marked heuristic, with find's absence
    kept as `seed_absence`; --no-semantic never reads the vectors. Exits 1 when the
    pack is empty (the absence says why).
glia query <REPO> [<FILE>] [-e <TEXT>] [--explain] [--gql] [--json] [--limit <N>] [--max-rows <N>] [--output <PRED>]... [--with <WITH>]...
    A Datalog program from <FILE>, -e or stdin (with --gql, a GQL read query: MATCH ...
    WHERE ... RETURN) over the code graph, the predicate prelude merged in
    (docs/rules.md). One table per output (the program's sinks, or each --output, cut
    to --limit, default 200): every value located, every row tiered fact / derived /
    heuristic, a row through a negation with its coverage caveats; --explain adds each
    row's witness edges; --json is the QueryAnswer. A GQL row that returns properties
    only is located at the first property's node, as its Datalog twin is. Exits 0 with
    rows, 1 with none (the absence says why), 2 on a usage, parse, analysis, row-budget
    (--max-rows) or build error, each diagnostic as `<source>:<line>:<col>: error:
    <message>`.

# A change (git rev or pasted diff)
glia delta <REPO> [--base <BASE>] [--category <CATEGORY>]... [--edges-only] [--json]
    What the working tree's change did to the graph against a git rev (default HEAD):
    nodes and edges added, removed, modified or moved, located.
glia diff-impact <REPO> [--base <REV>] [--depth <DEPTH>] [--diff <FILE>] [--direction forward|backward|both] [--full] [--json] [--live-only] [--no-cochange] [--offset <N>] [--ranking ppr|ppr-wide|bfs] [--scope <SCOPE>] [--top-files <N>] [--top-k <K>] [--with <WITH>]...
    The changed nodes (--base rev or --diff file) and one ranked blast radius around
    them, each row naming the change that reached it. Ranked and filed as blast-radius
    (--ranking, --no-cochange, --top-files, `selection`, `## files`), and cut as it is
    (--top-k 25, `impact.areas`, `impact.truncated.next`, --offset, --full); a capped
    answer keeps 25 changed rows (`changed_truncated` counts the rest), and a changed
    row that repeats a shown seed is its id and change, the full row under `impact.seeds`.
glia tests-for <REPO> [QNAMES]... [--base <REV>] [--depth <DEPTH>] [--diff <FILE>] [--files-only] [--json] [--limit <N>] [--no-collapse] [--no-module-level] [--no-signals] [--scope <SCOPE>] [--with <WITH>]...
    The tests to run for a change (seed qnames, --diff or --base), tiered FACT /
    DERIVED / HEURISTIC and ranked by the test and history snapshots' signals (failed
    in recent runs, on a failing trace, co-changes with a seed's module; a test found
    only by co-change is a heuristic `cochange` row). --no-signals ranks structurally
    only; --limit keeps the first N; --files-only prints test files for a runner.
    Test rows of one identity class fold into one; --no-collapse keeps them all.
glia patterns <REPO> [--base <REV>] [--group-by service|package] [--json] [--min-share <MIN_SHARE>] [--min-support <MIN_SUPPORT>] [--scope <SCOPE>] [--with <WITH>]...
    Pattern conformance: per service (or package), each route handler's role chain
    to its first effect sink; the chain held by --min-share percent of at least
    --min-support sighted handlers is the convention, and every handler off it a
    located DIVERGENCE. Blind handlers are listed, never counted. Counts, not rules;
    --base keeps the divergences the change touched.
glia review <REPO> [--base <REV>] [--depth <N>] [--full] [--json] [--markdown-rows <N>] [--max-impact <N>] [--max-tests <N>] [--models-min-tier fact|derived] [--no-cochange] [--offset <N>] [--ranking ppr|ppr-wide|bfs] [--title <TEXT>]
    The PR report for the working tree's change against a rev (default HEAD): the
    changed nodes, their ranked impact, the tests to run, every added / removed edge
    with its tier, the declared rules checked on both sides, and the inferred-model
    breaks contract-breaks finds (`model_breaks`). Markdown for a PR comment; a rule
    whose scope holds no node is listed under "Rules that check nothing", and a
    rule's coverage caveats print once per section. A CI gate: exits 1 only on a new
    BLOCKING violation (at or above its rule's min_tier; an advisory one is listed
    under its own heading, not failed) or a blocking model break. --title (default:
    the newest commit subject of <base>..HEAD) adds the PR's intent: its ids routed
    at the base through task / requirement rows, past commits ranked by its words,
    and the impact rows in id-routed files listed first (`intent`, `### Intent`, its file
    order the repo's calibrated pick, `selection_intent`). --ranking / --no-cochange rank
    the impact as blast-radius does, and the impact is its overview: --max-impact 25
    rows, 25 seeds and 25 changed rows, `impact.areas` (### Impact areas when cut),
    `impact.truncated.next`, --offset. --full keeps every impact row, impact seed,
    changed row and test row, and writes every row whole.
glia contract-breaks <REPO> [--avro backward|forward|full] [--base <REV>] [--breaking-only] [--json] [--models-min-tier fact|derived|heuristic] [--with <REPO>]...
    Did the change against a rev (default HEAD) break a client? Every OpenAPI /
    AsyncAPI op and proto / Avro / JSON Schema type paired old -> new and judged by
    its format's evolution rules, one row per field change; every client whose call
    lost its provider is orphaned (--with adds client repos). The inferred contract
    models too (`models`): every client <-> server model exchange (http_model, the
    GraphQL selections) judged old -> new, one row per model pair with the client and
    server field lines (a JSON rename, a field removed or now optional, a new
    required request field, a type change). A model row blocks only when its tier is
    at least --models-min-tier (default fact, so the resolver-paired, derived rows
    are advisory, counted in `advisory`, not `breaking`). And the names (`names`,
    any style): every client site, in any repo of the build, still using a name the
    change removed or renamed on the server side of a paired exchange; heuristic, so
    a name row blocks only under --models-min-tier heuristic. One change to a schema
    many operations share is one row listing its `operations`. A CI gate: exits 1 on
    a breaking change, an orphaned client or a blocking model or name break, 0
    otherwise.
glia cochange <REPO> [FILES]... [--base <REV>] [--json] [--min-confidence <SHARE>] [--min-support <N>] [--top <N>] [--unlinked-only] [--with <WITH>]...
    What usually changes with these files (or the working tree's change against
    --base): each with a directional confidence (`0.80 (4/5)`), its support and
    whether a static link joins them (`none`: a blind spot). Needs a `glia history
    sync` snapshot. Heuristic; exits 1 with no rows. --base refuses --with and
    --no-overlay.
glia timeline build <REPO> [--head <HEAD>] [--json] [--revs <REVS>]
    Build the last --revs commits (default 20, at most 200) of --head's first-parent
    chain, one incremental build each on the shared parse cache, into the sidecar
    <repo>/.glia/graph/timeline.gmap.
glia timeline history <REPO> <QNAME> [--category <CATEGORY>] [--json]
    Every edge span that ever touched the node: since which rev, until which rev or
    still present, a file move followed. Before any build it exits 0 with the note
    naming `glia timeline build`.
glia timeline as-of <REPO> <REV> [--json]
    The graph at one rev of the window (an index, or a commit id prefix of 7+ hex
    chars): its counts by category. history and as-of read the sidecar, never rebuild.
glia watch <REPO> [--debounce-ms <N>] [--persist-every <S>] [--poll-ms <N>]
    Keep one repo's build open and stream what each change does to the graph: a
    foreground loop, no daemon, socket or pidfile. Prints a ready line, then one NDJSON
    object per update on stdout (the files that moved, the groups rebuilt, the totals,
    the located delta rows). Stat-polls every --poll-ms (500), applies a batch once it
    stays still for --debounce-ms (200), and writes <repo>/.glia/graph/ and the parse
    cache every --persist-every seconds (60; 0 = only on exit) and on exit. Stops on
    SIGINT, SIGTERM or stdin EOF with exit 0; refuses --no-overlay; 2 on a bad repo or
    a build error, 5 when a persist fails.

# Rules
glia check <REPO> [--json] [--strict] [--with <WITH>]...
    Evaluate .glia/overlay.toml's [[constraint]] rules (forbid_edge, transitive
    forbid_edge with `via` facades, no_cycle, and version 2's `invariant` with a
    Datalog `rule`) and its reflexion model ([[component]], [[layer]], `allow` rules:
    the component matrix, divergences, absences, unmapped files) to located, tiered
    VIOLATIONs. A violation blocks when its tier is at least its rule's min_tier (fact
    or derived, default derived); a heuristic one is advisory under any setting. A
    declared rule that checks nothing (a stanza the build anchored nowhere; one the
    build or the overlay loader rejected, such as a version-2 key in a version-1 file
    or a path two components claim; a whole file the loader ignored, listed once; one
    the graph does not hold; a scope no located node sits in) is listed under "Rules
    that check nothing" with its reason and a fix; --strict makes each a rule error.
    A rule scoped to a directory inside a project anchors on that project. The
    markdown prints a rule's coverage caveats once, under its heading. Exits 0 when
    nothing blocks, 1 on a blocking violation, 2 on an error or a rule it cannot
    evaluate: CI-ready.
glia flags <REPO> [--json] [--quiet-days <QUIET_DAYS>] [--scope <SCOPE>] [--status dead|undefined|single_site|quiet]... [--with <WITH>]...
    Stale feature flags (LaunchDarkly / OpenFeature / Unleash / Flagsmith / Split
    reads, Flipt definitions), each with its readers and definitions: dead and
    undefined (derived), single_site (fact), quiet (heuristic: every reader unchanged
    for --quiet-days, from `glia history sync --blame`). A report: exits 0.

# Inputs from outside the source: each writes a .glia/ snapshot or sidecar that the
# next build, or an answer at query time, reads. A build never fetches, syncs, ingests or embeds.
glia docs sync <REPO> [--source confluence] --space <SPACE> [--email <EMAIL>] [--exclude <PATTERN>]... [--include <PATTERN>]... [--site <SITE>] [--token <TOKEN>]
    Pull a Confluence space into <repo>/.glia/docs-snapshot/ (network step);
    credentials from the flags, then CONFLUENCE_SITE / CONFLUENCE_EMAIL /
    CONFLUENCE_TOKEN, then ./.env. --include / --exclude keep or drop pages by
    title, for every source.
glia docs sync <REPO> --source dir --path <DIR> [--container <NAME>] [--url-base <URL>] [--exclude <PATTERN>]... [--include <PATTERN>]...
    A local GitHub / GitLab wiki checkout's Markdown pages (no dot-entries,
    _-prefixed files or symlinks); the container defaults to the directory's name.
glia docs sync <REPO> --source mediawiki --api <URL> --namespace <N> | --category <NAME> [--container <NAME>] [--max-pages <N>] [--token <TOKEN>]
    One namespace or category of a MediaWiki through its Action API (never by
    following links); a bearer token from --token or MEDIAWIKI_TOKEN.
glia docs sync <REPO> --source notion --database <ID> | --page <ID> [--api <URL>] [--max-pages <N>] [--token <TOKEN>]
    A Notion database's pages, or a page and its sub-pages, blocks converted to
    Markdown; the integration token from --token or NOTION_TOKEN.
glia docs push --file <FILE> --space <SPACE> --title <TITLE> [--email <EMAIL>] [--markdown] [--page-id <PAGE_ID>] [--site <SITE>] [--token <TOKEN>]
    Create or update (--page-id) a Confluence page; --markdown converts the file first.
glia history sync <REPO> [--blame] [--blame-max-files <BLAME_MAX_FILES>] [--max-commits <MAX_COMMITS>] [--no-messages] [--since <SINCE>]
    Read the local git history into <repo>/.glia/history-snapshot/: churn and blame
    ATTN cells, CO_CHANGES edges, and messages.jsonl (each commit's redacted,
    trailer-stripped message, author time and merged-branch label, read only by
    query-time ranking; --no-messages keeps none). No author identities are stored.
glia history calibrate <REPO> [--json] [--max-commits <N>] [--max-seeds <N>] [--no-semantic] [--sets <SETS>]
    Replay the history snapshot over the graph and write <repo>/.glia/calibration-snapshot/:
    the per-repo ranking each ranked answer picks. Run after `glia history sync`; a
    build never calibrates. It scores each candidate ranking on the repo's own past
    commits, oldest first, picking with only the commits before each one (leak-free):
    SELh0 / SELs0 for the text-free answers (blast-radius, diff-impact, review's impact;
    SELs0 under --no-cochange), SELt2 for review's intent, SELf2t for find --files. pack
    never reads the record (ruling EC-9b). When the repo's .glia/vectors.jsonl (glia embed)
    holds vectors of the surface model, SELf2t also measures the blended orders
    FUSE[T1s,SEMp:t2] and LX[IDR,FUSE[T1s,SEMp:t2]], and find --files reads that record
    only with vectors; --no-semantic calibrates without them.
    One line per (set, shape) with the pick, its online recall@10 against the fallback's
    and a bootstrap 95% CI. --max-commits (300) test commits, --max-seeds (5) queries
    each, --sets a subset of the groups structure, review, find. An answer ignores a
    record another glia build wrote (reason build_stamp): calibrate again after an
    upgrade. Exits 1 with no history snapshot, 2 on a build error.
glia tests ingest <REPO> [--junit <PATH>...] [--lcov <PATH>...] [--log <PATH>...] [--reset] [--run <LABEL>] [--window <N>]
    Add one CI run's JUnit XML, CI logs and lcov to <repo>/.glia/test-snapshot/ as
    its newest run; the snapshot keeps the last --window runs (default 10; --reset
    drops the earlier ones): FAIL cells per test (fails in the window, latest run)
    and COVERAGE cells. Messages are redacted for secrets before they are stored.
glia scip import <REPO> <INDEX> [--prefix <PREFIX>]
    Decode a SCIP index (scip-python, scip-typescript, scip-java, scip-go,
    rust-analyzer) into <repo>/.glia/scip-snapshot/; the next build binds it as
    FACT-tier CALLS / USES / IMPLEMENTS / INHERITS_FROM edges and confirms name-only
    ones. --prefix: the directory the index's paths are relative to. Exits 1 when
    the index cannot be decoded (nothing written).
glia vocab sync <REPO> [--json] [--rev <REV>]
    Read one commit's tracked text files (default HEAD) into
    <repo>/.glia/vocab-snapshot/: every identifier-like token with the files it
    occurs in. find's absence and gaps read it; the build never does. Exits 1 when
    the sync fails (nothing written).
glia embed <REPO> [--check] [--json] [--limit <N>] [--model bge-small|test-hash] [--recipe t1|t2] [--replace]
    Embed each node's text (name, qname, leading doc, imports; code last under
    --recipe t2) with a local model (default bge-small, cached under the user cache,
    never inside the repo) into <repo>/.glia/vectors.jsonl, re-embedding only nodes
    whose text changed. Never part of a build; find, flows and pack read it.
    --replace rewrites a sidecar of another model; --limit embeds at most N nodes;
    --check counts the nodes with no fresh vector, loads no model and writes nothing.
    Exits 1 under --check when nodes need embedding, 2 on an error.
glia overlay propose <REPO> [--category <CATEGORY>]... [--json] [--snippet-lines <SNIPPET_LINES>] [--top-k <TOP_K>] [--with <WITH>]...
    The overlay loop's work list: the gaps an overlay stanza could close, per
    category, each with its gap id, location and source lines, and a suspected
    edge's paste-ready [[edge]]. Writes nothing.
glia overlay try <REPO> --candidate <CANDIDATE> [--json] [--no-leave-one-out] [--with <WITH>]...
    Build a candidate stanza file against the repo (base, with it, and without each
    stanza in turn) and report what each stanza changes and a keep / review / drop
    verdict. --no-leave-one-out: two builds, no per-stanza attribution. A
    [[constraint]] rule stanza is judged by `glia check` on the build with every
    stanza (holds / violated / checks_nothing / error / orphaned / duplicate; a rule
    whose scope holds no node checks nothing and is review, as glia check lists it),
    and a graph stanza that adds or hides a declared rule's violation is review.
glia overlay accept <REPO> [--candidate <CANDIDATE>] [--dry-run] [--json] [--only <ONLY>]... [--remove <REMOVE>]...
    Merge chosen candidate stanzas (--only `edge#2`, `constraint#1`) into
    .glia/overlay.toml, or remove orphaned, redundant or rotted rules by gap id,
    validated and written atomically: the only writer of that file. It refuses a rule
    that does not parse or defines no `violation`; a rule whose scope holds no node is
    written, and glia check lists it as checking nothing. It moves the file to
    `version = 2` only when a written rule needs it. Exits 0 written (or --dry-run), 1 when validation refuses,
    2 on an error. The overlay commands refuse --no-overlay.
glia cell set <REPO> <QNAME> <CELL> [--dims <DIMS>] [--file <PATH>] [--hint <HINT>] [--json <ENTRY>] [--kind <KIND>] [--model <MODEL>] [--text <TEXT>]
    Write a CONSTRAINT / DECISION / CONV entry (one of --json, --text, --file) or a
    VECTOR (--file, --model, --dims) on a node; kept in .glia/cells.jsonl /
    vectors.jsonl across rebuilds. --hint takes the node's v1 identity hint
    (`v1|<kind id>|<file name>|<body hash>|<local name>`, the form engram-export's `<out>.hints.jsonl`
    lists), so the row re-binds after a move: a malformed or kind-contradicting hint
    exits 2 and writes nothing, and when a fresh build binds the qname to a node
    whose own hint differs the write is applied with target=hint_differs and a note
    naming the fields that differ.
glia cell rm <REPO> <QNAME> <CELL> [--id <ID>] [--source <SOURCE>]
    Remove one entry (--id, --source) or a node's VECTOR.
glia cell ls <REPO> [--check] [--json] [--qname <QNAME>] [--rekey] [--writer <NS>]
    List the sidecar rows. --check binds each against a fresh build and exits 1 on an
    orphaned, ambiguous or rejected row; --rekey rewrites rows a moved node re-bound.
```

## More

- [docs/overview.md](./docs/overview.md): across services, coverage (languages, frameworks, resolvers, the measured matrix), numbers, architecture, how glia compares, earlier experiments, acknowledgments.
- [docs/history.md](./docs/history.md): what each release shipped, 0.4.x to 0.5.4.
- [docs/overlay.md](./docs/overlay.md) (`.glia/overlay.toml`, the sidecars and snapshots), [docs/rules.md](./docs/rules.md) (Datalog and GQL), [docs/embeddings.md](./docs/embeddings.md) (models and devices), [docs/cache.md](./docs/cache.md) (the shared parse cache), [SECURITY.md](./SECURITY.md) (what glia never does).
- Problems: open an issue ([bug](./.github/ISSUE_TEMPLATE/bug.md), [embedding device](./.github/ISSUE_TEMPLATE/embedding-device.md) with your `[embed] device=` line).

## Roadmap

**0.5.4, embeddings and precision** (released 2026-10-31: tag `v0.5.4`, `glia-py` 0.5.4 and `glia-cli` 0.5.4 on PyPI): embeddings in glia (`glia embed`, GPU by default) blended into `find --files` and `pack`'s fallback, corrections and `[[convention]]`s in `pack` / `review`, the identifier vocabulary (`glia vocab sync`), boundary health (`glia health`, and `blind` rows in answers), compact overview answers, name-level drift in `contract-breaks` / `review`, org-internal package routing and Maven / Gradle / pub / Hex / NuGet dependencies, the edge-precision audit and its fixes, one lowercase tier vocabulary, rustfmt and rustdoc gates, the `glia-cli` PyPI package and the task skills. Its measured experiments, each judged by a rule committed before the run, with its report's verdict line:
- Embedding gate, the semantic tier alone (FB.6, [report](./dev-notes/embedding-gate-0.5.4.md)): `VERDICT: FAIL`.
- Fusion gate, BM25 blended with similarity (FB.6b, [report](./dev-notes/fusion-gate-0.5.4.md)): `Verdict rows: R1=yes (8/9) R2=yes (8/9) -> PASS`, `VERDICT: PASS`; so `find --files` ranks by `LX[IDR,FUSE[T1s,SEMp:t2]]` when a repo holds vectors.
- Edge precision (FD.3c, [report](./dev-notes/edge-precision-0.5.4.md)): 503 judged edges on five public repos, fact 0.998 [0.995, 1.000], derived 0.599 [0.497, 0.700], heuristic 0.912 [0.826, 0.997]; the under-floor strata were fixed (FD.73-FD.81) and are re-measured in 0.5.5.
- Agent before / after (FE.4, FH.13, [record](./dev-notes/agent-ab-probe-0.5.4.md)): on repos a frontier model can grep, it found the same files with or without glia; the probe sets on the shapes where glia should matter are built and pinned, and run as 0.5.5's baseline.
- Surface replay (FA.1), PR intent (FC.19) and placement (FC.21): not run in 0.5.4, no verdict; they moved to 0.5.5 with their tracks (T8, T7).

**0.5.5** ([seed](./dev-notes/next-leap-0.5.5.md)): the tracks moved out of 0.5.4 (per-language residuals, the goal layer and clarification queue, concept search / PR intent / placement, incremental resolution and multi-repo watch, build-target graphs), then proof: precision on real repos, the agent probes, calibrated confidence, `decide/` and `glia judge`; plus docs as a second graph, wave agents that never run the full gates, and the edge-audit re-run. Every release in full: [docs/history.md](./docs/history.md).

## License

See [`LICENSE`](./LICENSE). Glia Software License v0.1, an overlay on PolyForm Noncommercial 1.0.0 with Additional Permissions for worker-protective commercial use.

### Tier check

| If you are... | Cost |
|---|---|
| Individual, student, academic, researcher, hobbyist, OSS contributor | Free |
| Nonprofit | Free |
| For-profit org with **fewer than 500 STEM workers** | Free |
| **Worker-owned** org (workers hold ≥50% equity) | Free at any size |
| **Certified B Corporation** in good standing | Free at any size |
| Org where **≥50% of STEM workers are covered by a recognized union** under an active CBA | Free at any size |
| Any other for-profit org | Commercial license required |

Commercial license inquiries: `j.r.chahwan@gmail.com` or open an issue on [the repo](https://github.com/James-Chahwan/glia). Author retains discretion to grant free Commercial Licenses case-by-case. When in doubt, ask. Past compliant use is never retroactively revoked (LICENSE §5.3).

Why the license is shaped this way, and what it selects for: [docs/license.md](./docs/license.md). Acknowledgments: [docs/overview.md](./docs/overview.md#acknowledgments).
