//! glia-code-domain — shared code-domain types for every language parser.
//!
//! Extracted from `glia-parser-python` at v0.4.3b so Go + TypeScript
//! parsers can share the constants + structural types without a weird
//! inter-parser dependency. All code-language parsers produce a `FileParse`,
//! and `glia-graph` consumes the uniform shape.
//!
//! Registry-locked u32 values live here as the single source of truth.
//! See `memory/reference_code_domain_registries.md` for the semantic notes.

use std::collections::HashMap;

use glia_core::{Cell, CellPayload, CellTypeId, Edge, EdgeCategoryId, Node, NodeId, NodeKindId};

/// Graph-type tag for any code-language graph. First arg to `NodeId::from_parts`.
pub const GRAPH_TYPE: &str = "code";

/// Which directories the repo walk descends into and which collapse to a single
/// REGION anchor. Shared so the builder's walk and `store::is_gmap_stale` cannot
/// drift apart. (A8.1)
pub mod walk_gating;

/// Manifest-rooted sub-projects detected during that same walk: which manifest
/// roots a directory, its ecosystem and its label. (A8.4)
pub mod project_roots;

// 0.5.0 leap module slots (L0.1): declared here so each owner edits only its
// own file. Doc-only until the owning packet fills it.

/// ORM table-cell writer / reader (`table_cell` / `table_of`). (A13.1)
pub mod data_entity;

/// Edge evidence: the EVIDENCE edge-cell payload and its helpers. (LC.3a)
pub mod evidence;

/// Sidecar / overlay input records (cells.jsonl, vectors.jsonl, constraints). (LF.1a)
pub mod external_inputs;

/// The `.glia/overlay.toml` schema and loader. (LF.2a)
pub mod glia_config;

/// The code domain profile tables (`CODE_TABLES`). (LD.14a)
pub mod profile;

/// Git-history / test-report snapshot records and `data_hash`. (LF.5a)
pub mod snapshots;

// 0.5.1 slot (C0.1): declared here so its owner edits only its own file.

/// CODE cells stored as spans into the source: the CodeSpan codec. (CD.7c)
pub mod code_span;

// ============================================================================
// Node kinds
// ============================================================================

pub mod node_kind {
    use super::NodeKindId;

    // v0.4.1 — universal entity kinds
    pub const MODULE: NodeKindId = NodeKindId(1);
    pub const CLASS: NodeKindId = NodeKindId(2);
    pub const FUNCTION: NodeKindId = NodeKindId(3);
    pub const METHOD: NodeKindId = NodeKindId(4);

    // v0.4.3b — framework / type-system additions
    pub const ROUTE: NodeKindId = NodeKindId(5);
    pub const PACKAGE: NodeKindId = NodeKindId(6);
    pub const INTERFACE: NodeKindId = NodeKindId(7);
    pub const STRUCT: NodeKindId = NodeKindId(8);
    pub const ENDPOINT: NodeKindId = NodeKindId(9);
    pub const ENUM: NodeKindId = NodeKindId(10);

    // v0.4.10 — cross-stack entity kinds
    pub const GRPC_SERVICE: NodeKindId = NodeKindId(11);
    pub const GRPC_CLIENT: NodeKindId = NodeKindId(12);
    pub const QUEUE_CONSUMER: NodeKindId = NodeKindId(13);
    pub const QUEUE_PRODUCER: NodeKindId = NodeKindId(14);
    pub const GRAPHQL_RESOLVER: NodeKindId = NodeKindId(15);
    pub const GRAPHQL_OPERATION: NodeKindId = NodeKindId(16);
    pub const WS_HANDLER: NodeKindId = NodeKindId(17);
    pub const WS_CLIENT: NodeKindId = NodeKindId(18);
    pub const EVENT_HANDLER: NodeKindId = NodeKindId(19);
    pub const EVENT_EMITTER: NodeKindId = NodeKindId(20);
    pub const CLI_COMMAND: NodeKindId = NodeKindId(21);
    pub const CLI_INVOCATION: NodeKindId = NodeKindId(22);

    // v0.4.11a — data source entity kinds (D1)
    pub const DATABASE: NodeKindId = NodeKindId(23);
    pub const CACHE: NodeKindId = NodeKindId(24);
    pub const BLOB_STORE: NodeKindId = NodeKindId(25);
    pub const SEARCH_INDEX: NodeKindId = NodeKindId(26);
    pub const EMAIL_SERVICE: NodeKindId = NodeKindId(27);

    // v0.4.11a — frontend framework entity kinds (F-react / F-angular / F-vue)
    pub const COMPONENT: NodeKindId = NodeKindId(28);
    pub const HOOK: NodeKindId = NodeKindId(29);
    pub const SERVICE: NodeKindId = NodeKindId(30);
    pub const DIRECTIVE: NodeKindId = NodeKindId(31);
    pub const PIPE: NodeKindId = NodeKindId(32);
    pub const GUARD: NodeKindId = NodeKindId(33);
    pub const COMPOSABLE: NodeKindId = NodeKindId(34);

    // v0.4.13 — attribute entity kind (A+ composition cells)
    /// A named attribute on a class — qname `Module::Class::attr_name`. Emitted
    /// by parsers from `self.x = ...` assignments and class-level attribute
    /// declarations. Enables BFS path synthesis to walk `Class → attr` without
    /// re-parsing source at activation time.
    pub const ATTRIBUTE: NodeKindId = NodeKindId(35);

    // v0.4.x — DB resolver entity kind. Single node kind covers SQL Tables,
    // NoSQL Collections, and Graph-DB NodeLabels via the qname prefix:
    //   `data_entity:sql:<name>`     — Postgres / MySQL / SQLite tables
    //   `data_entity:nosql:<name>`   — MongoDB / DynamoDB / Firestore collections
    //   `data_entity:graph:<name>`   — Neo4j / ArangoDB labels
    /// A named persistence entity (table, collection, graph-label) that
    /// `DbResolver` joins across services to surface shared-data dependencies.
    pub const DATA_ENTITY: NodeKindId = NodeKindId(36);

    // v0.4.x — Cron resolver entity kind. One node per scheduled invocation,
    // qname `cron:<schedule>` so two services running at the same cadence
    // pair under `CronResolver`. Source detail (workflow name, target
    // command/handler) lives on a cell payload.
    pub const CRON_JOB: NodeKindId = NodeKindId(37);

    // v0.4.x — Config resolver entity kind. One node per env-var name across
    // the entire merged graph (qname `config:env:<NAME>`); flavor segment
    // reserves room for future config-file / secrets-manager flavors.
    pub const CONFIG_KEY: NodeKindId = NodeKindId(38);

    // v0.4.x — IaC resolver entity kind. One node per declared infra resource
    // (k8s manifest, docker-compose service, Dockerfile-built image), qname
    // `infra:<kind>:<name>` so a Deployment named `api` and a Service named
    // `api` each get their own node.
    pub const INFRA_RESOURCE: NodeKindId = NodeKindId(39);

    // v0.4.x — Package-deps resolver entity kind. One node per declared
    // ecosystem package, qname `package:<ecosystem>:<name>` (e.g.
    // `package:npm:react`, `package:cargo:tokio`, `package:gomod:github.com/gin-gonic/gin`).
    // PackageResolver pairs across repos by full qname.
    pub const PACKAGE_DEP: NodeKindId = NodeKindId(40);

    // v0.4.13 — collapsed-region anchor. One node stands in for a whole
    // build-output / vendored / gitignored directory (e.g. `www/`,
    // `node_modules/`) instead of emitting a node per file inside it. qname
    // `region:<repo-relative-path>`; provenance + file count live in the ORIGIN
    // cell. Preserves the spatial map without the per-file flood. (glia-v2 G1/G10)
    pub const REGION: NodeKindId = NodeKindId(41);

    // v0.4.14 — a prose section from an external `.md` doc (README, ARCHITECTURE,
    // docs/). qname `docs::<dirs ::-joined>::<file_stem>::<section_slug>` (LB.12,
    // `dir_stem_qname`; a root README is `docs::README::<slug>`); the CODE cell holds the
    // chunk text. The engram exporter maps this kind to `Content::Proposition`
    // (not Symbol) with provenance `documentation`. (glia-v5 G18)
    pub const DOC_SECTION: NodeKindId = NodeKindId(42);

    // v0.4.14 — a top-level state variable / constant (Solidity public state, Go
    // package var, Rust static/const, TS exported const, etc.). Emitted as a
    // `Content::Symbol` like a function; the distinct kind lets consumers rank
    // it differently if useful. (glia-v5 G19)
    pub const STATE_VAR: NodeKindId = NodeKindId(43);

    // Tier-4 — a documentation container (a Confluence space, a Notion database,
    // a wiki). `DOC_SECTION`s from an external source CONTAINS-nest under it;
    // repo `.md` docs stay flat (no space). Carries source/url/version provenance.
    pub const DOC_SPACE: NodeKindId = NodeKindId(44);

    // ------------------------------------------------------------------
    // RESERVED ids — central locked-id allocation (packet W0.3)
    //
    // Node-kind, edge-category and cell-type ids are allocated HERE and
    // NOWHERE ELSE. A packet that needs a new id takes the next free one in
    // this file, in its own commit, together with its mandatory `ALL` row —
    // it never picks an id inside a parser, resolver or extractor. Ids are
    // locked once allocated: they are baked into `.gmap` node ids and into
    // every consumer's decode table, so they are never renumbered or reused.
    //
    // The constants below are RESERVATIONS. The id and its `ALL` row exist so
    // that `name()` / pyo3 `kind_names()` decode correctly the moment the
    // first emitter lands; nothing emits them yet. Owning packet per id:
    //   45 PROJECT        — A8.4           (emitted since A8.5)
    //   46 MESSAGE_TYPE   — A10.5 / A12.1  (emitted since A10.5)
    //   47 GRPC_SERVER    — A5.3           (emitted since A5.3)
    //   48 RPC_PROCEDURE  — A10.9
    //   49 RPC_CALL       — A10.9
    // ------------------------------------------------------------------

    /// RESERVED (A8.4) — a build/workspace project unit: an MSBuild `.csproj`,
    /// a Gradle subproject, a Cargo workspace member. The anchor a repo's
    /// modules hang off when one repo holds several independent projects.
    ///
    /// A8.4 DETECTS the roots during the walk ([`crate::project_roots`]), and
    /// since A8.5 `engine::walk::build_project_graph` emits one node per root.
    /// Each node is an edge-less anchor like REGION. Its qname is
    /// `project:<rel_path>`, or `project:.` for the repo root. Its nav name is
    /// the manifest label. Its ORIGIN cell carries the `project_root` payload
    /// (see [`crate::cell_type::ORIGIN`]).
    pub const PROJECT: NodeKindId = NodeKindId(45);

    /// (A10.5 / A12.1) — a declared message / payload schema type
    /// (protobuf `message`, Avro record, Thrift struct) that RPC procedures
    /// and queue payloads reference by name.
    ///
    /// Emitted since A10.5 by `extractors::schemas` for every protobuf
    /// `message` and `enum`. qname `message:<flavor>:<qualified name>`,
    /// mirroring `DATA_ENTITY`'s flavor scheme: flavor `proto` today
    /// (`avro` / `jsonschema` reserved), qualified name as protobuf writes it
    /// (`message:proto:user.v1.Outer.Inner`). The nav name is the bare
    /// declared name (`Inner`).
    pub const MESSAGE_TYPE: NodeKindId = NodeKindId(46);

    /// (A5.3) — the code that implements a proto service: a class that extends
    /// or embeds the generated base (`: Greeter.GreeterBase`,
    /// `pb.UnimplementedGreeterServer`, `GreeterServicer`, …) or the call that
    /// registers one into a server (`RegisterGreeterServer(`,
    /// `add_GreeterServicer_to_server(`). Distinct from `GRPC_SERVICE` (the
    /// declared service) and `GRPC_CLIENT` (the calling stub).
    ///
    /// Emitted since A5.3 by `extractors::grpc::extract_grpc_server_nodes`,
    /// one per (service, file): qname `grpc_server:<Service>`, nav name the bare
    /// service name. `GrpcStackResolver` pairs it back to its service as
    /// `grpc:<pkg>.<Service> --HANDLED_BY--> grpc_server:<Service>`, and the
    /// marker is `HANDLED_BY` each method that implements one of the rpcs.
    pub const GRPC_SERVER: NodeKindId = NodeKindId(47);

    /// RESERVED (A10.9) — a single declared remote procedure within a service
    /// (a proto `rpc`, a tRPC procedure, a JSON-RPC method).
    pub const RPC_PROCEDURE: NodeKindId = NodeKindId(48);

    /// RESERVED (A10.9) — a call site that invokes a remote procedure, the
    /// caller-side counterpart of `RPC_PROCEDURE`.
    pub const RPC_CALL: NodeKindId = NodeKindId(49);

    /// Canonical id→name for every node kind. Single source of truth for decode
    /// tables (pyo3 `kind_names`), the CLI, and projection-text's display
    /// fallback — so no consumer reimplements a table that goes stale when a
    /// kind is added. Keep in lockstep with the constants above.
    pub const ALL: &[(NodeKindId, &str)] = &[
        (MODULE, "MODULE"),
        (CLASS, "CLASS"),
        (FUNCTION, "FUNCTION"),
        (METHOD, "METHOD"),
        (ROUTE, "ROUTE"),
        (PACKAGE, "PACKAGE"),
        (INTERFACE, "INTERFACE"),
        (STRUCT, "STRUCT"),
        (ENDPOINT, "ENDPOINT"),
        (ENUM, "ENUM"),
        (GRPC_SERVICE, "GRPC_SERVICE"),
        (GRPC_CLIENT, "GRPC_CLIENT"),
        (QUEUE_CONSUMER, "QUEUE_CONSUMER"),
        (QUEUE_PRODUCER, "QUEUE_PRODUCER"),
        (GRAPHQL_RESOLVER, "GRAPHQL_RESOLVER"),
        (GRAPHQL_OPERATION, "GRAPHQL_OPERATION"),
        (WS_HANDLER, "WS_HANDLER"),
        (WS_CLIENT, "WS_CLIENT"),
        (EVENT_HANDLER, "EVENT_HANDLER"),
        (EVENT_EMITTER, "EVENT_EMITTER"),
        (CLI_COMMAND, "CLI_COMMAND"),
        (CLI_INVOCATION, "CLI_INVOCATION"),
        (DATABASE, "DATABASE"),
        (CACHE, "CACHE"),
        (BLOB_STORE, "BLOB_STORE"),
        (SEARCH_INDEX, "SEARCH_INDEX"),
        (EMAIL_SERVICE, "EMAIL_SERVICE"),
        (COMPONENT, "COMPONENT"),
        (HOOK, "HOOK"),
        (SERVICE, "SERVICE"),
        (DIRECTIVE, "DIRECTIVE"),
        (PIPE, "PIPE"),
        (GUARD, "GUARD"),
        (COMPOSABLE, "COMPOSABLE"),
        (ATTRIBUTE, "ATTRIBUTE"),
        (DATA_ENTITY, "DATA_ENTITY"),
        (CRON_JOB, "CRON_JOB"),
        (CONFIG_KEY, "CONFIG_KEY"),
        (INFRA_RESOURCE, "INFRA_RESOURCE"),
        (PACKAGE_DEP, "PACKAGE_DEP"),
        (REGION, "REGION"),
        (DOC_SECTION, "DOC_SECTION"),
        (STATE_VAR, "STATE_VAR"),
        (DOC_SPACE, "DOC_SPACE"),
        // Reserved ids (see the RESERVED block above) — no emitter yet, but
        // present so every decode table already labels them correctly.
        (PROJECT, "PROJECT"),
        (MESSAGE_TYPE, "MESSAGE_TYPE"),
        (GRPC_SERVER, "GRPC_SERVER"),
        (RPC_PROCEDURE, "RPC_PROCEDURE"),
        (RPC_CALL, "RPC_CALL"),
    ];

    /// Name for a node-kind id, or `"UNKNOWN"` if unregistered.
    pub fn name(k: NodeKindId) -> &'static str {
        ALL.iter().find(|(id, _)| *id == k).map(|(_, n)| *n).unwrap_or("UNKNOWN")
    }
}

// ============================================================================
// Edge categories
// ============================================================================

pub mod edge_category {
    use super::EdgeCategoryId;

    // v0.4.1
    pub const DEFINES: EdgeCategoryId = EdgeCategoryId(1);
    pub const CONTAINS: EdgeCategoryId = EdgeCategoryId(2);
    pub const IMPORTS: EdgeCategoryId = EdgeCategoryId(3);
    pub const CALLS: EdgeCategoryId = EdgeCategoryId(4);
    pub const USES: EdgeCategoryId = EdgeCategoryId(5);
    pub const DOCUMENTS: EdgeCategoryId = EdgeCategoryId(6);
    pub const TESTS: EdgeCategoryId = EdgeCategoryId(7);

    // v0.4.3b
    pub const INJECTS: EdgeCategoryId = EdgeCategoryId(8);

    // v0.4.4 — HTTP stack
    /// Route → handler function. Emitted when gin/chi/net-http route
    /// registration links a path to a handler identifier.
    pub const HANDLED_BY: EdgeCategoryId = EdgeCategoryId(9);
    /// Endpoint → Route cross-repo link. Emitted by `HttpStackResolver`
    /// when a frontend HTTP call matches a backend route by (method, path).
    pub const HTTP_CALLS: EdgeCategoryId = EdgeCategoryId(10);

    // v0.4.10 — cross-stack resolvers
    pub const GRPC_CALLS: EdgeCategoryId = EdgeCategoryId(11);
    pub const QUEUE_FLOWS: EdgeCategoryId = EdgeCategoryId(12);
    pub const GRAPHQL_CALLS: EdgeCategoryId = EdgeCategoryId(13);
    pub const WS_CONNECTS: EdgeCategoryId = EdgeCategoryId(14);
    pub const EVENT_FLOWS: EdgeCategoryId = EdgeCategoryId(15);
    pub const SHARES_SCHEMA: EdgeCategoryId = EdgeCategoryId(16);
    pub const CLI_INVOKES: EdgeCategoryId = EdgeCategoryId(17);

    // v0.4.11a — data access (D1). LE.4a: from the innermost FUNCTION /
    // METHOD whose body holds the statement (the MODULE when the statement is
    // at module scope, or for a declaration: ORM table, DDL, migration DSL,
    // and the data_sources provider buckets) to the data node. Extractor
    // re-homed edges carry an ACCESS_MODE edge cell (`read` / `write` /
    // `read_write`).
    pub const ACCESSES_DATA: EdgeCategoryId = EdgeCategoryId(18);

    // v0.4.x — DB resolver cross-service join. Emitted by `DbResolver` when
    // two services touch a `DATA_ENTITY` with the same (flavor, name).
    pub const SHARES_DATA_ENTITY: EdgeCategoryId = EdgeCategoryId(22);

    // v0.4.x — Cron resolver. `SCHEDULES` from a `CRON_JOB` to its target
    // (handler function / CLI command / image entrypoint). `SHARES_CRON_SCHEDULE`
    // pairs CRON_JOB nodes across repos when the full (schedule, target) match —
    // drift / accidental duplication signal.
    pub const SCHEDULES: EdgeCategoryId = EdgeCategoryId(23);
    pub const SHARES_CRON_SCHEDULE: EdgeCategoryId = EdgeCategoryId(24);

    // v0.4.x — Config resolver. `READS_CONFIG` from the innermost function /
    // method holding the read (the module at module scope; LE.4b) to a
    // `CONFIG_KEY` it dereferences (e.g. `os.environ['DB_URL']`).
    // `DEFINES_CONFIG` from a Dockerfile / .env / k8s manifest module to a
    // `CONFIG_KEY` it sets. `SHARES_CONFIG` pairs CONFIG_KEY nodes across
    // repos when the same key is touched by multiple services.
    pub const READS_CONFIG: EdgeCategoryId = EdgeCategoryId(25);
    pub const DEFINES_CONFIG: EdgeCategoryId = EdgeCategoryId(26);
    pub const SHARES_CONFIG: EdgeCategoryId = EdgeCategoryId(27);

    // v0.4.x — IaC resolver. `INFRA_REFERENCES` from one infra resource to
    // another inside the same merged graph (Deployment → Image, Service →
    // Deployment via selector). `SHARES_INFRA_REF` joins INFRA_RESOURCE nodes
    // across repos when the same name is referenced (image built by repo A
    // referenced by repo B's k8s manifest).
    pub const INFRA_REFERENCES: EdgeCategoryId = EdgeCategoryId(28);
    pub const SHARES_INFRA_REF: EdgeCategoryId = EdgeCategoryId(29);

    // v0.4.x — Package-deps resolver. Module (manifest file) → package node.
    // SHARES_DEPENDENCY pairs PACKAGE_DEP nodes across repos when multiple
    // services depend on the same package.
    pub const DEPENDS_ON: EdgeCategoryId = EdgeCategoryId(30);
    pub const SHARES_DEPENDENCY: EdgeCategoryId = EdgeCategoryId(31);

    // v0.4.13 — composition edges (A+ access-path synthesis)
    /// Class → attribute. Emitted when a class body assigns `self.x = ...` or
    /// declares a class-level attribute. Lets BFS walk `Class → attr_qname`
    /// without re-parsing source at activation time.
    pub const HAS_ATTRIBUTE: EdgeCategoryId = EdgeCategoryId(19);
    /// Class → superclass. Emitted from a class's base-class list (Python
    /// `class Foo(Bar):`, equivalent in other languages). Lets the synthesizer
    /// walk inheritance when an attribute is defined on a parent.
    pub const INHERITS_FROM: EdgeCategoryId = EdgeCategoryId(20);
    /// Function / property / method → return type class. Emitted when the
    /// parser can statically identify the returned class (explicit type
    /// annotation or a cheap `return self.<known-typed-attr>` pattern inside
    /// an `@property`). Enables A+ to compose `self.root.opts` by jumping
    /// `Field.root → Schema` via RETURNS_TYPE, then `Schema → opts` via
    /// HAS_ATTRIBUTE.
    pub const RETURNS_TYPE: EdgeCategoryId = EdgeCategoryId(21);
    /// Class/contract → interface it implements. Distinct from `INHERITS_FROM`
    /// (class extends class): TS/Java/C#/Dart `implements`, Rust `impl Trait for`,
    /// Solidity `is <Interface>`. Maps to `EdgeKind::Implements`. (glia-v5 G12.5)
    pub const IMPLEMENTS: EdgeCategoryId = EdgeCategoryId(32);

    // ------------------------------------------------------------------
    // RESERVED ids — central locked-id allocation (packet W0.3). Edge
    // categories are allocated here and nowhere else; see the matching block
    // in `node_kind` for the rule. Owning packet per id:
    //   33 SHARES_DATA_SOURCE — A13.3 (LANDED: DbResolver emits it)
    //   34 RPC_CALLS          — A10.10 (LANDED: RpcStackResolver emits it)
    // 35-36 allocated centrally by L0.1 for the 0.5.0 leap (the packet texts
    // proposed CO_CHANGES = 38; compacted so ALL stays 1..=len):
    //   35 NAVIGATES_TO       — LA.6a
    //   36 CO_CHANGES         — LF.5b
    // ------------------------------------------------------------------

    /// Cross-repo pairing: two nodes reach the same external data source
    /// (`data_source:redis` — the same database / cache / bucket / search /
    /// email provider). The cross-graph counterpart of intra-repo
    /// `ACCESSES_DATA`. Emitted by `DbResolver` over DATABASE / CACHE /
    /// BLOB_STORE / SEARCH_INDEX / EMAIL_SERVICE nodes, always at
    /// `Confidence::Weak` (the provider needles are substring matches).
    ///
    /// Deliberately NOT in `CODE_TABLES.carry_edges`: a shared Postgres is an
    /// operational fact, not a code dependency, and carrying it would fan every
    /// blast radius across every service in the stack. (A13.3)
    pub const SHARES_DATA_SOURCE: EdgeCategoryId = EdgeCategoryId(33);

    /// `RPC_CALL` → `RPC_PROCEDURE`: a client call site to the remote procedure
    /// it names. The transport-generic counterpart of `GRPC_CALLS`; tRPC today,
    /// Connect / Twirp when those land. Emitted by `RpcStackResolver` on an
    /// exact procedure-path match (`rpc_call:<path>` ↔ `rpc:<path>`) — no
    /// substring fallback. In `CODE_TABLES.carry_edges`. (A10.10)
    pub const RPC_CALLS: EdgeCategoryId = EdgeCategoryId(34);

    /// RESERVED (LA.6a) — a frontend navigation link (`routerLink`, `<Link to>`,
    /// `navigate()`, `router.push`, origin share links) or a route redirect,
    /// pointing at the navigation `ROUTE` it lands on (`page:<path>` once LB.4c
    /// lands). A dead link stays an unresolved ref of this category rather than
    /// a guessed edge. Emitters LA.6b-d; resolved by `graph::nav` (LA.6a);
    /// read by LA.6e and the Engram edge table (LG.11).
    pub const NAVIGATES_TO: EdgeCategoryId = EdgeCategoryId(35);

    /// RESERVED (LF.5b) — `MODULE` ↔ `MODULE`: two files that repeatedly change
    /// together in the git-history snapshot. HEURISTIC, always
    /// `Confidence::Weak`, activation weight 0, NOT in `CODE_TABLES.carry_edges`,
    /// symmetric in cross_links. Emitter LF.5b; read by LF.5c / LF.5d and the
    /// Engram edge table (LG.11); the overlay loader (LF.2a) rejects it by name.
    pub const CO_CHANGES: EdgeCategoryId = EdgeCategoryId(36);

    /// Canonical id→name for every edge category. Single source of truth for
    /// decode tables (pyo3 `category_names`) and the CLI. Keep in lockstep with
    /// the constants above.
    pub const ALL: &[(EdgeCategoryId, &str)] = &[
        (DEFINES, "DEFINES"),
        (CONTAINS, "CONTAINS"),
        (IMPORTS, "IMPORTS"),
        (CALLS, "CALLS"),
        (USES, "USES"),
        (DOCUMENTS, "DOCUMENTS"),
        (TESTS, "TESTS"),
        (INJECTS, "INJECTS"),
        (HANDLED_BY, "HANDLED_BY"),
        (HTTP_CALLS, "HTTP_CALLS"),
        (GRPC_CALLS, "GRPC_CALLS"),
        (QUEUE_FLOWS, "QUEUE_FLOWS"),
        (GRAPHQL_CALLS, "GRAPHQL_CALLS"),
        (WS_CONNECTS, "WS_CONNECTS"),
        (EVENT_FLOWS, "EVENT_FLOWS"),
        (SHARES_SCHEMA, "SHARES_SCHEMA"),
        (CLI_INVOKES, "CLI_INVOKES"),
        (ACCESSES_DATA, "ACCESSES_DATA"),
        (HAS_ATTRIBUTE, "HAS_ATTRIBUTE"),
        (INHERITS_FROM, "INHERITS_FROM"),
        (RETURNS_TYPE, "RETURNS_TYPE"),
        (SHARES_DATA_ENTITY, "SHARES_DATA_ENTITY"),
        (SCHEDULES, "SCHEDULES"),
        (SHARES_CRON_SCHEDULE, "SHARES_CRON_SCHEDULE"),
        (READS_CONFIG, "READS_CONFIG"),
        (DEFINES_CONFIG, "DEFINES_CONFIG"),
        (SHARES_CONFIG, "SHARES_CONFIG"),
        (INFRA_REFERENCES, "INFRA_REFERENCES"),
        (SHARES_INFRA_REF, "SHARES_INFRA_REF"),
        (DEPENDS_ON, "DEPENDS_ON"),
        (SHARES_DEPENDENCY, "SHARES_DEPENDENCY"),
        (IMPLEMENTS, "IMPLEMENTS"),
        // Centrally-allocated ids (see the RESERVED block above).
        (SHARES_DATA_SOURCE, "SHARES_DATA_SOURCE"), // emitted by DbResolver (A13.3)
        (RPC_CALLS, "RPC_CALLS"),                   // emitted by RpcStackResolver (A10.10)
        // L0.1 leap reservations — no emitter yet.
        (NAVIGATES_TO, "NAVIGATES_TO"), // LA.6a
        (CO_CHANGES, "CO_CHANGES"),     // LF.5b
    ];

