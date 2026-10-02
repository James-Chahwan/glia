# glia 0.5.1 has landed - engram read-side handoff

**From:** the glia session, 2026-10-01; this revision is by the docs packet CZ.3 (2026-10-02), after
glia's last code wave.
**To:** an engram session.
**Status:** no contract change. `GMAP_FORMAT_VERSION` stays 6, and glia 0.5.1 touches no Engram file.
Engram needs **no code change**. What changes is the content of every export, the first `--since` run
after the upgrade, and one new sidecar beside every gmap, `<out>.meta.json` (section 6).

How the numbers were measured (2026-10-02). Two exporter builds ran on the same trees: `git archive`
copies of quokka-stack `a77d4cb` and Kina `a448fb3`, exported with `--no-persist`, so nothing was
written into either repo.
- The 0.5.0 side is the gmaps a glia v0.5.0 (`2170ff8`) exporter wrote on 2026-10-01, built against
  this repo's engram-core.
- The 0.5.1 side is glia HEAD `dbd8d2a`, the W18 close-out build of
  `engram-export/target/debug/glia-export-engram`. Its `-V` prints
  `glia-export-engram 0.5.0 (build 0.5.0+pa0fc26c918dfcc7c)`: the version field moves to 0.5.1 only at
  the bump.

The first revision of this doc (2026-10-01) measured glia `491c2ad` (after W10). Every number below
replaces it. Base digests differ from that revision's: a copy with no git remote takes its RepoId from
its directory name, and the copies sat in different directories. glia 0.5.1 is not tagged or pushed
yet: it all comes from the local `/home/ivy/Code/glia` checkout.

0.5.1 is glia's "catch-up leap": 198 packets (`dev-notes/leap-051-packets.json` in glia). 155 landed in
waves W0-W11 since v0.5.0, and a 43-packet finishing batch (groups CH-CL) in W12-W19. Packet ids below
are glossed in words; the hash after each is the glia commit that landed it.

---

## 1. TL;DR

| | quokka-stack 0.5.0 -> 0.5.1 | Kina 0.5.0 -> 0.5.1 |
|---|---|---|
| nodes / edges / files | 2,784 / 4,977 / 246 -> 3,272 / 6,709 / 288 | 4,084 / 9,245 / 530 -> 5,544 / 12,408 / 530 |
| Symbols / Propositions | 2,506 / 278 -> 2,730 / 542 | 4,053 / 31 -> 5,527 / 17 |
| Calls | 1,896 -> 2,416 | 2,790 -> 4,369 |
| Implements | 28 -> 27 | 305 -> 208 |
| Extends | 0 -> 3 | 3 -> 10 |
| Causes | 253 -> 310 | 1,083 -> 1,141 |
| Contains | 2,087 -> 2,246 | 2,991 -> 4,465 |
| Documents | 169 -> 1,142 | 8 -> 11 (4 doc-linker + 7 NatSpec at 0.5.1) |
| Imports / Cooccurs | 521 / 23 -> 533 / 32 | 1,916 / 149 -> 1,916 / 288 |
| identity hints | 2,642 -> 3,088 | 3,617 -> 5,078 |
| provenance changes | `documentation` 137 -> 401, `inferred:wrapper` 0 -> 12, `external` 0 -> 2, `generated_proto` 166 -> 183, `test_fixture` 21 -> 24 | `test_fixture` 404 -> 461, `documentation` 24 -> 10 |

The first `--since` run from a 0.5.0 gmap to a 0.5.1 one (the path a seeded store takes):

```
quokka-stack
[engram-export] v6 identity: prior graph unavailable at .../quokka050.engram-gmap.glia (old format v2 (this build reads v3)) - moves not detected this run
[engram-export] glia build=0.5.0+pa0fc26c918dfcc7c exporter=0.4.13 format_version=6 meta=.../quokka051.engram-gmap.meta.json
[engram-export] v6 since: base=4748061be1f53caf target=d65f627990f943ae added=543 removed=55 modified=393 (moved=58 location_only=333) edges +1758/-138; parse cache off
[engram-export] v6 since pairing: route=58 name=0 hint=0 hint_refused=0
Kina
[engram-export] v6 identity: prior graph unavailable at .../kina050.engram-gmap.glia (old format v2 (this build reads v3)) - moves not detected this run
[engram-export] glia build=0.5.0+pa0fc26c918dfcc7c exporter=0.4.13 format_version=6 meta=.../kina051.engram-gmap.meta.json
[engram-export] v6 since: base=e3f18515a90afc3d target=9d0e7988831cf720 added=1476 removed=16 modified=189 (moved=87 location_only=48) edges +3121/-111; parse cache off
[engram-export] v6 since pairing: route=85 name=0 hint=2 hint_refused=0
```

