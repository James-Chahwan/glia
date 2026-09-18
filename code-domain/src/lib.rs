//! repo-graph-code-domain — shared code-domain types for every language parser.
//!
//! Extracted from `repo-graph-parser-python` at v0.4.3b so Go + TypeScript
//! parsers can share the constants + structural types without a weird
//! inter-parser dependency. All code-language parsers produce a `FileParse`,
//! and `repo-graph-graph` consumes the uniform shape.
//!
//! Registry-locked u32 values live here as the single source of truth.
//! See `memory/reference_code_domain_registries.md` for the semantic notes.

use std::collections::HashMap;

use repo_graph_core::{Cell, CellPayload, CellTypeId, Edge, EdgeCategoryId, Node, NodeId, NodeKindId};

/// Graph-type tag for any code-language graph. First arg to `NodeId::from_parts`.
pub const GRAPH_TYPE: &str = "code";

/// Which directories the repo walk descends into and which collapse to a single
/// REGION anchor. Shared so the builder's walk and `store::is_gmap_stale` cannot
/// drift apart. (A8.1)
pub mod walk_gating;

/// Manifest-rooted sub-projects detected during that same walk: which manifest
/// roots a directory, its ecosystem and its label. (A8.4)
pub mod project_roots;

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
    // docs/). qname `docs::<file_stem>::<section_slug>`; the CODE cell holds the
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
    //   47 GRPC_SERVER    — A5.3
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

    /// RESERVED (A5.3) — the server-side registration that binds a service
    /// implementation into a gRPC server. Distinct from `GRPC_SERVICE` (the
    /// declared service) and `GRPC_CLIENT` (the calling stub).
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

    // v0.4.11a — module → data-source access (D1)
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

    // v0.4.x — Config resolver. `READS_CONFIG` from a code module to a
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
    // ------------------------------------------------------------------

    /// Cross-repo pairing: two nodes reach the same external data source
    /// (`data_source:redis` — the same database / cache / bucket / search /
    /// email provider). The cross-graph counterpart of intra-repo
    /// `ACCESSES_DATA`. Emitted by `DbResolver` over DATABASE / CACHE /
    /// BLOB_STORE / SEARCH_INDEX / EMAIL_SERVICE nodes, always at
    /// `Confidence::Weak` (the provider needles are substring matches).
    ///
    /// Deliberately NOT in `blast_carry_edges()`: a shared Postgres is an
    /// operational fact, not a code dependency, and carrying it would fan every
    /// blast radius across every service in the stack. (A13.3)
    pub const SHARES_DATA_SOURCE: EdgeCategoryId = EdgeCategoryId(33);

    /// `RPC_CALL` → `RPC_PROCEDURE`: a client call site to the remote procedure
    /// it names. The transport-generic counterpart of `GRPC_CALLS`; tRPC today,
    /// Connect / Twirp when those land. Emitted by `RpcStackResolver` on an
    /// exact procedure-path match (`rpc_call:<path>` ↔ `rpc:<path>`) — no
    /// substring fallback. In `blast_carry_edges()`. (A10.10)
    pub const RPC_CALLS: EdgeCategoryId = EdgeCategoryId(34);

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
    pub const ENDPOINT_HIT: CellTypeId = CellTypeId(6);
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
    // ------------------------------------------------------------------

    /// RESERVED (A12.1) — the message / payload schema type a node sends or
    /// receives, as the declared type name.
    pub const MESSAGE_TYPE: CellTypeId = CellTypeId(17);

    /// RESERVED (A5.1) — the RPC package / namespace a service declaration
    /// lives in (a proto `package foo.bar;`).
    pub const RPC_PACKAGE: CellTypeId = CellTypeId(18);

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
}

/// Classification of a call site by its syntactic shape. Resolution (which
/// node id the call actually targets) happens in `repo-graph-graph` using the
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
    let libs = library_names(&fp.imports, lang);
    let json = format!(
        "[{}]",
        libs.iter()
            .map(|l| format!("\"{}\"", l.replace('\\', "\\\\").replace('"', "\\\"")))
            .collect::<Vec<_>>()
            .join(",")
    );
    for n in &mut fp.nodes {
        n.cells.push(Cell {
            kind: cell_type::IMPORTS,
            payload: CellPayload::Json(json.clone()),
        });
    }
}

/// The per-file output every code-language parser produces. `repo-graph-graph`
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
}

