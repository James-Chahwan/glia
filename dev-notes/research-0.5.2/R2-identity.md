# R2 — cross-repo identity (0.5.2 bet B)

Research only, at glia `2170ff8`. The measurements come from a four-repo merge of git-archive copies:
`research/probe/r2-quokka-stack` (Angular web + Capacitor android + Go `turps`), `r2-quokka_web` (the standalone web
repo), `r2-quokka_app` (Flutter) and `r2-quokka` (Ionic). It was run with
`GLIA_NO_PERSIST=1 ./target/debug/glia merge ... --out research/ident/quokka-merge.json`. The analysis script is
`research/ident/dups.py`, and the blast-radius output is `research/ident/blast-login.json`.

## 1. What exists

- **No dedupe anywhere.** `merge_layouts` concatenates member graphs and states it: "There is no cross-repo node
  dedupe" (engine/src/merge.rs:25). A repo held by two members is refused, never fused (merge.rs:250-262). A NodeId
  hashes `(graph type, RepoId, kind, qname)` (`NodeId::from_parts`, graph/src/resolvers/package.rs:70), so one
  qname in two repos is two nodes. TODO.md:29 lists "Cross-repo node dedupe" as open.
- **Cross-repo "sameness" is already emitted, pairwise, as seven SHARES_* categories**, each a clique over the
  class members:

  | category (id) | code-domain/src/lib.rs | emitted by |
  |---|---|---|
  | SHARES_SCHEMA (16) | :326 | shared_schema.rs:89; message_schema.rs:60-62 via `emit_cross_repo_pairs` |
  | SHARES_DATA_ENTITY (22) | :339 | db.rs:358-360, :392 |
  | SHARES_CRON_SCHEDULE (24) | :346 | cron.rs:52 |
  | SHARES_CONFIG (27) | :356 | config.rs:51 |
  | SHARES_INFRA_REF (29) | :364 | iac.rs:88 |
  | SHARES_DEPENDENCY (31) | :370 | package.rs:37-57 |
  | SHARES_DATA_SOURCE (33) | :415 | db.rs:139-141 |

  The clique helper is `emit_cross_repo_pairs` (graph/src/resolvers/mod.rs:127-151): k members give k(k-1)/2
  edges. SHARES_SCHEMA and SHARES_DATA_ENTITY are carry edges, so blast radius and liveness walk them
  (code-domain/src/profile.rs:105-106). SHARES_SCHEMA also has PPR weight 2.0 (:155).
- **`IdentityIndex` is move identity over time inside one repo, not cross-repo equivalence**
  (graph/src/identity.rs:1-23). Its key `(kind, file basename, local name chain, FNV body hash)` (:46-57, :140-181)
  is still exactly a vendored-copy detector: `Identical` (same body) and `SameName` (same kind, basename and local
  chain) are its HEURISTIC tiers (:106-114).
- **PROJECT nodes carry the manifest's package label and ecosystem** in an ORIGIN cell
  (engine/src/walk.rs:567-596; code-domain/src/project_roots.rs:23-31). That is the key a shared-library class
  needs: `package:npm:@org/lib` in repo A ≡ the PROJECT labelled `@org/lib` in repo B. TODO.md:27, "Org-internal
  package routing", wants A's imports bound to B's real definitions, and names the workspace manifest (LC.10b) as
  its prerequisite. LC.10b has landed.
- **Named container sections are additive.** Readers look sections up by name (store/src/container.rs:89-104), so
  a new section never breaks an old reader.

## 2. Measured on James's own multi-repo set

Four repos, 4,317 nodes, 7,780 edges (341 cross).

| class | evidence | count |
|---|---|---|
| same `(kind, qname)` in ≥2 repos | PACKAGE_DEP 17, MODULE 5, STATE_VAR 2, ROUTE 2 (`page:/`, `page:/home`), CONFIG_KEY 1, CLASS 1, METHOD 1, PROJECT 1 | 30 |
| SHARES_DEPENDENCY | 41 edges over 17 classes (sizes: 5 of 2, 12 of 3). A star needs 29 edges | 41 |
| SHARES_SCHEMA | 36 edges over 22 classes (2: 9, 3: 12, 4: 1). A star needs 36; small classes gain nothing | 36 |
| same `(kind, qname minus its leading path segment)` in ≥2 repos: vendored / subtree copies, many diverged | METHOD 534, MODULE 78, INTERFACE 63, CLASS 62, FUNCTION 44 | 781 |
| byte-identical source files (≥200 B) across repos | quokka-stack/quokka_web ≡ quokka_web: 41; quokka-stack ≡ quokka_app: 7 | 48 |