    /// Name for an edge-category id, or `"UNKNOWN"` if unregistered.
    pub fn name(c: EdgeCategoryId) -> &'static str {
        ALL.iter().find(|(id, _)| *id == c).map(|(_, n)| *n).unwrap_or("UNKNOWN")
    }
}

// ============================================================================
// Cell types
// ============================================================================

pub mod cell_type {
    use super::CellTypeId;
    pub const CODE: CellTypeId = CellTypeId(1);
    pub const DOC: CellTypeId = CellTypeId(2);
    pub const POSITION: CellTypeId = CellTypeId(3);
    pub const INTENT: CellTypeId = CellTypeId(4);
    pub const ROUTE_METHOD: CellTypeId = CellTypeId(5);
    /// One client call site, a compact JSON cell stacked once per site (the
    /// graph builder appends the cells of every copy of a node), so a side's
    /// cells are the union of its sites. Two payloads:
    ///
    /// - HTTP, on an ENDPOINT: `{"method","path","file","line","col",
    ///   "confidence"[,"raw"][,"host"]}` (`endpoint::endpoint_hit_json`;
    ///   `hosts` / `template` added by the engine's endpoint fold and the
    ///   wrapper stage). ROUTE / ENDPOINT readers (the Locator, `glia arch`
    ///   `node_file`, `http_node_span`, gaps) key on `file` or on the kind.
    /// - Channel client, on a WS_CLIENT or GRPC_CLIENT (CB.21):
    ///   `{"via":"ws"|"grpc"[,"host":"<host[:port]>"]}`
    ///   (`endpoint::channel_hit_cell`). `host` is the authority the site
    ///   dials (a ws URL's, a gRPC dial target's), present only when the
    ///   source spells it as a literal. No `file`, so every file-keyed reader
    ///   skips it; the channel resolvers' host narrowing reads `host` exactly
    ///   as the HTTP resolver's does, and a hostless site blocks narrowing.
    pub const ENDPOINT_HIT: CellTypeId = CellTypeId(6);
    /// The tests that cover a node DIRECTLY: the sources of the TESTS edges
    /// into it (the Python parser's function-level edges, the engine's module
    /// name pairing, overlay-declared ones), a compact JSON cell
    /// `{"tests":[{"test":"<qname>","kind":"FUNCTION"}],"total":N}` with
    /// `tests` sorted by qname, deduped and capped at 50 (`total` is the real
    /// count). One cell per node, on its first copy in graph order. Never
    /// transitive: the tests reaching a node through its callers are a query
    /// (LE.3b tests-for), not a stored cell. Emitter LE.3a
    /// (`engine::passes::fill_test_cells`, the `fill_test_cells` Post pass).
    pub const TEST: CellTypeId = CellTypeId(7);
    pub const ATTN: CellTypeId = CellTypeId(8);
    pub const FAIL: CellTypeId = CellTypeId(9);
    pub const CONSTRAINT: CellTypeId = CellTypeId(10);
    pub const DECISION: CellTypeId = CellTypeId(11);
    pub const ENV: CellTypeId = CellTypeId(12);
    pub const CONV: CellTypeId = CellTypeId(13);
    pub const VECTOR: CellTypeId = CellTypeId(14);
    /// Provenance/locality of a node: a JSON cell
    /// `{"provenance":"build_output|vendored|generated|authored|submodule|worktree|nested_repo","region":"www","files":N}`.
    /// The last three are collapsed REGION anchors for a tree that belongs to
    /// ANOTHER repository (a `.git` dir, a submodule's or a linked worktree's
    /// `.git` file) — see `walk_gating::Collapse`. (A8.1)
    /// A `PROJECT` anchor (A8.5) carries
    /// `{"provenance":"project_root","ecosystem":"npm","manifest":"apps/web/package.json","label":"@shop/web","path":"apps/web"}`.
    /// Its `path` is repo-relative and is `""` for the repo root.
    /// Lets consumers (engram, neuropil) filter by *coordinate* rather than by
    /// string-matching keys, and preserves the spatial map of a repo without
    /// emitting a node per file inside a collapsed region. (glia-v2 G10/G14)
    pub const ORIGIN: CellTypeId = CellTypeId(15);
    /// External library names imported in a node's source file — a JSON array
    /// `["ethers","web3"]` (deduped, sorted, capped). Denormalized per-node so
    /// the engram exporter can fill `Content::Symbol.imports` without a parent
    /// lookup; gives the encoder library context. (glia-v5 G15)
    pub const IMPORTS: CellTypeId = CellTypeId(16);

    // ------------------------------------------------------------------
    // RESERVED ids — central locked-id allocation (packet W0.3). Cell types
    // are allocated here and nowhere else; see the matching block in
    // `node_kind` for the rule. Owning packet per id:
    //   17 MESSAGE_TYPE — A12.1
    //   18 RPC_PACKAGE  — A5.1
    // 19-25 allocated centrally by L0.1 for the 0.5.0 leap. The packet texts
    // proposed SCHEMA_FIELDS 26, COVERAGE 27, ENTRYPOINT 28; compacted so ALL
    // stays 1..=len, with ACCESS_MODE kept at 25 so every stale number (26,
    // 27, 28) is UNREGISTERED and decodes UNKNOWN instead of naming another
    // cell:
    //   19 DOC_TAGS      — LA.8
    //   20 ROLE          — LB.3a
    //   21 EVIDENCE      — LC.3a
    //   22 SCHEMA_FIELDS — LE.10a (LE.10b writes it too)
    //   23 COVERAGE      — LF.6c
    //   24 ENTRYPOINT    — LF.3b
    //   25 ACCESS_MODE   — LE.4a
    // ------------------------------------------------------------------

    /// RESERVED (A12.1) — the message / payload schema type a node sends or
    /// receives, as the declared type name.
    pub const MESSAGE_TYPE: CellTypeId = CellTypeId(17);

    /// RESERVED (A5.1) — the RPC package / namespace a service declaration
    /// lives in (a proto `package foo.bar;`).
    pub const RPC_PACKAGE: CellTypeId = CellTypeId(18);

    /// RESERVED (LA.8) — structured doc-comment tags, a JSON cell
    /// `{"style":"natspec","tags":[{"tag":"param","name":"to","text":"..."}]}`
    /// with tags in source order (NatSpec first; `name` only where the tag
    /// takes one). Emitter LA.8; Engram mapping LG.12.
    pub const DOC_TAGS: CellTypeId = CellTypeId(19);

    /// RESERVED (LB.3a) — the framework roles a CLASS / STRUCT / FUNCTION plays
    /// after the build-time role fold, a JSON cell `{"roles":["SERVICE",...]}`.
    /// Written by LB.3a; read through graph `roles_in` (LB.3b, LA.6a,
    /// LA.21a / LA.21b).
    pub const ROLE: CellTypeId = CellTypeId(20);

    /// RESERVED (LC.3a) — an EDGE cell recording why an edge exists, JSON
    /// `{"emitter":"...","rule":"...","file":"...","line":N,"basis":"site"}`.
    /// `rule`, `file` and `line` are optional; `line` is 0-based (the POSITION
    /// convention); `basis` is one of `site | from_node | to_node | file |
    /// none`. Writers LC.3a-d; readers LE.5, LC.10b, LE.1b.
    pub const EVIDENCE: CellTypeId = CellTypeId(21);

    /// RESERVED (LE.10a) — the declared fields of a schema, JSON
    /// `{"format":"proto","<section>":[{"name","type","number"?,"label"?,
    /// "oneof"?,"required"?,"default"?}]}`. On proto / Avro `MESSAGE_TYPE`
    /// nodes (LE.10a) and OpenAPI / AsyncAPI / Pact contract-op DOC_SECTIONs
    /// (LE.10b); diffed by LE.10c.
    pub const SCHEMA_FIELDS: CellTypeId = CellTypeId(22);

    /// RESERVED (LF.6c) — line coverage from an lcov snapshot, JSON
    /// `{"source":"lcov","lines":N,"hit":N}` (integers) on a MODULE / CLASS /
    /// FUNCTION / METHOD. Emitter LF.6c.
    pub const COVERAGE: CellTypeId = CellTypeId(23);

    /// RESERVED (LF.3b) — marks a node declared as an entrypoint in
    /// `.glia/overlay.toml`, JSON `{"source":"config","pattern":"...",
    /// "decl":"..."}`. Emitter LF.3b; read by the LD.6 entry seeding. Not the
    /// LD.6 entry-KIND table: that one derives entrypoints from node kinds,
    /// this cell carries the ones a user declared.
    pub const ENTRYPOINT: CellTypeId = CellTypeId(24);

    /// An EDGE cell on `ACCESSES_DATA`, `CellPayload::Text` `read | write |
    /// read_write`, taken from the SQL / collection-call / Cypher verb and
    /// folded over one function's statements; absent when no statement says.
    /// Emitter LE.4a (`anchor::rehome_to_owner`); readers LE.4d, LE.7a.
    pub const ACCESS_MODE: CellTypeId = CellTypeId(25);

    /// Canonical id→name for every cell type. Backs the pyo3 `cell_type_names`
    /// decode table so consumers that read structured cells (WP-J) can label
    /// them without a local table. Keep in lockstep with the constants above.
    pub const ALL: &[(CellTypeId, &str)] = &[
        (CODE, "CODE"),
        (DOC, "DOC"),
        (POSITION, "POSITION"),
        (INTENT, "INTENT"),
        (ROUTE_METHOD, "ROUTE_METHOD"),
        (ENDPOINT_HIT, "ENDPOINT_HIT"),
        (TEST, "TEST"),
        (ATTN, "ATTN"),
        (FAIL, "FAIL"),
        (CONSTRAINT, "CONSTRAINT"),
        (DECISION, "DECISION"),
        (ENV, "ENV"),
        (CONV, "CONV"),
        (VECTOR, "VECTOR"),
        (ORIGIN, "ORIGIN"),
        (IMPORTS, "IMPORTS"),
        // Reserved ids (see the RESERVED block above) — no emitter yet.
        (MESSAGE_TYPE, "MESSAGE_TYPE"),
        (RPC_PACKAGE, "RPC_PACKAGE"),
        // L0.1 leap reservations — no emitter yet.
        (DOC_TAGS, "DOC_TAGS"),           // LA.8
        (ROLE, "ROLE"),                   // LB.3a
        (EVIDENCE, "EVIDENCE"),           // LC.3a
        (SCHEMA_FIELDS, "SCHEMA_FIELDS"), // LE.10a / LE.10b
        (COVERAGE, "COVERAGE"),           // LF.6c
        (ENTRYPOINT, "ENTRYPOINT"),       // LF.3b
        (ACCESS_MODE, "ACCESS_MODE"),     // LE.4a
    ];

    /// Name for a cell-type id, or `"UNKNOWN"` if unregistered.
    pub fn name(c: CellTypeId) -> &'static str {
        ALL.iter().find(|(id, _)| *id == c).map(|(_, n)| *n).unwrap_or("UNKNOWN")
    }
}

// ============================================================================
// Errors
// ============================================================================

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("tree-sitter parse produced no tree")]
    NoTree,
    #[error("tree-sitter language init failed: {0}")]
    LanguageInit(String),
}

// ============================================================================
// Import records (language-agnostic shape)
// ============================================================================

/// An import statement as parsed from a source file. The resolver uses this
/// to wire cross-file bindings regardless of the source language.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct ImportStmt {
    /// qname of the module doing the importing (`myapp::auth`, `svc::users`).
    pub from_module: String,
    pub target: ImportTarget,
    /// 0-based row of the import statement (the POSITION `start_line`
    /// convention). One statement that imports several names gives each of
    /// its `ImportStmt`s the statement's row. The IMPORTS edge it resolves to
    /// carries it as its EVIDENCE site line (LC.3b).
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub enum ImportTarget {
    /// Whole-module import — Python `import foo.bar`, Go `import "github.com/x/y"`,
    /// TS `import * as f from "./foo"` or `import "./foo"`.
    /// Alias is the bound name in the importing module (None = default name).
    Module { path: String, alias: Option<String> },
    /// Named symbol import — Python `from foo.bar import baz`, TS `import { baz } from "./foo"`.
    /// Go doesn't have this form; Go imports are always Module.
    /// `level` is Python-specific (relative-import dot count); non-Python parsers pass 0.
    Symbol {
        module: String,
        name: String,
        alias: Option<String>,
        level: u32,
    },
}

// ============================================================================
// Call records
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct CallSite {
    pub from: NodeId,
    pub qualifier: CallQualifier,
    /// 0-based row of the call expression (POSITION convention), not of the
    /// enclosing declaration: the CALLS edge's EVIDENCE site line (LC.3b).
    pub line: u32,
}

/// An identifier reference that needs cross-file resolution into an edge of
/// a specific category. Used at v0.4.4 for route handler references.
///
/// Shape: parser sees `r.POST("/login", controllers.AuthHandler)` inside
/// `server.setupRoutes()`. It emits
/// ```ignore
/// UnresolvedRef {
///     from: route_id,                     // edge source (the Route node)
///     from_module: server_module_id,      // whose binding table resolves the qualifier
///     qualifier: Attribute { base: "controllers", name: "AuthHandler" },
///     category: HANDLED_BY,
///     line: 41,                           // the 0-based row of `r.POST(...)`
/// }
/// ```
/// `from_module` is separate from `from` because Route nodes are path-only
/// (package-agnostic) and have no unique enclosing module — the parser must
/// tell the resolver which package's imports to use.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct UnresolvedRef {
    pub from: NodeId,
    pub from_module: NodeId,
    pub qualifier: CallQualifier,
    pub category: EdgeCategoryId,
    /// 0-based row of the reference (POSITION convention): the registration
    /// call naming a handler, the heritage clause, the link. The edge it
    /// resolves to carries it as its EVIDENCE site line (LC.3b).
    pub line: u32,
}

/// The 0-based row of `byte_offset` in `source`, for emitters that find a
/// construct by scanning text rather than walking a tree: the number of `\n`
/// bytes before the offset. Counted on bytes, so an offset inside a multi-byte
/// character cannot panic; an offset past the end counts the whole source.
pub fn line_of(source: &str, byte_offset: usize) -> u32 {
    let bytes = source.as_bytes();
    let end = byte_offset.min(bytes.len());
    let rows = bytes[..end].iter().filter(|&&b| b == b'\n').count();
    u32::try_from(rows).unwrap_or(u32::MAX)
}

/// The scope a file gives the nodes it DECLARES by content rather than by
/// code structure (LB.12): its repo-relative directories `::`-joined, then
/// its file STEM - `services/orders/openapi.yaml` -> `services::orders::openapi`,
/// root `openapi.json` -> `openapi`, `docs/a/guide.md` -> `docs::a::guide`.
/// A contract op is `contract::<this>::<op>` and a markdown section
/// `docs::<this>::<slug>`, so two directories' same-named files keep their
/// own nodes, while a yaml / json twin of ONE spec in one directory
/// (`swagger.yaml` + `swagger.json`) still declares one set of ops. Only the
/// last extension goes (`users.controller.ts` -> `users.controller`), as
/// `Path::file_stem` has it; a `\` separator reads as `/`.
///
/// Not a MODULE qname: a non-code file's MODULE keeps its full file name
/// (LB.9a, `engine::extract::synthetic_module_qname`), so a twin's two files
/// stay two MODULEs while their ops merge.
pub fn dir_stem_qname(path: &str) -> String {
    let p = path.replace('\\', "/");
    let (dir, file) = match p.rsplit_once('/') {
        Some((dir, file)) => (Some(dir), file),
        None => (None, p.as_str()),
    };
    let stem = std::path::Path::new(file)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(file);
    match dir {
        Some(dir) => format!("{}::{stem}", dir.replace('/', "::")),
        None => stem.to_string(),
    }
}

/// Classification of a call site by its syntactic shape. Resolution (which
/// node id the call actually targets) happens in `glia-graph` using the
/// import table + symbol table, not in the parser.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub enum CallQualifier {
    /// `foo()` — bare name. Resolves to a local def, an imported symbol, or
    /// stays unresolved.
    Bare(String),
    /// Call on the enclosing method's receiver — Python `self.m()`, TS
    /// `this.m()`, Go `u.m()` where `u` is the method receiver. Resolves
    /// against the enclosing class's method set.
    SelfMethod(String),
    /// `super().m()` in Python — call on the enclosing class's parent method.
    /// Resolved against the enclosing class's recorded base class names, then
    /// the module's symbol table for that base class's methods.
    SuperMethod(String),
    /// `base.name()` where `base` is a plain identifier. Could be an imported
    /// module, an imported symbol, a struct instance, or a local variable.
    /// Disambiguation lives in the cross-file resolver.
    Attribute { base: String, name: String },
    /// `<complex>.name()` — receiver is a chained expression, not a plain
    /// identifier. Kept verbatim for diagnostics; not resolved at v0.4.3b.
    ComplexReceiver { receiver: String, name: String },
}

// ============================================================================
// FileParse + CodeNav
// ============================================================================

/// Normalize one import path to an external library name, per language, or
/// `None` for relative / intra-workspace / stdlib-ish imports. (glia-v5 G15)
pub fn library_name(path: &str, lang: &str) -> Option<String> {
    let p = path.trim().trim_matches(|c| c == '"' || c == '\'');
    if p.is_empty() {
        return None;
    }
    let scoped = |p: &str| -> Option<String> {
        // `@scope/pkg/...` → `@scope/pkg`
        let rest = p.strip_prefix('@')?;
        let mut it = rest.splitn(3, '/');
        Some(format!("@{}/{}", it.next()?, it.next()?))
    };
    match lang {
        "typescript" | "javascript" | "react" | "angular" | "vue" => {
            if p.starts_with('.') {
                return None; // relative
            }
            scoped(p).or_else(|| p.split('/').next().map(str::to_string))
        }
        "solidity" => {
            if p.starts_with('.') {
                return None;
            }
            scoped(p).or_else(|| p.split('/').next().map(str::to_string))
        }
        // Go: only external third-party modules count as library deps — their
        // import path's first segment is a domain (contains a '.'), e.g.
        // github.com, golang.org, gopkg.in. Internal imports (repo-relative
        // after go.mod prefix stripping, WP-G) and stdlib (fmt, net/http) have
        // no domain segment and must NOT leak into Symbol.imports. Return the
        // module path (host/org/repo). Parser stores paths `::`- or `/`-joined.
        "go" => {
            let segs: Vec<&str> =
                p.split(|c| c == '/' || c == ':').filter(|s| !s.is_empty()).collect();
            match segs.first() {
                Some(first) if first.contains('.') => {
                    Some(segs.iter().take(3).copied().collect::<Vec<_>>().join("/"))
                }
                _ => None,
            }
        }
        "python" => p.split('.').next().map(str::to_string),
        "rust" => {
            let top = p.split("::").next()?;
            if matches!(top, "crate" | "self" | "super" | "std" | "core" | "alloc") {
                return None;
            }
            // LA.1b: a raw `use` path may start with a type (`use Kind::*`,
            // `use Ordering::Less`); a crate name is never capitalised.
            if top.starts_with(|c: char| c.is_ascii_uppercase()) {
                return None;
            }
            Some(top.to_string())
        }
        "dart" => p
            .strip_prefix("package:")
            .and_then(|r| r.split('/').next())
            .map(str::to_string),
        "java" | "csharp" | "scala" | "kotlin" => {
            // dotted package — keep the first two segments as the library prefix.
            let segs: Vec<&str> = p.split('.').collect();
            (!segs.is_empty()).then(|| segs.iter().take(2).copied().collect::<Vec<_>>().join("."))
        }
        "c_cpp" => {
            let p = p.trim_matches(|c| c == '<' || c == '>');
            if p.is_empty() {
                return None;
            }
            p.split('/').next().map(str::to_string)
        }
        _ => {
            if p.starts_with('.') {
                return None;
            }
            p.split(|c| c == '/' || c == ':')
                .find(|s| !s.is_empty())
                .map(str::to_string)
        }
    }
}

/// External library names imported in a file (deduped, sorted, capped at 10).
/// (glia-v5 G15)
pub fn library_names(imports: &[ImportStmt], lang: &str) -> Vec<String> {
    let mut set = std::collections::BTreeSet::new();
    for imp in imports {
        let path = match &imp.target {
            ImportTarget::Module { path, .. } => path.as_str(),
            ImportTarget::Symbol { module, level, .. } => {
                if *level > 0 {
                    continue; // Python relative import — intra-package
                }
                module.as_str()
            }
        };
        if let Some(lib) = library_name(path, lang) {
            set.insert(lib);
        }
    }
    set.into_iter().take(10).collect()
}

/// Attach an `IMPORTS` cell (JSON array of library names) to every node of `fp`,
/// computed once from `fp.imports`. Always attaches (empty `[]` distinguishes
/// "no imports" from "not extracted"). The engram exporter reads it into
/// `Content::Symbol.imports`. (glia-v5 G15)
pub fn attach_imports_cell(fp: &mut FileParse, lang: &str) {
    let json = imports_json(&library_names(&fp.imports, lang));
    for n in &mut fp.nodes {
        n.cells.push(Cell {
            kind: cell_type::IMPORTS,
            payload: CellPayload::Json(json.clone()),
        });
    }
}

/// The IMPORTS cell payload: a JSON array of library names.
fn imports_json(libs: &[String]) -> String {
    format!(
        "[{}]",
        libs.iter()
            .map(|l| format!("\"{}\"", l.replace('\\', "\\\\").replace('"', "\\\"")))
            .collect::<Vec<_>>()
            .join(",")
    )
}

// ---------------------------------------------------------------------------
// Intra-repo import filter (A16.4, audit 2026-06-10 #12)
// ---------------------------------------------------------------------------

/// What a repo DECLARES, indexed so a candidate library name can be recognised
/// as an intra-repo reference rather than a dependency. `library_name` decides
/// from the import's syntax alone (Go's "no domain, not a dependency" arm); this
/// is the repo-shaped generalisation to every language, because the leak is
/// repo-shaped: `use crate::snapshot::Page` is only local because the repo has
/// a `snapshot` module.
///
/// Feed it only language-parser parses. A synthetic parse's file-derived module
/// (`config/logging.yaml` → `config::logging`) would shadow a real `logging`.
///
/// `CodeNav` is HashMap-backed; the index reads it and keeps only BTree
/// collections, whose contents do not depend on insertion order, so the index
/// is deterministic.
#[derive(Debug, Default, Clone)]
pub struct LocalModuleIndex {
    /// Every segment-aligned prefix and suffix of every MODULE / PACKAGE qname,
    /// normalised by [`import_segments`]. `src::snapshot` contributes `src`,
    /// `src::snapshot` and `snapshot`. A PACKAGE's last qname segment is its
    /// declared name, and when that name is dotted (Elixir
    /// `defmodule MyApp.Repo`) its own prefixes (`MyApp`) are declared
    /// namespaces too.
    paths: std::collections::BTreeSet<String>,
    /// Declared item names (CLASS / STRUCT / INTERFACE / ENUM / FUNCTION) →
    /// how many nodes declare each.
    items: std::collections::BTreeMap<String, usize>,
    /// Declared MODULE / PACKAGE names → how many nodes declare each. A separate
    /// count to `items`, because Java's `Helper.java` is one declaration but two nodes
    /// (MODULE `Helper`, CLASS `Helper::Helper`); one shared count would make
    /// every Java class ambiguous.
    modules: std::collections::BTreeMap<String, usize>,
    /// A6.8: the literal before the `*` of every tsconfig `paths` wildcard key
    /// (`@core/*` -> `@core/`). A TS-family specifier starting with one names
    /// an in-repo module, not a package ([`Self::is_alias_import`]).
    alias_prefixes: std::collections::BTreeSet<String>,
    /// A6.8: every wildcard-free tsconfig `paths` key (`@env`), matched whole.
    alias_exact: std::collections::BTreeSet<String>,
}

/// Split an import path or qname on every separator the parsers use (`::`,
/// `.`, `/`, `\`), dropping empty segments.
fn import_segments(s: &str) -> Vec<&str> {
    s.split(|c| matches!(c, ':' | '.' | '/' | '\\'))
        .filter(|seg| !seg.is_empty())
        .collect()
}

impl LocalModuleIndex {
    /// Fold one parse's declarations into the index.
    pub fn add_parse(&mut self, fp: &FileParse) {
        for (id, kind) in &fp.nav.kind_by_id {
            let name = fp.nav.name_by_id.get(id);
            let is_package = *kind == node_kind::PACKAGE;
            match *kind {
                node_kind::MODULE | node_kind::PACKAGE => {
                    if let Some(qname) = fp.nav.qname_by_id.get(id) {
                        // LB.9b: a MODULE named by its file name
                        // (`api::user.py`) declares its bare path
                        // (`api::user`), so the extension never becomes a
                        // local import segment (`import_segments` splits on
                        // `.`, which would declare `py` / `ts` local).
                        let bare = name
                            .filter(|_| !is_package)
                            .and_then(|n| bare_module_qname(qname, n));
                        self.add_path(bare.as_deref().unwrap_or(qname), is_package);
                    }
                    if let Some(name) = name {
                        *self.modules.entry(name.clone()).or_default() += 1;
                    }
                }
                node_kind::CLASS
                | node_kind::STRUCT
                | node_kind::INTERFACE
                | node_kind::ENUM
                | node_kind::FUNCTION => {
                    if let Some(name) = name {
                        *self.items.entry(name.clone()).or_default() += 1;
                    }
                }
                _ => {}
            }
        }
    }

    /// Declare a workspace crate's path identifier (`glia_engine`)
    /// local (LA.1b): a Rust `use` of a sibling crate is intra-repo, not a
    /// dependency. The engine seeds every Cargo package the walk found.
    pub fn add_local_crate(&mut self, name: &str) {
        let segs = import_segments(name);
        if !segs.is_empty() {
            self.paths.insert(segs.join("::"));
        }
    }

    fn add_path(&mut self, qname: &str, is_package: bool) {
        let segs = import_segments(qname);
        // A Rust `x/mod.rs` declares the module `x` (LA.1b: `pub use x::..`
        // is intra-repo), so its suffixes end at `x` too.
        if segs.len() > 1 && segs.last() == Some(&"mod") {
            let dir = &segs[..segs.len() - 1];
            for i in 1..=dir.len() {
                self.paths.insert(dir[dir.len() - i..].join("::"));
            }
        }
        for i in 1..=segs.len() {
            self.paths.insert(segs[..i].join("::"));
            self.paths.insert(segs[segs.len() - i..].join("::"));
        }
        if is_package {
            if let Some(declared) = qname.rsplit("::").next() {
                let segs = import_segments(declared);
                for i in 1..=segs.len() {
                    self.paths.insert(segs[..i].join("::"));
                }
            }
        }
    }

