//! Database migrations (A13.9): which tables a migration creates, alters or
//! drops, so "which service owns the table this migration alters" has an
//! answer.
//!
//! Two shapes, one output. No new kind: the migration file's MODULE anchors an
//! ACCESSES_DATA edge to each `data_entity:sql:<table>`, the same DATA_ENTITY
//! the query sites of `data_entities` mint, so a migration and the code that
//! reads the table meet on one node.
//!
//! * Raw SQL migration files (Flyway, golang-migrate, Liquibase formatted SQL,
//!   dbmate / sqlx / diesel / Prisma `migrations/` dirs). The walk reads a
//!   `.sql` only when [`is_migration_path`] admits it, and `engine::route`
//!   sends it to [`extract_sql_migration`]. The gate is deliberately narrow:
//!   a repo full of query fixtures (`tests/data/q.sql`) is never read.
//! * Migration DSLs in code files: Alembic, Django, Rails, knex / Sequelize /
//!   TypeORM, Laravel, EF Core. [`scan_migration_dsl`] runs from
//!   `data_entities::extract_data_entity_nodes` on every code file, which is
//!   right: `create_table :orders` declares a table wherever it appears.
//!
//! The DDL verb (`create` / `alter` / `drop` / `truncate`) rides only the
//! `[migrations]` marker for now; the write-mode edge cell arrives with LC.2
//! (LE.4).

use repo_graph_core::{NodeId, RepoId};

use crate::data_entities::{
    DataEntityFlavor, DataEntityNodes, EntitySink, canonical_sql_name, scan_sql_ddl,
    scan_sql_tables,
};

/// True when the walk should read `path` as a SQL migration. See
/// [`migration_framework`] for the shapes.
pub fn is_migration_path(path: &str) -> bool {
    migration_framework(path).is_some()
}

/// The migration tool a `.sql` path's shape names, or `None` when the path is
/// not a migration at all:
/// * `flyway`: basename `V<version>__<desc>.sql`, `U<version>__<desc>.sql` or
///   `R__<desc>.sql`, wherever it sits;
/// * `golang-migrate`: basename ending `.up.sql` / `.down.sql`;
/// * `liquibase`: under a `db/changelog/` directory;
/// * `raw`: under `db/{migrations,migration,migrate,sql}/`, under any
///   `migrations/` directory, or the schema dump `db/structure.sql` /
///   `db/schema.sql`.
pub fn migration_framework(path: &str) -> Option<&'static str> {
    // The walk asks this of every file: reject a non-`.sql` before allocating.
    let tail = path.as_bytes();
    if tail.len() < 4 || !tail[tail.len() - 4..].eq_ignore_ascii_case(b".sql") {
        return None;
    }
    let norm = path.replace('\\', "/");
    let (dir, base) = norm.rsplit_once('/').unwrap_or(("", norm.as_str()));
    let lower_base = base.to_ascii_lowercase();
    let stem_len = lower_base.strip_suffix(".sql")?.len();
    if is_flyway_name(&base[..stem_len]) {
        return Some("flyway");
    }
    if lower_base.ends_with(".up.sql") || lower_base.ends_with(".down.sql") {
        return Some("golang-migrate");
    }
    let segs: Vec<String> = dir
        .split('/')
        .filter(|s| !s.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    for pair in segs.windows(2) {
        if pair[0] != "db" {
            continue;
        }
        match pair[1].as_str() {
            "changelog" => return Some("liquibase"),
            "migrations" | "migration" | "migrate" | "sql" => return Some("raw"),
            _ => {}
        }
    }
    if segs.iter().any(|s| s == "migrations") {
        return Some("raw");
    }
    let schema_dump = matches!(lower_base.as_str(), "structure.sql" | "schema.sql");
    if schema_dump && segs.last().is_some_and(|s| s == "db") {
        return Some("raw");
    }
    None
}

