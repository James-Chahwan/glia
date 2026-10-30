# glia 0.5.4 - engram read-side handoff (0.5.2 -> 0.5.4)

**From:** the glia session, 2026-10-31, written by the handoff packet FZ.2 after glia's last code wave of
0.5.4 (W26, HEAD `9bd4d3f0`).
**To:** an engram session. Act on Engram only.
**Span (ruling HANDOFF-SPAN, James 2026-10-24): 0.5.2 -> 0.5.4.** Engram never applied
`dev-notes/engram-handoff-0.5.3.md`; this doc carries it forward (section 3) and adds 0.5.4's changes.
**Status:** `GMAP_FORMAT_VERSION` stays 6, `engram-core` is untouched, and no 0.5.4 packet edits an Engram
file. What reaches Engram: three data contracts (vectors, the vocabulary side file, corrections), one
exporter stderr line, the registries, and graph content (phantom nodes removed, some re-keyed).

0.5.4 shipped tracks T0, T1, T2, T3, T5, TS, T10, T11, TH and TZ: 121 landed packets in waves W0-W26 of
`dev-notes/leap-054-packets.json` since `v0.5.3` (`929d99ee`). Not shipped (ruling OPTION-3, James
2026-10-30): T4, T6 (the goal layer: no GOAL_UNIT / TRACES / UNIT is ever emitted), T7, T8, T9. The ask
James sent Engram is `dev-notes/engram-ask-0.5.4.md` (its section 6 is "0.5.4 as built"); this handoff
restates it with the pin and changes nothing in it.

