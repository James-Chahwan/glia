# neuropil handoff - glia 0.5.2 (the four bets 2026-10-16, the tweaks 2026-10-17)

**Deferred: neuropil skips 0.5.2 and upgrades to 0.5.3** (James, 2026-10-18: *Don't notify neuropil it's so far behind visual 3d stuff takes much more free time not contracting for work but on the other not committed it can go to 0.5.3*). Nobody is notified; this file stays the 0.5.2 part of neuropil's path, and 0.5.3's neuropil handoff covers 0.5.1 -> 0.5.3 by carrying this one forward.

**glia 0.5.2 is released 2026-10-18 (tag `v0.5.2`), and every pin below reads `=0.5.2`.** James,
2026-10-18: *"maybe we just do the 0.5.2 and slowly work on 0.5.3 over the next week"*. That reverses the
2026-10-17 plan of no 0.5.2 release (*"can't we just go 0.5.1-> 0.5.3 release"*), under which this file
was written as the 0.5.2 half of a combined 0.5.1 -> 0.5.3 handoff; it is now the whole 0.5.2 handoff,
and neuropil applies it after the publish. 0.5.3, paced over the week, is a normal 0.5.2 -> 0.5.3
release with its own handoff. Section 10 adds the tweaks (DT.1a-DT.13, waves W15-W26) to the W0-W14
content below, and section 11 the close-out (DT.5h-DT.5l, the dead_symbol false positives, waves
W27-W30, and DT.14).

The third glia -> neuropil handoff, after `neuropil-handoff-0.5.0.md` and `neuropil-handoff-0.5.1.md`.
neuropil is glia's in-process Rust consumer: it path-depends on six glia crates and calls
`generate_one` at startup (`crates/neuropil-app/src/state.rs:443`) and in `bin/tier-audit.rs:21`. Read
this in the neuropil session and act on neuropil only. 0.5.2 is "the four bets": 153 packets in
`dev-notes/leap-052-packets.json` (plan: `dev-notes/next-leap-0.5.2.md`), 151 landed in waves W0-W13
on glia's local `main` since v0.5.1 (`84e2e71`), and W14 is the two docs packets (DZ.1 the release
docs, DZ.2 these handoffs). Groups: DA the `glia-rules` Datalog crate, DB rules in the engine (check,
`glia query`, GQL), DC `glia watch` and `WatchSession`, DD cross-repo identity (SAME_AS), DE Engram
anchors and engine hygiene, DF the 0.5.1 language deferrals, DG / DH the rest of the coverage matrix.
Breaks were allowed when a packet declared them, and **consumers pin the release exactly**.

Sources, in order of authority:
- a run of glia's compile check, `scripts/check-neuropil.sh`, against glia HEAD `12f58844` (the W13
  close-out `f468af11` plus two plan commits; BUILD_STAMP `0.5.1+p0c104fe6f8df1704`, the version field
  still reads 0.5.1 before the bump), on a copy of neuropil with the 0.5.0 handoff's section 2 edits
  applied, run 2026-10-16 by the docs packet DZ.2. Section 1 has the output verbatim. Every compile
  claim comes from that run;
- each packet's landed `breaking` report and commit body. Where a report differs from its spec, the
  report wins, and where a report is wrong about neuropil this doc says so (section 6);
- the specs' `breaking` blocks in `dev-notes/leap-052-packets.json` and the routed handoff paragraphs
  in `dev-notes/leap-052-corrections.json`.

Every neuropil `file:line` was read read-only in neuropil's working tree: HEAD `b140d1e` with the same
50 dirty paths as at the 0.5.0 and 0.5.1 handoffs. Paths are relative to `crates/neuropil-app/src/`
unless they start with `crates/` or name a root file. Packet ids are glossed in words; the hash after
an id is the glia commit that landed it.

---

## 0. TL;DR

1. **The 0.5.0 handoff is still unapplied.** `Cargo.toml:112-117` still names `repo-graph-*` crates
   and the `driver` feature, so neuropil resolves against no glia since 0.5.0. Apply the 0.5.0
   handoff's sections 2-3 first; 0.5.2 stacks on top, and its pin (section 2) replaces 0.5.1's. The
   0.5.1 handoff's section 3 re-keys happen in the same move, so state is re-set once for both.
2. **0.5.2 adds no compile break.** With the 0.5.0 section 2 edits applied, the check reports
   **new=0 errors** under `--all-targets` against glia `12f58844` (section 1), and again with the
   tweaks, against glia `9a8e56e0` + DT.13 (section 10.1). Every 0.5.2 api_signature
   break (section 4) is on an item neuropil never names: grep of `crates/` finds 0 hits for each.
3. **Pin exactly, all six crates:** `version = "=0.5.2"` on every glia path dependency (section 2).
   Since 0.5.1 every glia crate takes the workspace version, so the 0.5.1 handoff's "only glia-engine
   carries the release version" no longer holds.
4. **Persisted state re-keys once for the nodes whose qname or kind moved** (section 3): Go ROUTEs on
   a constant-prefixed group, Python services' channel qnames under a requirements.txt / Pipfile root,
   sbt members' channel qnames, Kina's `${…}` client ENDPOINTs, templated OpenAPI ops, Akka and Vapor
   ROUTEs, Dart GraphQL operations, Elixir behaviour modules (PACKAGE -> INTERFACE), and the
   `queue_*:unresolved:<f>` tags that now have a name. NodeIds follow kind and qname: the RepoId scheme
   did not move.
5. **Display:** SAME_AS (edge category 37, D0.1 `9d180f01`) has no arm in `hud.rs:1516-1550`
   `edge_category_name` and prints `?` (as 32-36 already do); the SHARES_DEPENDENCY arm (`hud.rs:1548`)
   can no longer fire (DD.16 `14c6ae40`).
6. **Optional, in-process:** `glia_engine::watch::WatchSession` is the "per-shard incremental rebuild
   path" `watcher.rs:1-7` waits for (v1.1); `glia_engine::memory::release_freed_memory` after dropping
   a `GenerateResult` (section 7).
7. **Reinstall the `glia` binary from the v0.5.2 tag** with neuropil's move: the hooks prompt
   (`hooks_install_prompt.rs`) installs hooks that run `glia build .`, and two glia versions writing one
   `.glia/graph/` rebuild each other's layout ("written by another glia build").

Bump order:
1. glia is bumped in the release commit (`[workspace.package].version` in `Cargo.toml:74`,
   `py/pyproject.toml:7`, the excluded `engram-export/Cargo.toml:3`, all 0.5.1 -> 0.5.2 together);
   James pushes it and tags `v0.5.2` (2026-10-18).
2. In neuropil: the 0.5.0 handoff sections 2-3, the pin (section 2 here), `cargo check --workspace
   --all-targets`, then the re-test (section 8), re-setting state for this doc's section 3 and the
   0.5.1 handoff's section 3 together.

## 1. The compile check at this commit

A copy of neuropil with the 0.5.0 handoff's section 2 edits, checked with the unmodified script. The
copy and its target lived under `~/.cache/glia-DZ.2/` and were deleted afterwards; nothing under
neuropil or glia was written (the script compares neuropil's `git status` and `Cargo.lock` hash before
and after; 50 dirty paths before and after).

```
C=~/.cache/glia-DZ.2/npcheck
cp -p ../neuropil/Cargo.toml ../neuropil/Cargo.lock ../neuropil/rust-toolchain.toml "$C/np/"
rsync -a --exclude=target/ ../neuropil/crates/ "$C/np/crates/"
python3 dev-notes/rename-0.5.0.py --apply --quiet --root "$C/np"     # non-git root: every file
sed -i '/^glia-projection-text = /s/"driver"/"research"/' "$C/np/Cargo.toml"
NEUROPIL_DIR="$C/np" GLIA_CONSUMER_CACHE="$C/cc" bash scripts/check-neuropil.sh   # cold target
```

```
[rename-0.5.0] rewrote 165 token(s) in 50 file(s) under /home/ivy/.cache/glia-DZ.2/npcheck/np (0 unknown token(s) left)
[neuropil-check] note - neuropil differs from the baseline's (b140d1ede58ff6e77e8e7326a6a4cf21e822959e dirty=50 src=b690b6b4f6db -> unknown src=73b04787b8a9): NEW/FIXED may be neuropil's own edits, not the leap's
[neuropil-check] ok - new=0 fixed=0 preexisting=0 in 147.7s against glia 0.5.1+p0c104fe6f8df1704 (neuropil unknown, lock drift +40 -26)
```