What duplication does to an answer: `glia blast-radius r2-quokka-stack "POST /auth/login @turps" --with r2-quokka_web
--with r2-quokka_app --direction backward --depth 3` returns 11 rows. Eight of them are four symbols, each twice
(`AuthService::login`, `AuthService::legacyLogin`, `LoginComponent::login`, and the ENDPOINT): once from the stack's
`quokka_web/` copy and once from the standalone repo, at different lines (113 vs 114, 108 vs 127). The rows carry no
repo label, so only the path prefix tells them apart. The same answer also lists three DOC_SECTION contract ops for
one operation (swagger.json, feature.yaml, the handler annotation). That contract-op class also exists INSIDE one
repo.

## 3. Which equivalence classes are real

Identity means "the same thing": collapse it in answers. Sharing means "two things that use the same name": keep it
as a relation.

| class | key | tier | real? | kind |
|---|---|---|---|---|
| packages | PACKAGE_DEP `package:<eco>:<name>` | FACT (manifest-declared) | yes: 17 classes above | identity |
| shared library ↔ its consumer | PACKAGE_DEP name ≡ PROJECT ORIGIN (ecosystem, label) | FACT (both sides declared) | yes in org monorepos and multi-repo orgs; none in the quokka set; needs a label spike per ecosystem | identity, and the enabler of TODO.md:27 |
| proto / avro messages | MESSAGE_TYPE full qname `message:<flavor>:<fq name>` (message_schema.rs:1-13) | FACT for the name; a `diverged` flag from comparing SCHEMA_FIELDS | yes (5 MESSAGE_TYPE in quokka-stack; common in client/server splits) | identity, possibly diverged |
| contract ops | (METHOD, normalised path) over `contract::...` DOC_SECTION ops | DERIVED | yes, cross-repo AND intra-repo (3 ops for POST /auth/login) | identity |
| vendored / subtree copies | IdentityIndex body hash (Identical), then path-suffix + (kind, local) congruence (SameName) | HEURISTIC | yes, common: 48 identical files, 781 suffix matches | identity, possibly diverged |
| env / config keys, DB tables, cron schedules, infra refs | name | FACT for the name, not for the thing | two services reading `DATABASE_URL` are not one node | sharing: keep SHARES_* |
| same checkout twice | RepoId | n/a | refused by merge (merge.rs:250-262) | keep refusing |

## 4. Design options

1. **Merge nodes** (rewrite members to a canonical id). Rejected. It destroys per-repo addressability and
   provenance, cannot represent two diverged copies, cannot be undone on a re-merge, and makes a merged shard no
   longer the member's shard, which breaks LC.10b's "merge == joint build" guarantee
   (engine/tests/merge_layouts.rs:165).
2. **SAME_AS clique edges.** Additive and simple, but k(k-1)/2 edges, and traversals still visit every member.
   It is today's SHARES_DEPENDENCY shape.
3. **SAME_AS star edges** to a deterministic representative (the minimum NodeId in the class, or the declaring side:
   the PROJECT for a library, the proto-declaring repo for a message). O(k) edges. Each edge carries one EVIDENCE
   cell: emitter `resolver:identity`, rule `package_name` / `manifest_name` / `message_fqname` / `contract_op` /
   `body_hash` / `path_suffix`, plus a `diverged` bit.
4. **Class table** as a new named section in `cross_stack.gmap` (`Vec<Class { rep, members, rule, tier,
   diverged }>`). O(k) and richer, but it is a second representation every consumer (delta, why, check, Engram
   export) must learn.

Union-find vs e-graphs:

- The congruence we need is "equivalent parents ⇒ same-named children are candidates". Module M ≡ M' makes class C
  in M a candidate for C in M', then method m. A deterministic worklist over `(parent class, kind, local name)`
  covers it, confirmed by body hash for vendored copies. That is Downey–Sethi–Tarjan / Nelson–Oppen congruence
  closure restricted to the containment function.
- **egg** (equality saturation, e-class analysis) is built for term rewriting. We rewrite nothing. It would be a new
  dependency (a Cargo.lock change moves PARSER_STAMP), and its e-class ids are not stable across runs without extra
  work.
- **egglog** unifies Datalog and e-graphs. It becomes interesting only once the Datalog bet lands: identity rules
  like `same_as(X,Y) :- package_dep(X,N), project_label(Y,N)` would be Datalog rules feeding the same class table.

Recommendation: a hand-rolled union-find (union by minimum id, so the representative is deterministic; path
compression) in `activation::algo`, plus code-domain rules.

## 5. Recommendation

**SAME_AS star edges, computed by a union-find `identity` resolver, persisted as ordinary cross edges, and collapsed
at query time.**

