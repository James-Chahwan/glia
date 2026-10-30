# Edge precision, 0.5.4 (FD.2 - FD.3d, report FD.3c)

How often is an edge glia writes true? This is the audit's finding and 0.5.5's precision
baseline (seed §F item 12). Numbers come from `bench/edge-audit/report.json` /
`report.md`, written by `bash scripts/edge-audit.sh report` from the committed sample
(`bench/edge-audit/samples/audit.jsonl`), verdicts (`verdicts/structure.jsonl`,
`verdicts/mechanism.jsonl`), the blind re-judge (`verdicts/rejudge.jsonl`) and the
adjudication (`adjudication.jsonl`). `report --check` fails when the committed report is not
derived from them. Every repo below is public. No private repository was sampled.

## Method

- **Panel.** Five trees, each built by `generate_one`:

  | label | source | rev |
  |---|---|---|
  | glia | this repo (fixture trees `bench/`, `dev-notes/`, `tests/`, `engine/tests/data/` skipped) | 0df1bbbb89b7276e298b301d6c29bdf628d39d53 |
  | shell | github.com/pop-os/shell (TypeScript) | 9616931a2f8da33ff60d6332cbb1e4036c9ae2b3 |
  | websocket | github.com/gorilla/websocket (Go) | dcea2f088ce10b1b0722c4eb995a4e145b5e9047 |
  | samples | github.com/flutter/samples (Dart, C, Kotlin, ..) | 291ba126bfc1279d074b91e61ad44919768ebe7f |
  | microservices-demo | github.com/GoogleCloudPlatform/microservices-demo (11 services, 5 languages) | 38e7348eb289eb5b87c0c6e8cb19ced0449dc389 |

- **Strata.** (edge category, evidence tier) as `glia why` prints it. 26 strata hold
  sampled edges. 30 edges are drawn per stratum (10 for DEFINES / CONTAINS / HAS_ATTRIBUTE),
  split across repos by size, smallest xxh64 keys first. A stratum of at most n edges is
  censused. That gives 503 samples: 299 structural (FD.3a, FD.3d) and 204 mechanism /
  cross-stack (FD.3b).
- **Judging.** Each sample is judged against source under `RUBRIC.md` as `correct`, `wrong`
  or `unsure`. The judges were FD.3a, FD.3d and FD.3b. 0 unsure, 65 wrong.
- **Estimator.** Per stratum, p = correct / (correct + wrong) with a Wilson 95% interval. Per
  category and per tier, p is stratified with weights N_h / N and the finite-population
  correction. Floors are fact 0.95, derived 0.90 and heuristic 0.70. One caveat: the
  stratified variance is zero for a stratum with p = 0 or 1, so a category whose strata are
  all at 1.000 prints a [1.000, 1.000] interval. For those, read the stratum's Wilson
  interval instead (e.g. CALLS|fact [0.893, 1.000]).

## Agreement (the blind re-judge)

A second judge (FD.3c) re-judged a fixed subset without the first verdicts. The subset rule
is applied per family, where a family is a first-verdict file. It takes the larger of two
sets: the ids ending in hex `0`, or the first 10% of ids, at least 20. In both families the
first-10% set was larger: structure 30 of 299 (24 end in `0`) and mechanism 21 of 204 (17
end in `0`). Cohen's kappa is computed over the three verdicts, with `unsure` its own
category. `edge_audit report` computes it, and its unit tests check kappa on identical
verdicts (1.0) and on the textbook 2x2 table (0.4).

| family | re-judged | agree | raw agreement | expected agreement | kappa |
|---|---:|---:|---:|---:|---:|
| mechanism | 21 | 21 | 1.000 | 0.556 | 1.000 |
| structure | 30 | 30 | 1.000 | 1.000 | undefined |
| overall | 51 | 51 | 1.000 | 0.763 | 1.000 |

There were 0 disagreements, so `adjudication.jsonl` is empty and no verdict was replaced. No
family is LOW AGREEMENT (kappa < 0.6), so no precision number below is provisional on
agreement grounds. Read the agreement with these limits:

- **Structure kappa is undefined.** Both judges called all 30 structure ids `correct`. The
  first 30 structure ids hold none of that family's 14 wrong verdicts, so the expected
  agreement is 1 and kappa is undefined. The structure re-judge confirms 30 correct
  verdicts. It says nothing about agreement on a `wrong` one.
