# glia 0.5.3 — scale, plus the Engram sidecars (scope seed)

Seeded 2026-10-15 at `6da2965`, before any 0.5.2 wave ran. This is a scope list, not a spec: the packet spec run
happens after 0.5.2 lands and its repo-graph handoff is done, so every packet is verified against what 0.5.2
actually built. Items join this list as 0.5.2's waves surface them (close-out followups with no 0.5.2 owner).

## 0. Decisions (verbatim)

- 2026-09-19 / 09-30 (0.5.1 plan §1, §5): 0.5.3 = **scale**: Stack Graphs-style incremental resolution and
  build-target graphs.
- 2026-10-02 (0.5.2 plan §0.1): moved here: multi-repo `glia watch --with` (Q13), the watch body-only fast path A2
  (Q15), org-internal package routing (Q19); 0.5.2 plan §4.3 lists the spec run's findings with no owner.
- 2026-10-15, after comparing glia with AWS Context: *"i think we have some of this covered in the builds to come and
  can we possibly integrate engram ?"*, then *"yeah thats a 0.5.3 spec if ivee ever seen it so yeah lets spec up 0.5.3
  with what we also need but do the 0.5.2 uplift then hand over to repo-graph session"*. Order: 0.5.2 waves → the
  0.5.2 handoffs → this spec run.
- 2026-10-16, on the first pass of this seed: *"a review queue for human input clarification with an agent should
  work better why do we need a db export of the graph lmao can't we do somehting better then that"*; *"yeah lets get
  the leftover list in 0.5.3"*; *"fix 2."* (the self-graph hygiene cluster); *"3. yep add it"* (watch memory budget);
  *"4. is a good idea"* (one tier vocabulary); *"5. sounds good"* (runner fixes); and on find / flows: *"i noticed this
  my self with my work at work when it did a cache search with flow it didn't find it because it thought the flow was
  named cache thats probably something we should look at for find and flow"*. Recorded in §2 C and §2 F.

## 1. The train

| release | contents |
|---|---|
| **0.5.2** bets | Datalog rules + GQL, `glia watch`, cross-repo identity, Engram anchors, deferrals, the matrix rest (`next-leap-0.5.2.md`) |
| **0.5.3** scale | this doc |
| **0.5.4** proof | precision rate, agent before / after, big-repo timings, calibrated confidence, `decide/` a–e, `glia judge` |

## 2. Scope seeds

### A. Scale (on the train since 0.5.1)

- **Stack Graphs-style incremental resolution** (0.5.1 plan §5: github/stack-graphs archived 2025-09-09; one
  partial-path model across 20 languages is the unknown), with watch's A2 body-only fast path on top (R2-watch).
- **Build-target graphs**: `bazel query` through a `snapshots/` crate, since macros and `select()` defeat a static
  parse. The likely first real non-code domain on the 0.5.0 seam.
- **Multi-repo watch** (`glia watch --with`).
- **Org-internal package routing** (sibling-repo imports resolved to the sibling's symbols, on 0.5.2's SAME_AS).

### B. Engram, data only (proposed 2026-10-15)

Engram's boundary holds: `engram-core` is the only shared crate, Engram depends on no glia crate, and side files
cross as data, never as a code dependency (R2-engram §2 and Engram's 2026-10-02 answer). 0.5.2's Engram anchors
(cross-clone determinism, re-key pairing) are the prerequisite: Engram's learning lives on experience → code links,
so they must survive a rename.

- **Semantic tier in `find`.** Engram writes per-qname embeddings as VECTOR cells into `.glia/vectors.jsonl` (the
  LF.1 sidecar, `external_inputs::VECTORS_FILE`; a build loads them as cells, but no answer queries them today). `find` gains a nearest-vector
  tier below its lexical tiers, HEURISTIC, reported as its own match tier, never mixed into the lexical order. glia
  takes no embedding-model dependency.
- **Corrections as cells.** Engram's consolidated corrections land as DECISION / CONV cells on the node they name
  (`external_inputs::WRITABLE` = CONSTRAINT, DECISION, CONV, VECTOR), HEURISTIC, `source = "api"`.
- **A written cell contract** for Engram as a writer: which cell types, the `source` value, the tier, how a
  re-keyed node's cells follow it (0.5.2 DE's identity_hint pairing).
