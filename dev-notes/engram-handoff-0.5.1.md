# glia 0.5.1 has landed - engram read-side handoff

**From:** the glia session (the 0.5.1 release step; it has no packet id), 2026-10-01.
**To:** an engram session.
**Status:** no contract change. `GMAP_FORMAT_VERSION` stays 6, and glia 0.5.1 touches no Engram file.
Engram needs **no code change**. What changes is the content of every export, and the first `--since`
run after the upgrade. Every number below was measured on 2026-10-01 with two exporter builds run side by
side on the same trees: glia v0.5.0 (`2170ff8`, built in a scratch directory against this repo's
engram-core) and glia HEAD (`491c2ad`, the W10 close-out build of
`engram-export/target/debug/glia-export-engram`). The trees were `git archive` copies of quokka-stack
`a77d4cb` and Kina `a448fb3`, exported with `--no-persist`, so nothing was written into either repo.
glia 0.5.1 is not tagged or pushed yet: it all comes from the local `/home/ivy/Code/glia` checkout.

0.5.1 is glia's "catch-up leap": 155 packets (`dev-notes/leap-051-packets.json` in glia), landed in waves
W0-W11 since v0.5.0. Packet ids below are glossed in words; the hash after each is the glia commit that
landed it.

---

## 1. TL;DR

| | quokka-stack 0.5.0 -> 0.5.1 | Kina 0.5.0 -> 0.5.1 |
|---|---|---|
| nodes / edges / files | 2,784 / 4,977 / 246 -> 2,934 / 5,770 / 247 | 4,084 / 9,245 / 530 -> 4,090 / 9,613 / 530 |
| Symbols / Propositions | 2,506 / 278 -> 2,660 / 274 | 4,053 / 31 -> 4,073 / 17 |
| Calls | 1,896 -> 2,349 | 2,790 -> 3,176 |
| Implements | 28 -> 24 | 305 -> 208 |
| Causes | 253 -> 266 | 1,083 -> 1,140 |
| Contains | 2,087 -> 2,217 | 2,991 -> 3,010 |
| Documents | 169 -> 359 | 8 -> 11 (4 doc-linker + 7 NatSpec at 0.5.1) |
| Imports / Cooccurs | 521 / 23 -> 523 / 32 | 1,916 / 149 -> unchanged |
| identity hints | 2,642 -> 2,791 | 3,617 -> 3,623 |
| provenance changes | `inferred:wrapper` 0 -> 12, `external` 0 -> 1, `generated_proto` 166 -> 183, `documentation` 137 -> 133 | `test_fixture` 404 -> 454, `documentation` 24 -> 10 |

The first `--since` run from a 0.5.0 gmap to a 0.5.1 one (the path a seeded store takes):

```
quokka-stack
[engram-export] v6 identity: prior graph unavailable at .../q050.engram-gmap.glia (old format v2 (this build reads v3)) - moves not detected this run
[engram-export] v6 since: base=cde54e8831589657 target=0c99f6fbdfd3333e added=154 removed=4 modified=393 (moved=58 location_only=333) edges +737/-22; parse cache off
Kina
[engram-export] v6 identity: prior graph unavailable at .../k050.engram-gmap.glia (old format v2 (this build reads v3)) - moves not detected this run
[engram-export] v6 since: base=863302f752a9be7d target=66d480bd01f52070 added=20 removed=14 modified=188 (moved=87 location_only=48) edges +420/-122; parse cache off
```

What Engram should know:
1. **Pin:** build the exporter from glia's `v0.5.1` tag (section 2). engram-export's own version is
   0.4.13 and did not move, so `glia-export-engram -V` cannot tell a 0.5.0 build from a 0.5.1 one.