- **One judging procedure, two passes.** Both judges are the same model family running
  `RUBRIC.md`. Kappa measures whether that procedure is consistent, not whether independent
  people agree.
- **The re-judge was not fully blind.** Before judging, the re-judge read this packet's
  handoff text. That text names the mechanisms of the first judges' wrong verdicts (the
  pubspec `environment:` block, common-word doclinks, kind-blind `tail_unique`). The
  re-judge also saw, by accident, the first verdicts of 6 of the 51 ids: the three smallest
  ids of each verdict file, seen while checking the file format. Without those 6, agreement
  is 45 of 45 and mechanism kappa stays 1.000.
- **One verdict matched for a different reason.** On `0090f0f3775c3a3c` (samples
  `web/simplistic_editor/README.md`, section "Replacements Layer", DOCUMENTS ->
  `BasicTextField`) both judges said `wrong`, but they gave different reasons. The first
  reason says the tie bound "the copy in the separate top-level simplistic_editor/ sample".
  At source, `web/simplistic_editor` is a symlink to `../simplistic_editor`, so the two are
  one file. The re-judge called the edge wrong because the section mentions the widget only
  in passing (a wrapper wraps it). The verdict stands. The mechanism is the symlink
  double-walk (cluster D3 below).

0.5.5 should re-judge stratified by first verdict (every `wrong` plus a matched number of
`correct`), so that the structure family's agreement on wrong verdicts gets measured.

## Precision

Per tier (stratified):

| tier | N | judged | p | 95% CI |
|---|---:|---:|---:|---|
| fact | 165636 | 324 | 0.998 | [0.995, 1.000] |
| derived | 1644 | 128 | 0.599 | [0.497, 0.700] |
| heuristic | 181 | 51 | 0.912 | [0.826, 0.997] |

The derived tier is low because DOCUMENTS|derived holds 893 of its 1644 edges at p 0.387.

Per category (stratified, the strata of one category pooled):

| category | N | judged | p | 95% CI |
|---|---:|---:|---:|---|
| ACCESSES_DATA | 15 | 15 | 0.467 | census |
| CALLS | 96729 | 32 | 1.000 | stratum [0.893, 1.000] |
| CONTAINS | 397 | 10 | 1.000 | stratum [0.723, 1.000] |
| DEFINES | 49393 | 11 | 1.000 | stratum [0.741, 1.000] |
| DEFINES_CONFIG | 239 | 30 | 0.667 | [0.509, 0.824] |
| DEPENDS_ON | 1316 | 30 | 0.933 | [0.845, 1.000] |
| DOCUMENTS | 893 | 31 | 0.387 | [0.219, 0.556] |
| GRPC_CALLS | 18 | 18 | 1.000 | census |
| HANDLED_BY | 67 | 47 | 0.876 | [0.813, 0.939] |
| HAS_ATTRIBUTE | 2032 | 11 | 1.000 | stratum [0.741, 1.000] |
| IMPLEMENTS | 705 | 71 | 0.842 | [0.743, 0.941] |
| IMPORTS | 5784 | 61 | 0.966 | [0.906, 1.000] |
| INHERITS_FROM | 204 | 38 | 1.000 | strata [0.886, 1.000] / census |
| INJECTS | 2 | 2 | 1.000 | census |
| READS_CONFIG | 162 | 30 | 0.767 | [0.630, 0.903] |
| RETURNS_TYPE | 1 | 1 | 1.000 | census |
| SCHEDULES | 1 | 1 | 1.000 | census |
| TESTS | 226 | 31 | 0.934 | [0.851, 1.000] |
| USES | 9275 | 31 | 1.000 | stratum [0.890, 1.000] |
| WS_CONNECTS | 2 | 2 | 1.000 | census |

Per repo (reported, not gated): glia 0.994 [0.990, 0.997] (204 judged), microservices-demo
0.975 [0.960, 0.990] (146), samples 0.996 [0.995, 0.996] (103), shell 0.999 (18) and
websocket 0.989 [0.976, 1.000] (32). The cross-service pairing edges glia is distinctive for
are all censuses and all correct: GRPC_CALLS 18/18, HANDLED_BY|derived 17/17 and WS_CONNECTS
2/2.

## Under floor: the strata, their mechanisms, the follow-ups

