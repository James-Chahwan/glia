//! Cross-cutting data-source extraction (D1, v0.4.11a).
//!
//! Detects DB / cache / blob-store / search / email client usage in a source
//! file and emits a DATA_SOURCE node per unique (kind, provider) pair plus an
//! `ACCESSES_DATA` edge from the enclosing module. Nodes are keyed by
//! `data_source:<provider>` (global qname) so multiple modules that reach the
//! same external dependency converge on a single node.
//!
//! Substring matching is kept deliberately — we want broad framework coverage
//! across 20 languages without per-language syntactic analysis. False-positive
//! risk is bounded by the medium confidence tier and by keeping patterns
//! distinctive (`pg.` not `pg`, `sqlx::` not `sqlx`, etc.).
//!
//! CJ.1c: in a Rust or Python file a pattern counts only at an occurrence
//! whose first byte is code ([`LazyGuard`]): a scanner's own needle table, a
//! doc comment or a test's embedded sample source names a client, it does not
//! use one. Inside code the match stays a substring
//! (`QueueFramework::RedisList` still reads as redis); every other language
//! is unchanged. fired_on: `[code-guard] data_sources lang=.. dropped=..`.

use glia_code_domain::{CodeNav, GRAPH_TYPE, edge_category, node_kind};
use glia_core::{Confidence, Edge, Node, NodeId, NodeKindId, RepoId};

use crate::code_guard::LazyGuard;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataSourceKind {
    Database,
    Cache,
    BlobStore,
    Search,
    Email,
}

impl DataSourceKind {
    fn node_kind_id(self) -> NodeKindId {
        match self {
            Self::Database => node_kind::DATABASE,
            Self::Cache => node_kind::CACHE,
            Self::BlobStore => node_kind::BLOB_STORE,
            Self::Search => node_kind::SEARCH_INDEX,
            Self::Email => node_kind::EMAIL_SERVICE,
        }
    }
}

pub struct DataSourceNodes {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub nav: CodeNav,
}

/// Scan `source` (read from `path`, which picks the CJ.1c guard; `""` = no
/// guard) for data-source client usage and emit nodes + edges anchored to
/// `module_id`. Dedups by provider within a single file.
pub fn extract_data_source_nodes(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
) -> DataSourceNodes {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut nav = CodeNav::default();
    let mut seen = std::collections::HashSet::new();
    let mut guard = LazyGuard::new(path, source);

    for (pattern, kind, provider) in PATTERNS {
        // A provider already emitted takes nothing more from this file, so
        // its later patterns are never scanned (or counted as refusals).
        if seen.contains(provider) {
            continue;
        }
        // The first occurrence the guard admits counts; the scan stops there.
        if !source
            .match_indices(pattern)
            .any(|(at, _)| guard.admits(at))
        {
            continue;
        }
        seen.insert(*provider);
        let qname = format!("data_source:{provider}");
        let id = NodeId::from_parts(GRAPH_TYPE, repo, kind.node_kind_id(), &qname);
        nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Medium,
            cells: vec![],
        });
        nav.record(id, provider, &qname, kind.node_kind_id(), Some(module_id));
        edges.push(Edge {
            from: module_id,
            to: id,
            category: edge_category::ACCESSES_DATA,
            confidence: Confidence::Medium,
            cells: Vec::new(),
        });
    }
    guard.report("data_sources");

    DataSourceNodes { nodes, edges, nav }
}

