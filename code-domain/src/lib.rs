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
    /// `{"provenance":"build_output|vendored|generated|authored","region":"www","files":N}`.
    /// Lets consumers (engram, neuropil) filter by *coordinate* rather than by
    /// string-matching keys, and preserves the spatial map of a repo without
    /// emitting a node per file inside a collapsed region. (glia-v2 G10/G14)
    pub const ORIGIN: CellTypeId = CellTypeId(15);
    /// External library names imported in a node's source file — a JSON array
    /// `["ethers","web3"]` (deduped, sorted, capped). Denormalized per-node so
    /// the engram exporter can fill `Content::Symbol.imports` without a parent
    /// lookup; gives the encoder library context. (glia-v5 G15)
    pub const IMPORTS: CellTypeId = CellTypeId(16);
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
        // Go: the parser stores import paths `::`-joined (mapped to repo qnames)
        // or `/`-joined; take the last non-empty segment either way.
        "go" => p
            .rsplit(|c| c == '/' || c == ':')
            .find(|s| !s.is_empty())
            .map(str::to_string),
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
#[derive(Debug, Default)]
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
#[derive(Debug, Default, Clone)]
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