What Engram should know:
1. **Pin:** build the exporter from glia's `v0.5.1` tag (section 2). `glia-export-engram -V` prints glia's
   version line and BUILD_STAMP (`glia-export-engram 0.5.1 (build 0.5.1+p<16 hex>)` once tagged), and
   every export leaves `<out>.meta.json` beside the gmap, saying which glia wrote it. `glia inspect
   <out>.glia` reads the same build stamp from the history layout (CK.3, `d26ddf5`).
2. **Upgrade a seeded store with one `--since` run** (section 3). It prints the "prior graph
   unavailable" warning once, because the history layout the 0.5.0 exporter wrote is glia `.gmap` format
   2 and 0.5.1 reads format 3. The diff is still exact. The next run reads the new history and the
   warning is gone: a second Kina run printed
   `unchanged added=0 removed=0 modified=0 (moved=0 location_only=0) edges +0/-0`.
3. **Route facts pair correctly now.** At `491c2ad` the hint pass gave 17 route facts a neighbour's
   FactId: 15 on Kina, 2 on quokka-stack. The first revision offered three workarounds: accept, reset
   salience, or re-seed. CK.2 (`d9bdbbc`) fixed the matching in the exporter, so none is needed. All 145
   moved routes keep their verb and path tail (section 3.2).
4. **Re-keyed client endpoints arrive as removed + added** (section 3.1). An ENDPOINT fact carries no
   identity hint (0 of 95 on quokka-stack) and is not a ROUTE, so no pass pairs it. quokka's `/api`
   re-keys (CH.5b, CH.5c) and the `<unresolved>` sites that now read a real path (CH.3a) get fresh
   FactIds, and learned state on the old endpoint facts is forgotten. The rename rule is in section 3.1
   if Engram wants to carry salience across.
5. **New facts:**
   - Extends from Go struct embeds (CI.2a).
   - Implements from methods promoted through embeds (CI.2b), from Go methods typed through an in-repo
     type alias (CI.6) and from TS overrides of abstract members (CH.1b).
   - Calls from Go package-var initialisers (CI.1) and from TS call-initialised fields: Kina gains 1,455
     STATE_VAR Symbols (CH.2).
   - Cooccurs from function-level TESTS (CL.5b).
   - Propositions from project-root and SDD feature docs (CJ.2).
   - Provenance: `inferred:wrapper` and `external` are kept by the default drop set. More nodes carry
     `test_fixture`, which it drops, and a repo can now declare test paths in `.glia/overlay.toml`
     (CJ.4) (section 4).
6. **`.gitignore`:** add `*.engram-gmap.meta.json` beside the other sidecars, and have any step that
   copies a gmap with its sidecars copy the meta file too (section 6).

## 2. The pin

Engram links no glia crate: engram-core is the only thing the two repos share, and glia 0.5.1 did not
touch it. The exporter is glia's `engram-export` crate, built by glia's `scripts/check-engram-export.sh`.
So the exact pin is the glia commit the exporter is built from: the `v0.5.1` tag. The glia session
builds it there (its release close-out runs `bash scripts/check-engram-export.sh`, which leaves the
binary at `engram-export/target/debug/glia-export-engram`). The Engram session only reads glia, so it
checks the build before exporting instead of checking anything out:

```
git -C /home/ivy/Code/glia describe --tags --exact-match      # expect v0.5.1
/home/ivy/Code/glia/engram-export/target/debug/glia-export-engram -V
#   glia-export-engram 0.5.1 (build 0.5.1+p<16 hex>): glia's version line and BUILD_STAMP
```

A gmap already written says which glia built it, without the binary. Its `<out>.meta.json` holds
`glia_version`, `build_stamp`, `parser_stamp` and `gmap_digest`, with `meta_version` 1 (section 6).
`glia inspect <out>.glia` prints the history layout's `build_stamp`. The crate's own version stays
`0.4.13`, shown as `exporter_version` and in the `exporter=` field of the marker. Whether it moves to the
workspace version at the bump is open on glia's side.

The docs that name the exporter path (`ROADMAP.md:114-116`, `docs/dogfood-quokka-stack.md:17-19`, `:40`)
keep working unchanged.

**The check at HEAD.** The W18 close-out ran `check-engram-export.sh` at glia `dbd8d2a`:
`[engram-export] check: ok - build + 66 tests passed against engram-core GMAP_FORMAT_VERSION=6`. The
count was 61 at W10, 65 after CK.2 and 66 after CK.3. That ran after this repo's last engram-core commit
(`b740a58`, which removed `encodable_text_short`; glia never used it).