- The rename count (165 tokens in 50 files) and the source hash `73b04787b8a9` are the 0.5.0 and 0.5.1
  runs' own, so the copy is the same neuropil those checks saw.
- `errors.txt` is empty: 0 errors across every neuropil target and the glia crates they pull in
  (rustc 1.95.0 from neuropil's `rust-toolchain.toml`, cargo `Finished ... in 2m 27s`).
- The `note` line is expected: the copy is not a git checkout.
- **Lock drift +40 / -26**, one more than 0.5.1's +39: the 26 `repo-graph-*` path packages become
  `glia-*` (all at the workspace version, 0.5.1 until the bump); glia-store, glia-parser-kotlin,
  glia-stamp and glia-doc enter, and **`glia-rules`**, 0.5.2's one new crate (D0.2 `35cfec86`, no
  external dependency); the registry crates are 0.5.1's nine (`rayon`, `rayon-core`,
  `crossbeam-deque`, `crossbeam-epoch`, `ignore`, `globset`, `bstr`, `tree-sitter-kotlin-ng`,
  `lz4_flex 0.11.6`). 0.5.2 adds no registry crate.

`check-neuropil.sh` still copies neuropil verbatim, so run as-is it reports only the cargo resolution
error until neuropil renames. The `NEUROPIL_DIR` recipe above stands in for it, as it did for 0.5.0 and
0.5.1.

## 2. Cargo: the pin

| where | 0.5.0 handoff asked | 0.5.2 adds |
|---|---|---|
| `Cargo.toml:112-117` | `repo-graph-<x> = { path = "../glia/<dir>" }` -> `glia-<x> = { path = ... }`; projection-text feature `driver` -> `research` | `version = "=0.5.2"` on all six: `glia-core`, `glia-code-domain`, `glia-graph`, `glia-engine`, `glia-activation`, `glia-projection-text` (e.g. `glia-engine = { path = "../glia/engine", version = "=0.5.2" }`) |
| `crates/neuropil-app/Cargo.toml:86-94` | `repo-graph-<x>.workspace = true` -> `glia-<x>.workspace = true` | nothing |
| `crates/**/*.rs` | 165 `repo_graph_*` path tokens -> `glia_*` (0.5.0 handoff section 2) | nothing |

All six crates read `version.workspace = true` (checked: the copy's lock lists every glia package at
0.5.1, the workspace version). A `version` on a path dependency must match the crate's own version, so
check out glia at the `v0.5.2` tag before adding it: a glia checkout before the release commit reads
0.5.1, and there `=0.5.2` fails resolution.

## 3. What re-keys: qname-keyed state and NodeId anchors

`NodeId = xxhash(graph_type, repo, kind, qname)`. 0.5.2 did not touch the RepoId scheme (DE.1 made
graph CONTENT independent of the RepoId, not the ids), so only nodes whose kind or qname moved get a
new id. neuropil keys state by qname in `view_state.rs` (`hidden_nodes` `:192`, `hex_collapsed`
`:210`, `extra_qname_prefixes` `:131`, `hide_node` `:316`), the session `bookmarks` table
(`crates/neuropil-session/src/lib.rs:196` `add_bookmark`) and `bookmarks.rs`, and by NodeId in
`annotations.rs:45`, `:55` (`.neuropil/annotations.json`) and `flow_replay.rs`. For the nodes below,
those entries stop matching once; an anchored annotation falls back to its stored `world_pos`
(`annotations.rs:279-282`).

| family | packet (commit) | old -> new | measured |
|---|---|---|---|
| Python services under a requirements.txt / Pipfile root | DE.9 a requirements.txt or Pipfile beside a .py file roots a python project (`e0370f74`) | HTTP / channel qnames under a NESTED such root gain the owner suffix: `GET /health` -> `GET /health @svc/email`; `grpc_client:ProductCatalogService` -> `grpc_client:ProductCatalogService @src/recommendationservice`; a node two such services shared unowned splits into one per owner (`grpc_client:<Svc>` -> `... @src/emailservice` + `... @src/recommendationservice`, `grpc_server:Health` likewise, `endpoint:*` -> `... @src/loadgenerator`, `POST /` -> `POST / @src/shoppingassistantservice`); new `project:<dir>` PROJECTs | microservices-demo: 21 qnames gone, 35 new (nodes 3,083 -> 3,097) |
| sbt sub-projects | DG.12 sbt multi-project builds (`ff881e56`) | ROUTE / ENDPOINT / QUEUE_* / WS_* / GRAPHQL_* / GRPC_* / RPC qnames of files under a member gain ` @<dir>` (new NodeIds); new `project:<dir>` PROJECTs (ecosystem sbt, label the sbt id). A `[[project]]` stanza at a root labelled by its directory now gives it the declared label: nav name and ORIGIN `label` change, qname and NodeId do not | fixture only |
| Go ROUTEs on a constant-prefixed group | DF.7 route-group prefixes held in a constant (`0fab4960`) | `<METHOD> <local path>[ @owner]` -> `<METHOD> <const prefix><local path>[ @owner]` (`GET /items` -> `GET /api/v1/items`, `GET /deep` -> `GET /m/api/v1/deep`); HANDLED_BY / HTTP_CALLS move with them | quokka-stack `a77d4cb` and grpc-go `6f50403`: whole graph unchanged |
| templated OpenAPI ops | DF.18 OpenAPI server bases recorded (`78645340`) | `contract::<scope>::<METHOD>:<path>` -> `contract::<scope>::<METHOD>:<base><path>` for a document whose first server url is templated with defaulted variables (`contract::docs::openapi::GET:/users` -> `contract::docs::openapi::GET:/api/v1/users`, name `GET /api/v1/users`) | no bench, matrix or quokka op moved |
| Kina's frontend ENDPOINTs | DF.20 endpoint bases held in a class member or a config const fold (`7d2f16a2`) | `endpoint:<M>:${…}/<rest>` -> `endpoint:<M>:<base path>/<rest>`: `endpoint:GET:${…}/stats @frontend` -> `endpoint:GET:/admin/stats @frontend`, `${…}/trades` -> `/api/trades`; nine `${…}` nodes merge into the real-path node another call site already had | Kina: 85 call sites, ENDPOINTs 112 -> 103 |
| Akka / Pekko HTTP ROUTEs | DG.10 every path matcher segment under its enclosing pathPrefix (`774de9b4`) | `ANY /<first literal>` -> `ANY /<prefix segments>/<every matcher segment>` (`ANY /ws` -> `ANY /ws/chat`; `ANY /users` under `pathPrefix("api")` -> `ANY /api/users`; `Segment` -> `:segment`, number / UUID matchers -> `:id`, `Remaining` -> `*`). Removed: ROUTEs read from a non-directive `path(` (`subpath(`, a comment, a string) or an unreadable matcher | fixtures; matrix/scala/ws's server ROUTE becomes `ANY /ws/chat` |
| Swift Vapor ROUTEs | DH.20 Vapor routes keep `:id` (`121041d3`) | `GET /users/id` -> `GET /users/:id`, `DELETE /users/id` -> `DELETE /users/:id`, `PATCH /todos/todoID/patched` -> `PATCH /todos/:todoID`. Removed: phantom ROUTEs from `.get(` / `.post(` calls with neither `use:` nor a trailing closure (`GET /id` from `req.parameters.get("id")`) | fixtures |
| Elixir behaviour modules | DH.17b behaviours as INTERFACE (`8f72df22`) | a pure behaviour module (`@callback` / `@macrocallback` only) is INTERFACE instead of PACKAGE: same qname, the PACKAGE NodeId goes and an INTERFACE one comes. Qname-keyed state is unaffected; NodeId-keyed annotations orphan | fixtures |
| Dart GraphQL operations | DH.8 graphql-ruby / Absinthe roots, Dart operation names (`edfba300`) | `graphql_op:client.query` / `client.mutate` / `client.subscribe` -> `graphql_op:<OpName>` when the call's own `gql(..)` takes a same-file constant or an inline literal; new `graphql_resolver:<Root>` / `graphql_resolver:<field>` (graphql-ruby, Absinthe) | fixtures |
| Ruby queues | DH.1 Ruby broker rows (`41f4c589`) | `queue_producer:unresolved:sns` (a Ruby `.publish(topic_arn: "arn:..:<t>")`) -> `queue_producer:<t>`; new `queue_producer:<t>` / `queue_consumer:<t>` for Bunny, Karafka, google-cloud-pubsub, aws-sdk-sqs / sns (unreadable sites keep a `queue_*:unresolved:<rabbitmq\|sqs>` tag) | fixtures |
| PHP queues | DH.2 PHP broker rows and Laravel jobs (`214105be`) | `queue_producer:unresolved:rabbitmq` beside a php-amqplib positional publish -> `queue_producer:<routing key>` (the exchange on a fanout); `queue_consumer:unresolved:kafka` beside a php-rdkafka subscribe -> `queue_consumer:<topic>`; new `queue_producer:<JobClass>` / `queue_consumer:<JobClass>` for Laravel queued jobs | fixtures |
| Dart queues | DH.3 Dart broker rows (`4d96781a`) | `queue_producer:unresolved:kafka` beside a kafka_dart `producer.sendMessage(topic: ..)` -> `queue_producer:<topic>`; new nodes for dart_amqp, mqtt_client, dart_nats, googleapis Pub/Sub, package:redis, aws_sqs_api, redis_task_queue | fixtures |
| Elixir queues | DH.4 Elixir broker rows (`57ae47f1`) | `queue_producer:unresolved:oban` -> `queue_producer:<Worker>` wherever the pipe form names the worker; new nodes for AMQP, brod / broadway_kafka, tortoise, Gnat, broadway_cloud_pub_sub, google_api_pub_sub, Redix, ExAws.SQS | fixtures |
| stub-only gRPC clients | DE.8 generated gRPC stubs mint no GRPC_CLIENT (`38b4f41e`, declared edge_removal) | `grpc_client:<Svc> @<owner>` minted only by a generated stub file is gone, with its GRPC_CALLS; a client a hand-written site also mints keeps its qname and NodeId, and its POSITION moves to that site | microservices-demo Go services 36 -> 13 GRPC_CALLS, Python 18 -> 3; grpc-go GRPC_CLIENT 28 -> 21 (SearchService, SearchServiceV3 gone) |

