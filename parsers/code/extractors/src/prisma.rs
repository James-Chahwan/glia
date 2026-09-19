//! Prisma schema (A13.16): the `model` blocks of a `.prisma` file, which
//! declare the whole data model of a Prisma stack.
//!
//! The walk admits a `.prisma` file only through [`is_prisma_schema`] (never a
//! `detect_language` arm: the const-table scan would bind
//! `provider = "postgresql"` as a constant), and `engine::route` sends it to
//! [`extract_prisma_models`]. The Prisma DSL is block-structured and regular,
//! so a line scanner with brace tracking reads it; no tokenizer.
//!
//! Identity follows the A13.1 ORM entity identity rule
//! (`code_domain::data_entity`): `model User` mints
//! `data_entity:<flavor>:User`, keyed on the MODEL, and a declared
//! `@@map("app_users")` rides a CODE table cell `{table, orm: "prisma"}` built
//! by `table_cell`. The flavor comes from the `datasource` block's `provider`:
//! `mongodb` gives `nosql`, every relational provider gives `sql`. Each model
//! is an ACCESSES_DATA target of the schema file's MODULE.
//!
//! Out of scope: the TypeScript delegate link (`prisma.user.findMany()` ->
//! model `User`). It needs this file's model list inside the TS parse, which
//! is cross-file resolution and belongs in the graph crate, not here.

use repo_graph_code_domain::data_entity::{orm, table_cell};
use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, edge_category, node_kind};
use repo_graph_core::{Confidence, Edge, Node, NodeId, RepoId};

use crate::data_entities::DataEntityNodes;

/// Deeper nesting than this is not a Prisma schema; the scan stops there.
const MAX_DEPTH: usize = 8;

/// True for a Prisma schema file: any `<stem>.prisma`, which covers the single
/// `schema.prisma` and the `prismaSchemaFolder` multi-file layout.
pub fn is_prisma_schema(path: &str) -> bool {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    base.len() > ".prisma".len() && base.ends_with(".prisma")
}

/// One `.prisma` file's models, plus what its marker reports.
pub struct PrismaSchema {
    pub entities: DataEntityNodes,
    /// Models declared in the file.
    pub models: usize,
    /// Models carrying an `@@map` table cell.
    pub mapped: usize,
    /// The provider that chose the flavor, and where it was declared.
    pub provider: Option<String>,
    pub provider_here: bool,
}

impl PrismaSchema {
    /// The fired_on line: `[orm-prisma] models=N mapped=M provider=<p|->
    /// datasource=<here|repo|none> file=<path>`.
    pub fn marker(&self, path: &str) -> String {
        let datasource = match (&self.provider, self.provider_here) {
            (Some(_), true) => "here",
            (Some(_), false) => "repo",
            (None, _) => "none",
        };
        format!(
            "[orm-prisma] models={} mapped={} provider={} datasource={datasource} file={path}",
            self.models,
            self.mapped,
            self.provider.as_deref().unwrap_or("-"),
        )
    }
}

/// The provider every `datasource` block across `sources` agrees on, or `None`
/// when none declares one or two disagree. A `prismaSchemaFolder` schema
/// declares its datasource in one file and its models in the others; the model
/// files inherit this.
pub fn shared_provider<'a>(sources: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let mut agreed: Option<String> = None;
    for provider in sources.into_iter().filter_map(|s| scan(s).provider) {
        match &agreed {
            Some(p) if *p != provider => return None,
            _ => agreed = Some(provider),
        }
    }
    agreed
}

/// Scan one `.prisma` file. `repo_provider` ([`shared_provider`]) sets the
/// flavor when the file declares no datasource of its own.
pub fn extract_prisma_models(
    source: &str,
    module_id: NodeId,
    repo: RepoId,
    repo_provider: Option<&str>,
) -> PrismaSchema {
    let scanned = scan(source);
    let provider_here = scanned.provider.is_some();
    let provider = scanned.provider.or_else(|| repo_provider.map(str::to_owned));
    let flavor = if provider.as_deref() == Some("mongodb") { "nosql" } else { "sql" };
    let mut out = DataEntityNodes {
        nodes: Vec::new(),
        edges: Vec::new(),
        nav: CodeNav::default(),
    };
    let mut mapped = 0;
    for (model, table) in &scanned.models {
        let qname = format!("data_entity:{flavor}:{model}");
        let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::DATA_ENTITY, &qname);
        mapped += usize::from(table.is_some());
        out.nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Strong,
            cells: table.iter().map(|t| table_cell(t, orm::PRISMA)).collect(),
        });
        out.nav.record(id, model, &qname, node_kind::DATA_ENTITY, Some(module_id));
        out.edges.push(Edge {
            from: module_id,
            to: id,
            category: edge_category::ACCESSES_DATA,
            confidence: Confidence::Strong,
            cells: Vec::new(),
        });
    }
    PrismaSchema {
        models: scanned.models.len(),
        mapped,
        entities: out,
        provider,
        provider_here,
    }
}

