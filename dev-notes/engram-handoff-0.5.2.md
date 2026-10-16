# glia 0.5.2 has landed - engram read-side handoff

**From:** the glia session, 2026-10-16, written by the docs packet DZ.2 after glia's last code wave.
**To:** an engram session.
**Status:** no contract change. `GMAP_FORMAT_VERSION` stays 6, `engram-core` is untouched, and glia
0.5.2 edits no Engram file. Engram needs **no code change**. What changes: every export's bytes once
(cross-clone determinism), identity hints on ENDPOINT and `page:` facts, how `--since` pairs re-keys and
renames, an opt-in PPR side file, generated files leaving the export, SAME_AS edges as Cooccurs 0.3 in
place of SHARES_DEPENDENCY, and the keys of the facts whose qname moved (section 4). These are the 0.5.2
group DE, "Engram anchors" (Engram's 2026-10-02 answer reframed the fourth bet from PPR memory to
determinism and re-key pairing, `dev-notes/research-0.5.2/R2-engram.md`).

**This doc re-measures nothing.** Every number comes from the landed packet's commit body or report
(cited by commit), or from Engram's own determinism check at glia `3921a636` (recorded in glia's
`dev-notes/engram-ask-0.5.3.md` section 5). The Kina + glia replays (re-keys with X1 off and on, and
`flat-ppr` vs `flat-fs`) run once at the 0.5.2 release candidate after W14 (James, 2026-10-16); they
are Engram's measurement of all of this.

0.5.2 is glia's "four bets": 153 packets (`dev-notes/leap-052-packets.json` in glia), 151 landed in
waves W0-W13 since v0.5.1 (`84e2e71`), and W14 is the two docs packets. It is not tagged or pushed yet,
and stays unreleased while James tests it: everything here comes from the local `/home/ivy/Code/glia`
checkout at `12f58844`. Packet ids are glossed in words; the hash after an id is the glia commit that
landed it.

---

## 1. TL;DR

1. **Pin `=0.5.2`:** build the exporter from glia's `v0.5.2` tag (section 2). The excluded
   `glia-engram-export` crate now carries the release version (`engram-export/Cargo.toml:3` reads 0.5.1
   at HEAD and moves to 0.5.2 at the bump), so `glia-export-engram -V`, `<out>.meta.json`'s
   `glia_version` and its `exporter_version` all read 0.5.2.
2. **Every clone of one commit exports the same bytes** (DE.2, `f2d12c54`). The eval's single-clone
   determinism check and its one cached export series (`docs/eval-plan-2026-09.md:582-592`, "Organ
   verdict v3, first attempt: blocked by its determinism check", and `:622`, "every run reads one cached
   export series (`--export-cache`)") can go. The first export after the upgrade changes every gmap
   digest once (section 3.1).
3. **ENDPOINT and `page:` facts carry identity hints, and a re-keyed endpoint arrives as a move**
   (DE.3, `41b08e9f`): it used to be removed + added with a fresh FactId (section 3.3).
4. **A renamed symbol pairs by its name's words; a hint pair whose names share no word is refused**
   (DE.4, `8bbcc0c7`): 15 wrong cross-pairs on Kina's history are gone; a true rename that shares no
   word is removed + added (section 3.4). Engram's "renamed" figures need the release-candidate replay.
5. **An opt-in PPR candidate side file**, `<out>.ppr.jsonl` with `--ppr-candidates <K>` (DE.5,
   `4504f2d6`): data only, bound to the gmap by its digest (section 3.5).
6. **Generated files leave the export** (DE.7, `db774107`): nodes of a file with a generator header or a
   codegen name carry provenance `generated`, which the exporter drops; `.pbgrpc.dart` / `_pb.js` /
   `_grpc_pb.js` / `_pb.d.ts` become `generated_proto` (Engram's seed drop set already drops it).
   Stub-only gRPC clients are gone with their Causes edges (DE.8, `38b4f41e`).
7. **SAME_AS (glia edge category 37) is exported as Cooccurs 0.3**; SHARES_DEPENDENCY and the message
   half of SHARES_SCHEMA are never emitted (D0.1, DD.16; section 3.7).
8. **Keys that move** (section 4): fact keys follow glia qnames, and a handful of families re-key; the
   table says which `--since` pass carries each one.
9. **`.gitignore`:** add `*.engram-gmap.ppr.jsonl`; and `*.engram-gmap.meta.json`, which the 0.5.1
   handoff asked for and Engram's `.gitignore` still lacks (checked at Engram `cbf09bb`).

## 2. The pin

Engram links no glia crate: `engram-core` is the only thing the two repos share, and 0.5.2 did not
touch it. The exporter is glia's `engram-export` crate, built by glia's `scripts/check-engram-export.sh`.
The exact pin is the exporter built at the `v0.5.2` tag, `glia-engram-export =0.5.2`:

```
git -C /home/ivy/Code/glia describe --tags --exact-match      # expect v0.5.2
/home/ivy/Code/glia/engram-export/target/debug/glia-export-engram -V
#   glia-export-engram 0.5.2 (build 0.5.2+p<16 hex>)
```

A gmap already written names its glia in `<out>.meta.json` (`glia_version`, `exporter_version`,
`build_stamp`, `parser_stamp`, `gmap_digest`, `meta_version` 1); every run prints `[engram-export] glia
build=<BUILD_STAMP> exporter=<crate version> format_version=6 meta=<out>.meta.json`. Before the bump the
W13 close-out build reads `0.5.1+p0c104fe6f8df1704`.

**The check at HEAD.** The W13 close-out (`f468af11`) ran `check-engram-export.sh`:
`[engram-export] check: ok - build + 95 tests passed against engram-core GMAP_FORMAT_VERSION=6` (66 at
the 0.5.1 handoff). The exporter's `Cargo.lock` gained `glia-rules` (D0.2, `35cfec86`, 0.5.2's one new
crate, no external dependency) and one `glia-activation` edge (DE.5). PARSER_STAMP moved, so the first
export after the upgrade reparses from scratch; that alone changes no graph content.

## 3. What changes in the export

### 3.1 Cross-clone determinism (DE.2, `f2d12c54`)

glia's NodeId hashes the RepoId (the origin URL, the git directory name, or the directory name outside
git), so an exporter keyed by qname must never order or tie-break by it. DE.1 (`98650a9c`) made the
qname-level graph RepoId-free; DE.2 removed the exporter's four NodeId orderings:

| ordering | old | new |
|---|---|---|
| gmap edge order | glia's NodeId order (per-graph build order, then NodeId-sorted cross edges) | a stable sort by (from, EdgeKind, to, weight bits), duplicates kept |
| `identity_hint` ordinal of same-kind nodes sharing both rows in a file | (start row, NodeId) | (start row, end row, qname, NodeId) |
| a duplicate-key tie | the lower NodeId | the least kind id (a CLASS beats its COMPONENT twin) |
| an `@inheritdoc` base | the first matching heritage target in edge order | the least matching heritage qname |

Measured (DE.2's gate, `--no-persist`, git clone / `git archive` copies): Kina `a448fb3`, six copies
(two clones, a clone of a clone, a clone with no remote, two archives): 5 distinct gmap digests -> 1
(`ea67bc4d7ec03328`), gmap / files.json / meta.json identical; quokka-stack `a77d4cb` archive pair
`9012fe9078d8e656` / `a3fb97bf759d7a30` -> `14251ab9a3e49e80` both; glia `19b03712` archive pair ->
`7177b15915a65df2` both; a Kina `a448fb3~5 -> a448fb3` `--since` on two clones gave byte-identical
`.diff`s. Hint row ties: Kina 0, quokka-stack 9, glia 21 nodes; key kind ties: 0 on all three. Against
one copy's HEAD export, node keys and contents and the edge multiset are equal; only edge order and the
row-tie hints move (quokka 8, glia 10).

What Engram sees: every gmap digest changes once; the next `--since` run lists location_only changes on
the row-tie nodes and no edge churn (the diff compares edge sets). Engram's own check (engram-34,
2026-10-16, PASS at `3921a636`): clone pairs at different directory names and depths gave identical
`.engram-gmap`, `.files.json`, `.meta.json` and `.ppr.jsonl` on Kina, glia, quokka and hono, and 0 of 89
files differed over 30-commit `--since` chains. Its control (the 0.5.1 binary, also 0 differences)
used clones, which carry their origin URL and so one RepoId; DE.2's diverging copies were the
remote-less clone and the archives, whose RepoId comes from the directory name. Both results hold.

New stderr line after `v6 identity:` on every export: `[engram-export] v6 order: <e> edges sorted by
key; hint row ties by qname=<t>; key ties by kind=<k>` (Kina `12532 edges ... qname=0 ... kind=0`).
`ExportStats` gains `hint_row_ties` and `key_kind_ties` (Engram links neither).

### 3.2 DE.1's one-time Documents re-target (`98650a9c`)