**Added only, no existing qname moves** (the DH packets declared `node_id` for nodes they add): DH.5
`config:env:<NAME>` from Dart `Platform.environment[..]`, Elixir `System.get_env` / `fetch_env!` and
Laravel `env(..)` (`d7230be3`); DH.6 `config:flag:<key>` (Flipper, FunWithFlags, LaunchDarkly PHP) and
`config:secret:aws_sm/<ref>` (`c4bb7da0`); DH.7 `event_emit:<name>` / `event_handle:<name>` for
ActiveSupport::Notifications, Laravel events, Dart event_bus and Phoenix.PubSub (`6db8612a`); DH.9
`ws:<path>` (Ratchet) and `ws_client:<path>` / `ws_client:ws` (Dart) (`df81a9c1`); DH.10 `cli:<name>`
CLI_COMMANDs (package:args, Mix tasks `cli:shop.sync`, composer `bin`, gemspec `executables`, a gemspec
with an executable is now a manifest MODULE) and CLI_INVOCATIONs (Symfony Process, `shell_exec`,
`Process.run`, `System.cmd`) (`9c895029`); DH.11 `data_entity:sql:<table>` from sqflite helpers and Ecto
migrations (`13902a97`); DH.12 elixir-grpc `grpc_client:<Service>` (`ab306320`); DH.14 PHP interface
METHODs `<Interface>::<method>` (`a08fc669`); DH.17b ExUnit FUNCTIONs `<module>::test <name>` /
`<module>::test <describe> <name>` (qnames with spaces, as ROUTE names have); DH.18 Ecto
`data_entity:sql:<Model>` and schemaless `data_entity:sql:<table>` (`411c063e`); DH.21 libcurl
`endpoint:<METHOD>:<path>` (`837297c7`). Edges only, no node added or moved: DH.13 Ruby INHERITS_FROM /
INJECTS / constructed-receiver CALLS (`e8223e1f`), DH.15 Dart injectable INJECTS and shelf_router
HANDLED_BY (`304037c1`), DH.17a Elixir remote CALLS (`77e8d9fa`), DH.19 Compojure / reitit HANDLED_BY
(`db240e69`). From DG / DF, also additive: Scala test-case FUNCTIONs `<suite>::<description>` (DG.11
`c8357fbb`), Rust / Scala broker, event, GraphQL, WebSocket, cron, CLI and Slick nodes (DG.1-DG.9),
Python signal EVENT nodes (DF.12 `5bec20cd`, DF.13 `dc4b37a7`), PHP nodes after a `use \A\B\{..}` line
(DF.1 `e39dfe8a`; such a file used to yield only its MODULE).

**Removed with no successor:** DG.1 / DG.2 / DH.1 / DH.2 / DH.4's broker gates suppress eventbus's
verb-fallback `event_emit:<verb>` / `event_handle:<verb>` phantoms in files that name one of the new
broker libraries (C / C++ files naming `google/cloud/pubsub` included), plus `event_emit:ProducerRecord`,
the Alpakka GooglePubSub EVENT twins and inline redis4cats `unresolved:redispubsub` tags (`089e03d8`).

To make qname state move-proof, the 0.5.0 handoff's section 4.2 still applies: store
`glia_graph::identity::Identity::hint()` beside the qname and rebind a miss through
`IdentityIndex::build(&merged).rebind(qname, kind, hint)`.

## 4. Engine API changes (the api_signature breaks), against neuropil

neuropil uses `glia_engine::generate_one`, `glia_graph::MergedGraph`, `glia_activation::{activate,
ActivationConfig}` (`render/graph.rs:8`, `:340`), `glia_projection_text::composition::{render_cells,
synth_paths}` and `synth_callsite_argflow` (`inject.rs:17-18`), and `glia_core` / `glia_code_domain`
ids. None of those changed shape since v0.5.1 (`git diff v0.5.1..HEAD` touches no `ActivationConfig`,
`activate` or `projection-text/src` line). Every item below: grep of `crates/` for its name, 0 hits.