/// Flyway's file-name convention, on the stem without `.sql`: `V1__init`,
/// `V1.2__add`, `V2_1__add`, `U3__undo`, `R__views`.
fn is_flyway_name(stem: &str) -> bool {
    if let Some(desc) = stem.strip_prefix("R__") {
        return !desc.is_empty();
    }
    let Some(rest) = stem.strip_prefix('V').or_else(|| stem.strip_prefix('U')) else {
        return false;
    };
    let Some((version, desc)) = rest.split_once("__") else {
        return false;
    };
    version.bytes().next().is_some_and(|c| c.is_ascii_digit())
        && version
            .bytes()
            .all(|c| c.is_ascii_digit() || c == b'.' || c == b'_')
        && !desc.is_empty()
}

/// One migration `.sql` file's tables, plus what its marker reports.
pub struct SqlMigration {
    pub entities: DataEntityNodes,
    /// [`migration_framework`]'s name for the path (`raw` if it has none).
    pub framework: &'static str,
    /// Distinct DDL verbs, first-seen order.
    pub ddl: Vec<&'static str>,
}

impl SqlMigration {
    /// The fired_on line: `[migrations] file=<path> framework=<fw> tables=N
    /// ddl=<verbs|->`.
    pub fn marker(&self, path: &str) -> String {
        let ddl = if self.ddl.is_empty() {
            "-".to_string()
        } else {
            self.ddl.join(",")
        };
        format!(
            "[migrations] file={path} framework={} tables={} ddl={ddl}",
            self.framework,
            self.entities.nodes.len()
        )
    }
}

/// The tables a migration `.sql` names: its DDL (`CREATE TABLE users`) and the
/// DML a data migration runs (`INSERT INTO users`). The whole file is SQL, so
/// no `has_sql_context` gate, and only the SQL scanners run: a Postgres cast
/// (`'a'::text`) is not a Cypher label. Two precision steps a code file does
/// not need, both measured on real migration dirs: comments and string values
/// are blanked first (`-- copied from the old schema`, a seed row's
/// `'tenders from Quotations and Tenders'`), and a capture that is a SQL
/// keyword or a system catalog is dropped (`ON UPDATE CASCADE`,
/// `DO UPDATE SET`, `UPDATE OF col`, `FROM pg_constraint`).
pub fn extract_sql_migration(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
) -> SqlMigration {
    let sql = blank_sql_noise(source);
    let mut sink = EntitySink::new(module_id, repo);
    let mut ddl = Vec::new();
    for (verb, table) in scan_sql_ddl(&sql) {
        if !ddl.contains(&verb) {
            ddl.push(verb);
        }
        sink.emit(DataEntityFlavor::Sql, &table);
    }
    for table in scan_sql_tables(&sql) {
        if !is_keyword_or_catalog(&table) {
            sink.emit(DataEntityFlavor::Sql, &table);
        }
    }
    SqlMigration {
        entities: sink.finish(),
        framework: migration_framework(path).unwrap_or("raw"),
        ddl,
    }
}

/// `source` with every `-- line` / `/* block */` comment and every
/// single-quoted string value (`'text'`, `E'it\'s'`) replaced by spaces,
/// newlines kept. In SQL a single quote delimits a value, never a name, so no
/// table is lost; double-quoted, backticked and bracketed identifiers are
/// left alone. Only ASCII bytes delimit what is blanked and every blanked byte
/// becomes an ASCII space, so the result is valid UTF-8 whenever `source` is.
fn blank_sql_noise(source: &str) -> String {
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

/// A `FROM` / `JOIN` / `INTO` / `UPDATE` capture that is not a user table: a
/// SQL keyword the clause grammar puts there (`ON UPDATE CASCADE`,
/// `BEFORE UPDATE ON t`, `DO UPDATE SET`, `UPDATE OF col`, `FOR UPDATE SKIP
/// LOCKED`, `FROM LATERAL`), or a system catalog (`pg_*`, `sqlite_*`, the
/// `information_schema` views, whose schema prefix `canonical_sql_name`
/// strips).
fn is_keyword_or_catalog(name: &str) -> bool {
    const KEYWORDS: &[&str] = &[
        "ALL", "ANY", "AS", "CASCADE", "CASE", "CROSS", "DEFAULT", "DISTINCT", "EACH", "FOR",
        "FULL", "INNER", "LATERAL", "LEFT", "NATURAL", "NO", "NOT", "NOWAIT", "OF", "ON", "ONLY",
        "OUTER", "RESTRICT", "RIGHT", "ROW", "SET", "SKIP", "STRICT", "TABLE", "USING", "VALUES",
        "WITH",
    ];
    const CATALOG_VIEWS: &[&str] = &[
        "tables",
        "columns",
        "schemata",
        "views",
        "routines",
        "triggers",
        "table_constraints",
        "key_column_usage",
        "constraint_column_usage",
        "referential_constraints",
    ];
    let lower = name.to_ascii_lowercase();
    KEYWORDS.iter().any(|k| k.eq_ignore_ascii_case(name))
        || lower.starts_with("pg_")
        || lower.starts_with("sqlite_")
        || CATALOG_VIEWS.contains(&lower.as_str())
}

// ----------------------------------------------------------------------------
// Migration DSLs
// ----------------------------------------------------------------------------

/// One table a migration DSL call names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DslTable {
    /// `alembic` | `django` | `rails` | `knex` | `sequelize` | `typeorm` |
    /// `laravel` | `efcore`.
    pub framework: &'static str,
    pub table: String,
}

