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
//!   - Raw SQL (any language with a string literal): `FROM/JOIN/INTO/UPDATE`,
//!     read only from SQL-shaped string literals (LG.3b, [`sql_statements`]):
//!     adjacent literals joined across one concatenation token, the joined
//!     text must open with a statement verb, CTE names, SQL keywords and
//!     function calls are never tables. Comments and prose are never scanned.
//!   - SQL DDL (A13.9): `CREATE [TEMP] TABLE`, `ALTER TABLE`, `DROP TABLE`,
//!     `TRUNCATE [TABLE]`, anchored on the whole phrase, never a bare `TABLE`
//!   - Migration DSLs (A13.9, `migrations::scan_migration_dsl`): Alembic
//!     `op.create_table`, Django `migrations.CreateModel`, Rails
//!     `create_table :t`, knex / Sequelize `createTable`, Laravel
//!     `Schema::create`, EF Core `migrationBuilder.CreateTable`
//!   - SQLAlchemy / Django: `__tablename__ = '...'` / `db_table = '...'`
//!   - Mongoose: `mongoose.model('<Name>', ...)`
//!   - DynamoDB: `TableName: '...'` / `TableName='...'`, `dynamodb.Table('...')`
//!   - Beanie: the `name = '...'` line of a `class Settings:` block
//!   - Driver collection calls: `.collection('x')` (Node / Firestore),
//!     `.Collection("x")` (Go mongo-driver / Firestore), `.getCollection("x")`
//!     (Java), `.GetCollection<T>("x" | nameof(T))` (C# MongoDB.Driver)
//!   - Cypher: `MATCH (x:<Label>)` / `MERGE (x:<Label>)` (label-only; query
//!     parsing punted), read only from Cypher-shaped string literals joined
//!     like SQL (LA.28, [`scan_cypher_labels`]): a Rust `match`, SQL `MERGE
//!     INTO`, prose and comments are not Cypher; `::` and property-map values
//!     are never labels.
//!
//! A declaration (ORM table, Mongoose, DynamoDB, Beanie) is named only by the
//! single-line literal that starts its argument or assignment
//! ([`leading_literal`] / [`inline_literal`], LA.42), never by a later string,
//! and every scanner's name must be [`entity_shaped`] at the emit funnel.
//!
//! Debug markers, one line per file that produced or rejected anything:
//!   `GLIA_DATA_DEBUG=1 ... 2>&1 | grep '\[data-entity\] literals='` (LG.3b)
//!   `GLIA_DATA_DEBUG=1 ... 2>&1 | grep '\[data-entity\] cypher '` (LA.28)
//!   `GLIA_DATA_DEBUG=1 ... 2>&1 | grep '\[data-entity\] decl '` (LA.42)

use std::ops::Range;
use std::sync::OnceLock;

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

/// The one emission funnel: every scanner's captures become DATA_ENTITY nodes
/// hung off `module_id` by ACCESSES_DATA, deduped per (flavor, name) in
/// first-seen order. Shared with `migrations::extract_sql_migration` (A13.9),
/// so a migration `.sql` and a code file mint identical entities.
pub(crate) struct EntitySink {
    module_id: NodeId,
    repo: RepoId,
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    nav: CodeNav,
    // Membership only, never iterated: output order is the push order.
    seen: std::collections::HashSet<(DataEntityFlavor, String)>,
}

impl EntitySink {
    pub(crate) fn new(module_id: NodeId, repo: RepoId) -> Self {
        Self {
            module_id,
            repo,
            nodes: Vec::new(),
            edges: Vec::new(),
            nav: CodeNav::default(),
            seen: std::collections::HashSet::new(),
        }
    }

    pub(crate) fn emit(&mut self, flavor: DataEntityFlavor, name: &str) {
        // Flavor-agnostic noise gate: numerics and English/JS keywords are never
        // real table/collection/label names, regardless of how they were
        // captured (raw SQL, `.collection('callback')`, etc.). (glia-v2 G7)
        if is_noise_entity_name(name) {
            return;
        }
        if !self.seen.insert((flavor, name.to_string())) {
            return;
        }
        let repo = self.repo;
        let qname = format!("data_entity:{}:{}", flavor.as_str(), name);
        let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::DATA_ENTITY, &qname);
        self.nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Medium,
            cells: vec![],
        });
        self.nav.record(
            id,
            name,
            &qname,
            node_kind::DATA_ENTITY,
            Some(self.module_id),
        );
        self.edges.push(Edge {
            from: self.module_id,
            to: id,
            category: edge_category::ACCESSES_DATA,
            confidence: Confidence::Medium,
        });
    }

    pub(crate) fn finish(self) -> DataEntityNodes {
        DataEntityNodes {
            nodes: self.nodes,
            edges: self.edges,
            nav: self.nav,
        }
    }
}

pub fn extract_data_entity_nodes(source: &str, module_id: NodeId, repo: RepoId) -> DataEntityNodes {
    let mut sink = EntitySink::new(module_id, repo);

    // LG.3b: raw SQL is read only from string literals that ARE a SQL
    // statement, never from the whole file. A file-wide scan behind a
    // file-wide `select ` gate read comments, prose strings and Go's
    // `select {` statement as SQL (`copied from the pool` -> table `the`).
    let lit = sql_statements(source);
    let mut stats = ScanStats {
        literals: lit.literals,
        sql: lit.statements.len(),
        rejected_fmt: lit.rejected_fmt,
        ..ScanStats::default()
    };
    for stmt in &lit.statements {
        let scan = statement_tables(&stmt.scan);
        stats.ctes += scan.ctes;
        stats.rejected_fn += scan.rejected_fn;
        for (name, pos) in scan.tables {
            stats.note_table(&name, stmt.to_source(pos));
            sink.emit(DataEntityFlavor::Sql, &name);
        }
    }
    // A13.9: the DDL phrase names its table. After the FROM scan, so a
    // statement the DDL scan adds nothing to keeps its node order.
    for stmt in &lit.statements {
        for (_, name) in scan_sql_ddl(&stmt.scan) {
            // The DDL scan reports no offset; the name's first occurrence in
            // the statement is close enough for the marker's line.
            let pos = stmt.scan.find(name.as_str()).unwrap_or(0);
            stats.note_table(&name, stmt.to_source(pos));
            sink.emit(DataEntityFlavor::Sql, &name);
        }
    }
    // LA.42: the declaration scanners name an entity only from the literal
    // that starts the argument / assignment; `decl` feeds their marker.
    let mut decl = DeclStats::default();
    for name in scan_orm_table_decls(source, &mut decl) {
        decl.funnel(&name);
        sink.emit(DataEntityFlavor::Sql, &name);
    }
    // A13.9: a migration DSL call (`op.create_table("users")`, Rails
    // `create_table :users`) names the table it creates or alters, wherever it
    // appears. The scanner has no path, so the marker says which DSL fired.
    let dsl = crate::migrations::scan_migration_dsl(source);
    for hit in &dsl {
        sink.emit(DataEntityFlavor::Sql, &hit.table);
    }
    for line in crate::migrations::dsl_markers(&dsl) {
        eprintln!("{line}");
    }
    for name in scan_mongoose_models(source, &mut decl) {
        decl.funnel(&name);
        sink.emit(DataEntityFlavor::Nosql, &name);
    }
    for name in scan_dynamodb_tables(source, &mut decl) {
        decl.funnel(&name);
        sink.emit(DataEntityFlavor::Nosql, &name);
    }
    for name in scan_collection_calls(source) {
        stats.note_collection(&name);
        sink.emit(DataEntityFlavor::Nosql, &name);
    }
    for name in scan_beanie_documents(source, &mut decl) {
        decl.funnel(&name);
        sink.emit(DataEntityFlavor::Nosql, &name);
    }
    if debug_enabled() && decl.needles > 0 {
        eprintln!("{}", decl.marker());
    }
    for name in scan_cypher_labels(source) {
        sink.emit(DataEntityFlavor::Graph, &name);
    }

    if debug_enabled() && stats.fired() {
        eprintln!("{}", stats.marker(source));
    }
    sink.finish()
}

/// True when `GLIA_DATA_DEBUG` is set (read once): the `[data-entity]` line
/// is printed per file. The extractor has no path, so the line names no file
/// and is never always-on; run it on one file or fixture to attribute it.
fn debug_enabled() -> bool {
    static DEBUG: OnceLock<bool> = OnceLock::new();
    *DEBUG.get_or_init(|| std::env::var("GLIA_DATA_DEBUG").is_ok_and(|v| !v.is_empty() && v != "0"))
}

/// What one file's scan saw, for the debug marker.
#[derive(Default)]
struct ScanStats {
    literals: usize,
    sql: usize,
    /// `(table, 1-based source line of its first hit)`, first-seen order.
    tables: Vec<(String, usize)>,
    ctes: usize,
    collections: Vec<String>,
    rejected_fn: usize,
    rejected_fmt: usize,
    /// Source offsets of the table hits, turned into lines only when the
    /// marker prints (a newline count per hit is not free).
    table_offsets: Vec<usize>,
}

impl ScanStats {
    fn note_table(&mut self, name: &str, source_offset: usize) {
        if !self.tables.iter().any(|(t, _)| t == name) {
            self.tables.push((name.to_string(), 0));
            self.table_offsets.push(source_offset);
        }
    }

    fn note_collection(&mut self, name: &str) {
        if !self.collections.iter().any(|c| c == name) {
            self.collections.push(name.to_string());
        }
    }

    fn fired(&self) -> bool {
        self.sql > 0
            || !self.collections.is_empty()
            || self.rejected_fn > 0
            || self.rejected_fmt > 0
    }

    /// `[data-entity] literals=N sql=N tables=a@L,b@L ctes=N collections=x,y
    /// rejected_fn=N rejected_fmt=N` (`-` for an empty list; `@L` is the
    /// 1-based source line of the table's first hit).
    fn marker(&mut self, source: &str) -> String {
        let b = source.as_bytes();
        for (slot, &off) in self.tables.iter_mut().zip(&self.table_offsets) {
            let end = off.min(b.len());
            slot.1 = 1 + b[..end].iter().filter(|&&c| c == b'\n').count();
        }
        let tables = if self.tables.is_empty() {
            "-".to_string()
        } else {
            self.tables
                .iter()
                .map(|(t, line)| format!("{t}@{line}"))
                .collect::<Vec<_>>()
                .join(",")
        };
        let collections = if self.collections.is_empty() {
            "-".to_string()
        } else {
            self.collections.join(",")
        };
        format!(
            "[data-entity] literals={} sql={} tables={tables} ctes={} collections={collections} rejected_fn={} rejected_fmt={}",
            self.literals, self.sql, self.ctes, self.rejected_fn, self.rejected_fmt
        )
    }
}

// ----------------------------------------------------------------------------
// String literals (LG.3b). The extractor has no language parameter, so one
// quote-aware rule set that is safe across every host language: comments are
// skipped, every literal form a host writes SQL in is read, and a literal is
// only ever cut at an ASCII delimiter, so every range is on a char boundary.
// ----------------------------------------------------------------------------

/// One string literal: `outer` spans its delimiters (prefixes such as `r#`,
/// `@`, `<<~ID` included), `body` the text between them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Lit {
    pub(crate) outer: Range<usize>,
    pub(crate) body: Range<usize>,
}

/// Every string literal in `source`, in source order. Recognised: `"…"` and
/// `'…'` with backslash escapes (a `'…'` must close on its own line, else it
/// is an apostrophe and is dropped; a `'` inside a word opens nothing unless
/// the word is a string prefix such as `f` / `rb`), backticks (Go raw, JS
/// templates with `${…}` kept as text), triple quotes (Python, Kotlin, Scala,
/// Swift, Java text blocks, C# raw), C# verbatim `@"…"` (`""` escape), Rust
/// raw `r"…"` / `r#"…"#`, and Ruby / PHP heredocs (`<<~ID`, `<<-ID`, `<<ID`,
/// `<<<ID`, `<<<'ID'`, `<<<"ID"`) up to the line that is `ID` (PHP: `ID;`).
/// Skipped as comments: `//` to end of line, `/* … */`, and `# ` to end of
/// line when the `#` starts the line or follows whitespace (Python / Ruby /
/// shell; `#[attr]`, `#include`, `#region`, `this.#x` are untouched). JS /
/// Ruby regex literals are skipped too ([`regex_literal_end`]).
///
/// Linear: a double-quote, backtick or triple-quote form that never closes
/// marks its delimiter as closed-for-good, so a file of unbalanced quotes is
/// not rescanned from every opener; heredoc search is bounded.
pub(crate) fn string_literals(source: &str) -> Vec<Lit> {
    let b = source.as_bytes();
    let n = b.len();
    let mut out = Vec::new();
    // Delimiters (`"`, `` ` ``, triple `"` / `'`) known to have no closer
    // anywhere after the position where a scan for one ran out.
    let mut exhausted = Exhausted::default();
    let mut i = 0;
    while i < n {
        let c = b[i];
        let prev = if i > 0 { b[i - 1] } else { b'\n' };
        let found = match c {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                i = line_end(b, i);
                continue;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i = find_bytes(b, i + 2, b"*/").map_or(n, |j| j + 2);
                continue;
            }
            b'/' => {
                // A JS / Ruby regex literal (`replace(/'/g, "''")`) holds
                // quotes that would flip the parity of every later literal.
                i = regex_literal_end(b, i).unwrap_or(i + 1);
                continue;
            }
            b'#' if prev.is_ascii_whitespace() && matches!(b.get(i + 1), Some(b' ' | b'\t')) => {
                i = line_end(b, i);
                continue;
            }
            b'<' if b.get(i + 1) == Some(&b'<') => heredoc(b, i),
            b'r' if !is_word_byte(prev) || (prev == b'b' && (i < 2 || !is_word_byte(b[i - 2]))) => {
                rust_raw(b, i)
            }
            b'@' | b'$' => csharp_verbatim(b, i),
            b'"' | b'\'' | b'`' => quoted(b, i, &mut exhausted),
            _ => None,
        };
        match found {
            Some(lit) => {
                i = lit.outer.end.max(i + 1);
                out.push(lit);
            }
            None => i += 1,
        }
    }
    out
}

#[derive(Default)]
struct Exhausted {
    double: bool,
    backtick: bool,
    triple_double: bool,
    triple_single: bool,
}

fn is_word_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// Offset of the `\n` ending the line `i` is on, or the source length.
fn line_end(b: &[u8], i: usize) -> usize {
    b[i..]
        .iter()
        .position(|&c| c == b'\n')
        .map_or(b.len(), |p| i + p)
}

fn find_bytes(b: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if from >= b.len() {
        return None;
    }
    b[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| from + p)
}