    /// True if `lib` (any of `::` `.` `/` `\` as separator) is a segment-aligned
    /// prefix or suffix of a declared module / package path.
    pub fn is_local_path(&self, lib: &str) -> bool {
        let segs = import_segments(lib);
        !segs.is_empty() && self.paths.contains(&segs.join("::"))
    }

    /// A6.8: declare a tsconfig `compilerOptions.paths` key local, as written
    /// (`@core/*`, `@env`). A key with a `*` declares every specifier that
    /// starts with its literal prefix (`@core/`); a key without one declares
    /// exactly itself. A key with an empty literal prefix (`*`, the catch-all
    /// that maps bare specifiers into `node_modules`) declares nothing: it
    /// would make every package local. The aliases of every tsconfig the
    /// build read are declared repo-wide: a TS file under one project root
    /// importing a real package spelt like another root's alias would lose
    /// that package from its cell.
    pub fn add_alias_prefix(&mut self, key: &str) {
        let key = key.trim();
        match key.split_once('*') {
            Some((prefix, _)) => {
                if !prefix.is_empty() {
                    self.alias_prefixes.insert(prefix.to_string());
                }
            }
            None => {
                if !key.is_empty() {
                    self.alias_exact.insert(key.to_string());
                }
            }
        }
    }

    /// A6.8: true if the raw import `specifier` (quotes allowed) matches a
    /// declared tsconfig alias ([`Self::add_alias_prefix`]). Asked of the
    /// specifier, never of its [`library_name`]: that keeps only the first
    /// segment(s), so `~/lib/x` under `~/*` is the library `~`, which no longer
    /// shows the `~/` the alias names.
    pub fn is_alias_import(&self, specifier: &str) -> bool {
        let spec = specifier.trim().trim_matches(|c| c == '"' || c == '\'');
        !spec.is_empty()
            && (self.alias_exact.contains(spec)
                || self.alias_prefixes.iter().any(|p| spec.starts_with(p.as_str())))
    }

    /// True if exactly one declaration in the repo carries `name`. An item
    /// declaration (class, struct, …) wins over a module of the same name.
    pub fn is_local_symbol(&self, name: &str) -> bool {
        match self.items.get(name) {
            Some(n) => *n == 1,
            None => self.modules.get(name) == Some(&1),
        }
    }
}

/// Languages whose `Symbol` imports name a namespace the parser emits no node
/// for (Java / Kotlin / Scala packages) or a crate path (Rust), so the imported
/// item's name is the only repo-shaped evidence. Anywhere else a bare name
/// match over-fires: a one-app Django project's single `models.py` would drop
/// `django` for its `django.db` models import, and TS
/// `import { User } from 'firebase/auth'` beside the repo's own `interface User`
/// would drop `firebase`.
fn symbol_evidence_applies(lang: &str) -> bool {
    matches!(lang, "java" | "kotlin" | "scala" | "rust")
}

/// A6.8: the language tags whose imports resolve through tsconfig `paths`
/// (the TS family, as [`library_name`] groups them). A Python `import config`
/// is never a TS alias, whatever a tsconfig in the repo declares.
fn ts_alias_applies(lang: &str) -> bool {
    matches!(lang, "typescript" | "javascript" | "react" | "angular" | "vue")
}

/// The form of a candidate library name that is looked up in the index. A
/// C/C++ candidate is an include path (`mathutil.h`). The header's MODULE is
/// named by its file name (`c::mathutil.h`, LB.10a) with its stem as nav
/// name, so the index holds LB.9b's bare form (`c::mathutil`) and the
/// candidate is looked up by its stem.
fn local_lookup_key<'a>(lib: &'a str, lang: &str) -> &'a str {
    if lang == "c_cpp" {
        if let Some((stem, ext)) = lib.rsplit_once('.') {
            let header_or_source = matches!(
                ext,
                "h" | "hh" | "hpp" | "hxx" | "inl" | "ipp" | "tpp" | "c" | "cc" | "cpp" | "cxx"
            );
            if header_or_source && !stem.is_empty() {
                return stem;
            }
        }
    }
    lib
}

/// [`library_names`] minus every name that resolves inside the repo. Returns
/// the kept names (deduped, sorted, capped at 10 AFTER filtering, so a file
/// whose first ten names were local still reports its real dependencies) and
/// how many distinct names were dropped.
fn filter_library_names(
    imports: &[ImportStmt],
    lang: &str,
    local: &LocalModuleIndex,
) -> (Vec<String>, usize) {
    let mut kept = std::collections::BTreeSet::new();
    let mut dropped = std::collections::BTreeSet::new();
    for imp in imports {
        let (path, symbol) = match &imp.target {
            ImportTarget::Module { path, .. } => (path.as_str(), None),
            ImportTarget::Symbol { module, name, level, .. } => {
                if *level > 0 {
                    continue; // Python relative import — intra-package
                }
                (module.as_str(), Some(name.as_str()))
            }
        };
        let Some(lib) = library_name(path, lang) else { continue };
        let is_local = local.is_local_path(local_lookup_key(&lib, lang))
            || (ts_alias_applies(lang) && local.is_alias_import(path))
            || (symbol_evidence_applies(lang) && symbol.is_some_and(|n| local.is_local_symbol(n)));
        if is_local {
            dropped.insert(lib);
        } else {
            kept.insert(lib);
        }
    }
    // A name one import proves local and another does not is still a dependency.
    let dropped = dropped.difference(&kept).count();
    (kept.into_iter().take(10).collect(), dropped)
}

/// [`library_names`] without the names that resolve inside the repo (A16.4).
pub fn library_names_filtered(
    imports: &[ImportStmt],
    lang: &str,
    local: &LocalModuleIndex,
) -> Vec<String> {
    filter_library_names(imports, lang, local).0
}

/// [`attach_imports_cell`] with intra-repo names filtered out (A16.4). Leaves
/// every node with exactly one IMPORTS cell: an existing one (the router's raw
/// cell, or one replayed from a parse cache) is rewritten in its slot and any
/// duplicate removed; a node without one gains it. Idempotent. Returns
/// `(kept, dropped)` — library names in the cell, and distinct names dropped.
pub fn attach_imports_cell_filtered(
    fp: &mut FileParse,
    lang: &str,
    local: &LocalModuleIndex,
) -> (usize, usize) {
    let (libs, dropped) = filter_library_names(&fp.imports, lang, local);
    let json = imports_json(&libs);
    for n in &mut fp.nodes {
        let mut seen = false;
        n.cells.retain_mut(|c| {
            if c.kind != cell_type::IMPORTS {
                return true;
            }
            if seen {
                return false;
            }
            seen = true;
            c.payload = CellPayload::Json(json.clone());
            true
        });
        if !seen {
            n.cells.push(Cell {
                kind: cell_type::IMPORTS,
                payload: CellPayload::Json(json.clone()),
            });
        }
    }
    (libs.len(), dropped)
}

/// The per-file output every code-language parser produces. `glia-graph`
/// consumes a `Vec<FileParse>` to build a `RepoGraph`.
///
/// `Clone` + serde: the incremental build (WP-D) caches the per-file parse so an
/// unchanged file skips tree-sitter on the next build. serde uses bincode (the
/// cache is transient/regenerable — not the rkyv `.gmap` path).
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct FileParse {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub imports: Vec<ImportStmt>,
    pub calls: Vec<CallSite>,
    /// Identifier refs that aren't call expressions but still need cross-file
    /// resolution into an edge. v0.4.4 use case: route handler references.
    pub refs: Vec<UnresolvedRef>,
    pub nav: CodeNav,
    /// v0.4.13b — method ids tagged as property-style (read as `self.x`, not
    /// `self.x()`). Python: `@property` decorator. Other languages: equivalent
    /// getter annotations (e.g. Kotlin `val x: T get()`). Lets synth path BFS
    /// filter method→class hops to only those that are syntactically valid
    /// attribute reads.
    pub properties: std::collections::HashSet<NodeId>,
}

/// LB.9b: the stem-form qname of a code MODULE named by its full file name.
///
/// When two or more code files share a directory and a stem, whatever their
/// build groups (`api/user.py` + `api/user.ts`, LB.9b; `src/util.ts` +
/// `src/util.js`, LB.13), the engine names each MODULE by its file name
/// (`api::user.py`, `api::user.ts`) and keeps the stem as its nav name
/// (`user`). For such a MODULE this returns the qname its imports name
/// (`api::user`); for every other qname it returns None: a normal code
/// module's last segment IS its name, and a non-code MODULE's name is its
/// whole file name (LB.9a, `api::user.proto` named `user.proto`).
pub fn bare_module_qname(qname: &str, name: &str) -> Option<String> {
    let (dir, last) = qname.rsplit_once("::").unwrap_or(("", qname));
    let ext = last.strip_prefix(name)?.strip_prefix('.')?;
    if name.is_empty() || ext.is_empty() || ext.contains(['.', ':']) {
        return None;
    }
    Some(if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}::{name}")
    })
}

/// LB.13: which sibling a bare import binds when one graph holds several
/// files of a stem (`src/util.ts` + `src/util.js`, both named by file name).
///
/// Keyed by the IMPORTER's file extension; each list is that language's own
/// resolution order, first match wins. Only languages that import FILES have
/// one: Java / Kotlin imports name classes, Elixir aliases name `defmodule`s,
/// Terraform modules are directories, and a `.cljc` file is compiled for both
/// platforms and loads a different sibling on each. Empty = no rule: the
/// import stays unresolved (and counted), never guessed.
pub fn same_stem_order(importer_ext: &str) -> &'static [&'static str] {
    match importer_ext {
        // tsc: `.ts` `.tsx` (`.d.ts`), then `.js` `.jsx` under `allowJs`.
        "ts" | "tsx" => &["ts", "tsx", "js", "jsx", "vue"],
        // Node / Vite `resolve.extensions` default: `.mjs` `.js` `.mts` `.ts`
        // `.jsx` `.tsx`.
        "js" | "jsx" | "vue" => &["js", "ts", "jsx", "tsx", "vue"],
        // Clojure `RT.load`: `.clj` before `.cljc`, never `.cljs`.
        "clj" => &["clj", "cljc"],
        // ClojureScript: `.cljs` before `.cljc`, never `.clj`.
        "cljs" => &["cljs", "cljc"],
        _ => &[],
    }
}

/// Code-domain navigation indices — what the strict `Node` shape pushed out of
/// per-node fields. Merged across files by v0.4.3 into one per-repo index.
///
/// serde (bincode only — `NodeId` map keys aren't strings, so serde_json can't
/// take it) so a `FileParse` round-trips through the WP-D parse cache.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct CodeNav {
    /// Simple name (`"login"`), not the full qualified name.
    pub name_by_id: HashMap<NodeId, String>,
    /// Full qualified name (`"myapp::users::User::login"`). Used by the resolver
    /// to map import targets onto node ids.
    pub qname_by_id: HashMap<NodeId, String>,
    pub kind_by_id: HashMap<NodeId, NodeKindId>,
    /// Direct parent: method → class, class → module, function → module (or
    /// enclosing function for nested defs).
    pub parent_of: HashMap<NodeId, NodeId>,
    /// Inverse of `parent_of`.
    pub children_of: HashMap<NodeId, Vec<NodeId>>,
    /// Declared type of an instance field / property, per owning CLASS / STRUCT
    /// node: owner -> (field name -> declared simple type name, with generics,
    /// namespace qualifiers and `?` already stripped). Filled by parsers that
    /// can read a declared type off the AST, through [`CodeNav::record_field_type`];
    /// read by `resolve_calls`' receiver-type inference (A6.2a), which binds
    /// `_repo.Find()` / `this.repo.find()` to a method of that type.
    ///
    /// Build-time only: deliberately NOT mirrored into the store's
    /// `CodeNavStore`, because resolution finishes before the .gmap is written.
    pub field_types: HashMap<NodeId, HashMap<String, String>>,
    /// Local bindings of a fn / METHOD body, per enclosing callable: scope ->
    /// (bound name -> simple type name). A parameter, a `let` or any pattern
    /// binding. `""` records a local of unknown type: it still shadows a
    /// same-named field in the receiver pass. A type of the form `self.<f>`
    /// aliases the enclosing type's field `f` (`let r = &self.repo`). Filled
    /// through [`CodeNav::record_local_type`] (Rust, LA.35a); read by
    /// `resolve_calls`' receiver-type inference, innermost-first, before
    /// `field_types`.
    ///
    /// Two more forms (Go, CA.2a): a MODULE scope holds that file's
    /// package-level vars (the generic receiver pass looks up the caller's
    /// own scope, never a MODULE, so only a language hook reads them); and a
    /// type ending in `()` is a normalised call chain (`svc.Repo()`,
    /// `repositories.NewX()`, arguments elided): the local holds that call's
    /// result, whose type a language hook reads off [`CodeNav::return_types`].
    /// Go keeps a package qualifier (`repositories.UserRepository`), which no
    /// bare-name lookup matches.
    ///
    /// Build-time only, like `field_types`: never mirrored into the store.
    pub local_types: HashMap<NodeId, HashMap<String, String>>,
    /// Per-scope language facts that are neither a node, an edge, an import
    /// nor a call site: scope (a MODULE, or a fn / METHOD) -> its [`NavFact`]s
    /// in the order the parser recorded them. Filled through
    /// [`CodeNav::record_fact`] (CB.6); each variant is read by one builder.
    ///
    /// Build-time only, like `field_types`: never mirrored into the store. A
    /// reader walks scopes in a fixed order (`g.nodes`), never this map's.
    pub nav_facts: HashMap<NodeId, Vec<NavFact>>,
    /// The type a FUNCTION / METHOD (an interface METHOD included) returns,
    /// as the source names it: its first result, pointers unwrapped, generic
    /// arguments dropped, a package qualifier kept (Go:
    /// `func X() (*repositories.UserRepository, error)` ->
    /// `repositories.UserRepository`). A callable whose first result owns no
    /// in-repo method (a predeclared type, a slice / map / func type, a type
    /// parameter) records nothing. Filled through
    /// [`CodeNav::record_return_type`] (Go, CA.2a); read by the Go call hook
    /// (CA.2b) to type a receiver that is a call chain.
    ///
    /// Build-time only, like `field_types`: never mirrored into the store.
    pub return_types: HashMap<NodeId, String>,
    /// The normalised signature of a METHOD (a struct method or an interface
    /// method element): its parameter types then its result types, each list
    /// in parentheses, parameter names and package qualifiers dropped and no
    /// whitespace (Go: `func (r *R) Build(cfg json.RawMessage) (*x.Bundle,
    /// error)` -> `(RawMessage)(*Bundle,error)`). An implementation and the
    /// interface method it satisfies, written in different packages, record
    /// the same text. A method on a generic receiver, or an element of an
    /// interface with type parameters, records nothing (unknown). Filled
    /// through [`CodeNav::record_method_sig`] (Go, CA.3a); read by the Go
    /// implicit-IMPLEMENTS pass (CA.3b), after the Go build has replaced each
    /// in-repo type alias a text names by its target ([`NavFact::TypeAlias`],
    /// CI.6).
    ///
    /// Build-time only, like `field_types`: never mirrored into the store.
    pub method_sigs: HashMap<NodeId, String>,
}

/// A build-time fact a parser records for a scope (a MODULE, or a fn / METHOD)
/// that is not a node, edge, import or call site. Read by one builder each;
/// never mirrored into the store (resolution finishes before the .gmap is
/// written). A new variant goes at the END, so the old variants keep their
/// bincode tags (the parse cache discards on any PARSER_STAMP move anyway).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum NavFact {
    /// C/C++ (CB.19 -> CB.25): a function prototype declared, not defined, at
    /// file / namespace scope of this MODULE; `ns` is the enclosing C++
    /// namespace path (`""` = global).
    DeclaresFn { ns: String, name: String },
    /// C++ (CB.19 -> CB.25): `using namespace <ns>;` in this MODULE (at file
    /// scope, `within` = `""`, or inside the namespace `within`).
    UsingNamespace { within: String, ns: String },
    /// C++ (CB.19 -> CB.25): `using <ns>::<name>;`.
    UsingName { within: String, ns: String, name: String },
    /// Go (CB.23 -> CB.20): the call on 0-based row `line` of this fn passes a
    /// router mount as argument `arg` (0-based, receiver excluded) of the
    /// function or method named `callee`.
    MountArg { line: u32, callee: String, arg: u32, mount: Mount },
    /// Go (CB.23 -> CB.20): this fn assigns a router mount to field `field` of
    /// struct `owner`, named `<package dir qname>::<Type>` (`api::Server`): a
    /// Go package is its directory (LA.13b), and a struct's methods may sit in
    /// other files of it, whose file-module qnames differ.
    FieldMount { owner: String, field: String, mount: Mount },
    /// C/C++ (CB.19 -> CB.25): a function this MODULE defines with internal
    /// linkage (`static` in C / C++, or inside an anonymous namespace): never
    /// the definition a prototype in another file names.
    InternalLinkage { name: String },
    /// GraphQL / tRPC (CB.24): this MODULE builds a client whose base URL has
    /// the literal authority `host` (`via` = `"graphql"` | `"rpc"`), on 0-based
    /// row `line`; the post-cache graft spreads it to its project's
    /// GRAPHQL_OPERATION / RPC_CALL sides.
    ClientHost { via: String, host: String, line: u32 },
    /// Dart (CH.5a): this MODULE builds an HTTP client whose base URL is the
    /// expression `expr` (`via` = `"dio"`: a `BaseOptions(baseUrl: ..)`
    /// argument, or the right side of `<recv>.options.baseUrl = ..`) on
    /// 0-based row `line`; the endpoint fold (CH.5c) resolves it to the
    /// project's client base path.
    ClientBase { via: String, expr: String, line: u32 },
    /// Dart (CH.5a): getter or constant `name` (`Env.apiBaseUrl` in a type,
    /// `apiBase` at library level) evaluates to the URL-shaped literal `value`,
    /// `${…}` for each interpolation.
    ValueLiteral { name: String, value: String },
    /// TypeScript (CH.1b): this METHOD is an `abstract` member declared without
    /// a body (recorded by the TS parser's `visit_abstract_method`); the graph
    /// crate's `emit_abstract_implements` pairs a subclass method of the same
    /// name with it as a method-level IMPLEMENTS.
    AbstractMethod,
    /// TypeScript (CH.3b): this callable (>= 1 parameter) reads, itself or
    /// through same-file callees two deep, exactly one API-prefix-named member
    /// `key` (`apiPrefix`); the endpoint fold (CH.5b) prefixes a URL built
    /// through it with that key's configured value.
    UrlPrefixKey { key: String },
    /// Go (CI.6): this MODULE declares the type alias `type <name> =
    /// <target>`; `shape` is the target's CA.3a type shape (package
    /// qualifiers dropped, the text [`CodeNav::method_sigs`] entries are
    /// written in). Not recorded for a generic alias, a target with a parse
    /// error, or an identity re-export whose shape is its own name. Read by
    /// the Go build to resolve aliases in method signatures before the
    /// implicit-IMPLEMENTS signature compare.
    TypeAlias { name: String, shape: String },
}

/// Where a Go router group's prefix comes from (CB.6; recorded by CB.23,
/// resolved by CB.20). A provisional ROUTE on a Param / Field mount is named
/// by [`endpoint::mount_route_qname`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum Mount {
    /// A literal prefix known in the file (`/api/v2`; `""` = a router root).
    Const(String),
    /// Parameter `index` of the enclosing fn `fn_qname` (receiver excluded),
    /// then `suffix`.
    Param { fn_qname: String, index: u32, suffix: String },
    /// Field `field` of struct `owner` (`<package dir qname>::<Type>`), then
    /// `suffix`.
    Field { owner: String, field: String, suffix: String },
}

impl CodeNav {
    /// Record `fact` for `scope` (CB.6), after the scope's earlier facts. A
    /// fact equal to one already recorded for that scope is dropped, so a
    /// scope visited twice (a C++ MODULE walked through both branches of an
    /// `#ifdef`) records each fact once.
    pub fn record_fact(&mut self, scope: NodeId, fact: NavFact) {
        let facts = self.nav_facts.entry(scope).or_default();
        if !facts.contains(&fact) {
            facts.push(fact);
        }
    }

    /// Record that `scope` (a fn / METHOD) binds a local `name` of simple type
    /// `ty` (`""` = a local whose type is unknown). An empty `name` is
    /// ignored. The first record stores `ty`; a later record of a DIFFERENT
    /// type stores `""`: a name bound to two types in one body (Rust
    /// shadowing, two match arms) is a local of unknown type, which loses an
    /// edge rather than guess one.
    pub fn record_local_type(&mut self, scope: NodeId, name: &str, ty: &str) {
        if name.is_empty() {
            return;
        }
        let locals = self.local_types.entry(scope).or_default();
        match locals.get_mut(name) {
            Some(existing) if existing != ty => existing.clear(),
            Some(_) => {}
            None => {
                locals.insert(name.to_string(), ty.to_string());
            }
        }
    }

    /// Record that the callable `f` returns type `ty` (CA.2a). An empty `ty`
    /// is ignored; a second record for `f` replaces the first.
    pub fn record_return_type(&mut self, f: NodeId, ty: &str) {
        if ty.is_empty() {
            return;
        }
        self.return_types.insert(f, ty.to_string());
    }

    /// Record that the METHOD `m` has the normalised signature `sig` (CA.3a).
    /// An empty `sig` is ignored (a signature always holds its two
    /// parenthesised lists); a second record for `m` replaces the first.
    pub fn record_method_sig(&mut self, m: NodeId, sig: &str) {
        if sig.is_empty() {
            return;
        }
        self.method_sigs.insert(m, sig.to_string());
    }

    /// Record that `owner` (a CLASS / STRUCT) declares a field or property
    /// `field` of simple type `type_name`. Empty names are ignored. A second
    /// record for the same `(owner, field)` replaces the first.
    pub fn record_field_type(&mut self, owner: NodeId, field: &str, type_name: &str) {
        if field.is_empty() || type_name.is_empty() {
            return;
        }
        self.field_types
            .entry(owner)
            .or_default()
            .insert(field.to_string(), type_name.to_string());
    }

    /// Record a node's navigation metadata. Parsers call this right after
    /// pushing the `Node` onto the FileParse.
    pub fn record(
        &mut self,
        id: NodeId,
        name: &str,
        qname: &str,
        kind: NodeKindId,
        parent: Option<NodeId>,
    ) {
        self.name_by_id.insert(id, name.to_string());
        self.qname_by_id.insert(id, qname.to_string());
        self.kind_by_id.insert(id, kind);
        if let Some(p) = parent {
            self.parent_of.insert(id, p);
            self.children_of.entry(p).or_default().push(id);
        }
    }
}

// ============================================================================
// Shared client-HTTP endpoint emission (Pattern A — handoff v6 P1)
// ============================================================================
//
// A client-side HTTP call — `dio.get('/x')` (Dart), `requests.get(url)`
// (Python), `http.Get(url)` (Go), `restTemplate.getForObject(url)` (Java),
// `URLSession…dataTask(url)` (Swift) — should become an ENDPOINT node that
// `HttpStackResolver` pairs to a server ROUTE → HTTP_CALLS. The TypeScript
// parser already emits this shape (fetch/axios); today no other language does,
// so cross-stack HTTP is TS-only. This helper emits the IDENTICAL shape so the
// one resolver works uniformly across languages (cross-benefit lives in the
// shared crate, not duplicated per parser).