/// Where a DSL call keeps its table name.
#[derive(Clone, Copy)]
enum Arg {
    /// The first positional argument: `op.create_table("users", …)`.
    First,
    /// A Ruby call's first argument, parenthesised or not, as a string or a
    /// symbol: `create_table :users`, `create_table("users")`.
    Ruby,
    /// A keyword argument at the call's own depth: Django `name="Order"`
    /// (`=`), EF Core `table: "Users"` (`:`).
    Kw(&'static str, u8),
}

struct Dsl {
    framework: &'static str,
    /// One of these must occur in the file before the DSL's needles are
    /// searched: a few cheap substring tests per code file instead of ~50.
    gate: &'static [&'static str],
    needles: &'static [(&'static str, Arg)],
}

const DSLS: &[Dsl] = &[
    Dsl {
        framework: "alembic",
        gate: &["alembic"],
        needles: &[
            ("op.create_table(", Arg::First),
            ("op.drop_table(", Arg::First),
            ("op.rename_table(", Arg::First),
            ("op.add_column(", Arg::First),
            ("op.drop_column(", Arg::First),
            ("op.alter_column(", Arg::First),
            ("op.batch_alter_table(", Arg::First),
        ],
    },
    Dsl {
        framework: "django",
        gate: &["migrations."],
        needles: &[
            ("migrations.CreateModel(", Arg::Kw("name", b'=')),
            ("migrations.DeleteModel(", Arg::Kw("name", b'=')),
            ("migrations.AddField(", Arg::Kw("model_name", b'=')),
            ("migrations.RemoveField(", Arg::Kw("model_name", b'=')),
            ("migrations.AlterField(", Arg::Kw("model_name", b'=')),
            ("migrations.RenameField(", Arg::Kw("model_name", b'=')),
            ("migrations.AddIndex(", Arg::Kw("model_name", b'=')),
            ("migrations.AddConstraint(", Arg::Kw("model_name", b'=')),
        ],
    },
    Dsl {
        framework: "rails",
        gate: &["ActiveRecord::"],
        needles: &[
            ("create_table", Arg::Ruby),
            ("change_table", Arg::Ruby),
            ("drop_table", Arg::Ruby),
            ("rename_table", Arg::Ruby),
            ("add_column", Arg::Ruby),
            ("remove_column", Arg::Ruby),
            ("change_column", Arg::Ruby),
            ("rename_column", Arg::Ruby),
            ("add_index", Arg::Ruby),
            ("remove_index", Arg::Ruby),
            ("add_reference", Arg::Ruby),
            ("remove_reference", Arg::Ruby),
            ("add_timestamps", Arg::Ruby),
            ("add_foreign_key", Arg::Ruby),
        ],
    },
    // knex, and the Sequelize / TypeORM spellings of the same builder calls:
    // the receiver names the tool (see `js_framework`).
    Dsl {
        framework: "knex",
        gate: &["Table", "schema.table("],
        needles: &[
            (".createTable(", Arg::First),
            (".createTableIfNotExists(", Arg::First),
            (".alterTable(", Arg::First),
            (".dropTable(", Arg::First),
            (".dropTableIfExists(", Arg::First),
            (".renameTable(", Arg::First),
            ("schema.table(", Arg::First),
        ],
    },
    Dsl {
        framework: "sequelize",
        gate: &["queryInterface."],
        needles: &[
            ("queryInterface.addColumn(", Arg::First),
            ("queryInterface.removeColumn(", Arg::First),
            ("queryInterface.changeColumn(", Arg::First),
            ("queryInterface.renameColumn(", Arg::First),
            ("queryInterface.addIndex(", Arg::First),
            ("queryInterface.addConstraint(", Arg::First),
        ],
    },
    Dsl {
        framework: "laravel",
        gate: &["Schema::"],
        needles: &[
            ("Schema::create(", Arg::First),
            ("Schema::table(", Arg::First),
            ("Schema::drop(", Arg::First),
            ("Schema::dropIfExists(", Arg::First),
            ("Schema::rename(", Arg::First),
        ],
    },
    Dsl {
        framework: "efcore",
        gate: &["migrationBuilder."],
        needles: &[
            ("migrationBuilder.CreateTable(", Arg::Kw("name", b':')),
            ("migrationBuilder.DropTable(", Arg::Kw("name", b':')),
            ("migrationBuilder.RenameTable(", Arg::Kw("name", b':')),
            ("migrationBuilder.AddColumn<", Arg::Kw("table", b':')),
            ("migrationBuilder.AlterColumn<", Arg::Kw("table", b':')),
            ("migrationBuilder.DropColumn(", Arg::Kw("table", b':')),
            ("migrationBuilder.RenameColumn(", Arg::Kw("table", b':')),
            ("migrationBuilder.CreateIndex(", Arg::Kw("table", b':')),
            ("migrationBuilder.AddForeignKey(", Arg::Kw("table", b':')),
            ("migrationBuilder.InsertData(", Arg::Kw("table", b':')),
        ],
    },
];