2. **Upgrade a seeded store with one `--since` run** (section 3). It prints the "prior graph
   unavailable" warning once, because the history layout the 0.5.0 exporter wrote is glia `.gmap`
   format 2 and 0.5.1 reads format 3. The diff is still exact; the next run reads the new history and
   the warning is gone (measured: a second Kina run printed `unchanged added=0 removed=0 modified=0`).
3. **17 route facts get the wrong learned state** in that one diff: 15 on Kina, 2 on quokka-stack
   (section 3.2). Accept them, reset their salience, or re-seed from scratch; the choice is Engram's.
4. **New provenance values** reach Engram: `inferred:wrapper` and `external` are kept by the default drop
   set, and more nodes carry `test_fixture`, which it drops (section 4.3).

## 2. The pin

Engram links no glia crate: engram-core is the only thing the two repos share, and glia 0.5.1 did not
touch it. The exporter is glia's `engram-export` crate, built by glia's `scripts/check-engram-export.sh`.
So the exact pin is the glia commit the exporter is built from: the `v0.5.1` tag. The glia session
builds it there (its release close-out runs `bash scripts/check-engram-export.sh`, which leaves the
binary at `engram-export/target/debug/glia-export-engram`). The Engram session only reads glia, so it
checks the build before exporting instead of checking anything out:

```
git -C /home/ivy/Code/glia describe --tags --exact-match      # expect v0.5.1; the binary itself cannot say
ls -l /home/ivy/Code/glia/engram-export/target/debug/glia-export-engram   # built after that tag's commit
```

The docs that name the exporter path (`ROADMAP.md:114-116`, `docs/dogfood-quokka-stack.md:17-19`, `:40`)
keep working unchanged.