## 3. Upgrading a seeded store

A seeded store applies only `<gmap>.diff` whose base is its `applied_digest`
(`crates/engram-live/src/gmap_sync.rs:67-160`). So the upgrade is one `--since` export from the gmap the
store last applied:

```
X=/home/ivy/Code/glia/engram-export/target/debug/glia-export-engram     # built at v0.5.1
$X /home/ivy/Code/quokka-stack --since /tmp/quokka-stack.engram-gmap --out /tmp/quokka-stack.engram-gmap
# then start engram-mcp / engram-live with --seed /tmp/quokka-stack.engram-gmap as usual
```

The export writes `<out>`, `<out>.diff`, `<out>.files.json`, `<out>.meta.json` and the `<out>.glia/`
history.

### 3.1 What the diff holds

The diff matches nodes in four passes (glia's `engram-export/src/diff.rs`, CK.2):
1. keys;
2. ROUTEs by path within their file and verb;
3. other Symbols by (file, kind, name);
4. an `identity_hint` unique on both sides among the nodes still unmatched. This pass refuses two ROUTEs
   of different verbs or unalike paths (`hint_refused`).

The `v6 since pairing:` line counts each pass. Measured:

- **Renamed Go routes pair as moves.** CB.23 (Go parser emits router mounts, `e072dce`) with CB.20 (the
  build-time mount pass, `af2f094`) gives a Go ROUTE registered on a mounted group its full path:
  quokka-stack `POST /activity @turps` -> `POST /api/protected/activity @turps`, Kina
  `GET /stats @backend` -> `GET /admin/stats @backend`. The diff lists them as `modified` with
  `prior_key != node.key`, and Engram carries each FactId across: 58 on quokka-stack (all by path) and
  87 on Kina (85 by path, 2 by hint). A function mounted twice becomes two ROUTEs, and the second is
  `added`: `GET /api/protected/user/2fa @turps` and `POST /api/trades @backend`.
- **Client endpoints re-key as removed + added.** An ENDPOINT has no file of its own, so it carries no
  identity hint, and it is not a ROUTE: no pass pairs it. A re-key arrives as removed + added, a fresh
  FactId; it is not a move.
  - quokka-stack: 50 endpoint keys removed and 92 added.
    - CH.5b (`17c7bef`) folds the configured API prefix into the call sites read through `buildApiUrl`:
      `endpoint:GET:/protected/friends @quokka_web` -> `endpoint:GET:/api/protected/friends @quokka_web`.
    - CH.5c (`4eb3909`) does the same for quokka_android under its Dio base URL:
      `endpoint:POST:/protected/swipe @quokka_android` -> `endpoint:POST:/api/protected/swipe
      @quokka_android`.
    - CH.3a (`4c05d47`) gives sites that read `endpoint:<M>:<unresolved>` a real path, and the four
      `endpoint:{GET,POST,PATCH,DELETE}:<unresolved> @quokka_web` nodes go.
    - CH.5a (`c44a41e`) adds the generic `dio.get<T>(..)` calls (quokka_android ENDPOINTs 18 -> 41).
  - Kina: `endpoint:DELETE:<unresolved>` and `endpoint:PATCH:<unresolved>` are removed (CH.3c,
    `58fb26c`: a `Map` / store receiver is no HTTP client), and `endpoint:GET:/api/notifications
    @frontend` is added (CH.3a).
  - The rename rule, if Engram wants to carry salience across: `endpoint:<M>:<p> @quokka_web` ->
    `endpoint:<M>:/api<p> @quokka_web`, and `endpoint:<M>:<p> @quokka_android` ->
    `endpoint:<M>:/api<p> @quokka_android`, for every `<p>` the 0.5.0 gmap held. An `<unresolved>`
    node has no single successor.
- **location_only** (quokka 333, Kina 48): nodes whose hint or span moved and nothing else. Inserting a
  node ahead of same-kind siblings in a file shifts their ordinals (every Dart file that gained
  constructors, section 4.1); a decorated TS method's span now starts at its first decorator (CB.4,
  `84a3581`).
- **Other `modified`:** Kina's 50 `frontend/e2e/` nodes now carry `test_fixture` (CG.2a, `5004a5a`), so
  `apply_gmap_diff` forgets them under the default drop set (`crates/engram/src/store.rs:1437-1442`);
  quokka's `endpoint:GET:/search @quokka_web` gains provenance `external` (CG.4b, `0a23e66`); 1 quokka and
  3 Kina doc sections changed text.