const PATTERNS: &[(&str, DataSourceKind, &str)] = &[
    // ----- Database -----
    ("pg.", DataSourceKind::Database, "postgres"),
    ("psycopg", DataSourceKind::Database, "postgres"),
    ("mysql.", DataSourceKind::Database, "mysql"),
    ("sqlite", DataSourceKind::Database, "sqlite"),
    ("MongoClient", DataSourceKind::Database, "mongodb"),
    ("mongoose.", DataSourceKind::Database, "mongodb"),
    ("Prisma", DataSourceKind::Database, "prisma"),
    ("TypeORM", DataSourceKind::Database, "typeorm"),
    ("Sequelize", DataSourceKind::Database, "sequelize"),
    ("sqlalchemy", DataSourceKind::Database, "sqlalchemy"),
    ("diesel::", DataSourceKind::Database, "diesel"),
    ("sqlx::", DataSourceKind::Database, "sqlx"),
    ("ActiveRecord", DataSourceKind::Database, "activerecord"),
    ("Ecto.Repo", DataSourceKind::Database, "ecto"),
    ("gorm.", DataSourceKind::Database, "gorm"),
    ("database/sql", DataSourceKind::Database, "database_sql"),
    // ----- Cache -----
    ("Redis", DataSourceKind::Cache, "redis"),
    ("redis.", DataSourceKind::Cache, "redis"),
    ("Memcached", DataSourceKind::Cache, "memcached"),
    ("memcache", DataSourceKind::Cache, "memcached"),
    // ----- Blob store -----
    ("S3Client", DataSourceKind::BlobStore, "s3"),
    ("boto3.client('s3')", DataSourceKind::BlobStore, "s3"),
    ("BlobServiceClient", DataSourceKind::BlobStore, "azure_blob"),
    ("google-cloud-storage", DataSourceKind::BlobStore, "gcs"),
    ("@google-cloud/storage", DataSourceKind::BlobStore, "gcs"),
    // ----- Search -----
    ("Elasticsearch", DataSourceKind::Search, "elasticsearch"),
    ("OpenSearch", DataSourceKind::Search, "opensearch"),
    ("Algolia", DataSourceKind::Search, "algolia"),
    ("Meilisearch", DataSourceKind::Search, "meilisearch"),
    ("Typesense", DataSourceKind::Search, "typesense"),
    // ----- Email -----
    ("SendGrid", DataSourceKind::Email, "sendgrid"),
    ("Mailgun", DataSourceKind::Email, "mailgun"),
    ("nodemailer", DataSourceKind::Email, "nodemailer"),
    ("smtplib", DataSourceKind::Email, "smtp"),
    ("@aws-sdk/client-ses", DataSourceKind::Email, "ses"),
    ("postmark", DataSourceKind::Email, "postmark"),
    ("Resend", DataSourceKind::Email, "resend"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use glia_code_domain::GRAPH_TYPE;

    fn module_id(repo: RepoId) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MODULE, "test")
    }

    #[test]
    fn detects_database_node_and_edge() {
        let repo = RepoId(1);
        let mid = module_id(repo);
        let out =
            extract_data_source_nodes("const db = new MongoClient(url);", "", mid, repo);
        assert_eq!(out.nodes.len(), 1);
        assert_eq!(out.edges.len(), 1);
        assert_eq!(out.edges[0].from, mid);
        assert_eq!(out.edges[0].category, edge_category::ACCESSES_DATA);
        assert_eq!(
            out.nav.kind_by_id.get(&out.nodes[0].id).copied(),
            Some(node_kind::DATABASE)
        );
    }

    #[test]
    fn detects_multiple_kinds() {
        let repo = RepoId(1);
        let mid = module_id(repo);
        let source = "import Redis from 'redis';\nconst es = new Elasticsearch();";
        let out = extract_data_source_nodes(source, "", mid, repo);
        let kinds: Vec<_> = out
            .nodes
            .iter()
            .map(|n| out.nav.kind_by_id.get(&n.id).copied().unwrap())
            .collect();
        assert!(kinds.contains(&node_kind::CACHE));
        assert!(kinds.contains(&node_kind::SEARCH_INDEX));
    }

    #[test]
    fn dedupes_provider_within_file() {
        let repo = RepoId(1);
        let mid = module_id(repo);
        let out = extract_data_source_nodes(
            "redis.set(k, v);\nRedis.Client.new();",
            "",
            mid,
            repo,
        );
        assert_eq!(out.nodes.len(), 1, "same provider deduped once");
    }

    #[test]
    fn node_qname_is_global() {
        let repo = RepoId(1);
        let mid = module_id(repo);
        let out = extract_data_source_nodes("use diesel::prelude::*;", "", mid, repo);
        let qname = out.nav.qname_by_id.get(&out.nodes[0].id).unwrap();
        assert_eq!(qname, "data_source:diesel");
    }

    fn providers(src: &str, path: &str) -> Vec<String> {
        let repo = RepoId(1);
        let out = extract_data_source_nodes(src, path, module_id(repo), repo);
        let mut q: Vec<String> = out.nav.qname_by_id.values().cloned().collect();
        q.sort();
        q
    }

    #[test]
    fn literal_patterns_mint_nothing_in_rust_and_python() {
        let rs = "let db = \"new MongoClient(url)\";\n// SendGrid, Elasticsearch\n";
        assert!(providers(rs, "src/x.rs").is_empty());
        // The same Rust text read as TypeScript keeps the HEAD substring match.
        assert_eq!(
            providers(rs, "src/x.ts"),
            ["data_source:elasticsearch", "data_source:mongodb", "data_source:sendgrid"]
        );
        // Python code keeps its client; a docstring / comment mention does not.
        assert_eq!(
            providers("import redis\nr = redis.Redis()\n", "app/x.py"),
            ["data_source:redis"]
        );
        assert!(providers("\"\"\"Uses MongoClient.\"\"\"\n# smtplib\n", "app/x.py").is_empty());
        // A refused occurrence does not hide a later one in code.
        assert_eq!(
            providers("// Redis\nlet c = Redis::open(url);\n", "src/x.rs"),
            ["data_source:redis"]
        );
    }
}
