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
- 2026-10-16, on the open questions (with the session's options and recommendations): *"1. yeah outta 0.5.3"*;
  *"2. a makes sense so does b"* (the semantic tier feeds `find` and the flow search, and `pack` as a fallback only;
  never blast-radius); *"3. A first and get the hand off to engram ready we may need to decide what happens to engram
  soon"* (fixtures, contract and validator in glia first; the Engram ask drafted in `dev-notes/engram-ask-0.5.3.md`,
  not sent); *"4. D yeap totally a great call"* (proximity, then the clarification queue).
- 2026-10-16, later: *"well shouldn't we bring parts of engram in if we need the embeddings ?"* and *"okay i don't
  understand the issue of using the embedding models thats tens of mb when it solves isn't it locally run in engram i
  swear"*: glia embeds locally itself (fastembed / bge-small, as Engram does), §B.
- 2026-10-16: *"also its great to take this from engram and lets us focus on the substrate vs the specialisation"*.
  The split this release follows: glia is the substrate (the graph, its embeddings, query and rules, the answers every
  consumer reads); consumers are the specialisation (Engram: learning from use over session history and consolidated
  memory; repo-graph: MCP transport and the human conversation surface, including the clarification queue's).
- 2026-10-16, on Engram's evaluation and corrections (answering "sure" to the session's recommendations): the
  determinism check runs now at 0.5.2 wave 5 (`3921a636`); the Kina + glia replays (re-keys with X1 off and on,
  `flat-ppr` vs `flat-fs`) run once at the 0.5.2 release candidate after W14; the 8 public streams only if those pass.
  Corrections: James then, *"no still in 0.5.3"* - the Engram corrections writer stays in 0.5.3 (§B).

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

### B. Embeddings in glia; Engram, data only (proposed 2026-10-15, revised 2026-10-16)

