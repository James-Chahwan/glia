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
- **Corrections that reach every agent through a review step**: glia's overlay loop shares an accepted stanza through
  git only. Candidate: `overlay propose` writes pending candidates a reviewer lists and accepts.
- **Queries scoped to the caller's identity**: matters only if the shared cache or a hosted repo-graph serves more than
  one team. Candidate, likely out.
- **Metadata readable by SQL engines**: `.gmap` is readable only through glia. Candidate: a Parquet / DuckDB
  projection of nodes, edges and cells.
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

## 3. Open questions for James

1. Section C: which candidates become packets (review queue, SQL projection, identity-scoped queries)?
2. Section B: does `find`'s semantic tier also feed `pack` and blast-radius seeding, or stay in `find`?
3. Does B ship with a glia-side writer for tests only (a fixture `.glia/vectors.jsonl`), or wait for Engram's writer?
4. Section E (DE.1): an ambiguous bare doc mention — keep the one Weak bind, rank by proximity, or mint no edge?

## 4. Spec run

After 0.5.2's DZ.2 and the repo-graph handoff: the same method as 0.5.1 / 0.5.2 (per group a spec agent and an
adversarial verifier, one integrator for wave 0 and the release docs), packet ids `E<group>.<n>[letter]` with `E0.*`
as wave 0, packets in `dev-notes/leap-053-packets.json`, the runner's `--release 053` mode.