| packet (commit) | item: old -> new |
|---|---|
| DD.3 identity rules in DomainTables (`92a960fd`) | `glia_activation::profile::DomainTables` gains the required field `identity_rules: &'static [IdentityRule]` (the struct stays exhaustive, so every literal must fill it); new `IdentityRule`, `IdentityTier`, `DomainTables::identity_rule(name)`; `CODE_TABLES.activation_weights` 20 -> 21 rows, `(SAME_AS, 0.0)` |
| DD.7 SCIP identity (`16020321`) | `glia_graph::IdentityResolver` (a unit struct at DD.5) -> `IdentityResolver<'a>` with a private field, built with `IdentityResolver::new()` (or `Default`) / `IdentityResolver::with_scip(&[(NodeId, String)])` |
| DD.9 find collapses (`acd819ef`) | `glia_engine::find::FindOptions` gains `collapse: Collapse` (default on); `FoundNode` gains `identity: Option<RowIdentity>` |
| DD.10 blast radius over the quotient (`1604b425`) | `glia_engine::BlastOptions` gains `collapse: Collapse`; `BlastAnswer` / `BlastSeed` gain `identity`; new flat `glia_graph::blast_radius_over`; `MergedGraph::blast_radius` and `blast_radius_filtered` keep their signatures |
| DD.11 trace and flows (`6938c604`) | `glia_engine::trace::TraceOptions` gains `collapse: Collapse`; `TraceHop` gains `to_identity` (last field); `entry_flows_with_live(merged, live, labels, depth)` -> `(merged, live, labels, depth, collapse: &Collapse)`; `entry_flows` keeps its signature |
| DD.12 tests-for (`5e293e54`) | `glia_engine::tests_for::TestsForArgs` gains `collapse: Collapse`; `TestHit` gains `identity` |
| DD.13 serves (`da4e9857`) | `glia_engine::serves::Server` gains `identity` and `handler_identity: Vec<HandlerIdentity>`; new `serves::serves_with(merged, live, channel, mechanism, &Collapse)` and `#[non_exhaustive] HandlerIdentity { qname, identity }`; `serves` / `serves_with_live` collapse by default |
| DD.14 arch (`1934981b`) | new `glia_graph::cross_links_with` (flat), `glia_engine::arch::service_map_collapsed`; `service_map` / `service_map_with` collapse by default; SAME_AS joins the symmetric categories (a SAME_AS link is from <= to) |
| DD.16 SAME_AS replaces SHARES_DEPENDENCY (`14c6ae40`) | `glia_graph::PackageResolver` and `glia_graph::MessageSchemaResolver` removed; CODE_PASSES 26 at v0.5.1 -> 25, Resolve 15 -> 14 (DD.5 added `identity`, DD.16 removed `message_schema` and `package`; `[passes] domain=code resolve=14`) |
| DB.1 overlay version 2 (`00177a71`) | `ConstraintKind::ForbidEdge { from, to }` -> `{ from, to, transitive, via }`; `ConstraintKind::Invariant { text }` -> `{ text, rule }`; `ConstraintRule` gains `min_tier`; `glia_config::ConstraintDecl` gains `rule` / `transitive` / `via` / `min_tier`; `glia_config::SUPPORTED_VERSION` -> `SUPPORTED_VERSIONS` (`[1, 2]`) + `LATEST_VERSION` (2); new `V2_CONSTRAINT_FIELDS`, `MIN_TIERS`, `MAX_RULE_CHARS`, `external_inputs::validate_declared_entry`, `RULE_IS_OVERLAY_ONLY` |
| DB.3a check (`333ec252`) | `glia_engine::check` violations gain `blocking`, `subject`, `file`, `line`, `caveats`; new consts `RULE`, `FORBID_PATH`, `DEFAULT_MIN_TIER` |
| DB.4 gaps (`50d22541`) | `glia_engine::gaps::CATEGORIES` `[&str; 12]` -> `[&str; 13]` (`rotted_rule`) |
| DB.9b overlay try (`48a94843`) | `overlay_loop::TryReport` gains `rules: Option<RulesReport>`; `StanzaTrial` gains `rule` / `rules`; new `RuleHit`, `RulesDelta`, `RuleTrial`, `RulesReport` and the `RULE_*` / `MAX_RULE_HITS` consts |
| DE.2-DE.4 (`f2d12c54`, `41b08e9f`, `8bbcc0c7`) | engram-export's `ExportStats` / `DiffStats` fields (engram-export only) |
| DE.11 the 0.5.0 aliases removed (`a7f2c70a`) | pyo3 / CLI only: `PyGraph.patterns_experimental`, `glia_py.patterns_vs_rev_experimental`, `glia patterns --experimental` gone |

Additive public items, for reference: the module slots `glia_engine::{identity, memory, rules,
watch}`, `glia_activation::algo::{classes, ppr}` and `algo::reach::bfs_levels`, the `glia-rules`
crate, `glia_code_extractors::generated`; DD.4's `ProjectRoot.label_source` and
`project_roots::{LABEL_MANIFEST, LABEL_DIR, LABEL_ROOT, LABEL_DECLARED}` (`efa0ac66`); DH.17a's flat
`glia_graph::build_elixir` (`77e8d9fa`); DF.9's `third_party_clients` on the WebSocket / gRPC / GraphQL
resolvers (`b4b8565f`); DF.7's `code-domain::endpoint` const-placeholder helpers and the build-time
`NavFact::GoStringConst` variant; DF.15's `NavFact::GoPackageName`. Removed from a `publish = false`
crate: `glia_code_extractors::grpc::is_generated_source` (DE.7 `db774107`, replaced by
`generated::generated_by_banner`). `QueueFramework` gains `Apalis` (DG.1) and `RedisTaskQueue` (DH.3)
and is not `#[non_exhaustive]`: an exhaustive `match` outside glia would break (neuropil has none).

## 5. Behaviour changes with no required code change

### 5.1 Rendering and views

- **SAME_AS edges** (D0.1, DD.5 `1b631092`, DD.6 `fe20779b`, DD.7): member -> class centre (the least
  qname, kind, first graph index), EVIDENCE `resolver:identity`, rules `package_name`,
  `manifest_name`, `message_fqname`, `contract_op`, `contract_route`, `scip_symbol` (FACT / DERIVED;
  `/diverged` when copies differ) and `body_hash` / `path_suffix` (HEURISTIC, Weak, multi-repo only).
  A single-repo build gains them only for contract-op and workspace-package classes; `hud.rs:1516`
  `edge_category_name` prints `?` for them. Add `else if c == SAME_AS { "same_as" }` (and, while there,
  arms for 32-36: IMPLEMENTS, SHARES_DATA_SOURCE, RPC_CALLS, NAVIGATES_TO, CO_CHANGES, which print `?`
  too).
- **SHARES_DEPENDENCY** (31) is never emitted and SHARES_SCHEMA (16) no longer joins MESSAGE_TYPEs
  (DD.16): `hud.rs:1548` can no longer fire; `hud.rs:1533` still fires for CLASS / INTERFACE / STRUCT
  pairs. No qname or NodeId moves.
- **Elixir behaviours as INTERFACE** (DH.17b): `layout/tier_stack.rs:181` `kind_to_sub` has no
  INTERFACE arm (PACKAGE goes to `Packages`), so they move to `Other`; `file_tree.rs:245`
  `kind_priority` ranks them with classes.
- **PROJECT ORIGIN** gains `label_source` (`manifest` / `dir` / `root` / `declared`) (DD.4); neuropil
  reads no ORIGIN.
- **ORIGIN provenance** gains `generated` (a generator header or a codegen file name, DE.7),
  `generated_proto` on `.pbgrpc.dart` / `_pb.js` / `_grpc_pb.js` / `_pb.d.ts`, and `external` on
  unpaired third-party WS_CLIENT / GRPC_CLIENT / GRAPHQL_OPERATION (DF.10 `1f150479`).
- **`ENDPOINT_HIT`** on WS / gRPC / GraphQL clients may end `"external":true` (DF.8 `f1de14d9`); a
  config-folded TS site gains `"alias":"this.<m>"` (DF.20). `hud.rs` and `code_viewer.rs` print Json
  payloads raw; neuropil parses no field.
- **Pub/Sub INFRA_RESOURCEs** carry a second CODE cell, Json (DF.14 `3b271bf4`), after the text one:
  `code_viewer.rs` shows the first CODE cell, still the block's source.
- **OpenAPI DOC_SECTIONs** may carry ORIGIN `bases` (DF.18) or `annotated_base` / `annotated_at`
  (DF.19 `61de0c5b`).

### 5.2 Edges

- **TypeScript:** CALLS from class-typed parameters and locals (DF.22a `4135c395`, rule
  `receiver_type`), into another class's STATE_VAR (DF.17 `04f17964`, rule `receiver_state`), through
  inherited methods on a typed receiver (DF.22b `5ebb3db5`, rule `inherited_receiver`); method-level
  IMPLEMENTS from a concrete override to the member it overrides (DF.22c `8a4b6557`, rule
  `concrete_override`). A TS `super.m()` CALLS edge's EVIDENCE rule goes `inherited_method` ->
  `super_method` (cell_value). No qname moves.
- **Go:** an un-aliased import binds the package clause name (DF.15 `d2f0be22`); method-value handlers of
  typed parameters and locals bind HANDLED_BY (DF.16 `511a2c0c`).
- **PHP / C#:** symbol imports bind through declared namespaces (DF.2 `7fbc75f2`, rule `namespace`);
  wrong `tail_unique` IMPORTS into undeclared namespaces go (symfony-demo -2); PHP heritage INHERITS_FROM /
  IMPLEMENTS (DH.14).
- **Dart:** own-package `package:<name>/..` IMPORTS (DF.4a `edf4079b`), plain-import CALLS and heritage
  rules `global_unique` -> `import_binding` (DF.4b `2692e523`; the rule change is a cell_value break its
  spec did not declare), injectable INJECTS and shelf_router HANDLED_BY (DH.15).