/// The offset just past a regex literal opening at `i` (a `/` that is not a
/// comment), or `None` when the `/` is a division. A regex can only start
/// where an operand starts: the previous non-blank byte is one of
/// `( , = : [ ! & | ? { ;` or the start of the source, so `a / b`, `(x) / 2`
/// and `</div>` stay divisions and tags. It must close on its own line, past
/// escapes and `[…]` classes; the flags after it are skipped.
fn regex_literal_end(b: &[u8], i: usize) -> Option<usize> {
    let before = b[..i].iter().rev().find(|c| !matches!(c, b' ' | b'\t'));
    if !matches!(
        before,
        None | Some(
            b'(' | b','
                | b'='
                | b':'
                | b'['
                | b'!'
                | b'&'
                | b'|'
                | b'?'
                | b'{'
                | b';'
                | b'\n'
                | b'\r'
        )
    ) {
        return None;
    }
    let mut j = i + 1;
    let mut class = false;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 1,
            b'\n' => return None,
            b'[' => class = true,
            b']' => class = false,
            b'/' if !class => {
                j += 1;
                while j < b.len() && b[j].is_ascii_alphabetic() {
                    j += 1;
                }
                return Some(j);
            }
            _ => {}
        }
        j += 1;
    }
    None
}

/// A `'` inside a word (`don't`, Haskell `foldl'`) opens nothing, unless the
/// word before it is a 1-2 letter string prefix (`f'…'`, `rb'…'`, `u'…'`).
fn apostrophe_opens(b: &[u8], i: usize) -> bool {
    let mut s = i;
    while s > 0 && is_word_byte(b[s - 1]) {
        s -= 1;
    }
    let word = &b[s..i];
    word.is_empty() || (word.len() <= 2 && word.iter().all(|c| b"rRbBuUfF".contains(c)))
}

/// A `"…"`, `'…'`, `` `…` `` or triple-quoted literal opening at `i`.
fn quoted(b: &[u8], i: usize, exhausted: &mut Exhausted) -> Option<Lit> {
    let q = b[i];
    if q == b'\'' && !apostrophe_opens(b, i) {
        return None;
    }
    let run = b[i..].iter().take(3).take_while(|&&c| c == q).count();
    if run == 3 && q != b'`' {
        let flag = if q == b'"' {
            &mut exhausted.triple_double
        } else {
            &mut exhausted.triple_single
        };
        if *flag {
            return None;
        }
        let start = i + 3;
        let mut j = start;
        while j + 2 < b.len() {
            if b[j] == b'\\' {
                j += 2;
                continue;
            }
            if b[j] == q && b[j + 1] == q && b[j + 2] == q {
                return Some(Lit {
                    outer: i..j + 3,
                    body: start..j,
                });
            }
            j += 1;
        }
        *flag = true;
        return None;
    }
    if run == 2 {
        // `""`, `''`, ``` `` ```: the empty literal.
        return Some(Lit {
            outer: i..i + 2,
            body: i + 1..i + 1,
        });
    }
    let flag = match q {
        b'"' => Some(&mut exhausted.double),
        b'`' => Some(&mut exhausted.backtick),
        _ => None,
    };
    if flag.as_ref().is_some_and(|f| **f) {
        return None;
    }
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 2,
            c if c == q => {
                return Some(Lit {
                    outer: i..j + 1,
                    body: i + 1..j,
                });
            }
            // A single-quoted literal never spans lines: this was an
            // apostrophe (prose, `'a` lifetime, `# don't` comment).
            b'\n' if q == b'\'' => return None,
            _ => j += 1,
        }
    }
    if let Some(f) = flag {
        *f = true;
    }
    None
}

/// Rust raw `r"…"` / `r#"…"#` (also after `b`) at `i` (the `r`): no escapes,
/// closed by `"` plus as many `#` as opened.
fn rust_raw(b: &[u8], i: usize) -> Option<Lit> {
    let mut j = i + 1;
    let mut hashes = 0;
    while b.get(j) == Some(&b'#') && hashes < 16 {
        hashes += 1;
        j += 1;
    }
    if b.get(j) != Some(&b'"') {
        return None;
    }
    let start = j + 1;
    let mut k = start;
    while k < b.len() {
        if b[k] == b'"'
            && b.len() >= k + 1 + hashes
            && b[k + 1..k + 1 + hashes].iter().all(|&c| c == b'#')
        {
            return Some(Lit {
                outer: i..k + 1 + hashes,
                body: start..k,
            });
        }
        k += 1;
    }
    None
}

/// C# verbatim `@"…"`, `$@"…"`, `@$"…"` at `i`: `""` is an escaped quote.
fn csharp_verbatim(b: &[u8], i: usize) -> Option<Lit> {
    let open = match (b[i], b.get(i + 1), b.get(i + 2)) {
        (b'@', Some(b'"'), _) => i + 1,
        (b'@', Some(b'$'), Some(b'"')) | (b'$', Some(b'@'), Some(b'"')) => i + 2,
        _ => return None,
    };
    let start = open + 1;
    let mut j = start;
    while j < b.len() {
        if b[j] == b'"' {
            if b.get(j + 1) == Some(&b'"') {
                j += 2;
                continue;
            }
            return Some(Lit {
                outer: i..j + 1,
                body: start..j,
            });
        }
        j += 1;
    }
    None
}

/// Lines searched for a heredoc terminator before the opener is taken for a
/// shift operator (`1<<SHIFT`).
const HEREDOC_MAX_LINES: usize = 4000;

/// A Ruby / PHP heredoc opening at `i` (the first `<`). The body is every line
/// after the opener's line up to the terminator line; the literal ends at the
/// end of the terminator line, and the rest of the opener's line is not read.
fn heredoc(b: &[u8], i: usize) -> Option<Lit> {
    let mut j = i + 2;
    let php = b.get(j) == Some(&b'<');
    let mut flexible = false;
    if php {
        j += 1;
        while matches!(b.get(j), Some(b' ' | b'\t')) {
            j += 1;
        }
    } else if matches!(b.get(j), Some(b'~' | b'-')) {
        flexible = true;
        j += 1;
    }
    let quote = match b.get(j) {
        Some(&q @ (b'\'' | b'"')) => {
            j += 1;
            Some(q)
        }
        _ => None,
    };
    let id_start = j;
    if !b
        .get(j)
        .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_')
    {
        return None;
    }
    while j < b.len() && is_word_byte(b[j]) {
        j += 1;
    }
    let id = &b[id_start..j];
    if quote.is_some_and(|q| b.get(j) != Some(&q)) {
        return None;
    }
    // A bare `<<ID` is a shift (`1<<SHIFT`, `Vec<<T as X>::Y>`) unless the id
    // is an upper-case word of two or more bytes and a terminator follows.
    let bare = !php && !flexible && quote.is_none();
    if bare
        && (id.len() < 2
            || !id
                .iter()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == b'_'))
    {
        return None;
    }
    let body_start = line_end(b, j) + 1;
    if body_start > b.len() {
        return None;
    }
    let mut line_start = body_start;
    for _ in 0..HEREDOC_MAX_LINES {
        if line_start >= b.len() {
            return None;
        }
        let end = line_end(b, line_start);
        let line = &b[line_start..end];
        let trimmed_start = line
            .iter()
            .position(|c| !c.is_ascii_whitespace())
            .unwrap_or(line.len());
        let t = &line[trimmed_start..];
        let t = &t[..t.len()
            - t.iter()
                .rev()
                .take_while(|c| c.is_ascii_whitespace())
                .count()];
        let is_terminator = if php {
            t.starts_with(id) && t.get(id.len()).is_none_or(|c| !is_word_byte(*c))
        } else {
            t == id
        };
        if is_terminator {
            return Some(Lit {
                outer: i..end,
                body: body_start..line_start,
            });
        }
        line_start = end + 1;
    }
    None
}

// ----------------------------------------------------------------------------
// SQL statements: adjacent literals joined across concatenation, kept when the
// joined text is a SQL statement.
// ----------------------------------------------------------------------------

/// One piece of a joined statement: `(source_start, text_start, len)` — the
/// literal body at `source_start` is `text[text_start..text_start + len]`.
pub(crate) type Piece = (usize, usize, usize);

/// A SQL statement built from one or more adjacent string literals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SqlStmt {
    /// The literal bodies, concatenated (a builder step adds a `\n` between).
    pub(crate) text: String,
    /// `text` with `\n`-style escapes, SQL comments and single-quoted values
    /// blanked ([`blank_escapes`], [`blank_sql_noise`]), and for a builder
    /// fragment everything outside its subqueries ([`keep_subqueries`]):
    /// same length, same offsets. Every scan reads it.
    pub(crate) scan: String,
    pub(crate) pieces: Vec<Piece>,
}

impl SqlStmt {
    /// The byte offset in the original source of `text_pos` (an offset into
    /// `text` / `scan`). Pieces start and end at ASCII delimiters, so the
    /// result is a char boundary whenever `text_pos` is. LE.4a re-homes each
    /// ACCESSES_DATA site to its enclosing function by this offset.
    pub(crate) fn to_source(&self, text_pos: usize) -> usize {
        let piece = self
            .pieces
            .iter()
            .rev()
            .find(|(_, t, _)| *t <= text_pos)
            .or(self.pieces.first());
        match piece {
            Some(&(s, t, _)) => s + text_pos.saturating_sub(t),
            None => text_pos,
        }
    }
}

/// The SQL statements of a source file, plus what the marker reports.
pub(crate) struct LiteralSql {
    /// String literals seen (before joining).
    pub(crate) literals: usize,
    pub(crate) statements: Vec<SqlStmt>,
    /// Joined literals that opened with a SQL verb but carry a Go fmt verb no
    /// SQL driver uses (`delete from spaces key=%q: %w`).
    pub(crate) rejected_fmt: usize,
}

/// The string literals of a source file, grouped into the runtime values they
/// build ([`joined_literals`]). Shared by the SQL scan ([`sql_statements`])
/// and the Cypher scan ([`cypher_statements`], LA.28).
struct JoinedLiterals {
    /// Every string literal, in source order.
    lits: Vec<Lit>,
    /// `joints[k]`: how literal k attaches to literal k - 1 (`joints[0]` is
    /// [`Joint::Apart`]).
    joints: Vec<Joint>,
}

impl JoinedLiterals {
    /// The joined runs in source order, each as its literals and how each one
    /// attaches to the one before it. Never empty.
    fn groups(&self) -> impl Iterator<Item = (&[Lit], &[Joint])> + '_ {
        let n = self.lits.len();
        let mut start = 0;
        (1..=n).filter_map(move |k| {
            if k < n && self.joints[k] != Joint::Apart {
                return None;
            }
            let run = start..k;
            start = k;
            Some((&self.lits[run.clone()], &self.joints[run]))
        })
    }
}

/// Group adjacent literals whose gap holds only whitespace and at most one
/// concatenation token (`+` Go / Java / JS / C#, `.` PHP, `\` a line
/// continuation, nothing at all for Python / C implicit concatenation), so
/// `"SELECT o.id " +\n "FROM orders o"` is one value, and the lines of a
/// string builder (see [`Joint::Builder`]). Nothing is joined yet: a scan
/// checks a run's first word before paying for [`join_group`].
fn joined_literals(source: &str) -> JoinedLiterals {
    let lits = string_literals(source);
    let b = source.as_bytes();
    let joints = (0..lits.len())
        .map(|k| {
            if k == 0 {
                Joint::Apart
            } else {
                joint(b, &lits[k - 1], &lits[k])
            }
        })
        .collect();
    JoinedLiterals { lits, joints }
}

/// One run's literal bodies back to back, and where each came from. A builder
/// append (`AppendLine`, `WriteString`, `+=`) is one line of the value: a
/// `\n` keeps its tokens apart.
fn join_group(source: &str, group: &[Lit], joints: &[Joint]) -> (String, Vec<Piece>) {
    let mut text = String::new();
    let mut pieces = Vec::with_capacity(group.len());
    for (lit, j) in group.iter().zip(joints) {
        if *j == Joint::Builder {
            text.push('\n');
        }
        pieces.push((lit.body.start, text.len(), lit.body.len()));
        text.push_str(&source[lit.body.clone()]);
    }
    (text, pieces)
}

/// The joined literals ([`joined_literals`]) that are SQL: kept iff the run
/// opens a SQL statement ([`opens_sql_statement`]) or is a builder fragment
/// holding a subquery ([`has_subquery`]), and is not a Go fmt message
/// ([`has_go_fmt_verb`]).
pub(crate) fn sql_statements(source: &str) -> LiteralSql {
    let joined = joined_literals(source);
    let b = source.as_bytes();
    let mut out = LiteralSql {
        literals: joined.lits.len(),
        statements: Vec::new(),
        rejected_fmt: 0,
    };
    for (group, group_joints) in joined.groups() {
        // Cheap first: no allocation unless the first word is a SQL verb or
        // a piece holds a parenthesised subquery.
        let verb = group_opens_with_verb(source, group);
        if !verb && !group.iter().any(|lit| has_subquery(&b[lit.body.clone()])) {
            continue;
        }
        let (text, pieces) = join_group(source, group, group_joints);
        let mut scan = blank_sql_noise(&blank_escapes(&text));
        let is_sql = if verb {
            opens_sql_statement(&scan)
        } else if has_subquery(scan.as_bytes()) && !ends_like_prose(&scan) {
            // A query-builder fragment: `" AND EXISTS (SELECT 1 FROM t …)"`.
            // Only the subqueries are SQL; the rest of the literal is not read.
            scan = keep_subqueries(&scan);
            true
        } else {
            false
        };
        if !is_sql {
            continue;
        }
        if has_go_fmt_verb(&scan) {
            out.rejected_fmt += 1;
            continue;
        }
        out.statements.push(SqlStmt { text, scan, pieces });
    }
    out
}

/// How a literal attaches to the one before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Joint {
    /// Not part of the same statement.
    Apart,
    /// Concatenated: the runtime value is the two bodies back to back.
    Concat,
    /// The next line of a string builder (`sb.AppendLine("…")`,
    /// `.append("…")`, `sb.WriteString("…")`, `parts.push("…")`,
    /// `q += "…"`, `$sql .= "…"`).
    Builder,
}

/// Builder methods whose calls append their argument to one statement.
const BUILDER_METHODS: &[&str] = &[
    "Append",
    "AppendLine",
    "append",
    "WriteString",
    "write",
    "push",
    "concat",
];

/// Longest gap between two literals that can still join them.
const MAX_JOINT_GAP: usize = 160;

fn joint(b: &[u8], a: &Lit, next: &Lit) -> Joint {
    if next.outer.start < a.outer.end || next.outer.start - a.outer.end > MAX_JOINT_GAP {
        return Joint::Apart;
    }
    let gap = &b[a.outer.end..next.outer.start];
    let mut tokens = 0;
    let concat = gap.iter().all(|&c| {
        if c.is_ascii_whitespace() {
            return true;
        }
        if matches!(c, b'+' | b'.' | b'\\') && tokens == 0 {
            tokens += 1;
            return true;
        }
        false
    });
    if concat {
        Joint::Concat
    } else if builder_gap(gap) {
        Joint::Builder
    } else {
        Joint::Apart
    }
}