pub mod endpoint {
    use super::{cell_type, edge_category, node_kind, GRAPH_TYPE};
    use glia_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};
    use std::collections::HashSet;

    use super::CodeNav;

    /// One extracted client HTTP call. `path` MUST use `${…}` for any
    /// interpolated segment so it normalises the same way TS template paths do:
    /// `normalise_http_path` in glia-graph collapses any segment containing
    /// `${` (or `:id` / `{id}`) to `{}`, so `/users/${…}` matches route
    /// `/users/{id}` uniformly.
    pub struct ClientEndpoint {
        /// Upper-case HTTP verb, e.g. `"GET"`.
        pub method: String,
        /// Request path, e.g. `"/users/${…}"`.
        pub path: String,
        /// Call-site file (relative), carried on the ENDPOINT_HIT cell so a route
        /// can be traced back to the specific frontend call-site.
        pub file: String,
        pub line: usize,
        pub col: usize,
        pub confidence: Confidence,
    }

    fn esc(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                _ => out.push(c),
            }
        }
        out
    }

    /// Optional trailing fields of an ENDPOINT_HIT payload, beyond the
    /// [`ClientEndpoint`] core. Every field defaults to absent, and an absent
    /// field writes nothing, so `HitExtras::default()` is the pre-A3.3 payload
    /// byte for byte.
    ///
    /// Carried beside the endpoint rather than as `ClientEndpoint` fields so
    /// the parsers that build `ClientEndpoint` literals (eleven of them) do not
    /// all have to change when one opts in.
    #[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
    pub struct HitExtras<'a> {
        /// A3.3: the call-site literal `ep.path` was normalised from (see
        /// [`normalise_client_path`]). Only when the path was rewritten.
        pub raw: Option<&'a str>,
        /// A11.5: the request authority, `host[:port]`, from an absolute URL
        /// literal (see [`client_url_split`]). The field A11.4's host
        /// narrowing reads.
        pub host: Option<&'a str>,
    }

    /// The ENDPOINT_HIT payload. The [`HitExtras`] are written LAST, `raw`
    /// then `host`, and only when present, so every endpoint without them
    /// keeps a byte-identical payload. `host` goes after `raw` because that is
    /// where the engine's endpoint fold appends it (`Fields::set` on a payload
    /// that has no `host` yet), so a parser that pre-sets it produces the
    /// same folded bytes as one that leaves it to the fold.
    ///
    /// `path` is written in place of `ep.path`: the caller passes the
    /// canonical form (LB.5), so the cell and the qname agree.
    fn endpoint_hit_json(ep: &ClientEndpoint, path: &str, extras: HitExtras<'_>) -> String {
        let conf = match ep.confidence {
            Confidence::Strong => "strong",
            Confidence::Medium => "medium",
            Confidence::Weak => "weak",
        };
        let raw = extras
            .raw
            .map(|r| format!(r#","raw":"{}""#, esc(r)))
            .unwrap_or_default();
        let host = extras
            .host
            .map(|h| format!(r#","host":"{}""#, esc(h)))
            .unwrap_or_default();
        format!(
            r#"{{"method":"{}","path":"{}","file":"{}","line":{},"col":{},"confidence":"{}"{}{}}}"#,
            esc(&ep.method),
            esc(path),
            esc(&ep.file),
            ep.line,
            ep.col,
            conf,
            raw,
            host,
        )
    }

    /// Join a group/router prefix with a relative path template.
    ///
    /// The shared primitive every framework route extractor needs (go
    /// chi/gin, elixir, and the framework packets that follow), hoisted here
    /// so the prefix edge cases cannot drift apart per-parser. Semantics are
    /// the ones the go router walker has always had:
    ///
    /// - an empty `prefix` returns `path` unchanged — it deliberately does NOT
    ///   force a leading `/` (that is [`abs_path`], kept separate precisely so
    ///   hoisting this fn could not move existing go route qnames);
    /// - `path == "/"` returns the prefix as-is, so a group's index route is
    ///   the group itself rather than `"/api/"`;
    /// - a trailing `/` run on the prefix and a leading `/` on the path never
    ///   double up.
    ///
    /// ```text
    /// join_path("",      "users")  == "users"
    /// join_path("/api",  "users")  == "/api/users"
    /// join_path("/api/", "/users") == "/api/users"
    /// join_path("/api",  "/")      == "/api"
    /// ```
    pub fn join_path(prefix: &str, path: &str) -> String {
        if prefix.is_empty() {
            return path.to_string();
        }
        if path == "/" {
            return prefix.to_string();
        }
        let p = prefix.trim_end_matches('/');
        if path.starts_with('/') {
            format!("{p}{path}")
        } else {
            format!("{p}/{path}")
        }
    }

    /// Join a stack of nested scope prefixes with a relative path template.
    ///
    /// The elixir `scope` / `resources` walker's semantics, hoisted verbatim:
    /// each non-empty stack entry contributes `/` + itself with its trailing
    /// `/` run trimmed, then `path` is appended with a `/` inserted when it
    /// does not already start with one. The result is always absolute.
    ///
    /// ```text
    /// join_scope(&[],                            "/users") == "/users"
    /// join_scope(&["/api".into()],               "users")  == "/api/users"
    /// join_scope(&["/api".into(), "v1".into()],  "users")  == "/api/v1/users"
    /// ```
    pub fn join_scope(stack: &[String], path: &str) -> String {
        let mut full = String::new();
        for s in stack {
            if !s.is_empty() {
                if !s.starts_with('/') {
                    full.push('/');
                }
                full.push_str(s.trim_end_matches('/'));
            }
        }
        if !path.starts_with('/') {
            full.push('/');
        }
        full.push_str(path);
        if full.is_empty() {
            "/".to_string()
        } else {
            full
        }
    }

    /// Force a route template to have exactly one leading `/`.
    ///
    /// The single place the "route qnames are absolute" precondition that
    /// `index_route_node` relies on is satisfied, so the framework route
    /// extractors do not each re-derive it. Trims surrounding whitespace and
    /// collapses a run of leading slashes; an empty or slash-only input is the
    /// root `"/"`. Never panics.
    ///
    /// Deliberately does NOT trim a trailing `/`: `normalise_http_path` in
    /// glia-graph already does that downstream, and stripping it twice
    /// would change existing route qnames.
    ///
    /// ```text
    /// abs_path("api/users") == "/api/users"
    /// abs_path("//x")       == "/x"
    /// abs_path("")          == "/"
    /// abs_path("/api/")     == "/api/"
    /// ```
    pub fn abs_path(p: &str) -> String {
        let body = p.trim().trim_start_matches('/');
        if body.is_empty() {
            return "/".to_string();
        }
        format!("/{body}")
    }

    // ------------------------------------------------------------------
    // JVM annotation routes (A4.4 recipe, hoisted by A14.4)
    //
    // Java and Kotlin build ONE graph (the JVM family), so a Spring /
    // Micronaut / JAX-RS controller must mint the same ROUTE name in either
    // language. Each parser reads a declaration's OWN annotations off its
    // AST as `(simple name, path argument)` pairs; everything from there to
    // the `{VERB} {path}` names is this pure-string recipe, the one place a
    // later change (LB.4 / LB.5) edits for both languages.
    // ------------------------------------------------------------------

    /// Spring `@GetMapping`-style and Micronaut `@Get`-style verb annotations.
    pub fn mapping_verb(name: &str) -> Option<&'static str> {
        Some(match name {
            "GetMapping" | "Get" => "GET",
            "PostMapping" | "Post" => "POST",
            "PutMapping" | "Put" => "PUT",
            "DeleteMapping" | "Delete" => "DELETE",
            "PatchMapping" | "Patch" => "PATCH",
            "Head" => "HEAD",
            "Options" => "OPTIONS",
            _ => return None,
        })
    }

    /// A JAX-RS verb marker (`@GET`, `@POST`, …) — upper-case, so it never
    /// collides with Micronaut's `@Get` / `@Post`.
    pub fn jaxrs_verb(name: &str) -> Option<&'static str> {
        Some(match name {
            "GET" => "GET",
            "POST" => "POST",
            "PUT" => "PUT",
            "DELETE" => "DELETE",
            "PATCH" => "PATCH",
            "HEAD" => "HEAD",
            "OPTIONS" => "OPTIONS",
            _ => return None,
        })
    }

    /// A Spring `RestTemplate` convenience method's HTTP verb:
    /// `getForObject` / `getForEntity` → GET, `postForObject` / `postForLocation`
    /// → POST, `patchForObject`, `headForHeaders`, `optionsForAllow`, and bare
    /// `put` / `delete`. Hoisted from the Java parser by A14.6 so Java and Kotlin
    /// read one table (LA.22's client breadth extends it here).
    ///
    /// The `*For*` families are self-describing; bare `put` / `delete` are not
    /// (`map.put("/k", v)`, Javalin's `app.delete("/x", h)`), so each caller
    /// gates them further: Java on the URL path filter, Kotlin on the receiver.
    pub fn rest_template_verb(name: &str) -> Option<&'static str> {
        if name.starts_with("getFor") {
            Some("GET")
        } else if name.starts_with("postFor") {
            Some("POST")
        } else if name.starts_with("patchFor") {
            Some("PATCH")
        } else if name.starts_with("headFor") {
            Some("HEAD")
        } else if name.starts_with("optionsFor") {
            Some("OPTIONS")
        } else if name == "put" {
            Some("PUT")
        } else if name == "delete" {
            Some("DELETE")
        } else {
            None
        }
    }

    /// The verb of the first `HttpMethod.<VERB>` reference in `text` (one
    /// argument of RestTemplate's `.exchange(url, HttpMethod.GET, …)` /
    /// `.execute(…)`, or of WebClient's `.method(HttpMethod.GET)`), case-folded
    /// and checked against the HTTP verb set ([`jaxrs_verb`]'s). Pure text, so
    /// the Java and Kotlin client arms share it (A14.6).
    pub fn http_method_ref_verb(text: &str) -> Option<&'static str> {
        let idx = text.find("HttpMethod.")?;
        let verb: String = text[idx + "HttpMethod.".len()..]
            .chars()
            .take_while(|c| c.is_ascii_alphabetic())
            .collect();
        jaxrs_verb(&verb.to_ascii_uppercase())
    }

    /// Compose a class-level prefix with an action template. Spring,
    /// Micronaut and JAX-RS all CONCATENATE — a leading `/` on the method
    /// template does not make it absolute — so `@RequestMapping("/api/v1/users")`
    /// with `@GetMapping("/{id}")` is `/api/v1/users/{id}`. [`join_path`] does
    /// the slash bookkeeping; [`abs_path`] supplies the leading `/` that
    /// `join_path` deliberately does not force.
    ///
    /// ```text
    /// compose_route_path("/api", "/users") == "/api/users"
    /// compose_route_path("api",  "")       == "/api"
    /// compose_route_path("",     "users")  == "/users"
    /// ```
    pub fn compose_route_path(class_prefix: &str, tmpl: &str) -> String {
        if tmpl.is_empty() {
            return abs_path(class_prefix);
        }
        abs_path(&join_path(class_prefix, tmpl))
    }

    /// The route prefix a type contributes to its action methods: the first
    /// of Spring `@RequestMapping`, Micronaut `@Controller` or JAX-RS `@Path`
    /// among the type's own annotations that carries a non-empty path.
    /// Empty when the type is not prefixed.
    pub fn jvm_route_prefix(anns: &[(String, Option<String>)]) -> String {
        for (name, arg) in anns {
            if matches!(name.as_str(), "RequestMapping" | "Controller" | "Path")
                && let Some(p) = arg
                && !p.is_empty()
            {
                return p.clone();
            }
        }
        String::new()
    }

    /// The `(verb, path)` ROUTEs one declaration's own annotations declare,
    /// composed onto `class_prefix` (empty at class level, the enclosing
    /// type's [`jvm_route_prefix`] at method level), in emission order:
    ///
    /// - per annotation, in source order: a [`mapping_verb`] maps its path;
    ///   Spring `@RequestMapping` / Micronaut `@Controller` map theirs as the
    ///   `ANY` wildcard (`method = RequestMethod.GET` is not read);
    /// - then JAX-RS: `@Path` takes the verb of a [`jaxrs_verb`] marker on the
    ///   same declaration (`ANY` without one); a verb marker with no `@Path`
    ///   maps the resource root itself.
    ///
    /// An annotation with no path of its own (`@PostMapping`) maps the class
    /// prefix; with no prefix either there is nothing to name, so it yields
    /// no route rather than an invented one.
    pub fn jvm_annotation_routes(
        anns: &[(String, Option<String>)],
        class_prefix: &str,
    ) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        let mut emit = |verb: &'static str, tmpl: Option<&str>| {
            if tmpl.is_none() && class_prefix.is_empty() {
                return;
            }
            out.push((verb, compose_route_path(class_prefix, tmpl.unwrap_or_default())));
        };
        for (name, arg) in anns {
            if let Some(verb) = mapping_verb(name) {
                emit(verb, arg.as_deref());
            }
            if matches!(name.as_str(), "RequestMapping" | "Controller") {
                emit("ANY", arg.as_deref());
            }
        }
        let verb = anns.iter().find_map(|(n, _)| jaxrs_verb(n));
        if let Some((_, arg)) = anns.iter().find(|(n, _)| n == "Path") {
            emit(verb.unwrap_or("ANY"), arg.as_deref());
        } else if let Some(verb) = verb {
            emit(verb, None);
        }
        out
    }

    /// The ONE canonical path form for ROUTE / ENDPOINT qnames (LB.5): exactly
    /// one leading `/`.
    ///
    /// Returned byte-identical when [`is_canonical_http_path`] already holds —
    /// it starts with `/`, starts with a `${` base placeholder (the resolver's
    /// BaseFold tier and the engine's endpoint fold read that shape), is empty,
    /// or is the `<unresolved>` placeholder. Anything else goes through
    /// [`abs_path`]. The early return is load-bearing: `abs_path` trims
    /// whitespace and collapses `//x`, and neither may move a qname that was
    /// already correct.
    ///
    /// ```text
    /// canonical_http_path("api/users") == "/api/users"
    /// canonical_http_path("  a ")      == "/a"
    /// canonical_http_path("/x")        == "/x"
    /// canonical_http_path("//x")       == "//x"
    /// canonical_http_path("${…}/u")    == "${…}/u"
    /// canonical_http_path("")          == ""
    /// ```
    pub fn canonical_http_path(p: &str) -> std::borrow::Cow<'_, str> {
        if is_canonical_http_path(p) {
            std::borrow::Cow::Borrowed(p)
        } else {
            std::borrow::Cow::Owned(abs_path(p))
        }
    }

    /// True when `p` is already in [`canonical_http_path`]'s form, i.e. that
    /// function would return it unchanged. The `[http-qname]` census in the
    /// HTTP resolver uses it to name any emitter that bypasses the builders.
    pub fn is_canonical_http_path(p: &str) -> bool {
        p.is_empty() || p == "<unresolved>" || p.starts_with('/') || p.starts_with("${")
    }

    /// The ROUTE qname `<METHOD> <path>`, with the path canonical: one node
    /// per (method, path), `ANY` for a method-agnostic registration. Every
    /// server parser builds it here; since LB.11a (go) and LB.11b (ts_routes)
    /// nothing emits the per-path `route:<path>` shape, which the HTTP
    /// resolver still reads for tolerance and its `[http-qname] pathonly=`
    /// census counts.
    pub fn route_qname(method: &str, path: &str) -> String {
        format!("{method} {}", canonical_http_path(path))
    }

    /// Opens the mount token of a provisional mount ROUTE qname
    /// ([`mount_route_qname`]).
    const MOUNT_OPEN: &str = "<mount:";

    /// CB.6: the qname of a Go ROUTE registered on a router group whose prefix
    /// the parser cannot read in the file (a parameter- or field-held group):
    /// `<METHOD> <mount:<token>><path>`, token `param:<fn_qname>#<index>` or
    /// `field:<owner>.<field>`, `<path>` the mount's suffix joined with
    /// `local_path` ([`join_path`]), canonical. A canonical path starts with
    /// `/` or `${`, is empty or is the `<unresolved>` placeholder, never
    /// `<mount:`, so no real ROUTE collides with one; the build's mount pass
    /// (CB.20) rewrites every provisional before the language graph is
    /// returned, so the HTTP resolver never sees one. A token never holds a
    /// `>`: a Go qname is identifiers and directory names.
    ///
    /// A Const mount is never provisional: it returns the plain ROUTE qname,
    /// `route_qname(method, join_path(prefix, local_path))`, byte for byte
    /// what the Go parser builds for a group whose prefix it reads.
    ///
    /// ```text
    /// Param { Register, 0, "" },    "/u"  -> "GET <mount:param:api::routes::Register#0>/u"
    /// Param { Register, 0, "/me" }, "p"   -> "GET <mount:param:api::routes::Register#0>/me/p"
    /// Field { api::Server, admin, "" }, "/x" -> "GET <mount:field:api::Server.admin>/x"
    /// Const("/api/v2"),             "users" -> "GET /api/v2/users"
    /// ```
    pub fn mount_route_qname(method: &str, mount: &super::Mount, local_path: &str) -> String {
        use super::Mount;
        let (token, suffix) = match mount {
            Mount::Const(prefix) => return route_qname(method, &join_path(prefix, local_path)),
            Mount::Param {
                fn_qname,
                index,
                suffix,
            } => (format!("param:{fn_qname}#{index}"), suffix),
            Mount::Field {
                owner,
                field,
                suffix,
            } => (format!("field:{owner}.{field}"), suffix),
        };
        let path = canonical_http_path(&join_path(suffix, local_path)).into_owned();
        format!("{method} {MOUNT_OPEN}{token}>{path}")
    }

    /// CB.6: the inverse of [`mount_route_qname`] for a provisional qname:
    /// `(method, mount, path)`. The mount's suffix was folded into the path
    /// when the qname was built, so the mount comes back with an empty suffix
    /// and `path` holds suffix and local path together (one route, however it
    /// was reached). `None` for anything else, a canonical `<METHOD> <path>`
    /// ROUTE qname included. The caller strips an LB.4a owner first with
    /// [`split_owner`]: this reads the whole tail as the path.
    pub fn parse_mount_route_qname(q: &str) -> Option<(String, super::Mount, String)> {
        use super::Mount;
        let (method, rest) = q.split_once(' ')?;
        if method.is_empty() {
            return None;
        }
        let (token, path) = rest.strip_prefix(MOUNT_OPEN)?.split_once('>')?;
        let mount = if let Some(p) = token.strip_prefix("param:") {
            let (fn_qname, index) = p.rsplit_once('#')?;
            if fn_qname.is_empty() || index.is_empty() || !index.bytes().all(|b| b.is_ascii_digit())
            {
                return None;
            }
            Mount::Param {
                fn_qname: fn_qname.to_string(),
                index: index.parse().ok()?,
                suffix: String::new(),
            }
        } else {
            // A Go field name holds no `.`; an owner's directory may.
            let (owner, field) = token.strip_prefix("field:")?.rsplit_once('.')?;
            if owner.is_empty() || field.is_empty() {
                return None;
            }
            Mount::Field {
                owner: owner.to_string(),
                field: field.to_string(),
                suffix: String::new(),
            }
        };
        Some((method.to_string(), mount, path.to_string()))
    }

    /// ENDPOINT qname `endpoint:<METHOD>:<path>`, with the path canonical —
    /// the shape `HttpStackResolver::parse_endpoint_qname` reads.
    pub fn endpoint_qname(method: &str, path: &str) -> String {
        format!("endpoint:{method}:{}", canonical_http_path(path))
    }

    /// LB.4a: the separator between an HTTP qname and its OWNER segment, the
    /// repo-relative dir of the nested project root the node lives under:
    /// `GET /health @services/users`, `POST /users @api`,
    /// `endpoint:GET:/health @web`, `page:/users @web`. A suffix, so every
    /// reader that parses from the `route:` / `endpoint:` / `<METHOD> ` start
    /// keeps working once it strips it with [`split_owner`]. A canonical path
    /// never contains a space, so the LAST ` @` is the separator.
    pub const OWNER_SEP: &str = " @";

    /// `qname` qualified with `owner` ([`OWNER_SEP`]). An empty owner leaves the
    /// qname unchanged: a node outside every nested project root keeps its
    /// pre-0.5.0 identity. The caller passes a whitespace-free owner (the
    /// engine escapes whitespace in a root path), or [`split_owner`] could not
    /// read it back.
    pub fn with_owner(qname: &str, owner: &str) -> String {
        if owner.is_empty() {
            qname.to_string()
        } else {
            format!("{qname}{OWNER_SEP}{owner}")
        }
    }

    /// Split an HTTP qname into `(qname without owner, owner)`. `(q, None)`
    /// when `q` carries no owner: no ` @`, or what follows the last one is
    /// empty or holds whitespace. An `@` with no space before it (an npm scope
    /// in a path segment, `/pkg/@scope`) is never a separator. The inverse of
    /// [`with_owner`]; every consumer that parses a ROUTE / ENDPOINT / page
    /// qname strips the owner with this first.
    pub fn split_owner(q: &str) -> (&str, Option<&str>) {
        match q.rsplit_once(OWNER_SEP) {
            Some((base, owner)) if !owner.is_empty() && !owner.contains(char::is_whitespace) => {
                (base, Some(owner))
            }
            _ => (q, None),
        }
    }

    /// True if `name` names an HTTP *client* receiver — `dio`, `http`,
    /// `httpClient`, `apiClient`, `api`, `userApi`, `restClient`, `_client`. A
    /// `.get('/x')` on one of these is an OUTBOUND call (an ENDPOINT), never a
    /// server route registration. A leading `_` is stripped (Dart private
    /// fields) and the comparison is case-folded.
    ///
    /// Shared by the two route scanners that see both shapes in one textual
    /// form (A3.5): Dart's shelf scan (lifted verbatim from there, so its
    /// behaviour is unchanged) and ts_routes' Express/Koa/Hono scan, where
    /// Angular's `this.http.get('/users')` used to mint a phantom server ROUTE
    /// that the service's own ENDPOINT then paired to.
    ///
    /// The false negatives are DELIBERATE — do not "fix" them here:
    /// - a server router named `api` / `*Api` / `*client` registering a NAMED
    ///   handler (`api.get('/users', getUsers)`) reads as a client and its
    ///   route is lost — it is indistinguishable from `api.post('/users',
    ///   body)`. (ts_routes keeps the registration when the last argument is
    ///   an INLINE function, which no HTTP client takes: Hono's `const api =
    ///   new Hono()` routers.)
    /// - `dio` is a substring test, so `audioRouter` / `studioRouter` count as
    ///   clients too.
    ///
    /// Server routers are conventionally `app` / `router` / `server`, and a
    /// lost route is recoverable from the Next.js / NestJS / Hapi / Bun shapes,
    /// whereas a phantom route poisons every downstream primitive (the service
    /// is reported as calling itself over HTTP). Not covered: NestJS's
    /// `httpService` (no `client`/`api` suffix) — widening this changes Dart.
    pub fn is_http_client_receiver(name: &str) -> bool {
        let n = name.trim_start_matches('_').to_ascii_lowercase();
        n == "dio"
            || n.contains("dio")
            || n == "http"
            || n.ends_with("client")
            || n == "api"
            || n.ends_with("api")
    }

    /// The identifier immediately preceding byte index `at` — the receiver of
    /// a `.method(` call: `http` in `this.http.get(`, `app` in `app.get(`.
    /// Whitespace between the identifier and `at` is skipped. Empty for
    /// cascades (`..get`), call results (`request(app).get(`) and other
    /// non-identifier prefixes. Identifier bytes are `[A-Za-z0-9_]`, so a JS
    /// `$http` yields `http`.
    ///
    /// Never panics: an `at` past the end is clamped, and an `at` that is not
    /// on a char boundary yields `""`.
    pub fn ident_before(source: &str, at: usize) -> &str {
        let bytes = source.as_bytes();
        let mut end = at.min(bytes.len());
        while end > 0 && bytes[end - 1].is_ascii_whitespace() {
            end -= 1;
        }
        let mut start = end;
        while start > 0 {
            let ch = bytes[start - 1];
            if ch.is_ascii_alphanumeric() || ch == b'_' {
                start -= 1;
            } else {
                break;
            }
        }
        source.get(start..end).unwrap_or("")
    }

    /// Drop a `?query` / `#fragment` and everything after it.
    fn cut_query(s: &str) -> &str {
        &s[..s.find(['?', '#']).unwrap_or(s.len())]
    }

    /// Split `scheme://authority` off a query-free URL. The SINGLE place that
    /// decides what an authority is; [`url_split`], [`url_to_path`] and
    /// [`normalise_client_path`] all go through it.
    ///
    /// A `://` only counts as a scheme separator when no `/` comes before it,
    /// so a path that embeds a URL (`/proxy/http://x/y`) is not split. Callers
    /// cut the query first (see [`cut_query`]), so a `://` inside a query value
    /// (`/login?next=https://x/y`) is never seen here at all.
    ///
    /// Returns `(authority, rest)`: `rest` starts at the first `/` after the
    /// authority, or is `"/"` when the URL has no path.
    fn split_authority(s: &str) -> (Option<&str>, &str) {
        let Some(i) = s.find("://") else {
            return (None, s);
        };
        if s[..i].contains('/') {
            return (None, s);
        }
        let rest = &s[i + 3..];
        match rest.find('/') {
            Some(j) => (Some(&rest[..j]), &rest[j..]),
            None => (Some(rest), "/"),
        }
    }

    /// Split a URL literal into `(authority, path)` (A11.2).
    ///
    /// - `authority` is `host[:port]`, with any `user[:pass]@` prefix dropped;
    ///   None when the literal has no `scheme://` or the authority is empty
    ///   (`file:///x`).
    /// - `path` is the request path with the query and fragment dropped; None
    ///   when what is left does not start with `/` (a bare word, a relative
    ///   hint, a `${…}` base the caller could not resolve).
    ///
    /// ```text
    /// http://h:8080/a?b      -> (Some("h:8080"), Some("/a"))
    /// https://u:p@api.x      -> (Some("api.x"),  Some("/"))
    /// /a#frag                -> (None,           Some("/a"))
    /// /login?next=http://x/y -> (None,           Some("/login"))
    /// auth/login             -> (None,           None)
    /// ```
    ///
    /// Does not trim; [`url_to_path`] trims before calling. The engine's
    /// endpoint-fold pass (`engine/src/endpoint_fold.rs`) records the
    /// authority as `"host"` on ENDPOINT_HIT for the parsers that hand it a
    /// `raw`/`template` (TS, Dart); the other clients record it at extraction
    /// through [`client_url_split`] (A11.5).
    pub fn url_split(raw: &str) -> (Option<String>, Option<String>) {
        let (authority, rest) = split_authority(cut_query(raw));
        let host = authority
            .map(|a| a.rsplit_once('@').map_or(a, |(_, h)| h))
            .filter(|h| !h.is_empty())
            .map(str::to_string);
        let path = rest.starts_with('/').then(|| rest.to_string());
        (host, path)
    }

    /// Extract the request PATH from a URL literal. Absolute URLs
    /// (`http://host/x`, `https://…/x`) → the path (`/x`); already-relative
    /// paths (`/x`) pass through; a bare host, a non-path string, or a variable
    /// → None. Query/fragment are dropped. Lets a client call to
    /// `http://api/users` pair with route `/users` (addresses the host-prefix
    /// normalisation gap, handoff Pattern I). Interpolation reconstruction
    /// (`$id`/`${expr}`/f-string) stays per-parser — call this AFTER it.
    ///
    /// The path half of [`url_split`] on the trimmed literal. Since A11.2 the
    /// query is cut BEFORE the scheme is looked for and a scheme must precede
    /// the first `/`, so `/login?next=http://x/y` is `/login` (was `/y`) and
    /// `/proxy/http://x/y` is itself (was `/y`).
    pub fn url_to_path(raw: &str) -> Option<String> {
        url_split(raw.trim()).1
    }

    /// [`url_split`] for a CLIENT call literal a parser has reconstructed
    /// (A11.5): `(host, path)` where `path` is EXACTLY [`url_to_path`]`(raw)`,
    /// so a parser that swaps `url_to_path` for this keeps every ENDPOINT
    /// qname, and `host` is the authority to record as `"host"` on
    /// ENDPOINT_HIT.
    ///
    /// The host is kept only when it is a literal authority. Parsers turn
    /// every interpolation into `${…}`, so a reconstructed literal can put a
    /// placeholder in either half of `scheme://authority`, and both mean the
    /// service is not named in the source:
    ///
    /// ```text
    /// http://api.example.com/users  -> (Some("api.example.com"), Some("/users"))
    /// http://svc:8080/x?y=1         -> (Some("svc:8080"),        Some("/x"))
    /// https://${…}/users            -> (None,                    Some("/users"))
    /// http://localhost:${…}/users   -> (None,                    Some("/users"))
    /// ${…}://api/users              -> (None,                    Some("/users"))
    /// /users/${…}                   -> (None,                    Some("/users/${…}"))
    /// ```
    ///
    /// A literal authority is `host[:port]` (a bracketed IPv6 literal
    /// included) spelled from ASCII letters, digits and `.-_~:[]`. That
    /// rejects `${…}`, a Python `{name}`, a Swift `\(x)` and a `%s` format
    /// verb alike, without a list of every language's placeholder syntax.
    pub fn client_url_split(raw: &str) -> (Option<String>, Option<String>) {
        let raw = raw.trim();
        let (host, path) = url_split(raw);
        let scheme_ok = raw.split_once("://").is_some_and(|(scheme, _)| {
            let mut cs = scheme.chars();
            cs.next().is_some_and(|c| c.is_ascii_alphabetic())
                && cs.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        });
        let host = host.filter(|h| {
            scheme_ok
                && h.chars().all(|c| {
                    c.is_ascii_alphanumeric()
                        || matches!(c, '.' | '-' | '_' | '~' | ':' | '[' | ']')
                })
        });
        (host, path)
    }

    /// CB.21: the ENDPOINT_HIT a channel CLIENT site carries (see
    /// [`cell_type::ENDPOINT_HIT`]): `{"via":"<via>","host":"<host>"}`, or
    /// `{"via":"<via>"}` when the site names no literal authority. The one
    /// writer of that payload, shared by the WebSocket and gRPC client
    /// extractors; `via` is the channel (`ws`, `grpc`). A hostless cell is
    /// written on purpose: it is what tells host narrowing that one site
    /// dials somewhere unknown.
    pub fn channel_hit_cell(via: &str, host: Option<&str>) -> Cell {
        let payload = match host {
            Some(h) => format!(r#"{{"via":"{}","host":"{}"}}"#, esc(via), esc(h)),
            None => format!(r#"{{"via":"{}"}}"#, esc(via)),
        };
        Cell {
            kind: cell_type::ENDPOINT_HIT,
            payload: CellPayload::Json(payload),
        }
    }

    /// Request path for a CLIENT call literal, for parsers that reconstruct
    /// interpolation themselves (TypeScript, Dart). Unlike [`url_to_path`] this
    /// never returns None: it is a normaliser, not a filter, so no endpoint is
    /// ever lost.
    ///
    /// ```text
    /// https://api.x/users?a=1  -> /users        scheme+host+query dropped
    /// /users?a=1#frag          -> /users        query+fragment dropped
    /// ${…}/users               -> ${…}/users    interpolated base kept for the
    ///                                           resolver's BaseFold tier
    /// auth/login               -> auth/login    relative hint, untouched
    /// <unresolved>             -> <unresolved>
    /// ```
    ///
    /// Returns `(path, changed)` so the caller can record the original literal
    /// as provenance (`"raw"` on ENDPOINT_HIT). A relative hint gains its
    /// leading `/` later, where the qname is built ([`canonical_http_path`],
    /// LB.5), not here.
    ///
    /// The query is cut BEFORE the host is looked for, so a `://` that only
    /// appears in a query value (`/login?next=https://x/y`) is never mistaken
    /// for a scheme. A scheme also has to come before the first `/`, so a path
    /// that embeds a URL (`/proxy/http://x/y`) is left alone. A literal that is
    /// ONLY a query (`?page=2`) is returned unchanged rather than as `""`,
    /// which `normalise_http_path` would read as the root route `/`.
    pub fn normalise_client_path(raw: &str) -> (String, bool) {
        if raw.is_empty() || raw == "<unresolved>" {
            return (raw.to_string(), false);
        }
        let (_, path) = split_authority(cut_query(raw));
        if path.is_empty() {
            return (raw.to_string(), false);
        }
        (path.to_string(), path != raw)
    }

    /// Stable ENDPOINT node id for a `(method, path)` — [`endpoint_qname`],
    /// the qname convention `HttpStackResolver::parse_endpoint_qname` reads.
    /// The path is canonicalised, so this is the id
    /// [`push_client_endpoint_with`] mints for the same `(method, path)`.
    pub fn endpoint_id(repo: RepoId, method: &str, path: &str) -> NodeId {
        let qname = endpoint_qname(method, path);
        NodeId::from_parts(GRAPH_TYPE, repo, node_kind::ENDPOINT, &qname)
    }

    /// Emit the ENDPOINT node (+ ENDPOINT_HIT cell) once per `(method, path)`,
    /// record it in nav, and push a CALLS edge from the enclosing node `from`.
    /// `seen` dedups the node across a file; the CALLS edge is pushed per call
    /// site. Returns the endpoint NodeId.
    pub fn push_client_endpoint(
        repo: RepoId,
        ep: &ClientEndpoint,
        from: NodeId,
        nodes: &mut Vec<Node>,
        edges: &mut Vec<Edge>,
        nav: &mut CodeNav,
        seen: &mut HashSet<NodeId>,
    ) -> NodeId {
        push_client_endpoint_with_raw(repo, ep, None, from, nodes, edges, nav, seen)
    }

    /// [`push_client_endpoint`] that also records `raw`, the call-site literal
    /// `ep.path` was normalised from (see [`normalise_client_path`]), as a
    /// `"raw"` field on the ENDPOINT_HIT payload. Pass `None` when the path was
    /// not rewritten; the payload is then byte-identical to
    /// `push_client_endpoint`'s.
    ///
    /// A separate entry point rather than a `ClientEndpoint` field so the
    /// parsers that build `ClientEndpoint` literals do not all have to change
    /// at once; a parser opts in by calling this.
    #[allow(clippy::too_many_arguments)]
    pub fn push_client_endpoint_with_raw(
        repo: RepoId,
        ep: &ClientEndpoint,
        raw: Option<&str>,
        from: NodeId,
        nodes: &mut Vec<Node>,
        edges: &mut Vec<Edge>,
        nav: &mut CodeNav,
        seen: &mut HashSet<NodeId>,
    ) -> NodeId {
        let extras = HitExtras { raw, host: None };
        push_client_endpoint_with(repo, ep, extras, from, nodes, edges, nav, seen)
    }

    /// [`push_client_endpoint`] with every optional ENDPOINT_HIT field (see
    /// [`HitExtras`]). The general entry point: the other two are this with
    /// fewer extras. The extras are written on the node's single cell, so
    /// the call site that first emits a `(method, path)` in a file decides
    /// them, the same as `file`/`line`/`col`.
    ///
    /// LB.5: `ep.path` is canonicalised ([`canonical_http_path`]) before the
    /// qname, the display name and the cell's `path` are built, so a relative
    /// `api/users` and a slashed `/api/users` are ONE node. When that rewrote
    /// the path and the caller recorded no `raw` of its own, the original is
    /// written as `raw` (A3.3's meaning: the literal the path was normalised
    /// from).
    #[allow(clippy::too_many_arguments)]
    pub fn push_client_endpoint_with(
        repo: RepoId,
        ep: &ClientEndpoint,
        extras: HitExtras<'_>,
        from: NodeId,
        nodes: &mut Vec<Node>,
        edges: &mut Vec<Edge>,
        nav: &mut CodeNav,
        seen: &mut HashSet<NodeId>,
    ) -> NodeId {
        let path = canonical_http_path(&ep.path);
        let qname = endpoint_qname(&ep.method, &path);
        let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::ENDPOINT, &qname);
        if seen.insert(id) {
            let extras = HitExtras {
                raw: extras
                    .raw
                    .or_else(|| (path != ep.path.as_str()).then_some(ep.path.as_str())),
                ..extras
            };
            nodes.push(Node {
                id,
                repo,
                confidence: ep.confidence,
                cells: vec![Cell {
                    kind: cell_type::ENDPOINT_HIT,
                    payload: CellPayload::Json(endpoint_hit_json(ep, &path, extras)),
                }],
            });
            let display = format!("{} {}", ep.method, path);
            nav.record(id, &display, &qname, node_kind::ENDPOINT, None);
        }
        edges.push(Edge {
            from,
            to: id,
            category: edge_category::CALLS,
            confidence: ep.confidence,
            cells: Vec::new(),
        });
        id
    }

    /// Best-effort source span for an HTTP node that carries no POSITION cell
    /// (A3.6). The reader for the two JSON payloads this crate's writers and
    /// its parsers produce:
    ///
    /// - `ENDPOINT_HIT` — every client language, via [`endpoint_hit_json`];
    /// - `ROUTE_METHOD` as JSON — parser-go's `route_method_cell` and
    ///   ts_routes. The other ROUTE_METHOD writers store the bare verb
    ///   (`Text("GET")`), which fails to parse and is skipped.
    ///
    /// ENDPOINT_HIT is tried before ROUTE_METHOD, and within a kind the FIRST
    /// usable cell wins — the same rule `locate_node` applies to a node that
    /// carries several POSITION cells (A2.8).
    ///
    /// Returns `(file, line0)`. `line0` is ZERO-indexed to match POSITION's
    /// `start_line` (`glia_doc::position_json` stores
    /// `start_position().row`), while these cells store `row + 1`. A `line`
    /// of 0 is ts_routes' "unknown" placeholder and yields `None`, as does a
    /// missing or non-integer `line`. A cell without a non-empty string
    /// `file` is skipped. Total: never panics, `None` on any failure.
    pub fn http_node_span(cells: &[Cell]) -> Option<(String, Option<i64>)> {
        [cell_type::ENDPOINT_HIT, cell_type::ROUTE_METHOD]
            .into_iter()
            .find_map(|kind| {
                cells
                    .iter()
                    .filter(|c| c.kind == kind)
                    .find_map(|c| span_of(&c.payload))
            })
    }

    /// `(file, line0)` from one ENDPOINT_HIT / ROUTE_METHOD payload.
    fn span_of(payload: &CellPayload) -> Option<(String, Option<i64>)> {
        let (CellPayload::Json(s) | CellPayload::Text(s)) = payload else {
            return None;
        };
        let v: serde_json::Value = serde_json::from_str(s).ok()?;
        let file = v
            .get("file")
            .and_then(serde_json::Value::as_str)
            .filter(|f| !f.is_empty())?;
        let line0 = match v.get("line").and_then(serde_json::Value::as_i64) {
            Some(n) if n >= 1 => Some(n - 1),
            _ => None,
        };
        Some((file.to_string(), line0))
    }
}