An ambiguous backticked doc mention (Weak `ambiguous`, or Medium `qualified` whose `Class::method` tail
has several candidates) now binds the least (qname, kind id, graph index) candidate instead of the
lowest NodeId. On the first export after the upgrade such a Documents fact can re-target once (removed +
added edges in the `--since` diff); after that every clone exports the same edges. Measured on one copy,
glia graph DOCUMENTS edges moved: Kina 4, glia 159, quokka-stack 127 (edge counts unchanged). Facts that
history (CO_CHANGES, churn ATTN) and SCIP put on a file holding two MODULEs land on the MODULE with the
least qname.

### 3.3 Anchors for ENDPOINT and `page:` facts (DE.3, `41b08e9f`)

An ENDPOINT has no file of its own and a client-router `page:` ROUTE has no POSITION, so neither carried
an `identity_hint`, and a re-key of one arrived as removed + added. Now:
- a positionless ENDPOINT takes its least (file, line, col) ENDPOINT_HIT call site;
- a `page:` ROUTE takes the file and start row of its least-qname located HANDLED_BY target;
- the node joins its anchor file's (file token, own kind) group, ranked by (row, col, end row, qname);
  col is 0 for every positioned node, so positioned ordinals are unchanged. The hint keeps the
  `<anchor file token>:<9|5>:<ordinal>` shape (no contract bump); the span stays empty.

Measured (debug exporter, `--no-persist`): Kina `a448fb3` `5233/5535 hints ... anchored endpoint=103/103
page=52/57` (HEAD `5078/5535`); quokka-stack `a77d4cb` `3200/3269 hints ... anchored endpoint=95/95
page=20/39` (HEAD `3085/3269`); on both, no positioned node's hint differs from HEAD's.

What Engram sees: the first `--since` after the upgrade lists these facts as location_only
modifications (the hint went None -> Some). Afterwards an endpoint re-key whose verb holds and whose
path is alike (a prefix gained or lost, a leading `${…}` base dropped, `<unresolved>` resolved) is a
move (`prior_key` -> `key`), so Engram keeps the FactId. A later call inserted ahead in a file shifts
the endpoints after it (location_only), the way positioned siblings shift. ROUTE ordinals in a
component file shift when pages join its ROUTE group (location_only; measured 0 on Kina and
quokka-stack). The `v6 identity:` line ends with `; anchored endpoint=<a>/<A> page=<p>/<P>`.
`ExportStats` gains `positionless_endpoints`, `anchored_endpoints`, `positionless_pages`,
`anchored_pages`; `DiffStats` gains `by_endpoint`.

### 3.4 Rename pairing (DE.3 + DE.4, `8bbcc0c7`)

The `--since` diff (glia `engram-export/src/diff.rs`) now matches nodes in five passes, each over what
the earlier ones left:
1. keys;
2. ROUTEs and ENDPOINTs by path within (file token, hint kind, verb), equal paths first, then alike
   ones (the shorter path is a `/`-suffix of the longer, after dropping a leading `${…}` segment;
   `<unresolved>` is alike to any path); the owner suffix is not compared;
3. other Symbols by (file token, kind, name), when unique on both sides;
4. **new (DE.4):** within a hint slot (file token, kind), same parent key (the key minus its last `::`
   segment), names whose words fit (camel / acronym / `_` / `-` boundaries, lower-cased; Jaccard >= 1/3
   or containment), each the other's unique best;
5. a unique `identity_hint`, now refused for two ROUTEs / ENDPOINTs of another verb or unalike paths
   (since 0.5.1's CK.2) and, since DE.4, for two other facts whose names share no word (counted in
   `hint_refused`).

The pairing line: `[engram-export] v6 since pairing: route=<r> name=<n> hint=<h> hint_refused=<x>`
(0.5.1) -> `[engram-export] v6 since pairing: route=<r> endpoint=<e> name=<n> words=<w> hint=<h>
hint_refused=<x>`. `DiffStats` gains `by_endpoint` (DE.3) and `by_words` (DE.4).

