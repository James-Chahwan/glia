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
    //   45 PROJECT        — A8.4
    //   46 MESSAGE_TYPE   — A10.5 / A12.1
    //   47 GRPC_SERVER    — A5.3
    //   48 RPC_PROCEDURE  — A10.9
    //   49 RPC_CALL       — A10.9
    // ------------------------------------------------------------------

    /// RESERVED (A8.4) — a build/workspace project unit: an MSBuild `.csproj`,
    /// a Gradle subproject, a Cargo workspace member. The anchor a repo's
    /// modules hang off when one repo holds several independent projects.
    pub const PROJECT: NodeKindId = NodeKindId(45);

    /// RESERVED (A10.5 / A12.1) — a declared message / payload schema type
    /// (protobuf `message`, Avro record, Thrift struct) that RPC procedures
    /// and queue payloads reference by name.
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
    //   34 RPC_CALLS          — A10.10
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

    /// RESERVED (A10.10) — call site → remote procedure. The transport-generic
    /// counterpart of `GRPC_CALLS`, for tRPC / JSON-RPC / Thrift.
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
        (RPC_CALLS, "RPC_CALLS"),                   // no emitter yet
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

    fn endpoint_hit_json(ep: &ClientEndpoint) -> String {
        let conf = match ep.confidence {
            Confidence::Strong => "strong",
            Confidence::Medium => "medium",
            Confidence::Weak => "weak",
        };
        format!(
            r#"{{"method":"{}","path":"{}","file":"{}","line":{},"col":{},"confidence":"{}"}}"#,
            esc(&ep.method),
            esc(&ep.path),
            esc(&ep.file),
            ep.line,
            ep.col,
            conf,
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

    /// Extract the request PATH from a URL literal. Absolute URLs
    /// (`http://host/x`, `https://…/x`) → the path (`/x`); already-relative
    /// paths (`/x`) pass through; a bare host, a non-path string, or a variable
    /// → None. Query/fragment are dropped. Lets a client call to
    /// `http://api/users` pair with route `/users` (addresses the host-prefix
    /// normalisation gap, handoff Pattern I). Interpolation reconstruction
    /// (`$id`/`${expr}`/f-string) stays per-parser — call this AFTER it.
    pub fn url_to_path(raw: &str) -> Option<String> {
        let s = raw.trim();
        let after_host = if let Some(i) = s.find("://") {
            let rest = &s[i + 3..];
            match rest.find('/') {
                Some(j) => &rest[j..],
                None => "/",
            }
        } else {
            s
        };
        if !after_host.starts_with('/') {
            return None;
        }
        let end = after_host.find(['?', '#']).unwrap_or(after_host.len());
        Some(after_host[..end].to_string())
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
        let qname = format!("endpoint:{}:{}", ep.method, ep.path);
        let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::ENDPOINT, &qname);
        if seen.insert(id) {
            nodes.push(Node {
                id,
                repo,
                confidence: ep.confidence,
                cells: vec![Cell {
                    kind: cell_type::ENDPOINT_HIT,
                    payload: CellPayload::Json(endpoint_hit_json(ep)),
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