/// True when `gap` (the code between two literals) is one builder step:
/// `)[;] [recv](.|->)Method(` with Method a [`BUILDER_METHODS`] entry, or
/// `[;] recv += ` / `[;] recv .= `. The receiver is a plain path.
fn builder_gap(gap: &[u8]) -> bool {
    let first = gap.iter().find(|c| !c.is_ascii_whitespace());
    let last = gap.iter().rev().find(|c| !c.is_ascii_whitespace());
    if !matches!(first, Some(b')' | b';')) && last != Some(&b'=') {
        return false;
    }
    let g: Vec<u8> = gap
        .iter()
        .copied()
        .filter(|c| !c.is_ascii_whitespace())
        .collect();
    let path = |r: &[u8]| {
        r.iter()
            .all(|&c| is_word_byte(c) || matches!(c, b'.' | b'$' | b'-' | b'>'))
    };
    let body = g.strip_prefix(b";").unwrap_or(&g);
    for op in [b"+=".as_slice(), b".=".as_slice()] {
        if let Some(recv) = body.strip_suffix(op) {
            return !recv.is_empty() && path(recv);
        }
    }
    let Some(rest) = g.strip_prefix(b")") else {
        return false;
    };
    let rest = rest.strip_prefix(b";").unwrap_or(rest);
    let Some(call) = rest.strip_suffix(b"(") else {
        return false;
    };
    let Some(split) = call.iter().rposition(|&c| c == b'.' || c == b'>') else {
        return false;
    };
    let (recv, method) = (&call[..split], &call[split + 1..]);
    path(recv) && BUILDER_METHODS.iter().any(|m| m.as_bytes() == method)
}

/// `text` with every `\n`, `\t` and `\r` escape (a literal body is read raw)
/// replaced by two spaces, so `"SELECT a\nFROM users"` keeps `FROM` a word.
/// Same length, so offsets carry over.
fn blank_escapes(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = b.to_vec();
    let mut i = 0;
    while i + 1 < b.len() {
        if b[i] == b'\\' {
            if matches!(b[i + 1], b'n' | b't' | b'r') {
                out[i] = b' ';
                out[i + 1] = b' ';
            }
            i += 2;
        } else {
            i += 1;
        }
    }
    // Only ASCII bytes were replaced, by ASCII.
    String::from_utf8(out).unwrap_or_else(|_| text.to_string())
}

/// Statement verbs a joined literal may open with (the full shape is checked
/// by [`opens_sql_statement`] once the text is joined).
const SQL_VERBS: &[&str] = &[
    "SELECT", "INSERT", "UPDATE", "DELETE", "WITH", "CREATE", "ALTER", "DROP", "TRUNCATE", "MERGE",
    "REPLACE",
];

/// Offset of the first byte of `b` at or after `i` that is not whitespace, an
/// opening paren, a `\n` / `\t` / `\r` escape, or a SQL comment.
fn skip_sql_lead(b: &[u8], mut i: usize) -> usize {
    loop {
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b'(') {
            i += 1;
        }
        if b[i..].starts_with(b"\\") && matches!(b.get(i + 1), Some(b'n' | b't' | b'r')) {
            // A raw body's `\n` escape.
            i += 2;
        } else if b[i..].starts_with(b"--") {
            i = line_end(b, i);
        } else if b[i..].starts_with(b"/*") {
            i = find_bytes(b, i + 2, b"*/").map_or(b.len(), |j| j + 2);
        } else {
            return i;
        }
    }
}

fn group_opens_with_verb(source: &str, group: &[Lit]) -> bool {
    let b = source.as_bytes();
    for lit in group {
        let body = &b[lit.body.clone()];
        let s = skip_sql_lead(body, 0);
        if s == body.len() {
            continue; // an all-blank piece: the verb is in the next one
        }
        let w = next_word(body, s);
        return w.0 == s && SQL_VERBS.iter().any(|v| word_is(body, w, v));
    }
    false
}

/// True when a joined literal opens (after whitespace, `(`, escapes and SQL
/// comments) with a statement verb in a statement shape: `SELECT … FROM`, `INSERT [IGNORE |
/// OR <x>] INTO`, `UPDATE … SET`, `DELETE FROM`, `WITH [RECURSIVE] x AS (`,
/// `CREATE [OR REPLACE] [TEMP …] TABLE | [UNIQUE] INDEX | [MATERIALIZED] VIEW`,
/// `ALTER TABLE`, `DROP TABLE`, `TRUNCATE TABLE` (or an upper-case bare
/// `TRUNCATE`), `MERGE INTO`, `REPLACE INTO`. Prose that happens to start with
/// a verb (`Select from the list.`) ends like prose ([`ends_like_prose`]).
/// `scan` is the joined text with escapes and SQL noise blanked
/// ([`blank_escapes`], [`blank_sql_noise`]). A literal that opens with no
/// verb is SQL only when it holds a subquery ([`has_subquery`]).
fn opens_sql_statement(scan: &str) -> bool {
    let b = scan.as_bytes();
    let s = skip_sql_lead(b, 0);
    let first = next_word(b, s);
    if first.0 != s || first.0 == first.1 {
        return false;
    }
    let second = next_word(b, first.1);
    let shaped = if word_is(b, first, "SELECT") {
        has_word_ci(b, first.1, "FROM")
    } else if word_is(b, first, "INSERT") {
        let third = next_word(b, second.1);
        word_is(b, second, "INTO")
            || (word_is(b, second, "IGNORE") && word_is(b, third, "INTO"))
            || (word_is(b, second, "OR") && word_is(b, next_word(b, third.1), "INTO"))
    } else if word_is(b, first, "UPDATE") {
        has_word_ci(b, first.1, "SET")
    } else if word_is(b, first, "DELETE") {
        word_is(b, second, "FROM")
    } else if word_is(b, first, "WITH") {
        cte_at(b, first.1).is_some()
    } else if word_is(b, first, "CREATE") {
        create_ddl_shape(b, first.1)
    } else if word_is(b, first, "ALTER") || word_is(b, first, "DROP") {
        word_is(b, second, "TABLE")
    } else if word_is(b, first, "TRUNCATE") {
        word_is(b, second, "TABLE") || (&b[first.0..first.1] == b"TRUNCATE" && second.0 < second.1)
    } else if word_is(b, first, "MERGE") || word_is(b, first, "REPLACE") {
        word_is(b, second, "INTO")
    } else {
        false
    };
    shaped && !ends_like_prose(scan)
}

/// Prose that happens to hold SQL words ends in `.` or `!`; SQL never does.
fn ends_like_prose(scan: &str) -> bool {
    let tail = scan.trim_end();
    tail.ends_with('.') || tail.ends_with('!')
}

/// `scan` with everything outside its parenthesised subqueries (`(SELECT …)`,
/// to the matching `)` or the end) blanked to spaces, newlines kept. Same
/// length, so offsets carry over.
fn keep_subqueries(scan: &str) -> String {
    let b = scan.as_bytes();
    let mut keep = vec![false; b.len()];
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'(' && word_is(b, next_word(b, i + 1), "SELECT") {
            let mut depth = 0usize;
            let mut j = i;
            while j < b.len() {
                match b[j] {
                    b'(' => depth += 1,
                    b')' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            let end = (j + 1).min(b.len());
            keep[i..end].iter_mut().for_each(|k| *k = true);
            i = end;
        } else {
            i += 1;
        }
    }
    let out: Vec<u8> = b
        .iter()
        .zip(&keep)
        .map(|(&c, &k)| if k || c == b'\n' { c } else { b' ' })
        .collect();
    // Kept spans start at `(` and end after `)`: ASCII, so char boundaries.
    String::from_utf8(out).unwrap_or_else(|_| scan.to_string())
}

/// True when `b` holds a parenthesised subquery: `(` then `SELECT`, with a
/// `FROM` after it (a query-builder fragment such as `" AND EXISTS (SELECT 1
/// FROM entity_records er …)"`, which opens with no verb of its own).
fn has_subquery(b: &[u8]) -> bool {
    b.iter().enumerate().any(|(i, &c)| {
        if c != b'(' {
            return false;
        }
        let w = next_word(b, i + 1);
        word_is(b, w, "SELECT") && has_word_ci(b, w.1, "FROM")
    })
}

/// `CREATE` is followed by a table, index or view: `[OR REPLACE] [GLOBAL |
/// LOCAL] [TEMP | TEMPORARY | UNLOGGED | VIRTUAL] TABLE`, `[UNIQUE |
/// CLUSTERED | NONCLUSTERED | FULLTEXT | SPATIAL] INDEX`, `[MATERIALIZED]
/// VIEW`.
fn create_ddl_shape(b: &[u8], i: usize) -> bool {
    const MODIFIERS: &[&str] = &[
        "OR",
        "REPLACE",
        "GLOBAL",
        "LOCAL",
        "TEMP",
        "TEMPORARY",
        "UNLOGGED",
        "VIRTUAL",
        "UNIQUE",
        "CLUSTERED",
        "NONCLUSTERED",
        "FULLTEXT",
        "SPATIAL",
        "MATERIALIZED",
    ];
    let mut w = next_word(b, i);
    for _ in 0..4 {
        if !MODIFIERS.iter().any(|m| word_is(b, w, m)) {
            break;
        }
        w = next_word(b, w.1);
    }
    ["TABLE", "INDEX", "VIEW"].iter().any(|k| word_is(b, w, k))
}

/// True when `word` occurs as a whole word (ASCII case-insensitive) at or
/// after `from`.
fn has_word_ci(b: &[u8], from: usize, word: &str) -> bool {
    let w = word.as_bytes();
    let mut i = from;
    while i + w.len() <= b.len() {
        if b[i..i + w.len()].eq_ignore_ascii_case(w)
            && (i == 0 || !is_word_byte(b[i - 1]))
            && b.get(i + w.len()).is_none_or(|c| !is_word_byte(*c))
        {
            return true;
        }
        i += 1;
    }
    false
}

/// A Go fmt verb no SQL driver uses as a placeholder: `%v`, `%q`, `%w` (with
/// an optional `+` / `#` flag). SQL placeholders are `?`, `$n`, `:name`,
/// `@p`, `%s` and `%(name)s`. `scan` is the blanked text, so a `LIKE '%water%'`
/// value never counts.
fn has_go_fmt_verb(scan: &str) -> bool {
    let b = scan.as_bytes();
    b.iter().enumerate().any(|(i, &c)| {
        if c != b'%' {
            return false;
        }
        let mut j = i + 1;
        if matches!(b.get(j), Some(b'+' | b'#')) {
            j += 1;
        }
        matches!(b.get(j), Some(b'v' | b'q' | b'w'))
    })
}

/// The span of the CTE name a `WITH` (or a `,` in a WITH list) introduces at
/// `i`: `[RECURSIVE] <ident> [(cols)] AS [NOT] [MATERIALIZED] (`.
fn cte_at(b: &[u8], i: usize) -> Option<(usize, usize)> {
    let mut name = next_word(b, i);
    if word_is(b, name, "RECURSIVE") {
        name = next_word(b, name.1);
    }
    if name.0 == name.1 || (!b[name.0].is_ascii_alphabetic() && b[name.0] != b'_') {
        return None;
    }
    let mut j = name.1;
    while j < b.len() && b[j].is_ascii_whitespace() {
        j += 1;
    }
    if b.get(j) == Some(&b'(') {
        // A column list is flat: the first `)` closes it.
        let close = b[j..].iter().take(512).position(|&c| c == b')')?;
        j += close + 1;
    }
    let as_kw = next_word(b, j);
    if !word_is(b, as_kw, "AS") {
        return None;
    }
    let mut k = as_kw.1;
    let not = next_word(b, k);
    if word_is(b, not, "NOT") {
        k = not.1;
    }
    let mat = next_word(b, k);
    if word_is(b, mat, "MATERIALIZED") {
        k = mat.1;
    }
    while k < b.len() && b[k].is_ascii_whitespace() {
        k += 1;
    }
    (b.get(k) == Some(&b'(')).then_some(name)
}

/// Lower-cased CTE names `sql` defines: `WITH x AS (` and every `, y AS (`.
fn cte_names(sql: &str) -> Vec<String> {
    let b = sql.as_bytes();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let at = if b[i] == b',' {
            i += 1;
            cte_at(b, i)
        } else if is_word_byte(b[i]) {
            let w = next_word(b, i);
            i = w.1.max(i + 1);
            if word_is(b, w, "WITH") {
                cte_at(b, w.1)
            } else {
                None
            }
        } else {
            i += 1;
            None
        };
        if let Some((s, e)) = at {
            let name = sql[s..e].to_ascii_lowercase();
            if !out.contains(&name) {
                out.push(name);
            }
        }
    }
    out
}

/// One statement's tables, minus its CTE names.
#[derive(Default)]
pub(crate) struct SqlScan {
    /// `(table, offset of the name in the scanned text)`.
    pub(crate) tables: Vec<(String, usize)>,
    pub(crate) ctes: usize,
    pub(crate) rejected_fn: usize,
}

/// The tables one statement's text names ([`scan_sql_tables`]), minus the
/// names the statement's own `WITH` defines. `sql` is blanked text.
pub(crate) fn statement_tables(sql: &str) -> SqlScan {
    let ctes = cte_names(sql);
    let mut rejected_fn = 0;
    let tables = scan_sql_tables(sql, &mut rejected_fn)
        .into_iter()
        .filter(|(name, _)| !ctes.contains(&name.to_ascii_lowercase()))
        .collect();
    SqlScan {
        tables,
        ctes: ctes.len(),
        rejected_fn,
    }
}

/// Whole-file SQL mode (a migration `.sql`, already blanked by
/// [`blank_sql_noise`]): every statement is SQL, so the table scan runs over
/// the file, one `;`-separated statement at a time so a CTE name is dropped
/// only from the statement that defines it. Offsets are file offsets.
pub(crate) fn sql_file_tables(sql: &str) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    let mut start = 0;
    for (k, c) in sql
        .bytes()
        .enumerate()
        .chain(std::iter::once((sql.len(), b';')))
    {
        if c != b';' {
            continue;
        }
        // `;` is ASCII, so both cuts are char boundaries.
        let scan = statement_tables(&sql[start..k]);
        out.extend(scan.tables.into_iter().map(|(t, p)| (t, start + p)));
        start = (k + 1).min(sql.len());
    }
    out
}

/// `source` with every `-- line` / `/* block */` comment and every
/// single-quoted string value (`'text'`, `E'it\'s'`) replaced by spaces,
/// newlines kept. In SQL a single quote delimits a value, never a name, so no
/// table is lost; double-quoted, backticked and bracketed identifiers are
/// left alone. Only ASCII bytes delimit what is blanked and every blanked byte
/// becomes an ASCII space, so the result is valid UTF-8 whenever `source` is,
/// and it has `source`'s length, so offsets carry over. (A13.9, shared by the
/// literal scan since LG.3b.)
pub(crate) fn blank_sql_noise(source: &str) -> String {
    let b = source.as_bytes();
    let mut out = b.to_vec();
    let blank = |from: usize, to: usize, out: &mut Vec<u8>| {
        for (k, byte) in out.iter_mut().enumerate().take(to).skip(from) {
            if b[k] != b'\n' {
                *byte = b' ';
            }
        }
    };
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\'' => {
                // `E'…'` takes backslash escapes; a standard string doubles its
                // quote (`'it''s'`), which reads as two adjacent strings here.
                let escapes = i > 0
                    && matches!(b[i - 1], b'E' | b'e')
                    && (i < 2 || !(b[i - 2].is_ascii_alphanumeric() || b[i - 2] == b'_'));
                let mut j = i + 1;
                while j < b.len() && b[j] != b'\'' {
                    j += if escapes && b[j] == b'\\' { 2 } else { 1 };
                }
                let end = (j + 1).min(b.len());
                blank(i, end, &mut out);
                i = end;
            }
            q @ (b'"' | b'`') => {
                i += 1;
                while i < b.len() && b[i] != q {
                    i += 1;
                }
                i += 1;
            }
            b'-' if b.get(i + 1) == Some(&b'-') => {
                let mut j = i;
                while j < b.len() && b[j] != b'\n' {
                    j += 1;
                }
                blank(i, j, &mut out);
                i = j;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let mut j = i + 2;
                while j < b.len() && !(b[j] == b'*' && b.get(j + 1) == Some(&b'/')) {
                    j += 1;
                }
                let end = (j + 2).min(b.len());
                blank(i, end, &mut out);
                i = end;
            }
            _ => i += 1,
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| source.to_string())
}