/// How far a keyword-argument search reads into a call before giving up.
const KWARG_WINDOW: usize = 4096;

/// Every table a migration DSL call in `source` names, grouped by DSL, then by
/// needle, then in source order. Each capture is the call's literal table
/// argument, cleaned by `canonical_sql_name` (`public.users` → `users`); a
/// variable or expression argument names nothing. Django's `model_name="order"` is the lowercased
/// model: it takes the spelling of a `CreateModel(name="Order")` in the same
/// file when one matches, so both land on the model-keyed entity A13.17's
/// Django models emit.
pub fn scan_migration_dsl(source: &str) -> Vec<DslTable> {
    let mut out = Vec::new();
    for dsl in DSLS {
        if !dsl.gate.iter().any(|g| source.contains(g)) {
            continue;
        }
        let first = out.len();
        for &(needle, arg) in dsl.needles {
            let mut from = 0;
            while let Some(rel) = source[from..].find(needle) {
                let pos = from + rel;
                from = pos + needle.len();
                if !needle_boundary_ok(source.as_bytes(), pos, needle, arg) {
                    continue;
                }
                let raw = match arg {
                    Arg::First => first_literal(source, from),
                    Arg::Ruby => ruby_first_arg(source, from),
                    Arg::Kw(key, sep) => kwarg_literal(source, from, key, sep),
                };
                let Some(table) = raw.as_deref().and_then(canonical_sql_name) else {
                    continue;
                };
                let framework = if dsl.framework == "knex" {
                    js_framework(source.as_bytes(), pos)
                } else {
                    dsl.framework
                };
                out.push(DslTable { framework, table });
            }
        }
        if dsl.framework == "django" {
            respell_django_models(source, &mut out[first..]);
        }
    }
    out
}

/// One `[migrations] dsl framework=<fw> tables=N` line per DSL that captured
/// something in a file, frameworks first-seen, N its distinct tables.
pub fn dsl_markers(hits: &[DslTable]) -> Vec<String> {
    let mut frameworks: Vec<&'static str> = Vec::new();
    for hit in hits {
        if !frameworks.contains(&hit.framework) {
            frameworks.push(hit.framework);
        }
    }
    frameworks
        .into_iter()
        .map(|fw| {
            let mut tables: Vec<&str> = Vec::new();
            for hit in hits.iter().filter(|h| h.framework == fw) {
                if !tables.contains(&hit.table.as_str()) {
                    tables.push(&hit.table);
                }
            }
            format!("[migrations] dsl framework={fw} tables={}", tables.len())
        })
        .collect()
}