- **removed:**
  - quokka-stack 55: 4 doc sections, the 50 endpoint keys above, and the ROUTE
    `POST /auth/refresh @quokka_android`. That ROUTE was a Dio call misread as a server route; CH.5c
    makes it the ENDPOINT `endpoint:POST:/api/auth/refresh @quokka_android`.
  - Kina 16: 14 README sections and the 2 `<unresolved>` endpoints.
  - The doc sections go because CE.4a (fence-aware markdown chunking, `111bd1b`) stops reading a `#` line
    inside a code fence as a heading: `docs::README::terminal-1`, `...::start-dev-server` and more on
    Kina, `docs::.ai::WORKFLOW::list-open-mrs` and more on quokka-stack.
- **added:**
  - quokka-stack 543:
    - 268 doc sections from the docs CJ.2 now reads (`6681070`: 20 -> 61 docs);
    - the 92 endpoints;
    - 12 `data_entity:nosql:*` (CA.4);
    - 1 split route;
    - 170 code Symbols: mostly CB.9's Dart constructors and factories, CH.2's 29 STATE_VARs and CG.1's 5
      TS field methods (17 carry `generated_proto`, 3 `test_fixture`).
  - Kina 1,476: 1,455 STATE_VARs (CH.2, `faa563b`) and 19 TS field methods (CG.1), 1 split route and 1
    endpoint. 7 of the new Symbols sit in `*.spec.ts` files and carry
    `test_fixture`.
- **edges:** the diff compares distinct (from, kind, to) triples, so its counts match the distinct
  edges, not the totals in section 1 (Calls keeps one edge per call site: quokka-stack distinct Calls
  1,508 -> 1,956, Kina 2,104 -> 3,530).

  | kind | quokka-stack | Kina |
  |---|---|---|
  | Calls | +529 / -81 | +1,438 / -12 |
  | Causes | +103 / -46 | +60 / -2 |
  | Contains | +160 / -1 | +1,474 / -0 |
  | Cooccurs | +9 | +139 |
  | Documents | +939 / -6 | +3 |
  | Extends | +3 | +7 |
  | Implements | +3 / -4 | -97 |
  | Imports | +12 | 0 |

  The removed Calls and Causes on quokka-stack are mostly the edges of the re-keyed endpoints, which
  come back on the new keys.

### 3.2 The route pairing (CK.2)

At `491c2ad`, matching was keys, then any `identity_hint` unique on both sides. The hint is
`<file token>:<kind>:<ordinal>`. When a mounted function split into two ROUTEs, the clone took a hint
slot in its file, and every later ROUTE there shifted one ordinal. Where the keys changed too, the hint
pass paired a base route with its neighbour's new node: 17 route facts, 15 on Kina
(`POST /trades/:id/fund` with `POST /api/trades/:id/pre-funding-check`, ...) and 2 on quokka-stack
(`GET /user/search @turps` with `GET /api/protected/user/2fa @turps`, ...).

CK.2 (`d9bdbbc`) pairs moved ROUTEs by path within their file and verb before any hint. Equal paths
pair first, then paths alike up to a leading-segment prefix, mutual-unique within a tier. The hint pass
refuses two ROUTEs of different verbs or unalike paths. Measured at `dbd8d2a`:
- quokka-stack: `route=58 name=0 hint=0 hint_refused=0`.
- Kina: `route=85 name=0 hint=2 hint_refused=0`.
- Every one of the 145 moved pairs keeps its verb, and its new path ends in its old one:
  `POST /trades/:id/fund @backend` -> `POST /api/trades/:id/fund @backend`,
  `GET /user/search @turps` -> `GET /api/protected/user/search @turps`.

So the 17 facts land on their own routes. The first revision's three ways through (accept, reset
salience, re-seed) are no longer needed.