- **Swift / C++ / Rust / Scala / Ruby / Elixir / Clojure:** local receivers (DF.3 `314956fe`), lambda and
  `auto` calls plus CMake genex include roots (DF.5 `f8c3956e`, DF.6 `72a67c22`), libcurl HTTP_CALLS
  (DH.21), Rust trait IMPLEMENTS across files and handler-state INJECTS (DG.13a `2d99ca0e`, DG.13b
  `4bee7730`), http4s HANDLED_BY (DG.10), Ruby INHERITS_FROM / INJECTS / CALLS (DH.13), Elixir CALLS /
  IMPLEMENTS / INJECTS (DH.17a, DH.17b), Compojure / reitit HANDLED_BY (DH.19).
- **Queues / config / events:** Pub/Sub subscription -> topic QUEUE_FLOWS (DF.14, rule `binding`,
  Medium), the DG / DH broker rows, env / flag / secret READS_CONFIG (DG.6, DH.5, DH.6), in-process
  framework buses (DG.3, DH.7), Python signals (DF.12, DF.13).
- **Removed:** external WS / gRPC / GraphQL client pairings (DF.9 `b4b8565f`), stub-only GRPC_CALLS
  (DE.8), multi-routed OpenAPI prefix guesses (DF.19), SHARES_DEPENDENCY / message SHARES_SCHEMA (DD.16).
  DE.1 (`98650a9c`) moves an ambiguous backticked doc mention's DOCUMENTS edge to the least (qname,
  kind, graph index) candidate once (one copy before -> after: Kina 4, glia 159, quokka-stack 127 edges
  moved).
- `flow.rs` animations over Python, Dart, Elixir and Ruby backends get deeper; the tier stack's call
  lanes see more STATE_VAR and FUNCTION (`test ...`) heads.

### 5.3 The build

- **Cargo.lock moved, so PARSER_STAMP moved** (D0.2 and every packet editing a hashed source).
  neuropil's `generate_one` keeps no parse cache, so nothing rebuilds on its side.
- **New stderr lines at boot** (neuropil parses none): `[groups]`, `[const-fold]`, `[evidence-split]`,
  `[identity]` (only when a class forms), `[ts-locals]`, `[elixir-calls]`, `[elixir-types]`,
  `[c-http-client]`, `[eventbus] typed`, `[config] env-read`, `[event-py-signals]`,
  `[grpc-client] skipped generated file`, `[imports] dart plain-import names`; changed `[ts-inherit]`,
  `[recv]`, `[provenance]`, `[c-includes]`, `[go-mounts]`, `[go-routes]`, `[go-package]`,
  `[cpp-scope]`, `[swift-scope]`, `[cron] code`, `[cli-decl] manifest`, `[queues] resolver`,
  `[contract-link]`, `[roots] declared=`; `[schema-link]` is gone. `[graft-memo]` prints only on a
  build that holds a ParseCache (section 7).

## 6. Corrections to the packets' neuropil notes

- **The spec's rationale lists "run_code_passes removed" as an engine API change.** It was
  `pub(crate)` at v0.5.1 (`engine/src/profile.rs:291` at the tag), so DD.7's deletion of it is not a
  public change; nothing for neuropil.
- **DD.4** ("repo-graph wrapper and neuropil read no PROJECT ORIGIN"): correct (grep).
- **DH.17b** ("NodeId-keyed state of behaviour modules orphans; qname-keyed state is unaffected"):
  correct, section 3.