`report.json`'s `under_floor` lists 8 strata. Each is listed below with its `wrong` verdicts
grouped by mechanism, and each group has a proposed follow-up packet (title, files, fixture
idea) or is `covered by FD.<n>`. Under the follow-up rule (seed §4), the session specs these
before the next wave. Each one is gated by a public fixture built from a `wrong` sample.

### `ACCESSES_DATA|fact`: p 0.467, census of 15, 8 wrong

- **A1. Data-source needles inside non-access text (5).** `extractor:data_sources` matches
  `android.database.sqlite` names inside JNI type-descriptor strings of jnigen-generated
  bindings and C glue (4, samples). It also matches `Redis` inside the word `Redistribution`
  in a licence string (1, samples).
  Follow-up: **"data_sources needle precision: a data-source name inside a JNI type
  descriptor or a generated binding is no access, and a needle matches a whole word"**.
  Files: `parsers/code/extractors/src/data_sources.rs` (reading `generated.rs`'s
  classifier). Fixture: a jnigen-style C file and a Dart binding that hold
  `Landroid/database/sqlite/SQLiteDatabase;` descriptors, plus a page holding a licence
  string with "Redistribution". Forbid ACCESSES_DATA from both, and expect one from a real
  `openDatabase(..)` call.
- **A2. SQL / Cypher text in test literals (3).** `extractor:data_entities` reads a
  `CREATE TABLE orders` that a test writes to a temp tree, and two GQL strings passed to
  glia's own in-memory `query_gql` (one asserting an error). This is a residual of FD.4b,
  which landed before the audit rev: there a SQL literal counts when it is a call argument,
  and these literals are call arguments.
  Follow-up: **"data_entities: a statement literal counts only as the argument of a
  database client call, never of a file write or an in-process graph query API"**. Files:
  `parsers/code/extractors/src/data_entities.rs`. Fixture: a Rust test that writes
  `CREATE TABLE orders` with `std::fs::write`, and a Python test that passes `MATCH (n:X)` to
  a `query_gql` method. Forbid both. Expect a `conn.execute("SELECT .. FROM orders")` row.

### `DEFINES_CONFIG|fact`: p 0.667, 30 judged, 10 wrong

- **C1. pubspec `environment:` read as env vars (10).** `extractor:yaml` reads a
  `pubspec.yaml` `environment:` block (`sdk:`, `flutter:` version constraints) as
  `config:env:sdk` / `config:env:flutter` (samples, all 10).
  Follow-up: **"yaml env: a pubspec.yaml `environment:` block is SDK constraints, never env
  vars"**. Files: `parsers/code/extractors/src/config.rs` (`extract_yaml_env_defs` /
  `scan_compose_environment`) and, if the gate belongs in routing, `engine/src/route.rs`.
  Fixture: a Flutter pubspec with `environment: {sdk, flutter}` beside a docker-compose
  `environment:` list. Forbid `config:env:sdk` / `config:env:flutter`. Expect the compose
  keys.

### `DEPENDS_ON|fact`: p 0.933, 30 judged, 2 wrong

- **M1. Cargo target tables read as dependencies (2).** `extractor:manifest` reads the
  `name =` key of a Cargo `[[test]]` / `[[bench]]` table as a dependency named `name` (glia's
  own manifests).
  Follow-up: **"Cargo manifest: `[[test]]` / `[[bench]]` / `[[bin]]` / `[[example]]` tables
  declare targets, not dependencies"**. Files:
  `parsers/code/extractors/src/packages.rs` (`scan_cargo_toml`). Fixture: a Cargo.toml with
  `[dependencies] serde = ".."` and `[[test]] name = "smoke"`. Expect `package:cargo:serde`.
  Forbid `package:cargo:name` and `package:cargo:smoke`.

### `DOCUMENTS|derived`: p 0.387, 31 judged, 19 wrong (15 `ambiguous`, 4 `unique`)