- **One new edge category, SAME_AS.** It takes the next free id after 0.5.1's allocations, allocated in
  code-domain/src/lib.rs only, per the id rule. It is not a carry edge. The quotient view (below) makes carrying
  unnecessary.
- **Resolver `graph/src/resolvers/identity.rs`**, registered as the last `Resolve` PassSpec in `CODE_PASSES`
  (engine/src/profile.rs:110-238). It reads nodes, never other resolvers' edges.
  - FACT and DERIVED classes first: packages, package ↔ project, message fq names, contract ops.
  - Then HEURISTIC vendored copies: IdentityIndex body hash, then path-suffix congruence under already-equal parents.
  - Emits the member → representative stars in sorted order, with `diverged` for same-key / different-body pairs.
- **Format.** No FORMAT_VERSION bump: the edges live in `cross_stack.gmap`. Emitter `resolver:identity` means a
  layout merge drops and recomputes them over the union (merge.rs:224-227), as it does for every resolver.
- **Query time.** `activation::algo::classes` builds classes from the SAME_AS edges (O(E_same_as)). A domain-free
  `Quotient<G: GraphSource>` adapter maps each node to its representative. `blast_radius` / `trace` / `tests_for` /
  `find` / `serves` / `arch` collapse duplicates into one row with `also_in: [{repo label, file, line}]` and the
  class tier. The quokka blast above goes from 8 code rows to 4.
- **SHARES_* for sharing stays.** Whether PackageResolver's SHARES_DEPENDENCY clique and the message half of
  SHARES_SCHEMA become SAME_AS stars (an `edge_removal` break) or stay beside them (additive) is question 1 below.
- **Follow-on, not core.** Org-internal package routing (TODO.md:27): a resolver that, given PACKAGE_DEP(A) ≡
  PROJECT(B), binds A's external import specifiers (the IMPORTS cell survivors of the A16.4 filter,
  grafts.rs:464) to B's modules and exported symbols, emitting cross IMPORTS / CALLS. It is per ecosystem, so npm
  and Go first.

## 6. What consumers need

- **repo-graph MCP**: single-repo today (server.py:108-131 `generate(target)`). It already renders `⧉ cross-repo`
  hops (server.py:511-514), and the 0.5.0 handoff offered an optional multi-repo mode over `merge_gmaps`
  (dev-notes/repo-graph-handoff-0.5.0.md:240). Once that mode exists it needs one row per class with `also_in`,
  not N rows.
- **neuropil**: single-repo `generate_one` (neuropil crates/neuropil-app/src/state.rs:443). It dedups NodeIds that
  sit in several per-language graphs and treats `package:` / `external:` qnames as external (state.rs:~520-541).
  No multi-repo need today. SAME_AS would let it draw one node per class if it goes multi-repo.
- **Engram**: keys facts per repo. `CATEGORY_MAP` (engram-export/src/lib.rs:304) needs a SAME_AS row, and a test
  guards that every category has one. Mapping to an existing EdgeKind (e.g. `Cooccurs` 0.3) needs no Engram
  contract bump. A new EdgeKind is a v7 ask.
- **CLI multi-repo users** (`--with`, `glia merge`, `glia arch`): the blast / serves / flows duplication measured
  in §2.

## 7. Components (LOC, crate)

- `activation::algo::classes`: union-find plus the `Quotient` GraphSource adapter, domain-free. 150-250 LOC,
  `glia-activation`.
- code-domain: the SAME_AS id, an `identity_rules` table row per class in `CODE_TABLES` (kind pairs, key, tier), and
  the evidence rule names. 60-120 LOC, `glia-code-domain`.
- `graph/src/resolvers/identity.rs` IdentityResolver (packages, package ↔ project, messages with the diverged flag,
  contract ops, vendored copies via IdentityIndex plus congruence) and its `[identity]` fired_on marker. 450-800 LOC,
  `glia-graph`.
- Engine: the PassSpec row, answer dedupe with `also_in` in find / blast / trace / tests_for / serves / arch rows.
  250-500 LOC, `glia-engine`.
- CLI `glia identity` report (classes by rule and tier, diverged pairs) plus a flag to show uncollapsed rows; pyo3
  `identity_classes()`; surface snapshots. 150-250 LOC, `glia-cli` / `glia-py`.
- engram-export `CATEGORY_MAP` row. ~5 LOC, `engram-export` (excluded workspace).
- Tests: fixtures for two repos sharing an npm package, a proto message (one diverged), a vendored subtree with one
  diverged file, and an npm workspace lib consumed by a sibling repo; a merge == joint-build parity check with SAME_AS
  present. 400-600 LOC.

Optional:

- Migrate the Package / MessageSchema cliques to stars. 50-100 LOC, `glia-graph` (edge_removal break).
- Org-internal package routing, npm + Go. 500-900 LOC, `glia-graph` (+ an `engine` flag).

Total core **1,060-1,925 + tests 400-600**. With routing: +500-900.

## 8. 0.5.1 packets it needs first

- **CE-1 SCIP ingest.** A SCIP symbol is `scheme manager package-name version descriptor`, and cross-repo navigation
  treats package + version + qualified name as the unique id. That is a FACT-tier identity key for free wherever
  both repos are SCIP-indexed. CE-1 deliberately binds by position, "never by parsing symbol strings", so the
  identity resolver would read the package / descriptor part as a separate, declared use.
- **CD-7 FORMAT_VERSION 3.** CODE cells become spans, and the vendored-copy body hash reads CODE text through
  IdentityIndex (identity.rs, "reads every CODE cell once"). The resolver must be written against spans.
- **CC-5 contract-break check.** It pairs old vs new schema copies and OpenAPI ops; share the op-key normaliser with
  the contract-op class.
- **CD-1 communities and CD-4 duplicate flows.** Coordinate only: duplicate flows (exact by fingerprint) will see
  vendored copies as duplicates. With SAME_AS present they should report one class, not N flows.
- **CF matrix and C0.\*:** none. SAME_AS is allocated by 0.5.2's own wave 0.

## 9. Open questions only James can answer

1. Replace SHARES_DEPENDENCY and the message half of SHARES_SCHEMA with SAME_AS stars (an edge_removal break:
   consumers matching those categories change), or keep both (additive, duplicate information)?
2. Should answers collapse classes by default (with `also_in`), or only behind a flag? Default-on changes every
   multi-repo answer's row count.
3. Do vendored copies (a HEURISTIC tier) collapse by default, or only FACT / DERIVED classes? The quokka stack copy
   and the standalone quokka_web are diverged copies at different revisions: one row, or two?
4. Is org-internal package routing (TODO.md:27) part of this bet, or its own item in 0.5.3?
5. SAME_AS in Engram: map to an existing EdgeKind (no contract change) or ask for a new one (v7)?

## 10. Spikes worth running before packets

- **S1.** Check the PROJECT label per ecosystem against the matching PACKAGE_DEP name: npm `name`, Go module path
  vs import path, Cargo `package.name`, pyproject `project.name`, Maven `groupId:artifactId`. Needs a fixture per
  ecosystem. It decides whether package ↔ project is FACT or needs normalisation.
- **S2.** Run the vendored-copy detector (IdentityIndex over the merged quokka set) and count Identical vs SameName
  vs diverged classes. Hand-label 50 pairs for precision before a HEURISTIC class collapses answers.
- **S3.** Measure the answer-row reduction on the quokka set and one other multi-repo set (Kina + lapse, if they
  share contracts) for blast / serves / flows / arch.
- **S4.** Merge parity: with SAME_AS present, merge_layouts must still equal the joint build byte for byte.

## 11. Risks

- **HEURISTIC over-merge.** Angular boilerplate (`AppComponent`, `app.routes`) matches across unrelated apps; the
  §2 suffix count includes quokka (Ionic) vs quokka_web. Collapsing on SameName alone would fuse different apps.
  Body-hash confirmation and a tier gate are required.
- **Divergence hidden by collapse.** One row with `also_in` can hide that the copies differ (the 113 vs 114 lines
  above). `diverged` must show in the row.
- **Representative stability.** Minimum-NodeId representatives change when repo identities change (LB.1: a checkout
  without a remote keys on its directory name). Answers must not expose the representative as an identity.
- **Carry-edge interaction.** SHARES_SCHEMA is a carry edge. If both SHARES_SCHEMA and SAME_AS exist for one message
  pair, blast radius double-walks unless the quotient view is used consistently.
- **Scope creep.** Package routing is language-specific extractor work and could double this bet. Keep it separate.

Sources:
https://popl21.sigplan.org/details/POPL-2021-research-papers/23/egg-Fast-and-Extensible-Equality-Saturation ·
https://egraphs-good.github.io/ · https://arxiv.org/pdf/2304.04332 (egglog: unifying Datalog and equality
saturation) · https://sourcegraph.github.io/scip-java/docs/manual-configuration.html (package name + version +
qualified symbol = cross-repo id) · https://sourcegraph.com/blog/cross-repository-code-navigation ·
https://github.com/sourcegraph/scip-clang/blob/main/docs/CrossRepo.md · Downey, Sethi & Tarjan, "Variations on the
common subexpression problem", JACM 1980 · Nelson & Oppen, "Fast decision procedures based on congruence closure",
JACM 1980.