#[derive(Default)]
struct Scan {
    /// `(model, @@map table)` in declaration order, one per model name.
    models: Vec<(String, Option<String>)>,
    provider: Option<String>,
}

enum Block {
    Model(String, Option<String>),
    Datasource,
    Other,
}

fn scan(source: &str) -> Scan {
    let mut out = Scan::default();
    let mut depth = 0usize;
    let mut block = Block::Other;
    for raw in source.lines() {
        let (code, opens, closes) = strip_line(raw);
        let code = code.trim();
        if depth == 0 && opens > 0 {
            let mut words = code.split(|c: char| c.is_whitespace() || c == '{');
            let kw = words.next().unwrap_or("");
            let name = words.find(|w| !w.is_empty()).unwrap_or("");
            block = match kw {
                "model" if is_identifier(name) => Block::Model(name.to_owned(), None),
                "datasource" => Block::Datasource,
                _ => Block::Other,
            };
        } else if depth == 1 {
            match &mut block {
                Block::Model(_, table) if table.is_none() => {
                    if let Some(args) = code.strip_prefix("@@map") {
                        *table = first_string(args).filter(|t| !t.trim().is_empty());
                    }
                }
                Block::Datasource if out.provider.is_none() => {
                    out.provider = code
                        .strip_prefix("provider")
                        .and_then(|rest| rest.trim_start().strip_prefix('='))
                        .map(str::trim_start)
                        // `provider = env("X")` names a variable, not a provider.
                        .filter(|v| v.starts_with('"') || v.starts_with('['))
                        .and_then(first_string);
                }
                _ => {}
            }
        }
        depth = (depth + opens).saturating_sub(closes);
        if depth > MAX_DEPTH {
            break;
        }
        if depth == 0 {
            close_block(&mut block, &mut out);
        }
    }
    // A truncated file still declared the model it opened.
    close_block(&mut block, &mut out);
    out
}

fn close_block(block: &mut Block, out: &mut Scan) {
    if let Block::Model(name, table) = std::mem::replace(block, Block::Other)
        && !out.models.iter().any(|(m, _)| *m == name)
    {
        out.models.push((name, table));
    }
}

/// The line before its `//` comment, and its `{` / `}` counts, both read
/// outside string literals, so `@default("{")` moves no depth.
fn strip_line(line: &str) -> (&str, usize, usize) {
    let (mut opens, mut closes) = (0, 0);
    let (mut in_str, mut escaped) = (false, false);
    let bytes = line.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if in_str {
            match b {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_str = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' => opens += 1,
            b'}' => closes += 1,
            b'/' if bytes.get(i + 1) == Some(&b'/') => return (&line[..i], opens, closes),
            _ => {}
        }
    }
    (line, opens, closes)
}

/// The contents of the first `"..."` literal in `s`, escapes resolved.
fn first_string(s: &str) -> Option<String> {
    let (_, rest) = s.split_once('"')?;
    let mut value = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(value),
            '\\' => value.push(chars.next()?),
            _ => value.push(c),
        }
    }
    None
}

