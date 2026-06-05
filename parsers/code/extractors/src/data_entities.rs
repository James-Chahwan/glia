//! Cross-cutting data-entity extraction (v0.4.x — DB resolver substrate).
//!
//! Where `data_sources.rs` emits coarse provider buckets (`postgres`,
//! `mongodb`), this extractor pulls the fine-grained entity names — Tables,
//! Collections, NodeLabels — that `DbResolver` joins across services.
//!
//! Single node kind `DATA_ENTITY` is shared across SQL / NoSQL / Graph-DB
//! flavors via the qname prefix:
//!   `data_entity:sql:<table>`        — relational tables
//!   `data_entity:nosql:<collection>` — document collections
//!   `data_entity:graph:<label>`      — graph node labels
//!
//! Recognised shapes (intentionally narrow for v1 — long tail in v0.5+):
//!   - Raw SQL (any language with a string literal): `FROM/JOIN/INTO/UPDATE`
//!   - SQLAlchemy / Django: `__tablename__ = '...'` / `db_table = '...'`
//!   - Mongoose: `mongoose.model('<Name>', ...)`
//!   - Cypher: `MATCH (x:<Label>)` / `MERGE (x:<Label>)` (label-only; query
//!     parsing punted)

use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, edge_category, node_kind};
use repo_graph_core::{Confidence, Edge, Node, NodeId, RepoId};

pub struct DataEntityNodes {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub nav: CodeNav,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DataEntityFlavor {
    Sql,
    Nosql,
    Graph,
}

impl DataEntityFlavor {
    fn as_str(self) -> &'static str {
        match self {
            Self::Sql => "sql",
            Self::Nosql => "nosql",
            Self::Graph => "graph",
        }
    }
}

pub fn extract_data_entity_nodes(
    source: &str,
    module_id: NodeId,
    repo: RepoId,
) -> DataEntityNodes {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut nav = CodeNav::default();
    let mut seen: std::collections::HashSet<(DataEntityFlavor, String)> =
        std::collections::HashSet::new();

    let mut emit = |flavor: DataEntityFlavor, name: &str| {
        // Flavor-agnostic noise gate: numerics and English/JS keywords are never
        // real table/collection/label names, regardless of how they were
        // captured (raw SQL, `.collection('callback')`, etc.). (glia-v2 G7)
        if is_noise_entity_name(name) {
            return;
        }
        let key = (flavor, name.to_string());
        if !seen.insert(key.clone()) {
            return;
        }
        let qname = format!("data_entity:{}:{}", flavor.as_str(), name);
        let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::DATA_ENTITY, &qname);
        nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Medium,
            cells: vec![],
        });
        nav.record(id, name, &qname, node_kind::DATA_ENTITY, Some(module_id));
        edges.push(Edge {
            from: module_id,
            to: id,
            category: edge_category::ACCESSES_DATA,
            confidence: Confidence::Medium,
        });
    };

    // Only scan for raw-SQL table refs when the source actually contains a SQL
    // statement. Without this gate the `FROM`/`JOIN`/`INTO`/`UPDATE` scan fires
    // on ordinary English/JS ("copied from this", `Array.from(callback)`,
    // `Intl.DateTimeFormat`), minting bogus `data_entity:sql:*` nodes on repos
    // with zero SQL. (glia-v2 G7)
    if has_sql_context(source) {
        for name in scan_sql_tables(source) {
            emit(DataEntityFlavor::Sql, &name);
        }
    }
    for name in scan_orm_table_decls(source) {
        emit(DataEntityFlavor::Sql, &name);
    }
    for name in scan_mongoose_models(source) {
        emit(DataEntityFlavor::Nosql, &name);
    }
    for name in scan_dynamodb_tables(source) {
        emit(DataEntityFlavor::Nosql, &name);
    }
    for name in scan_collection_calls(source) {
        emit(DataEntityFlavor::Nosql, &name);
    }
    for name in scan_beanie_documents(source) {
        emit(DataEntityFlavor::Nosql, &name);
    }
    for name in scan_cypher_labels(source) {
        emit(DataEntityFlavor::Graph, &name);
    }

    DataEntityNodes { nodes, edges, nav }
}