- **D1. Common words, command and category names bound as symbols (14 ambiguous, 4
  unique).** `pass:doclink` binds a dictionary or CLI word (`graph` x2, `json`, `error`,
  `method`, `route`, `find`, `run`, `embed`, `location`, `impact`, `update`, `finding`), the
  edge category name `CALLS` (x2), or a placeholder (`PROJECT_ID` in a command line) to a
  same-named symbol. The bound symbols are mostly a test helper, a test-local const, a
  substrate-gap fixture, the cohort bench harness or a bench/latent model. Two more bind a
  real name to the wrong one of two same-named symbols: `communities` (engine answer ->
  activation algo) and `model_shop` (the test directory -> a CLI test method). The `unique`
  rule fails the same way when the common word happens to name one symbol.
  Follow-up: **"doclink precision: a bare mention binds only an identifier-shaped token
  (a qualified path, a `_` / case-change identifier, or a call `x(`), never a lone dictionary
  word, an edge-category, node-kind or CLI command name, or a placeholder; a candidate in a
  test, fixture or bench file binds only from a doc under that tree; an ambiguous tie prefers
  the doc's own project root before the least qname"**. Files: `engine/src/passes.rs`
  (`link_doc_sections` and its pick). It moves DE.1's least-qname tie rule, a declared
  content break. Fixture: a README whose sections say "graph", "run", "json" and "CALLS",
  beside a test helper `fn graph()`, a bench `def json()` and a test const `CALLS`. Forbid
  those DOCUMENTS edges. Expect DOCUMENTS to a backticked `ServiceRegistry` class the README
  describes.
- **D2. An ambiguous tie across sibling copies (1).** On samples, `location` (a Firestore
  field) binds a generated `PhotoBuilder.location` getter of another sample. D1's project-root
  preference covers this. The fixture gains two sample roots, each holding a `location`
  member.