- **DC.4** ("neuropil calls generate_one, which holds no ParseCache ... gains the memos only if it
  adopts generate_one_with_cache or a WatchSession"): correct; section 7.
- **DE.8** ("neuropil state keyed by a removed `grpc_client:` qname goes stale"): correct, section 3.
- **The 0.5.1 handoff, section 2** ("only glia-engine takes `=0.5.1`; the other five keep their own
  crate versions"): superseded. The 0.5.1 bump moved every glia crate to the workspace version, so all
  six take `=0.5.2`.

## 7. Optional: new in-process primitives

All by module path (glia's facade rule).

| primitive | fits in neuropil | packet (commit) |
|---|---|---|
| `glia_engine::watch::WatchSession::open(root, &BuildOptions::default())`, then `apply(&changed_paths)` -> `WatchUpdate` (`seq`, `files`, `groups_rebuilt`, `full_rebuild`, `counts`, `nodes`, `edges`, `phase_ms`), `result()` -> `&GenerateResult`, `poll()` (its own stat scan), `changed_paths()`, `persist(dir)`, `save_cache()`, `seq()` | `watcher.rs` counts notify events and defers "full hot-rebuild of the GliaGraph ... to v1.1 once we have a per-shard incremental rebuild path" (`watcher.rs:1-7`). A session re-reads only the changed paths, rebuilds only the language groups whose inputs changed, and equals a clean build byte for byte (`engine/tests/watch_parity.rs`). Feed it the `PathBuf`s the notify channel already carries (`WatcherHandle.rx`), then rebuild `GliaGraph` from `session.result().merged` instead of `GliaGraph::load`'s `generate_one` (`state.rs:443`). Memory: about twice a generate loop's steady state; glia open 658 MB, 12 updates 668 -> 748 MB (DC.9, pyo3 rows; the CLI's 100-update series levels off at 615-685 MB, DC.8) | DC.1-DC.7c (`36c09a42`, `d0158d20`, `b5b4fcc2`, `d30fc67b`, `9bbf002c`) |
| `glia_engine::generate_one_with_cache(path, &mut ParseCache)` with `ParseCache::load(path)` | a cheaper reload without a session: unchanged files skip tree-sitter, and DC.4's graft memos replay per-file const scans, RPC needles and non-code routing outcomes (`[graft-memo]` line) | DC.4 (`788a2abb`) |
| `glia_engine::memory::release_freed_memory(site: &str) -> Option<Trim>` (`Trim { rss_before_kb, rss_after_kb }`, `released_kb()`; `TRIM_ENV` = `GLIA_MALLOC_TRIM`, `0` opts out) | call after dropping a `GenerateResult` on reload: glibc keeps a dropped graph in per-thread arenas (glia: +600 MB over 15 rebuilds without a trim); prints `[memory] malloc_trim site=<site> rss_mb=<a>-><b>` | DE.10 (`4f1d2dea`) |
| `glia_engine::identity::{identity_classes, IdentityQuery, IdentityView, Collapse}` | a "same thing across repos" overlay in a multi-repo scene | DD.8 (`a7da80dc`), DD.15 (`9262a597`) |
| `glia_engine::rules::{query, query_gql, validate, PRELUDE}` | a query box over the loaded graph: Datalog with the predicate prelude, or a GQL `MATCH` read, rows tiered and located | DB.2a-DB.2c, DB.7c (`9346c5e3`, `90df5ef8`, `02dbb221`, `e1497412`) |

The 0.5.1 handoff's section 6 primitives are unchanged.

## 8. Re-test (neuropil's G24 protocol)

Run in neuropil after the 0.5.0 handoff's sections 2-3 and the pin:

```
cargo check --workspace --all-targets && cargo test --workspace
cargo run -p neuropil-app -- .                              # boot: [parallel] and [timing] lines on stderr
cargo run -p neuropil-app --features test-harness -- .      # in a second shell, then:
python3 tools/test_smoke.py
python3 tools/visual_regression.py compare
```

- Expect `visual_regression compare` to drift on scenes that draw Python, Dart, Elixir, Ruby, PHP or
  Scala backends (new queue, event, config and CLI nodes; deeper call graphs) and Kina's frontend
  (endpoints with real paths, more TS CALLS). Review the diffs, then re-record.
- Hidden nodes, bookmarks and anchored annotations on the section 3 families (and the 0.5.1 handoff's
  section 3 families) need re-setting once.
- From glia, the section 1 recipe should then print `new=0`, and once neuropil has renamed,
  `bash scripts/check-neuropil.sh` as-is should too, with the `note` that neuropil moved from the
  baseline's sha.

## 9. Done vs pending

- **Done on the glia side:** W0-W13 (151 packets, green but DH.16, a bench fixture recorded
  `partial`; the W13 close-out `f468af11` measured 5,552 workspace tests passing) and the docs packets
  DZ.1 / DZ.2 in W14. DZ.2 ran section 1: neuropil compiles against all of them once the 0.5.0
  section 2 edits are in.
- **Pending, neuropil session:** the 0.5.0 handoff sections 2, 3 and 6, this doc's section 2 pin and
  section 8 (with the 0.5.1 and 0.5.2 re-keys).
- **Landed after this doc was first written:** the tweaks, W15-W26 (section 10); DT.13 re-ran section
  1's check at the tweaks' last code commit: `new=0`.
- **Released 2026-10-18:** the 0.5.1 -> 0.5.2 bump is the release commit; James pushes it and tags
  `v0.5.2`. Then `cargo install --path cli --locked` from the tag, so the `glia` on PATH matches.
- **Pending, glia (not owned by a packet):** `check-neuropil.sh` still copies neuropil verbatim (the
  0.5.0 handoff's open item).
- **Checked, no neuropil change:** D0.3-D0.8 (empty module slots); DA.1-DA.20 (the `glia-rules`
  crate, `publish = false`, reached by neuropil only through glia-engine's dependency tree);
  DB.2a-DB.9c beyond section 4 (rules, check, review, gaps, query, overlay loop: neuropil calls none);
  DC.1-DC.10b beyond section 7 (build-stage refactors, byte-identical; watch parity gates); DD.1 / DD.2 /
  DD.8; DE.2-DE.6 (engram-export only); DE.10 (section 7); DE.11 (pyo3 / CLI only); DF.21 (coverage
  text); DH.16 (fixture data).

## 10. The 0.5.2 tweaks (DT.1a-DT.13, waves W15-W26, 2026-10-17)

After the W14 docs, James ran 0.5.2 against Kina (an Angular + Go + Solidity app) and quokka-stack (an
Angular / Flutter + Go app) and asked for the gaps they showed to be fixed inside 0.5.2
(`dev-notes/tweaks-0.5.2.md`, T1-T13): 57 packets, `5af245f6`..`40332f16` plus this docs packet.
Sources: each packet's landed commit body (its `Declared breaks` paragraph) and the routed handoff
paragraphs in `dev-notes/leap-052-corrections.json`; neuropil read read-only at the same `b140d1e` with
the same 50 dirty paths.

### 10.1 The compile check, re-run

DT.13 re-ran section 1's recipe unchanged (copy under `~/.cache/glia-DT.13/npcheck/`, deleted
afterwards; neuropil's `git status` 50 dirty paths before and after), against the glia tree at `9a8e56e0`
plus DT.13's one source edit (a doc comment in `engine/src/passes.rs`, so the stamp is the commit's):

```
[neuropil-check] note - neuropil differs from the baseline's (b140d1ede58ff6e77e8e7326a6a4cf21e822959e dirty=50 src=b690b6b4f6db -> unknown src=73b04787b8a9): NEW/FIXED may be neuropil's own edits, not the leap's
[neuropil-check] ok - new=0 fixed=0 preexisting=0 in 115.1s against glia 0.5.1+p99eb5b8878cba966 (neuropil unknown, lock drift +40 -26)
```

`errors.txt` empty; cargo `Finished ... in 1m 54s`; the lock drift is W14's (+40 / -26): the tweaks add
no crate. Every API change below is on an item neuropil never names: a grep of `crates/` for `NavFact`,
`CellWriteOutcome`, `glia_config`, `check::`, `same_as`, `INVOKED_MEMBERS`, `ModelResolver`,
`client_wires`, `wire_pairs`, `served_routes`, `without_routes`, `EvmContractResolver`,
`GqlgenResolver`, `HttpStackResolver`, `WebSocketStackResolver`, `identities(`, `ContractBreaks`,
`ReviewCounts`, `RuleTrial` and `CheckReport` finds 0 hits.

### 10.2 What re-keys (qname, so NodeId)

As section 3, `NodeId = xxhash(graph_type, repo, kind, qname)`: hidden nodes, bookmarks and anchored
annotations on these stop matching once.

| family | packet (commit) | old -> new |
|---|---|---|
| TS client ENDPOINTs with a trailing query interpolation | DT.4 (`3a830b3f`) | `endpoint:GET:/api/orders${…}` -> `endpoint:GET:/api/orders` (examples-glia); Kina `endpoint:GET:${…}/smr${…}` -> `endpoint:GET:${…}/smr`, `endpoint:GET:/admin/smr${…}` -> `endpoint:GET:/admin/smr`. The hit drops the substitution from `template` and gains `"query":"<ident>"\|"inline"` after `alias` |
| WebSocket clients | DT.3a (`8429d100`), DT.3b (`d405c8e6`) | `ws_client:ws` -> the path the code builds: `ws_client:ws @web` -> `ws_client:/live/orders @web` (examples-glia), `ws_client:ws @quokka_web` -> `ws_client:/api/protected/chat/ws @quokka_web` (quokka, a builder-read client: the node and its WS_CONNECTS are Weak); a config-member base folds (`ws_client:/prices` -> `ws_client:/rt/prices`) and the hit gains `host` |
| NATS producers and consumers | DT.2c (`1986a9cc`) | `queue_producer:<leading literal>` (`orders.`, `orders-`, `audit.`) -> `queue_producer:<subject with * tokens>` or `queue_producer:unresolved:nats`; `queue_producer:shop.orders.%s.placed` -> `queue_producer:shop.orders.*.placed`; `orders.${id}.placed` / `orders.{order_id}.placed` -> `orders.*.placed`; a built consumer subject -> `queue_consumer:unresolved:nats`. A `*` is a literal character to a qname filter |
| anonymous GraphQL documents | DT.11a (`717cfbdd`) | new `graphql_op:<first root field>` for `{ orders { .. } }` / `query ($n: Int) { .. }` |
| EVM clients, between EVM packets | DT.10k (`62533102`) | `evm_call:recv:contract.<m>` (DT.10e, an abigen receiver) -> `evm_call:abi:<X>MetaData.<m>` (or `abi:<X>ABI`); no 0.5.1 build had either |

### 10.3 Engine API changes, against neuropil (0 hits each)

| packet (commit) | item: old -> new |
|---|---|
| DT.1a (`5af245f6`), DT.1q (`e7dfb31d`), DT.10g (`e2fe3ca6`) | `glia_code_domain::NavFact` gains `SliceResult`, `ReExportStar`, `ReExportNamespace`, `ReExportName`, `ExportLocal`, `Exports`, `EvmAbi` (appended; build-time only, old bincode tags kept); new public slots `glia_code_domain::wire` (the contract-model JSON vocabulary) and `::evm` (DT.10a `fb811da7`; DT.10g adds `AbiRef`, `GO_PACKAGE`, `ClientHit`, `client_hit_cell`, `read_client_hit(s)`) |
| DT.1h (`44f20d6b`), DT.1k (`bbe70ea0`) | new `same_as::ALL_RULES` `[&str; 10]` (`RULES` `[&str; 8]` then `MODEL_RULES` `[&str; 2]`; DT.1h made them 9 and 1, DT.1k 10 and 2), `RULE_HTTP_MODEL`, `RULE_GRAPHQL_MODEL`, `IDENTITY_EMITTERS` / `is_identity_emitter`; `CODE_TABLES.identity_rules` 8 -> 10 rows; `glia_graph::ModelResolver`; CODE_PASSES Resolve 14 -> 17 (`gqlgen` DT.5f `6a8b49c3`, `evm` DT.10b `b53fbd67`, `models`) |
| DT.1g, DT.1m, DT.1n, DT.1o, DT.1p | new flat `glia_graph::{client_wires, wire_pairs, graphql_pairs, wire_fields, graphql_fields, graphql_fields_all}` and their `#[non_exhaustive]` types; `ResolvedShape::judged_against`; `glia_engine::contract_breaks::{model_breaks, ModelBreak, ..}`; `ContractBreaks.{models, advisory}`, `Review.{model_breaks, unanchored}`, `ReviewCounts.model_breaks`, `ContractBreakArgs` / `ReviewArgs.models_min_tier`, `review::{caveat_fold, CaveatFold}` |
| DT.5d (`5dd52313`), DT.5e (`787fe464`) | `glia_code_domain::profile::{InvokedMembers, INVOKED_MEMBERS, invoked_member}` (11 rows + the flutter row) |
| DT.6a (`5112e840`), DT.3c (`18af8bb5`), DT.10b, DT.5f | `MountedHttpResolver::without_routes`; `WebSocketStackResolver::served_routes`; `glia_graph::{EvmContractResolver, GqlgenResolver}` (unit structs) |
| DT.7b (`f542b49b`), DT.7c (`6c301ec0`) | `glia_engine::check::{check_with_roots, CheckOptions, Unanchored, ORPHANED, REJECTED, ABSENT, EMPTY_SCOPE}`, `CheckReport.unanchored`; `glia_config::Rejection`, `LoadedConfig.rejections`; `overlay_loop::RULE_CHECKS_NOTHING`, `RuleTrial.{reason, fix}` |
| DT.8b (`75bf3def`) | `glia_rules::gql::Compiled` gains `pub anchor: Option<String>` (a struct literal or exhaustive destructuring breaks) |
| DT.12a (`5bf66c91`), DT.12b (`e3699f97`) | `glia_graph::identity::{identities, HINT_VERSION}` (now pub); `glia_store` `CellWriteOutcome.{node_hint, hint_differs}`, `CellWriteOutcome::set_binding`, `write_cell` refusing a non-v1 hint |
| DT.11b (`c8673f13`), DT.3b, DT.11a | `glia_code_extractors::graphql::{graphql_transport_endpoints, TransportNodes, TransportCounts, operation_root_fields}`, `websocket::{extract_ws_client_nodes_with_consts, replace_ws_client_nodes, WsClientFold, WsFoldCounts}` (`publish = false` crate) |

### 10.4 Behaviour changes with no required code change

- **New node families** (none re-keys an existing one): RPC_PROCEDURE `evm:<Contract>.<fn>` per external
  / public Solidity function, interface member and public getter, HANDLED_BY its METHOD (DT.10a: Kina
  +38); RPC_CALL `evm_call:<scope>.<fn>` and EVENT_HANDLER `evm_event:<scope>.<E>` EVM clients (DT.10c
  `f5dad689`, DT.10i `1d7b02d9`, DT.10d `347d9087`, DT.10j `79a4ca79`, DT.10e `53593205`, DT.10f
  `6606dce9`, DT.10k), paired by RPC_CALLS / EVENT_FLOWS `resolver:evm`; MESSAGE_TYPE
  `message:graphql:<T>` per SDL object / input / interface type (DT.1j `a52201dd`); `unresolved:<family>`
  queue tags in mixed-row files with USES / HANDLED_BY into them (DT.2a `1a7acd56`); nats.go /
  JetStream queue nodes on any receiver (DT.2b `ec7c885c`); ENDPOINTs for awaited generic axios calls
  (DT.1d `0685f7a4`), Dio calls inside closures (DT.5e) and GraphQL client transports
  (`endpoint:POST:<path>`, DT.11b). **Display:** RPC_CALLS (34) already prints `?` in `hud.rs:1516-1550`
  `edge_category_name`; RPC_PROCEDURE / RPC_CALL are in neither `tier_stack.rs:54-60`'s surface kinds
  nor `kind_to_sub` (`:181`), nor `code_viewer.rs:430-447`'s route-like set, so EVM nodes land in the
  default lane. Optional arms.
- **Removed:** EventBusResolver's name-only Solidity event pairs (DT.10b; 0 on every test bed); spurious
  GRAPHQL_RESOLVERs from multi-line argument lists (DT.11b); the RabbitMQ / SNS reading of a `.Publish(`
  a NATS row read (DT.2b); a tail-only WS_CONNECTS (DT.3b); name-only GraphQL pairs whose document
  selects other fields (DT.11a); HTTP_CALLS into test-only ROUTEs (DT.6a); a name-only ref bound to a
  different node than the barrel's (DT.1q). **Undeclared, DT.10j:** in an ethers file a member that a TS
  object-literal / parseAbi / human-readable ABI does not list is no longer an RPC_CALL (counted
  `not_in_abi=`); builds between DT.10c and DT.10j minted it, no 0.5.1 build did.
- **Edges:** IMPORTS `dir_index` (DT.1b `f4dd85de`, Kina +82) and `reexport` (DT.1q, quokka +35; 49
  INJECTS re-ruled `global_unique` -> `import_binding`); handler -USES-> STRUCT `graph:go_wire` (DT.1f
  `4dee0a11`, Kina USES 4 -> 54); TS USES `value_ref`, CALLS `accessor_read` / `accessor_write` (DT.5g
  `9d4494af`, Kina +1026 USES, +91 CALLS); Angular CLASS -> METHOD CALLS / USES (DT.5a `b0856ea5` Kina
  +342, DT.5b `402ca6e6` quokka +245, DT.5c `e22c2d21` Kina +68 class-scope receiver CALLS; DT.5c also
  changes `calls::receiver_type` for a `this.<f>` site from a CLASS to read that class's own field);
  Dart CALLS / USES `method_ref` (DT.5e, quokka +86 / +24); gqlgen HANDLED_BY (DT.5f); GRAPHQL_CALLS by
  root field, rule `root_field` (DT.11a); QUEUE_FLOWS `pattern` (Weak, DT.2c); SAME_AS `http_model` /
  `graphql_model` (DT.1h, DT.1k), emitter `resolver:models`, also inside one repo. A named queue argument
  (`subject="x"`) now reads its value (**undeclared, DT.2c**).
- **Liveness and roles** (neuropil's `flow.rs` animations and tier lanes): Flutter widget / State
  classes carry ROLE COMPONENT and are entries (DT.5e); a used owner's framework- and runtime-invoked
  members and constructors are live (DT.5d); Solidity contracts' external surface is live (DT.10a).
- **Cells** (`hud.rs` / `code_viewer.rs` print Json raw; neuropil parses none): SCHEMA_FIELDS on TS
  interfaces and field-only classes, Go structs, Dart models and GraphQL types (DT.1c `2009dc35`, DT.1l
  `f247e21e`, DT.1j); MESSAGE_TYPE wire cells on Go handlers (DT.1e `d5e6f011`, `model` keys DT.1f),
  GraphQL resolvers / operations (DT.1j) and TS GraphQL callers (DT.1k); trailing `response` / `request`
  on TS / Dart ENDPOINT_HITs (DT.1d, DT.1l), `query` (DT.4), `host` on a folded WS client (DT.3b),
  `"via":"graphql"` on a transport ENDPOINT (DT.11b), evm hits and a JSON CODE on each procedure
  (DT.10a-DT.10g); a CODE text cell on every GRAPHQL_OPERATION read from a document (DT.11a); a
  sub-project `[[constraint]]` now writes its CONSTRAINT entry on the enclosing PROJECT or a MODULE
  (DT.7a `8dbf40f9`), where it wrote none.
- **Answers neuropil does not call:** `glia check` / `PyGraph.check` list rules that check nothing
  (DT.7b, DT.7c; CI exit codes move for an empty-scope rule, 2 -> 0 / 1, and for a sub-project rule
  that is now checked); caveats once per rule (DT.8a `f0dd035a`); located GQL rows (DT.8b);
  `contracts --fields` model drift (DT.1i `d8051a4b`, DT.1n `12a430d5`); model breaks in contract-breaks
  / review (DT.1o `86b76fe5`, DT.1p `40332f16`); `gaps` drops WS-served routes (DT.3c), test-only routes
  (DT.6b `b3537e71`) and implied GraphQL transports (DT.11c `02f78792`); the coverage caveats for
  QUEUE_FLOWS / WS_CONNECTS / GRAPHQL_CALLS (DT.11z `71a13fe2`) and RPC_CALLS / EVENT_FLOWS (DT.10h
  `2fc07e62`); pyo3 traversal category names (DT.9 `b1ecad68`); `glia cell set --hint` (DT.12b).
- **The build:** PARSER_STAMP moved again (parsers, extractors, graph, engine); neuropil's
  `generate_one` keeps no parse cache. New stderr lines at boot are listed in CLAUDE.md's marker list
  (`[passes] domain=code resolve=17`, `[models] ..`, `[evm-link]`, `[imports] reexport`, ...).

Every DT packet: declared no break, DT.1a, DT.1g, DT.1m, DT.1o, DT.13; DT.12a is engram-export only
(`<out>.hints.jsonl`); the rest are above.

## 11. The close-out: dead_symbol false positives (DT.5h-DT.5l, waves W27-W30, 2026-10-17)

James, 2026-10-17 (`dev-notes/tweaks-0.5.2.md` ruling 7): *"lets fix the false positives that counts
as closeout i think"*. The re-demo's remaining `dead_symbol` rows on Kina (52) and quokka-stack (124)
were audited, 103 false and 73 truly dead, and each false mechanism became one packet: DT.5h
(`332e66e5`) Dart names and member reads, DT.5l (`b7f4dace`) runtime-invoked entries and members,
DT.5i (`bf53a65e`) Riverpod receivers and the Dio interceptor ROLE, DT.5j (`784d8553`) Go receiver
typing and values, DT.5k (`aa30014b`) TS receivers. At glia HEAD `1212a3be` Kina reads 32 rows and
quokka-stack 41, the audit's truly dead set exactly; the released 0.5.2 carries all of it. Sources: each packet's landed commit body and the routed
handoff paragraphs in `dev-notes/leap-052-corrections.json`; neuropil read read-only at the same
`b140d1e` with the same 50 dirty paths.

### 11.1 The compile check, re-run

DT.14 re-ran section 1's recipe unchanged against glia HEAD `1212a3be` (the W30 close-out `cc5ac06c`
plus one plan commit; DT.14's own edits are docs, outside every hashed root), copy and target under
`~/.cache/glia-DT.14/npcheck/`, deleted afterwards; neuropil's `git status` 50 dirty paths before and
after:

```
[rename-0.5.0] rewrote 165 token(s) in 50 file(s) under /home/ivy/.cache/glia-DT.14/npcheck/np (0 unknown token(s) left)
[neuropil-check] note - neuropil differs from the baseline's (b140d1ede58ff6e77e8e7326a6a4cf21e822959e dirty=50 src=b690b6b4f6db -> unknown src=73b04787b8a9): NEW/FIXED may be neuropil's own edits, not the leap's
[neuropil-check] ok - new=0 fixed=0 preexisting=0 in 114.5s against glia 0.5.1+p395be331c3dfb5b8 (neuropil unknown, lock drift +40 -26)
```

`errors.txt` empty; cargo `Finished ... in 1m 54s`; the lock drift is W14's (+40 / -26): the close-out
adds no crate. A grep of neuropil's `crates/` finds 0 hits for each item 11.3 names (`NamedEntry`,
`EntryRule`, `is_entry`, `InvokedMembers`, `invoked_member`, `INVOKED_MEMBERS`, `node_ext`,
`method_signature`, `build_dart`, `NavFact`, `MemberRead`, `RiverpodProvider`, `ReceiverType`,
`CODE_TABLES`, `entrypoint_reachable`, `roles_in`, `.properties`, `ENTRYPOINT`).

### 11.2 What re-keys

Nothing on the test beds: no packet moved a qname or a kind, and no node was added on Kina,
quokka-stack, examples-glia or any fixture. Two paths can mint a new node id elsewhere: a Dio / http
call written in a Dart class field's initialiser now runs the client-call detector and mints its
ENDPOINT (DT.5h), and an HTTP client call inside a Python `__main__` guard does the same from the
MODULE (DT.5l).

### 11.3 Engine API changes, against neuropil (0 hits each)

| packet (commit) | item: old -> new |
|---|---|
| DT.5l (`b7f4dace`) | `glia_activation::profile::NamedEntry` gains the pub field `exts: &'static [&'static str]` (a struct literal outside activation / code-domain breaks) and `accepts_ext`; `EntryRule::is_entry_at(kind, name, roles, ext)` (additive; `is_entry` keeps its signature and stays extension-blind); `glia_code_domain::profile::InvokedMembers` gains the pub fields `sigs`, `siblings`, `owner_bases` (a struct literal breaks) and `matches_at` / `gates` / `checks_signature`; new `ExternalBase { library, name }`, `invoked_member_at`, `node_ext(cells)`; `INVOKED_MEMBERS` 12 -> 33 rows (`invoked_member`, 4 arguments, never matches the 20 gated ones: go_runtime, transloco, the four Angular bases, dio; the new apple_app row is ungated); `CODE_TABLES.entry.named` gains `FUNCTION init` limited to `.go`; `glia_parser_go::method_signature(code)` |
| DT.5h (`332e66e5`), DT.5i (`bf53a65e`), DT.5k (`aa30014b`) | `glia_code_domain::NavFact` gains `MemberRead { receiver, name, line }` (DT.5h), `RiverpodProvider { read, notifier }` (DT.5i), `ReceiverType { receiver, ty }`, `MemberWrite { receiver, name, line }`, `Getter`, `Setter` (DT.5k), appended (build-time only, old bincode tags kept); NavFact is not `#[non_exhaustive]`, so an exhaustive `match` outside the workspace stops compiling |
| DT.5h (`332e66e5`) | new flat `glia_graph::build_dart(repo, parses, resolve_source)` (engine's Dart group calls it); `RepoGraph.properties` holds Dart getters / setters (persisted in each shard's code section) |

### 11.4 Behaviour changes with no required code change

- **Edges** (only additions, but one Python move): Dart CALLS `accessor_read` / `accessor_write`, USES
  `method_ref` in any value position, `graph:reads` `accessor_read` / `tear_off` / `static_member`, a
  CLASS's field-initialiser CALLS / USES (DT.5h; quokka-stack +58 unique edges); Dart CALLS
  `graph:dart` `provider_receiver` (DT.5i; quokka-stack +64 CALLS); Go CALLS through range elements,
  qualified fields, func values and composite literals, USES `method_value` / `func_value` /
  `composite_literal`, HANDLED_BY `package_function`, TESTS (DT.5j; Kina +524 pairs, 492 of them
  composite_literal edges; quokka-stack +107); TS CALLS `receiver_type` / `receiver_return` /
  `receiver_accessor_read` / `receiver_accessor_write`, INJECTS from inline `inject(T)`, USES
  `static_member` (DT.5k; Kina +127 edge rows, quokka-stack +18); Python MODULE -> callee CALLS for a
  `__main__` guard, and CALLS `nested_def`, which moves a bare call off a shadowed module-level def onto
  the caller's nested def (DT.5l; one edge in the new fixture, none on the test beds). neuropil's
  `edge_category_name` (`hud.rs:1516-1550`) already names CALLS / USES / INJECTS / HANDLED_BY / TESTS.
- **Evidence:** 305 Kina CALLS pairs through a package-qualified Go field move from emitter
  `graph:calls` rule `receiver_type` to `graph:go_packages` `receiver_field_chain` (DT.5j); neuropil
  reads no EVIDENCE cell (0 hits).
- **Cells:** ROLE COMPONENT on Swift `@main` types and Kotlin Flutter activities (DT.5l), ROLE SERVICE
  on Dio interceptor classes (DT.5i), an ENTRYPOINT cell `{"decl":"<file>:<1-based row>",
  "source":"main_guard"}` on a Python script MODULE (DT.5l). **Correction to DT.5i's and DT.5l's
  neuropil notes** ("neuropil sees ROLE SERVICE on Dio interceptors (roles_in)", "neuropil's roles_in
  reads it"): neuropil calls no `roles_in` and reads no ROLE cell (0 hits); its tier lanes key on node
  kind, so these classes stay in their CLASS lane. The HUD's cell list (`hud.rs:1386-1389`) shows the
  new cells under `cell_label`'s fallback `CELL` (`hud.rs:1552-1569`) with the raw payload. Optional:
  `ROLE` / `ENTRYPOINT` arms there.
- **Liveness:** neuropil computes none (no `entrypoint_reachable`); glia's own answers (`gaps`
  dead_symbol, the rules prelude's `live`, entry flows) move.
- **The build:** PARSER_STAMP moved with every packet (the dart, python, swift, kotlin, go and
  typescript parsers, extractors, graph, code-domain, activation); neuropil's `generate_one` keeps no
  parse cache. New stderr lines at boot are in CLAUDE.md's marker list (`[reads]`, `[dart-scope]`,
  `[dart-riverpod]`, `[go-receivers] typed`, `[go-values]`, `[ts-receivers]`, `[recv] ts receivers
  bound:`, `[resolve] static member uses bound:`, `[invoked]`, `[py-main]`, `[py-nested]`,
  `[swift-app]`).

Every close-out packet declared a break, all above; DT.14 (these docs) declared none.