James, 2026-10-16: *"well shouldn't we bring parts of engram in if we need the embeddings ?"*, then *"okay i don't
understand the issue of using the embedding models thats tens of mb when it solves isn't it locally run in engram i
swear"*. So glia embeds itself, locally, with the library Engram uses (fastembed-rs on the ONNX runtime, bge-small;
`engram-live`'s `local-embed` feature): inference runs in-process, the model downloads once. glia depends on fastembed
directly and copies no Engram code, so the boundary holds: `engram-core` is the only shared crate, Engram depends on
no glia crate, and anything between them crosses as side files (R2-engram §2, Engram's 2026-10-02 answer). 0.5.2's
Engram anchors (cross-clone determinism, re-key pairing) still matter: Engram's learning lives on experience → code
links, so they must survive a rename.

- **`glia embed`, a snapshot step (never inside `glia build`,** like `history sync` / `scip import`): embeds each
  node's text - name, qname, leading_doc and code, since Engram measured full text beating name + qname 78 vs 54 of
  100 - and writes VECTOR rows to `.glia/vectors.jsonl` (the LF.1 sidecar, `external_inputs::VECTORS_FILE`; a build
  already loads it as cells). Each vector is cached by a hash of its text, so a re-run embeds only changed nodes; rows
  carry `model` and `dims`. Its own crate behind a Cargo feature (the CLI on; whether the PyPI wheel ships it is
  measured in the spec run). Determinism needs no special handling: the text-hash cache gives one machine the same
  vectors every run, and nothing gates on cross-machine layout bytes (James: the byte-identical concern "could be
  overstated").
- **When embeds run** (James: *"we could have a way to determine when to run the embeds"*). The cache makes a run
  cost only the changed nodes, so the question is the trigger. Candidates for the spec run:
  `glia watch` embeds changed nodes in the background after an apply, debounced; `install-hooks` adds a post-commit /
  post-merge `glia embed`; a semantic query counts nodes whose text hash has no vector and embeds them inline under a
  small threshold, else answers from what it has with an absence caveat ("<n> nodes not embedded; run glia embed");
  an explicit `glia embed` always works. A `[embed]` marker reports embedded / cached / stale counts.
- **Semantic tier** in `find` and the flow search (§F.6), and `pack` as a fallback (§3): the query text is embedded
  in-process with the sidecar's model; HEURISTIC, reported as its own match tier with its similarity, never mixed into
  the lexical order. Blast-radius never seeds from vectors.
- **The sidecar stays writer-agnostic:** the written VectorRow contract and its validator (rows whose qname matches
  no node, mixed models, wrong dims) hold for any writer, and Engram can read glia's vectors (same model, same space)
  instead of embedding again.
- **Engram's embedding learnings** (engram-34's answer, 2026-10-16, measured unless dated; binding for the spec run):
  - Model: fastembed-rs `EmbeddingModel::BGESmallENV15` = `Xenova/bge-small-en-v1.5`, `onnx/model.onnx`, fp32, 384
    dims, CLS pooling (ort 2.0.0-rc.9 / ORT 1.20). Not `BGESmallENV15Q`, a different artifact. Rows name the exact
    artifact in `model`; a reader skips rows whose model differs from its query model.
  - Text: Engram embeds name, qname, leading doc (500-char cap) and up to 10 imports not already in name + qname; never
    a code body. Its 78-vs-54 was that text vs name + qname + imports (the gap was paraphrased questions, 28 vs 9 of
    44). **Code bodies were never measured**: the spec run A/Bs them. fastembed silently truncates at 512 tokens, so
    code goes LAST and truncation never eats the doc.
  - Query: the raw question, same function, no BGE instruction prefix (untested; a reader-side A/B). fastembed
    L2-normalises; rank by raw cosine (a hashed sparse projection lost 11-13 of 100 lookups).
  - **No cosine floor means "no match".** A dense tier is never silent, and Engram's best silence rule failed held-out.
    So the semantic tier never decides presence: `find` / the flow search / `pack`'s fallback keep the lexical absence
    ("nothing matches by name") beside any semantic rows.
  - Determinism: on CPU a text's vector is bit-identical alone, padded or batched (256-text chunks), so the text-hash
    cache holds. GPU bit-identity is untested: the determinism gate stays CPU-only.
  - Gotchas: the model cache defaults to `.fastembed_cache` under the CWD (set `FASTEMBED_CACHE_DIR` to a fixed user
    cache, never a repo dir); two ORT sessions at once thrash (13 s vs 0.8 s), and each takes every core, so cap
    intra-op threads and never embed concurrently with a build's rayon pool. GPU recipe (if wanted): Microsoft's
    onnxruntime-linux-x64-gpu-1.20.1, ort features `cuda, load-dynamic, ndarray, half`, `ORT_DYLIB_PATH`,
    `CUDAExecutionProvider::default().build().error_on_failure()` (else silent CPU fallback); never combine
    `ort-download-binaries` with `load-dynamic`.
- **Corrections as cells: IN 0.5.3 (James, 2026-10-16: "no still in 0.5.3"); Engram has no source yet.** Engram forms no consolidated corrections today: its Cell C (lessons from
  user corrections) failed its ceiling check on 2026-10-11 (2 of 13 recurrences in a later session) and was not built,
  so the Engram writer of `.glia/cells.jsonl` is new Engram work (forming consolidated corrections, then exporting
  them). It is judged on glia's side by whether the cells improve answers, not by Engram's recall ceiling, which
  measured a different use (corrections as recall cues). glia's side already exists: the LF.1 sidecar and the `orphaned_cell` gaps row. Agreed lifetime rule
  for any writer: the writer owns its rows, glia never deletes another source's rows, an orphan is kept, reported and
  attached to nothing, and the writer re-anchors or deletes it on its next write; an entry `id` is a hash of the
  correction's content.
- **Engram's corrections writer, scoped** (engram-34, 2026-10-16, with James's rulings: code-naming corrections only;
  Engram writes `.glia/cells.jsonl` and never commits it, James commits; the live writer reads the session transcript at
  sleep, the hook log stays text-free). DECISION = a choice about what specific code does or keeps; process rules are
  never exported. Few rows (Engram's read: about 7 of 46 recorded corrections name code, all Kina; 0 for glia). The
  node is picked from the Symbol facts the corrected edits touched (or "no node", no row); `qname` + glia's
  `identity_hint`, no `kind`. One row per (node, class), rewritten in place; `id = "engram:" + hash(class key + node
  hint)`, `source = "api"`; written at each Engram sleep, only Engram's own rows touched. glia's answers to its
  questions (from the code): free-form ids of 1..=128 chars; extra entry keys are stored but unread, so the why stays
  in `text`; qname + hint binds without `kind` unless two same-qname nodes defeat the hint (`ambiguous`, nothing
  applied); other writers exist (`glia cell` CLI, pyo3 `set_cell` / `write_cell`), so a writer takes
  `.glia/cells.lock` (exclusive create, 30 s stale) or shells out to `glia cell set`, which adds validation and
  redaction.
- **A caller hint on the cell write surfaces** (engram-34, 2026-10-16). `glia cell set` (`SetArgs`: repo, qname, cell,
  json / text / file, kind, model, dims) and pyo3 `write_cell` / `PyGraph.set_cell` take no hint; the store's
  `CellWrite::with_hint` already does, and the store keeps the caller's hint over the bound node's. Today the hint is
  captured only when `.glia/graph` is fresh, so a writer after edits (Engram at sleep) writes a hint-less `pending`
  row; it binds by qname at the next build, and a later rename orphans it (`--rekey` needs a hint). Add `--hint` /
  `hint=`, validated as an identity hint; when the layout is fresh and the caller's hint names a different node than
  the qname binds, report it (`target`), never pick silently. No 0.5.2 packet owns `cli/src/cmd/store/cell.rs` or
  `py/src/cells.rs`. Interim for any writer on 0.5.2: build first (`glia build`), then `cell set`, so the store
  captures the hint.
- **Engram ask: the repo's identifier vocabulary** (engram-34, 2026-10-16; James picked 0.5.3).
  `glia-export-engram --vocab` writes `<out>.vocab.jsonl`: every identifier-like token (`[A-Za-z_][A-Za-z0-9_]*`,
  3+ chars) in the repo's tracked text at the exported commit, including everything the export filters (tests,
  generated, vendored, noise, non-code); rows `{"t", "files": [file ids], "n"}` with `files` capped at 16 (`"common":
  true` past it), sorted by token, bound to the gmap by digest like `.ppr.jsonl`. Why: Engram's "(not in memory: X)"
  note caught 60/60 never-existed names on Kina and 43/60 on glia with 0 false flags, but only 0.74 / 0.61 of what it
  flagged is truly absent from the repo; the rest are real names the export never emits as symbols (locals, fields,
  keys, filtered paths), and only glia sees the whole tracked text. Engram uses it under its own rule (a live A/B for
  the note; name -> files as a needle-tested pool source). Measured on glia's tree (2026-10-16, Engram's rule, a
  Python count over `git ls-files`): 3,640 text files (33.2 MB), 52,764 tokens, 4,779 in more than 16 files; 2.7 MB of
  JSONL per export, 0.6 MB deflated. Spec questions: tracked files from the git index vs the walk (which also reads
  untracked, unignored files); a full file per `--since` step vs a token / file diff. Substrate value beyond Engram:
  `find`'s absence could say "no node named X, but X occurs in <files>" (a FACT-tier "named but not modelled"
  signal, and a blind-spot candidate for `gaps`).
