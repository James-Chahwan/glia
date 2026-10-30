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
//! whose first byte is code (`LazyGuard`): a scanner's own needle table, a
//! doc comment or a test's embedded sample source names a client, it does not
//! use one. Inside code the match stays a substring
//! (`QueueFramework::RedisList` still reads as redis); every other language
//! is unchanged. fired_on: `[code-guard] data_sources lang=.. dropped=..`.
//!
//! FD.73: two shape rules on an occurrence, in every language and before the
//! guard: one inside a JVM type descriptor (`Landroid/database/sqlite/X;`,
//! jnigen's bindings, whole or split across adjacent literals) names no
//! client (`in_jvm_descriptor`), and a capitalised word needle (`Redis`,
//! `Prisma`, `Resend` ..) reads only at a camel or word tail, never inside a
//! longer word like `Redistribution` (`word_tail_ok`). The rules key on the
//! text, not on the file: a generated sqlc / Prisma client really opens a
//! database. fired_on, once per file with a refusal:
//! `[data-sources] refused jvm_descriptor=<j> word_tail=<w> path=<p>`.

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
    let rust = path
        .rsplit_once('.')
        .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("rs"));
    let (mut refused_descriptor, mut refused_word) = (0usize, 0usize);

    for (pattern, kind, provider) in PATTERNS {
        // A provider already emitted takes nothing more from this file, so
        // its later patterns are never scanned (or counted as refusals).
        if seen.contains(provider) {
            continue;
        }
        // The first occurrence the guard admits counts; the scan stops there.
        // FD.5a: a library name in a Ruby / PHP / Dart / Elixir import
        // string counts (`import 'package:redis/redis.dart'`), one in a
        // comment does not.
        // FD.6: in a Rust file a needle also has to read as a Rust path
        // ([`rust_path_reading`]), so a local `redis.contains(..)` or a
        // type `PrismaSchema` names no library.
        // FD.73: a JVM type descriptor or a longer word holds no client
        // name; both refusals are shape rules, decided before the guard.
        let mut crate_named: Option<bool> = None;
        if !source.match_indices(pattern).any(|(at, _)| {
            if in_jvm_descriptor(source, at) {
                refused_descriptor += 1;
                return false;
            }
            if !word_tail_ok(source, at, pattern) {
                refused_word += 1;
                return false;
            }
            let reads = !rust
                || rust_path_occurrence(source, at, pattern)
                || match crate_named {
                    Some(named) => named,
                    None => {
                        let named = rust_names_crate(source, provider, &mut guard);
                        crate_named = Some(named);
                        named
                    }
                };
            reads && guard.admits_name(at)
        }) {
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
    if refused_descriptor > 0 || refused_word > 0 {
        eprintln!(
            "[data-sources] refused jvm_descriptor={refused_descriptor} word_tail={refused_word} path={path}"
        );
    }

    DataSourceNodes { nodes, edges, nav }
}

fn is_ident_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// FD.6: true when the `pattern` occurrence at byte `at` of a Rust file names
/// its library the way Rust code does. The table's needles are written for
/// the dynamic languages (`redis.`, `gorm.`, `Prisma`), and in Rust such text
/// is mostly code naming a library in its own identifiers (a local `redis`, a
/// type `PrismaSchema`, a fn `sqlalchemy_tablename_decl`: glia's own
/// extractors). An occurrence counts when the needle is itself a path
/// (`diesel::`) or the identifier it starts is a path's first segment
/// (`Redis::open(..)`, `Elasticsearch::new(..)`); else only when the file
/// names the provider as a crate ([`rust_names_crate`]).
fn rust_path_occurrence(source: &str, at: usize, pattern: &str) -> bool {
    if pattern.ends_with("::") {
        return true;
    }
    let b = source.as_bytes();
    if at > 0 && is_ident_byte(b[at - 1]) {
        return false;
    }
    let mut end = at;
    while end < b.len() && is_ident_byte(b[end]) {
        end += 1;
    }
    end > at && b[end..].starts_with(b"::")
}

/// FD.6: true when a Rust file names `provider` as a crate path in code
/// (the guard admits it) at an identifier start, ASCII case ignored
/// (`use redis::Commands;`, `mongodb::Client::with_uri_str(..)`); a
/// `"Redis::publish("` needle-table string names none.
fn rust_names_crate(source: &str, provider: &str, guard: &mut LazyGuard<'_>) -> bool {
    let b = source.as_bytes();
    let path = format!("{provider}::");
    let n = path.len();
    b.windows(n).enumerate().any(|(i, w)| {
        w.eq_ignore_ascii_case(path.as_bytes())
            && (i == 0 || !is_ident_byte(b[i - 1]))
            && guard.admits(i)
    })
}

/// A byte of a JVM descriptor token: a class-name character, the inner-class
/// `$` or the package separator `/`.
fn is_descriptor_byte(c: u8) -> bool {
    is_ident_byte(c) || c == b'$' || c == b'/'
}

/// FD.73: true when the occurrence at byte `at` sits inside a JVM type
/// descriptor (`(Ljava/lang/String;ILandroid/database/sqlite/X;)V`): JNI
/// bindings (jnigen's Dart and C, Kotlin / Java JNI glue) name JVM classes
/// there, never a client library. The token around `at` (bytes of
/// [`is_descriptor_byte`]) must open after a descriptor delimiter
/// (`( ) ; [ " '`, or the file start), read `L<pkg>/..` once its leading JVM
/// primitive letters (`[BCDFIJSZ]`) are stripped, hold a `/` before `at`, and
/// end at `;` (the class's end) or at a `/` right before a closing quote
/// (jnigen's C splits one descriptor across adjacent literals). So
/// `"modernc.org/sqlite"` (the run stops at `.`), `"Lib/sqlite/x.db"` and a Go
/// `"database/sql"` import (no `L` class start) are no descriptor.
/// Language-blind: a descriptor is the same text in every language.
fn in_jvm_descriptor(source: &str, at: usize) -> bool {
    let b = source.as_bytes();
    let mut start = at;
    while start > 0 && is_descriptor_byte(b[start - 1]) {
        start -= 1;
    }
    if start > 0 && !matches!(b[start - 1], b'(' | b')' | b';' | b'[' | b'"' | b'\'') {
        return false;
    }
    let mut class = start;
    while class < at
        && matches!(
            b[class],
            b'B' | b'C' | b'D' | b'F' | b'I' | b'J' | b'S' | b'Z'
        )
    {
        class += 1;
    }
    if class >= at || b[class] != b'L' || !b[class..at].contains(&b'/') {
        return false;
    }
    let mut end = at;
    while end < b.len() && is_descriptor_byte(b[end]) {
        end += 1;
    }
    match b.get(end) {
        Some(b';') => true,
        Some(b'"' | b'\'') => b[end - 1] == b'/',
        _ => false,
    }
}

/// FD.73: true unless `pattern` is a capitalised word needle (first byte
/// ASCII uppercase, last byte an ASCII letter: `Redis`, `Prisma`, `Resend`,
/// `MongoClient` ..) whose occurrence at `at` runs on into a longer lowercase
/// word (`Redistribution`, `Prismatic`, `Resending`). A camel tail
/// (`RedisTemplate`) or any non-lowercase byte (`.`, `(`, `;`, a quote, a
/// space) ends the word. Lowercase, dotted, `::` and path needles are
/// unchanged, so `memcache` still reads `memcached` and `psycopg` `psycopg2`.
fn word_tail_ok(source: &str, at: usize, pattern: &str) -> bool {
    let p = pattern.as_bytes();
    let word = p.first().is_some_and(u8::is_ascii_uppercase)
        && p.last().is_some_and(u8::is_ascii_alphabetic);
    !word
        || !source
            .as_bytes()
            .get(at + p.len())
            .is_some_and(u8::is_ascii_lowercase)
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
    // FD.73: Redisson's `Redis` runs on into `s`, which the word-tail rule
    // refuses, so the JVM client has its own row.
    ("Redisson", DataSourceKind::Cache, "redis"),
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
        let out = extract_data_source_nodes("const db = new MongoClient(url);", "", mid, repo);
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
        let out = extract_data_source_nodes("redis.set(k, v);\nRedis.Client.new();", "", mid, repo);
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
            providers(rs, "src/x.vue"),
            [
                "data_source:elasticsearch",
                "data_source:mongodb",
                "data_source:sendgrid"
            ]
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

    /// FD.6: in Rust a needle reads only as a path, or in a file that names
    /// the provider's crate: glia's own extractors name libraries in locals,
    /// types and fns.
    #[test]
    fn rust_needles_read_only_as_paths() {
        let idents = "fn sqlalchemy_tablename_decl() {}\npub struct PrismaSchema;\n\
                      fn f(redis: &str, acc: Acc) -> bool { acc.gorm.tagged; redis.contains(\"x\") }\n\
                      fn g(pg: Page) { pg.path; }\n";
        assert!(
            providers(idents, "src/x.rs").is_empty(),
            "{:?}",
            providers(idents, "src/x.rs")
        );
        // The same text read as TypeScript keeps the substring reading.
        assert!(!providers(idents, "src/x.ts").is_empty());
        // A path occurrence and a named crate both count.
        assert_eq!(
            providers("let es = Elasticsearch::new(t);\n", "src/x.rs"),
            ["data_source:elasticsearch"]
        );
        assert_eq!(
            providers(
                "use redis::Commands;\nfn f(redis: R) { redis.get(k); }\n",
                "src/x.rs"
            ),
            ["data_source:redis"]
        );
        assert_eq!(
            providers("use diesel::prelude::*;\n", "src/x.rs"),
            ["data_source:diesel"]
        );
        // A crate path inside a string names no crate.
        assert!(
            providers(
                "const N: &str = \"Redis::publish(\";\nfn f(redis: R) { redis.get(k); }\n",
                "src/x.rs"
            )
            .is_empty()
        );
        // `MyRedis::new` starts no path segment named for the library.
        assert!(providers("let c = MyRedis::new();\n", "src/x.rs").is_empty());
    }

    /// FD.73: a JNI type descriptor names JVM classes, not a client library,
    /// whole in a Dart raw string or split across adjacent C literals.
    #[test]
    fn jvm_descriptor_is_no_access() {
        let dart = "class Context {\n  static final _id = jni.JMethodIDPtr(r\"openOrCreateDatabase\",\n      \
                    r\"(Ljava/lang/String;ILandroid/database/sqlite/SQLiteDatabase$CursorFactory;)Landroid/database/sqlite/SQLiteDatabase;\");\n}\n";
        assert!(
            providers(dart, "lib/hc.dart").is_empty(),
            "{:?}",
            providers(dart, "lib/hc.dart")
        );
        let c = "#include \"dartjni.h\"\nvoid f(jclass k) {\n  load_method(k, &_m, \"openOrCreateDatabase\",\n      \
                 \"(Ljava/lang/String;ILandroid/database/sqlite/\"\n      \
                 \"SQLiteDatabase$CursorFactory;)Landroid/database/sqlite/SQLiteDatabase;\");\n}\n";
        assert!(
            providers(c, "src/hc.c").is_empty(),
            "{:?}",
            providers(c, "src/hc.c")
        );
    }

    /// FD.73: a capitalised word needle reads only at a camel or word tail,
    /// never inside a longer lowercase-continued word.
    #[test]
    fn word_needles_end_at_a_word_tail() {
        for src in [
            "const t = '4. Redistribution. You may reproduce';\n",
            "String banner() => 'Prismatic colours';\n",
            "String s() => 'Resending';\n",
        ] {
            assert!(
                providers(src, "lib/l.dart").is_empty(),
                "{src}: {:?}",
                providers(src, "lib/l.dart")
            );
        }
    }

    /// FD.73 controls: real imports and constructors keep their providers.
    #[test]
    fn client_imports_and_constructors_still_read() {
        let cases: &[(&str, &str, &[&str])] = &[
            (
                "import 'package:sqlite3/sqlite3.dart';\nfinal db = sqlite3.open('a.db');\n",
                "lib/store.dart",
                &["data_source:sqlite"],
            ),
            (
                "final c = RedisConnection();\n",
                "lib/cache.dart",
                &["data_source:redis"],
            ),
            (
                "package repo\n\nimport \"database/sql\"\n",
                "go/repo.go",
                &["data_source:database_sql"],
            ),
            (
                "package repo\n\nimport _ \"modernc.org/sqlite\"\n",
                "go/db.go",
                &["data_source:sqlite"],
            ),
            (
                "import org.redisson.Redisson;\nclass C { Object c() { return Redisson.create(); } }\n",
                "jvm/C.java",
                &["data_source:redis"],
            ),
            (
                "const r = new IORedis();\n",
                "src/a.ts",
                &["data_source:redis"],
            ),
            (
                "const r = Redis.createClient();\n",
                "src/b.ts",
                &["data_source:redis"],
            ),
            (
                "using StackExchange.Redis;\n",
                "src/C.cs",
                &["data_source:redis"],
            ),
            (
                "import { PrismaClient } from '@prisma/client';\nconst p = new PrismaClient();\n",
                "src/db.ts",
                &["data_source:prisma"],
            ),
        ];
        for (src, path, want) in cases {
            assert_eq!(providers(src, path), *want, "{path}: {src}");
        }
    }

    #[test]
    fn descriptor_shape() {
        let d = "(Ljava/lang/String;ILandroid/database/sqlite/SQLiteDatabase;)V";
        let at = d.find("sqlite").unwrap();
        assert!(in_jvm_descriptor(d, at));
        let at = d.find("database/sql").unwrap();
        assert!(in_jvm_descriptor(d, at));
        // A primitive / array prefix before the class letter.
        let d = "\"[[Landroid/database/sqlite/X;\"";
        assert!(in_jvm_descriptor(d, d.find("sqlite").unwrap()));
        // jnigen's split: the run stops at a `/` before the closing quote.
        let d = "\"(ILandroid/database/sqlite/\" \"X;)V\"";
        assert!(in_jvm_descriptor(d, d.find("sqlite").unwrap()));
        // Paths that are no descriptor.
        for s in [
            "\"modernc.org/sqlite\"",
            "\"Lib/sqlite/x.db\"",
            "\"database/sql\"",
            "import 'package:sqlite3/sqlite3.dart';",
            "\"Lfoo/sqlite\"",
        ] {
            let at = s.find("sql").unwrap();
            assert!(!in_jvm_descriptor(s, at), "{s}");
        }
    }

    #[test]
    fn word_tail_shape() {
        let ok = |s: &str, p: &str| word_tail_ok(s, s.find(p).unwrap(), p);
        assert!(ok("RedisTemplate", "Redis"));
        assert!(ok("Redis.create", "Redis"));
        assert!(ok("new Redis(", "Redis"));
        assert!(ok("x Redis", "Redis"));
        assert!(!ok("Redistribution", "Redis"));
        assert!(!ok("Prismatic", "Prisma"));
        assert!(!ok("Resending", "Resend"));
        // Lowercase / dotted needles are unchanged.
        assert!(ok("memcached", "memcache"));
        assert!(ok("psycopg2", "psycopg"));
        assert!(ok("redis.get", "redis."));
        assert!(ok("Ecto.Repo.insert", "Ecto.Repo"));
        assert!(!ok("S3Clients", "S3Client"));
    }
}