// ----------------------------------------------------------------------------
// Raw SQL: pull table names from `FROM <name>`, `JOIN <name>`, `INTO <name>`,
// `UPDATE <name>` clauses. Case-insensitive on the keyword, identifier-shaped
// on the name. Callers hand it SQL text only: one literal statement, or a
// migration file.
// ----------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Clause {
    From,
    Join,
    Into,
    Update,
}

/// `(table, offset of the name in sql)` for every FROM / JOIN / INTO / UPDATE
/// clause, in text order. One forward pass. What is never a table: a function (`FROM unnest(`,
/// `JOIN LATERAL jsonb_array_elements(`), a FROM that is a function argument
/// (`EXTRACT(YEAR FROM ts)`, `SUBSTRING(x FROM 2)`: inside a paren that holds
/// no SELECT / DELETE / UPDATE; `IS DISTINCT FROM x`), the `ONLY` / `LATERAL`
/// modifiers (the name after them is read instead), and the SQL keywords
/// [`canonical_sql_name`] rejects. Each function-shaped reject adds one to
/// `rejected_fn`.
pub(crate) fn scan_sql_tables(sql: &str, rejected_fn: &mut usize) -> Vec<(String, usize)> {
    let b = sql.as_bytes();
    let mut out = Vec::new();
    // One frame per open paren: true once it holds a query verb, so a FROM in
    // it reads a subquery's table, not a function argument.
    let mut frames: Vec<bool> = Vec::new();
    let mut prev_word = (0, 0);
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'(' => {
                frames.push(false);
                i += 1;
                continue;
            }
            b')' => {
                frames.pop();
                i += 1;
                continue;
            }
            c if !is_word_byte(c) => {
                i += 1;
                continue;
            }
            _ => {}
        }
        let s = i;
        while i < b.len() && is_word_byte(b[i]) {
            i += 1;
        }
        let w = (s, i);
        if ["SELECT", "DELETE", "UPDATE"]
            .iter()
            .any(|v| word_is(b, w, v))
            && let Some(top) = frames.last_mut()
        {
            *top = true;
        }
        let clause = if word_is(b, w, "FROM") {
            Some(Clause::From)
        } else if word_is(b, w, "JOIN") {
            Some(Clause::Join)
        } else if word_is(b, w, "INTO") {
            Some(Clause::Into)
        } else if word_is(b, w, "UPDATE") {
            Some(Clause::Update)
        } else {
            None
        };
        let spaced = matches!(b.get(i), Some(b' ' | b'\t' | b'\n' | b'\r'));
        if let (Some(clause), true) = (clause, spaced) {
            let fn_arg = clause == Clause::From
                && (frames.last() == Some(&false) || word_is(b, prev_word, "DISTINCT"));
            if fn_arg {
                *rejected_fn += 1;
            } else if let Some(hit) = clause_table(sql, i, clause, rejected_fn) {
                out.push(hit);
            }
        }
        prev_word = w;
    }
    out
}

/// The table a clause keyword ending at `after_kw` names.
fn clause_table(
    sql: &str,
    after_kw: usize,
    clause: Clause,
    rejected_fn: &mut usize,
) -> Option<(String, usize)> {
    let b = sql.as_bytes();
    let (mut raw, mut end) = read_sql_ident(sql, after_kw)?;
    if raw.eq_ignore_ascii_case("ONLY") || raw.eq_ignore_ascii_case("LATERAL") {
        (raw, end) = read_sql_ident(sql, end)?;
    }
    if matches!(clause, Clause::From | Clause::Join) {
        let mut j = end;
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        if b.get(j) == Some(&b'(') {
            *rejected_fn += 1;
            return None;
        }
    }
    let name = canonical_sql_name(raw)?;
    // `raw` is a subslice of `sql`.
    let pos = raw.as_ptr() as usize - sql.as_ptr() as usize;
    Some((name, pos))
}

// ----------------------------------------------------------------------------
// SQL DDL (A13.9): the table a `CREATE / ALTER / DROP / TRUNCATE` statement
// names. Anchored on the whole phrase: `find_keyword_ci` only needs whitespace
// on both sides and `has_sql_context` passes any file holding `select `, so a
// bare `TABLE` keyword would read the prose `the users table maps …` as a
// table `maps`.
// ----------------------------------------------------------------------------

/// `(verb, table)` for every DDL statement in `source`, verb in the marker's
/// spelling (`create` / `alter` / `drop` / `truncate` / `index`). Shapes:
/// `CREATE [OR REPLACE] [GLOBAL|LOCAL] [TEMP|TEMPORARY|UNLOGGED|VIRTUAL] TABLE`,
/// `ALTER TABLE`, `DROP TABLE`, `TRUNCATE [TABLE]`, each then skipping
/// `IF [NOT] EXISTS` and Postgres `ONLY`. `DROP` and `TRUNCATE` take a comma
/// list. A bare `TRUNCATE x` (no `TABLE`) counts only when `x` ends the
/// statement, so the prose `truncate long names` names nothing. And
/// `CREATE [UNIQUE] INDEX … ON <table> (` (see [`index_table`]).
pub(crate) fn scan_sql_ddl(source: &str) -> Vec<(&'static str, String)> {
    let b = source.as_bytes();
    let mut out = Vec::new();
    for (keyword, verb) in [
        ("CREATE", "create"),
        ("ALTER", "alter"),
        ("DROP", "drop"),
        ("TRUNCATE", "truncate"),
    ] {
        let kw_lower = keyword.to_ascii_lowercase();
        let mut search_from = 0;
        while search_from < source.len() {
            let Some(rel) = find_keyword_ci(&source[search_from..], keyword, &kw_lower) else {
                break;
            };
            let after_kw = search_from + rel + keyword.len();
            search_from = after_kw;
            let mut w = next_word(b, after_kw);
            if verb == "create" {
                if word_is(b, w, "OR") {
                    let replace = next_word(b, w.1);
                    if !word_is(b, replace, "REPLACE") {
                        continue;
                    }
                    w = next_word(b, replace.1);
                }
                // `CREATE [UNIQUE] INDEX … ON <table>` indexes a table it
                // does not name first; its own reader, then the next match.
                let mut index = w;
                while ["UNIQUE", "CLUSTERED", "NONCLUSTERED", "FULLTEXT", "SPATIAL"]
                    .iter()
                    .any(|m| word_is(b, index, m))
                {
                    index = next_word(b, index.1);
                }
                if word_is(b, index, "INDEX") {
                    if let Some(table) = index_table(source, index.1) {
                        out.push(("index", table));
                    }
                    continue;
                }
                if word_is(b, w, "GLOBAL") || word_is(b, w, "LOCAL") {
                    w = next_word(b, w.1);
                }
                if ["TEMP", "TEMPORARY", "UNLOGGED", "VIRTUAL"]
                    .iter()
                    .any(|m| word_is(b, w, m))
                {
                    w = next_word(b, w.1);
                }
            }
            let has_table = word_is(b, w, "TABLE");
            if !has_table && verb != "truncate" {
                continue;
            }
            let bare_truncate = !has_table;
            // Tailwind's `class="truncate block"` is a lowercase bare TRUNCATE
            // ending at a quote; SQL in a host string is written `TRUNCATE`.
            let upper_kw = &b[after_kw - keyword.len()..after_kw] == keyword.as_bytes();
            let mut i = if has_table { w.1 } else { after_kw };
            let guard = next_word(b, i);
            if word_is(b, guard, "IF") {
                let mut exists = next_word(b, guard.1);
                if word_is(b, exists, "NOT") {
                    exists = next_word(b, exists.1);
                }
                if !word_is(b, exists, "EXISTS") {
                    continue;
                }
                i = exists.1;
            }
            let only = next_word(b, i);
            if word_is(b, only, "ONLY") {
                i = only.1;
            }
            let list = matches!(verb, "drop" | "truncate");
            while let Some((raw, end)) = read_sql_ident(source, i) {
                if bare_truncate && !ends_statement(b, end, upper_kw) {
                    break;
                }
                if verb == "create" && !opens_table_body(b, end) {
                    break;
                }
                if let Some(name) = canonical_sql_name(raw) {
                    out.push((verb, name));
                }
                let mut j = end;
                while j < b.len() && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                if !(list && b.get(j) == Some(&b',')) {
                    break;
                }
                i = j + 1;
            }
        }
    }
    out
}

/// The table a `CREATE … INDEX` names, reading from just past `INDEX`:
/// `[CONCURRENTLY] [IF NOT EXISTS] [name] ON [ONLY] <table>`, then the column
/// list `(` or a `USING <method>` must follow, so the prose `create index
/// files on startup` names nothing.
fn index_table(source: &str, i: usize) -> Option<String> {
    let b = source.as_bytes();
    let mut p = i;
    let concurrently = next_word(b, p);
    if word_is(b, concurrently, "CONCURRENTLY") {
        p = concurrently.1;
    }
    let guard = next_word(b, p);
    if word_is(b, guard, "IF") {
        let not = next_word(b, guard.1);
        let exists = next_word(b, not.1);
        if !(word_is(b, not, "NOT") && word_is(b, exists, "EXISTS")) {
            return None;
        }
        p = exists.1;
    }
    let mut on = next_word(b, p);
    if !word_is(b, on, "ON") {
        let (_, name_end) = read_sql_ident(source, p)?;
        on = next_word(b, name_end);
        if !word_is(b, on, "ON") {
            return None;
        }
    }
    p = on.1;
    let only = next_word(b, p);
    if word_is(b, only, "ONLY") {
        p = only.1;
    }
    let (raw, end) = read_sql_ident(source, p)?;
    let mut j = end;
    while j < b.len() && b[j].is_ascii_whitespace() {
        j += 1;
    }
    let body = b.get(j) == Some(&b'(') || word_is(b, next_word(b, j), "USING");
    if !body {
        return None;
    }
    canonical_sql_name(raw)
}

/// The identifier-shaped word at the first non-whitespace byte at or after
/// `i`, as `(start, end)`; empty (`start == end`) when that byte starts none.
fn next_word(b: &[u8], i: usize) -> (usize, usize) {
    let mut s = i;
    while s < b.len() && b[s].is_ascii_whitespace() {
        s += 1;
    }
    let mut e = s;
    while e < b.len() && (b[e].is_ascii_alphanumeric() || b[e] == b'_') {
        e += 1;
    }
    (s, e)
}

fn word_is(b: &[u8], (s, e): (usize, usize), word: &str) -> bool {
    b[s..e].eq_ignore_ascii_case(word.as_bytes())
}

/// The last part of the (possibly schema-qualified) SQL identifier at the first
/// non-whitespace byte at or after `i`, and the offset just past the whole
/// name: `users`, `public.users`, `"users"`, `` `db`.`users` ``,
/// `[dbo].[Users]`, and `\"users\"` escaped inside a host-language string.
/// `None` when no identifier starts there or a quote does not close within
/// 128 bytes. The raw part still goes through [`canonical_sql_name`].
fn read_sql_ident(source: &str, i: usize) -> Option<(&str, usize)> {
    let b = source.as_bytes();
    let mut k = i;
    while k < b.len() && b[k].is_ascii_whitespace() {
        k += 1;
    }
    let mut last = None;
    loop {
        if b.get(k) == Some(&b'\\') && matches!(b.get(k + 1), Some(b'"' | b'`')) {
            k += 1;
        }
        let Some(&c) = b.get(k) else { break };
        let (start, end, next) = if matches!(c, b'"' | b'`' | b'[') {
            let close = if c == b'[' { b']' } else { c };
            let s = k + 1;
            let mut e = s;
            while e < b.len() && b[e] != close && e - s <= 128 {
                e += 1;
            }
            if b.get(e) != Some(&close) {
                return None;
            }
            // `\"users\"`: the escape before the closing quote is not a name byte.
            let name_end = if e > s && b[e - 1] == b'\\' { e - 1 } else { e };
            (s, name_end, e + 1)
        } else {
            let mut e = k;
            while e < b.len() && (b[e].is_ascii_alphanumeric() || b[e] == b'_' || b[e] == b'$') {
                e += 1;
            }
            if e == k {
                break;
            }
            (k, e, e)
        };
        last = Some((&source[start..end], next));
        k = next;
        if b.get(k) != Some(&b'.') {
            break;
        }
        k += 1;
    }
    last
}

/// True when what follows a `CREATE TABLE` name is a table body: the column
/// list `(`, a `CREATE TABLE … AS / LIKE / PARTITION OF / CLONE / USING /
/// WITH / SELECT` form, or the end of the host string or source. The prose
/// `create table rows lazily` names nothing.
fn opens_table_body(b: &[u8], end: usize) -> bool {
    let mut j = end;
    while j < b.len() && b[j].is_ascii_whitespace() {
        j += 1;
    }
    match b.get(j) {
        None | Some(b'(' | b';' | b'"' | b'\'' | b'`' | b')' | b'\\' | b',') => true,
        Some(_) => {
            let w = next_word(b, j);
            const FORMS: &[&str] = &[
                "AS",
                "LIKE",
                "PARTITION",
                "OF",
                "CLONE",
                "COPY",
                "USING",
                "WITH",
                "SELECT",
            ];
            FORMS.iter().any(|form| word_is(b, w, form))
        }
    }
}