fn is_ident(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// A needle starting with an identifier must not continue one
/// (`myop.create_table(` is not Alembic). A Ruby needle is a bare method call:
/// no receiver (`op.drop_table` is Alembic's), and a space or `(` after it
/// (`create_tables` is another method). A `.createTable(` needle takes any
/// receiver, a chained call on its own line included.
fn needle_boundary_ok(b: &[u8], pos: usize, needle: &str, arg: Arg) -> bool {
    let prev = pos.checked_sub(1).map(|p| b[p]);
    if needle.starts_with('.') {
        return true;
    }
    if prev.is_some_and(is_ident) {
        return false;
    }
    if matches!(arg, Arg::Ruby) {
        if prev.is_some_and(|p| matches!(p, b'.' | b':' | b'@' | b'$')) {
            return false;
        }
        return matches!(b.get(pos + needle.len()), Some(b' ' | b'\t' | b'('));
    }
    true
}

/// The builder call's tool, from its receiver: Sequelize's `queryInterface`,
/// TypeORM's `queryRunner`, otherwise knex (`knex.schema`, `table`, …).
fn js_framework(b: &[u8], dot: usize) -> &'static str {
    let mut s = dot;
    while s > 0 && is_ident(b[s - 1]) {
        s -= 1;
    }
    match &b[s..dot] {
        b"queryInterface" => "sequelize",
        b"queryRunner" => "typeorm",
        _ => "knex",
    }
}

/// The quoted string literal at the first non-whitespace byte at or after `i`
/// (newlines included: `op.create_table(\n    "users", …)`). `None` when the
/// argument is not a literal or does not close within 128 bytes.
fn first_literal(source: &str, i: usize) -> Option<String> {
    let b = source.as_bytes();
    let mut k = i;
    while k < b.len() && b[k].is_ascii_whitespace() {
        k += 1;
    }
    quoted_at(source, k)
}

fn quoted_at(source: &str, k: usize) -> Option<String> {
    let b = source.as_bytes();
    let q = *b.get(k)?;
    if !matches!(q, b'"' | b'\'' | b'`') {
        return None;
    }
    let s = k + 1;
    let mut e = s;
    while e < b.len() && b[e] != q && b[e] != b'\n' && e - s <= 128 {
        e += 1;
    }
    (b.get(e) == Some(&q)).then(|| source[s..e].to_string())
}

/// A Ruby call's first argument: `create_table :users`, `create_table "users"`,
/// `create_table(:users)`. The symbol form reads an identifier after a single
/// `:` (`::Const` is not a symbol).
fn ruby_first_arg(source: &str, i: usize) -> Option<String> {
    let b = source.as_bytes();
    let mut k = i;
    while k < b.len() && matches!(b[k], b' ' | b'\t') {
        k += 1;
    }
    if b.get(k) == Some(&b'(') {
        k += 1;
        while k < b.len() && b[k].is_ascii_whitespace() {
            k += 1;
        }
    }
    if b.get(k) == Some(&b':') {
        let s = k + 1;
        if !b
            .get(s)
            .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_')
        {
            return None;
        }
        let mut e = s;
        while e < b.len() && is_ident(b[e]) {
            e += 1;
        }
        return Some(source[s..e].to_string());
    }
    quoted_at(source, k)
}