CK.2's declared trade-off, measured nowhere yet: a root route `GET /` re-registered under a mount
prefix, and a route whose method changed, now arrive as removed + added with a fresh FactId. HEAD's
ordinal hint paired them. Both upgrades above print `hint_refused=0`.

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
| CB.23 + CB.20 + CB.11 Go route mounts (`e072dce`, `af2f094`, `8b3d8fd`), CI.5 two more mount shapes (`ecb0c81`) | mounted Go ROUTE keys gain their prefix (section 3.1). Kina also loses 1 false HTTP_CALLS (`endpoint:GET:${…}/public/stats @frontend` -> `GET /stats @backend`) and gains 33 correct ones from the mount-segment fold. CI.5's shapes (a field of another package's struct, a parameter-rooted field group) move no ROUTE on either tree |
| CH.3a URL builders, URL methods, `const` locals, `readonly` fields (`4c05d47`), CH.3c the non-HTTP receiver gate (`58fb26c`), CH.5b / CH.5c the `/api` fold (`17c7bef`, `4eb3909`) | endpoint keys re-key as removed + added (section 3.1). The ENDPOINT's `identity_hint` stays absent, so nothing pairs them |
| CH.2 call-initialised TS class fields (`faa563b`) | new STATE_VAR Symbols `<Class>::<field>` for `signal(..)`, `computed(..)`, `input.required<T>()`, `toSignal(..)`, `x$ = this.subject.asObservable()` ... (any call but `inject(..)`): Kina +1,455 (1,426 of them signal factories), quokka-stack +29. Each carries Contains from its class and Calls for its initialiser |
| CH.1 TS abstract classes (`d12e804`) | CLASS / METHOD Symbols for `abstract class` declarations; neither tree has one |
| CB.9 Dart constructors, factories, operators (`ec5c6ea`), CB.17 Dart extensions and initialisers (`205861c`) | new METHOD Symbols `<T>::<T>`, `<T>::_`, `<T>::fromJson`, `<T>::operator+` ...; ATTRIBUTE `<Enum>::<c>`; CLASS `<module>::extension<T>` (keys with `<` `>`); Dart CALLS now start at the constructor, not the class |
| CG.1 TS / JS arrow-function class fields (`8bff4f9`) | new METHOD Symbols `<Class>::<field>` with their body's CALLS (quokka: `handleMotionPreferenceChange`, the Engram battery's marketing-ascii-hero miss; Kina: 19, such as `PasswordField::onChange`) |
| CB.10 Swift (`90d1076`), CB.19 C/C++ (`70286f0`), CB.1 `.hh` / `.inl` / `.graphqls` routing (`84448eb`), CB.14 Python GraphQL resolvers (`a2a619e`), CA.6b Ktor-client endpoints (`d739141`), CB.13 tRPC callers (`636cc57`) | new Symbols; a C++ out-of-line member of a nested type or of a class declared only in a newly routed header changes from FUNCTION `<file>::Q::m` to METHOD `<scope>::Q::m` (a new key) |
| CA.4 inferred Go collection wrappers (`a1e05a5`) | DATA_ENTITY Symbols `data_entity:nosql:<name>` with provenance `inferred:wrapper` (quokka-stack: 12, from `NewCollection[T]`) |
| CB.3a verb-named events removed (`444848b`), CB.3b constant-keyed events fold to the literal (`5a968a7`) | `event_emit:emit` / `Subject.next` / `publish` ... and `event_handle:on` / `subscribe` ... no longer exist; a constant-keyed site is keyed by its literal. Neither tree above has one; most of these are in the export's default noise filter anyway. |
| CL.1 queue framework tags only for unexplained calls (`6da6473`), CL.2 / CL.3 Go, JVM and .NET broker rows (`b896904`, `7456d6e`), CL.4 JobRunr / Hangfire / asynq (`d1fd77c`) | a `queue_consumer:<q>` minted by a RabbitMQ declaration alone and an explained `queue_*:unresolved:<family>` tag no longer exist; new `queue_producer:<t>` / `queue_consumer:<t>` Symbols, a job keyed by the method or task type it runs. Neither tree has one |
| CL.7b .NET application settings (`2e0ef8a`) | new `config:setting:<Section:Key>` Symbols from `appsettings*.json` and from every env define `Section__Key` (DEFINES_CONFIG, EVIDENCE rule `dotnet_env_override`). Neither tree has one |
| CL.6a / CL.6b GraphQL clients and HotChocolate (`c052a5b`, `7941f3a`), CL.7a C# env reads (`3241195`), CL.8 Go EventBus (`ceb7953`), CL.9 NestJS cron (`05f01ed`), CL.10 JVM process launches (`a8ef0f4`) | new Symbols in existing key shapes; none on either tree |
| CJ.1a-CJ.1c the `.rs` / `.py` literal and comment guard (`96f9280`, `aae59c6`, `472de38`) | a queue / event / WS / gRPC / GraphQL / cron / data-source / env / secret / flag Symbol minted from a needle inside a Rust or Python string literal or comment no longer exists. That is glia's own self-export (175 queue / event phantoms -> 0, 36 WS / gRPC / GraphQL -> 1); neither tree above is Rust or Python |
| CB.4 decorated TS methods (`84a3581`) | the Symbol's span (bytes and `start_line`) starts at the first decorator |
| CB.22 C/C++ include paths (`24b3cfa`), CB.10 | `Content::Symbol.imports` (from the IMPORTS cell) gains C/C++ system and third-party headers (`stdio.h`, `curl`); a Swift test module lists `X`, not `@testable import X` |