/// True when the identifier ending at `end` also ends its statement. Gates the
/// bare `TRUNCATE x` form only. A `;` always ends it. The softer ends (a list
/// comma, a closing quote / paren of the host string, the end of the source,
/// a `TRUNCATE` option word) count only after an uppercase `TRUNCATE`, so
/// `class="truncate block"` and `truncate long names` name nothing.
fn ends_statement(b: &[u8], end: usize, upper_kw: bool) -> bool {
    let mut j = end;
    while j < b.len() && (b[j] == b' ' || b[j] == b'\t') {
        j += 1;
    }
    if b.get(j) == Some(&b';') {
        return true;
    }
    if !upper_kw {
        return false;
    }
    match b.get(j) {
        None | Some(b',' | b'"' | b'\'' | b'`' | b')' | b'\\') => true,
        Some(_) => {
            let w = next_word(b, j);
            ["CASCADE", "RESTRICT", "RESTART", "CONTINUE"]
                .iter()
                .any(|opt| word_is(b, w, opt))
        }
    }
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

/// True for captured "names" that are never real data entities, whatever the
/// flavor: pure numerics (`FROM 2`) and English/JS keywords that follow
/// `from`/`into` in prose or get passed to `.collection(...)`. Applied at the
/// single `emit` funnel so SQL, NoSQL and graph flavors are all protected.
/// (glia-v2 G7) Also true for any name that is not [`entity_shaped`] (LA.42):
/// one gate for every scanner and flavor.
fn is_noise_entity_name(name: &str) -> bool {
    // Shape first, on the name as emitted: ` users` is not a name either.
    if !entity_shaped(name) {
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
            // LG.3b: English determiners a prose `… from the list` or
            // `… from all emails` puts where a table name goes.
            | "the"
            | "a"
            | "an"
            | "all"
            | "each"
            | "every"
            | "any"
            | "some"
            | "both"
            | "another"
            | "its"
            | "my"
            | "your"
            | "our"
            | "their"
            | "his"
            | "her"
    )
}

/// True for a name a table, collection or label can have (LA.42): 1..=128
/// bytes, starting with an alphanumeric char or `_`, every char alphanumeric
/// or one of `_ - . /` (Mongo `system.users`, DynamoDB `orders-v2`, Firestore
/// `users/abc/orders`). Non-ASCII letters pass (`usuários`). Whitespace,
/// quotes, brackets and code (`, `, `<Name>`, `...`, `${t}`) do not.
fn entity_shaped(name: &str) -> bool {
    let mut chars = name.chars();
    (1..=128).contains(&name.len())
        && chars
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
        && chars.all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/'))
}

/// Strip schema prefix and noise; reject SQL keywords / placeholders that
/// would otherwise leak through (`SELECT`, `?`, `:param`).
pub(crate) fn canonical_sql_name(raw: &str) -> Option<String> {
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
    // Reject SQL keywords that can appear right after FROM / JOIN / INTO /
    // UPDATE: `DO UPDATE SET`, `ON UPDATE CASCADE`, `FOR UPDATE SKIP LOCKED`,
    // `UPDATE OF col`, `BEFORE UPDATE ON t`, `JOIN LATERAL`, `FROM DUAL`, and
    // `TABLE`, which the A13.9 DDL scan reads past and must never capture.
    if SQL_NAME_KEYWORDS
        .iter()
        .any(|k| k.eq_ignore_ascii_case(last))
    {
        return None;
    }
    Some(last.to_string())
}

/// Words [`canonical_sql_name`] never takes for a table name.
const SQL_NAME_KEYWORDS: &[&str] = &[
    "SELECT", "WHERE", "AND", "OR", "IF", "EXISTS", "NULL", "TRUE", "FALSE", "FROM", "JOIN",
    "INTO", "LATERAL", "SET", "ONLY", "VALUES", "UNNEST", "DUAL", "TABLE", "ALL", "ANY", "AS",
    "CASCADE", "CASE", "CROSS", "DEFAULT", "DISTINCT", "EACH", "FOR", "FULL", "INNER", "LEFT",
    "NATURAL", "NO", "NOT", "NOWAIT", "OF", "ON", "OUTER", "RESTRICT", "RIGHT", "ROW", "SKIP",
    "STRICT", "USING", "WITH",
];

// ----------------------------------------------------------------------------
// ORM table declarations: `__tablename__ = 'users'` (SQLAlchemy) and
// `db_table = 'users'` (Django Meta inner class).
// ----------------------------------------------------------------------------

fn scan_orm_table_decls(source: &str, stats: &mut DeclStats) -> Vec<String> {
    let mut out = Vec::new();
    for needle in ["__tablename__", "db_table"] {
        let mut search_from = 0;
        while let Some(rel) = source[search_from..].find(needle) {
            let pos = search_from + rel;
            search_from = pos + needle.len();
            stats.needles += 1;
            // `__tablename__ = 'users'`, `__tablename__: str = "users"`; a
            // `def __tablename__(cls):` or a later `=` names nothing.
            let lit = assigned_value(&source[pos + needle.len()..])
                .map_or(LeadingLit::NotLiteral, inline_literal);
            if let Some(name) = stats.take(lit) {
                match canonical_sql_name(name) {
                    Some(cleaned) => out.push(cleaned),
                    None => stats.rejected_shape += 1,
                }
            }
        }
    }
    out
}

/// The value of an assignment whose target just ended at the start of `s`:
/// spaces / tabs, an optional `: <annotation>`, then `=` (not `==`), all on
/// the target's line. `None` for any other shape (`(cls):`, `_name = …`, a
/// newline before the `=`).
fn assigned_value(s: &str) -> Option<&str> {
    let b = s.as_bytes();
    let mut i = blank_prefix_len(s);
    if b.get(i) == Some(&b':') {
        i += 1 + b[i + 1..].iter().position(|&c| c == b'=' || c == b'\n')?;
    }
    if b.get(i) != Some(&b'=') || b.get(i + 1) == Some(&b'=') {
        return None;
    }
    Some(&s[i + 1..])
}

/// What a declaration's name position holds (LA.42). A declaration scanner
/// names an entity only from `Lit`: never from a literal further on, which
/// belongs to some other expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LeadingLit<'a> {
    /// A `'…'`, `"…"` or backtick literal that starts the text and closes on
    /// its line; the body between the quotes.
    Lit(&'a str),
    /// A quote starts the text but the literal does not close on its line,
    /// holds a backslash, or is empty (a Python `"""` reads as `""`).
    Malformed,
    /// The text starts with something else: an identifier, a call, an
    /// f-string or template prefix, code.
    NotLiteral,
}

/// The literal that starts a call argument: whitespace, newlines included,
/// may precede it (`mongoose.model(\n  'User',`).
fn leading_literal(s: &str) -> LeadingLit<'_> {
    literal_after(s, |c| matches!(c, b' ' | b'\t' | b'\r' | b'\n')).0
}

/// The literal that starts an assignment's value or an object key's value:
/// only spaces and tabs may precede it, as the value is on its line.
fn inline_literal(s: &str) -> LeadingLit<'_> {
    literal_after(s, |c| matches!(c, b' ' | b'\t')).0
}

/// The literal at the first byte of `s` that `skip` does not match, and the
/// offset just past its closing quote (0 unless `Lit`). The body is sliced at
/// ASCII quote bytes, so it is always on a char boundary.
fn literal_after(s: &str, skip: fn(u8) -> bool) -> (LeadingLit<'_>, usize) {
    let b = s.as_bytes();
    let open = b.iter().position(|&c| !skip(c)).unwrap_or(b.len());
    let Some(&q @ (b'\'' | b'"' | b'`')) = b.get(open) else {
        return (LeadingLit::NotLiteral, 0);
    };
    let start = open + 1;
    for (k, &c) in b[start..].iter().enumerate() {
        if c == q {
            if k == 0 {
                break;
            }
            return (LeadingLit::Lit(&s[start..start + k]), start + k + 1);
        }
        if matches!(c, b'\n' | b'\r' | b'\\') {
            break;
        }
    }
    (LeadingLit::Malformed, 0)
}

/// What the declaration scanners (ORM table, Mongoose, DynamoDB, Beanie) read
/// in one file, for the `[data-entity] decl` marker (LA.42). Every needle hit
/// lands in exactly one of `kept`, `rejected_nonliteral`, `rejected_shape`.
#[derive(Debug, Default, PartialEq, Eq)]
struct DeclStats {
    needles: usize,
    kept: usize,
    /// Needle hits whose name position held no literal (`NotLiteral`,
    /// `Malformed`, an ORM needle that is not assigned on its line, a Beanie
    /// `Settings` block with no `name` line).
    rejected_nonliteral: usize,
    /// Literals the emit funnel (or `canonical_sql_name`) rejected.
    rejected_shape: usize,
}

impl DeclStats {
    /// The literal's body, or `None` after counting the non-literal.
    fn take<'a>(&mut self, lit: LeadingLit<'a>) -> Option<&'a str> {
        match lit {
            LeadingLit::Lit(name) => Some(name),
            LeadingLit::Malformed | LeadingLit::NotLiteral => {
                self.rejected_nonliteral += 1;
                None
            }
        }
    }

    /// Count a scanner's capture at the funnel: kept or shape-rejected.
    fn funnel(&mut self, name: &str) {
        if is_noise_entity_name(name) {
            self.rejected_shape += 1;
        } else {
            self.kept += 1;
        }
    }

    fn marker(&self) -> String {
        format!(
            "[data-entity] decl needles={} kept={} rejected_nonliteral={} rejected_shape={}",
            self.needles, self.kept, self.rejected_nonliteral, self.rejected_shape
        )
    }
}

// ----------------------------------------------------------------------------
// Mongoose: `mongoose.model('User', schema)` and `model('User', schema)`.
// ----------------------------------------------------------------------------

fn scan_mongoose_models(source: &str, stats: &mut DeclStats) -> Vec<String> {
    let mut out = Vec::new();
    for needle in ["mongoose.model(", "models.model("] {
        let mut search_from = 0;
        while let Some(rel) = source[search_from..].find(needle) {
            let pos = search_from + rel;
            search_from = pos + needle.len();
            stats.needles += 1;
            // The model name is argument 1: `mongoose.model(modelName, s)`
            // names nothing, whatever string comes later.
            if let Some(name) = stats.take(leading_literal(&source[search_from..])) {
                out.push(name.to_string());
            }
        }
    }
    out
}

// ----------------------------------------------------------------------------
// DynamoDB: `TableName: 'users'` (JS SDK v3 command objects), `TableName='users'`
// (Python boto3 kwargs), and `dynamodb.Table('users')` (boto3 resource API).
// ----------------------------------------------------------------------------

fn scan_dynamodb_tables(source: &str, stats: &mut DeclStats) -> Vec<String> {
    let mut out = Vec::new();

    // Object-key / kwarg form: `TableName: '...'`, `TableName: "..."`,
    // `TableName='...'`. The needle ends at the colon/equals; the value is the
    // literal that follows on the same line (`TableName: process.env.T` names
    // nothing, whatever string comes later).
    for needle in ["TableName:", "TableName =", "TableName="] {
        let mut search_from = 0;
        while let Some(rel) = source[search_from..].find(needle) {
            let pos = search_from + rel;
            search_from = pos + needle.len();
            // Word-boundary check on the preceding char so we don't match
            // `MyTableName:`.
            let prev_ok = pos == 0 || {
                let p = source.as_bytes()[pos - 1];
                !(p.is_ascii_alphanumeric() || p == b'_' || p == b'$')
            };
            if !prev_ok {
                continue;
            }
            stats.needles += 1;
            if let Some(name) = stats.take(inline_literal(&source[search_from..])) {
                out.push(name.to_string());
            }
        }
    }

    // boto3 resource form: `dynamodb.Table('users')`. The `dynamodb.` prefix
    // disambiguates from generic `.Table(...)` builders in other libs.
    let needle = "dynamodb.Table(";
    let mut search_from = 0;
    while let Some(rel) = source[search_from..].find(needle) {
        search_from += rel + needle.len();
        stats.needles += 1;
        if let Some(name) = stats.take(leading_literal(&source[search_from..])) {
            out.push(name.to_string());
        }
    }
    out
}

// ----------------------------------------------------------------------------
// Driver collection calls: `.collection('<name>')` (Firestore and the Node
// MongoDB driver), `.Collection("<name>")` (Go mongo-driver, Go Firestore),
// `.getCollection("<name>")` (Java MongoDB driver), `.GetCollection<T>("<name>")`
// / `.GetCollection<T>(nameof(T))` (C# MongoDB.Driver). All land in the NoSQL
// flavor namespace, which is exactly what the resolver wants — the join
// semantic doesn't care which client wrote the data.
// ----------------------------------------------------------------------------

/// Collection-call needles; a needle ending in `<` is followed by a balanced
/// type-argument list and then `(`.
const COLLECTION_NEEDLES: &[&str] = &[
    ".collection(",
    ".Collection(",
    ".getCollection(",
    ".GetCollection(",
    ".GetCollection<",
];

fn scan_collection_calls(source: &str) -> Vec<String> {
    let bytes = source.as_bytes();
    let mut hits: Vec<(usize, String)> = Vec::new();
    for needle in COLLECTION_NEEDLES {
        let mut search_from = 0;
        while let Some(rel) = source[search_from..].find(needle) {
            let pos = search_from + rel;
            search_from = pos + needle.len();
            // Word-boundary on what precedes the `.` so `someother.collection(`
            // matches but a bare `collection(` (no receiver) doesn't; the
            // receiver name is not gated (Firestore and Mongo both legitimate).
            let Some(&prev) = pos.checked_sub(1).and_then(|p| bytes.get(p)) else {
                continue;
            };
            if !(prev.is_ascii_alphanumeric() || prev == b'_' || prev == b')' || prev == b']') {
                continue;
            }
            let mut open = pos + needle.len();
            if needle.ends_with('<') {
                let Some(close) = skip_type_args(bytes, open) else {
                    continue;
                };
                let mut k = close;
                while k < bytes.len() && bytes[k].is_ascii_whitespace() {
                    k += 1;
                }
                if bytes.get(k) != Some(&b'(') {
                    continue;
                }
                open = k + 1;
            }
            if let Some(name) = collection_arg(source, open) {
                hits.push((pos, name));
            }
        }
    }
    // Source order across needles.
    hits.sort_by_key(|(pos, _)| *pos);
    hits.into_iter().map(|(_, name)| name).collect()
}

/// Offset just past the `>` closing the type-argument list whose `<` ends
/// just before `i`; `None` when it does not close within one line of text.
fn skip_type_args(b: &[u8], i: usize) -> Option<usize> {
    let mut depth = 1usize;
    let mut j = i;
    while j < b.len() && j - i < 256 {
        match b[j] {
            b'<' => depth += 1,
            b'>' => {
                depth -= 1;
                if depth == 0 {
                    return Some(j + 1);
                }
            }
            b'\n' | b'(' | b')' | b';' => return None,
            _ => {}
        }
        j += 1;
    }
    None
}

/// The collection name a call's FIRST argument (starting at `i`, just past
/// the `(`) spells: a single-line string literal that starts the argument
/// ([`leading_literal`]'s rule, LA.42), or C# `nameof(Ident)` (the name is
/// the last segment of `Ident`). Anything else — a variable, an interpolated
/// `$"…"` / `` `${x}` ``, a literal that does not close on its line — names
/// nothing.
fn collection_arg(source: &str, i: usize) -> Option<String> {
    let b = source.as_bytes();
    let mut k = i;
    while k < b.len() && b[k].is_ascii_whitespace() {
        k += 1;
    }
    // `k` is already past the whitespace, so the reader skips nothing.
    let (name, end) = match literal_after(&source[k..], |_| false) {
        (LeadingLit::Lit(name), past) => (name, k + past),
        (LeadingLit::Malformed, _) => return None,
        (LeadingLit::NotLiteral, _) if b[k..].starts_with(b"nameof(") => {
            let start = k + "nameof(".len();
            let close = start + b[start..].iter().take(256).position(|&c| c == b')')?;
            let inner = source[start..close].trim();
            if inner.is_empty()
                || !inner
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'.')
            {
                return None;
            }
            (inner.rsplit('.').next().unwrap_or(inner), close + 1)
        }
        _ => return None,
    };
    let mut j = end;
    while j < b.len() && b[j].is_ascii_whitespace() {
        j += 1;
    }
    if !matches!(b.get(j), Some(b',' | b')')) {
        return None;
    }
    if name.is_empty() || name.len() >= 128 || name.contains("${") || name.contains("#{") {
        return None;
    }
    Some(name.to_string())
}