**The check at HEAD.** The W10 close-out ran `check-engram-export.sh` at glia `17a63e2`:
`[engram-export] check: ok - build + 61 tests passed against engram-core GMAP_FORMAT_VERSION=6`. That
ran after this repo's last engram-core commit (`b740a58`, which removed `encodable_text_short`; glia
never used it). glia's two later commits (`491c2ad`, `2794ec5`) touch dev-notes, README and a CLI test,
nothing the exporter builds. The 0.5.0 exporter also still builds against engram-core at `b740a58`
(this doc's baseline build).

## 3. Upgrading a seeded store

A seeded store applies only `<gmap>.diff` whose base is its `applied_digest`
(`crates/engram-live/src/gmap_sync.rs:67-160`). So the upgrade is one `--since` export from the gmap the
store last applied:

```
X=/home/ivy/Code/glia/engram-export/target/debug/glia-export-engram     # built at v0.5.1
$X /home/ivy/Code/quokka-stack --since /tmp/quokka-stack.engram-gmap --out /tmp/quokka-stack.engram-gmap
# then start engram-mcp / engram-live with --seed /tmp/quokka-stack.engram-gmap as usual
```

### 3.1 What the diff holds

The diff matches keys first, then an `identity_hint` that is unique on both sides among the nodes still
unmatched (glia's `engram-export/src/diff.rs`, "Matching nodes"). Measured:

- **Renamed Go routes pair as moves.** CB.23 (Go parser emits router mounts, `e072dce`) with CB.20 (the
  build-time mount pass, `af2f094`) gives a Go ROUTE registered on a mounted group its full path:
  quokka-stack `POST /activity @turps` -> `POST /api/protected/activity @turps`, Kina
  `GET /stats @backend` -> `GET /admin/stats @backend`. The key changes, but the hint
  (`<file>:<kind>:<ordinal>`) usually does not, so the diff lists them as `modified` with
  `prior_key != node.key` and Engram carries each FactId across: 58 on quokka-stack, 87 on Kina (17 of
  these pairs are wrong, section 3.2). A function mounted twice becomes two ROUTEs; the second is
  `added` (`GET /api/protected/user/:publicId @turps`, `GET /api/trades/:id/dispute/proposals @backend`).
- **location_only** (quokka 333, Kina 48): nodes whose hint or span moved and nothing else. Inserting a
  node ahead of same-kind siblings in a file shifts their ordinals (every Dart file that gained
  constructors, section 4.1); a decorated TS method's span now starts at its first decorator (CB.4,
  `84a3581`).
- **Other `modified`:** Kina's 50 `frontend/e2e/` nodes now carry `test_fixture` (CG.2a, `5004a5a`), so
  `apply_gmap_diff` forgets them under the default drop set (`crates/engram/src/store.rs:1437-1442`);
  quokka's `endpoint:GET:/search @quokka_web` gains provenance `external` (CG.4b, `0a23e66`); 1 quokka and
  3 Kina doc sections changed text.
- **removed:** doc sections only. CE.4a (fence-aware markdown chunking, `111bd1b`) stops reading a `#`
  line inside a code fence as a heading: Kina loses 14 README sections (`docs::README::terminal-1`,
  `...::start-dev-server`, ...), quokka-stack 4 (`docs::.ai::WORKFLOW::list-open-mrs`, ...).
- **added:** quokka 154 (135 Dart constructors and factories in `quokka_android`, 12
  `data_entity:nosql:*`, 5 TS field methods, 1 split route, and the `turps::docs::swagger.yaml` module
  that CB.2 now reads; it is the 247th file), Kina 20 (19 TS field methods, 1 split route).
- **edges:** quokka +737 / -22 (Calls +419 / -4, Contains +131 / -1, Documents +158 / -8, Causes +18 / -5,
  Cooccurs +9, Imports +2, Implements -4); Kina +420 / -122 (Calls +316, Causes +82 / -25, Contains +19,
  Documents +3, Implements -97).

### 3.2 The 17 cross-paired route facts

When a mounted function splits into two ROUTEs, the clone takes a hint slot in its file, so every later
ROUTE in that file shifts one ordinal. Where the keys changed too, the hint pass pairs a base route with
its neighbour's new node. The node's content arrives correctly; the FactId, salience and experience links
it inherits belong to the other route. Measured:

| repo | base key (whose FactId is reused) | paired with |
|---|---|---|
| quokka-stack | `GET /user/search @turps` | `GET /api/protected/user/2fa @turps` |
| quokka-stack | `GET /user/:publicId @turps` | `GET /api/protected/user/search @turps` |
| Kina | `POST /trades/:id/pre-funding-check @backend` | `POST /api/trades @backend` |
| Kina | `POST /trades/:id/fund` | `POST /api/trades/:id/pre-funding-check` |
| Kina | `POST /trades/:id/confirm` | `POST /api/trades/:id/fund` |
| Kina | `POST /trades/:id/release` | `POST /api/trades/:id/confirm` |
| Kina | `POST /trades/:id/dispute` | `POST /api/trades/:id/release` |
| Kina | `POST /trades/:id/cancel` | `POST /api/trades/:id/dispute` |
| Kina | `POST /trades/:id/messages` | `POST /api/trades/:id/cancel` |
| Kina | `POST /trades/:id/extend-deadline` | `POST /api/trades/:id/messages` |
| Kina | `POST /trades/:id/pending-proof` | `POST /api/trades/:id/extend-deadline` |
| Kina | `GET /trades/:id/pending-proofs` | `POST /api/trades/:id/pending-proof` |
| Kina | `POST /trades/:id/pending-proof/:proofId/review` | `GET /api/trades/:id/pending-proofs` |
| Kina | `POST /trades/:id/dispute/propose` | `POST /api/trades/:id/pending-proof/:proofId/review` |
| Kina | `POST /trades/:id/dispute/respond` | `POST /api/trades/:id/dispute/propose` |
| Kina | `POST /trades/:id/dispute/penalty-hold` | `POST /api/trades/:id/dispute/respond` |
| Kina | `GET /trades/:id/dispute/proposals` | `POST /api/trades/:id/dispute/penalty-hold` |

(Kina keys all end ` @backend`.) The other 56 + 72 route moves pair correctly. Three ways through:
- **apply the diff** and accept it: 17 route facts carry a neighbour's salience until use corrects it;
- **apply the diff, then reset** the salience of the 17 target keys above;
- **re-seed**: delete the store and seed from the 0.5.1 gmap. Every FactId is new and learned state is
  lost; nothing is mis-bound.

This is an exporter matching rule, and the fix belongs in glia (for example: skip the hint pass for a
file whose same-kind node count changed between base and target). It is not fixed in 0.5.1.

### 3.3 The history layout

`--since <prior>` reads `<prior>.glia/`, the graph the previous export recorded, to carry file identities
across moves (LG.9, 0.5.0's move-stable hints). The 0.5.0 exporter wrote it as `.gmap` format 2; glia
0.5.1's format is 3 (CD.7b, EVIDENCE strings interned, `4fdf434`), so the first 0.5.1 run cannot read it
and prints the warning in section 1. It then writes a format-3 history. The effect is one run with no
file-move detection, which matters only if a file was moved or renamed between the two exports. CD.7c
(CODE cells stored as spans into the source, `5611813`) also changed how the history is written: it is
now written with no repo root, so its CODE stays inline and move detection works after the sources
change. CD.7c does not reach the gmap itself: the exporter builds the graph in memory, where CODE is
always text, so Symbols' doc and Propositions' text are read as before.

## 4. What changed in the facts

### 4.1 Symbols

| packet (commit) | effect on the export |
|---|---|
| CB.23 + CB.20 + CB.11 Go route mounts (`e072dce`, `af2f094`, `8b3d8fd`) | mounted Go ROUTE keys gain their prefix (section 3.1). Kina also loses 1 false HTTP_CALLS (`endpoint:GET:${…}/public/stats @frontend` -> `GET /stats @backend`) and gains 33 correct ones from the new mount-segment fold. |
| CB.9 Dart constructors, factories, operators (`ec5c6ea`), CB.17 Dart extensions and initialisers (`205861c`) | new METHOD Symbols `<T>::<T>`, `<T>::_`, `<T>::fromJson`, `<T>::operator+` ...; ATTRIBUTE `<Enum>::<c>`; CLASS `<module>::extension<T>` (keys with `<` `>`); Dart CALLS now start at the constructor, not the class |
| CG.1 TS / JS arrow-function class fields (`8bff4f9`) | new METHOD Symbols `<Class>::<field>` with their body's CALLS (quokka: `handleMotionPreferenceChange`, the Engram battery's marketing-ascii-hero miss; Kina: 19, such as `PasswordField::onChange`) |
| CB.10 Swift (`90d1076`), CB.19 C/C++ (`70286f0`), CB.1 `.hh` / `.inl` / `.graphqls` routing (`84448eb`), CB.14 Python GraphQL resolvers (`a2a619e`), CA.6b Ktor-client endpoints (`d739141`), CB.13 tRPC callers (`636cc57`) | new Symbols; a C++ out-of-line member of a nested type or of a class declared only in a newly routed header changes from FUNCTION `<file>::Q::m` to METHOD `<scope>::Q::m` (a new key) |
| CA.4 inferred Go collection wrappers (`a1e05a5`) | DATA_ENTITY Symbols `data_entity:nosql:<name>` with provenance `inferred:wrapper` (quokka-stack: 12, from `NewCollection[T]`) |
| CB.3a verb-named events removed (`444848b`), CB.3b constant-keyed events fold to the literal (`5a968a7`) | `event_emit:emit` / `Subject.next` / `publish` ... and `event_handle:on` / `subscribe` ... no longer exist; a constant-keyed site is keyed by its literal. Neither tree above has one; most of these are in the export's default noise filter anyway. |
| CB.4 decorated TS methods (`84a3581`) | the Symbol's span (bytes and `start_line`) starts at the first decorator |
| CB.22 C/C++ include paths (`24b3cfa`), CB.10 | `Content::Symbol.imports` (from the IMPORTS cell) gains C/C++ system and third-party headers (`stdio.h`, `curl`); a Swift test module lists `X`, not `@testable import X` |

### 4.2 Propositions and Documents

- **CE.4a (`111bd1b`):** fenced `#` lines are no longer sections (the removed keys in 3.1), and the 2nd+
  section with a repeated slug gets `<...>::<slug>-N` (one NodeId used to be emitted twice). Kina's
  Propositions go 31 -> 17, quokka's 278 -> 274.
- **CG.3 (`dd191fa`):** glia now stores a markdown section's whole text (up to 64 KiB) in its CODE cell,
  so the doc linker sees backticked names past byte 500: Documents edges quokka 169 -> 359, Kina 1 -> 4
  from the linker. The exporter keeps capping Proposition text at 500 bytes (`cap_prose` in glia's
  `engram-export/src/lib.rs:568-590`), so **Proposition text is byte-identical** to 0.5.0. Engram's
  `b740a58` now embeds the full `encodable_text`; if it wants the whole section as Proposition text,
  that is a one-line change in glia's exporter (drop the `cap_prose` call). Ask glia; it is not in 0.5.1.
- **CB.2 (`2ff7f49`):** a config yaml with a versionless `openapi:` / `swagger:` key no longer mints
  contract Propositions; swaggo's alphabetical `swagger.yaml` now does. quokka-stack has both
  `turps/docs/swagger.json` and `swagger.yaml`, whose ops share keys; the exporter keeps one
  (`44 duplicate key(s) resolved to the located node`, up from 4). The twins share one identity by design (LB.12), and the exporter's
  resolution to the located node is the intended handling; Engram needs nothing.

### 4.3 Provenance

| value | where from | Engram default |
|---|---|---|
| `test_fixture` on more nodes | CG.2a path-based test / fixture provenance (`5004a5a`): `e2e/`, `cypress/`, `fixtures/`, `testdata/`, `__mocks__/`, `integration_test/`, first-segment `test(s)/`, `*.spec.tsx`, `*.cy.ts`, `test_*.py` ... Kina +50 (`frontend/e2e`), glia +2,545 (bench fixtures), quokka +0 | dropped (`gmap_sync.rs:20-31`); `--include-tests` keeps them |
| `external` | CG.4b third-party endpoints (`0a23e66`): an ENDPOINT whose every call site dials a public, unconfigured host and that nothing pairs. quokka: `endpoint:GET:/search @quokka_web` (nominatim) | kept (not in the drop set); add it to the drop set to hide third-party APIs |
| `inferred:wrapper` | CA.4 (`a1e05a5`) | kept |
| `generated_proto` on new nodes | CB.9: constructors and factories inside `.pb.dart` files (quokka 166 -> 183) | dropped |

`crates/engram-live/examples/quokka_recall.rs:39-50` keeps its own copy of the drop list; it needs the
same decision.

### 4.4 Edges

- **Calls** up 24% on quokka-stack and 14% on Kina: CA.1 Go calls inside closures (`efdf872`, including
  the returned-middleware case the battery found), CA.2a / CA.2b Go receivers typed from a call's
  return, a local, a parameter or a field chain (`43260b9`, `7788f90`), CB.18 Swift implicit self
  (`008f2b8`), CB.25 C++ class scope (`b78c622`), CA.6a Kotlin receivers (`bae111b`), CB.7 PHP `use`
  (`0aca1b9`), and CG.1's TS field methods (`8bff4f9`). CB.18 and CB.25 also retarget a bare call inside
  a member from a same-named free function to the member (the old edge goes).
- **Implements** down: CA.3b (`c8f2257`) drops a Go type -> interface pair whose method signatures
  differ, or, for a one-method interface or a test-file side, whose packages no import links. Kina
  305 -> 208 exported (glia measured 132 -> 84 type-level). The exporter maps IMPLEMENTS to
  `Implements` at a fixed 0.8, so the confidence change on the survivors (Strong -> Medium on method-level
  pairs) does not reach Engram.
- **Causes:** CA.5a Go route handlers as receiver method values (`22c3d82`: Kina routes with a handler
  36 -> 89 of 90), host narrowing for WS / gRPC / GraphQL / tRPC (CB.21, `016775d`; CB.24, `99502f9`),
  external endpoints no longer paired (CG.4b), feature-flag reads now start at the reading function
  (CC.7a, `74f9402`), in-process events across nested projects (CB.5, `25dcc4f`).
- **Documents:** section 4.2.
- **Contains:** the new Dart, Swift and TS members; CB.15 (`0742baf`) hangs a C# / PHP namespace opened
  by several files under its first file (its key is unchanged).

## 5. `identity_hint`, for the record

Several packet reports say "Engram's identity_hint follows the qname" (CB.3b, CE.4a) or "existing hints
do not change" (CG.1). Neither is how the exporter builds it. The hint is `<file token>:<kind>:<ordinal>`,
where the ordinal ranks the node among same-kind nodes in its file by start row, then NodeId (glia's
`engram-export/src/lib.rs:470-505`). It is name-free:
- a rename keeps the hint when the file, the kind and the rank hold, which is why 145 renamed Go routes
  paired as moves, 128 of them with the right FactId (section 3.1);
- a node inserted ahead of same-kind siblings shifts their hints; through a diff that is harmless for
  nodes whose key is unchanged (keys match first), and it caused the 17 cross-pairs where the key changed
  too (section 3.2);
- a kind change (C++ FUNCTION -> METHOD) changes the hint.

An empty store seeded from a 0.5.1 gmap mints fresh FactIds, so none of this applies to a full re-seed.

## 6. engram-export changes in 0.5.1

| packet (commit) | change | Engram action |
|---|---|---|
| CG.2b `--exclude-path <glob>` (`301a7d5`) | drops every node whose POSITION file matches the glob (repo-relative, `*` spans `/`) and leaves those files out of the file table. `ExportOptions.exclude_paths` and `ExportStats.dropped_excluded_path` are new pub fields (Engram links neither). Without the flag, glia's export is byte-identical. | optional: for glia's own repo, `--exclude-path 'bench/substrate-gap/fixtures/*'` leaves the fixture nodes out of the gmap (CG.2a counted 2,473 there) instead of dropping them at seed time |
| CD.7c history layout written rootless (`5611813`) | section 3.3 | none |
| CG.3 export-side `cap_prose` (`dd191fa`) | Proposition text stays at 500 bytes | section 4.2 |
| CG.4b provenance test (`0a23e66`) | `external` is exported through the ORIGIN cell | section 4.3 |
| C0.7 (`457eba5`), CD.6a (`fc7cc14`) | `engram-export/Cargo.lock` gained the engine's new crates (lz4_flex, blake3, toml_edit, glia-projection-text) and lost three unused parser lines under glia-graph | none |

The exporter's markers are unchanged in shape; the lines above are verbatim.

## 7. Not verified, and open

- **Not run:** a seed or diff apply in Engram itself. The diff counts above come from the exporter; what
  `apply_gmap_diff` does with them is read from `store.rs:1393-1480`, not executed.
- **Not measured:** glia's own repo (its working tree is mid-wave). CG.2a's report gives +2,545
  `test_fixture` nodes there.
- **glia-side, open:** the hint-pass cross-pairing (section 3.2); the exporter does not print glia's BUILD_STAMP, so a gmap cannot say which glia built it.
- **The 0.5.2 bet** "Engram PPR memory" (glia `dev-notes/research-0.5.2/R2-engram.md`) waits for 0.5.1
  to ship; nothing in 0.5.1 depends on it.