Measured, Kina, 13 two-step `--since` runs over its history (DE.4's gate, HEAD exporter vs DE.4): 29
moves, all by hint -> 17 moves, all by words (`words=17 hint=0 hint_refused=14` summed; `endpoint=0` on
that engine). None of the 15 wrong cross-pairs, nor `CLAUDE::overview -> planning-wiki-...`, remains;
`AdminComponent::resolve -> resolveDispute` now pairs; every listed true rename is kept
(`PendingSMRWithUser`, `CreateSMRInput`, `ListCompletedForIFTI`, the three `ListPendingHits`,
`ClearedEntryIDsForUser`, `isLanding -> landing`, `CLAUDE::repo-graph`, `Latest`, `Insert`, `fileOpen`,
`externalReference -> lodgementReference`, `createAndFile`, `AnnualReportCounts`), plus one questionable
kept pair, `fx::StaticSource -> fx::SourceStatus` (`1aca01c`). A rename that shares no word is removed +
added: `SuspiciousMatter -> RegulatorReport` (Kina `07370cf`), LG.9's `place -> submit`.

Engram's replay question generator will see endpoint moves (DE.3), lose the questions built on the
cross-pairs, and gain resolve -> resolveDispute-type renames. Engram's "renamed" figure (the briefing's
4 / 9) predates Engram's X1; Engram's own answer puts it at 9/9 on Kina and 21/25 on glia since X1 and
re-scopes the re-key test to "re-keys with X1 off and on" at the release-candidate replay.

### 3.5 The PPR candidate side file (DE.5, `4504f2d6`)

Opt-in: `glia-export-engram ... --ppr-candidates <K>` (K in 1..=200) also writes `<out>.ppr.jsonl` (for
`<out>` = `x.engram-gmap`, `x.engram-gmap.ppr.jsonl`), after the diff and before `<out>.glia/`. Data
only: no glia code reads it, Engram needs no glia code to read it, and the gmap, `files.json`,
`meta.json`, `--since` diff and `<out>.glia/` bytes are the same with or without the flag.

- **Graph:** one node per gmap node, one edge per gmap edge whose two keys are both exported, each
  weighted `weight.unwrap_or(1.0)` (what Engram multiplies a hop by); the walk is undirected.
- **Rows:** for every Symbol and Proposition fact in gmap order, push PPR (Andersen-Chung-Lang,
  `glia_activation::algo::ppr::push_ppr`) with alpha 0.5 (glia's damping) and epsilon 0.001; the fact
  itself dropped, scores rounded to 6 decimals, a 0 score dropped, the top K by (score descending, key
  ascending); a fact with no neighbour gets `[]`.
- **Format:** JSONL, every object's keys sorted. Line 1 is the header
  `{"alpha":0.5,"epsilon":0.001,"gmap_digest":"<16 hex>","k":<K>,"ppr_side":1,"walk":"undirected","weights":"gmap edge weight, 1.0 when none"}`;
  `gmap_digest` is the gmap's `engram_core::content_digest`, so a reader drops a side file whose digest
  is not its gmap's. Each further line is `{"candidates":[["<key>",<score>],...],"key":"<key>"}`.
  `ppr_side` is the schema version, bumped when a field is renamed, removed or changes meaning.
- **Marker:** `[engram-export] ppr side: <facts> facts, k=<K>, <pushes> pushes, <ms> ms -> <path>` after
  the `glia build=` line.
- **Exit codes:** 8 = the side file could not be written (the gmap, its sidecars and the diff are
  already complete); 5 = a stale side file could not be removed (as for the diff). Every run without the
  flag removes a stale side file, which named another digest.
- **Measured:** Kina `a448fb3`, 5,535 facts = 5,535 gmap nodes, 163,432 pushes; side bytes identical
  over 3 runs; debug wall time 1.39x the plain export. Engram's determinism check found the side file
  identical across clones (section 3.1).

Engram's 2026-10-02 rule applies: a candidate source enters its pool as a weight-0 feature and stays only
if it beats ablation; no PPR default read path.

### 3.6 Generated code (DE.7, `db774107`; DE.8, `38b4f41e`)

- **Provenance.** One classifier (`glia_code_extractors::generated`): protoc / codegen file names, or a
  generator header in a file's first 40 comment lines (`code generated by`, `<auto-generated`,
  `@generated`, `generated` with `do not edit` ...). Nodes of a header-marked file, or of a `.g.dart` /
  `.freezed.dart` / `.gr.dart` / `.mocks.dart` / `.g.cs` / `.designer.cs` file, gain provenance
  `generated` (old: no ORIGIN), which the exporter's `DROP_PROVENANCE` (dependency, synthetic, generated)
  drops. Nodes of `.pbgrpc.dart`, `_pb.js`, `_grpc_pb.js` and `_pb.d.ts` files gain `generated_proto`
  (old: none), which the exporter keeps and Engram's `default_drop_provenance`
  (`crates/engram-live/src/gmap_sync.rs:20-31`) drops at seed time. Matching is ASCII-case-insensitive.
  A generated file under a test path stays `test_fixture`.
