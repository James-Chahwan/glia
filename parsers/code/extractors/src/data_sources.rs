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
/// `module_id`. Dedups by provider within a single file. `test_file` (the
/// caller's test-path rule) turns on HQ.8's test-double rules; `false` reads
/// exactly as before.
pub fn extract_data_source_nodes(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
    test_file: bool,
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
    let (mut test_camel, mut test_quoted) = (0usize, 0usize);
    let go = path
        .rsplit_once('.')
        .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("go"));
    // HQ.8: the providers a test file mocks, in PATTERNS order.
    let mocked = if test_file {
        mocked_providers(source)
    } else {
        Vec::new()
    };

    for (pattern, kind, provider) in PATTERNS {
        // A provider already emitted takes nothing more from this file, so
        // its later patterns are never scanned (or counted as refusals).
        if seen.contains(provider) || mocked.contains(provider) {
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
            if test_file && camel_middle(source, at, pattern) {
                test_camel += 1;
                return false;
            }
            if test_file && in_quoted_prose(source, at, go) {
                test_quoted += 1;
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
    let test_mocked = mocked.len();
    if refused_descriptor + refused_word + test_mocked + test_camel + test_quoted > 0 {
        eprintln!(
            "[data-sources] refused jvm_descriptor={refused_descriptor} word_tail={refused_word} \
             test_mocked={test_mocked} test_camel={test_camel} test_quoted={test_quoted} path={path}"
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
    !is_word_needle(pattern)
        || !source
            .as_bytes()
            .get(at + pattern.len())
            .is_some_and(u8::is_ascii_lowercase)
}

/// FD.73's word-needle class: first byte ASCII uppercase, last an ASCII
/// letter (`Redis`, `Prisma`, `MongoClient`).
fn is_word_needle(pattern: &str) -> bool {
    let p = pattern.as_bytes();
    p.first().is_some_and(u8::is_ascii_uppercase) && p.last().is_some_and(u8::is_ascii_alphabetic)
}

/// HQ.8 (test files): true when a word needle's occurrence at `at` follows an
/// ASCII alphanumeric byte, a camel middle (`ProcessStaleRedisKeysJob`,
/// `mockPrisma`): the test names its own type or local, not the client.
fn camel_middle(source: &str, at: usize, pattern: &str) -> bool {
    is_word_needle(pattern) && at > 0 && source.as_bytes()[at - 1].is_ascii_alphanumeric()
}

/// HQ.8: the starts of a line that imports (its quoted text names a library).
const IMPORT_LINE_PREFIXES: &[&str] = &[
    "import ", "import{", "from ", "require", "use ", "using ", "@import", "#include",
];

/// HQ.8: text anywhere on a line that makes it an import line (a CommonJS
/// `require(..)`, a dynamic `import(..)`, the `} from '..'` tail of a
/// multi-line import).
const IMPORT_LINE_INFIXES: &[&str] = &[
    "require(",
    "require '",
    "require \"",
    "import(",
    " from '",
    " from \"",
];

/// HQ.8 (test files): true when the occurrence at `at` sits inside quoted
/// text (an odd count of one unescaped `'`, `"` or `` ` `` before it on its
/// line) on a line that imports nothing: a test title, a Kotlin backtick
/// name, an assertion message. A per-line count, not a lexer. In a Go file a
/// line holding only an optional alias and one string is an import-block line.
fn in_quoted_prose(source: &str, at: usize, go: bool) -> bool {
    let b = source.as_bytes();
    let start = b[..at]
        .iter()
        .rposition(|&c| c == b'\n')
        .map_or(0, |i| i + 1);
    let end = b[at..]
        .iter()
        .position(|&c| c == b'\n')
        .map_or(b.len(), |i| at + i);
    let line = &source[start..end];
    let trimmed = line.trim_start();
    if IMPORT_LINE_PREFIXES.iter().any(|p| trimmed.starts_with(p))
        || IMPORT_LINE_INFIXES.iter().any(|p| line.contains(p))
        || (go && go_import_spec(trimmed))
    {
        return false;
    }
    let before = &b[start..at];
    [b'\'', b'"', b'`'].iter().any(|&q| {
        before
            .iter()
            .enumerate()
            .filter(|&(i, &c)| {
                c == q
                    && before[..i]
                        .iter()
                        .rev()
                        .take_while(|&&p| p == b'\\')
                        .count()
                        % 2
                        == 0
            })
            .count()
            % 2
            == 1
    })
}

/// A Go import-block line: an optional alias (`_`, `.` or an identifier) and
/// one double-quoted or raw string, nothing else (a trailing comment allowed).
fn go_import_spec(line: &str) -> bool {
    let line = line.split("//").next().unwrap_or_default().trim_end();
    let rest = line
        .trim_start_matches(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '.')
        .trim_start();
    let q = match rest.as_bytes().first() {
        Some(&c @ (b'"' | b'`')) => c,
        _ => return false,
    };
    rest.len() >= 2
        && rest.as_bytes()[rest.len() - 1] == q
        && !rest[1..rest.len() - 1].contains(q as char)
}

/// The argument span a mock opener takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MockSpan {
    /// Up to the matching `>`.
    Generic,
    /// Up to the matching `)`.
    Call,
    /// The rest of the declaration: to `;` or the end of the line, the next
    /// line too when the annotation stands alone on its line.
    Annotation,
}

/// HQ.8: the mock constructs a test file builds a double with, per language.
/// An opener counts at an identifier boundary only (`double(` not
/// `toDouble(`; an annotation also ends at one, so `@Mock` is not `@MockBean`).
const MOCK_OPENERS: &[(&str, MockSpan)] = &[
    // TypeScript / JavaScript
    ("mockDeep<", MockSpan::Generic),
    ("mock<", MockSpan::Generic),
    ("createMock<", MockSpan::Generic),
    ("DeepMockProxy<", MockSpan::Generic),
    ("Mocked<", MockSpan::Generic),
    ("jest.mock(", MockSpan::Call),
    ("vi.mock(", MockSpan::Call),
    ("jest.mocked(", MockSpan::Call),
    ("vi.mocked(", MockSpan::Call),
    // Python
    ("MagicMock(", MockSpan::Call),
    ("AsyncMock(", MockSpan::Call),
    ("create_autospec(", MockSpan::Call),
    ("patch(", MockSpan::Call),
    ("patch.object(", MockSpan::Call),
    ("mocker.patch(", MockSpan::Call),
    // Ruby
    ("instance_double(", MockSpan::Call),
    ("class_double(", MockSpan::Call),
    ("object_double(", MockSpan::Call),
    ("double(", MockSpan::Call),
    ("spy(", MockSpan::Call),
    // JVM
    ("mock(", MockSpan::Call),
    ("Mockito.mock(", MockSpan::Call),
    ("mockk<", MockSpan::Generic),
    ("mockk(", MockSpan::Call),
    ("spyk(", MockSpan::Call),
    ("@Mock", MockSpan::Annotation),
    ("@MockBean", MockSpan::Annotation),
    ("@MockK", MockSpan::Annotation),
    ("@SpyBean", MockSpan::Annotation),
    // C#
    ("new Mock<", MockSpan::Generic),
    ("Substitute.For<", MockSpan::Generic),
    ("A.Fake<", MockSpan::Generic),
];

/// Longest argument span a mock opener takes, in bytes.
const MOCK_SPAN_CAP: usize = 256;

/// HQ.8: the byte ranges of `source` inside a mock construct's argument span,
/// in source order (overlaps kept).
fn mock_spans(source: &str) -> Vec<(usize, usize)> {
    let b = source.as_bytes();
    let mut spans = Vec::new();
    for &(opener, shape) in MOCK_OPENERS {
        for (at, _) in source.match_indices(opener) {
            if at > 0 && is_ident_byte(b[at - 1]) {
                continue;
            }
            let body = at + opener.len();
            if shape == MockSpan::Annotation && b.get(body).is_some_and(|&c| is_ident_byte(c)) {
                continue;
            }
            let cap = (body + MOCK_SPAN_CAP).min(b.len());
            let end = match shape {
                MockSpan::Generic => matching_close(b, body, cap, b'<', b'>'),
                MockSpan::Call => matching_close(b, body, cap, b'(', b')'),
                MockSpan::Annotation => annotation_end(b, body, cap),
            };
            spans.push((body, end));
        }
    }
    spans.sort_unstable();
    spans
}

/// The byte after `open`'s opener whose `close` balances it (depth one at
/// `from`), or `cap` when none does within it.
fn matching_close(b: &[u8], from: usize, cap: usize, open: u8, close: u8) -> usize {
    let mut depth = 1usize;
    for (i, &c) in b[from..cap].iter().enumerate() {
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return from + i;
            }
        }
    }
    cap
}

/// An annotation's declaration: to `;` or the end of its line; when nothing
/// but blanks follows the annotation on its line, the next line to `;` / its end.
fn annotation_end(b: &[u8], from: usize, cap: usize) -> usize {
    let line_end = |s: usize| {
        b[s..cap]
            .iter()
            .position(|&c| c == b'\n' || c == b';')
            .map_or(cap, |i| s + i)
    };
    let end = line_end(from);
    let alone = b[from..end].iter().all(u8::is_ascii_whitespace);
    if alone && b.get(end) == Some(&b'\n') && end + 1 < cap {
        line_end(end + 1)
    } else {
        end
    }
}

/// HQ.8: the providers a test file mocks, deduplicated in PATTERNS order: a
/// provider whose needle occurs inside a mock span (FD.73's descriptor and
/// word-tail rules as for any occurrence).
fn mocked_providers(source: &str) -> Vec<&'static str> {
    let spans = mock_spans(source);
    let mut out: Vec<&'static str> = Vec::new();
    if spans.is_empty() {
        return out;
    }
    for &(pattern, _, provider) in PATTERNS {
        if out.contains(&provider) {
            continue;
        }
        let hit = source.match_indices(pattern).any(|(at, _)| {
            spans.iter().any(|&(s, e)| s <= at && at < e)
                && !in_jvm_descriptor(source, at)
                && word_tail_ok(source, at, pattern)
        });
        if hit {
            out.push(provider);
        }
    }
    out
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
        let out =
            extract_data_source_nodes("const db = new MongoClient(url);", "", mid, repo, false);
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
        let out = extract_data_source_nodes(source, "", mid, repo, false);
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
            false,
        );
        assert_eq!(out.nodes.len(), 1, "same provider deduped once");
    }

    #[test]
    fn node_qname_is_global() {
        let repo = RepoId(1);
        let mid = module_id(repo);
        let out = extract_data_source_nodes("use diesel::prelude::*;", "", mid, repo, false);
        let qname = out.nav.qname_by_id.get(&out.nodes[0].id).unwrap();
        assert_eq!(qname, "data_source:diesel");
    }

    fn providers(src: &str, path: &str) -> Vec<String> {
        providers_in(src, path, false)
    }

    fn providers_in(src: &str, path: &str, test_file: bool) -> Vec<String> {
        let repo = RepoId(1);
        let out = extract_data_source_nodes(src, path, module_id(repo), repo, test_file);
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

    /// HQ.8: the three held-out wrong shapes (a mocked client, a camel-middle
    /// job name, a backtick test name) mint nothing in a test file and keep
    /// their BASE edge outside one; a real client and an import in a test keep
    /// theirs.
    const HOPP: &str = "import { mockDeep } from 'jest-mock-extended';\n\
        import { PrismaService } from '../prisma/prisma.service';\n\
        const mockPrisma = mockDeep<PrismaService>();\n\
        mockPrisma.order.findMany.mockResolvedValue([]);\n";
    const RSPEC: &str = "require 'rails_helper'\n\nRSpec.describe Internal::RemoveStaleRedisKeysJob do\n  \
        it 'enqueues' do\n    expect(Internal::ProcessStaleRedisKeysJob).to receive(:perform_later)\n  end\nend\n";
    const KT: &str = "import org.redisson.codec.Kryo5Codec\n\nclass NameCodecTest {\n    @Test\n    \
        fun `keeps writing names when Redisson rebinds the codec to a classloader`() {\n        \
        Kryo5Codec().valueEncoder.encode(\"x\")\n    }\n}\n";

    #[test]
    fn test_doubles_are_no_data_access() {
        assert!(providers_in(HOPP, "src/m.service.spec.ts", true).is_empty());
        assert!(providers_in(RSPEC, "spec/jobs/r_spec.rb", true).is_empty());
        assert!(providers_in(KT, "src/test/kotlin/C.kt", true).is_empty());
        // Outside a test file: unchanged from BASE.
        assert_eq!(
            providers_in(HOPP, "src/m.service.spec.ts", false),
            ["data_source:prisma"]
        );
        assert_eq!(
            providers_in(RSPEC, "spec/jobs/r_spec.rb", false),
            ["data_source:redis"]
        );
        assert_eq!(
            providers_in(KT, "src/test/kotlin/C.kt", false),
            ["data_source:redis"]
        );
    }

    #[test]
    fn real_clients_in_tests_still_read() {
        let cases: &[(&str, &str)] = &[
            (
                "redis = Redis.new\nredis.set('warm', '1')\n",
                "spec/jobs/w_spec.rb",
            ),
            ("import 'package:redis/redis.dart';\n", "test/r_test.dart"),
            ("const Redis = require('ioredis');\n", "src/a.spec.ts"),
            ("import { Redis } from 'ioredis';\n", "src/b.spec.ts"),
            ("import {\n  Redis,\n} from 'ioredis';\n", "src/c.spec.ts"),
            (
                "package r\n\nimport (\n\t\"database/sql\"\n)\n",
                "r/x_test.go",
            ),
        ];
        let want = |p: &str| {
            if p.ends_with(".go") {
                vec!["data_source:database_sql".to_string()]
            } else {
                vec!["data_source:redis".to_string()]
            }
        };
        for (src, path) in cases {
            assert_eq!(providers_in(src, path, true), want(path), "{path}: {src}");
        }
        assert_eq!(
            providers_in(
                "import sqlalchemy as sa\n\ndef test_x(s):\n    s.execute(sa.select(sa.func.count()))\n",
                "tests/test_x.py",
                true
            ),
            ["data_source:sqlalchemy"]
        );
    }

    #[test]
    fn mock_openers_per_language() {
        let mocked: &[(&str, &str)] = &[
            ("r = MagicMock(spec=Redis)\n", "tests/test_a.py"),
            (
                "@patch('app.cache.redis.get')\ndef test_x(m): pass\n",
                "tests/test_b.py",
            ),
            (
                "let(:r) { instance_double(Redis) }\nRedis.new\n",
                "spec/a_spec.rb",
            ),
            ("val r = mockk<RedissonClient>()\n", "src/test/A.kt"),
            (
                "@MockBean\nprivate RedisTemplate t;\nRedis x;\n",
                "src/test/B.java",
            ),
            ("@Mock lateinit var r: RedissonClient\n", "src/test/C.kt"),
            (
                "var r = new Mock<IRedisClient>();\nvar c = new Redis();\n",
                "tests/D.cs",
            ),
            ("var r = Substitute.For<Redis>();\n", "tests/E.cs"),
            ("jest.mock('ioredis', () => Redis);\n", "src/f.spec.ts"),
        ];
        for (src, path) in mocked {
            assert!(
                providers_in(src, path, true).is_empty(),
                "{path}: {src}: {:?}",
                providers_in(src, path, true)
            );
        }
        // An opener counts at an identifier boundary only.
        assert_eq!(
            providers_in("x.toDouble(Redis.n)\nRedis.new\n", "spec/b_spec.rb", true),
            ["data_source:redis"]
        );
        // `@Mock` alone is no `@MockitoSettings`.
        assert_eq!(
            providers_in("@MockitoSettings Redis r;\n", "src/test/F.java", true),
            ["data_source:redis"]
        );
    }

    #[test]
    fn quoted_and_camel_shapes() {
        let q = |s: &str| in_quoted_prose(s, s.find("Redis").unwrap(), false);
        assert!(q("it('drops Redis keys', () => {"));
        assert!(q("fun `when Redis rebinds`() {"));
        assert!(q("assert x, \"Redis down\""));
        assert!(!q("const c = Redis.create('a');"));
        assert!(!q("x = 'a'; Redis.new"));
        assert!(!q("s = 'it\\'s'; Redis.new"));
        assert!(!q("import Redis from 'redis';"));
        assert!(!q("  } from 'Redis';"));
        assert!(in_quoted_prose("\t\"x/sqlite\"\n", 4, false));
        assert!(!in_quoted_prose("\t\"x/sqlite\"\n", 4, true));
        assert!(!in_quoted_prose("\tdb \"x/sqlite\" // driver\n", 7, true));
        let c = |s: &str, p: &str| camel_middle(s, s.find(p).unwrap(), p);
        assert!(c("ProcessStaleRedisKeysJob", "Redis"));
        assert!(c("mockPrisma", "Prisma"));
        assert!(!c("Redis.new", "Redis"));
        assert!(!c("<PrismaService>", "Prisma"));
        assert!(!c("app_redis.get", "redis."));
    }
}