- **Spec item: scope-level conventions.** In glia, CONV is a free-text note on ONE node (overlay `[[note]]`; the
  format spec's "conversation history"), not a convention. A convention that covers a scope ("modals are centred,
  never bottom-anchored": every modal component) has no home: until it has one, Engram writes it as a CONV note on the
  corrected node (`convention: ...`). The spec run decides whether a convention attaches to a module / component /
  project scope (the reflexion model's components are a candidate anchor) and which answers show it.
- **Engram's half** (corrections, its own eval rule: a new candidate source enters its pool as a weight-0 feature and
  stays only if it beats its ablation) is a handoff ask, not a packet: `dev-notes/engram-ask-0.5.3.md`, unsent.
  Learning from use (the release gate over agent-session history) stays Engram's: glia has no session data.
- Rule for every learned or embedded row: it ranks, it never decides what is true. A FACT / DERIVED row is never
  promoted or hidden by a learned or similarity score.

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
- W5 (DG.13b): Rust DI depth: a handler extracting a wrapper struct (`State<AppState>` holding the services) injects
  AppState, not its fields; following AppState's field types would reach the services.
- W5 (DH.12): elixir-grpc: a file only handed a channel (`GRPC.Channel`, no `GRPC.Stub`) gets nothing for a service
  without a Service / Svc suffix; the server side (`use GRPC.Server, service: ..`) mints no GRPC_SERVER.
- W5 (DD.7): the SCIP snapshot is read and its fresh documents re-hashed twice per build (the Resolve identity pass,
  then the Post SCIP stage); a cache on CodeBuildCtx could share the read.
- W5 (DH.7): Laravel string events (`Event::dispatch('order.placed')`, `Event::listen('order.*', ..)`), the
  EventServiceProvider `$listen` array, `broadcast(new X)`, paren-less Elixir remote calls and Rails
  `ActiveSupport::Notifications.monotonic_subscribe` / `subscribed` are unread.
- W5 (DH.3): Dart SNS (`aws_sns_api` `sns.publish(topicArn: ..)`) has no row; redis_task_queue's
  `worker.handle(type, handlerFn)` and dart_amqp's `consumer.listen(cb)` bind no HANDLED_BY; dart_amqp
  `exchange.publish(msg, 'routing.key')` never reads the routing key.

## 3. Open questions for James

All four answered 2026-10-16 (§0); recorded as rulings:

1. Identity-scoped queries: **out of 0.5.3.** Revisit with a hosted repo-graph; provenance on every node and cell
   first, filtering by the host.
2. The semantic tier feeds `find` and the flow search (§F.6), and **`pack` as a fallback only** (seeds only when
   nothing matches by name, each labelled HEURISTIC with its similarity). Blast-radius never seeds from vectors.
3. **glia's half first, against fixtures** (fixture `.glia/vectors.jsonl` files, a deterministic test-only embedding,
   the VectorRow contract and its validator), **then glia embeds itself** (revised 2026-10-16, §B): `glia embed` with
   fastembed / bge-small, the query embedded in-process. The Engram ask (`dev-notes/engram-ask-0.5.3.md`, draft)
   shrinks to corrections as cells and reading glia's vectors; James decides Engram's direction before it is sent.
4. An ambiguous bare doc mention: **proximity, then the clarification queue.** Bind the one candidate a recorded rule
   picks (same directory / project / service as the doc, or its owner also named in the section) at Weak with the rule
   as evidence; when no rule separates them, mint no edge, queue the question (§2 C) and report a gaps row. Measure at
   spec time how many of glia's 240 / quokka-stack's 193 / Kina's 4 proximity settles.

## 4. Spec run

After 0.5.2's DZ.2 and the repo-graph handoff: the same method as 0.5.1 / 0.5.2 (per group a spec agent and an
adversarial verifier, one integrator for wave 0 and the release docs), packet ids `E<group>.<n>[letter]` with `E0.*`
as wave 0, packets in `dev-notes/leap-053-packets.json`, the runner's `--release 053` mode.