// ----------------------------------------------------------------------------
// Beanie (Pydantic + Motor): `class Foo(Document): class Settings: name = "..."`.
// The collection name lives on a `Settings` inner class. Scan for `class
// Settings:` and read the `name = '<value>'` line of its block, within the
// next ~256 bytes.
// ----------------------------------------------------------------------------

fn scan_beanie_documents(source: &str, stats: &mut DeclStats) -> Vec<String> {
    let mut out = Vec::new();
    let needle = "class Settings:";
    let mut search_from = 0;
    while let Some(rel) = source[search_from..].find(needle) {
        let pos = search_from + rel;
        let after = pos + needle.len();
        search_from = after;
        stats.needles += 1;
        // Snap DOWN: a multibyte char on the cut would panic the slice.
        let win_end = source.floor_char_boundary((after + 256).min(source.len()));
        let line_start = source[..pos].rfind('\n').map_or(0, |n| n + 1);
        let indent = blank_prefix_len(&source[line_start..pos]);
        if let Some(name) = stats.take(settings_name(&source[after..win_end], indent)) {
            out.push(name.to_string());
        }
    }
    out
}

/// The `name` a Beanie `Settings` block assigns. `block` starts right after
/// `class Settings:` (its first line is that line's remainder); `indent` is
/// the byte width of the `class Settings:` line's leading spaces / tabs. The
/// block's lines are walked — blank and comment-only lines skipped, stopping
/// at the first line not indented deeper — and the first line that assigns
/// the word `name` (`name = …`, `name: str = …`) decides: its value must be a
/// literal on that line. A later field's literal is never read.
fn settings_name(block: &str, indent: usize) -> LeadingLit<'_> {
    for line in block.split('\n').skip(1) {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let body = line.trim_start_matches([' ', '\t']);
        if body.is_empty() || body.starts_with('#') {
            continue;
        }
        if line.len() - body.len() <= indent {
            break;
        }
        let Some(rest) = body.strip_prefix("name") else {
            continue;
        };
        if rest.bytes().next().is_some_and(is_word_byte) {
            continue;
        }
        if let Some(value) = assigned_value(rest) {
            return inline_literal(value);
        }
    }
    LeadingLit::NotLiteral
}

/// Byte length of `s`'s leading spaces and tabs.
fn blank_prefix_len(s: &str) -> usize {
    s.len() - s.trim_start_matches([' ', '\t']).len()
}

// ----------------------------------------------------------------------------
// Cypher labels (LA.28): `MATCH (x:Label)` / `MERGE (x:Label)` read only from
// Cypher-shaped string literals, joined like SQL ([`joined_literals`]). A
// Rust `match`, SQL `MERGE INTO`, prose `create` and comments are not Cypher.
// Label-only: relationships, properties and return clauses are not read.
// ----------------------------------------------------------------------------

/// The node labels of every Cypher statement in `source`, in statement order
/// then label order. Under `GLIA_DATA_DEBUG` prints the file's
/// `[data-entity] cypher …` line ([`cypher_marker`]).
fn scan_cypher_labels(source: &str) -> Vec<String> {
    let stmts = cypher_statements(source);
    let labels: Vec<String> = stmts
        .iter()
        .flat_map(|stmt| extract_node_labels(&stmt.text))
        .collect();
    if debug_enabled()
        && let Some(line) = cypher_marker(source, &stmts, &labels)
    {
        eprintln!("{line}");
    }
    labels
}

/// One Cypher statement: a joined literal run that [`is_cypher_statement`].
struct CypherStmt {
    /// The literal bodies back to back, `\n`-style escapes blanked
    /// ([`blank_escapes`]).
    text: String,
    /// Source span from the first literal's opening delimiter to the last
    /// one's closing delimiter.
    span: Range<usize>,
}

/// Words a Cypher statement can open with ([`is_cypher_statement`] checks the
/// shape that must follow).
const CYPHER_OPENERS: &[&str] = &[
    "MATCH", "OPTIONAL", "MERGE", "CREATE", "UNWIND", "WITH", "CALL", "USE",
];

/// Clauses that carry node patterns.
const CYPHER_PATTERN_CLAUSES: &[&str] = &["MATCH", "MERGE", "CREATE"];

/// The joined literals ([`joined_literals`]) of `source` that are Cypher
/// statements, in source order.
fn cypher_statements(source: &str) -> Vec<CypherStmt> {
    let joined = joined_literals(source);
    let mut out = Vec::new();
    for (group, joints) in joined.groups() {
        // Cheap first: no allocation unless the run opens with a Cypher word.
        if !group_opens_with_cypher_word(source, group) {
            continue;
        }
        let (Some(first), Some(last)) = (group.first(), group.last()) else {
            continue;
        };
        let (text, _) = join_group(source, group, joints);
        let text = blank_escapes(&text);
        if is_cypher_statement(&text) {
            out.push(CypherStmt {
                text,
                span: first.outer.start..last.outer.end,
            });
        }
    }
    out
}

fn group_opens_with_cypher_word(source: &str, group: &[Lit]) -> bool {
    let b = source.as_bytes();
    for lit in group {
        let body = &b[lit.body.clone()];
        let s = skip_cypher_lead(body, 0);
        if s == body.len() {
            continue; // an all-blank piece: the keyword is in the next one
        }
        let w = next_word(body, s);
        return w.0 == s && CYPHER_OPENERS.iter().any(|k| word_is(body, w, k));
    }
    false
}

/// Offset of the first byte of `b` at or after `i` that is not whitespace, an
/// opening paren, a `\n` / `\t` / `\r` escape or a Cypher comment.
fn skip_cypher_lead(b: &[u8], mut i: usize) -> usize {
    loop {
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b'(') {
            i += 1;
        }
        if b[i..].starts_with(b"\\") && matches!(b.get(i + 1), Some(b'n' | b't' | b'r')) {
            i += 2;
        } else if b[i..].starts_with(b"//") {
            i = line_end(b, i);
        } else if b[i..].starts_with(b"/*") {
            i = find_bytes(b, i + 2, b"*/").map_or(b.len(), |j| j + 2);
        } else {
            return i;
        }
    }
}

/// True when `text` (a joined literal, escapes blanked) is a Cypher statement,
/// ASCII case-insensitive on its start (whitespace, `(` and comments
/// skipped): `MATCH` / `OPTIONAL MATCH` / `MERGE` / `CREATE` followed by
/// whitespace and then a node pattern `(` or a path variable `p =`, which
/// rejects `CREATE TABLE`, `CREATE INDEX`, SQL `MERGE INTO` and a bare
/// `create()` call; `UNWIND` / `WITH` / `CALL` / `USE` only when the
/// statement also holds a pattern clause followed by whitespace and `(`.
fn is_cypher_statement(text: &str) -> bool {
    let b = text.as_bytes();
    let s = skip_cypher_lead(b, 0);
    let first = next_word(b, s);
    if first.0 != s || first.0 == first.1 {
        return false;
    }
    let clause = if word_is(b, first, "OPTIONAL") {
        let clause = next_word(b, first.1);
        if !word_is(b, clause, "MATCH") {
            return false;
        }
        clause
    } else {
        first
    };
    if CYPHER_PATTERN_CLAUSES.iter().any(|k| word_is(b, clause, k)) {
        return opens_pattern(b, clause.1, true);
    }
    ["UNWIND", "WITH", "CALL", "USE"]
        .iter()
        .any(|k| word_is(b, first, k))
        && holds_pattern_clause(b, first.1)
}

/// True when the clause keyword ending at `end` is followed by whitespace and
/// then a node pattern `(`, or (with `path_var`) a path variable `p =`.
fn opens_pattern(b: &[u8], end: usize, path_var: bool) -> bool {
    if !b.get(end).is_some_and(|c| c.is_ascii_whitespace()) {
        return false;
    }
    let w = next_word(b, end);
    if b.get(w.0) == Some(&b'(') {
        return true;
    }
    if !path_var || w.0 == w.1 || b[w.0].is_ascii_digit() {
        return false;
    }
    let mut j = w.1;
    while j < b.len() && b[j].is_ascii_whitespace() {
        j += 1;
    }
    b.get(j) == Some(&b'=') && b.get(j + 1) != Some(&b'=')
}

/// True when `b` holds, at or after `from`, a word-bounded `MATCH` / `MERGE`
/// / `CREATE` followed by whitespace and `(`.
fn holds_pattern_clause(b: &[u8], from: usize) -> bool {
    (from..b.len()).any(|i| {
        (i == 0 || !is_word_byte(b[i - 1]))
            && CYPHER_PATTERN_CLAUSES.iter().any(|kw| {
                let k = kw.as_bytes();
                b.get(i..i + k.len())
                    .is_some_and(|w| w.eq_ignore_ascii_case(k))
                    && opens_pattern(b, i + k.len(), false)
            })
    })
}

/// The `:Label`s of the node patterns in a Cypher statement: `(p:Person)`,
/// `(:Person)`, `(p:Person:Admin)`. Inside a `( … )` labels are read only
/// before the property map `{` (`{active:true}` holds values, not labels,
/// and is skipped to its closing `}`); a `:` next to another `:` (`::`, a
/// Cypher type predicate) is skipped whole; relationship types in `[ … ]`
/// sit outside every `( … )` and are never read. A label opens with a letter
/// or `_`. Bytes only: every slice is cut at an ASCII delimiter.
fn extract_node_labels(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'(' {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        let mut labels_open = true;
        while j < b.len() && b[j] != b')' {
            match b[j] {
                b'{' => {
                    labels_open = false;
                    j = brace_end(b, j);
                }
                b':' if labels_open => {
                    let run = b[j..].iter().take_while(|&&c| c == b':').count();
                    if run > 1 {
                        j += run;
                        continue;
                    }
                    let s = j + 1;
                    let mut k = s;
                    while k < b.len() && is_word_byte(b[k]) {
                        k += 1;
                    }
                    if k > s && k - s < 128 && !b[s].is_ascii_digit() {
                        out.push(text[s..k].to_string());
                    }
                    j = k.max(s);
                }
                _ => j += 1,
            }
        }
        i = j + 1;
    }
    out
}

/// Offset just past the `}` closing the `{` at `open`, or the end of `b`.
fn brace_end(b: &[u8], open: usize) -> usize {
    let mut depth = 0usize;
    for (k, &c) in b.iter().enumerate().skip(open) {
        match c {
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return k + 1;
                }
            }
            _ => {}
        }
    }
    b.len()
}