/// The literal value of keyword argument `key` (`key=` / `key:`) at the call's
/// own depth, scanning from `i`, the byte after the needle. A needle ending in
/// `<` (EF Core's `AddColumn<string>(`) first skips to the call's `(`. Nested
/// calls, lists and lambdas (`columns: table => new { … }`) and quoted text
/// are stepped over; the call's closing paren ends the search.
fn kwarg_literal(source: &str, i: usize, key: &str, sep: u8) -> Option<String> {
    let b = source.as_bytes();
    let end = (i + KWARG_WINDOW).min(b.len());
    let mut k = i;
    if k > 0 && b[k - 1] == b'<' {
        while k < end && b[k] != b'(' {
            k += 1;
        }
        k += 1;
    }
    let key_b = key.as_bytes();
    let mut depth = 0usize;
    while k < end {
        match b[k] {
            q @ (b'"' | b'\'') => {
                k += 1;
                while k < end && b[k] != q {
                    if b[k] == b'\\' {
                        k += 1;
                    }
                    k += 1;
                }
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
            }
            _ if depth == 0
                && b[k..].starts_with(key_b)
                && (k == 0 || !is_ident(b[k - 1]))
                && !b.get(k + key_b.len()).is_some_and(|c| is_ident(*c)) =>
            {
                let mut j = k + key_b.len();
                while j < b.len() && matches!(b[j], b' ' | b'\t') {
                    j += 1;
                }
                if b.get(j) == Some(&sep) && b.get(j + 1) != Some(&sep) {
                    return first_literal(source, j + 1);
                }
            }
            _ => {}
        }
        k += 1;
    }
    None
}