- **Measured:** quokka-stack: swaggo's `turps/docs/docs.go` (3 nodes) leaves the export (removed in the
  next `--since` diff); `quokka_chat.pbgrpc.dart`'s nodes become `generated_proto` (8 distinct, 9
  entries); generated_proto 183 -> 192, generated 0 -> 3 on the glia side. Kina, grpc-go,
  microservices-demo and glia unchanged.
- **The `[provenance]` stderr line** changes `generated=<n>` -> `generated=<n> (name=<a> banner=<b>
  qname=<c>)`; a parser of that line must read the group.
- **Stub-only gRPC clients (DE.8).** Generated stubs mint no GRPC_CLIENT: a `grpc_client:<Svc>
  @<owner>` that only a generated file minted leaves the export with its Causes (GRPC_CALLS) edges; a
  client a hand-written site also mints keeps its key and moves its span to that site (quokka-stack
  `grpc_client:QuokkaChatService @quokka_android` now at `lib/core/services/chat_service.dart:58`, was
  `lib/core/generated/quokka_chat.pbgrpc.dart:29`). microservices-demo: the Go services' GRPC_CALLS 36
  -> 13, the Python services' 18 -> 3.

### 3.7 SAME_AS and the removed categories (D0.1, DD.5-DD.7, DD.16)

- **SAME_AS = edge category 37** (D0.1, `9d180f01`, the one registry allocation of 0.5.2). The excluded
  exporter's `CATEGORY_MAP` maps it like the SHARES_* rows it replaces: `(SAME_AS, Cooccurs, 0.3)`
  (`engram-export/src/lib.rs`, pinned by `every_registered_category_is_mapped`). Q20: SAME_AS needs no
  Engram contract change.
- **The edges** (DD.5 `1b631092`): a star from each member to the class centre, the least (qname, kind,
  first graph index), so the centre is chosen by content and is RepoId-invariant at qname level.
  Rules: `package_name`, `manifest_name` (a package and its manifest-named project), `message_fqname`,
  `contract_op`, `contract_route`, `scip_symbol` (DD.7 `16020321`, where both repos are SCIP-indexed),
  and the HEURISTIC `body_hash` / `path_suffix` (DD.6 `fe20779b`, vendored copies, multi-repo only).
  A single-repo export can carry SAME_AS only for contract-op and workspace-package classes; not
  measured on an Engram export.
- **Removed (DD.16, `14c6ae40`):** SHARES_DEPENDENCY (31) is never emitted; SHARES_SCHEMA (16) is never
  emitted between MESSAGE_TYPEs (CLASS / INTERFACE / STRUCT SHARES_SCHEMA stay). A multi-repo export
  loses those Cooccurs edges and carries SAME_AS Cooccurs 0.3 stars instead (a star has n - 1 edges where
  the SHARES_DEPENDENCY clique had n(n-1)/2). Engram's single-repo eval never had either. The exporter
  keeps its SHARES_* map rows.

### 3.8 ORIGIN and CODE cells: what reaches the gmap

The exporter reads one ORIGIN key, `provenance` (`origin_provenance` in `engram-export/src/lib.rs`), and
only text CODE cells. So:
- **reaches the export:** provenance `external` on unpaired third-party WS_CLIENT / GRPC_CLIENT /
  GRAPHQL_OPERATION nodes (DF.10, `1f150479`; not in either drop set, so it is kept), `generated` and
  `generated_proto` (section 3.6);
- **does not reach the export:** PROJECT ORIGIN's new `label_source` (`manifest` / `dir` / `root` /
  `declared`, DD.4 `efa0ac66`; DD.4's report says Engram "carries ORIGIN as data", but only
  `provenance` is read), OpenAPI DOC_SECTION ORIGIN `bases` (DF.18, `78645340`) and `annotated_base` /
  `annotated_at` (DF.19, `61de0c5b`), the `"external":true` field on WS / gRPC / GraphQL ENDPOINT_HITs
  (DF.8, `f1de14d9`; ENDPOINT_HIT is not exported), and the Json CODE cell on `google_pubsub_*`
  INFRA_RESOURCEs (DF.14, `3b271bf4`; the Symbol keeps the text cell).

### 3.9 New and changed facts (graph content, no key moves)

- **Calls:** TS class-typed parameters and locals (DF.22a `4135c395`), another class's STATE_VAR
  (DF.17 `04f17964`), inherited methods on a typed receiver (DF.22b `5ebb3db5`); Go package-clause
  imports and typed handler bases (DF.15, DF.16); Dart plain and own-package imports (DF.4a, DF.4b);
  Swift / C++ locals, lambdas and `auto` (DF.3, DF.5); Elixir remote calls (DH.17a); Ruby constructed
  receivers (DH.13); libcurl clients (DH.21). Between existing Symbols; no hint moves.