Sources, in order of authority: the exporter source and the code at glia HEAD `9bd4d3f0`; each 0.5.4
packet's landed result (its `breaking` text) and commit body; the routed paragraphs in
`dev-notes/leap-054-corrections.json` (FZ.2's entry). Engram was read read-only at HEAD `e6ae2bf`
(5 dirty paths). Packet ids are glossed in words; the hash after an id is the glia commit that landed it.

---

## 1. TL;DR

1. **Pin `=0.5.4`**: build the exporter at the `v0.5.4` tag once James tags it (section 2). Rebuild every
   cache and replay chain once: the stamps moved (F0.6).
2. **`cell ls --check --rekey` no longer rewrites `engram:` rows** (FB.11 `e6999de7`) unless `--writer
   engram` is passed. Engram's corrections writer leaves a gone row with a `v1|` hint to glia's `--rekey`
   (`crates/engram-live/src/corrections.rs:920-925`): pass `--writer engram`, or re-anchor those rows
   itself (section 4.3). **The one Engram code change 0.5.4 asks for.**
3. **The vectors contract is fixed** (FB.2 `c7af7f02`, FB.3 `94106c1f`, FB.14 `a4975faf`): rows keyed by
   `(qname, kind)`, optional `text_hash` and `device`; one model per file; no layout holds a VECTOR cell
   (section 4.1). glia's own vectors use Engram's model, `Xenova/bge-small-en-v1.5`, 384 dims.
4. **`--vocab`** (FB.13d `4dad2709`): an opt-in `<out>.vocab.jsonl`, exit 10 (section 4.2).
5. **One new stderr line under `--since`** (FD.34 `68a91f76`): `[engram-export] v6 since refusals:
   hint_refused=<n> (verb_mismatch=<a> path_unalike=<b> shape_mismatch=<c> no_words=<d> words_unfit=<e>)`.
   The `v6 since pairing:` line is unchanged, so `crates/engram-live/examples/replay.rs`' sums stand.
6. **Graph content moves** (section 5): phantom nodes read from comments, literals and look-alike text
   are removed in every language, and four families re-key. Facts keyed by a removed node's identity
   hint orphan; no exporter code change.
7. **The 0.5.3 break stands**: `--ppr-candidates` is gone (section 3).

## 2. The pin

Engram links no glia crate; the exporter is glia's excluded `engram-export` crate, built and tested by
glia's `scripts/check-engram-export.sh`. The pin is the exporter built at the `v0.5.4` tag,
`glia-engram-export =0.5.4`:

```
git -C /home/ivy/Code/glia describe --tags --exact-match      # expect v0.5.4
/home/ivy/Code/glia/engram-export/target/debug/glia-export-engram -V
#   glia-export-engram 0.5.4 (build 0.5.4+p<stamp>)
```

**Not tagged yet.** At `9bd4d3f0` the workspace and `engram-export/Cargo.toml:3` read `0.5.3`; the bump is
the release step after these docs. `engram-export/Cargo.lock` gained 4 entries (F0.2 `50af6405`). PARSER_STAMP / BUILD_STAMP moved at every packet that edited a hashed source, and once
more by design (F0.6 `fed4d59b`, ruling STAMP-LOCK: one commit gives one stamp in every checkout and
packaging), so `<out>.meta.json`'s `build_stamp` / `parser_stamp` move and the first export reparses.

## 3. The 0.5.3 handoff, carried forward

`dev-notes/engram-handoff-0.5.3.md` stands whole: **`glia-export-engram --ppr-candidates` is gone** (EG.4,
`19747e61`): clap exits 2, `<out>.ppr.jsonl` is never written and a stale one is deleted before the gmap
write, exit 8 is retired (hints stays 9), the exporter no longer depends on glia-activation. Its section 3
lists the 0.5.2 handoff text that removal supersedes (TL;DR item 5, section 3.5, section 5's DE.5 row, the
`.ppr.jsonl` `.gitignore` advice). Everything else in `dev-notes/engram-handoff-0.5.2.md` stands. 0.5.3
changed no graph content; 0.5.4 does (section 5).

## 4. The data contracts

### 4.1 Vectors (`.glia/vectors.jsonl`)

| what | 0.5.2 / 0.5.3 | 0.5.4 |
|---|---|---|
| row | `{"qname","kind"?,"hint"?,"model","dims","b64"}` | `+ "text_hash"?` (`t<digits>:<16 lowercase hex>`, the recipe and a hash of the text embedded; FB.2) `+ "device"?` (`cpu` \| `webgpu` \| `cuda`, absent = cpu, never decides freshness; FB.14) |
| key | qname | **`(qname, kind)`**: a same-qname twin (CLASS / SERVICE, METHOD / ATTRIBUTE) keeps two rows; a kind-less removal matches only a kind-less row |
| payload | `dims` little-endian f32, base64 | unchanged: finite and unit-norm, checked |
| models | | one model per file; a reader skips rows of another model |
| unknown fields | a 0.5.3 glia rejects a row carrying `text_hash` (its struct denies unknown fields) | accepted |
| build | VECTOR cells applied into the layout | **query-time** (FB.3): no layout or gmap carries a VECTOR cell; writing the file never stales a layout or wakes a watch; a VECTOR cell write (`glia cell set`) changes the sidecar only (`write_through` `sidecar_only`) |
| audit | | `glia cell ls --check` prints `issue <kind> .glia/vectors.jsonl:<line> qname=<q> ..` and **exits 1** on an audit issue |

**Writer-agnostic** (FB.5a `9bd0430d`): `glia embed` keeps "foreign" rows, rows of its model with no
`text_hash`, untouched unless a glia row now holds their `(qname, kind)`; it writes under
`.glia/cells.lock` (the lock the cell API has taken since 0.5.0), so an Engram writer goes through
`glia cell set` or takes the same lock.

**glia's own vectors use Engram's model** (`glia_embed::MODEL_BGE_SMALL` = `Xenova/bge-small-en-v1.5`,
`DIMS_BGE_SMALL` = 384, fp32), so Engram can read them. The recipe: t1 is Engram's text exactly (name,
qname, leading doc capped at 500 chars, up to 10 imported libraries, never code); t2 appends the node's
code last, and **t2 is the default** (FB.7a `77141f1a`, picked by FB.6's gate). Reading glia's default
vectors is therefore a body change under Engram's rule; `glia embed --recipe t1` writes Engram's text.
fastembed 4.9.1 -> 6.0.0 / ort rc.9 -> rc.13 (FB.14) moves CPU vectors at most ~3e-7 (min cosine
0.99999988): treat 0.5.3 and 0.5.4 vectors as equal, or re-embed. An existing t1 sidecar re-embeds in full
on the next `glia embed`.

### 4.2 The vocabulary side file (`--vocab`, FB.13d)

`glia-export-engram --vocab` (off by default) writes `<out>.vocab.jsonl` after the hints side file; the
format is `dev-notes/engram-ask-0.5.4.md` section 6: a header `{"files","gmap_digest","tokens","vocab_side":1}`
(drop a file whose `gmap_digest` is not its gmap's), path rows `{"f","path"}` for tracked text files the
export's file table does not hold (ids from the table's largest + 1), then token rows `{"files":[..],"n","t"}`
or `{"common":true,"n","t"}` past 16 files, sorted by token. The capture is the work tree's checked-out
commit (`HEAD`); on a dirty tree the marker `[engram-export] vocab side: <tokens> tokens, <extra> extra
files, <bytes> bytes, <ms> ms dirty=<n> -> <path>` counts the modified tracked paths. A `--since` step writes
a full file (Engram's estimate: about 2.7 MB per step on glia). **Every run removes a stale
`<out>.vocab.jsonl` before writing the gmap** (exit 5 if it cannot), so a run without `--vocab` leaves none.
**Exit 10**: `--vocab` outside a git work tree or with no commit (`error: --vocab needs a git work tree`,
nothing written), or a vocabulary that cannot be captured or written (the gmap, sidecars, diff and hints are
already complete). The repo-side snapshot the same capture writes is `.glia/vocab-snapshot/{files.jsonl,
vocab.jsonl, meta.json}` (`glia vocab sync`, FB.13a / FB.13b; `meta.json` written last, `data_hash` =
xxh64 over files.jsonl then vocab.jsonl); an existing `.glia/.gitignore` needs `vocab-snapshot/` by hand.

### 4.3 Corrections (`.glia/cells.jsonl`)

- **The lifetime rule stands** (agreed 2026-10-16): the writer owns its rows; glia keeps, reports and
  attaches nothing to an orphan; ids are content hashes. `glia cell ls --check --rekey` (FB.11) now leaves a
  rekeyed row whose id has a `<ns>:` prefix (`[a-z][a-z0-9_-]*`, so `engram:`) unless `--writer <ns>`
  names it; `--json`'s `rekey` gains `skipped_foreign` when > 0. Engram re-anchors its own rows, or passes
  `--writer engram` (TL;DR item 2).
- **CONV entries** (FB.12a `abeef44d`): a CONV entry's `kind` must be absent or `"convention"`; any other
  `kind` is refused by `glia cell set`, the cells.jsonl stage and pyo3. Engram's convention rows
  (`corrections.rs:617-620`: no `kind`, text `convention: ..`) stay valid as notes. A row
  `{"source":"api","id":"engram:<hash>","kind":"convention","scope":"<path>","text":".."}` (a non-empty
  `scope` required) becomes a scope-level convention: pack and review then show it (FB.12b `d88bce65`, tier
  `heuristic`), and pack's ladder shows any DECISION / CONV entry as a `note[api/<id>, heuristic]: ..` line
  (FB.11).
- **DECISION cells on sub-scoped decisions** (FD.65 `bf325da4`): an overlay `[[decision]]` with no `anchor`
  inside a project now anchors on the deepest enclosing PROJECT, else the least-qname MODULE (was dropped):
  new DECISION facts on existing nodes.
- **The hint rules are unchanged**: `<out>.hints.jsonl`, `glia cell set --hint`, LB.6's v1 form.

### 4.4 The registries and the new cells in the gmap

| id | what the exporter does |
|---|---|
| node `GOAL_UNIT` 50 (F0.1 `a27aa00e`) | registered, never emitted in 0.5.4; one would export as a Symbol under the node-kind rule |
| edge `TRACES` 38 | `CATEGORY_MAP` row `(TRACES, Cooccurs, None)`: no conductance opinion (a test pins it); never emitted in 0.5.4 |
| cell `UNIT` 26 | never emitted |
| cell `IMPORT_BINDINGS` 27 (FA.21 `bff59e64`) | on every code MODULE with an external import: JSON rows `{"spec","name","alias","line"}` (Go specs `::`-joined, `line` 0-based), sorted, at most 256 then `{"capped":<n>}`, MODULE only. **The exporter reads IMPORTS only and ignores it**: no export byte moves for it |

FA.22 / FA.23 (`31385a5d`, `3df0ee91`): multi-repo exports gain cross-repo IMPORTS (rule `org_import`) and
CALLS / USES (rules `org_call` / `org_ref`) edges, exported under `CATEGORY_MAP` like any other; a
single-repo export is unaffected. FH.17 (`f3829dba`): an ENDPOINT_HIT may end `"test_site":true`; the exporter
passes it through and the hint side is unaffected. FD.22 (`e43ea02a`): pick_primary, cochange, overlay bind,
declared and cell targets break ties by (qname, kind, graph index), never NodeId, so every clone agrees on a
tie (Engram's DE.1 / DE.2 expectation).

## 5. Graph content: what Engram's facts see

No exporter change for any of these; facts keyed by a removed node's identity hint orphan, and a re-keyed
node is a new fact. Every packet whose landed break is `node_id` or `qname_shape`:

| packet (commit) | node ids gone or new | qnames that change |
|---|---|---|
| FD.4a cron / CLI phantoms (`784193cf`) | CRON_JOB from k8s text outside yaml / json and non-schedule Celery values; CLI_INVOCATION from Rust / Python literals | none |
| FD.4b data entities (`056f6843`) | in .rs / .py, DATA_ENTITY from a SQL / Cypher literal not passed to a call, nested literals, comments, migration needles in literals | none |
| FD.4c Rust routes (`9888e2b8`) | ROUTEs read from literals or comments | none |
| FD.4d WebSocket needles (`53cc6574`) | `ws_client:ws` from a bare `io(` in a file binding its own `io`; non-Ratchet `->route(` handlers | `ws:/ws/$x` -> `ws:/ws/{x}` (Ratchet double-quoted paths) |
| FD.5a guard: Ruby, PHP, Dart, Elixir (`bb9e08d7`) | nodes read from comments and non-interpolated strings | none |
| FD.5b guard: compiled and JS-family (`76c74d49`) | the same for Go, TS / JS, Java, Kotlin, Scala, C#, Swift, C / C++; `clap::Command::new` mints nothing | none |
| FD.31 manifests (`0f599ac3`) | **new**: `package:maven:<g>:<a>`, `package:pub:<n>`, `package:hex:<n>`, `package:nuget:<n>` and synthetic MODULEs `<dir>::pom.xml` / `build.gradle(.kts)` / `<Name>.csproj`, with SAME_AS `package_name` / `manifest_name` classes; nothing removed | none |
| FD.76 yaml env (`834bee60`) | CONFIG_KEY `config:env:<k>` defined only by a pubspec / GitHub Actions / GitLab CI `environment:` block | none |
| FD.77 TOML array tables (`6542bb7a`) | `package:cargo:name` / `path` / `harness` .., `package:pypi:name` / `url` / `priority` | none |
| FD.78 directory symlinks (`43cb576d`) | every node under an in-repo directory link, a second link to an outside directory, a link cycle | files under a link are read once, at the target's path |

Edges only (no node moves): FD.6 (`9298185e`), FD.73 (`98e0dfd8`), FD.74 (`e00d1263`), FD.75 (`e7ddaee7`),
FD.79 (`d22aeaad`: doclink DOCUMENTS from lone words, registry names, placeholders and out-of-tree test
candidates; glia's own tree 830 -> 344; DOCUMENTS exports as `Documents`, so `documents_edges` drops),
FD.80 (`836d5170`: gRPC HANDLED_BY moves from bootstraps to handlers, rule `register_impl`), FD.81
(`ebf41c7b`: Go implicit IMPLEMENTS across go.mod modules), FH.14b, FH.15. Added: FH.4a / FH.4b / FH.17
(endpoints from a call's shape, inferred HTTP wrappers), FH.5, FH.16, FH.19, FH.20 (TS imports, Nest
handlers, type aliases as INTERFACE nodes), FH.23 (`abi_partial` EVM pairs), FH.3b (`c7236c39`: NestJS
swagger op DOC_SECTIONs under a symbolic prefix re-key to the folded path), FH.6 (`312d1e00`: Vue POSITION
rows move to real lines; `.svelte` / `.astro` nodes appear, so an export of a repo with SFCs moves). The
per-language table is section 4 of `dev-notes/repo-graph-handoff-0.5.4.md`.

## 6. Needs no change (checked)

- `GMAP_FORMAT_VERSION` 6, `<out>.files.json`, `<out>.meta.json` (beyond its stamps and version),
  `<out>.hints.jsonl`, `<out>.glia/`, the `--since` diff and the `v6 since pairing:` line.
- FD.34b (`b95a739a`): `body_hash`'s ASCII fast path is bit-identical (a 10,000-case seeded test against the old function).
- FD.7a / FD.7c (the one `Tier` type): no Engram or exporter code names the old items (grep).
- The rest of 0.5.4 (compact answers, health, contract-breaks names, embeddings in answers, the CLI wheel,
  the skills) is answer surface Engram does not read.

## 7. Re-test

At glia, the close-out's gate:

```
bash scripts/check-engram-export.sh
[engram-export] check: ok - build + 101 tests passed against engram-core GMAP_FORMAT_VERSION=6
```

0.5.3 recorded 95; the 101 is the line in W23's close-out commit `a4d089f0` (FB.13d's `vocab_side_v6` tests
among them), and W26's close-out (`9c29a42a`) records `engram-export ok`.

In Engram, after building the exporter at the tag:
- export one repo you replay; expect the meta stamps to move and the gmap digest to move where section 5
  touches that repo (a repo with no phantom in those families keeps its facts);
- `--since` over a short chain: the new `v6 since refusals:` line, an unchanged `since pairing:` line;
- `--vocab` on one step: `<out>.vocab.jsonl` whose header `gmap_digest` equals `meta.json`'s; a step without
  `--vocab` leaves none;
- the corrections writer's `glia cell ls --check --rekey` with `--writer engram` rewrites a moved `engram:` row.

## 8. Done vs pending

- **Done, glia:** W0-W26, each close-out green (`cargo test --workspace` 7050 passing at W26).
- **Pending, James:** the 0.5.3 -> 0.5.4 bump, the `v0.5.4` tag; whether and when to send the ask
  (`dev-notes/engram-ask-0.5.4.md`, sent 2026-10-16 as a briefing; its section 6 is the as-built record).
- **Pending, Engram:** the pin (section 2); `--writer engram` or self-re-anchoring (section 4.3); accept
  `text_hash` / `device` and the `(qname, kind)` key in any vectors reader (section 4.1); optionally read
  `--vocab` and glia's vectors under Engram's own eval rule (a new source enters at weight 0 and stays only
  if it beats its ablation).
- **Followups the landed packets named for Engram:** F0.1 (TRACES -> `Cooccurs`, no weight), F0.2 / F0.6
  (the lock, the stamps), FA.21 (IMPORT_BINDINGS ignored), FA.22 (cross edges), FB.2, FB.3, FB.14
  (vectors), FB.11, FB.12a, FB.12b (corrections), FB.13a / FB.13d (vocab), FD.22 (ties), FD.34 (the
  refusals line), FD.65, FH.17 and every graph-content packet in section 5 (FD.4a-FD.5b, FD.6, FD.31,
  FD.73-FD.81, FH.3b, FH.6). No other landed break names Engram.

## Release-docs addendum (FZ.1, 1ac10500)

- README.md is install / quickstart / CLI only. The long prose moved to docs/overview.md, the release history to docs/history.md and the license rationale to docs/license.md. A link into an old README section now points at one of those files.
- PARSER_STAMP moves once more: FZ.1 gave the 16 reserved slot files a `//!` line in hashed roots, so every parse cache re-parses once after the upgrade. No graph content changes.