// ============================================================================
// Shared INFRA_RESOURCE identity (A13.5)
// ============================================================================
//
// Two crates emit INFRA_RESOURCE nodes — `parsers/code/extractors/src/iac.rs`
// (k8s manifests, docker-compose, Dockerfiles) and
// `parsers/code/terraform/src/lib.rs`. `IacResolver` pairs them on the VERBATIM
// qname, so they only ever join if they build the identical string. They did
// not: iac.rs emitted `infra:<kind>:<name>` while terraform emitted
// `<module_qname>::<type>.<name>`, which additionally embedded the file's module
// path and so could not even join terraform-to-terraform across repos. This
// module is the single definition of that string, so the shape cannot drift
// again by a copied `format!`.

pub mod infra {
    /// `infra:<kind>:<name>` — the single INFRA_RESOURCE qname shape.
    pub fn qname(kind: &str, name: &str) -> String {
        format!("infra:{kind}:{name}")
    }

    /// Provider-specific resource type → the canonical kind k8s/compose also
    /// use (the lower-cased `K8S_KINDS` vocabulary in `iac.rs`, plus `image`).
    ///
    /// Deliberately partial: anything absent is returned VERBATIM, so
    /// `aws_s3_bucket` stays `infra:aws_s3_bucket:data` — a uniform shape
    /// without a false k8s join. Two entries must never map onto one kind if a
    /// repo realistically declares both side by side under the same name: the
    /// qname is the node identity, so such a pair would COLLAPSE into one node.
    const ALIASES: &[(&str, &str)] = &[
        // Long-running workloads → `service`
        ("aws_ecs_service", "service"),
        ("aws_apprunner_service", "service"),
        ("google_cloud_run_service", "service"),
        ("azurerm_container_group", "service"),
        // Function-as-a-service → also `service` (the callable unit)
        ("aws_lambda_function", "service"),
        ("google_cloudfunctions_function", "service"),
        ("azurerm_function_app", "service"),
        // Container registries → `image`
        ("aws_ecr_repository", "image"),
        ("google_artifact_registry_repository", "image"),
        ("docker_image", "image"),
        // Terraform's own kubernetes provider → the same kinds the YAML path
        // emits, so a repo declaring an object in BOTH places pairs them.
        ("kubernetes_deployment", "deployment"),
        ("kubernetes_service", "service"),
        ("kubernetes_cron_job", "cronjob"),
        ("kubernetes_cron_job_v1", "cronjob"),
        ("kubernetes_config_map", "configmap"),
        ("kubernetes_secret", "secret"),
        ("aws_secretsmanager_secret", "secret"),
        ("kubernetes_ingress", "ingress"),
        ("kubernetes_ingress_v1", "ingress"),
        ("aws_lb", "ingress"),
    ];

    /// Fold a provider-specific resource type onto the canonical kind, or
    /// return it verbatim when it has no counterpart. Case-insensitive on the
    /// ASCII resource type (HCL types are ASCII by construction).
    pub fn canonical_kind(raw_type: &str) -> &str {
        for (from, to) in ALIASES {
            if raw_type.eq_ignore_ascii_case(from) {
                return to;
            }
        }
        raw_type
    }
}

// ============================================================================
// JVM family: the vocabulary Java and Kotlin share (A14.4)
// ============================================================================

/// What the Java and Kotlin parsers must agree on because the engine builds
/// them as ONE graph (the JVM family): a Kotlin Spring Data repository has to
/// name the DATA_ENTITY id a Java `@Entity` mints and back, and both must
/// refuse the same value types as injected beans. Each parser keeps its own
/// AST walk; only these strings are shared, so the two cannot drift apart by
/// a copied table. The annotation-route recipe lives beside the other route
/// helpers, in [`endpoint`](super::endpoint) (`jvm_annotation_routes`).
pub mod jvm {
    /// DATA_ENTITY flavor of a relational model (JPA `@Entity`) — the
    /// `<flavor>` segment of `data_entity:<flavor>:<Model>`, the vocabulary
    /// every DATA_ENTITY emitter shares and DbResolver buckets on.
    pub const SQL_FLAVOR: &str = "sql";
    /// DATA_ENTITY flavor of a document model (Spring Data Mongo `@Document`).
    pub const NOSQL_FLAVOR: &str = "nosql";

    /// Class annotations (simple names) that mark a persistent model, with
    /// the flavor each implies. Checked in order: `@Document` first, so a
    /// class carrying both is a Mongo document.
    pub const DATA_ENTITY_ANNOTATIONS: &[(&str, &str)] =
        &[("Document", NOSQL_FLAVOR), ("Entity", SQL_FLAVOR)];

    /// The DATA_ENTITY qname `data_entity:<flavor>:<Model>`, keyed on the
    /// model's simple name (the A13.1 identity rule) so a repository that
    /// names the bare type from any file of either language reaches the node
    /// its annotated class emits. The SURFACE name is kept verbatim —
    /// DbResolver folds it to its canonical form at index time.
    pub fn data_entity_qname(flavor: &str, model: &str) -> String {
        format!("data_entity:{flavor}:{model}")
    }

    /// Spring Data repository base interfaces whose first type parameter is
    /// the managed entity (`interface FooRepo extends JpaRepository<Foo, Long>`,
    /// `interface FooRepo : JpaRepository<Foo, Long>`), with the DATA_ENTITY
    /// flavor of that entity. The Mongo bases manage `@Document`s (`nosql`);
    /// every other base is read as managing a JPA `@Entity` (`sql`). The
    /// store-agnostic bases (`Repository`, `CrudRepository`, the Kotlin
    /// coroutine ones, …) carry no store of their own, so over a `@Document`
    /// declared in another file they name the `sql` id and the edge does not
    /// reach the `nosql` node.
    pub const REPOSITORY_BASES: &[(&str, &str)] = &[
        ("Repository", SQL_FLAVOR),
        ("CrudRepository", SQL_FLAVOR),
        ("JpaRepository", SQL_FLAVOR),
        ("PagingAndSortingRepository", SQL_FLAVOR),
        ("JpaSpecificationExecutor", SQL_FLAVOR),
        ("ReactiveCrudRepository", SQL_FLAVOR),
        ("ReactiveSortingRepository", SQL_FLAVOR),
        ("R2dbcRepository", SQL_FLAVOR),
        ("CoroutineCrudRepository", SQL_FLAVOR),
        ("CoroutineSortingRepository", SQL_FLAVOR),
        ("MongoRepository", NOSQL_FLAVOR),
        ("ReactiveMongoRepository", NOSQL_FLAVOR),
    ];

    /// The flavor of the entity a repository base manages, or `None` when
    /// `base` (a simple name) is not a Spring Data repository base.
    pub fn repository_flavor(base: &str) -> Option<&'static str> {
        REPOSITORY_BASES
            .iter()
            .find(|(b, _)| *b == base)
            .map(|(_, flavor)| *flavor)
    }

    /// Types that are never DI beans — skipped as injected dependencies. The
    /// Java denylist: primitives are excluded structurally by node kind, this
    /// covers the boxed / value types a `type_identifier` can name.
    pub fn is_non_injectable_type(name: &str) -> bool {
        matches!(
            name,
            "String"
                | "CharSequence"
                | "Object"
                | "Integer"
                | "Long"
                | "Double"
                | "Float"
                | "Short"
                | "Byte"
                | "Boolean"
                | "Character"
                | "Number"
                | "BigDecimal"
                | "BigInteger"
        )
    }

    /// Kotlin's additions to [`is_non_injectable_type`]: its value types
    /// (`Int`, `Char`, `Unit`, …, which are Java primitives and so never
    /// reach the Java list), the top / bottom types, and the collection
    /// interfaces a generic-stripped `List<Handler>` reduces to. Kotlin
    /// only: a Java `List` is a `generic_type` the Java walk never reads.
    pub fn is_kotlin_value_type(name: &str) -> bool {
        matches!(
            name,
            "Int"
                | "Long"
                | "Double"
                | "Float"
                | "Boolean"
                | "Char"
                | "Byte"
                | "Short"
                | "Unit"
                | "Any"
                | "Nothing"
                | "List"
                | "Map"
                | "Set"
                | "MutableList"
                | "MutableMap"
                | "MutableSet"
                | "Collection"
                | "Array"
        )
    }
}

// ============================================================================
// Dependency-injection fired-on counters (A7.0)
// ============================================================================

/// Grep-able fired-on marker for dependency-injection extraction: one stderr
/// line per repo build, printed by the engine only when some count is non-zero.
///
/// ```text
/// [di] injects refs: python=0 go=2 typescript=0 java=0 csharp=0 php=0 scala=0 (shapes: go-provider=2) repo=<label>
/// ```
///
/// * **Language tokens** count the INJECTS `UnresolvedRef`s the repo's parses
///   carry. The engine counts them off the `FileParse`s, so they stay right
///   for cache-served files and for emitters that never call [`record`].
///   Every [`LANGS`] entry is printed, zero or not, so ` go=[1-9]` is an
///   unambiguous grep. Any other language appears only when non-zero. The
///   spellings are the matrix row names (`bench/substrate-gap/matrix_vocab.py`),
///   so `go=` lines up with the `go/injects` cell.
/// * **Shape tokens** are what detectors reported through [`record`] while
///   THIS build ran. A file served from the parse cache ran no detector, so
///   shapes can undercount the language totals. A gap means "cached", not
///   "broken".
///
/// A parser calls `di_stats::record(DiShape::X)` beside each INJECTS ref it
/// pushes and adds nothing to this module. Other per-language fired-on lines
/// (`[recv]`, `[heritage]`) should copy the shape `[tag] label: lang=N …`,
/// with their whole fixed language set printed.
///
/// Diagnostics only: nothing here reaches the graph, the store, or any
/// ordering decision. The counters are process-global, like the engine's
/// `SuppressPanicHook`, so they assume one build at a time per process (true
/// today). Concurrent builds (parallel test threads) can only smear counts
/// between two stderr lines.
pub mod di_stats {
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// One INJECTS extraction shape. Discriminants are dense and index the
    /// counter bank. A new variant goes at the end of this enum AND of [`DiShape::ALL`].
    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    #[repr(usize)]
    pub enum DiShape {
        TsCtor = 0,
        TsInjectFn = 1,
        TsNestCtor = 2,
        JavaCtor = 3,
        JavaField = 4,
        JavaLombok = 5,
        JavaJsr330 = 6,
        CsCtor = 7,
        CsPrimaryCtor = 8,
        CsFromServices = 9,
        PyFastapiDepends = 10,
        PhpCtor = 11,
        GoProvider = 12,
        ScalaCtor = 13,
        KotlinCtor = 14,
        KotlinField = 15,
    }

    impl DiShape {
        /// Every shape, in discriminant order (`ALL[i] as usize == i`).
        pub const ALL: [DiShape; 16] = [
            Self::TsCtor,
            Self::TsInjectFn,
            Self::TsNestCtor,
            Self::JavaCtor,
            Self::JavaField,
            Self::JavaLombok,
            Self::JavaJsr330,
            Self::CsCtor,
            Self::CsPrimaryCtor,
            Self::CsFromServices,
            Self::PyFastapiDepends,
            Self::PhpCtor,
            Self::GoProvider,
            Self::ScalaCtor,
            Self::KotlinCtor,
            Self::KotlinField,
        ];

        /// Wire token printed in the `shapes:` group.
        pub const fn token(self) -> &'static str {
            match self {
                Self::TsCtor => "ts-ctor",
                Self::TsInjectFn => "ts-inject-fn",
                Self::TsNestCtor => "ts-nest-ctor",
                Self::JavaCtor => "java-ctor",
                Self::JavaField => "java-field",
                Self::JavaLombok => "java-lombok",
                Self::JavaJsr330 => "java-jsr330",
                Self::CsCtor => "csharp-ctor",
                Self::CsPrimaryCtor => "csharp-primary-ctor",
                Self::CsFromServices => "csharp-fromservices",
                Self::PyFastapiDepends => "py-fastapi-depends",
                Self::PhpCtor => "php-ctor",
                Self::GoProvider => "go-provider",
                Self::ScalaCtor => "scala-ctor",
                Self::KotlinCtor => "kotlin-ctor",
                Self::KotlinField => "kotlin-field",
            }
        }

        /// The language token this shape belongs to: a [`LANGS`] row, or
        /// `kotlin`. Kotlin is not a matrix row (the engine builds it into the
        /// Java graph), so like any non-[`LANGS`] language its refs token
        /// prints only when non-zero.
        pub const fn lang(self) -> &'static str {
            match self {
                Self::TsCtor | Self::TsInjectFn | Self::TsNestCtor => "typescript",
                Self::JavaCtor | Self::JavaField | Self::JavaLombok | Self::JavaJsr330 => "java",
                Self::CsCtor | Self::CsPrimaryCtor | Self::CsFromServices => "csharp",
                Self::PyFastapiDepends => "python",
                Self::PhpCtor => "php",
                Self::GoProvider => "go",
                Self::ScalaCtor => "scala",
                Self::KotlinCtor | Self::KotlinField => "kotlin",
            }
        }
    }

    /// Languages always printed, in matrix row order.
    pub const LANGS: [&str; 7] = [
        "python",
        "go",
        "typescript",
        "java",
        "csharp",
        "php",
        "scala",
    ];

    static COUNTS: [AtomicUsize; DiShape::ALL.len()] =
        [const { AtomicUsize::new(0) }; DiShape::ALL.len()];

    /// Count one INJECTS ref pushed by `shape`'s detector.
    pub fn record(shape: DiShape) {
        if let Some(c) = COUNTS.get(shape as usize) {
            c.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Zero every shape counter. The engine calls this before a build parses,
    /// so a stray `parse_one` outside a build cannot leak into its line.
    pub fn reset() {
        for c in &COUNTS {
            c.store(0, Ordering::Relaxed);
        }
    }

    /// Print the `[di]` line for one repo build, then zero the shape counters.
    /// `lang_refs` holds `(language, INJECTS refs)` pairs. Repeated languages
    /// are summed. Zeroing matters: a long-lived process (neuropil) or a test
    /// that builds twice (engine/tests/byte_identical.rs) must not accumulate.
    pub fn flush_marker(lang_refs: &[(&str, usize)], repo: &str) {
        let shapes = COUNTS.each_ref().map(|c| c.swap(0, Ordering::Relaxed));
        if let Some(line) = render(lang_refs, &shapes, repo) {
            eprintln!("{line}");
        }
    }

    /// The marker text, or `None` when every count is zero. `shapes` is in
    /// [`DiShape::ALL`] order.
    pub(crate) fn render(
        lang_refs: &[(&str, usize)],
        shapes: &[usize; DiShape::ALL.len()],
        repo: &str,
    ) -> Option<String> {
        let mut langs: Vec<(&str, usize)> = LANGS.iter().map(|l| (*l, 0)).collect();
        for &(lang, n) in lang_refs {
            match langs.iter_mut().find(|(l, _)| *l == lang) {
                Some(slot) => slot.1 += n,
                None => langs.push((lang, n)),
            }
        }
        let total = langs.iter().map(|(_, n)| n).sum::<usize>() + shapes.iter().sum::<usize>();
        if total == 0 {
            return None;
        }
        let lang_part: Vec<String> = langs
            .iter()
            .enumerate()
            .filter(|(i, (_, n))| *i < LANGS.len() || *n > 0)
            .map(|(_, (lang, n))| format!("{lang}={n}"))
            .collect();
        let shape_part: Vec<String> = DiShape::ALL
            .iter()
            .zip(shapes)
            .filter(|(_, n)| **n > 0)
            .map(|(s, n)| format!("{}={n}", s.token()))
            .collect();
        let shape_part = if shape_part.is_empty() {
            "none".to_string()
        } else {
            shape_part.join(" ")
        };
        Some(format!(
            "[di] injects refs: {} (shapes: {shape_part}) repo={repo}",
            lang_part.join(" ")
        ))
    }

    /// Read and zero one shape counter. Racy under parallel tests that record
    /// the same shape, so parser tests should assert on `refs` instead.
    #[doc(hidden)]
    pub fn take_for_test(shape: DiShape) -> usize {
        COUNTS
            .get(shape as usize)
            .map_or(0, |c| c.swap(0, Ordering::Relaxed))
    }
}

/// A6.2a fired_on marker for receiver-type inference, printed once per repo
/// build by the engine:
///
/// ```text
/// [recv] receiver-typed calls bound: csharp=N java=N typescript=N python=N ruby=N go=N dart=N rust=N (fields: csharp=F .. rust=F) repo=<label>
/// ```
///
/// * **bound** counts calls `resolve_calls` bound ONLY through a receiver's
///   type: a field's declared type
///   ([`CodeNav::field_types`](CodeNav::field_types)) or a local's
///   ([`CodeNav::local_types`](CodeNav::local_types), Rust since
///   LA.35a). The generic pass does not know its language, so it calls
///   [`record`](recv_stats::record) per bind and the engine
///   [`take`](recv_stats::take)s the count after each per-language build.
/// * **fields** counts the declared field types the parses carry, per
///   language, so a cache-served file counts too. `fields > 0` with `bound = 0`
///   means the carrier is populated but nothing resolved against it.
///
/// Every [`LANGS`](recv_stats::LANGS) entry is printed, zero or not, so
/// ` csharp=[1-9]` is an unambiguous grep. Any other language appears only
/// when non-zero. Diagnostics only, like [`di_stats`], but the count is PER
/// THREAD (LG.1c): [`record`](recv_stats::record),
/// [`reset`](recv_stats::reset) and [`take`](recv_stats::take) act on the
/// calling thread's counter. The engine builds a repo's languages
/// concurrently, each build on one thread (the graph crate spawns none), and
/// brackets it with `reset()` .. `take()` on that thread, so each language's
/// count holds its own binds and no other build's, in this repo or another.
pub mod recv_stats {
    use std::cell::Cell;

    /// Languages always printed: the matrix rows whose parser records
    /// receiver types (C# since A6.2a; java / typescript by A6.2b / A6.2c;
    /// python / ruby / go / dart by LA.23a-e; rust fields and locals by LA.35a).
    pub const LANGS: [&str; 8] =
        ["csharp", "java", "typescript", "python", "ruby", "go", "dart", "rust"];

    thread_local! {
        /// This thread's receiver-typed binds since its last [`reset`] /
        /// [`take`].
        static BOUND: Cell<usize> = const { Cell::new(0) };
    }

    /// Count one call bound through a receiver's declared type, on the
    /// calling thread's counter.
    pub fn record() {
        BOUND.with(|b| b.set(b.get() + 1));
    }

    /// Zero the calling thread's counter. The engine calls this before each
    /// language build, on the thread that runs it, so a stray `build_*`
    /// earlier on that thread cannot leak into it.
    pub fn reset() {
        BOUND.with(|b| b.set(0));
    }

    /// Read and zero the calling thread's counter: the binds of the build that
    /// just finished on this thread.
    pub fn take() -> usize {
        BOUND.with(|b| b.replace(0))
    }

    /// Print the `[recv]` line for one repo build when any count is non-zero.
    /// `bound` and `fields` hold `(language, count)` pairs; a repeated
    /// language is summed.
    pub fn flush_marker(bound: &[(&str, usize)], fields: &[(&str, usize)], repo: &str) {
        if let Some(line) = render(bound, fields, repo) {
            eprintln!("{line}");
        }
    }

    /// The marker text, or `None` when every count is zero.
    pub(crate) fn render(
        bound: &[(&str, usize)],
        fields: &[(&str, usize)],
        repo: &str,
    ) -> Option<String> {
        let bound = tally(bound);
        let fields = tally(fields);
        let total: usize = bound.iter().chain(&fields).map(|(_, n)| n).sum();
        if total == 0 {
            return None;
        }
        Some(format!(
            "[recv] receiver-typed calls bound: {} (fields: {}) repo={repo}",
            group(&bound),
            group(&fields)
        ))
    }

    /// [`LANGS`] in order (zero-filled), then any other language in first-seen
    /// order, with repeated languages summed.
    fn tally<'a>(pairs: &[(&'a str, usize)]) -> Vec<(&'a str, usize)> {
        let mut out: Vec<(&str, usize)> = LANGS.iter().map(|l| (*l, 0)).collect();
        for &(lang, n) in pairs {
            match out.iter_mut().find(|(l, _)| *l == lang) {
                Some(slot) => slot.1 += n,
                None => out.push((lang, n)),
            }
        }
        out
    }

    /// `lang=N` for every [`LANGS`] entry and every other non-zero language.
    fn group(tallied: &[(&str, usize)]) -> String {
        tallied
            .iter()
            .enumerate()
            .filter(|(i, (_, n))| *i < LANGS.len() || *n > 0)
            .map(|(_, (lang, n))| format!("{lang}={n}"))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

// ============================================================================
// Doc ingestion (Tier-4 seam)
// ============================================================================
//
// Source-agnostic doc record fed to the DOC_SECTION builder. Today the only
// producer is the repo `.md` file walk (FileDocSource in the engine); the
// Confluence / Notion / wiki adapters produce the SAME shape, so the builder +
// `link_doc_sections` are source-agnostic. `provenance` is carried but not yet
// emitted as a cell (that lands with the first external adapter) — so routing
// file docs through this type is byte-identical to the pre-seam path.

/// Where a doc came from. `File` = repo markdown (today's path).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DocSourceKind {
    File,
    Confluence,
    Notion,
    Wiki,
}