- **Implements:** TS concrete overrides (DF.22c `8a4b6557`, method-level, rule `concrete_override`); Rust
  `impl Trait for Type` across files (DG.13a `2d99ca0e`; glia's own export gains them, `CrossGraphResolver`
  17 implementors); PHP `implements` (DH.14); Elixir `@behaviour` (DH.17b). The exporter maps IMPLEMENTS
  to Implements at a fixed 0.8.
- **Extends:** Ruby `class X < Base` (DH.13), PHP `extends` (DH.14).
- **Causes:** Python signal EVENT_FLOWS (DF.12, DF.13), Pub/Sub topic -> subscription QUEUE_FLOWS (DF.14),
  the Rust / Scala / Ruby / PHP / Dart / Elixir broker, event, GraphQL, WebSocket, CLI and cron rows
  (DG.1-DG.8, DH.1-DH.12), Clojure / Dart / Scala HANDLED_BY (DH.19, DH.15, DG.10). Removed: external WS /
  gRPC / GraphQL pairings (DF.9 `b4b8565f`), DE.8's stub calls, eventbus verb-fallback phantoms in broker
  files (DG.1, DG.2, DH.1, DH.2, DH.4).
- **Cooccurs:** TESTS from Scala test cases (DG.11 `c8357fbb`), ExUnit tests (DH.17b) and `[walk] tests`
  declared paths (DF.11 `a18f668b`, rule `declared_name`).
- **Evidence-only:** a TS `super.m()` CALLS edge's rule `inherited_method` -> `super_method` (DF.22c);
  PHP namespace imports `tail_unique` -> `namespace` (DF.2 `7fbc75f2`); Dart heritage `global_unique` ->
  `import_binding` (DF.4b `2692e523`; a cell_value change its spec did not declare). The gmap carries no EVIDENCE, so nothing moves in it; DF.2 also removes wrong
  PHP Imports facts (symfony-demo -2).
- **Propositions:** CJ.2's project-root docs ingestion now also reads README / `docs/` at the new Python
  roots (DE.9).

## 4. Keys and `identity_hint`: the re-keys, by packet

A fact's key is its glia qname; its `identity_hint` is `<file token>:<kind>:<ordinal>`, name-free. A
qname re-key is a key change; the `--since` diff carries the FactId across it when one of passes 2-5
(section 3.4) pairs the two, else it is removed + added (a fresh FactId). A kind change with the key
unchanged pairs by key (pass 1); passes 2-5 group by hint slot, which holds the kind, so a kind change
with a new key pairs nowhere. The pass named below is read from `diff.rs`'s rules, not measured on an
export; the release-candidate replay measures it.