fn is_identifier(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use repo_graph_code_domain::cell_type;
    use repo_graph_code_domain::data_entity::table_of;

    const SCHEMA: &str = r#"// model Draft { id Int @id }
generator client {
  provider = "prisma-client-js"
}

datasource db {
  provider = "postgresql"
  url      = env("DATABASE_URL")
}

enum Role {
  USER
  ADMIN
}

model User {
  id    Int    @id @default(autoincrement())
  email String @unique @default("{")
  @@index([email])
  @@map("app_users")
}

model Order {
  id     Int @id
  userId Int @map("user_id")
}
"#;

    fn module() -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, RepoId(1), node_kind::MODULE, "prisma::schema")
    }

    fn entity(qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, RepoId(1), node_kind::DATA_ENTITY, qname)
    }

    fn names(out: &PrismaSchema) -> Vec<&str> {
        out.entities
            .nodes
            .iter()
            .filter_map(|n| out.entities.nav.qname_by_id.get(&n.id).map(String::as_str))
            .collect()
    }

    #[test]
    fn prisma_model_emits_entity() {
        let out = extract_prisma_models(SCHEMA, module(), RepoId(1), None);
        assert_eq!(names(&out), ["data_entity:sql:User", "data_entity:sql:Order"]);
        assert_eq!((out.models, out.mapped), (2, 1));
        let order = entity("data_entity:sql:Order");
        let edge = out.entities.edges.iter().find(|e| e.to == order).expect("Order edge");
        assert_eq!(edge.from, module());
        assert_eq!(edge.category, edge_category::ACCESSES_DATA);
        assert_eq!(out.entities.nav.name_by_id[&order], "Order");
        assert_eq!(
            out.marker("prisma/schema.prisma"),
            "[orm-prisma] models=2 mapped=1 provider=postgresql datasource=here \
             file=prisma/schema.prisma"
        );
    }

    #[test]
    fn prisma_map_attribute_overrides_model_name() {
        let out = extract_prisma_models(SCHEMA, module(), RepoId(1), None);
        let user = &out.entities.nodes[0];
        assert_eq!(user.id, entity("data_entity:sql:User"), "keyed on the model, not the table");
        assert_eq!(user.cells.len(), 1);
        assert_eq!(user.cells[0].kind, cell_type::CODE);
        assert_eq!(table_of(&user.cells), Some("app_users".to_string()));
        // A field-level `@map` names a column: Order carries no table cell.
        assert!(out.entities.nodes[1].cells.is_empty());
        let named = extract_prisma_models(
            "model A {\n  id Int @id\n  @@map(name: \"a_rows\")\n}\n",
            module(),
            RepoId(1),
            None,
        );
        assert_eq!(table_of(&named.entities.nodes[0].cells), Some("a_rows".to_string()));
    }

    #[test]
    fn prisma_mongodb_provider_uses_nosql_flavor() {
        let src = "datasource db {\n  provider = \"mongodb\"\n}\nmodel Event {\n  id String @id\n}\n";
        let out = extract_prisma_models(src, module(), RepoId(1), None);
        assert_eq!(names(&out), ["data_entity:nosql:Event"]);
        // A prismaSchemaFolder model file inherits the repo's one provider;
        // its own datasource wins over the repo's.
        let model_only = "model Event {\n  id String @id\n}\n";
        assert_eq!(shared_provider([src, model_only]), Some("mongodb".to_string()));
        let out = extract_prisma_models(model_only, module(), RepoId(1), Some("mongodb"));
        assert_eq!(names(&out), ["data_entity:nosql:Event"]);
        assert!(out.marker("m.prisma").contains("provider=mongodb datasource=repo"));
        let out = extract_prisma_models(SCHEMA, module(), RepoId(1), Some("mongodb"));
        assert_eq!(names(&out)[0], "data_entity:sql:User");
        // Two schemas that disagree leave the model file to the sql default.
        assert_eq!(shared_provider([src, SCHEMA, model_only]), None);
        // `env(...)` names a variable, not a provider.
        let env = "datasource db {\n  provider = env(\"P\")\n}\n";
        assert_eq!(shared_provider([env]), None);
    }

    #[test]
    fn is_prisma_schema_rejects_other_files() {
        assert!(is_prisma_schema("prisma/schema.prisma"));
        assert!(is_prisma_schema("schema.prisma"));
        assert!(is_prisma_schema("prisma/schema/user.prisma"));
        assert!(!is_prisma_schema("prisma/.prisma"));
        assert!(!is_prisma_schema("prisma/schema.prisma.bak"));
        assert!(!is_prisma_schema("prisma/schema.ts"));
        assert!(!is_prisma_schema("node_modules/.prisma/client/index.js"));
        assert!(!is_prisma_schema("prisma/migrations/20240101_init/migration.sql"));
    }

    #[test]
    fn prisma_scan_skips_non_models_and_stays_bounded() {
        let out = extract_prisma_models(SCHEMA, module(), RepoId(1), None);
        for absent in ["Role", "Draft", "client", "db", "app_users", "user_id"] {
            assert!(!names(&out).iter().any(|q| q.ends_with(&format!(":{absent}"))), "{absent}");
        }
        let deep = format!("model Deep {}\n", "{".repeat(20));
        let out = extract_prisma_models(&deep, module(), RepoId(1), None);
        assert_eq!(out.models, 1, "the header is still a declaration");
        let one_line = extract_prisma_models("model Tag { id Int @id }\n", module(), RepoId(1), None);
        assert_eq!(names(&one_line), ["data_entity:sql:Tag"]);
    }
}