// ----------------------------------------------------------------------------
// Raw SQL: pull table names from `FROM <name>`, `JOIN <name>`, `INTO <name>`,
// `UPDATE <name>` clauses inside string literals. Case-insensitive on the
// keyword, identifier-shaped on the name.
// ----------------------------------------------------------------------------

/// True when `source` contains an unambiguous SQL statement signature. Gates
/// the raw-SQL table scan so plain prose/JS that happens to use the words
/// `from`/`join`/`into`/`update` doesn't get mistaken for SQL. (glia-v2 G7)
fn has_sql_context(source: &str) -> bool {
    let lower = source.to_ascii_lowercase();
    const SIG: &[&str] = &[
        "select ",
        "insert into",
        "delete from",
        "create table",
        "alter table",
        "truncate table",
        "merge into",
    ];
    if SIG.iter().any(|s| lower.contains(s)) {
        return true;
    }
    // `UPDATE <table> SET ...` — the verb alone is too common, pair it with SET.
    lower.contains("update ") && lower.contains(" set ")
}

fn scan_sql_tables(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for keyword in ["FROM", "JOIN", "INTO", "UPDATE"] {
        let mut search_from = 0;
        let bytes = source.as_bytes();
        let kw_lower = keyword.to_ascii_lowercase();
        while search_from < source.len() {
            let Some(rel) = find_keyword_ci(&source[search_from..], keyword, &kw_lower) else {
                break;
            };
            let pos = search_from + rel;
            let after_kw = pos + keyword.len();
            // Require at least one whitespace before the identifier.
            let mut k = after_kw;
            while k < bytes.len() && (bytes[k] == b' ' || bytes[k] == b'\t' || bytes[k] == b'\n') {
                k += 1;
            }
            // Optional quoting: "users", `users`, [users] (SQL Server). Strip and
            // capture the identifier inside.
            let (start, end) = if k < bytes.len() && (bytes[k] == b'"' || bytes[k] == b'`') {
                let delim = bytes[k];
                let s = k + 1;
                let mut j = s;
                while j < bytes.len() && bytes[j] != delim {
                    j += 1;
                }
                (s, j)
            } else if k < bytes.len() && bytes[k] == b'[' {
                let s = k + 1;
                let mut j = s;
                while j < bytes.len() && bytes[j] != b']' {
                    j += 1;
                }
                (s, j)
            } else {
                let s = k;
                let mut j = s;
                while j < bytes.len() && is_sql_ident_char(bytes[j]) {
                    j += 1;
                }
                (s, j)
            };
            if end > start {
                let raw = &source[start..end];
                if let Some(cleaned) = canonical_sql_name(raw) {
                    out.push(cleaned);
                }
            }
            search_from = after_kw + 1;
        }
    }
    out
}