- **The Engram half** (writing the sidecars, Engram's own eval rule: a new candidate source enters its pool as a
  weight-0 feature and stays only if it beats its ablation) belongs to the Engram session: a handoff ask, not a
  packet.
- Rule for every row that comes from Engram: learning ranks, it never decides what is true. A FACT / DERIVED row
  is never promoted or hidden by a learned score.

### C. Gaps from the AWS Context comparison (candidates for James, from public sources)

AWS Context (announced at AWS Summit New York, 2026-06-17) is a knowledge graph over an organisation's data catalog
and warehouse, queried by agents over MCP. It reads no source code. What it has that glia does not, and how 0.5.3
could answer each (each needs James's ruling before it becomes a packet):

- **Semantic search** → B's VECTOR tier.
- **Learning from use** → Engram's learned gate, through B; never a glia-side learned ranking.
- **Corrections that reach every agent through a review step** — **IN (James, 2026-10-16), as a clarification queue,
  not an accept list.** glia's overlay loop shares an accepted stanza through git only. What glia cannot settle
  (an ambiguous bind, a `suspected_edge`, a rotted rule, a doc mention with two candidates, an unpaired client) becomes
  a queued question carrying its evidence and candidate answers; an agent works it through with a human, and the answer
  lands as the overlay stanza / DECISION cell it implies, with who answered and why. The queue and the answer format
  are glia's (engine + CLI + pyo3); the conversation surface is repo-graph's MCP.
- **Queries scoped to the caller's identity**: matters only if the shared cache or a hosted repo-graph serves more than
  one team. Candidate, likely out.
- **Metadata readable by SQL engines** — **DROPPED (James, 2026-10-16: "why do we need a db export of the graph").**
  A copy of the graph in another engine goes stale and loses the tiers and witnesses. The better answer ships in 0.5.2:
  `glia query` (Datalog, plus GQL, the ISO graph query language) over the live graph, through the CLI, pyo3 and so the
  MCP.
- **Business meaning (glossary, metrics, lineage)**: a business-data domain is where glia would meet AWS Context head
  on. Not proposed; build-target graphs stay the first non-code domain.

### D. 0.5.2 spec-run findings with no owner (0.5.2 plan §4.3)

- Go generic instantiation calls `F[T](...)` mint no CALLS.
- Angular lifecycle hooks and constructors of reached components are dead_symbol rows; TS interface members are not
  METHOD nodes, so interface-typed receivers bind nothing.
- TS: call-initialised locals have no type; module-level instance consts read in a function; statics inherited
  through a subclass name; superclass STATE_VARs through a typed receiver; default-imported class types.
- A `[[constraint]]` with no anchor and no PROJECT scope is orphaned silently; `[walk] tests` is invisible to a layout
  merge.
- RepoId-ordered tie-breaks left in pick_primary, cochange, overlay bind, declared, cells; a witness path among equal
  alternatives can differ between checkouts.
- Per-group remainders: maven / gradle / pubspec / mix / csproj PACKAGE_DEPs; tRPC external marking, Swift protocol
  receivers, Dart export barrels; Rust trait members, Juniper / Sangria; Elixir one-line defs, Absinthe
  import_fields, Ruby mixins.
- Generated HTTP clients (0.5.2 §4.2 decision 5 kept generated-code provenance to gRPC sites).

### F. Committed by James (2026-10-16)

Not candidates: each becomes packets in the spec run.

1. **The §E leftover list is a 0.5.3 group** (per-language precision residuals, with §D's), packeted like 0.5.2's DF /
   DG / DH matrix groups and gated by the substrate-gap fixtures and the matrix.
2. **Self-graph hygiene, fixed first** (§E W2 and W3): every needle scanner reads through the CJ.1 code guard (cron
   YAML, CLI `INVOCATION_PATTERNS`, `data_entities.rs` SQL literals, the migration DSLs), the guard covers the other
   languages' comments and strings (Ruby first), and glia's self-scan gains a zero-phantom gate. A declared
   graph-content change.
3. **A memory budget for long-lived watch.** DC.4's memos add one entry per walked path and a watch session keeps them;
   multi-repo watch multiplies that. Measure RSS over repeated applies on the 0.5.2 WatchSession (DC.7a), then cap or
   evict, with a `[watch]` RSS marker and a gate.
4. **One tier vocabulary.** `rules::value::Tier`, the string consts in `why.rs` / `check.rs` and `implementors.rs`'s
   enum become one code-domain type (§E W1, DA.3), before Engram's HEURISTIC rows add a fifth reading.
5. **Runner fixes** (the 0.5.2 waves found both; the session landed them for the remaining 0.5.2 waves too):
   `closeout.py` lists every followup it could not fold (no packet id, or ids of landed packets only) instead of
   dropping it, and `gen_wave.py --only <ids> --resume <notes.json>` renders a continuation run of unreturned packets
   (a Workflow resume re-runs every agent after the first changed one).
6. **Concept search over flows (find and trace / flows).** `trace <word>` resolves the word to a node by qname, name or
   find's top hit, and its entry-flow fallback matches flow keys made from entry names alone (`engine/src/trace.rs`
   module doc), so "cache" finds a node or a flow *named* cache, never the flows that *go through* a cache (James hit
   this at work). A flow should match a word through what it touches: hop names and qname words, data sources and
   DATA_ENTITY kinds, mechanisms, external hosts, docs governing a hop. Rank by match tier, then the matching hop's
   depth; show which hop matched. `find` gains the same view (the flows through its hits). §B's semantic tier feeds
   this too (open question 2, in part).

### E. From the 0.5.2 waves

Close-out followups that name no 0.5.2 packet are appended here as each wave lands.

- W0 (D0.6): `engine/tests/git_fixture/mod.rs` `root` and `git_mv` are unused in the `contract_breaks` test binary
  (present since 0.5.1). A shared test module compiled per binary; unfinished wiring, not a `dead_code` allow.
- W0 (D0.1): `cargo fmt --check` is not clean in code-domain (`code_span.rs`, `profile.rs`) at 0.5.1; no gate runs
  rustfmt. Candidate: one formatting commit plus a fmt gate in the close-out.
- W1 (DA.3): `rules::value::Tier` is a fourth tier vocabulary, beside the string consts in engine `why.rs` and
  `check.rs` and `implementors.rs`'s uppercase enum. Its words match `why.rs`, so DB maps them 1:1; one shared
  vocabulary is in no 0.5.2 item.
- W1 (DE.1): an ambiguous bare doc mention still binds one candidate at Weak, now stable rather than RepoId-dependent.
  Measured weak binds: glia 240, quokka-stack 193, Kina 4. Alternatives: proximity ranking, or mint no edge (question 4).
- W1 (DD.1): `RUSTDOCFLAGS=-D warnings cargo doc -p glia-activation` fails on links broken before 0.5.2 (`EffectSink`
  in profile.rs; `plan` → the private `ppr_vector`); the `algo` module-doc links went to DD.3 as a correction.
- W1 (DF.1): PHP `use` mid-line (`<?php use \A\{B};`, `namespace X; use \A\{B};`) is still swallowed by
  tree-sitter-php 0.24; the mask reads only line-start statements.
- W1 (DF.2): a PHP namespaced `use` under a root the repo does not own still takes the `tail_unique` guess
  (`use Symfony\Bundle\SecurityBundle\Security` → PACKAGE `App::Security`); a file with several statement-form
  `namespace A; … namespace B;` blocks lists its types under every namespace (needs a parser-side type owner).
- W1 (DF.5): C++ heritage: a `base_class_clause` should become an unresolved INHERITS_FROM ref so `CppScope.bases`
  fills (binds Flutter runners' `this->Show()`, 30 lambda sites, and implicit-this calls to base members); and
  `make_box().get()` arrives as `Attribute { base: "make_box()" }`, bindable through `return_types`.
- W1 (DF.14): a Pub/Sub consumer whose subscription S is bound to topic T ≠ S still pairs exactly with a producer
  publishing to a topic named S; the declared binding contradicts that pair.
- W1 (DF.17): TS locals from `inject()` (`const auth = inject(AuthService); auth.user()`) have no type, and Angular
  template reads (`{{ auth.user()?.displayName }}`) bind nothing (Kina guards and templates).
- W1 (DF.18): an OpenAPI YAML `servers:` sequence at indent 0 (`servers:\n- url: /v1`, what yaml.v2 emitters write)
  reads as no servers; fixing it moves those documents' op qnames, so the packet declares a qname break. Flow-style
  `servers: [{url: ..}]` / `variables: {..}` stay unread.
- W1 (DF.20): `deployment_hosts` could apply a `.replace` span's transform to each alternative and write `hosts`; the
  config-expression grammar could take a SCREAMING_CASE bare identifier (`ADMIN_BASE + '/users'`).
- W2 (DG.7a, DG.8, DH.1): the CJ.1 code guard misses three needle scanners, so glia's self-graph mints phantoms: the
  k8s CronJob YAML scanner in `cron.rs` (`cron:0 6 * * 1:report`), the CLI invocation needles
  (`INVOCATION_PATTERNS`, 10 phantom `cli_invoke` nodes from `cli.rs` comments and test literals), and Ruby source
  (a `# topic "x"` comment in a Karafka file reads as a site; LazyGuard covers `.rs` / `.py` only). Each is a declared
  graph-content change for glia's self-scan.
- W2 (DD.4): `ProjectInfo`, `glia projects --json` and py `project_roots` do not surface the PROJECT ORIGIN's
  `label_source`; showing it moves `py/api_surface` and the `test_arch.py` key pin.
- W2 (DF.16): a Go handler base that is a `self.<field>` alias or a chain rooted in another local
  (`h := svc.Handlers()`) keeps base `h`; binding it needs the registering callable on the ROUTE ref.
- W2 (DH.20): Vapor: SelfMethod HANDLED_BY from a ROUTE (`boot(routes:)` with `use: self.index`, the common controller
  shape) needs the registering method's owner; grouped builders (`app.grouped("users")`) do not join the prefix; a
  client call with a trailing closure mints a phantom `POST /https://...` ROUTE; `app.get("/users")` reads
  `GET //users`; `app.get(use: index)` mints no `GET /`.
- W2 (DH.19): reitit route emission should read the route-data map with `map_entries` (emits `{:post {:handler b}}`
  too; a declared new-ROUTE break); a Clojure FUNCTION behind an unbound `:as` alias has no HANDLED_BY fallback
  (`unique_global_method` looks only at METHODs).
- W2 (DH.15): Dart injectable `@factoryMethod` / `@module` / prefixed annotations (`@di.injectable`); a shelf
  handler that is a method of the enclosing class (`router.get('/x', _list)`) and `@Route.get('/p')` need
  class-scoped handler refs.
- W2 (DH.13): Ruby: dry-auto_inject readers could bind `user_service.find(id)` through a recorded field type; a scoped
  superclass reduces to its last segment (`ActiveRecord::Base` → an app `Base`); registrations in another file than
  the include bind only by the camelised-name convention.
- W2 (DH.14): PHP enum methods are not nodes (`visit_enum` does not walk `enum_declaration_list`), so
  `enum E implements I` gets no method-level IMPLEMENTS.
- W2 (DH.1): a Bunny fanout / topic consumer (`q.bind("logs")`) does not name the exchange a key-less fanout
  `.publish(` goes to; the producer keeps `queue_producer:unresolved:rabbitmq`.
- W2 (DG.3, DG.4, DG.5, DG.7a, DG.8, DG.9, DG.10): Rust / Scala matrix residuals. Event bus: Akka Java / Kotlin
  `getEventStream()`, a bare-imported `Sender<Event>`, async_broadcast, an enum only inferred from
  `broadcast::channel(16)`. GraphQL: Juniper / Sangria (also in D), `#[derive(SimpleObject)]` root struct fields. WS:
  an axum fn mounted on two routes reads its first path; `get(Self::chat)` pairs only through route inheritance;
  Akka's Java DSL and Scala 3 braceless directives. Cron: tokio-cron `JobBuilder` chains, multi-statement run closures,
  a lifetime in call args unclosing the quote scan. CLI: scala.sys.process pipeline operators, an argv built elsewhere,
  `.run` without parentheses, `Seq.apply(..)`. DB: Slick tables through a type alias, a generic base or a qualified
  `extends profile.api.Table[`. HTTP: Akka `pathEnd` / `pathSuffix` / `rawPathPrefix` / `|` prefixes, Scala 3
  brace-less blocks, handler names for inline directive blocks.
- W3 (DH.11): the W2 code-guard cluster grows: the `data_entities.rs` SQL-literal scan and the older migration DSL
  needles do not read through `LazyGuard`, so glia's own `data_entities.rs` / `migrations.rs` test literals mint 67
  self-noise ACCESSES_DATA edges at HEAD (a declared graph-content change for `.rs` / `.py`).
- W3 (DF.10): `glia serves` lists external HTTP ENDPOINTs (match `external`) but not the external WS / gRPC / GraphQL
  clients DF.10 now marks; `serves::external_servers` is ENDPOINT-only.
- W3 (DF.15): CI.5's parser-side `import_dirs` and the `external_pkgs` / `import_local_names` skip set still use path
  spellings, so a Go package imported without an alias under a clause name unlike its directory keeps the
  `api.Server` field-owner miss; needs a graph-side owner mapping.
- W3 (DF.22b): TS inherited calls: a STATE_VAR declared on a superclass is not looked up (`nearest_definer` reads
  `class_methods`, METHODs only); CL.5a's `[recv] constructed receivers` tally still counts a receiver this pass binds
  as unbound.
- W3 (DF.13): Python signals: re-exports through `__init__.py` and star imports bind nothing (the parser emits no
  ImportStmt for `from x import *`). Decorator handlers (`@ee.on`, `@sig.connect`, `@receiver`) anchor to the module,
  not HANDLED_BY the decorated def, because the decorator line sits outside the def's POSITION span (`anchor::attach`,
  every Python decorator site). `marker_swap::swap` could take the evidence rule and the IMPORTS behaviour as
  parameters, shrinking `grafts.rs` `swap_signal_nodes` to a `replace_event_nodes` call.
- W3 (DG.11): ScalaTest: a suite extending an in-house base (`MySpec extends BaseSpec`, BaseSpec extends AnyFlatSpec)
  is not recognised (needs a resolver-level base walk); Scala 3 brace-less `test("x"):`, cases registered in loops or
  helper defs, and fixture-suite bases (`FixtureAnyFlatSpec`) mint no case.
- W3 (DH.2): Laravel queues: `implements ShouldQueue` on the next line, `self::dispatch()` / `static::dispatch()`,
  `dispatchSync` / `dispatchIf` / `Queue::push(new X)`, subclasses of a ShouldQueue base and the global `Redis` alias
  have no rows; a job consumer is anchored at its class line, not HANDLED_BY `handle()` (the CL.4 binding; the
  coverage caveat went to DF.21).
- W3 (DH.5, DH.10): Dart `Platform.environment.containsKey('X')` / `String.fromEnvironment('X')` and Elixir
  `Application.get_env` are unread; PHP `exec(` (collides with `regex.exec`), a fully qualified
  `new \Symfony\...\Process(`, package:args `aliases`, pubspec `executables:` and gemspec `executables.concat(..)` /
  `+=` are unread.
- W3 (DH.8): GraphQL: Absinthe `import_fields` is not expanded across modules; graphql-ruby root classes are found by
  name, not through `query Types::X`; a Dart document passed by value (`document: listOrders`) or QueryOptions built
  outside the call is unread; anonymous `query { .. }` is unnamed. `graphql.rs`'s private `elixir_code_mask` could
  become DH.10's `CodeMap::script(.., Script::Elixir)` once that lexer blanks sigils.
- W3 (DH.9): Ratchet: a Slim / Laravel `->route(` in a Ratchet file reads as a socket route, and `"/ws/$x"` is read
  verbatim; Dart `Uri(scheme: 'ws', ..)` and adjacent-literal concatenation read as the generic `ws_client:ws`; the ws
  call-argument reader is a private copy of `queue_topic.rs`'s.
- W3 (DH.11): sqflite renames, Ecto `rename table(:a), to: table(:b)` (only `a`), `create(table(:x))` and Dart raw
  strings `r'users'` name no table.
- W3 (DH.21): libcurl: handle state is per function (options set in a helper read as GET), the handle key is textual
  (`curl` ≠ `self->curl`), and non-literal URLs (an `snprintf` buffer, `BASE "/x"`) name nothing; a const-table fold
  of `#define BASE "http://.."` would cover the macro case.
- W4 (DG.13a): Rust: no method-level IMPLEMENTS (the parser mints no METHOD under a trait, `visit_type`), and no
  IMPLEMENTS at all for an impl whose self type is declared in another file (glia's own `impl GraphSource for
  MergedGraph` / `for RepoGraph` in `graph/src/traversal.rs`), for tuples, references, blanket impls and impls inside
  fn bodies; the self type could bind by the same path rules (`type_in_scope`).
- W4 (DC.5): glia's self-graph (the installed MCP) has no nodes for the functions in `engine/src/build/mod.rs` and
  `assemble.rs` (`finish_build`, `build_graphs_for_repo`); check on HEAD with the self-graph hygiene group (§F.2).
- W4 (DG.7b): HOCON `include` and `${X}` substitutions are not followed; a Quartz schedule whose expression comes only
  from an included file or an env substitution is dropped (counted in `unresolved=`).
- W4 (DG.2): Alpakka AMQP: a consumer-side `TemporaryQueueSourceSettings(..).withRoutingKey("k")` or
  `BindingDeclaration(..).withRoutingKey("k")` reads as a producer of `k`; gate `.withRoutingKey(` on a receiver chain
  rooted at AmqpWriteSettings / WriteMessage.
- W4 (DE.4): `engram-export/tests/keep_v6.rs`'s list of tests pinning `--since` accuracy could name DE.4's four
  `diff.rs` rename tests (no 0.5.2 packet owns the file).

## 3. Open questions for James

1. ~~Section C: which candidates become packets~~ — review queue IN as a clarification queue, SQL projection DROPPED (2026-10-16). Still open: identity-scoped queries (likely out).
2. Section B: the semantic tier feeds `find` and the flow search (§F.6, 2026-10-16). Still open: does it also feed `pack` and blast-radius seeding?
3. Does B ship with a glia-side writer for tests only (a fixture `.glia/vectors.jsonl`), or wait for Engram's writer?
4. Section E (DE.1): an ambiguous bare doc mention — keep the one Weak bind, rank by proximity, or mint no edge?

## 4. Spec run

After 0.5.2's DZ.2 and the repo-graph handoff: the same method as 0.5.1 / 0.5.2 (per group a spec agent and an
adversarial verifier, one integrator for wave 0 and the release docs), packet ids `E<group>.<n>[letter]` with `E0.*`
as wave 0, packets in `dev-notes/leap-053-packets.json`, the runner's `--release 053` mode.