| packet (commit) | old key -> new key | the diff pass that carries it |
|---|---|---|
| DE.9 requirements.txt / Pipfile roots (`e0370f74`) | HTTP / channel qnames under a nested python root gain ` @<dir>`: `GET /health` -> `GET /health @svc/email`; `grpc_client:ProductCatalogService` -> `... @src/recommendationservice`; a node two services shared unowned splits into one per owner (microservices-demo: 21 keys gone, 35 new) | ROUTE / ENDPOINT: pass 2 (owner not compared, equal path); other channel nodes: pass 3 (the nav name keeps no suffix) for the copy in the old file; the other owners' copies are added |
| DG.12 sbt sub-projects (`ff881e56`) | ROUTE / ENDPOINT / QUEUE_* / WS_* / GRAPHQL_* / GRPC_* / RPC keys under a member gain ` @<dir>` | as DE.9 |
| DF.7 Go constant group prefixes (`0fab4960`) | `GET /items` -> `GET /api/v1/items`, `GET /deep` -> `GET /m/api/v1/deep` (quokka-stack, grpc-go unchanged) | pass 2 (prefix gained: alike) |
| DF.18 OpenAPI server bases (`78645340`) | `contract::docs::openapi::GET:/users` -> `contract::docs::openapi::GET:/api/v1/users` (a document whose first server url is templated) | a Proposition: pass 4 (same parent, word containment) |
| DF.20 TS endpoint bases from config (`7d2f16a2`) | Kina `endpoint:GET:${…}/stats @frontend` -> `endpoint:GET:/admin/stats @frontend`, `${…}/trades` -> `/api/trades` (85 call sites); 9 `${…}` nodes merge into a real-path node that already existed | pass 2 (the `${…}` segment dropped, then alike); a merged node's base has no free target and is removed |
| DG.10 Akka / Pekko path matchers (`774de9b4`) | `ANY /users` -> `ANY /api/users`; `ANY /ws` -> `ANY /ws/chat`; ROUTEs read from a non-directive `path(` or an unreadable matcher are removed | pass 2 for a prefix gained (`/api/users`); a path grown at its end (`/ws/chat` does not end in `/ws`) is unalike: removed + added |
| DH.20 Swift Vapor routes (`121041d3`) | `GET /users/id` -> `GET /users/:id`; `PATCH /todos/todoID/patched` -> `PATCH /todos/:todoID`; phantom `GET /id`-style ROUTEs removed | unalike paths: removed + added |
| DH.17b Elixir behaviours (`8f72df22`) | a pure behaviour module is INTERFACE instead of PACKAGE (same key, new kind and hint slot); new FUNCTION keys `<module>::test <name>` | the key is unchanged, so pass 1 pairs it (a modified Symbol whose kind changed) |
| DH.8 Dart GraphQL operations (`edfba300`) | `graphql_op:client.query` / `client.mutate` / `client.subscribe` -> `graphql_op:<OpName>`; new `graphql_resolver:<Root>` / `<field>` keys | passes 3-5 need an equal name or a shared word; unless the operation name shares one with `client.query` (`GetUser` does not), removed + added |
| DH.1 Ruby brokers (`41f4c589`) | `queue_producer:unresolved:sns` -> `queue_producer:<topic>` in Ruby SNS files; new `queue_producer:<t>` / `queue_consumer:<t>` | queue keys mostly carry no file of their own (no hint): removed + added |
| DH.2 PHP brokers (`214105be`) | `queue_producer:unresolved:rabbitmq` -> `queue_producer:<routing key>`; `queue_consumer:unresolved:kafka` -> `queue_consumer:<topic>`; new Laravel `queue_*:<JobClass>` | as DH.1 |
| DH.3 Dart brokers (`4d96781a`) | `queue_producer:unresolved:kafka` -> `queue_producer:<topic>` beside kafka_dart `sendMessage` | as DH.1 |
| DH.4 Elixir brokers (`57ae47f1`) | `queue_producer:unresolved:oban` -> `queue_producer:<Worker>` for the pipe form | as DH.1 |

**New keys only, nothing re-keyed** (the DH packets declared `node_id` for nodes they add): DH.5
`config:env:<NAME>` (Dart, Elixir, Laravel env reads, `d7230be3`); DH.6 `config:flag:<key>`,
`config:secret:aws_sm/<ref>` (`c4bb7da0`); DH.7 `event_emit:<name>` / `event_handle:<name>` (Rails,
Laravel, Dart event_bus, Phoenix.PubSub, `6db8612a`); DH.9 `ws:<path>`, `ws_client:<path>` (`df81a9c1`);
DH.10 `cli:<name>` and CLI invocations, a gemspec manifest MODULE (`9c895029`); DH.11
`data_entity:sql:<table>` (sqflite, Ecto migrations, `13902a97`); DH.12 elixir-grpc `grpc_client:<Service>`
(`ab306320`); DH.14 PHP interface METHODs `<Interface>::<method>` (`a08fc669`); DH.18 Ecto
`data_entity:sql:<Model>` (`411c063e`); DH.21 libcurl `endpoint:<METHOD>:<path>` (`837297c7`); DG.11
Scala test-case FUNCTIONs `<suite>::<description>`; DE.9's and DG.12's `project:<dir>` PROJECTs. Edges
only: DH.13 (`e8223e1f`), DH.15 (`304037c1`), DH.17a (`77e8d9fa`), DH.19 (`db240e69`).

**Removed keys with no successor:** DE.8's stub-only `grpc_client:<Svc> @<owner>`; DE.7's generated
files' facts; DG.1 / DG.2 / DH.1 / DH.2 / DH.4's eventbus verb-fallback `event_emit:<verb>` /
`event_handle:<verb>` phantoms in broker files, `event_emit:ProducerRecord` and redis4cats
`unresolved:redispubsub` tags (`4dac65bb`, `089e03d8`); DG.10's and DH.20's phantom ROUTEs.

## 5. engram-export changes in 0.5.2