### 4.2 Propositions and Documents

- **CE.4a (`111bd1b`):** fenced `#` lines are no longer sections (the removed keys in 3.1), and the 2nd+
  section with a repeated slug gets `<...>::<slug>-N` (one NodeId used to be emitted twice). Kina's
  Propositions go 31 -> 17.
- **CJ.2 (`6681070`):** glia now reads the well-known files and `docs/` tree at every PROJECT root, and
  SDD feature docs (`features/<f>/*.md`, spec-kit `specs/<NNN-slug>/`), at the repo root or a project
  root. quokka-stack's `[docs] scope: project_root=5 project_docs=0 feature=36 spec_kit=0 (root=1 docs=3
  ai=16 adr=0)` adds 268 Propositions (278 -> 542 with CE.4a's 4 removed). Kina's `specs/` is a symlink
  into its untracked `.wiki/specs`, so a `git archive` copy cannot hold it. CJ.2's effect on Kina's real
  tree is not measured here.
- **CG.3 (`dd191fa`):** glia now stores a markdown section's whole text (up to 64 KiB) in its CODE cell,
  so the doc linker sees backticked names past byte 500. With CJ.2's new sections, Documents edges go
  quokka 169 -> 1,142 and Kina 1 -> 4 from the linker. The exporter keeps capping Proposition text at
  500 bytes (`cap_prose` in glia's `engram-export/src/lib.rs`), so **Proposition text is byte-identical**
  to 0.5.0 for a section that existed then. Engram's `b740a58` embeds the full `encodable_text`. If
  Engram wants the whole section as Proposition text, that is a one-line change in glia's exporter (drop
  the `cap_prose` call). Ask glia; it is not in 0.5.1.
- **CB.2 (`2ff7f49`):** a config yaml with a versionless `openapi:` / `swagger:` key no longer mints
  contract Propositions; swaggo's alphabetical `swagger.yaml` now does. quokka-stack has both
  `turps/docs/swagger.json` and `swagger.yaml`, whose ops share keys; the exporter keeps one
  (`44 duplicate key(s) resolved to the located node`, up from 4). The twins share one identity by
  design (LB.12), and the exporter's resolution to the located node is the intended handling; Engram
  needs nothing.

### 4.3 Provenance

| value | where from | Engram default |
|---|---|---|
| `test_fixture` on more nodes | CG.2a path-based test / fixture provenance (`5004a5a`): `e2e/`, `cypress/`, `fixtures/`, `testdata/`, `__mocks__/`, `integration_test/`, first-segment `test(s)/`, `*.spec.tsx`, `*.cy.ts`, `test_*.py` ... Kina 404 -> 461 (`frontend/e2e` and new nodes in spec files), quokka 21 -> 24, glia +2,545 (bench fixtures). Also, since CJ.4 (`ac554f2`), any path a repo declares in `.glia/overlay.toml` `[walk] tests = [..]` (gitignore syntax, repo-root anchored, additive); neither tree declares one | dropped (`gmap_sync.rs:20-31`); `--include-tests` keeps them |
| `external` | CG.4b third-party endpoints (`0a23e66`): an ENDPOINT whose every call site dials a public, unconfigured host and that nothing pairs. quokka: `endpoint:GET:/search @quokka_web` (nominatim) and, since CH.3a read its `const` URL, `endpoint:GET:/reverse @quokka_web` | kept (not in the drop set); add it to the drop set to hide third-party APIs |
| `inferred:wrapper` | CA.4 (`a1e05a5`) | kept |
| `generated_proto` on new nodes | CB.9: constructors and factories inside `.pb.dart` files (quokka 166 -> 183) | dropped |

`crates/engram-live/examples/quokka_recall.rs:39-50` keeps its own copy of the drop list; it needs the
same decision.

### 4.4 Edges

- **Calls** up 27% on quokka-stack and 57% on Kina (totals):
  - Go: CA.1 calls inside closures (`efdf872`, including the returned-middleware case the battery
    found); CA.2a / CA.2b receivers typed from a call's return, a local, a parameter or a field chain
    (`43260b9`, `7788f90`); CI.1 Calls **from a Go package-var STATE_VAR Symbol** for the calls in its
    initialiser (`26d2bd9`: quokka-stack +43, Kina +5).
  - TS: CH.2 Calls from the new TS STATE_VARs, and from members into them for `this.<signal>()` reads
    (`faa563b`: Kina +1,203 at CH.2); CH.1b Calls through TS superclasses for a `this.m()` /
    `super.m()` the caller's class cannot bind (`d8e5c4c`, EVIDENCE rule `inherited_method`; no
    `[ts-inherit]` line on either tree, so 0 there); CG.1's TS field methods (`8bff4f9`).
  - Other languages: CB.18 Swift implicit self (`008f2b8`), CB.25 C++ class scope (`b78c622`), CA.6a
    Kotlin receivers (`bae111b`), CB.7 PHP `use` (`0aca1b9`), and CL.5a calls on a freshly constructed
    object, `new Calc().add(..)` (`052d0c5`; `[recv] constructed receivers: bound=0` on both trees).
    CB.18 and CB.25 also retarget a bare call inside a member from a same-named free function to the
    member (the old edge goes).
- **Implements:**
  - CA.3b (`c8f2257`) drops a Go type -> interface pair whose method signatures differ, or, for a
    one-method interface, whose packages share no import path. Kina 305 -> 208 exported (glia measured
    132 -> 84 type-level).
  - CI.3 (`99e2d10`) narrowed the test-file rule: a pair with a side in a `_test.go` file pairs only
    when that test file imports the other side's package directly (or both share a package). grpc-go
    drops 9 type-level + 10 method-level pairs; `implementors` of `Closable` goes 8 -> 1. Kina, lapse
    and quokka-stack lose none.
  - New pairs come from methods promoted through embedded fields (CI.2b, `5fc42bb`: a gRPC server
    embedding `pb.UnimplementedXServer` implements `XServer`; quokka-stack +1, grpc-go 675 -> 1,098
    type-level). Others come from signatures equal once an in-repo type alias is resolved (CI.6,
    `4942139`: quokka-stack's `turps::Services::chat::server::Server` -> `QuokkaChatServiceServer` and
    its stream method; grpc-go 1,098 -> 1,137).
  - TS overrides of abstract members get a method-level Implements (CH.1b, EVIDENCE rule
    `abstract_override`; none on either tree).
  - quokka-stack nets 28 -> 27 (+3 / -4). The exporter maps IMPLEMENTS to `Implements` at a fixed
    0.8, so the confidence of a pair (Strong, Medium) does not reach Engram.
- **Extends:** CI.2a (`643de54`) makes every in-repo Go struct embed a STRUCT -> STRUCT / INTERFACE
  INHERITS_FROM, exported as Extends: quokka-stack 0 -> 3, Kina 3 -> 10 (its 7 embeds).
- **Cooccurs:** CL.5b (`eb302a2`) adds function-level TESTS (test fn -> unit fn) beside the module
  pairs, and the exporter maps TESTS to Cooccurs: Kina +139 (`[tests] fn TESTS edges: 139 (test
  fns=83, helpers skipped=10, module pairs=57, already joined=0)`).
- **Imports:** CH.4 (`3e6e9f3`) makes a method passed by value (`addEventListener('resize',
  this.onResize)`) a USES, exported as Imports: quokka-stack +10 of its +12.
- **Causes:**
  - CA.5a Go route handlers as receiver method values (`22c3d82`: Kina routes with a handler 36 -> 89
    of 90). CI.4 (`961c832`) binds handlers typed in another file of the package; it moves none on
    these trees.
  - Host narrowing for WS / gRPC / GraphQL / tRPC (CB.21, `016775d`; CB.24, `99502f9`).
  - External endpoints are no longer paired (CG.4b). Feature-flag reads now start at the reading
    function (CC.7a, `74f9402`). In-process events pair across nested projects (CB.5, `25dcc4f`).
  - The HTTP_CALLS of quokka's re-keyed endpoints now pair `exact` (`[http] exact=91 rprefix=1`, was
    `exact=3 rprefix=90`).
- **Documents:** section 4.2.
- **Contains:** the new Dart, Swift and TS members and STATE_VARs; CB.15 (`0742baf`) hangs a C# / PHP
  namespace opened by several files under its first file (its key is unchanged).

## 5. `identity_hint`, for the record

The hint is `<file token>:<kind>:<ordinal>`, where the ordinal ranks the node among same-kind nodes in
its file by start row, then NodeId (glia's `engram-export/src/lib.rs`). It is name-free, and since CK.2
it is the diff's fourth and last matching pass, after keys, route paths and (file, kind, name):
- a ROUTE rename pairs by path before the hint is read, so all 145 renamed Go routes paired as moves
  with the right FactId (section 3.2);
- a node inserted ahead of same-kind siblings shifts their hints; through a diff that is harmless for
  nodes whose key is unchanged (keys match first), and the hint pass now refuses an unalike ROUTE pair;
- a kind change (C++ FUNCTION -> METHOD) changes the hint;
- a node with no file of its own (an ENDPOINT, most `config:` / `queue_` / `data_entity:` keys) has no
  hint, so a re-key of one is removed + added (section 3.1).

Several packet reports say "Engram's identity_hint follows the qname" (CB.3b, CE.4a) or "existing hints
do not change" (CG.1); neither is how the exporter builds it. An empty store seeded from a 0.5.1 gmap
mints fresh FactIds, so none of this applies to a full re-seed.

## 6. engram-export changes in 0.5.1

| packet (commit) | change | Engram action |
|---|---|---|
| CK.3 the build sidecar and version (`d26ddf5`) | `-V` / `--version` print glia's version line, `glia-export-engram <release> (build <BUILD_STAMP>)` (was `glia-export-engram 0.4.13`). Every run prints `[engram-export] glia build=<BUILD_STAMP> exporter=<crate version> format_version=6 meta=<out>.meta.json` after the `wrote` line. Every gmap write (the bin, `export_engram_gmap`, `write_engram_gmap`; `--since` included, never on a refused run) leaves `<out>.meta.json` right after `<out>.files.json`, tmp-then-rename. It is sorted JSON with nine keys: `build_stamp`, `exporter`, `exporter_version`, `format_version`, `glia_version`, `gmap_digest`, `meta_version` (1), `parser_stamp`, `version_line`. `gmap_digest` is the gmap's content digest, the `digest=` of the `wrote` line (quokka-stack: `d65f627990f943ae` in both), so a sidecar left beside a newer gmap is detectable. Gmap, diff, files.json and history bytes are unchanged | add `*.engram-gmap.meta.json` to Engram's `.gitignore` (it lists `*.engram-gmap`, `.files.json`, `.diff` and `.glia/` today); any seed-copy step that copies a gmap with its sidecars copies the meta file too. Reading it is optional |
| CK.2 route-aware `--since` matching (`d9bdbbc`) | the four passes of section 3.1; a new line after the unchanged `v6 since:` one: `[engram-export] v6 since pairing: route=<r> name=<n> hint=<h> hint_refused=<x>`. `DiffStats` gains `by_route`, `by_name`, `by_hint`, `hint_refused` (Engram links none of it). No `GmapDiff` format change | none: the 17 cross-paired facts of the first revision are gone (section 3.2) |
| CG.2b `--exclude-path <glob>` (`301a7d5`) | drops every node whose POSITION file matches the glob (repo-relative, `*` spans `/`) and leaves those files out of the file table. `ExportOptions.exclude_paths` and `ExportStats.dropped_excluded_path` are new pub fields (Engram links neither). Without the flag, glia's export is byte-identical. | optional: for glia's own repo, `--exclude-path 'bench/substrate-gap/fixtures/*'` leaves the fixture nodes out of the gmap (CG.2a counted 2,473 there) instead of dropping them at seed time |
| CD.7c history layout written rootless (`5611813`) | section 3.3 | none |
| CG.3 export-side `cap_prose` (`dd191fa`) | Proposition text stays at 500 bytes | section 4.2 |
| CG.4b provenance test (`0a23e66`) | `external` is exported through the ORIGIN cell | section 4.3 |
| C0.7 (`457eba5`), CD.6a (`fc7cc14`) | `engram-export/Cargo.lock` gained the engine's new crates (lz4_flex, blake3, toml_edit, glia-projection-text) and lost three unused parser lines under glia-graph | none |

Every other exporter marker is unchanged in shape; the lines above are verbatim.

## 7. Not verified, and open

- **Not run:** a seed or diff apply in Engram itself. The diff counts above come from the exporter; what
  `apply_gmap_diff` does with them is read from `store.rs:1393-1480`, not executed.
- **Not measured:** glia's own repo, and CJ.2 on Kina's real tree (its `specs/` symlink, section 4.2).
  CG.2a's report gives +2,545 `test_fixture` nodes on glia; CJ.1a-CJ.1c's measured removals there are in
  section 4.1.
- **Closed since the first revision:** the hint-pass cross-pairing (CK.2, section 3.2) and the build
  stamp (CK.3, section 6).
- **Open, Engram's call:** whether Engram still wants an engram-core v7 producer field, or whether
  `<out>.meta.json` (bound to its gmap by `gmap_digest`) is the contract. glia needs nothing either way.
- **The 0.5.2 bet** "Engram PPR memory" (glia `dev-notes/research-0.5.2/R2-engram.md`) waits for 0.5.1
  to ship; nothing in 0.5.1 depends on it.