/// Provenance for a doc — the seam carries it; adapters populate url/container/
/// version and (later) emit it as a DOC_SECTION cell.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DocProvenance {
    pub kind: DocSourceKind,
    /// Canonical URL of the source page (external sources).
    pub url: Option<String>,
    /// Space key / database id / wiki name.
    pub container: Option<String>,
    /// Version number / etag for incremental sync.
    pub version: Option<String>,
}

impl DocProvenance {
    /// Provenance for a repo markdown file (today's default).
    pub fn file() -> Self {
        Self {
            kind: DocSourceKind::File,
            url: None,
            container: None,
            version: None,
        }
    }
}

/// One document to ingest: a logical path/id + its markdown text + provenance.
/// The DOC_SECTION builder chunks `text` by heading and keys nodes off
/// `rel_path`: a repo file by its directories + stem ([`dir_stem_qname`],
/// LB.12), an external record by its container + stem.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DocRecord {
    pub rel_path: String,
    pub text: String,
    pub provenance: DocProvenance,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dir_stem_qname_scopes_by_directory_and_stem() {
        assert_eq!(dir_stem_qname("services/orders/openapi.yaml"), "services::orders::openapi");
        assert_eq!(dir_stem_qname("services/billing/openapi.json"), "services::billing::openapi");
        // A root-level file is its stem alone: the pre-LB.12 qname, unchanged.
        assert_eq!(dir_stem_qname("openapi.json"), "openapi");
        assert_eq!(dir_stem_qname("README.md"), "README");
        assert_eq!(dir_stem_qname("docs/a/guide.md"), "docs::a::guide");
        assert_eq!(dir_stem_qname(".glia/scratch/spec.json"), ".glia::scratch::spec");
        // Only the last extension goes; a dotfile is its own stem.
        assert_eq!(dir_stem_qname("src/users.controller.ts"), "src::users.controller");
        assert_eq!(dir_stem_qname("svc/Dockerfile"), "svc::Dockerfile");
        assert_eq!(dir_stem_qname("cfg/.env"), "cfg::.env");
        // A backslash separator normalises, so a Windows path scopes the same.
        assert_eq!(dir_stem_qname(r"specs\004-x\contracts\openapi.json"), "specs::004-x::contracts::openapi");
        assert_eq!(dir_stem_qname(r"a.b\c.yaml"), "a.b::c");
    }

    #[test]
    fn line_of_counts_newlines_on_bytes() {
        let src = "a\nbé\nc";
        assert_eq!(line_of(src, 0), 0);
        assert_eq!(line_of(src, 1), 0, "the newline itself is still row 0");
        assert_eq!(line_of(src, 2), 1);
        // Byte 4 is inside the two-byte `é`: no panic, still row 1.
        assert_eq!(line_of(src, 4), 1);
        assert_eq!(line_of(src, src.len()), 2);
        assert_eq!(line_of(src, usize::MAX), 2, "past the end counts the whole source");
        assert_eq!(line_of("", 3), 0);
    }

    #[test]
    fn node_kind_names_resolve_and_cover() {
        // Every ALL entry resolves to its own name via name().
        for (id, n) in node_kind::ALL {
            assert_eq!(node_kind::name(*id), *n);
        }
        // Spot-check the ids that the old hardcoded tables missed (41/42/43).
        assert_eq!(node_kind::name(node_kind::REGION), "REGION");
        assert_eq!(node_kind::name(node_kind::DOC_SECTION), "DOC_SECTION");
        assert_eq!(node_kind::name(node_kind::STATE_VAR), "STATE_VAR");
        // Unregistered id falls back, never panics.
        assert_eq!(node_kind::name(NodeKindId(9999)), "UNKNOWN");
    }

    #[test]
    fn go_library_name_excludes_internal_and_stdlib() {
        // External third-party (domain first segment) → module path.
        assert_eq!(
            library_name("github.com::external::lib", "go"),
            Some("github.com/external/lib".to_string())
        );
        assert_eq!(
            library_name("github.com/external/lib", "go"),
            Some("github.com/external/lib".to_string())
        );
        // Internal import (go.mod prefix stripped to a repo-relative path) — WP-G:
        // must NOT leak into Symbol.imports.
        assert_eq!(library_name("internal::util", "go"), None);
        // Stdlib — no domain, excluded.
        assert_eq!(library_name("fmt", "go"), None);
        assert_eq!(library_name("net::http", "go"), None);
    }

    // ---- A16.4: intra-repo import filter -----------------------------------

    /// A FileParse declaring `decls` (kind, qname) and importing `imports`.
    /// A node's nav name is its last qname segment, as the parsers record it.
    fn decl_parse(decls: &[(NodeKindId, &str)], imports: Vec<ImportStmt>) -> FileParse {
        let mut fp = FileParse { imports, ..Default::default() };
        for (kind, qname) in decls {
            let id = NodeId::from_parts(GRAPH_TYPE, glia_core::RepoId(1), *kind, qname);
            let name = qname.rsplit("::").next().unwrap_or(qname);
            fp.nodes.push(Node {
                id,
                repo: glia_core::RepoId(1),
                confidence: glia_core::Confidence::Strong,
                cells: Vec::new(),
            });
            fp.nav.record(id, name, qname, *kind, None);
        }
        fp
    }

    // ---- LB.9b: MODULEs named by file name ---------------------------------

    #[test]
    fn bare_module_qname_strips_only_a_file_named_modules_extension() {
        assert_eq!(bare_module_qname("api::user.py", "user"), Some("api::user".to_string()));
        // A normal module: its last segment is its name.
        assert_eq!(bare_module_qname("api::user", "user"), None);
        // A non-code MODULE (LB.9a): named by its whole file name.
        assert_eq!(bare_module_qname("api::user.proto", "user.proto"), None);
        // A dotted stem keeps its dots; only the one extension goes.
        assert_eq!(
            bare_module_qname("api::user.test.ts", "user.test"),
            Some("api::user.test".to_string())
        );
        assert_eq!(bare_module_qname("x.py", "x"), Some("x".to_string()));
        // Two extensions past the name, or no name at all: not a file-named MODULE.
        assert_eq!(bare_module_qname("api::user.test.ts", "user"), None);
        assert_eq!(bare_module_qname("api::.py", ""), None);
        assert_eq!(bare_module_qname("api::users.py", "user"), None);
    }

    #[test]
    fn a_file_named_module_declares_its_bare_path() {
        // `api/user.py` beside `api/user.ts`: MODULE `api::user.py`, nav name
        // `user`. Its local paths are the bare form's, never `py`.
        let id = NodeId::from_parts(
            GRAPH_TYPE,
            glia_core::RepoId(1),
            node_kind::MODULE,
            "api::user.py",
        );
        let mut fp = FileParse::default();
        fp.nav.record(id, "user", "api::user.py", node_kind::MODULE, None);
        let mut local = LocalModuleIndex::default();
        local.add_parse(&fp);
        assert!(local.is_local_path("api.user"));
        assert!(local.is_local_path("user"));
        assert!(!local.is_local_path("py"), "the extension is not a local module");
        assert!(!local.is_local_path("user.py"));
    }

    // ---- LB.13: which same-stem sibling an importer's language loads -------

    #[test]
    fn same_stem_order_is_the_importers_own_resolution_order() {
        // TypeScript tries its own files first, JavaScript its own.
        for ext in ["ts", "tsx"] {
            assert_eq!(same_stem_order(ext).first(), Some(&"ts"), "{ext}");
        }
        for ext in ["js", "jsx", "vue"] {
            assert_eq!(same_stem_order(ext).first(), Some(&"js"), "{ext}");
            assert!(same_stem_order(ext).contains(&"ts"), "{ext}: allowJs / bundlers load .ts");
        }
        // A JVM Clojure load never offers the ClojureScript file, and back.
        assert_eq!(same_stem_order("clj"), ["clj", "cljc"]);
        assert_eq!(same_stem_order("cljs"), ["cljs", "cljc"]);
        assert!(!same_stem_order("clj").contains(&"cljs"));
        assert!(!same_stem_order("cljs").contains(&"clj"));
        // No file-import rule: a .cljc loads a different sibling per platform;
        // Java / Kotlin import classes, Elixir aliases modules, Terraform
        // modules are directories, C/C++ includes name the file.
        for ext in ["cljc", "java", "kt", "ex", "exs", "tf", "hcl", "cpp", "h", "py", ""] {
            assert!(same_stem_order(ext).is_empty(), "{ext}");
        }
    }

    fn index_of(decls: &[(NodeKindId, &str)]) -> LocalModuleIndex {
        let mut local = LocalModuleIndex::default();
        local.add_parse(&decl_parse(decls, Vec::new()));
        local
    }

    fn module_import(path: &str) -> ImportStmt {
        ImportStmt {
            from_module: String::new(),
            target: ImportTarget::Module { path: path.to_string(), alias: None },
            line: 0,
        }
    }

    fn symbol_import(module: &str, name: &str) -> ImportStmt {
        ImportStmt {
            from_module: String::new(),
            target: ImportTarget::Symbol {
                module: module.to_string(),
                name: name.to_string(),
                alias: None,
                level: 0,
            },
            line: 0,
        }
    }

    fn imports_payloads(fp: &FileParse) -> Vec<Vec<String>> {
        fp.nodes
            .iter()
            .map(|n| {
                n.cells
                    .iter()
                    .filter(|c| c.kind == cell_type::IMPORTS)
                    .map(|c| match &c.payload {
                        CellPayload::Json(s) | CellPayload::Text(s) => s.clone(),
                        CellPayload::Bytes(_) => String::new(),
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn local_index_matches_prefix_and_suffix() {
        let local = index_of(&[(node_kind::MODULE, "src::snapshot")]);
        assert!(local.is_local_path("snapshot"), "suffix");
        assert!(local.is_local_path("src"), "prefix");
        assert!(local.is_local_path("src::snapshot"), "whole qname");
        assert!(local.is_local_path("src.snapshot") && local.is_local_path("src/snapshot"), "normalised");
        assert!(!local.is_local_path("snap"), "segment-aligned, not substring");
        assert!(!local.is_local_path(""), "empty is never local");
        // Only MODULE / PACKAGE qnames are paths: a CLASS is not a namespace.
        let local = index_of(&[(node_kind::CLASS, "models::Widget")]);
        assert!(!local.is_local_path("models"));
    }

    #[test]
    fn local_index_symbol_is_ambiguity_safe() {
        let one = index_of(&[(node_kind::CLASS, "a::Helper")]);
        assert!(one.is_local_symbol("Helper"));
        let two = index_of(&[(node_kind::CLASS, "a::Helper"), (node_kind::CLASS, "b::Helper")]);
        assert!(!two.is_local_symbol("Helper"), "two declarations: ambiguous");
        // Java's Helper.java declares MODULE `Helper` AND CLASS `Helper::Helper`:
        // one declaration, two nodes. It must still count as unique.
        let java = index_of(&[(node_kind::MODULE, "Helper"), (node_kind::CLASS, "Helper::Helper")]);
        assert!(java.is_local_symbol("Helper"));
        assert!(!java.is_local_symbol("Other"));
    }

    /// LA.1b: the Rust parser emits raw `use` paths, so a sibling workspace
    /// crate (`use reexp_lib::..`) and a type path (`use Kind::*`) reach the
    /// filter; neither is a dependency, while an external crate is.
    #[test]
    fn rust_raw_paths_keep_external_crates_only() {
        let mut local = index_of(&[
            (node_kind::MODULE, "app::src::main"),
            (node_kind::MODULE, "app::src::resolvers::mod"),
        ]);
        local.add_local_crate("reexp_lib");
        local.add_local_crate("");
        assert!(local.is_local_path("reexp_lib"));
        assert!(!local.is_local_path("serde"));
        let imports = vec![
            symbol_import("reexp_lib", "generate_one"),
            symbol_import("serde", "Serialize"),
            symbol_import("std::io", "Read"),
            symbol_import("crate::util", "helper"),
            symbol_import("super", "*"),
            symbol_import("Kind", "*"),
            module_import("self::m"),
            module_import("tokio"),
            symbol_import("resolvers", "HttpResolver"),
        ];
        assert_eq!(
            library_names_filtered(&imports, "rust", &local),
            vec!["serde".to_string(), "tokio".to_string()]
        );
        assert_eq!(library_name("Ordering::Less", "rust"), None, "a type, never a crate");
    }

    #[test]
    fn filtered_drops_rust_sibling_module() {
        // `use snapshot::Page;` — a path through a sibling module.
        let local = index_of(&[(node_kind::MODULE, "src::snapshot"), (node_kind::MODULE, "src::confluence_rest")]);
        let imports = vec![symbol_import("snapshot", "Page")];
        assert_eq!(library_names(&imports, "rust"), vec!["snapshot".to_string()], "the leak this fixes");
        assert!(library_names_filtered(&imports, "rust", &local).is_empty());
    }

    #[test]
    fn filtered_drops_python_own_package() {
        let local = index_of(&[(node_kind::MODULE, "myapp::auth"), (node_kind::MODULE, "myapp::users")]);
        let imports = vec![symbol_import("myapp.users", "User"), module_import("requests")];
        assert_eq!(library_names_filtered(&imports, "python", &local), vec!["requests".to_string()]);
    }

    /// A6.8: a TS specifier a tsconfig `paths` key matches names an in-repo
    /// module, so it leaves the library cell, while a real package stays. The
    /// match is on the specifier: `~/lib/polyfills` is the library `~`.
    #[test]
    fn filtered_drops_ts_path_aliases() {
        let mut local = index_of(&[(node_kind::MODULE, "src::app::core::auth.service")]);
        for key in ["@core/*", "@env", "~/*", "*", ""] {
            local.add_alias_prefix(key);
        }
        let imports = vec![
            symbol_import("@core/auth.service", "AuthService"),
            symbol_import("@env", "environment"),
            module_import("~/lib/polyfills"),
            symbol_import("@angular/core", "Injectable"),
            symbol_import("@environment/prod", "config"),
            module_import("rxjs"),
        ];
        assert_eq!(
            library_names(&imports, "typescript"),
            ["@angular/core", "@core/auth.service", "@env", "@environment/prod", "rxjs", "~"],
            "the leak this fixes"
        );
        assert_eq!(
            library_names_filtered(&imports, "typescript", &local),
            ["@angular/core", "@environment/prod", "rxjs"],
            "an exact key is matched whole: `@environment/prod` is not `@env`"
        );
        assert!(local.is_alias_import("\"@core/x\""), "quotes are the parser's, not the key's");
        assert!(!local.is_alias_import("lodash"), "the catch-all `*` declared nothing");
        assert!(!local.is_alias_import(""));
        // Only the TS family resolves through tsconfig.
        let py = vec![module_import("@env")];
        assert_eq!(library_names_filtered(&py, "python", &local), ["@env"]);
    }

    #[test]
    fn filtered_drops_java_own_package_by_symbol() {
        // java-spring-imports: no PACKAGE node exists, so only the uniquely
        // declared class places `com.example.util` in the repo.
        let local = index_of(&[
            (node_kind::MODULE, "Main"),
            (node_kind::CLASS, "Main::Main"),
            (node_kind::MODULE, "Helper"),
            (node_kind::CLASS, "Helper::Helper"),
        ]);
        let imports = vec![symbol_import("com::example::util", "Helper")];
        assert!(library_names_filtered(&imports, "java", &local).is_empty());
        // An external class from a package the repo does not declare stays.
        let imports = vec![symbol_import("org::slf4j", "Logger")];
        assert_eq!(library_names_filtered(&imports, "java", &local), vec!["org::slf4j".to_string()]);
    }

    #[test]
    fn filtered_keeps_external() {
        let local = index_of(&[(node_kind::MODULE, "cmd::server"), (node_kind::PACKAGE, "internal::util")]);
        let imports = vec![module_import("github.com::external::lib")];
        assert_eq!(
            library_names_filtered(&imports, "go", &local),
            vec!["github.com/external/lib".to_string()]
        );
    }

    #[test]
    fn filtered_drops_declared_namespaces_and_local_headers() {
        // PHP / C#: the declared namespace is a PACKAGE node.
        let local = index_of(&[(node_kind::PACKAGE, "App::Services"), (node_kind::PACKAGE, "Shop::Services")]);
        let php = vec![symbol_import("App::Services", "Greeter"), symbol_import("GuzzleHttp", "Client")];
        assert_eq!(library_names_filtered(&php, "php", &local), vec!["GuzzleHttp".to_string()]);
        let cs = vec![symbol_import("Shop", "Services"), symbol_import("System::Threading", "Tasks")];
        assert_eq!(library_names_filtered(&cs, "csharp", &local), vec!["System::Threading".to_string()]);
        // Elixir: `defmodule MyApp.Repo` in ex/repo.ex — the dotted declared
        // name and its parents are declared namespaces.
        let local = index_of(&[(node_kind::PACKAGE, "ex::repo::MyApp.Repo"), (node_kind::PACKAGE, "ex::web::MyAppWeb.Router")]);
        let ex = vec![module_import("MyApp.Repo"), module_import("MyAppWeb"), module_import("Plug.Conn")];
        assert_eq!(library_names_filtered(&ex, "elixir", &local), vec!["Plug.Conn".to_string()]);
        // C: `#include "mathutil.h"` next to c/mathutil.h (MODULE `c::mathutil`).
        let local = index_of(&[(node_kind::MODULE, "c::mathutil"), (node_kind::MODULE, "c::main")]);
        let c = vec![module_import("mathutil.h"), module_import("zlib.h")];
        assert_eq!(library_names_filtered(&c, "c_cpp", &local), vec!["zlib.h".to_string()]);
    }

    #[test]
    fn symbol_evidence_is_gated_to_namespace_languages() {
        // A one-app Django project has exactly one `models.py`, so `models` is
        // uniquely declared — but the `django.db` models import is still a
        // dependency on django. Same for TS `import { User } from 'firebase/auth'`
        // beside the repo's own `interface User`.
        let local = index_of(&[(node_kind::MODULE, "shop::models"), (node_kind::INTERFACE, "src::types::User")]);
        let py = vec![symbol_import("django.db", "models")];
        assert_eq!(library_names_filtered(&py, "python", &local), vec!["django".to_string()]);
        let ts = vec![symbol_import("firebase/auth", "User")];
        assert_eq!(library_names_filtered(&ts, "typescript", &local), vec!["firebase".to_string()]);
    }

    #[test]
    fn filtered_caps_after_filtering() {
        // Twelve local modules sort before the one real dependency; capping
        // before filtering would report none of the file's dependencies.
        let decls: Vec<(NodeKindId, String)> =
            (0..12).map(|i| (node_kind::MODULE, format!("a{i:02}::m"))).collect();
        let decl_refs: Vec<(NodeKindId, &str)> = decls.iter().map(|(k, q)| (*k, q.as_str())).collect();
        let local = index_of(&decl_refs);
        let mut imports: Vec<ImportStmt> = (0..12).map(|i| module_import(&format!("a{i:02}.m"))).collect();
        imports.push(module_import("zzz"));
        assert_eq!(library_names(&imports, "python").len(), 10);
        assert_eq!(library_names_filtered(&imports, "python", &local), vec!["zzz".to_string()]);
    }

    #[test]
    fn attach_filtered_is_idempotent() {
        let mut fp = decl_parse(
            &[(node_kind::MODULE, "myapp::auth"), (node_kind::FUNCTION, "myapp::auth::login")],
            vec![module_import("myapp.users"), module_import("requests")],
        );
        let mut local = LocalModuleIndex::default();
        local.add_parse(&fp);
        // A POSITION after the raw cell: the rewrite must keep the cell's slot.
        attach_imports_cell(&mut fp, "python");
        for n in &mut fp.nodes {
            n.cells.push(Cell { kind: cell_type::POSITION, payload: CellPayload::Json("{}".into()) });
        }
        assert_eq!(imports_payloads(&fp)[0], vec![r#"["myapp","requests"]"#.to_string()], "raw cell");

        assert_eq!(attach_imports_cell_filtered(&mut fp, "python", &local), (1, 1));
        assert_eq!(attach_imports_cell_filtered(&mut fp, "python", &local), (1, 1));
        for (n, payloads) in fp.nodes.iter().zip(imports_payloads(&fp)) {
            assert_eq!(payloads, vec![r#"["requests"]"#.to_string()], "exactly one filtered cell");
            let kinds: Vec<CellTypeId> = n.cells.iter().map(|c| c.kind).collect();
            assert_eq!(kinds, vec![cell_type::IMPORTS, cell_type::POSITION], "slot kept");
        }
        // A node that never had the cell gains exactly one.
        let mut bare = decl_parse(&[(node_kind::MODULE, "x")], vec![module_import("requests")]);
        assert_eq!(attach_imports_cell_filtered(&mut bare, "python", &local), (1, 0));
        assert_eq!(imports_payloads(&bare), vec![vec![r#"["requests"]"#.to_string()]]);
    }

    #[test]
    fn edge_and_cell_names_resolve() {
        for (id, n) in edge_category::ALL {
            assert_eq!(edge_category::name(*id), *n);
        }
        assert_eq!(edge_category::name(edge_category::IMPLEMENTS), "IMPLEMENTS");
        for (id, n) in cell_type::ALL {
            assert_eq!(cell_type::name(*id), *n);
        }
        assert_eq!(cell_type::name(cell_type::POSITION), "POSITION");
    }

    /// W0.3 guard — `registry_ids_are_unique_and_contiguous` (fired_on marker).
    ///
    /// Ids are allocated centrally in this file and locked forever. Two packets
    /// planned in parallel both reaching for "the next free id" is the exact
    /// failure this catches: a duplicate makes `name()`'s `.iter().find()`
    /// return whichever row comes first — a silently wrong label handed to
    /// pyo3 `kind_names()` / `category_names()` / `cell_type_names()`.
    ///
    /// Each table must be exactly `1..=len` — no duplicate, no gap — so a
    /// duplicate allocation fails twice over (the dup, and the hole it leaves).
    /// The pinned counts must be bumped deliberately when an id is reserved.
    #[test]
    fn registry_ids_are_unique_and_contiguous() {
        fn check(what: &str, ids: Vec<u32>, names: Vec<&str>, expect_len: usize) {
            assert_eq!(
                ids.len(),
                expect_len,
                "{what}: ALL length changed - bump the pinned count deliberately"
            );
            let mut sorted = ids.clone();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(sorted.len(), ids.len(), "{what}: duplicate id in ALL");
            let expected: Vec<u32> = (1..=ids.len() as u32).collect();
            assert_eq!(sorted, expected, "{what}: ids must be a gap-free 1..=len range");
            let mut n = names.clone();
            n.sort_unstable();
            n.dedup();
            assert_eq!(n.len(), names.len(), "{what}: duplicate name in ALL");
        }

        check(
            "node_kind",
            node_kind::ALL.iter().map(|(id, _)| id.0).collect(),
            node_kind::ALL.iter().map(|(_, n)| *n).collect(),
            49,
        );
        check(
            "edge_category",
            edge_category::ALL.iter().map(|(id, _)| id.0).collect(),
            edge_category::ALL.iter().map(|(_, n)| *n).collect(),
            36,
        );
        check(
            "cell_type",
            cell_type::ALL.iter().map(|(id, _)| id.0).collect(),
            cell_type::ALL.iter().map(|(_, n)| *n).collect(),
            25,
        );
    }

    // ------------------------------------------------------------------
    // A4.0 — shared endpoint path helpers. These four are the contract eight
    // later route-composition packets build on, so the edge cases are pinned
    // here rather than re-derived per parser.
    // ------------------------------------------------------------------

    /// A3.5 — the receiver test both route scanners (Dart shelf, ts_routes)
    /// share. Server routers stay routes; client receivers do not, including
    /// the documented false negatives (`api` router, `audio*` substring).
    #[test]
    fn http_client_receiver_classifies_client_and_server_names() {
        for client in ["http", "dio", "_dio", "httpClient", "apiClient", "_client", "restClient", "api", "userApi", "HTTP"] {
            assert!(endpoint::is_http_client_receiver(client), "{client} is a client");
        }
        for server in ["app", "router", "server", "r", "v1", "fastify", "hono", "koaRouter", ""] {
            assert!(!endpoint::is_http_client_receiver(server), "{server} is not a client");
        }
        // Deliberate false negatives, pinned so nobody "fixes" one silently.
        assert!(endpoint::is_http_client_receiver("audioRouter"));
        assert!(!endpoint::is_http_client_receiver("httpService"));
    }

    #[test]
    fn ident_before_reads_the_call_receiver() {
        let at = |s: &str, needle: &str| s.find(needle).unwrap_or(0);
        let s = "return this.http.get('/users');";
        assert_eq!(endpoint::ident_before(s, at(s, ".get(")), "http");
        let s = "app.get('/x', h)";
        assert_eq!(endpoint::ident_before(s, at(s, ".get(")), "app");
        let s = "$http.get('/x')";
        assert_eq!(endpoint::ident_before(s, at(s, ".get(")), "http");
        let s = "request(app).get('/x')";
        assert_eq!(endpoint::ident_before(s, at(s, ".get(")), "");
        let s = "router\n  ..get('/x')";
        assert_eq!(endpoint::ident_before(s, at(s, ".get(")), "");
        let s = "_dio .post('/x')";
        assert_eq!(endpoint::ident_before(s, at(s, ".post(")), "_dio");
        // Out-of-range and mid-char offsets never panic.
        assert_eq!(endpoint::ident_before("app", 99), "app");
        assert_eq!(endpoint::ident_before("é.get(", 1), "");
        assert_eq!(endpoint::ident_before("", 0), "");
    }

    /// An empty prefix is a pure pass-through: `join_path` must NOT force a
    /// leading `/`, or go's chi/gin route qnames move (graph/tests/
    /// go_smoke_graph.rs). Forcing the slash is `abs_path`'s separate job.
    #[test]
    fn join_path_empty_prefix_passthrough() {
        assert_eq!(endpoint::join_path("", "users"), "users");
        assert_eq!(endpoint::join_path("", "/users"), "/users");
        assert_eq!(endpoint::join_path("", "/"), "/");
        assert_eq!(endpoint::join_path("", ""), "");
    }

    /// A trailing `/` run on the prefix and a leading `/` on the path never
    /// double up, and `path == "/"` is the group's own index route.
    #[test]
    fn join_path_no_double_slash() {
        assert_eq!(endpoint::join_path("/api/", "/users"), "/api/users");
        assert_eq!(endpoint::join_path("/api", "/users"), "/api/users");
        assert_eq!(endpoint::join_path("/api", "users"), "/api/users");
        assert_eq!(endpoint::join_path("/api/", "users"), "/api/users");
        assert_eq!(endpoint::join_path("/api///", "/users"), "/api/users");
        // A group's index route collapses to the group itself, not `/api/`.
        assert_eq!(endpoint::join_path("/api", "/"), "/api");
        // ...and the prefix is returned verbatim in that branch, trailing
        // slash included — pinned because it is the one asymmetry.
        assert_eq!(endpoint::join_path("/api/", "/"), "/api/");
        // A relative prefix stays relative.
        assert_eq!(endpoint::join_path("api", "users"), "api/users");
    }

    /// Nested elixir `scope` stacks compose left-to-right: every entry gets
    /// exactly one leading `/`, its trailing `/` run trimmed, empty entries
    /// skipped, and the result is always absolute.
    #[test]
    fn join_scope_nested() {
        assert_eq!(
            endpoint::join_scope(&["/api".into(), "v1".into()], "users"),
            "/api/v1/users"
        );
        assert_eq!(endpoint::join_scope(&[], "/users"), "/users");
        assert_eq!(endpoint::join_scope(&[], "users"), "/users");
        assert_eq!(endpoint::join_scope(&["api".into()], "/users"), "/api/users");
        assert_eq!(
            endpoint::join_scope(&["/api/".into(), String::new(), "v1/".into()], "/users"),
            "/api/v1/users"
        );
        // Degenerate: nothing at all still yields the absolute root.
        assert_eq!(endpoint::join_scope(&[], ""), "/");
    }

    fn anns(list: &[(&str, Option<&str>)]) -> Vec<(String, Option<String>)> {
        list.iter()
            .map(|(n, a)| (n.to_string(), a.map(str::to_string)))
            .collect()
    }

    /// A14.4: the JVM annotation-route recipe both the Java and the Kotlin
    /// parser call. Pinned case by case against the A4.4 behaviour it was
    /// hoisted from, so a Java ROUTE name cannot move with the hoist.
    #[test]
    fn jvm_annotation_routes_compose_like_a4_4() {
        use endpoint::{compose_route_path, jvm_annotation_routes, jvm_route_prefix};
        // Class level: `@RestController @RequestMapping("/api")` — the prefix
        // itself is an ANY route; the stereotype maps nothing.
        let class = anns(&[("RestController", None), ("RequestMapping", Some("/api"))]);
        assert_eq!(jvm_route_prefix(&class), "/api");
        assert_eq!(jvm_annotation_routes(&class, ""), vec![("ANY", "/api".to_string())]);
        // Method level composes onto the prefix; a leading `/` does not reset it.
        let get = anns(&[("GetMapping", Some("/users/{id}"))]);
        assert_eq!(
            jvm_annotation_routes(&get, "/api"),
            vec![("GET", "/api/users/{id}".to_string())]
        );
        assert_eq!(jvm_annotation_routes(&get, ""), vec![("GET", "/users/{id}".to_string())]);
        // A marker maps the prefix; with no prefix it names nothing.
        let post = anns(&[("PostMapping", None)]);
        assert_eq!(jvm_annotation_routes(&post, "api"), vec![("POST", "/api".to_string())]);
        assert!(jvm_annotation_routes(&post, "").is_empty());
        // Micronaut `@Controller("/m")` + `@Get("/x")`.
        let micronaut = anns(&[("Controller", Some("/m"))]);
        assert_eq!(jvm_route_prefix(&micronaut), "/m");
        assert_eq!(
            jvm_annotation_routes(&anns(&[("Get", Some("/x"))]), "/m"),
            vec![("GET", "/m/x".to_string())]
        );
        // JAX-RS: `@GET @Path("/{id}")` under `@Path("/r")`; a bare `@DELETE`
        // maps the resource root; `@Path` alone is ANY.
        let jaxrs = anns(&[("GET", None), ("Path", Some("/{id}"))]);
        assert_eq!(jvm_annotation_routes(&jaxrs, "/r"), vec![("GET", "/r/{id}".to_string())]);
        assert_eq!(
            jvm_annotation_routes(&anns(&[("DELETE", None)]), "/r"),
            vec![("DELETE", "/r".to_string())]
        );
        assert_eq!(
            jvm_annotation_routes(&anns(&[("Path", Some("/r"))]), ""),
            vec![("ANY", "/r".to_string())]
        );
        // An empty prefix argument is no prefix.
        assert_eq!(jvm_route_prefix(&anns(&[("RequestMapping", Some(""))])), "");
        assert_eq!(compose_route_path("/api/", "users"), "/api/users");
        assert_eq!(compose_route_path("api", ""), "/api");
    }

    #[test]
    fn jvm_vocabulary_is_shared_by_both_parsers() {
        assert_eq!(jvm::data_entity_qname(jvm::SQL_FLAVOR, "User"), "data_entity:sql:User");
        assert_eq!(jvm::DATA_ENTITY_ANNOTATIONS[0], ("Document", jvm::NOSQL_FLAVOR));
        assert_eq!(jvm::repository_flavor("JpaRepository"), Some("sql"));
        assert_eq!(jvm::repository_flavor("ReactiveMongoRepository"), Some("nosql"));
        assert_eq!(jvm::repository_flavor("CoroutineCrudRepository"), Some("sql"));
        assert_eq!(jvm::repository_flavor("UserRepository"), None);
        assert!(jvm::is_non_injectable_type("String"));
        assert!(!jvm::is_non_injectable_type("Int"), "Java list stays Java's");
        assert!(jvm::is_kotlin_value_type("Int"));
        assert!(!jvm::is_kotlin_value_type("UserService"));
    }

    #[test]
    fn jvm_client_verb_tables_are_shared_by_both_parsers() {
        // A14.6: the RestTemplate / HttpMethod tables the Java and Kotlin
        // client arms both read.
        use endpoint::{http_method_ref_verb, rest_template_verb};
        assert_eq!(rest_template_verb("getForObject"), Some("GET"));
        assert_eq!(rest_template_verb("postForLocation"), Some("POST"));
        assert_eq!(rest_template_verb("headForHeaders"), Some("HEAD"));
        assert_eq!(rest_template_verb("put"), Some("PUT"));
        assert_eq!(rest_template_verb("delete"), Some("DELETE"));
        assert_eq!(rest_template_verb("get"), None, "WebClient's verb, not RestTemplate's");
        assert_eq!(rest_template_verb("exchange"), None);
        assert_eq!(http_method_ref_verb("HttpMethod.GET"), Some("GET"));
        assert_eq!(http_method_ref_verb("org.springframework.http.HttpMethod.patch"), Some("PATCH"));
        assert_eq!(http_method_ref_verb("HttpMethod.valueOf(m)"), None);
        assert_eq!(http_method_ref_verb("\"/users\""), None);
    }

    #[test]
    fn di_stats_render_kotlin_shapes_under_the_kotlin_token() {
        use di_stats::{DiShape, render};
        let mut shapes = [0usize; 16];
        shapes[DiShape::KotlinCtor as usize] = 1;
        shapes[DiShape::KotlinField as usize] = 1;
        assert_eq!(
            render(&[("kotlin", 2)], &shapes, "k").as_deref(),
            Some(
                "[di] injects refs: python=0 go=0 typescript=0 java=0 csharp=0 php=0 scala=0 kotlin=2 \
                 (shapes: kotlin-ctor=1 kotlin-field=1) repo=k"
            )
        );
    }

    /// `abs_path` is the single place a route template is forced to exactly
    /// one leading `/`. It must never trim a trailing one — `normalise_http_path`
    /// in glia-graph already does that, and doing it twice would move
    /// existing route qnames.
    #[test]
    fn abs_path_forces_single_leading_slash() {
        assert_eq!(endpoint::abs_path("api/users"), "/api/users");
        assert_eq!(endpoint::abs_path("//x"), "/x");
        assert_eq!(endpoint::abs_path(""), "/");
        // Already absolute → unchanged.
        assert_eq!(endpoint::abs_path("/api/users"), "/api/users");
        // Leading runs collapse; interior doubles are left alone.
        assert_eq!(endpoint::abs_path("////api//users"), "/api//users");
        // Whitespace-only / slash-only degenerate to the root.
        assert_eq!(endpoint::abs_path("   "), "/");
        assert_eq!(endpoint::abs_path("/"), "/");
        assert_eq!(endpoint::abs_path("///"), "/");
        // Trailing slash is PRESERVED.
        assert_eq!(endpoint::abs_path("/api/"), "/api/");
        assert_eq!(endpoint::abs_path("api/"), "/api/");
        // Surrounding whitespace is trimmed.
        assert_eq!(endpoint::abs_path("  /api  "), "/api");
    }

    /// LB.5 — the canonical HTTP qname path: one leading `/` added to a
    /// relative literal; every shape that is already canonical, or is a
    /// placeholder, comes back byte-identical (borrowed, not rebuilt).
    #[test]
    fn canonical_http_path_adds_one_slash_and_never_moves_a_correct_path() {
        use std::borrow::Cow;
        let c = endpoint::canonical_http_path;
        assert_eq!(c("api"), "/api");
        assert_eq!(c("api/users"), "/api/users");
        assert_eq!(c("  a "), "/a");
        assert_eq!(c("protected/x/"), "/protected/x/");
        // Already canonical or exempt: untouched — `abs_path` would have
        // collapsed `//x` and trimmed ` /x `'s trailing space.
        for same in [
            "/x",
            "//x",
            "/x ",
            "/",
            "${…}/u",
            "${…}",
            "",
            "<unresolved>",
        ] {
            assert!(
                matches!(c(same), Cow::Borrowed(s) if s == same),
                "{same:?} moved"
            );
            assert!(endpoint::is_canonical_http_path(same), "{same:?}");
        }
        for rel in ["api", "  a ", "users/${…}", ":id", "?page=2"] {
            assert!(!endpoint::is_canonical_http_path(rel), "{rel:?}");
            assert!(endpoint::is_canonical_http_path(&c(rel)), "{rel:?}");
        }
    }

    /// LB.5 — the qname builders share `canonical_http_path`, so a relative
    /// and a slashed literal build the SAME qname in every shape (LB.11b
    /// removed the third, the per-path `route:` builder, with its last
    /// caller).
    #[test]
    fn http_qname_builders_canonicalise_the_path() {
        assert_eq!(endpoint::route_qname("GET", "widgets"), "GET /widgets");
        assert_eq!(endpoint::route_qname("GET", "/widgets"), "GET /widgets");
        assert_eq!(
            endpoint::endpoint_qname("GET", "auth/login"),
            "endpoint:GET:/auth/login"
        );
        assert_eq!(
            endpoint::endpoint_qname("GET", "${…}/users"),
            "endpoint:GET:${…}/users"
        );
        assert_eq!(
            endpoint::endpoint_qname("GET", "<unresolved>"),
            "endpoint:GET:<unresolved>"
        );
        assert_eq!(
            endpoint::endpoint_id(glia_core::RepoId(1), "DELETE", "x"),
            endpoint::endpoint_id(glia_core::RepoId(1), "DELETE", "/x"),
        );
    }

    /// LB.4a — the owner segment round-trips through every HTTP qname shape,
    /// an empty owner is no owner, and an `@` without the space before it is
    /// part of the path, never an owner.
    #[test]
    fn owner_segment_round_trips_and_ignores_bare_at() {
        use endpoint::{split_owner, with_owner};
        for base in [
            "GET /health",
            "route:/users",
            "endpoint:GET:/health",
            "page:/users",
            "endpoint:GET:${…}/users",
        ] {
            let q = with_owner(base, "services/users");
            assert_eq!(q, format!("{base} @services/users"));
            assert_eq!(split_owner(&q), (base, Some("services/users")));
            assert_eq!(split_owner(base), (base, None), "no owner: {base}");
        }
        assert_eq!(with_owner("GET /x", ""), "GET /x");
        assert_eq!(with_owner("GET /x", "packages/@shop/web"), "GET /x @packages/@shop/web");
        assert_eq!(split_owner("GET /x @packages/@shop/web"), ("GET /x", Some("packages/@shop/web")));
        // `@` inside a path segment, no space before it: not an owner.
        assert_eq!(split_owner("route:/pkg/@scope/x"), ("route:/pkg/@scope/x", None));
        assert_eq!(split_owner("GET /users/@me"), ("GET /users/@me", None));
        // An empty or whitespace-bearing tail is not an owner.
        assert_eq!(split_owner("GET /x @"), ("GET /x @", None));
        assert_eq!(split_owner("GET /x @a b"), ("GET /x @a b", None));
    }

    /// CB.6: a fact recorded twice for one scope is kept once (a C++ MODULE
    /// walked through both branches of an `#ifdef` records its prototype
    /// twice), in first-record order; scopes stay apart, and a fact that
    /// differs in any field is a second fact.
    #[test]
    fn nav_fact_record_is_idempotent() {
        let mut nav = CodeNav::default();
        let (module, func) = (NodeId(1), NodeId(2));
        let proto = NavFact::DeclaresFn {
            ns: String::new(),
            name: "codec_encode".into(),
        };
        let using = NavFact::UsingNamespace {
            within: String::new(),
            ns: "shop".into(),
        };
        nav.record_fact(module, proto.clone());
        nav.record_fact(module, using.clone());
        nav.record_fact(module, proto.clone());
        nav.record_fact(func, proto.clone());
        assert_eq!(nav.nav_facts[&module], vec![proto.clone(), using.clone()]);
        assert_eq!(nav.nav_facts[&func], vec![proto.clone()]);

        let in_ns = NavFact::DeclaresFn {
            ns: "shop".into(),
            name: "codec_encode".into(),
        };
        nav.record_fact(module, in_ns.clone());
        assert_eq!(nav.nav_facts[&module], vec![proto, using, in_ns]);

        // Two calls on two rows passing one mount are two call sites.
        let mount = Mount::Const("/api/v2".into());
        let at = |line| NavFact::MountArg {
            line,
            callee: "RegisterUsers".into(),
            arg: 0,
            mount: mount.clone(),
        };
        nav.record_fact(func, at(4));
        nav.record_fact(func, at(4));
        nav.record_fact(func, at(9));
        assert_eq!(nav.nav_facts[&func][1..], [at(4), at(9)]);
    }

    /// CB.6: a provisional mount ROUTE qname reads back as the method, the
    /// mount and the local path. A mount's suffix is folded into the local
    /// path (canonical), so the parsed mount carries an empty suffix; a Const
    /// mount is never provisional and builds the plain ROUTE qname.
    #[test]
    fn mount_route_qname_round_trips() {
        use endpoint::{
            mount_route_qname, parse_mount_route_qname, route_qname, split_owner, with_owner,
        };
        let param = Mount::Param {
            fn_qname: "api::routes::Register".into(),
            index: 1,
            suffix: String::new(),
        };
        let q = mount_route_qname("GET", &param, "/users");
        assert_eq!(q, "GET <mount:param:api::routes::Register#1>/users");
        assert_eq!(
            parse_mount_route_qname(&q),
            Some(("GET".into(), param.clone(), "/users".into()))
        );

        let field = Mount::Field {
            owner: "api::Server".into(),
            field: "admin".into(),
            suffix: String::new(),
        };
        let q = mount_route_qname("POST", &field, "/users/:id");
        assert_eq!(q, "POST <mount:field:api::Server.admin>/users/:id");
        assert_eq!(
            parse_mount_route_qname(&q),
            Some(("POST".into(), field, "/users/:id".into()))
        );

        // `me := rg.Group("/me"); me.GET("p", h)`: the suffix joins the local
        // path through `join_path`, then the path is canonical.
        let me = Mount::Param {
            fn_qname: "api::routes::Register".into(),
            index: 0,
            suffix: "/me".into(),
        };
        let q = mount_route_qname("GET", &me, "p");
        assert_eq!(q, "GET <mount:param:api::routes::Register#0>/me/p");
        let bare = Mount::Param {
            fn_qname: "api::routes::Register".into(),
            index: 0,
            suffix: String::new(),
        };
        assert_eq!(
            parse_mount_route_qname(&q),
            Some(("GET".into(), bare.clone(), "/me/p".into()))
        );
        assert_eq!(
            mount_route_qname("GET", &bare, "/me/p"),
            q,
            "suffix + local and the folded path are one route"
        );

        // A group's index route is the group; an owner dir with a dot splits
        // at the LAST `.` (a Go field name has none).
        let dotted = Mount::Field {
            owner: "v1.2::api::Server".into(),
            field: "grp".into(),
            suffix: "/x".into(),
        };
        let q = mount_route_qname("ANY", &dotted, "/");
        assert_eq!(q, "ANY <mount:field:v1.2::api::Server.grp>/x");
        let parsed = Mount::Field {
            owner: "v1.2::api::Server".into(),
            field: "grp".into(),
            suffix: String::new(),
        };
        assert_eq!(
            parse_mount_route_qname(&q),
            Some(("ANY".into(), parsed, "/x".into()))
        );

        // Const: exactly the Go parser's `route_qname(join_path(prefix, local))`.
        assert_eq!(
            mount_route_qname("GET", &Mount::Const("/api/v2".into()), "users"),
            "GET /api/v2/users"
        );
        assert_eq!(
            mount_route_qname("GET", &Mount::Const("/api/".into()), "/"),
            route_qname("GET", "/api/")
        );
        assert_eq!(
            mount_route_qname("GET", &Mount::Const(String::new()), "users"),
            "GET /users"
        );
        assert_eq!(
            parse_mount_route_qname(&mount_route_qname("GET", &Mount::Const("/a".into()), "/b")),
            None
        );

        // LB.4a's owner suffix: the caller strips it with split_owner first.
        let q = with_owner(&mount_route_qname("GET", &param, "/users"), "turps");
        assert_eq!(q, "GET <mount:param:api::routes::Register#1>/users @turps");
        let (base, owner) = split_owner(&q);
        assert_eq!(owner, Some("turps"));
        assert_eq!(
            parse_mount_route_qname(base),
            Some(("GET".into(), param, "/users".into()))
        );
    }

    /// CB.6: a canonical ROUTE qname, and anything not in the exact
    /// provisional shape, parses to None, so the mount pass never touches a
    /// real route.
    #[test]
    fn a_plain_route_qname_is_not_a_mount() {
        use endpoint::{parse_mount_route_qname, route_qname};
        assert_eq!(parse_mount_route_qname(&route_qname("GET", "/users")), None);
        assert_eq!(
            parse_mount_route_qname(&route_qname("GET", "<unresolved>")),
            None
        );
        for q in [
            "GET /users",
            "ANY /",
            "GET /users @turps",
            "GET <unresolved>",
            "route:/users",
            "endpoint:GET:/users",
            "page:/users",
            "",
            "GET",
            "<mount:param:f#0>/x",
            " <mount:param:f#0>/x",
            "GET <mount:param:f#0/x",
            "GET <mount:param:f>/x",
            "GET <mount:param:f#>/x",
            "GET <mount:param:f#x>/x",
            "GET <mount:param:f#+1>/x",
            "GET <mount:param:#0>/x",
            "GET <mount:field:Server>/x",
            "GET <mount:field:.grp>/x",
            "GET <mount:field:api::Server.>/x",
            "GET <mount:const:/api>/x",
            "GET /x<mount:param:f#0>/y",
        ] {
            assert_eq!(parse_mount_route_qname(q), None, "{q:?}");
        }
    }

    /// LB.5 — a relative client path becomes the canonical node, carries the
    /// original literal as `raw`, and merges with the slashed call to the same
    /// path; a caller's own `raw` wins over the relative literal.
    #[test]
    fn client_endpoint_relative_path_is_canonical_with_raw() {
        use endpoint::HitExtras;
        use glia_core::{Confidence, RepoId};
        let ep = |path: &str| endpoint::ClientEndpoint {
            method: "DELETE".into(),
            path: path.into(),
            file: "a.ts".into(),
            line: 1,
            col: 1,
            confidence: Confidence::Strong,
        };
        let from = NodeId::from_parts(GRAPH_TYPE, RepoId(1), node_kind::METHOD, "m");
        let (mut nodes, mut edges) = (Vec::new(), Vec::new());
        let (mut nav, mut seen) = (CodeNav::default(), Default::default());
        let mut push = |e: &endpoint::ClientEndpoint, extras: HitExtras<'_>| {
            endpoint::push_client_endpoint_with(
                RepoId(1),
                e,
                extras,
                from,
                &mut nodes,
                &mut edges,
                &mut nav,
                &mut seen,
            )
        };
        let rel = push(&ep("protected/x"), HitExtras::default());
        let abs = push(&ep("/protected/x"), HitExtras::default());
        assert_eq!(rel, abs, "relative and slashed calls are one ENDPOINT");
        assert_eq!(nodes.len(), 1);
        assert_eq!(edges.len(), 2, "one CALLS edge per call site");
        assert_eq!(
            nodes[0].cells[0].payload,
            CellPayload::Json(
                r#"{"method":"DELETE","path":"/protected/x","file":"a.ts","line":1,"col":1,"confidence":"strong","raw":"protected/x"}"#
                    .into()
            )
        );
        assert_eq!(
            nav.qname_by_id.get(&rel).map(String::as_str),
            Some("endpoint:DELETE:/protected/x")
        );
        assert_eq!(
            nav.name_by_id.get(&rel).map(String::as_str),
            Some("DELETE /protected/x")
        );

        let (mut nodes, mut edges) = (Vec::new(), Vec::new());
        let (mut nav, mut seen) = (CodeNav::default(), Default::default());
        endpoint::push_client_endpoint_with(
            RepoId(1),
            &ep("users?x=1"),
            HitExtras {
                raw: Some("http://h/users?x=1"),
                host: None,
            },
            from,
            &mut nodes,
            &mut edges,
            &mut nav,
            &mut seen,
        );
        match &nodes[0].cells[0].payload {
            CellPayload::Json(j) => assert!(j.contains(r#""raw":"http://h/users?x=1""#), "{j}"),
            other => panic!("{other:?}"),
        }
    }

    /// A3.3 — the client-path normaliser drops scheme+host and query+fragment
    /// and NEVER deletes a path: everything it cannot improve comes back as-is.
    #[test]
    fn normalise_client_path_strips_host_and_query_only() {
        let n = |s: &str| endpoint::normalise_client_path(s);
        // Absolute URL: scheme + host + query all go.
        assert_eq!(n("https://api.example.com/users?active=1"), ("/users".into(), true));
        assert_eq!(n("http://api/users"), ("/users".into(), true));
        // Bare host → the root path.
        assert_eq!(n("https://api.example.com"), ("/".into(), true));
        assert_eq!(n("https://api.example.com?x=1"), ("/".into(), true));
        // Interpolated host / scheme is still a host.
        assert_eq!(n("https://${…}/users/${…}"), ("/users/${…}".into(), true));
        assert_eq!(n("${…}://${…}/users"), ("/users".into(), true));
        // Query + fragment on a relative path.
        assert_eq!(n("/users?a=1#frag"), ("/users".into(), true));
        assert_eq!(n("/users#frag"), ("/users".into(), true));
        assert_eq!(n("${…}/users?page=${…}"), ("${…}/users".into(), true));
        // Untouched: already a path, an interpolated base, a relative hint.
        assert_eq!(n("/users"), ("/users".into(), false));
        assert_eq!(n("/users/${…}"), ("/users/${…}".into(), false));
        assert_eq!(n("${…}/users"), ("${…}/users".into(), false));
        assert_eq!(n("auth/login"), ("auth/login".into(), false));
        assert_eq!(n("<unresolved>"), ("<unresolved>".into(), false));
        assert_eq!(n(""), (String::new(), false));
        // A `://` that is only in the query is not a scheme.
        assert_eq!(
            n("/login?next=https://x/y"),
            ("/login".into(), true)
        );
        // A path that embeds a URL keeps it: the scheme must precede any `/`.
        assert_eq!(n("/proxy/http://x/y"), ("/proxy/http://x/y".into(), false));
        // A query-only literal is NOT turned into "" (which would read as `/`).
        assert_eq!(n("?page=2"), ("?page=2".into(), false));
        assert_eq!(n("#top"), ("#top".into(), false));
    }

    /// A11.2 — `url_split` returns the authority AND the path, and its path
    /// half is exactly what `url_to_path` returns.
    #[test]
    fn url_split_separates_authority_from_path() {
        let s = |x: &str| endpoint::url_split(x);
        let some = |x: &str| Some(x.to_string());
        assert_eq!(s("http://h:8080/a?b"), (some("h:8080"), some("/a")));
        assert_eq!(s("/a"), (None, some("/a")));
        assert_eq!(
            s("https://api.example.com"),
            (some("api.example.com"), some("/"))
        );
        assert_eq!(
            s("https://api.example.com#x/y"),
            (some("api.example.com"), some("/"))
        );
        assert_eq!(
            s("http://u:p@users-service:8080/users"),
            (some("users-service:8080"), some("/users"))
        );
        assert_eq!(
            s("http://users-service:8080/users/${…}"),
            (some("users-service:8080"), some("/users/${…}"))
        );
        // An empty authority is no authority.
        assert_eq!(s("file:///etc/hosts"), (None, some("/etc/hosts")));
        // Not a path: a relative hint, an unresolved base, a bare word.
        assert_eq!(s("auth/login"), (None, None));
        assert_eq!(s("${…}/users"), (None, None));
        assert_eq!(s("users-service:8080"), (None, None));
        // The same scheme rules as normalise_client_path.
        assert_eq!(s("/login?next=https://x/y"), (None, some("/login")));
        assert_eq!(s("/proxy/http://x/y"), (None, some("/proxy/http://x/y")));
        assert_eq!(s("?page=2"), (None, None));

        for raw in [
            "http://h:8080/a?b",
            "  https://api.example.com/users  ",
            "/users#frag",
            "auth/login",
            "https://api",
            "",
        ] {
            assert_eq!(endpoint::url_to_path(raw), s(raw.trim()).1, "{raw:?}");
        }
    }

    /// A11.2 — the pre-A11.2 `url_to_path` results every parser relies on are
    /// unchanged; only a `://` in a query or after a `/` is read differently.
    #[test]
    fn url_to_path_keeps_its_contract() {
        let p = endpoint::url_to_path;
        assert_eq!(p("http://api/users"), Some("/users".into()));
        assert_eq!(
            p("https://api.example.com/users?active=1"),
            Some("/users".into())
        );
        assert_eq!(p("https://api.example.com"), Some("/".into()));
        assert_eq!(p(" /users/${…} "), Some("/users/${…}".into()));
        assert_eq!(p("users"), None);
        assert_eq!(p("${…}/users"), None);
        assert_eq!(p("SELECT * FROM t"), None);
        // Changed by A11.2 (previously `/y` for both).
        assert_eq!(p("/login?next=https://x/y"), Some("/login".into()));
        assert_eq!(p("/proxy/http://x/y"), Some("/proxy/http://x/y".into()));
    }

    /// A3.3 — `raw` rides on ENDPOINT_HIT only when given; without it the
    /// payload is byte-identical to the pre-A3.3 writer.
    #[test]
    fn endpoint_hit_carries_raw_only_when_given() {
        use glia_core::{Confidence, RepoId};
        let ep = endpoint::ClientEndpoint {
            method: "POST".into(),
            path: "/users".into(),
            file: "lib/api.dart".into(),
            line: 3,
            col: 5,
            confidence: Confidence::Strong,
        };
        let from = NodeId::from_parts(GRAPH_TYPE, RepoId(1), node_kind::METHOD, "m");
        let payload = |raw: Option<&str>| {
            let (mut nodes, mut edges) = (Vec::new(), Vec::new());
            let (mut nav, mut seen) = (CodeNav::default(), Default::default());
            endpoint::push_client_endpoint_with_raw(
                RepoId(1), &ep, raw, from, &mut nodes, &mut edges, &mut nav, &mut seen,
            );
            match &nodes[0].cells[0].payload {
                CellPayload::Json(j) => j.clone(),
                _ => String::new(),
            }
        };
        assert_eq!(
            payload(None),
            r#"{"method":"POST","path":"/users","file":"lib/api.dart","line":3,"col":5,"confidence":"strong"}"#
        );
        assert_eq!(
            payload(Some("https://api.example.com/users")),
            r#"{"method":"POST","path":"/users","file":"lib/api.dart","line":3,"col":5,"confidence":"strong","raw":"https://api.example.com/users"}"#
        );
        // The unchanged entry point is the `None` case exactly.
        let (mut nodes, mut edges) = (Vec::new(), Vec::new());
        let (mut nav, mut seen) = (CodeNav::default(), Default::default());
        endpoint::push_client_endpoint(
            RepoId(1), &ep, from, &mut nodes, &mut edges, &mut nav, &mut seen,
        );
        assert_eq!(
            nodes[0].cells[0].payload,
            CellPayload::Json(payload(None))
        );
    }

    /// CB.21 — a channel client's ENDPOINT_HIT: `via` first, `host` only when
    /// given, escaped; never a `file`, so the file-keyed readers skip it.
    #[test]
    fn channel_hit_cell_carries_via_and_an_optional_host() {
        let json = |c: Cell| match c.payload {
            CellPayload::Json(j) => j,
            other => panic!("not JSON: {other:?}"),
        };
        let c = endpoint::channel_hit_cell("ws", Some("chat-svc:8080"));
        assert_eq!(c.kind, cell_type::ENDPOINT_HIT);
        assert_eq!(json(c), r#"{"via":"ws","host":"chat-svc:8080"}"#);
        assert_eq!(json(endpoint::channel_hit_cell("grpc", None)), r#"{"via":"grpc"}"#);
        assert_eq!(
            json(endpoint::channel_hit_cell("grpc", Some("a\"b"))),
            r#"{"via":"grpc","host":"a\"b"}"#
        );
        assert_eq!(endpoint::http_node_span(&[endpoint::channel_hit_cell("ws", Some("h"))]), None);
    }

    /// A11.5 — `host` rides on ENDPOINT_HIT only when given, after `raw`; the
    /// default extras are the plain payload byte for byte.
    #[test]
    fn endpoint_hit_carries_host_after_raw_only_when_given() {
        use endpoint::HitExtras;
        use glia_core::{Confidence, RepoId};
        let ep = endpoint::ClientEndpoint {
            method: "GET".into(),
            path: "/users".into(),
            file: "client.go".into(),
            line: 9,
            col: 15,
            confidence: Confidence::Strong,
        };
        let from = NodeId::from_parts(GRAPH_TYPE, RepoId(1), node_kind::METHOD, "m");
        let payload = |extras: HitExtras<'_>| {
            let (mut nodes, mut edges) = (Vec::new(), Vec::new());
            let (mut nav, mut seen) = (CodeNav::default(), Default::default());
            endpoint::push_client_endpoint_with(
                RepoId(1),
                &ep,
                extras,
                from,
                &mut nodes,
                &mut edges,
                &mut nav,
                &mut seen,
            );
            match &nodes[0].cells[0].payload {
                CellPayload::Json(j) => j.clone(),
                _ => String::new(),
            }
        };
        let base = r#"{"method":"GET","path":"/users","file":"client.go","line":9,"col":15,"confidence":"strong""#;
        assert_eq!(payload(HitExtras::default()), format!("{base}}}"));
        assert_eq!(
            payload(HitExtras {
                raw: None,
                host: Some("api.example.com")
            }),
            format!(r#"{base},"host":"api.example.com"}}"#)
        );
        assert_eq!(
            payload(HitExtras {
                raw: Some("http://api:80/users"),
                host: Some("api:80")
            }),
            format!(r#"{base},"raw":"http://api:80/users","host":"api:80"}}"#)
        );
        // The older entry points are the default extras exactly.
        let (mut nodes, mut edges) = (Vec::new(), Vec::new());
        let (mut nav, mut seen) = (CodeNav::default(), Default::default());
        endpoint::push_client_endpoint(
            RepoId(1),
            &ep,
            from,
            &mut nodes,
            &mut edges,
            &mut nav,
            &mut seen,
        );
        assert_eq!(
            nodes[0].cells[0].payload,
            CellPayload::Json(payload(HitExtras::default()))
        );
    }

    /// A11.5 — `client_url_split`'s path half IS `url_to_path`, and its host
    /// half is kept only when the scheme and authority are both literal.
    #[test]
    fn client_url_split_keeps_only_literal_authorities() {
        let s = endpoint::client_url_split;
        let some = |x: &str| Some(x.to_string());
        assert_eq!(
            s("http://api.example.com/users"),
            (some("api.example.com"), some("/users"))
        );
        assert_eq!(s("http://api/users"), (some("api"), some("/users")));
        assert_eq!(s("http://svc:8080/x?y=1"), (some("svc:8080"), some("/x")));
        assert_eq!(
            s("  https://u:p@api.x/users  "),
            (some("api.x"), some("/users"))
        );
        assert_eq!(s("http://[::1]:8080/x"), (some("[::1]:8080"), some("/x")));
        assert_eq!(
            s("https://api.example.com/users/${…}"),
            (some("api.example.com"), some("/users/${…}"))
        );
        // Placeholders in the authority or the scheme: no host, path kept.
        assert_eq!(s("https://${…}/users"), (None, some("/users")));
        assert_eq!(s("http://localhost:${…}/users"), (None, some("/users")));
        assert_eq!(s("${…}://api/users"), (None, some("/users")));
        assert_eq!(s("http://{host}/users"), (None, some("/users")));
        assert_eq!(s("http://%s/users"), (None, some("/users")));
        // No authority at all.
        assert_eq!(s("/users/${…}"), (None, some("/users/${…}")));
        assert_eq!(s("${…}/users"), (None, None));
        assert_eq!(s("file:///etc/hosts"), (None, some("/etc/hosts")));
        assert_eq!(s("/login?next=https://x/y"), (None, some("/login")));

        for raw in [
            "http://api.example.com/users",
            "https://${…}/users",
            "${…}://api/users",
            " /users/${…} ",
            "users",
            "SELECT * FROM t",
            "/proxy/http://x/y",
            "",
        ] {
            assert_eq!(s(raw).1, endpoint::url_to_path(raw), "{raw:?}");
        }
    }

    /// W0.3 — every reserved id decodes to its own name (the reservation is
    /// only worth having if the decode tables already carry it).
    #[test]
    fn reserved_ids_decode() {
        assert_eq!(node_kind::name(node_kind::PROJECT), "PROJECT");
        assert_eq!(node_kind::name(node_kind::MESSAGE_TYPE), "MESSAGE_TYPE");
        assert_eq!(node_kind::name(node_kind::GRPC_SERVER), "GRPC_SERVER");
        assert_eq!(node_kind::name(node_kind::RPC_PROCEDURE), "RPC_PROCEDURE");
        assert_eq!(node_kind::name(node_kind::RPC_CALL), "RPC_CALL");
        assert_eq!(
            edge_category::name(edge_category::SHARES_DATA_SOURCE),
            "SHARES_DATA_SOURCE"
        );
        assert_eq!(edge_category::name(edge_category::RPC_CALLS), "RPC_CALLS");
        assert_eq!(cell_type::name(cell_type::MESSAGE_TYPE), "MESSAGE_TYPE");
        assert_eq!(cell_type::name(cell_type::RPC_PACKAGE), "RPC_PACKAGE");
        // L0.1 — the 0.5.0 leap reservations.
        assert_eq!(
            edge_category::name(edge_category::NAVIGATES_TO),
            "NAVIGATES_TO"
        );
        assert_eq!(edge_category::name(edge_category::CO_CHANGES), "CO_CHANGES");
        assert_eq!(cell_type::name(cell_type::DOC_TAGS), "DOC_TAGS");
        assert_eq!(cell_type::name(cell_type::ROLE), "ROLE");
        assert_eq!(cell_type::name(cell_type::EVIDENCE), "EVIDENCE");
        assert_eq!(cell_type::name(cell_type::SCHEMA_FIELDS), "SCHEMA_FIELDS");
        assert_eq!(cell_type::name(cell_type::COVERAGE), "COVERAGE");
        assert_eq!(cell_type::name(cell_type::ENTRYPOINT), "ENTRYPOINT");
        assert_eq!(cell_type::name(cell_type::ACCESS_MODE), "ACCESS_MODE");
        // Reserved, not emitted: the next free ids stay UNKNOWN.
        assert_eq!(node_kind::name(NodeKindId(50)), "UNKNOWN");
        assert_eq!(edge_category::name(EdgeCategoryId(37)), "UNKNOWN");
        assert_eq!(cell_type::name(CellTypeId(26)), "UNKNOWN");
    }

    /// L0.1 guard — `leap_ids_are_locked`. The 0.5.0 leap ids were allocated
    /// centrally and are baked into every `.gmap` from this commit. The packet
    /// texts proposed other numbers (SCHEMA_FIELDS 26, COVERAGE 27, ENTRYPOINT
    /// 28, CO_CHANGES 38); the allocation was compacted so each `ALL` table
    /// stays `1..=len`. Pinning each (const, id) pair makes a later renumbering
    /// fail by name, and the proposed numbers stay unregistered (UNKNOWN).
    #[test]
    fn leap_ids_are_locked() {
        assert_eq!(edge_category::NAVIGATES_TO, EdgeCategoryId(35));
        assert_eq!(edge_category::CO_CHANGES, EdgeCategoryId(36));
        assert_eq!(cell_type::DOC_TAGS, CellTypeId(19));
        assert_eq!(cell_type::ROLE, CellTypeId(20));
        assert_eq!(cell_type::EVIDENCE, CellTypeId(21));
        assert_eq!(cell_type::SCHEMA_FIELDS, CellTypeId(22));
        assert_eq!(cell_type::COVERAGE, CellTypeId(23));
        assert_eq!(cell_type::ENTRYPOINT, CellTypeId(24));
        assert_eq!(cell_type::ACCESS_MODE, CellTypeId(25));
        // The stale proposed numbers decode UNKNOWN, never a registered cell.
        for stale in [26, 27, 28] {
            assert_eq!(
                cell_type::name(CellTypeId(stale)),
                "UNKNOWN",
                "cell {stale}"
            );
        }
        assert_eq!(edge_category::name(EdgeCategoryId(38)), "UNKNOWN");
    }

    #[test]
    fn canonical_kind_maps_ecs_service_to_service() {
        assert_eq!(infra::canonical_kind("aws_ecs_service"), "service");
        assert_eq!(infra::canonical_kind("AWS_ECS_SERVICE"), "service");
        assert_eq!(infra::canonical_kind("kubernetes_deployment"), "deployment");
        assert_eq!(infra::canonical_kind("aws_ecr_repository"), "image");
        // The whole point: terraform and the k8s YAML path land on ONE qname.
        assert_eq!(
            infra::qname(infra::canonical_kind("aws_ecs_service"), "api"),
            infra::qname("service", "api")
        );
        assert_eq!(infra::qname("service", "api"), "infra:service:api");
    }

    #[test]
    fn di_stats_shape_table_is_dense_and_tokens_are_frozen() {
        use di_stats::{DiShape, LANGS};
        let tokens = [
            "ts-ctor",
            "ts-inject-fn",
            "ts-nest-ctor",
            "java-ctor",
            "java-field",
            "java-lombok",
            "java-jsr330",
            "csharp-ctor",
            "csharp-primary-ctor",
            "csharp-fromservices",
            "py-fastapi-depends",
            "php-ctor",
            "go-provider",
            "scala-ctor",
            "kotlin-ctor",
            "kotlin-field",
        ];
        assert_eq!(DiShape::ALL.len(), 16);
        for (i, s) in DiShape::ALL.iter().enumerate() {
            // Counter bank and render both rely on ALL[i] as usize == i.
            assert_eq!(*s as usize, i);
            assert_eq!(s.token(), tokens[i]);
            // A LANGS row, or kotlin: not a matrix row, printed when non-zero.
            assert!(
                LANGS.contains(&s.lang()) || s.lang() == "kotlin",
                "{:?} -> {}",
                s,
                s.lang()
            );
            // A token must never read as a language token under a `lang=` grep.
            assert!(!LANGS.contains(&s.token()));
        }
        // Matrix row spellings, not aliases: the consumers grep ` go=` / ` scala=`.
        assert_eq!(
            LANGS,
            [
                "python",
                "go",
                "typescript",
                "java",
                "csharp",
                "php",
                "scala"
            ]
        );
    }

    #[test]
    fn di_stats_render_prints_every_lang_and_only_fired_shapes() {
        use di_stats::{DiShape, render};
        let none = [0usize; 16];
        // Nothing counted: no line at all.
        assert_eq!(render(&[], &none, "r"), None);
        assert_eq!(render(&[("typescript", 0), ("dart", 0)], &none, "r"), None);

        let mut shapes = [0usize; 16];
        shapes[DiShape::GoProvider as usize] = 2;
        // Repeated languages sum; a non-LANGS language appears only when non-zero.
        let line = render(
            &[
                ("go", 1),
                ("go", 1),
                ("typescript", 3),
                ("dart", 0),
                ("kotlin", 4),
            ],
            &shapes,
            "fixtures/go-wire-di",
        );
        assert_eq!(
            line.as_deref(),
            Some(
                "[di] injects refs: python=0 go=2 typescript=3 java=0 csharp=0 php=0 scala=0 kotlin=4 \
                 (shapes: go-provider=2) repo=fixtures/go-wire-di"
            )
        );
        // Refs with no detector reporting (cache-served, or an emitter that
        // does not record yet) still print, with an explicit empty shape group.
        assert_eq!(
            render(&[("typescript", 1)], &none, "a").as_deref(),
            Some(
                "[di] injects refs: python=0 go=0 typescript=1 java=0 csharp=0 php=0 scala=0 (shapes: none) repo=a"
            )
        );
    }

    #[test]
    fn di_stats_record_take_and_flush_reset() {
        // The ONLY test in this crate that touches the process-global bank.
        use di_stats::{DiShape, flush_marker, record, reset, take_for_test};
        record(DiShape::ScalaCtor);
        assert_eq!(take_for_test(DiShape::ScalaCtor), 1);
        assert_eq!(take_for_test(DiShape::ScalaCtor), 0);

        record(DiShape::GoProvider);
        record(DiShape::TsInjectFn);
        flush_marker(&[("go", 1)], "test");
        // flush zeroes every slot, so a second build in-process starts clean.
        for s in DiShape::ALL {
            assert_eq!(take_for_test(s), 0, "{s:?} survived flush");
        }

        record(DiShape::JavaLombok);
        reset();
        assert_eq!(take_for_test(DiShape::JavaLombok), 0);
    }

    #[test]
    fn recv_stats_render_prints_every_lang_and_only_nonzero_extras() {
        use recv_stats::render;
        // Nothing counted: no line at all.
        assert_eq!(render(&[], &[], "r"), None);
        assert_eq!(render(&[("csharp", 0)], &[("python", 0)], "r"), None);
        // Fields recorded but nothing bound still prints: the carrier is live.
        assert_eq!(
            render(&[], &[("csharp", 2)], "a").as_deref(),
            Some(
                "[recv] receiver-typed calls bound: csharp=0 java=0 typescript=0 python=0 \
                 ruby=0 go=0 dart=0 rust=0 (fields: csharp=2 java=0 typescript=0 python=0 \
                 ruby=0 go=0 dart=0 rust=0) repo=a"
            )
        );
        // Repeated languages sum; a non-LANGS language appears only when non-zero.
        assert_eq!(
            render(
                &[("csharp", 1), ("csharp", 2), ("rust", 5), ("kotlin", 0)],
                &[("csharp", 4), ("rust", 2), ("kotlin", 1)],
                "fixtures/csharp-field-dispatch"
            )
            .as_deref(),
            Some(
                "[recv] receiver-typed calls bound: csharp=3 java=0 typescript=0 python=0 \
                 ruby=0 go=0 dart=0 rust=5 (fields: csharp=4 java=0 typescript=0 python=0 \
                 ruby=0 go=0 dart=0 rust=2 kotlin=1) repo=fixtures/csharp-field-dispatch"
            )
        );
    }

    #[test]
    fn recv_stats_record_take_and_reset() {
        // The counter is this test thread's own (LG.1c), so no other test can
        // move it.
        recv_stats::record();
        recv_stats::record();
        assert_eq!(recv_stats::take(), 2);
        assert_eq!(recv_stats::take(), 0);
        recv_stats::record();
        recv_stats::reset();
        assert_eq!(recv_stats::take(), 0);
    }

    /// LG.1c: two concurrent "builds" record different counts and each
    /// `take()` sees only its own. The barriers hold both threads between
    /// their records and their takes, so a shared counter would read 3 + 5 on
    /// one side and 0 on the other.
    #[test]
    fn recv_stats_counts_stay_on_their_thread() {
        let recorded = std::sync::Barrier::new(3);
        let taken = std::sync::Barrier::new(3);
        recv_stats::reset();
        recv_stats::record();
        let (a, b) = std::thread::scope(|s| {
            let build = |n: usize| {
                let (recorded, taken) = (&recorded, &taken);
                move || {
                    recv_stats::reset();
                    for _ in 0..n {
                        recv_stats::record();
                    }
                    recorded.wait();
                    let own = recv_stats::take();
                    taken.wait();
                    own
                }
            };
            let a = s.spawn(build(3));
            let b = s.spawn(build(5));
            recorded.wait();
            taken.wait();
            (a.join(), b.join())
        });
        assert_eq!((a.ok(), b.ok()), (Some(3), Some(5)));
        assert_eq!(
            recv_stats::take(),
            1,
            "the spawning thread keeps its own count"
        );
    }

    #[test]
    fn record_field_type_keys_by_owner_and_ignores_empty_names() {
        let r = glia_core::RepoId(1);
        let a = NodeId::from_parts(GRAPH_TYPE, r, node_kind::CLASS, "m::A");
        let b = NodeId::from_parts(GRAPH_TYPE, r, node_kind::CLASS, "m::B");
        let mut nav = CodeNav::default();
        nav.record_field_type(a, "_repo", "UserRepo");
        nav.record_field_type(a, "Archive", "UserRepo");
        nav.record_field_type(b, "_repo", "OrderRepo");
        nav.record_field_type(a, "", "UserRepo");
        nav.record_field_type(a, "_empty", "");
        assert_eq!(nav.field_types[&a].len(), 2);
        assert_eq!(nav.field_types[&a]["_repo"], "UserRepo");
        assert_eq!(nav.field_types[&b]["_repo"], "OrderRepo");
        // A re-record of the same field replaces the type.
        nav.record_field_type(a, "_repo", "CachedRepo");
        assert_eq!(nav.field_types[&a]["_repo"], "CachedRepo");
    }

    #[test]
    fn record_local_type_keys_by_scope_and_a_conflict_is_unknown() {
        let r = glia_core::RepoId(1);
        let f = NodeId::from_parts(GRAPH_TYPE, r, node_kind::FUNCTION, "m::f");
        let g = NodeId::from_parts(GRAPH_TYPE, r, node_kind::FUNCTION, "m::g");
        let mut nav = CodeNav::default();
        nav.record_local_type(f, "r", "Repo");
        nav.record_local_type(f, "r", "Repo");
        nav.record_local_type(f, "x", "");
        nav.record_local_type(g, "r", "Index");
        nav.record_local_type(f, "", "Repo");
        assert_eq!(nav.local_types[&f].len(), 2);
        // The same type twice keeps it; an unknown local is recorded as "".
        assert_eq!(nav.local_types[&f]["r"], "Repo");
        assert_eq!(nav.local_types[&f]["x"], "");
        assert_eq!(nav.local_types[&g]["r"], "Index");
        // A second, different type (shadowing) makes the local unknown, and
        // it stays unknown: a later record never revives a type.
        nav.record_local_type(f, "r", "Index");
        assert_eq!(nav.local_types[&f]["r"], "");
        nav.record_local_type(f, "r", "Repo");
        assert_eq!(nav.local_types[&f]["r"], "");
        // An unknown record after a typed one is a conflict too.
        nav.record_local_type(g, "r", "");
        assert_eq!(nav.local_types[&g]["r"], "");
    }

    /// CA.2a: one result type per callable; an empty type records nothing,
    /// a re-record replaces.
    #[test]
    fn record_return_type_ignores_an_empty_type() {
        let r = glia_core::RepoId(1);
        let f = NodeId::from_parts(GRAPH_TYPE, r, node_kind::FUNCTION, "m::f");
        let g = NodeId::from_parts(GRAPH_TYPE, r, node_kind::METHOD, "m::T::g");
        let mut nav = CodeNav::default();
        nav.record_return_type(f, "repositories.UserRepository");
        nav.record_return_type(g, "");
        assert_eq!(nav.return_types.len(), 1);
        assert_eq!(nav.return_types[&f], "repositories.UserRepository");
        nav.record_return_type(f, "Bundle");
        assert_eq!(nav.return_types[&f], "Bundle");
        assert!(!nav.return_types.contains_key(&g));
    }

    /// CA.3a: one signature per METHOD; an empty signature records nothing,
    /// a re-record replaces.
    #[test]
    fn record_method_sig_ignores_an_empty_signature() {
        let r = glia_core::RepoId(1);
        let m = NodeId::from_parts(GRAPH_TYPE, r, node_kind::METHOD, "m::T::Get");
        let n = NodeId::from_parts(GRAPH_TYPE, r, node_kind::METHOD, "m::T::Put");
        let mut nav = CodeNav::default();
        nav.record_method_sig(m, "(string)(string)");
        nav.record_method_sig(n, "");
        assert_eq!(nav.method_sigs.len(), 1);
        assert_eq!(nav.method_sigs[&m], "(string)(string)");
        nav.record_method_sig(m, "()()");
        assert_eq!(nav.method_sigs[&m], "()()");
        assert!(!nav.method_sigs.contains_key(&n));
    }

    #[test]
    fn canonical_kind_passes_unknown_type_through() {
        assert_eq!(infra::canonical_kind("aws_s3_bucket"), "aws_s3_bucket");
        assert_eq!(infra::canonical_kind("random_pet"), "random_pet");
        assert_eq!(
            infra::qname(infra::canonical_kind("aws_s3_bucket"), "data"),
            "infra:aws_s3_bucket:data"
        );
        // No alias may collapse two DIFFERENT canonical kinds onto each other.
        assert_ne!(
            infra::canonical_kind("kubernetes_service"),
            infra::canonical_kind("kubernetes_deployment")
        );
    }

    /// A3.6 — the HTTP span reader. One row per payload shape the parsers
    /// actually write, plus the malformed ones it must survive. Every `Some`
    /// line is the cell's 1-indexed `line` minus one (POSITION is 0-indexed).
    #[test]
    fn http_node_span_reads_endpoint_hit_and_json_route_method() {
        use endpoint::http_node_span;
        let json = |kind, s: &str| Cell { kind, payload: CellPayload::Json(s.into()) };
        let text = |kind, s: &str| Cell { kind, payload: CellPayload::Text(s.into()) };
        let hit = cell_type::ENDPOINT_HIT;
        let rm = cell_type::ROUTE_METHOD;
        let span = |f: &str, l: Option<i64>| Some((f.to_string(), l));

        let table: Vec<(&str, Vec<Cell>, Option<(String, Option<i64>)>)> = vec![
            (
                "ENDPOINT_HIT, as endpoint_hit_json writes it",
                vec![json(hit, r#"{"method":"GET","path":"/users","file":"web/api.ts","line":2,"col":21,"confidence":"strong"}"#)],
                span("web/api.ts", Some(1)),
            ),
            (
                "parser-go ROUTE_METHOD json",
                vec![json(rm, r#"{"method":"GET","handler":"listUsers","file":"main.go","line":15,"col":2}"#)],
                span("main.go", Some(14)),
            ),
            (
                "ts_routes line:0 is unknown, not row -1",
                vec![json(rm, r#"{"method":"GET","handler":"","file":"server.ts","line":0,"col":0}"#)],
                span("server.ts", None),
            ),
            ("bare-verb Text ROUTE_METHOD", vec![text(rm, "GET")], None),
            ("malformed JSON", vec![json(hit, r#"{"file":"a.ts","line":"#)], None),
            ("empty file is no file", vec![json(hit, r#"{"file":"","line":3}"#)], None),
            ("non-integer line", vec![json(hit, r#"{"file":"a.ts","line":"3"}"#)], span("a.ts", None)),
            (
                "a POSITION cell is not an HTTP span",
                vec![json(cell_type::POSITION, r#"{"file":"a.py","start_line":4,"end_line":9}"#)],
                None,
            ),
            (
                "first usable ROUTE_METHOD wins, the bare verb is skipped",
                vec![
                    text(rm, "POST"),
                    json(rm, r#"{"method":"GET","handler":null,"file":"a.go","line":7,"col":1}"#),
                    json(rm, r#"{"method":"PUT","handler":null,"file":"b.go","line":9,"col":1}"#),
                ],
                span("a.go", Some(6)),
            ),
            (
                "ENDPOINT_HIT outranks an earlier ROUTE_METHOD",
                vec![
                    json(rm, r#"{"method":"GET","handler":null,"file":"a.go","line":7,"col":1}"#),
                    json(hit, r#"{"file":"c.ts","line":1}"#),
                ],
                span("c.ts", Some(0)),
            ),
            ("no cells", vec![], None),
        ];
        for (what, cells, want) in table {
            assert_eq!(http_node_span(&cells), want, "{what}");
        }
    }
}