| packet (commit) | change | Engram action |
|---|---|---|
| D0.1 SAME_AS (`9d180f01`) | `CATEGORY_MAP` row `(SAME_AS, Cooccurs, 0.3)` and its assert | none (section 3.7) |
| D0.2 the glia-rules crate (`35cfec86`) | `engram-export/Cargo.lock` gains `glia-rules` | none |
| DE.2 cross-clone determinism (`f2d12c54`) | edge order, row-tie hints, key and `@inheritdoc` ties by content; `ExportStats.hint_row_ties` / `key_kind_ties`; `v6 order:` line; new `tests/determinism_v6.rs` (two clones, identical bytes and diffs) | drop the single-clone / `--export-cache` workarounds (section 1) |
| DE.3 anchors and endpoint pairing (`41b08e9f`) | ENDPOINT / `page:` hints; pass 2 covers ENDPOINTs; `DiffStats.by_endpoint`; four `ExportStats` fields; the `endpoint=` field and the `; anchored ...` tail | none: Engram's apply already re-keys by `prior_key` |
| DE.4 words pass (`8bbcc0c7`) | pass 4 (renames by words), the guarded hint pass; `DiffStats.by_words`; the `words=` field | re-measure renames (section 3.4) |
| DE.5 PPR side file (`4504f2d6`) | `--ppr-candidates <K>`, `ppr_side` module, `<out>.ppr.jsonl`, exit 8; `Cargo.toml` gains `glia-activation` | optional reader; `.gitignore` line |
| DE.6 keep pins (`cc67f519`) | `engram-export/tests/keep_v6.rs` is the one place pinning the three kept behaviours: `leading_doc` reaching `Symbol.doc` in 8 languages (Python, Go, TypeScript, Rust, Java, Dart, Solidity `@notice`, C# `<summary>`), the 500-byte `cap_prose`, and `--since` accuracy through CK.2's `diff.rs` tests and `since_v6.rs`'s `B_COUNTS` | none |
| DE.7 generated provenance (`db774107`) | `generated` nodes drop through `DROP_PROVENANCE`; the `[provenance]` group | section 3.6 |
| version | the crate version moved to the release (0.4.13 at the 0.5.1 handoff, 0.5.1 now, 0.5.2 at the bump), so `exporter=` in the `glia build=` line and `exporter_version` in meta.json read the release | none |

Every other exporter marker is unchanged in shape.

On glia's pyo3 / CLI side, DE.11 (`a7f2c70a`) removed the 0.5.0 aliases `PyGraph.patterns_experimental`,
`glia_py.patterns_vs_rev_experimental` (AttributeError) and `glia patterns --experimental` (exit 2);
Engram calls none of them.

## 6. Not verified, and open

- **Not run here:** an export, a seed or a diff apply on the 0.5.2 exporter, and the first `--since`
  across the upgrade (0.5.1 gmap -> 0.5.2 gmap). Expect, from sections 3-4: every digest changes; the
  row-tie hints and every newly anchored ENDPOINT / page are location_only; DE.1's Documents re-targets;
  DE.7's and DE.8's removed facts; the section 4 re-keys as moves or removed + added. FORMAT_VERSION
  stayed 3, and the exporter reads `<prior>.glia/` with `persist::load_layout`, which checks the format,
  not the build stamp, so the 0.5.1 history layout should load (unlike the 0.5.0 -> 0.5.1 upgrade's format
  2) and no "prior graph unavailable" warning is expected; read from the code, not run.
- **Engram to confirm, DE.4's precision-over-recall trade:** a true rename that shares no name word
  (`SuspiciousMatter -> RegulatorReport`) is now removed + added, a fresh FactId, where the hint pass
  used to pair it; in exchange no fact is handed a neighbour's FactId. One questionable kept pair
  remains on Kina (`fx::StaticSource -> fx::SourceStatus`).
- **Engram to confirm, DE.5's format:** the header binding (`gmap_digest` = `content_digest` of the
  gmap), the row shape `{"candidates":[[key,score],...],"key":..}`, K <= 200, alpha 0.5 / epsilon 0.001,
  the undirected walk over the exported edges' weights, and whether Engram wants a different weight or
  direction for its feature.
- **Not measured:** SAME_AS on a single-repo Engram export; CK.2's 0.5.1 trade-off (a root route
  re-registered under a mount prefix, or a route whose method changed, is removed + added); glia's own
  repo under DE.7 / DE.8 beyond "unchanged".
- **Engram's replays at the release candidate** (James, 2026-10-16): Kina + glia, re-keys with X1 off
  and on, `flat-ppr` vs `flat-fs`; the 8 public streams only if those pass.
- **Closed:** cross-clone determinism (DE.2, and Engram's PASS at `3921a636`); endpoint hints (DE.3);
  the hint pass's cross-pairs (DE.4). Q20-25 are answered by Engram (no v7, `cap_prose` stays 500 bytes,
  side files as data only, no PPR default read path).