- **D3. A directory symlink walked twice (1, and every samples stratum's N).** The walk
  follows directory symlinks. flutter/samples has nine of them, `web/<x> -> ../<x>`
  (`web/web_dashboard -> ../experimental/web_dashboard`), so each mirrored sample is indexed
  twice, under `web::<x>::..` and `<x>::..`. That duplication made the `BasicTextField` tie
  ambiguous and inflates the samples strata's N.
  Follow-up: **"walk: an in-repo directory symlink is walked once, at its target"**. Files:
  `engine/src/walk.rs` (`plan_walk`, and the read that follows symlinks). Fixture: a tree with
  `web/app -> ../app`. Forbid every `web::app::` node. Expect `app::` nodes once.

### `HANDLED_BY|fact`: p 0.833, 30 judged, 5 wrong

- **H1. The registration's enclosing function bound instead of the handler (4).**
  `extractor:rpc_needles` rule `anchor` binds the function around the gRPC registration:
  Node `loadAllProtos` (x2, PaymentService and Health), the Python server bootstrap
  `start()` (EmailService) and Go `run()` (productcatalog). The implementation the
  registration names is the handler.
- **H2. A health method no server registers (1).** shippingservice registers grpc's
  `health.NewServer()` for Health. The app's own `server.Check` is never registered for
  Health, yet it is bound.
  Follow-up (H1 + H2): **"rpc_needles: a gRPC registration binds HANDLED_BY to the
  implementation it names (`server.addService(svc, impl)`, `add_XServicer_to_server(impl,
  server)`, `RegisterXServer(s, impl)`), method by method, never to the enclosing function;
  a library servicer (`health.NewServer()`) mints no in-repo handler"**. Files:
  `engine/src/build/rpc_needles.rs`. Fixture: three tiny servers in the microservices-demo
  shape (Node `addService` inside a loader function, Python `add_..._to_server` inside
  `start()`, Go `RegisterXServer` plus `healthpb.RegisterHealthServer(s, health.NewServer())`).
  Expect HANDLED_BY to the impl methods. Forbid HANDLED_BY to `loadAllProtos` / `start` /
  `run` / the app `Check`.

### `IMPLEMENTS|derived`: p 0.800, 30 judged, 6 wrong

- **G1. Go implicit IMPLEMENTS across Go modules (6).** `graph:iface` rules `same_name` (4)
  and `method_signature` (2) pair a type with an interface of another Go module. In
  microservices-demo each service has its own go.mod and its own generated genproto copy, so
  the parameter types are distinct packages that share text.
  Follow-up: **"Go implicit IMPLEMENTS stays inside one Go module: graph:iface pairs a type
  and an interface only under the same nearest go.mod, or when the signatures' named types
  resolve to one package"**. Files: `graph/src/build.rs` (the iface pass, `GoPackages`), plus
  `engine/src/build/lang_build.rs` if the go.mod roots must reach the Go group. Fixture: two
  Go modules, each with a `genproto` package declaring `CartServiceClient` and a client type
  implementing it. Expect IMPLEMENTS within each module. Forbid it across.

### `IMPLEMENTS|fact`: p 0.933, 30 judged, 2 wrong

- **R1. A non-local self type credited to the file MODULE (2).** The Rust parser credits an
  impl whose self type is no local node to the file's MODULE as IMPLEMENTS. The two cases
  are a foreign type, `impl PathGraph for MergedGraph` in engine/src/check.rs, and a blanket
  `impl<T: HostScoped> HostScoped for &T` in graph/src/resolvers/host.rs.
  **Covered by FD.11** (not yet landed). Its step 3 leaves reference, tuple and blanket self
  types unbound, and its path rules bind a cross-file self type. FD.11's fixture should also
  carry a forbid row for MODULE -IMPLEMENTS-> on both a blanket impl and an impl for another
  crate's type. Files: `parsers/code/rust/src/lib.rs`, `graph/src/rust_paths.rs`.

### `READS_CONFIG|fact`: p 0.767, 30 judged, 7 wrong

- **E1. An env write read as a read (5).** `extractor:config` treats
  `os.environ["X"] = ..` (set for a child git process in glia's Python tests) as a
  READS_CONFIG.
- **E2. Test path literals read as Vault secrets (2).** `extractor:secrets_flags` reads a
  test's fixture path literal (a file the test writes) as `config:secret:vault/<path>`.
  Follow-up (E1 + E2): **"config reads: an `os.environ[X] = v` assignment (and `del` /
  `.pop`) is no READS_CONFIG, and a `vault/<path>` secret needs a Vault client call (hvac,
  the Vault API)"**. Files: `parsers/code/extractors/src/config.rs` (the `os.environ[` needle)
  and `parsers/code/extractors/src/secrets_flags.rs`. Fixture: a Python test that sets
  `os.environ["GIT_DIR"] = d` and writes `secret/data/api` to a temp file, beside an
  `hvac.Client().secrets.kv.read_secret_version(path="api")` reader. Forbid READS_CONFIG
  from the test. Expect the hvac row.

## Wrong verdicts in strata at or above their floor

These do not trigger the follow-up rule. They are listed so that 0.5.5 can measure whether
they grow.

- **I1. `IMPORTS|heuristic` (p 0.900, 3 wrong): `tail_unique` is kind-blind and
  stdlib-blind.** The Go import `"encoding/json"` binds the repo's `json.go`. Java
  `java.util.ArrayList` binds an EVENT_EMITTER node. C# `using static
  Grpc.Health.V1.Health` binds the in-repo GRPC_SERVER. FD.13 covers PHP's unowned `use`
  only. Candidate: **"IMPORTS tail_unique binds only a MODULE / PACKAGE / type node, never a
  Go stdlib path, a `java.` / `javax.` import or a namespace the repo does not declare"**
  (`graph/src/imports.rs`).
- **I2. `IMPORTS|fact` (p 0.968, 1 wrong): `rust_paths` misattributes an inline module's
  `use`.** A `use super::..` inside an inline `pub mod` is credited to the file's MODULE.
  Candidate: **"rust_paths: a `use` inside an inline `mod` is that module's import"**
  (`graph/src/rust_paths.rs`, `parsers/code/rust/src/lib.rs`).
- **T1. `TESTS|derived` (p 0.933, 2 wrong): `name_match` pairs a stem across packages.**
  Go's root `server_test.go` pairs `examples/echo/server.go`. A flutter drive host
  (`*_integration_test.dart` calling `integrationDriver()`) pairs its own integration suite.
  Candidate: **"TESTS name_match within one package (Go: one directory; Dart: one pubspec
  root), and a flutter drive host pairs nothing"** (`engine/src/passes.rs`). FD.19 covers
  declared `[walk] tests` stems, not this.

## What 0.5.5 re-runs

1. `bash scripts/edge-audit.sh sample` with the self row moved to the 0.5.5 rev. The
   rank-gate and cohort rows stay pinned, so their ids keep their verdicts. Then judge only
   the new ids.
2. Each follow-up above lands with its fixture. The strata it touches are re-judged on the
   re-drawn sample. An under-floor stratum leaves the list only when its p reaches the floor
   on fresh ids.
3. The agreement check, re-judged stratified by first verdict (see Agreement), so that the
   structure family's agreement on `wrong` verdicts is measured. Also count the accidental
   unblinding (0 ids is the target).
4. The category CI: replace the plug-in stratified variance, which is zero at p = 0 or 1,
   with a stratified Wilson or a Bayesian interval, so that an all-correct category stops
   printing [1.000, 1.000].