/// Django spells a model `name="OrderItem"` in `CreateModel` / `DeleteModel`
/// and `model_name="orderitem"` everywhere else. A lowercase capture that
/// matches a model named in this file takes that model's spelling.
fn respell_django_models(source: &str, hits: &mut [DslTable]) {
    let declared: Vec<String> = ["migrations.CreateModel(", "migrations.DeleteModel("]
        .iter()
        .flat_map(|needle| {
            source
                .match_indices(needle)
                .filter_map(|(pos, n)| kwarg_literal(source, pos + n.len(), "name", b'='))
                .collect::<Vec<_>>()
        })
        .collect();
    for hit in hits.iter_mut() {
        if let Some(model) = declared
            .iter()
            .find(|m| **m != hit.table && m.eq_ignore_ascii_case(&hit.table))
        {
            hit.table = model.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use repo_graph_code_domain::{GRAPH_TYPE, node_kind};

    fn module_id(repo: RepoId) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MODULE, "db::migrations::m")
    }

    fn tables(src: &str) -> Vec<(&'static str, String)> {
        scan_migration_dsl(src)
            .into_iter()
            .map(|h| (h.framework, h.table))
            .collect()
    }

    fn qnames(out: &DataEntityNodes) -> Vec<String> {
        let mut q: Vec<String> = out.nav.qname_by_id.values().cloned().collect();
        q.sort();
        q
    }

    #[test]
    fn flyway_path_matches() {
        for p in [
            "db/migrations/V1__create_users.sql",
            "src/main/resources/db/migration/V2_1__add_email.sql",
            "sql/V20240101.1__init.sql",
            "U3__undo_users.sql",
            "db/migration/R__views.sql",
            "db\\migrations\\V1__create_users.sql",
        ] {
            assert_eq!(migration_framework(p), Some("flyway"), "{p}");
        }
        // Not Flyway: no version, no description, lowercase prefix outside a
        // migrations dir.
        assert_eq!(migration_framework("tools/V__x.sql"), None);
        assert_eq!(migration_framework("tools/V1__.sql"), None);
        assert_eq!(migration_framework("tools/v1__init.sql"), None);
        assert_eq!(
            migration_framework("db/migrations/V1__create_users.py"),
            None
        );
    }

    #[test]
    fn golang_migrate_up_down_matches() {
        assert_eq!(
            migration_framework("migrations/000001_create_users.up.sql"),
            Some("golang-migrate")
        );
        assert_eq!(
            migration_framework("internal/store/000001_create_users.down.sql"),
            Some("golang-migrate")
        );
        assert_eq!(
            migration_framework("src/main/resources/db/changelog/changes/001-init.sql"),
            Some("liquibase")
        );
        assert_eq!(
            migration_framework("db/migrate/20240101_add_orders.sql"),
            Some("raw")
        );
        assert_eq!(
            migration_framework("migrations/2024-01-01-000000_create_users/up.sql"),
            Some("raw")
        );
        assert_eq!(
            migration_framework("prisma/migrations/20240101_init/migration.sql"),
            Some("raw")
        );
        assert_eq!(migration_framework("db/structure.sql"), Some("raw"));
    }

    #[test]
    fn query_fixture_sql_does_not_match() {
        for p in [
            "tests/data/q.sql",
            "queries/users.sql",
            "src/sql/report.sql",
            "db/seeds/data.sql",
            "app/structure.sql",
            "sql/users.sql",
        ] {
            assert!(!is_migration_path(p), "{p} must stay unread");
        }
    }

    #[test]
    fn sql_migration_names_its_tables_and_verbs() {
        let repo = RepoId(1);
        let src = "-- copied from the legacy schema\n\
                   CREATE TABLE IF NOT EXISTS users (\n  id SERIAL PRIMARY KEY\n);\n\
                   /* seed from the old dump */\n\
                   ALTER TABLE ONLY public.users ADD COLUMN email TEXT DEFAULT 'a'::text;\n\
                   INSERT INTO audit_log (msg) VALUES ('-- not a comment');\n\
                   DROP TABLE IF EXISTS tmp_a, tmp_b CASCADE;\n";
        let out = extract_sql_migration(src, "db/migrations/V1__init.sql", module_id(repo), repo);
        assert_eq!(
            qnames(&out.entities),
            [
                "data_entity:sql:audit_log",
                "data_entity:sql:tmp_a",
                "data_entity:sql:tmp_b",
                "data_entity:sql:users",
            ]
        );
        assert_eq!(out.ddl, ["create", "alter", "drop"]);
        assert_eq!(
            out.marker("db/migrations/V1__init.sql"),
            "[migrations] file=db/migrations/V1__init.sql framework=flyway tables=4 ddl=create,alter,drop"
        );
    }

    #[test]
    fn alembic_create_table_captured() {
        let src = r#"from alembic import op
import sqlalchemy as sa

def upgrade():
    op.create_table(
        "users",
        sa.Column("id", sa.Integer, primary_key=True),
    )
    op.add_column('orders', sa.Column("note", sa.Text))
    op.alter_column(table_name, "x")

def downgrade():
    op.drop_table("users")
"#;
        assert_eq!(
            tables(src),
            [
                ("alembic", "users".to_string()),
                ("alembic", "users".to_string()),
                ("alembic", "orders".to_string()),
            ]
        );
        assert_eq!(
            dsl_markers(&scan_migration_dsl(src)),
            ["[migrations] dsl framework=alembic tables=2"]
        );
    }

    #[test]
    fn rails_symbol_create_table_captured() {
        let src = r#"class CreateOrders < ActiveRecord::Migration[7.0]
  def change
    create_table :orders do |t|
      t.integer :user_id
    end
    add_column(:orders, :total, :decimal)
    add_index "orders", :user_id
    create_tables :nope
    connection.create_table :nope_either
  end
end
"#;
        assert_eq!(
            tables(src),
            [
                ("rails", "orders".to_string()),
                ("rails", "orders".to_string()),
                ("rails", "orders".to_string()),
            ]
        );
    }

    #[test]
    fn rails_needles_need_the_activerecord_gate() {
        // A Python helper spelled like a Rails call is not a Rails migration.
        assert!(tables("def setup():\n    create_table :users\n").is_empty());
    }

    #[test]
    fn django_model_name_takes_the_create_model_spelling() {
        let src = r#"from django.db import migrations, models

class Migration(migrations.Migration):
    operations = [
        migrations.CreateModel(
            name='OrderItem',
            fields=[
                ('id', models.AutoField(primary_key=True)),
                ('name', models.CharField(max_length=10, verbose_name='x')),
            ],
        ),
        migrations.AddField(
            field=models.CharField(max_length=5),
            model_name='orderitem',
            name='sku',
        ),
        migrations.RemoveField(model_name='invoice', name='total'),
    ]
"#;
        assert_eq!(
            tables(src),
            [
                ("django", "OrderItem".to_string()),
                ("django", "OrderItem".to_string()),
                ("django", "invoice".to_string()),
            ]
        );
    }

    #[test]
    fn js_builders_name_their_tool_by_receiver() {
        let src = r#"exports.up = (knex) =>
  knex.schema.createTable('users', (t) => { t.increments(); })
    .createTable("orders", (t) => {});
module.exports = { up: async (queryInterface) => {
  await queryInterface.createTable('Invoices', {});
  await queryInterface.addColumn('Invoices', 'total', {});
}};
await queryRunner.dropTable("legacy");
await queryRunner.createTable(new Table({ name: "skipped" }));
"#;
        assert_eq!(
            tables(src),
            [
                ("knex", "users".to_string()),
                ("knex", "orders".to_string()),
                ("sequelize", "Invoices".to_string()),
                ("typeorm", "legacy".to_string()),
                ("sequelize", "Invoices".to_string()),
            ]
        );
    }

    #[test]
    fn laravel_schema_facade_captured() {
        let src = "<?php\nSchema::create('app_users', function (Blueprint $table) {\n    $table->id();\n});\nSchema::dropIfExists(\"sessions\");\n";
        assert_eq!(
            tables(src),
            [
                ("laravel", "app_users".to_string()),
                ("laravel", "sessions".to_string())
            ]
        );
    }

    #[test]
    fn efcore_named_arguments_at_call_depth() {
        let src = r#"protected override void Up(MigrationBuilder migrationBuilder)
{
    migrationBuilder.CreateTable(
        name: "Users",
        columns: table => new
        {
            Id = table.Column<int>(type: "int", nullable: false),
            Email = table.Column<string>(name: "email_address", nullable: true)
        },
        constraints: table => { table.PrimaryKey("PK_Users", x => x.Id); });

    migrationBuilder.AddColumn<string>(
        name: "Phone",
        table: "Users",
        nullable: true);
    migrationBuilder.DropColumn(name: "Legacy", schema: "dbo", table: "Orders");
}
"#;
        assert_eq!(
            tables(src),
            [
                ("efcore", "Users".to_string()),
                ("efcore", "Users".to_string()),
                ("efcore", "Orders".to_string()),
            ]
        );
    }

    #[test]
    fn dsl_scan_is_quiet_on_ordinary_code() {
        let src = r#"import { schema } from "./schema";
const table = console.table(rows);
grid.addColumn("Name");
function createTable(name) { return name; }
x.createTable(name);
"#;
        assert!(tables(src).is_empty(), "{:?}", tables(src));
    }

    #[test]
    fn blanked_noise_keeps_utf8_and_identifiers() {
        let src = "SELECT 'a--b ünï' FROM \"t--x\"; -- ünïcode comment\n/* ✓ */ \
                   INSERT INTO e VALUES (E'it\\'s from x', 'it''s from y');";
        let out = blank_sql_noise(src);
        assert_eq!(out.len(), src.len());
        assert!(out.contains("FROM \"t--x\";"), "{out}");
        assert!(
            !out.contains("ünï") && !out.contains('✓') && !out.contains("from"),
            "{out}"
        );
        assert!(out.contains("INSERT INTO e VALUES (E"), "{out}");
    }

    #[test]
    fn migration_clause_keywords_and_catalogs_are_not_tables() {
        let repo = RepoId(1);
        let src = "CREATE TABLE trades (id INT REFERENCES users ON UPDATE CASCADE);\n\
                   CREATE TRIGGER t BEFORE UPDATE ON trades FOR EACH ROW EXECUTE f();\n\
                   CREATE TRIGGER u AFTER INSERT OR UPDATE OF status ON users EXECUTE g();\n\
                   INSERT INTO regimes (id) VALUES (1) ON CONFLICT (id) DO UPDATE SET id = 1;\n\
                   DO $$ BEGIN IF NOT EXISTS (SELECT 1 FROM pg_constraint) THEN\n\
                   RAISE EXCEPTION 'trades: UPDATE not allowed from here'; END IF; END $$;\n\
                   SELECT 1 FROM information_schema.columns;\n";
        let out = extract_sql_migration(src, "migrations/001_init.sql", module_id(repo), repo);
        assert_eq!(
            qnames(&out.entities),
            ["data_entity:sql:regimes", "data_entity:sql:trades"]
        );
    }
}