/// Find next case-insensitive occurrence of `kw_upper` (ASCII), preceded by a
/// non-word char (so `FROM` matches but `<EOL>FROM` and `FROMSOMETHING` don't).
fn find_keyword_ci(hay: &str, kw_upper: &str, kw_lower: &str) -> Option<usize> {
    let bytes = hay.as_bytes();
    let kw_len = kw_upper.len();
    if bytes.len() < kw_len {
        return None;
    }
    let upper = kw_upper.as_bytes();
    let lower = kw_lower.as_bytes();
    let mut i = 0;
    while i + kw_len <= bytes.len() {
        let mut ok = true;
        for j in 0..kw_len {
            if bytes[i + j] != upper[j] && bytes[i + j] != lower[j] {
                ok = false;
                break;
            }
        }
        if ok {
            let prev_ok = i == 0 || {
                let p = bytes[i - 1];
                !(p.is_ascii_alphanumeric() || p == b'_')
            };
            // Followed by whitespace so we don't match `FROMSOMETHING`.
            let next = bytes.get(i + kw_len).copied().unwrap_or(0);
            let next_ok = next == b' ' || next == b'\t' || next == b'\n';
            if prev_ok && next_ok {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

fn is_sql_ident_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'.'
}

/// True for captured "names" that are never real data entities, whatever the
/// flavor: pure numerics (`FROM 2`) and English/JS keywords that follow
/// `from`/`into` in prose or get passed to `.collection(...)`. Applied at the
/// single `emit` funnel so SQL, NoSQL and graph flavors are all protected.
/// (glia-v2 G7)
fn is_noise_entity_name(name: &str) -> bool {
    let name = name.trim();
    if name.is_empty() {
        return true;
    }
    if name.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    matches!(
        name.to_ascii_lowercase().as_str(),
        "this"
            | "that"
            | "these"
            | "those"
            | "it"
            | "them"
            | "self"
            | "here"
            | "there"
            | "where"
            | "within"
            | "callback"
            | "provided"
            | "above"
            | "below"
            | "which"
            | "what"
            | "async"
            | "await"
            | "return"
            | "import"
            | "export"
            | "undefined"
            | "null"
    )
}

/// Strip schema prefix and noise; reject SQL keywords / placeholders that
/// would otherwise leak through (`SELECT`, `?`, `:param`).
fn canonical_sql_name(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > 128 {
        return None;
    }
    // Strip schema: `public.users` → `users`.
    let last = raw.rsplit('.').next().unwrap_or(raw);
    // Reject anything that doesn't look like an identifier — guards against
    // catching `?`, parameter markers, parens.
    if !last.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    if last.is_empty() {
        return None;
    }
    let upper = last.to_ascii_uppercase();
    // Reject SQL keywords that can appear right after FROM/JOIN/etc.
    if matches!(
        upper.as_str(),
        "SELECT" | "WHERE" | "AND" | "OR" | "IF" | "EXISTS" | "NULL" | "TRUE" | "FALSE"
    ) {
        return None;
    }
    Some(last.to_string())
}

// ----------------------------------------------------------------------------
// ORM table declarations: `__tablename__ = 'users'` (SQLAlchemy) and
// `db_table = 'users'` (Django Meta inner class).
// ----------------------------------------------------------------------------

fn scan_orm_table_decls(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for needle in ["__tablename__", "db_table"] {
        let mut search_from = 0;
        while let Some(rel) = source[search_from..].find(needle) {
            let pos = search_from + rel;
            let after = &source[pos + needle.len()..];
            // Look for `= '<name>'` or `= "<name>"`.
            if let Some(eq) = after.find('=') {
                let tail = &after[eq + 1..];
                if let Some(name) = first_quoted_string(tail) {
                    if let Some(cleaned) = canonical_sql_name(&name) {
                        out.push(cleaned);
                    }
                }
            }
            search_from = pos + needle.len();
        }
    }
    out
}

/// Read the first single-/double-quoted string literal in `s`.
fn first_quoted_string(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'\'' || c == b'"' || c == b'`' {
            let delim = c;
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j] != delim {
                if bytes[j] == b'\\' && j + 1 < bytes.len() {
                    j += 2;
                } else {
                    j += 1;
                }
            }
            if j < bytes.len() {
                return Some(s[start..j].to_string());
            }
            return None;
        }
        i += 1;
    }
    None
}

// ----------------------------------------------------------------------------
// Mongoose: `mongoose.model('User', schema)` and `model('User', schema)`.
// ----------------------------------------------------------------------------

fn scan_mongoose_models(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for needle in ["mongoose.model(", "models.model("] {
        let mut search_from = 0;
        while let Some(rel) = source[search_from..].find(needle) {
            let pos = search_from + rel;
            let after = &source[pos + needle.len()..];
            if let Some(name) = first_quoted_string(after) {
                if !name.is_empty() && name.len() < 128 {
                    out.push(name);
                }
            }
            search_from = pos + needle.len();
        }
    }
    out
}

// ----------------------------------------------------------------------------
// DynamoDB: `TableName: 'users'` (JS SDK v3 command objects), `TableName='users'`
// (Python boto3 kwargs), and `dynamodb.Table('users')` (boto3 resource API).
// ----------------------------------------------------------------------------

fn scan_dynamodb_tables(source: &str) -> Vec<String> {
    let mut out = Vec::new();

    // Object-key / kwarg form: `TableName: '...'`, `TableName: "..."`,
    // `TableName='...'`. The needle ends at the colon/equals so any whitespace
    // before the value is allowed.
    for needle in ["TableName:", "TableName =", "TableName="] {
        let mut search_from = 0;
        while let Some(rel) = source[search_from..].find(needle) {
            let pos = search_from + rel;
            // Word-boundary check on the preceding char so we don't match
            // `MyTableName:`.
            let prev_ok = pos == 0 || {
                let p = source.as_bytes()[pos - 1];
                !(p.is_ascii_alphanumeric() || p == b'_' || p == b'$')
            };
            let after = &source[pos + needle.len()..];
            if prev_ok {
                if let Some(name) = first_quoted_string(after) {
                    if !name.is_empty() && name.len() < 128 {
                        out.push(name);
                    }
                }
            }
            search_from = pos + needle.len();
        }
    }

    // boto3 resource form: `dynamodb.Table('users')`. The `dynamodb.` prefix
    // disambiguates from generic `.Table(...)` builders in other libs.
    let mut search_from = 0;
    while let Some(rel) = source[search_from..].find("dynamodb.Table(") {
        let pos = search_from + rel;
        let after = &source[pos + "dynamodb.Table(".len()..];
        if let Some(name) = first_quoted_string(after) {
            if !name.is_empty() && name.len() < 128 {
                out.push(name);
            }
        }
        search_from = pos + "dynamodb.Table(".len();
    }
    out
}

// ----------------------------------------------------------------------------
// `.collection('<name>')` — covers both Firestore (`db.collection('users')`)
// and the native MongoDB driver (`db.collection('users')`). Both targets land
// in the NoSQL flavor namespace, which is exactly what the resolver wants —
// the join semantic doesn't care which client wrote the data.
// ----------------------------------------------------------------------------

fn scan_collection_calls(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    let needle = ".collection(";
    let bytes = source.as_bytes();
    let mut search_from = 0;
    while let Some(rel) = source[search_from..].find(needle) {
        let pos = search_from + rel;
        // Require something before the `.` — protects against weird leading
        // contexts but doesn't gate on receiver name (Firestore + Mongo both
        // legitimate).
        if pos == 0 {
            search_from = pos + needle.len();
            continue;
        }
        // Word-boundary on what precedes the `.` so `someother.collection(`
        // matches but `_collection(` (no `.`) doesn't.
        let prev = bytes[pos - 1];
        if !(prev.is_ascii_alphanumeric() || prev == b'_' || prev == b')' || prev == b']')
        {
            search_from = pos + needle.len();
            continue;
        }
        let after = &source[pos + needle.len()..];
        if let Some(name) = first_quoted_string(after) {
            if !name.is_empty() && name.len() < 128 {
                out.push(name);
            }
        }
        search_from = pos + needle.len();
    }
    out
}

// ----------------------------------------------------------------------------
// Beanie (Pydantic + Motor): `class Foo(Document): class Settings: name = "..."`.
// The collection name lives on a `Settings` inner class. Scan for `class
// Settings:` and pull `name = '<value>'` from the next ~256 bytes.
// ----------------------------------------------------------------------------

fn scan_beanie_documents(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    let needle = "class Settings:";
    let mut search_from = 0;
    while let Some(rel) = source[search_from..].find(needle) {
        let pos = search_from + rel;
        let after = pos + needle.len();
        let win_end = (after + 256).min(source.len());
        let window = &source[after..win_end];
        if let Some(name_idx) = find_word_in(window, "name") {
            let tail = &window[name_idx + "name".len()..];
            // Expect `name = '...'` or `name='...'`; require the equals.
            if let Some(eq) = tail.find('=') {
                let after_eq = &tail[eq + 1..];
                if let Some(name) = first_quoted_string(after_eq) {
                    if !name.is_empty() && name.len() < 128 {
                        out.push(name);
                    }
                }
            }
        }
        search_from = after;
    }
    out
}

/// Find the first occurrence of `word` in `s` with word-boundary checks on
/// both sides. Returns the byte offset of the start of the match.
fn find_word_in(s: &str, word: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let wb = word.as_bytes();
    let mut i = 0;
    while i + wb.len() <= bytes.len() {
        if &bytes[i..i + wb.len()] == wb {
            let prev_ok = i == 0 || {
                let p = bytes[i - 1];
                !(p.is_ascii_alphanumeric() || p == b'_')
            };
            let next_ok = i + wb.len() == bytes.len() || {
                let n = bytes[i + wb.len()];
                !(n.is_ascii_alphanumeric() || n == b'_')
            };
            if prev_ok && next_ok {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

// ----------------------------------------------------------------------------
// Cypher labels: `MATCH (x:Label)` / `MERGE (x:Label)`. Label-only — query
// shape (relationships, properties, return clauses) deferred to v0.5+.
// ----------------------------------------------------------------------------

fn scan_cypher_labels(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for keyword in ["MATCH", "MERGE", "CREATE"] {
        let kw_lower = keyword.to_ascii_lowercase();
        let mut search_from = 0;
        while search_from < source.len() {
            let Some(rel) = find_keyword_ci(&source[search_from..], keyword, &kw_lower) else {
                break;
            };
            let pos = search_from + rel;
            // Look forward for `(<var>:<Label>)` shapes within the next
            // ~256 bytes (one statement worth).
            let win_end = (pos + 256).min(source.len());
            let win = &source[pos..win_end];
            for label in extract_cypher_labels_in_window(win) {
                out.push(label);
            }
            search_from = pos + keyword.len() + 1;
        }
    }
    out
}

/// Pull `:Label` tokens that follow a `(` or `,` and a node-variable. Matches
/// `(p:Person)`, `(:Person)`, `(p:Person:Admin)` (colon-separated labels).
fn extract_cypher_labels_in_window(win: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = win.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'(' {
            // Inside a node pattern. Scan until `)` for `:Label` substrings.
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] != b')' {
                if bytes[j] == b':' {
                    let s = j + 1;
                    let mut k = s;
                    while k < bytes.len()
                        && (bytes[k].is_ascii_alphanumeric() || bytes[k] == b'_')
                    {
                        k += 1;
                    }
                    if k > s {
                        let label = &win[s..k];
                        if !label.is_empty() && label.len() < 128 {
                            out.push(label.to_string());
                        }
                    }
                    j = k;
                    continue;
                }
                j += 1;
            }
            i = j + 1;
            continue;
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module_id(repo: RepoId) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MODULE, "test")
    }

    fn entity_qnames(out: &DataEntityNodes) -> Vec<String> {
        out.nav.qname_by_id.values().cloned().collect()
    }

    #[test]
    fn raw_sql_from_join_into_update() {
        let repo = RepoId(1);
        let src = r#"
const q1 = "SELECT * FROM users WHERE id = ?";
const q2 = "INSERT INTO posts (title) VALUES (?)";
const q3 = "UPDATE comments SET text = ? WHERE id = ?";
const q4 = "SELECT u.* FROM users u JOIN orders o ON u.id = o.user_id";
"#;
        let out = extract_data_entity_nodes(src, module_id(repo), repo);
        let qnames = entity_qnames(&out);
        assert!(qnames.contains(&"data_entity:sql:users".to_string()));
        assert!(qnames.contains(&"data_entity:sql:posts".to_string()));
        assert!(qnames.contains(&"data_entity:sql:comments".to_string()));
        assert!(qnames.contains(&"data_entity:sql:orders".to_string()));
    }

    #[test]
    fn no_sql_context_means_no_sql_entities() {
        // Plain TS/JS with prose + JS idioms that use the words from/into/update
        // but contain ZERO SQL. Pre-fix this minted data_entity:sql:this,
        // :callback, :DateTimeFormat etc. (glia-v2 G7)
        let repo = RepoId(1);
        let src = r#"
// adapted from this gist; update within the callback provided above
const fmt = new Intl.DateTimeFormat('en');
const items = Array.from(callback(this));
this.update(2);
"#;
        let out = extract_data_entity_nodes(src, module_id(repo), repo);
        let qnames = entity_qnames(&out);
        assert!(
            qnames.iter().all(|q| !q.starts_with("data_entity:sql:")),
            "expected zero sql entities in SQL-free source, got {qnames:?}"
        );
    }

    #[test]
    fn nosql_collection_rejects_keyword_and_numeric_names() {
        // The flavor-agnostic noise gate also protects NoSQL captures: a quoted
        // `.collection('callback')` must not mint data_entity:nosql:callback,
        // and a numeric collection name is never real. (glia-v2 G7)
        let repo = RepoId(1);
        let src = r#"
db.collection('callback');
db.collection('2');
db.collection('users');
"#;
        let out = extract_data_entity_nodes(src, module_id(repo), repo);
        let qnames = entity_qnames(&out);
        assert!(qnames.contains(&"data_entity:nosql:users".to_string()));
        assert!(!qnames.contains(&"data_entity:nosql:callback".to_string()));
        assert!(!qnames.contains(&"data_entity:nosql:2".to_string()));
    }

    #[test]
    fn sql_strips_schema_prefix() {
        let repo = RepoId(1);
        let src = "const q = \"SELECT * FROM public.users JOIN reporting.events e ON e.uid = users.id\";";
        let out = extract_data_entity_nodes(src, module_id(repo), repo);
        let qnames = entity_qnames(&out);
        assert!(qnames.contains(&"data_entity:sql:users".to_string()));
        assert!(qnames.contains(&"data_entity:sql:events".to_string()));
    }

    #[test]
    fn sql_does_not_match_fromsomething() {
        // word-boundary check: FROMUSERS shouldn't match.
        let repo = RepoId(1);
        let src = "const word = \"FROMUSERS is a column name\";";
        let out = extract_data_entity_nodes(src, module_id(repo), repo);
        assert!(out.nodes.is_empty(), "FROMUSERS must not match `FROM users`");
    }

    #[test]
    fn sqlalchemy_tablename_decl() {
        let repo = RepoId(1);
        let src = r#"
class User(db.Model):
    __tablename__ = 'users'
    id = Column(Integer, primary_key=True)

class Post(db.Model):
    __tablename__ = "posts"
"#;
        let out = extract_data_entity_nodes(src, module_id(repo), repo);
        let qnames = entity_qnames(&out);
        assert!(qnames.contains(&"data_entity:sql:users".to_string()));
        assert!(qnames.contains(&"data_entity:sql:posts".to_string()));
    }

    #[test]
    fn django_db_table_meta() {
        let repo = RepoId(1);
        let src = r#"
class User(models.Model):
    name = models.CharField()
    class Meta:
        db_table = 'auth_users'
"#;
        let out = extract_data_entity_nodes(src, module_id(repo), repo);
        let qnames = entity_qnames(&out);
        assert!(qnames.contains(&"data_entity:sql:auth_users".to_string()));
    }

    #[test]
    fn mongoose_model() {
        let repo = RepoId(1);
        let src = r#"
import mongoose from 'mongoose';
const userSchema = new mongoose.Schema({ name: String });
export const User = mongoose.model('User', userSchema);
export const Post = mongoose.model('Post', postSchema);
"#;
        let out = extract_data_entity_nodes(src, module_id(repo), repo);
        let qnames = entity_qnames(&out);
        assert!(qnames.contains(&"data_entity:nosql:User".to_string()));
        assert!(qnames.contains(&"data_entity:nosql:Post".to_string()));
    }

    #[test]
    fn cypher_match_merge_labels() {
        let repo = RepoId(1);
        let src = r#"
const q1 = "MATCH (p:Person) RETURN p";
const q2 = "MERGE (u:User {id: $id})";
const q3 = "MATCH (a:Account)-[:OWNS]->(b:Wallet)";
"#;
        let out = extract_data_entity_nodes(src, module_id(repo), repo);
        let qnames = entity_qnames(&out);
        assert!(qnames.contains(&"data_entity:graph:Person".to_string()));
        assert!(qnames.contains(&"data_entity:graph:User".to_string()));
        assert!(qnames.contains(&"data_entity:graph:Account".to_string()));
        assert!(qnames.contains(&"data_entity:graph:Wallet".to_string()));
    }

    #[test]
    fn dynamodb_table_name_object_key() {
        let repo = RepoId(1);
        let src = r#"
import { DynamoDBClient, PutItemCommand } from '@aws-sdk/client-dynamodb';
const client = new DynamoDBClient({});
await client.send(new PutItemCommand({
    TableName: 'users',
    Item: { id: { S: '123' } },
}));
await client.send(new GetItemCommand({ TableName: "sessions", Key: {} }));
"#;
        let out = extract_data_entity_nodes(src, module_id(repo), repo);
        let qnames = entity_qnames(&out);
        assert!(qnames.contains(&"data_entity:nosql:users".to_string()));
        assert!(qnames.contains(&"data_entity:nosql:sessions".to_string()));
    }

    #[test]
    fn dynamodb_python_boto3_table() {
        let repo = RepoId(1);
        let src = r#"
import boto3
dynamodb = boto3.resource('dynamodb')
table = dynamodb.Table('users')
result = table.get_item(Key={'id': '123'})

# kwarg style on client
client = boto3.client('dynamodb')
client.put_item(TableName='audit_log', Item={})
"#;
        let out = extract_data_entity_nodes(src, module_id(repo), repo);
        let qnames = entity_qnames(&out);
        assert!(qnames.contains(&"data_entity:nosql:users".to_string()));
        assert!(qnames.contains(&"data_entity:nosql:audit_log".to_string()));
    }

    #[test]
    fn dynamodb_does_not_match_suffix_keys() {
        // `MyTableName:` and `LegacyTableName:` must not match `TableName:`.
        let repo = RepoId(1);
        let src = r#"
const config = { MyTableName: 'something', LegacyTableName: 'else' };
"#;
        let out = extract_data_entity_nodes(src, module_id(repo), repo);
        assert!(out.nodes.is_empty(), "suffix keys must not match TableName:");
    }

    #[test]
    fn firestore_and_mongo_collection_calls() {
        let repo = RepoId(1);
        let src = r#"
// Firestore
import { getFirestore } from 'firebase-admin/firestore';
const db = getFirestore();
const usersRef = db.collection('users');
const ordersRef = db.collection("orders");

// Native MongoDB driver — same shape, same flavor bucket.
const conn = await MongoClient.connect(uri);
const sessions = conn.db('app').collection('sessions');
"#;
        let out = extract_data_entity_nodes(src, module_id(repo), repo);
        let qnames = entity_qnames(&out);
        assert!(qnames.contains(&"data_entity:nosql:users".to_string()));
        assert!(qnames.contains(&"data_entity:nosql:orders".to_string()));
        assert!(qnames.contains(&"data_entity:nosql:sessions".to_string()));
    }

    #[test]
    fn collection_call_requires_dot_prefix() {
        // Bare `collection('users')` (no receiver) shouldn't match.
        let repo = RepoId(1);
        let src = r#"
const x = collection('not-a-call');
"#;
        let out = extract_data_entity_nodes(src, module_id(repo), repo);
        assert!(out.nodes.is_empty(), "bare collection() must not match");
    }

    #[test]
    fn beanie_settings_name() {
        let repo = RepoId(1);
        let src = r#"
from beanie import Document
from pydantic import Field

class User(Document):
    email: str
    name: str

    class Settings:
        name = "users"

class Post(Document):
    title: str

    class Settings:
        name = 'posts'
        use_revision = True
"#;
        let out = extract_data_entity_nodes(src, module_id(repo), repo);
        let qnames = entity_qnames(&out);
        assert!(qnames.contains(&"data_entity:nosql:users".to_string()));
        assert!(qnames.contains(&"data_entity:nosql:posts".to_string()));
    }

    #[test]
    fn beanie_skips_non_settings_class_with_name_field() {
        // `class Meta:` (Django) inside a Document-like body must NOT match —
        // Beanie strictly uses `class Settings:`.
        let repo = RepoId(1);
        let src = r#"
class User(models.Model):
    email = models.CharField()
    class Meta:
        name = "users"
"#;
        let out = extract_data_entity_nodes(src, module_id(repo), repo);
        // Django's `db_table` extractor catches its own shape; this fixture has
        // no `db_table`, so should be empty.
        assert!(
            out.nodes.is_empty(),
            "class Meta: with name field is not Beanie — must not emit"
        );
    }

    #[test]
    fn dedupes_within_file() {
        // Same table referenced twice should produce one node.
        let repo = RepoId(1);
        let src = r#"
const q1 = "SELECT * FROM users";
const q2 = "INSERT INTO users (name) VALUES (?)";
"#;
        let out = extract_data_entity_nodes(src, module_id(repo), repo);
        let users_count = out
            .nav
            .qname_by_id
            .values()
            .filter(|q| q.as_str() == "data_entity:sql:users")
            .count();
        assert_eq!(users_count, 1);
    }
}