/// `[data-entity] cypher statements=N labels=a,b rejected_outside_literal=K`
/// for a file with a Cypher statement or a rejected keyword hit, else `None`.
/// `labels` lists each label once, first-seen order (empty when none); `K`
/// counts the `MATCH` / `MERGE` / `CREATE` hits of [`find_keyword_ci`] (what
/// the pre-LA.28 whole-file scan read from) that fall outside every Cypher
/// statement.
fn cypher_marker(source: &str, stmts: &[CypherStmt], labels: &[String]) -> Option<String> {
    let mut rejected = 0;
    for keyword in CYPHER_PATTERN_CLAUSES {
        let kw_lower = keyword.to_ascii_lowercase();
        let mut search_from = 0;
        while search_from < source.len() {
            let Some(rel) = find_keyword_ci(&source[search_from..], keyword, &kw_lower) else {
                break;
            };
            let pos = search_from + rel;
            if !stmts.iter().any(|s| s.span.contains(&pos)) {
                rejected += 1;
            }
            // The keyword is followed by an ASCII whitespace byte, so this
            // is a char boundary.
            search_from = pos + keyword.len() + 1;
        }
    }
    if stmts.is_empty() && rejected == 0 {
        return None;
    }
    let mut seen: Vec<&str> = Vec::new();
    for label in labels {
        if !seen.contains(&label.as_str()) {
            seen.push(label);
        }
    }
    Some(format!(
        "[data-entity] cypher statements={} labels={} rejected_outside_literal={rejected}",
        stmts.len(),
        seen.join(",")
    ))
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
        let src =
            "const q = \"SELECT * FROM public.users JOIN reporting.events e ON e.uid = users.id\";";
        let out = extract_data_entity_nodes(src, module_id(repo), repo);
        let qnames = entity_qnames(&out);
        assert!(qnames.contains(&"data_entity:sql:users".to_string()));
        assert!(qnames.contains(&"data_entity:sql:events".to_string()));
    }

    #[test]
    fn create_table_if_not_exists_captured() {
        let repo = RepoId(1);
        let src = r#"
db.exec("CREATE TABLE IF NOT EXISTS users (id SERIAL PRIMARY KEY)");
db.exec("create temporary table if not exists `tmp`.`sessions` (id int)");
db.exec("CREATE OR REPLACE TABLE [dbo].[Invoices] (id INT)");
db.exec("CREATE TABLE \"audit_log\" (id INT)");
"#;
        let out = extract_data_entity_nodes(src, module_id(repo), repo);
        let mut qnames = entity_qnames(&out);
        qnames.sort();
        assert_eq!(
            qnames,
            [
                "data_entity:sql:Invoices",
                "data_entity:sql:audit_log",
                "data_entity:sql:sessions",
                "data_entity:sql:users",
            ]
        );
    }

    #[test]
    fn alter_table_only_captured() {
        // Raw SQL outside any string literal is a `.sql` file's shape, read by
        // the whole-file mode (`migrations::extract_sql_migration`); a code
        // file's scan reads SQL only from literals (LG.3b).
        let repo = RepoId(1);
        let src = r#"
ALTER TABLE ONLY users ADD COLUMN verified BOOLEAN DEFAULT false;
ALTER TABLE IF EXISTS ONLY public.orders DROP COLUMN note;
DROP TABLE IF EXISTS tmp_a, tmp_b CASCADE;
TRUNCATE TABLE events;
TRUNCATE carts;
"#;
        let code = extract_data_entity_nodes(src, module_id(repo), repo);
        assert!(
            entity_qnames(&code).is_empty(),
            "{:?}",
            entity_qnames(&code)
        );
        let out = crate::migrations::extract_sql_migration(
            src,
            "db/migrations/V1__init.sql",
            module_id(repo),
            repo,
        )
        .entities;
        let mut qnames = entity_qnames(&out);
        qnames.sort();
        assert_eq!(
            qnames,
            [
                "data_entity:sql:carts",
                "data_entity:sql:events",
                "data_entity:sql:orders",
                "data_entity:sql:tmp_a",
                "data_entity:sql:tmp_b",
                "data_entity:sql:users",
            ]
        );
        let ddl: Vec<&str> = scan_sql_ddl(src)
            .into_iter()
            .map(|(verb, _)| verb)
            .collect();
        assert_eq!(
            ddl,
            ["alter", "alter", "drop", "drop", "truncate", "truncate"]
        );
    }

    #[test]
    fn create_index_names_its_table() {
        let src = r#"
CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS idx_users_email ON users (email);
CREATE INDEX ON ONLY public.orders USING gin (tags);
create index "ix_events" on events(created_at);
-- we create index files on startup.
"#;
        let got = scan_sql_ddl(src);
        assert_eq!(
            got,
            [
                ("index", "users".to_string()),
                ("index", "orders".to_string()),
                ("index", "events".to_string()),
            ]
        );
    }

    #[test]
    fn table_in_prose_is_not_an_entity() {
        // `select ` opens the SQL gate for the whole file, so prose around a
        // real query must not read as DDL: no bare `TABLE` keyword, and a bare
        // `TRUNCATE x` needs `x` to end the statement.
        let repo = RepoId(1);
        let src = r#"
// the users table maps onto the Account model; truncate long names first.
// We create table rows lazily and drop table-like caches on exit.
const q = "SELECT id FROM users";
const tpl = `<select class="truncate block"></select>`;
"#;
        let out = extract_data_entity_nodes(src, module_id(repo), repo);
        assert_eq!(entity_qnames(&out), ["data_entity:sql:users"]);
    }

    #[test]
    fn drop_table_alone_opens_only_the_ddl_scan() {
        // `DROP TABLE` is not a `has_sql_context` signature; it opens the DDL
        // scan without letting the prose `from this` reach the FROM scan.
        let repo = RepoId(1);
        let src = "// adapted from somewhere\nconn.execute(\"DROP TABLE legacy_users\")\n";
        let out = extract_data_entity_nodes(src, module_id(repo), repo);
        assert_eq!(entity_qnames(&out), ["data_entity:sql:legacy_users"]);
    }

    #[test]
    fn sql_does_not_match_fromsomething() {
        // word-boundary check: FROMUSERS shouldn't match.
        let repo = RepoId(1);
        let src = "const word = \"FROMUSERS is a column name\";";
        let out = extract_data_entity_nodes(src, module_id(repo), repo);
        assert!(
            out.nodes.is_empty(),
            "FROMUSERS must not match `FROM users`"
        );
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

    // ---- LA.28: Cypher labels only from Cypher-shaped string literals ----

    /// The `data_entity:graph:*` labels of `src`, in emission order.
    fn graph_labels(src: &str) -> Vec<String> {
        let repo = RepoId(1);
        let out = extract_data_entity_nodes(src, module_id(repo), repo);
        out.nodes
            .iter()
            .filter_map(|n| out.nav.qname_by_id.get(&n.id))
            .filter_map(|q| q.strip_prefix("data_entity:graph:"))
            .map(str::to_string)
            .collect()
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn rust_match_over_paths_is_not_cypher() {
        let src = "pub fn pick(kind: u32, a: u64, b: u64) -> u64 {\n    match kind { k if k == (node_kind::CLASS) => a, _ => (merged::pick_primary(a, b)) }\n}\n";
        assert_eq!(graph_labels(src), Vec::<String>::new());
        // `::` is never a label, even inside a Cypher statement's parens.
        assert_eq!(
            extract_node_labels(
                "MATCH (n) WHERE (node_kind::CLASS) OR (merged::pick_primary(a, b))"
            ),
            Vec::<String>::new()
        );
        assert_eq!(
            extract_node_labels("MATCH (n:Person) WHERE (n.age :: INTEGER) AND (x::Y)"),
            strings(&["Person"])
        );
    }

    #[test]
    fn cypher_in_a_comment_is_not_a_label() {
        let src =
            "// the Cypher form MATCH (x:Label)\n# MERGE (n:Node)\n/* CREATE (m:Block) */\nx = 1\n";
        assert_eq!(graph_labels(src), Vec::<String>::new());
    }

    #[test]
    fn lowercase_prose_create_is_not_cypher() {
        let src = "// create a user (id:abc) here\n# create a user (id:abc) here\nmsg = \"create a user (id:abc) here\"\n";
        assert_eq!(graph_labels(src), Vec::<String>::new());
    }

    #[test]
    fn sql_merge_into_is_not_cypher() {
        let src =
            "q = \"MERGE INTO t USING s ON (t.id = :id) WHEN MATCHED THEN UPDATE SET x = 1\"\n";
        assert_eq!(graph_labels(src), Vec::<String>::new());
    }

    #[test]
    fn property_map_values_are_not_labels() {
        let src = "q = \"MATCH (u:User {active:true}) RETURN u\"\n";
        assert_eq!(graph_labels(src), strings(&["User"]));
        // The map is skipped to its own `}`: a `)` inside it ends nothing.
        assert_eq!(
            extract_node_labels(
                "MERGE (e:Event {at: datetime({epochMillis: $ts}), kind:x}) RETURN e"
            ),
            strings(&["Event"])
        );
    }

    #[test]
    fn java_spring_data_query_annotation() {
        let src = r#"
public interface MovieRepository extends Neo4jRepository<Movie, Long> {
    @Query("MATCH (m:Movie)<-[:ACTED_IN]-(p:Person) RETURN m")
    List<Movie> findActedIn();
}
"#;
        assert_eq!(graph_labels(src), strings(&["Movie", "Person"]));
    }

    #[test]
    fn concatenated_cypher_statement() {
        let js = "const q = \"MATCH \" + \"(o:Order) RETURN o\";\n";
        assert_eq!(graph_labels(js), strings(&["Order"]));
        let py =
            "tx.run(\n    \"MERGE (c:Company {id: $id}) \"\n    \"RETURN c\",\n    id=cid,\n)\n";
        assert_eq!(graph_labels(py), strings(&["Company"]));
    }

    #[test]
    fn cypher_statement_shapes() {
        for yes in [
            "MATCH (n) RETURN n",
            "  match (n:Person) return n",
            "OPTIONAL MATCH (n:Person) RETURN n",
            "MATCH p = (a)-[:KNOWS]->(b) RETURN p",
            "CREATE\n(n:Person)",
            "UNWIND $rows AS r MERGE (n:Person {id: r.id})",
            "WITH $x AS x MATCH (n:Person) RETURN n",
            "CALL db.labels() YIELD label MATCH (n) RETURN n",
            "(MATCH (n) RETURN n)",
        ] {
            assert!(is_cypher_statement(yes), "{yes:?}");
        }
        for no in [
            "CREATE TABLE users (id int)",
            "CREATE INDEX idx ON users (id)",
            "create()",
            "MATCH(n:Person) RETURN n",
            "MERGE INTO t USING s ON (t.id = :id)",
            "OPTIONAL (n)",
            "OPTIONAL MERGE (n)",
            "WITH x AS (SELECT 1) SELECT * FROM x",
            "UNWIND $rows AS r RETURN r",
            "create a user (id:abc) here",
            "MATCH x == (n)",
            "Matches (n:Person)",
        ] {
            assert!(!is_cypher_statement(no), "{no:?}");
        }
    }

    #[test]
    fn cypher_marker_counts_rejected_keywords() {
        let people = "from neo4j import GraphDatabase\n\ndef find_person(tx, name):\n    return tx.run(\"MATCH (p:Person {name: $name}) RETURN p\", name=name).single()\n\ndef upsert_company(tx, cid):\n    tx.run(\n        \"MERGE (c:Company {id: $id}) \"\n        \"RETURN c\",\n        id=cid,\n    )\n";
        let stmts = cypher_statements(people);
        let labels = scan_cypher_labels(people);
        assert_eq!(labels, strings(&["Person", "Company"]));
        assert_eq!(
            cypher_marker(people, &stmts, &labels).as_deref(),
            Some(
                "[data-entity] cypher statements=2 labels=Person,Company rejected_outside_literal=0"
            )
        );
        let merged = "// Graph merge helpers. A Rust `match` is not Cypher, and neither is this\n// comment about the Cypher form MATCH (x:Label).\npub fn pick(kind: u32) -> u64 {\n    match kind {\n        _ => 0,\n    }\n}\n";
        let stmts = cypher_statements(merged);
        assert_eq!(
            cypher_marker(merged, &stmts, &[]).as_deref(),
            Some("[data-entity] cypher statements=0 labels= rejected_outside_literal=3")
        );
        assert_eq!(cypher_marker("x = 1\n", &[], &[]), None);
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
        assert!(
            out.nodes.is_empty(),
            "suffix keys must not match TableName:"
        );
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

    /// `head` + `'x'` padding + U+1F600 + `tail`, with the 4-byte char starting
    /// at `anchor + width - 1`, so a `width`-byte window from `anchor` ends on
    /// the char's 2nd byte — the cut that panicked before LA.25a.
    fn cut_inside_char(head: &str, anchor: usize, width: usize, tail: &str) -> String {
        let at = anchor + width - 1;
        assert!(head.len() <= at, "head too long for the cut");
        let src = format!("{head}{}\u{1F600}{tail}", "x".repeat(at - head.len()));
        assert!(!src.is_char_boundary(anchor + width));
        src
    }

    #[test]
    fn beanie_window_cut_inside_a_multibyte_char() {
        let repo = RepoId(1);
        let head =
            "class Order(Document):\n    class Settings:\n        name = 'orders'\n        # ";
        let after = head.find("class Settings:").unwrap() + "class Settings:".len();
        let src = cut_inside_char(head, after, 256, "\n");
        let out = extract_data_entity_nodes(&src, module_id(repo), repo);
        let qnames = entity_qnames(&out);
        assert!(
            qnames.contains(&"data_entity:nosql:orders".to_string()),
            "{qnames:?}"
        );
    }

    #[test]
    fn cypher_window_cut_inside_a_multibyte_char() {
        // The statement sits inside a string literal, so this holds for the
        // window scan and for LA.28's literal-scoped rewrite alike.
        let repo = RepoId(1);
        let head = "q = \"MATCH (p:Person) ";
        let pos = head.find("MATCH").unwrap();
        let src = cut_inside_char(head, pos, 256, " RETURN p\"\n");
        let out = extract_data_entity_nodes(&src, module_id(repo), repo);
        let qnames = entity_qnames(&out);
        assert!(
            qnames.contains(&"data_entity:graph:Person".to_string()),
            "{qnames:?}"
        );
    }

    // ---- LG.3b: literal-scoped raw SQL, driver collection calls ----------

    fn sorted_qnames(src: &str) -> Vec<String> {
        let repo = RepoId(1);
        let mut q = entity_qnames(&extract_data_entity_nodes(src, module_id(repo), repo));
        q.sort();
        q
    }

    fn sql(names: &[&str]) -> Vec<String> {
        let mut v: Vec<String> = names
            .iter()
            .map(|n| format!("data_entity:sql:{n}"))
            .collect();
        v.sort();
        v
    }

    #[test]
    fn go_select_statement_is_not_sql() {
        // Go's `select {` channel statement and prose in comments / error
        // strings used to open the file-wide scan (quokka helper.go).
        let src = r#"package chat
// drain the backlog from each room before closing
func Fanout(done <-chan struct{}, in <-chan int) {
	for {
		select {
		case v := <-in:
			log.Printf("message from the stream: %d", v)
		case <-done:
			return
		}
	}
}
"#;
        assert!(sorted_qnames(src).is_empty(), "{:?}", sorted_qnames(src));
    }

    #[test]
    fn prose_after_a_real_query_is_not_scanned() {
        let src = r#"
// results are copied from the pool before they are returned
rows, err := db.Query("SELECT id, name FROM users WHERE active = true")
c.JSON(401, gin.H{"error": "Failed to extract email from token"})
c.String(200, "You've been unsubscribed from all emails.")
msg := "Select from the list below."
"#;
        assert_eq!(sorted_qnames(src), sql(&["users"]));
    }

    #[test]
    fn concatenated_literals_join() {
        let src = r#"
q := "SELECT o.id, i.sku " +
	"FROM orders o JOIN order_items i ON i.order_id = o.id " +
	"WHERE o.created_at > $1"
$sql = 'SELECT * ' . 'FROM invoices WHERE id = ?';
cur.execute("SELECT id "
            "FROM py_users")
"#;
        assert_eq!(
            sorted_qnames(src),
            sql(&["invoices", "order_items", "orders", "py_users"])
        );
        // A variable between the pieces breaks the join: the second piece is
        // not a statement on its own.
        let broken = r#"q := "SELECT id " + cols + "FROM hidden""#;
        assert!(
            sorted_qnames(broken).is_empty(),
            "{:?}",
            sorted_qnames(broken)
        );
    }

    #[test]
    fn cte_names_are_not_tables() {
        let src = r#"
const q = `WITH RECURSIVE task_counts(pid, n) AS (SELECT pursuit_id, count(*) FROM pursuit_task GROUP BY 1),
opp_ids AS MATERIALIZED (SELECT id FROM opportunities)
SELECT * FROM task_counts tc JOIN opp_ids o ON o.id = tc.pid JOIN briefs b ON b.id = o.id`
"#;
        assert_eq!(
            sorted_qnames(src),
            sql(&["briefs", "opportunities", "pursuit_task"])
        );
    }

    #[test]
    fn lateral_set_and_functions_are_not_tables() {
        let src = r#"
const scores = `SELECT r.id, EXTRACT(YEAR FROM COALESCE(r.start, now()))::int, SUBSTRING(r.code FROM 1 FOR 2)
FROM ONLY records r
LEFT JOIN LATERAL (SELECT SUM(amount) AS total FROM payments p WHERE p.rid = r.id) s ON true
CROSS JOIN LATERAL jsonb_array_elements(r.tags) t
WHERE r.a IS DISTINCT FROM r.b`
const upsert = `INSERT INTO prefs (user_id, theme) VALUES ($1, $2)
ON CONFLICT (user_id) DO UPDATE SET theme = EXCLUDED.theme`
const series = "SELECT g FROM generate_series(1, 10) g FOR UPDATE SKIP LOCKED"
"#;
        assert_eq!(sorted_qnames(src), sql(&["payments", "prefs", "records"]));
        let mut rejected_fn = 0;
        let got: Vec<String> = scan_sql_tables(
            "SELECT EXTRACT(EPOCH FROM ts) FROM unnest($1) u JOIN users x ON true",
            &mut rejected_fn,
        )
        .into_iter()
        .map(|(t, _)| t)
        .collect();
        assert_eq!(got, ["users"]);
        assert_eq!(rejected_fn, 2);
    }

    #[test]
    fn fmt_error_messages_are_not_sql() {
        let src = r#"
return fmt.Errorf("delete from spaces key=%q: %w", key, err)
return fmt.Errorf("update from queue set failed: %v", err)
rows, _ := db.Query("SELECT id FROM users WHERE name LIKE '%water%'")
"#;
        assert_eq!(sorted_qnames(src), sql(&["users"]));
        assert_eq!(sql_statements(src).rejected_fmt, 2);
    }

    #[test]
    fn python_triple_quote_sql() {
        let src = "# the users table is read from the replica\n\
                   QUERY = \"\"\"\n    SELECT u.id\n    FROM users u\n    JOIN teams t ON t.id = u.team_id\n\"\"\"\n\
                   other = '''INSERT INTO audit (msg) VALUES (%s)'''\n";
        assert_eq!(sorted_qnames(src), sql(&["audit", "teams", "users"]));
    }

    #[test]
    fn ruby_heredoc_sql() {
        let src = r#"
class Report
  # pull everything from the warehouse
  def rows
    connection.select_all(<<~SQL.squish)
      SELECT * FROM line_items li
      JOIN carts c ON c.id = li.cart_id
    SQL
  end

  def php_like
    $q = <<<'EOT'
    DELETE FROM sessions WHERE expires_at < now()
    EOT;
  end
end
"#;
        assert_eq!(
            sorted_qnames(src),
            sql(&["carts", "line_items", "sessions"])
        );
        // A shift is not a heredoc.
        assert!(string_literals("let x = 1<<SHIFT;\nlet y = 2;\n").is_empty());
    }

    #[test]
    fn rust_raw_string_sql() {
        let src = "fn q<'a>(db: &'a Db) {\n    \
                   sqlx::query(r#\"SELECT \"id\" FROM accounts WHERE note = 'a \"quoted\" word'\"#);\n    \
                   sqlx::query(r\"UPDATE ledgers SET x = 1\");\n}\n";
        assert_eq!(sorted_qnames(src), sql(&["accounts", "ledgers"]));
    }

    #[test]
    fn csharp_verbatim_and_multibyte_literals() {
        let src = "var q = @\"SELECT \"\"Id\"\" FROM [dbo].[Customers] WHERE Name = N'Zoë ✓'\";\n\
                   var label = \"Zoë ✓ picks from the café\";\n";
        assert_eq!(sorted_qnames(src), sql(&["Customers"]));
        for lit in string_literals(src) {
            assert!(src.is_char_boundary(lit.body.start) && src.is_char_boundary(lit.body.end));
        }
    }

    #[test]
    fn go_collection_capital_c() {
        let src = r#"
_, err := client.Database("shop").Collection("events").InsertOne(ctx, ev)
coll := fs.Collection(name)
MongoCollection<Document> c = database.getCollection("ledger");
"#;
        let repo = RepoId(1);
        let mut q = entity_qnames(&extract_data_entity_nodes(src, module_id(repo), repo));
        q.sort();
        assert_eq!(q, ["data_entity:nosql:events", "data_entity:nosql:ledger"]);
    }

    #[test]
    fn csharp_get_collection_literal_and_nameof() {
        let src = r#"
_orders = db.GetCollection<OrderReadModel>(nameof(OrderReadModel));
_audit = db.GetCollection<BsonDocument>("audit_log");
_nested = db.GetCollection<Dictionary<string, List<int>>>(nameof(Models.Snapshot));
_plain = db.GetCollection("plain_c");
"#;
        let repo = RepoId(1);
        let mut q = entity_qnames(&extract_data_entity_nodes(src, module_id(repo), repo));
        q.sort();
        assert_eq!(
            q,
            [
                "data_entity:nosql:OrderReadModel",
                "data_entity:nosql:Snapshot",
                "data_entity:nosql:audit_log",
                "data_entity:nosql:plain_c",
            ]
        );
    }

    #[test]
    fn csharp_get_collection_variable_mints_nothing() {
        let src = r#"
var dynamic = db.GetCollection<BsonDocument>(auditName);
var interp = db.GetCollection<BsonDocument>($"audit_{tenant}");
const late = db.collection(name); const other = "not_a_collection";
const tpl = db.collection(`${prefix}_users`);
"#;
        assert!(
            scan_collection_calls(src).is_empty(),
            "{:?}",
            scan_collection_calls(src)
        );
    }

    #[test]
    fn apostrophe_in_comment_does_not_open_a_literal() {
        // `don't` inside a comment and `'a` lifetimes must not swallow the
        // literal that follows.
        let src = "x = compute()  # don't touch\n\
                   q = \"SELECT id FROM widgets\"\n\
                   fn f<'a>(s: &'a str) -> &'a str { s }\n\
                   <p>It's from the menu</p>\n";
        assert_eq!(sorted_qnames(src), sql(&["widgets"]));
    }

    #[test]
    fn string_builder_appends_join_into_one_statement() {
        // webplatform ProductAdminService: the SELECT and each FROM / JOIN
        // line are separate AppendLine calls, and the first starts with `\n`.
        let src = r#"
var sqlQry = new StringBuilder(500);
sqlQry.AppendLine(
    "\nSELECT pv.MSRP, p.Sku, coalesce(nullif(pa.StrDefaultValue, ''), 'x') as AttDefault");
sqlQry.AppendLine("FROM Product p");
sqlQry.AppendLine("INNER JOIN ProductVariant pv ON p.ProductId = pv.ProductID");
sqlQry.AppendLine("LEFT JOIN Attribute pa on pa.attributeid = p.attributeid");
sqlQry.Append("WHERE Sku IN (");
StringBuilder sb = new StringBuilder();
sb.append("SELECT id ").append("FROM accounts a ").append("JOIN owners o ON o.id = a.owner_id");
var b strings.Builder
b.WriteString("UPDATE go_items ")
b.WriteString("SET done = true")
"#;
        assert_eq!(
            sorted_qnames(src),
            sql(&[
                "Attribute",
                "Product",
                "ProductVariant",
                "accounts",
                "go_items",
                "owners"
            ])
        );
        let py = "q = \"SELECT id \"\nq += \"FROM py_orders o \"\nq += \"JOIN py_lines l ON l.oid = o.id\"\n\
                  $sql = 'SELECT * ';\n$sql .= 'FROM php_rows';\n";
        assert_eq!(
            sorted_qnames(py),
            sql(&["php_rows", "py_lines", "py_orders"])
        );
        // A builder line alone is no statement: nothing to join it to.
        let lone = "sb.Append(\"total: \");\nsb.Append(\"FROM the list\");\n";
        assert!(sorted_qnames(lone).is_empty(), "{:?}", sorted_qnames(lone));
    }

    #[test]
    fn escaped_newlines_keep_keywords_words() {
        let src = "const q = \"SELECT a,\\n  b\\nFROM escaped_t\\n\\tJOIN other_t ON true\";\n";
        assert_eq!(sorted_qnames(src), sql(&["escaped_t", "other_t"]));
    }

    #[test]
    fn subquery_builder_fragment_is_sql() {
        // lapse opportunity_filters.go: a WHERE fragment appended to a builder.
        let src = r#"
sb.WriteString(" AND EXISTS (SELECT 1 FROM entity_records er WHERE er.record_id = o.record_id)")
sb.WriteString(" AND o.status = 'open' FROM the builder")
label := "(select from the menu)"
doc := "Loads rows from cache (SELECT id FROM docs_t) when warm"
"#;
        assert_eq!(sorted_qnames(src), sql(&["docs_t", "entity_records"]));
    }

    #[test]
    fn js_regex_quotes_do_not_flip_literals() {
        // Kina contracts/scripts: `replace(/'/g, "''")` before the SQL it prints.
        let src = r#"
const sqlAddr = w.address.replace(/'/g, "''");
const half = total / 2, ratio = (a) / b;
console.log(`UPDATE wallets SET addr = '${sqlAddr}' WHERE id = 1;`);
console.log("SELECT email FROM users WHERE email LIKE 'demo-%@x.com' ORDER BY email;");
"#;
        assert_eq!(sorted_qnames(src), sql(&["users", "wallets"]));
    }

    #[test]
    fn unterminated_quotes_stay_linear() {
        // Every `"` after the first is escaped, so each opener would rescan
        // to the end without the exhausted flag; the scan must still find the
        // closed literal before them.
        let mut src = String::from("q = \"SELECT id FROM gadgets\"\n");
        for _ in 0..20_000 {
            src.push_str("k = \\\"; ");
        }
        assert!(src.ends_with("k = \\\"; "));
        let lits = string_literals(&src);
        assert_eq!(&src[lits[0].body.clone()], "SELECT id FROM gadgets");
        assert_eq!(sorted_qnames(&src), sql(&["gadgets"]));
    }

    #[test]
    fn sql_file_mode_scans_raw_ddl() {
        let src = "CREATE TABLE IF NOT EXISTS users (\n  id SERIAL PRIMARY KEY\n);\n\
                   ALTER TABLE ONLY orders ADD COLUMN note TEXT;\n\
                   WITH stale AS (SELECT id FROM orders) DELETE FROM carts WHERE id IN (SELECT id FROM stale);\n\
                   INSERT INTO stale_log SELECT * FROM carts;\n";
        let repo = RepoId(1);
        let out = crate::migrations::extract_sql_migration(
            src,
            "db/migrations/V1__init.sql",
            module_id(repo),
            repo,
        );
        let mut q = entity_qnames(&out.entities);
        q.sort();
        assert_eq!(q, sql(&["carts", "orders", "stale_log", "users"]));
        let tables: Vec<String> = sql_file_tables(&blank_sql_noise(src))
            .into_iter()
            .map(|(t, _)| t)
            .collect();
        assert!(!tables.contains(&"stale".to_string()), "{tables:?}");
    }

    #[test]
    fn literal_hit_offsets_map_to_source() {
        let src = "q := \"SELECT o.id \" +\n\t\"FROM orders o\"\n";
        let lit = sql_statements(src);
        assert_eq!(lit.literals, 2);
        let stmt = &lit.statements[0];
        assert_eq!(stmt.text, "SELECT o.id FROM orders o");
        let scan = statement_tables(&stmt.scan);
        let (name, pos) = &scan.tables[0];
        assert_eq!(name, "orders");
        let at = stmt.to_source(*pos);
        assert_eq!(&src[at..at + "orders".len()], "orders");
    }

    #[test]
    fn marker_reports_the_scan() {
        let src = "q := `WITH recent AS (SELECT id FROM orders) SELECT * FROM recent r JOIN payments p ON true`\n\
                   e := fmt.Errorf(\"delete from spaces key=%q: %w\", k, err)\n\
                   c := client.Database(\"shop\").Collection(\"events\")\n";
        let lit = sql_statements(src);
        let mut stats = ScanStats {
            literals: lit.literals,
            sql: lit.statements.len(),
            rejected_fmt: lit.rejected_fmt,
            ..ScanStats::default()
        };
        for stmt in &lit.statements {
            let scan = statement_tables(&stmt.scan);
            stats.ctes += scan.ctes;
            stats.rejected_fn += scan.rejected_fn;
            for (name, pos) in scan.tables {
                stats.note_table(&name, stmt.to_source(pos));
            }
        }
        for name in scan_collection_calls(src) {
            stats.note_collection(&name);
        }
        assert!(stats.fired());
        assert_eq!(
            stats.marker(src),
            "[data-entity] literals=4 sql=1 tables=orders@1,payments@1 ctes=1 collections=events rejected_fn=0 rejected_fmt=1"
        );
    }

    // ---- LA.42: declaration names come from a leading single-line literal --

    #[test]
    fn collection_argument_must_start_with_a_literal() {
        assert_eq!(
            sorted_qnames("db.collection(name);\nconst LABEL = 'users';\n"),
            Vec::<String>::new()
        );
        assert_eq!(
            sorted_qnames("db.collection(\n  'orders'\n)\n"),
            ["data_entity:nosql:orders"]
        );
        // data_entities.rs's own comment shape: the backtick literal
        // ` matches but ` starts the argument and closes on its line.
        assert_eq!(
            sorted_qnames("// so `someother.collection(` matches but `_collection(` does not.\n"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn mongoose_model_needs_a_literal_first_argument() {
        assert_eq!(
            sorted_qnames("mongoose.model(modelName, schema);\nexport const LABEL = 'Registry';\n"),
            Vec::<String>::new()
        );
        assert_eq!(
            sorted_qnames("mongoose.model(\n  'User',\n  userSchema,\n)\n"),
            ["data_entity:nosql:User"]
        );
    }

    #[test]
    fn needle_tables_are_not_entities() {
        for src in [
            "NEEDLES = [\"mongoose.model(\", \"TableName:\", \"dynamodb.Table(\"]\n",
            "//! Mongoose: mongoose.model('<Name>', ...)\n",
            "// TableName: '...'\n",
        ] {
            assert_eq!(sorted_qnames(src), Vec::<String>::new(), "{src:?}");
        }
    }

    #[test]
    fn tablename_needs_an_assigned_literal() {
        assert_eq!(
            sorted_qnames(
                "@declared_attr\ndef __tablename__(cls):\n    return cls.__name__.lower()\n\nAUDIT = \"audit\"\n"
            ),
            Vec::<String>::new()
        );
        assert_eq!(
            sorted_qnames("__tablename__: str = \"orders\"\n"),
            ["data_entity:sql:orders"]
        );
    }

    #[test]
    fn beanie_name_must_be_a_literal_in_the_block() {
        assert_eq!(
            sorted_qnames(
                "class Settings:\n    name = collection_name()\n    validate_on_save = \"strict\"\n"
            ),
            Vec::<String>::new()
        );
        assert_eq!(
            sorted_qnames(
                "class Settings:\n    use_state_management = True\n    name = \"events\"\n"
            ),
            ["data_entity:nosql:events"]
        );
    }

    #[test]
    fn dynamodb_table_name_needs_a_literal() {
        assert_eq!(
            sorted_qnames(
                "{ TableName: process.env.ORDERS_TABLE, Item: { status: { S: \"pending\" } } }\n"
            ),
            Vec::<String>::new()
        );
        assert_eq!(
            sorted_qnames("table = dynamodb.Table(table_name)\nx = 'cache'\n"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn entity_shape_is_enforced_at_the_funnel() {
        for noise in [", ", "<Name>", "...", ") {\n let", "a b", "${t}"] {
            assert!(is_noise_entity_name(noise), "{noise:?} must be noise");
        }
        for name in [
            "users",
            "audit_log",
            "orders-v2",
            "system.users",
            "users/abc/orders",
            "OrderReadModel",
            "usuários",
        ] {
            assert!(!is_noise_entity_name(name), "{name:?} must be kept");
        }
    }

    /// The four declaration scanners plus the funnel count, as
    /// `extract_data_entity_nodes` runs them.
    fn decl_stats(src: &str) -> DeclStats {
        let mut decl = DeclStats::default();
        let names = [
            scan_orm_table_decls(src, &mut decl),
            scan_mongoose_models(src, &mut decl),
            scan_dynamodb_tables(src, &mut decl),
            scan_beanie_documents(src, &mut decl),
        ]
        .concat();
        for name in &names {
            decl.funnel(name);
        }
        decl
    }

    #[test]
    fn decl_marker_counts_every_needle_hit() {
        let needles = "NEEDLES = [\"mongoose.model(\", \"TableName:\", \"dynamodb.Table(\"]\n";
        assert_eq!(
            decl_stats(needles).marker(),
            "[data-entity] decl needles=3 kept=0 rejected_nonliteral=1 rejected_shape=2"
        );
        let registry = "export const User = mongoose.model(\"User\", userSchema);\n\
                        export const f = (m) => mongoose.model(modelName, schema);\n\
                        export const LABEL = \"Registry\";\n";
        assert_eq!(
            decl_stats(registry).marker(),
            "[data-entity] decl needles=2 kept=1 rejected_nonliteral=1 rejected_shape=0"
        );
        let models = "class Order:\n    __tablename__ = \"orders\"\n\n\
                      class Base:\n    def __tablename__(cls):\n        return 'x'\n    db_table = '...'\n";
        assert_eq!(
            decl_stats(models),
            DeclStats {
                needles: 3,
                kept: 1,
                rejected_nonliteral: 1,
                rejected_shape: 1
            }
        );
    }
}