impl CodeNav {
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
    use repo_graph_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};
    use std::collections::HashSet;

    use super::CodeNav;

    /// One extracted client HTTP call. `path` MUST use `${…}` for any
    /// interpolated segment so it normalises the same way TS template paths do:
    /// `normalise_http_path` in repo-graph-graph collapses any segment containing
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
    fn endpoint_hit_json(ep: &ClientEndpoint, extras: HitExtras<'_>) -> String {
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
            esc(&ep.path),
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
    /// repo-graph-graph already does that downstream, and stripping it twice
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
    /// as provenance (`"raw"` on ENDPOINT_HIT).
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

    /// Stable ENDPOINT node id for a `(method, path)` — `endpoint:<METHOD>:<path>`,
    /// the qname convention `HttpStackResolver::parse_endpoint_qname` reads.
    pub fn endpoint_id(repo: RepoId, method: &str, path: &str) -> NodeId {
        let qname = format!("endpoint:{method}:{path}");
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
        let qname = format!("endpoint:{}:{}", ep.method, ep.path);
        let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::ENDPOINT, &qname);
        if seen.insert(id) {
            nodes.push(Node {
                id,
                repo,
                confidence: ep.confidence,
                cells: vec![Cell {
                    kind: cell_type::ENDPOINT_HIT,
                    payload: CellPayload::Json(endpoint_hit_json(ep, extras)),
                }],
            });
            let display = format!("{} {}", ep.method, ep.path);
            nav.record(id, &display, &qname, node_kind::ENDPOINT, None);
        }
        edges.push(Edge {
            from,
            to: id,
            category: edge_category::CALLS,
            confidence: ep.confidence,
        });
        id
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
    }

    impl DiShape {
        /// Every shape, in discriminant order (`ALL[i] as usize == i`).
        pub const ALL: [DiShape; 14] = [
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
            }
        }

        /// The [`LANGS`] row this shape belongs to.
        pub const fn lang(self) -> &'static str {
            match self {
                Self::TsCtor | Self::TsInjectFn | Self::TsNestCtor => "typescript",
                Self::JavaCtor | Self::JavaField | Self::JavaLombok | Self::JavaJsr330 => "java",
                Self::CsCtor | Self::CsPrimaryCtor | Self::CsFromServices => "csharp",
                Self::PyFastapiDepends => "python",
                Self::PhpCtor => "php",
                Self::GoProvider => "go",
                Self::ScalaCtor => "scala",
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
/// `rel_path`'s stem, exactly as the file walk did.
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
            34,
        );
        check(
            "cell_type",
            cell_type::ALL.iter().map(|(id, _)| id.0).collect(),
            cell_type::ALL.iter().map(|(_, n)| *n).collect(),
            18,
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

    /// `abs_path` is the single place a route template is forced to exactly
    /// one leading `/`. It must never trim a trailing one — `normalise_http_path`
    /// in repo-graph-graph already does that, and doing it twice would move
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
        use repo_graph_core::{Confidence, RepoId};
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

    /// A11.5 — `host` rides on ENDPOINT_HIT only when given, after `raw`; the
    /// default extras are the plain payload byte for byte.
    #[test]
    fn endpoint_hit_carries_host_after_raw_only_when_given() {
        use endpoint::HitExtras;
        use repo_graph_core::{Confidence, RepoId};
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
        // Reserved, not emitted: the next free ids stay UNKNOWN.
        assert_eq!(node_kind::name(NodeKindId(50)), "UNKNOWN");
        assert_eq!(edge_category::name(EdgeCategoryId(35)), "UNKNOWN");
        assert_eq!(cell_type::name(CellTypeId(19)), "UNKNOWN");
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
        ];
        assert_eq!(DiShape::ALL.len(), 14);
        for (i, s) in DiShape::ALL.iter().enumerate() {
            // Counter bank and render both rely on ALL[i] as usize == i.
            assert_eq!(*s as usize, i);
            assert_eq!(s.token(), tokens[i]);
            assert!(LANGS.contains(&s.lang()), "{:?} -> {}", s, s.lang());
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
        let none = [0usize; 14];
        // Nothing counted: no line at all.
        assert_eq!(render(&[], &none, "r"), None);
        assert_eq!(render(&[("typescript", 0), ("dart", 0)], &none, "r"), None);

        let mut shapes = [0usize; 14];
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
}
