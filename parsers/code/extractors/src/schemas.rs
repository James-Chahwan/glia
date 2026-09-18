//! Declared message / payload schema types (A10.5).
//!
//! A `.proto` is the contract every service that speaks it shares, and the
//! `message User {}` block is what each language's generated `User` class is
//! derived from. `grpc.rs` owns the proto `service` / `rpc` path (A5.1); this
//! module adds the `message` and `enum` declarations from the same file, as
//! `node_kind::MESSAGE_TYPE` nodes, so a polyglot stack has one node per
//! declared type to share.
//!
//! qname convention: `message:<flavor>:<qualified name>`, flavor `proto` or
//! `avro` (A10.6; `jsonschema` reserved for A10.12). Each qualified name is
//! written the way its own format writes it: protobuf's
//! `<package>.<Outer>.<Inner>`, and Avro's fullname `<namespace>.<Name>`, where
//! a nested named type inherits the enclosing namespace rather than nesting
//! under the outer record (the name Avro codegen and other schemas use).

use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, cell_type, edge_category, node_kind};
use repo_graph_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};

/// Everything the schema declarations in one file contribute to the graph.
#[derive(Default)]
pub struct SchemaNodes {
    pub nodes: Vec<Node>,
    /// Outer message / record DEFINES each nested declaration.
    pub edges: Vec<Edge>,
    pub nav: CodeNav,
    /// proto `message` / Avro `record` declarations emitted (nested included).
    pub message_count: usize,
    /// `enum` declarations emitted (nested ones included).
    pub enum_count: usize,
    /// Avro `fixed` declarations emitted. Always 0 for proto.
    pub fixed_count: usize,
    /// Cells for the file's own MODULE node: an `.avsc`'s whole-file POSITION.
    /// Empty for proto, whose module POSITION comes from `grpc.rs`.
    pub module_cells: Vec<Cell>,
}

#[derive(Debug, PartialEq)]
enum Tok {
    Ident(String),
    Open,
    Close,
    Semi,
    /// Any other punctuation or literal — only matters because it breaks a
    /// `message <Name> {` / `package <name> ;` sequence.
    Other,
}

/// Comment- and string-aware tokenizer. Returns each token with its 0-indexed
/// line. Braces inside a string literal or a comment never count, and a
/// commented-out `// message Ghost {` produces no tokens at all.
fn tokenize(source: &str) -> Vec<(Tok, u32)> {
    let b = source.as_bytes();
    let mut out = Vec::new();
    let mut line: u32 = 0;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match c {
            b'\n' => {
                line += 1;
                i += 1;
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i < b.len() && !(b[i] == b'*' && b.get(i + 1) == Some(&b'/')) {
                    if b[i] == b'\n' {
                        line += 1;
                    }
                    i += 1;
                }
                i = (i + 2).min(b.len());
            }
            b'"' | b'\'' => {
                let start = line;
                i += 1;
                while i < b.len() && b[i] != c && b[i] != b'\n' {
                    // An escape consumes the next byte too; a string never
                    // spans a raw newline, so stop before counting one twice.
                    if b[i] == b'\\' && b.get(i + 1).is_some_and(|n| *n != b'\n') {
                        i += 1;
                    }
                    i += 1;
                }
                i = (i + 1).min(b.len());
                out.push((Tok::Other, start));
            }
            b'{' => {
                out.push((Tok::Open, line));
                i += 1;
            }
            b'}' => {
                out.push((Tok::Close, line));
                i += 1;
            }
            b';' => {
                out.push((Tok::Semi, line));
                i += 1;
            }
            _ if c.is_ascii_alphabetic() || c == b'_' => {
                let start = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'.')
                {
                    i += 1;
                }
                out.push((Tok::Ident(source[start..i].to_string()), line));
            }
            _ if c.is_ascii_whitespace() => i += 1,
            _ => {
                // Skip a whole run of digits so `= 12;` is one token, and never
                // split a multi-byte UTF-8 char (only ASCII is matched above).
                if c.is_ascii_digit() {
                    while i < b.len() && b[i].is_ascii_alphanumeric() {
                        i += 1;
                    }
                } else {
                    i += 1;
                }
                out.push((Tok::Other, line));
            }
        }
    }
    out
}

/// One `message` / `enum` declaration found in a `.proto`.
#[derive(Debug, PartialEq)]
pub struct ProtoTypeDecl {
    /// Dotted path inside the file, without the package (`Outer.Inner`).
    pub local_name: String,
    /// The bare declared name (`Inner`).
    pub name: String,
    pub is_enum: bool,
    /// 0-indexed line of the `message` / `enum` keyword.
    pub start_line: u32,
    /// 0-indexed line of the matching `}`.
    pub end_line: u32,
    /// `local_name` of the enclosing message, when nested.
    pub parent: Option<String>,
}

/// The `package` and every `message` / `enum` declaration of a `.proto`, in
/// source order of their opening keyword. Parsers extract; ids come later.
pub fn parse_proto_types(source: &str) -> (Option<String>, Vec<ProtoTypeDecl>) {
    let toks = tokenize(source);
    let mut package: Option<String> = None;
    let mut decls: Vec<ProtoTypeDecl> = Vec::new();
    // One frame per open brace: `Some(index into decls)` for a message / enum
    // body, `None` for anything else (service, rpc options, oneof, extend...).
    let mut frames: Vec<Option<usize>> = Vec::new();
    let mut last_line: u32 = 0;

    let mut i = 0;
    while i < toks.len() {
        let (tok, line) = &toks[i];
        last_line = *line;
        match tok {
            Tok::Ident(kw) if kw == "package" && frames.is_empty() => {
                if let (Some((Tok::Ident(p), _)), Some((Tok::Semi, _))) = (toks.get(i + 1), toks.get(i + 2))
                    && package.is_none()
                {
                    package = Some(p.clone());
                    i += 3;
                    continue;
                }
            }
            Tok::Ident(kw) if kw == "message" || kw == "enum" => {
                if let (Some((Tok::Ident(name), _)), Some((Tok::Open, _))) = (toks.get(i + 1), toks.get(i + 2))
                    && !name.contains('.')
                {
                    // The innermost enclosing message / enum body, if the
                    // brace directly around this declaration is one.
                    let parent = frames.last().copied().flatten().map(|p| decls[p].local_name.clone());
                    let local_name = match &parent {
                        Some(p) => format!("{p}.{name}"),
                        None => name.clone(),
                    };
                    decls.push(ProtoTypeDecl {
                        local_name,
                        name: name.clone(),
                        is_enum: kw == "enum",
                        start_line: *line,
                        end_line: *line,
                        parent,
                    });
                    frames.push(Some(decls.len() - 1));
                    i += 3;
                    continue;
                }
            }
            Tok::Open => frames.push(None),
            Tok::Close => {
                if let Some(Some(d)) = frames.pop() {
                    decls[d].end_line = *line;
                }
            }
            _ => {}
        }
        i += 1;
    }
    // An unterminated body ends at the file's last token.
    for frame in frames.into_iter().flatten() {
        decls[frame].end_line = last_line;
    }
    (package, decls)
}

fn json_str(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn position_cell(path: &str, start_line: u32, end_line: u32) -> Cell {
    Cell {
        kind: cell_type::POSITION,
        payload: CellPayload::Json(format!(
            r#"{{"file":"{}","start_line":{start_line},"end_line":{end_line}}}"#,
            json_str(path)
        )),
    }
}

/// One MESSAGE_TYPE node per `message` / `enum` declared in a `.proto`,
/// parented under the file's MODULE (nested declarations under their outer
/// message, with a DEFINES edge). Each carries a POSITION cell (declaration
/// line through its closing brace) and an ORIGIN `provenance: contract` cell,
/// which also keeps `tag_synthetic_provenance` from re-tagging it.
pub fn extract_proto_messages(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
) -> SchemaNodes {
    let (package, decls) = parse_proto_types(source);
    let mut out = SchemaNodes::default();
    let mut ids: std::collections::HashMap<String, NodeId> = std::collections::HashMap::new();
    let pkg_field = match &package {
        Some(p) => format!(r#","package":"{}""#, json_str(p)),
        None => String::new(),
    };

    for d in &decls {
        let full = match &package {
            Some(p) => format!("{p}.{}", d.local_name),
            None => d.local_name.clone(),
        };
        let qname = format!("message:proto:{full}");
        // A redeclared name (invalid proto, but a scanner must not emit two
        // nodes with one id) keeps the first declaration.
        if ids.contains_key(&d.local_name) {
            continue;
        }
        let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MESSAGE_TYPE, &qname);
        ids.insert(d.local_name.clone(), id);

        let decl = if d.is_enum { "enum" } else { "message" };
        out.nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Strong,
            cells: vec![
                position_cell(path, d.start_line, d.end_line),
                Cell {
                    kind: cell_type::ORIGIN,
                    payload: CellPayload::Json(format!(
                        r#"{{"provenance":"contract","source":"proto","decl":"{decl}"{pkg_field}}}"#
                    )),
                },
            ],
        });
        let parent = d.parent.as_ref().and_then(|p| ids.get(p).copied());
        if let Some(p) = parent {
            out.edges.push(Edge {
                from: p,
                to: id,
                category: edge_category::DEFINES,
                confidence: Confidence::Strong,
            });
        }
        out.nav.record(
            id,
            &d.name,
            &qname,
            node_kind::MESSAGE_TYPE,
            Some(parent.unwrap_or(module_id)),
        );
        if d.is_enum {
            out.enum_count += 1;
        } else {
            out.message_count += 1;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Avro `.avsc` (A10.6)
// ---------------------------------------------------------------------------

/// Deepest schema nesting the Avro walk follows.
const AVRO_MAX_DEPTH: usize = 32;
/// Most named types taken from one `.avsc`.
const AVRO_MAX_TYPES: usize = 500;

/// A JSON value that keeps each object's 0-indexed line span. serde_json's
/// `Value` drops positions, and a POSITION cell needs the declaration's braces.
enum Json {
    Obj { members: Vec<(String, Json)>, start_line: u32, end_line: u32 },
    Arr(Vec<Json>),
    Str(String),
    Other,
}

impl Json {
    fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj { members, .. } => members.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
}

/// Recursive-descent reader. It runs only on input serde_json has already
/// accepted, so its one job is line tracking; a surprise still yields `None`.
struct JsonReader<'a> {
    src: &'a str,
    i: usize,
    line: u32,
}

impl JsonReader<'_> {
    fn peek(&self) -> Option<u8> {
        self.src.as_bytes().get(self.i).copied()
    }

    fn skip_ws(&mut self) {
        while let Some(c) = self.peek() {
            match c {
                b'\n' => self.line = self.line.saturating_add(1),
                b' ' | b'\t' | b'\r' => {}
                _ => return,
            }
            self.i += 1;
        }
    }

    /// At an opening quote. serde_json decodes the escapes; a JSON string never
    /// holds a raw newline, so no line is crossed.
    fn string(&mut self) -> Option<String> {
        let start = self.i;
        self.i += 1;
        while let Some(c) = self.peek() {
            self.i += 1;
            match c {
                b'\\' => self.i += 1,
                b'"' => return serde_json::from_str(self.src.get(start..self.i)?).ok(),
                _ => {}
            }
        }
        None
    }

    fn value(&mut self, depth: usize) -> Option<Json> {
        // serde_json's own recursion limit, so anything it accepted fits.
        if depth > 128 {
            return None;
        }
        self.skip_ws();
        match self.peek()? {
            b'{' => {
                let start_line = self.line;
                self.i += 1;
                let mut members = Vec::new();
                loop {
                    self.skip_ws();
                    match self.peek()? {
                        b'}' => break,
                        b',' => self.i += 1,
                        b'"' => {
                            let key = self.string()?;
                            self.skip_ws();
                            (self.peek()? == b':').then_some(())?;
                            self.i += 1;
                            members.push((key, self.value(depth + 1)?));
                        }
                        _ => return None,
                    }
                }
                self.i += 1;
                Some(Json::Obj { members, start_line, end_line: self.line })
            }
            b'[' => {
                self.i += 1;
                let mut items = Vec::new();
                loop {
                    self.skip_ws();
                    match self.peek()? {
                        b']' => break,
                        b',' => self.i += 1,
                        _ => items.push(self.value(depth + 1)?),
                    }
                }
                self.i += 1;
                Some(Json::Arr(items))
            }
            b'"' => self.string().map(Json::Str),
            _ => {
                // number / true / false / null
                let start = self.i;
                while self.peek().is_some_and(|c| !b",]} \t\r\n".contains(&c)) {
                    self.i += 1;
                }
                (self.i > start).then_some(Json::Other)
            }
        }
    }
}

/// One named Avro type (`record` / `enum` / `fixed`) declared in an `.avsc`.
struct AvroDecl {
    /// Avro fullname: `<namespace>.<Name>`, or `<Name>` in the null namespace.
    fullname: String,
    name: String,
    namespace: String,
    decl: &'static str,
    start_line: u32,
    end_line: u32,
    /// Index of the enclosing declaration, when nested.
    parent: Option<usize>,
}

/// Collect every named type under `v`, in document order. `ns` is the
/// enclosing namespace a nested name inherits. Only schema positions are
/// followed (a record's `fields[].type`, a union's members, an array's
/// `items`, a map's `values`), so a field's `default` value is never read as a
/// schema.
fn walk_avro(v: &Json, ns: &str, parent: Option<usize>, depth: usize, out: &mut Vec<AvroDecl>) {
    if depth > AVRO_MAX_DEPTH || out.len() >= AVRO_MAX_TYPES {
        return;
    }
    let (start_line, end_line) = match v {
        Json::Arr(items) => {
            for item in items {
                walk_avro(item, ns, parent, depth + 1, out);
            }
            return;
        }
        Json::Obj { start_line, end_line, .. } => (*start_line, *end_line),
        Json::Str(_) | Json::Other => return,
    };
    let ty = v.get("type");
    let decl = match ty.and_then(Json::as_str) {
        Some("record") => Some("record"),
        Some("enum") => Some("enum"),
        Some("fixed") => Some("fixed"),
        _ => None,
    };
    if let (Some(decl), Some(raw)) = (decl, v.get("name").and_then(Json::as_str)) {
        // A dotted name is already a fullname, and its namespace overrides both
        // the `namespace` attribute and the enclosing one.
        let (namespace, name) = match raw.rsplit_once('.') {
            Some((n, bare)) => (n, bare),
            None => (v.get("namespace").and_then(Json::as_str).unwrap_or(ns), raw),
        };
        if name.is_empty() {
            return;
        }
        let fullname = if namespace.is_empty() {
            name.to_string()
        } else {
            format!("{namespace}.{name}")
        };
        out.push(AvroDecl {
            fullname,
            name: name.to_string(),
            namespace: namespace.to_string(),
            decl,
            start_line,
            end_line,
            parent,
        });
        let me = Some(out.len() - 1);
        if let Some(Json::Arr(fields)) = v.get("fields") {
            for field_type in fields.iter().filter_map(|f| f.get("type")) {
                walk_avro(field_type, namespace, me, depth + 1, out);
            }
        }
        return;
    }
    // An array / map schema, or a wrapper whose `type` is itself a schema.
    for key in ["items", "values"] {
        if let Some(s) = v.get(key) {
            walk_avro(s, ns, parent, depth + 1, out);
        }
    }
    if let Some(t @ (Json::Obj { .. } | Json::Arr(_))) = ty {
        walk_avro(t, ns, parent, depth + 1, out);
    }
}

/// One MESSAGE_TYPE node per named type (`record` / `enum` / `fixed`) an Avro
/// `.avsc` declares, including those declared inline in a field's type. Same
/// shape as [`extract_proto_messages`]: parented under the file's MODULE, a
/// nested declaration under its enclosing record with a DEFINES edge, a
/// POSITION cell spanning the declaration's braces and an ORIGIN
/// `provenance: contract` cell. Malformed JSON, or a schema that is only a
/// type string (`"string"`), yields nothing.
pub fn extract_avro_records(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
) -> SchemaNodes {
    let mut out = SchemaNodes::default();
    if serde_json::from_str::<serde::de::IgnoredAny>(source).is_err() {
        return out;
    }
    let mut reader = JsonReader { src: source, i: 0, line: 0 };
    let Some(root) = reader.value(0) else {
        return out;
    };
    let mut decls = Vec::new();
    walk_avro(&root, "", None, 0, &mut decls);

    let mut seen: std::collections::HashMap<&str, NodeId> = std::collections::HashMap::new();
    // Node id per declaration index; a redeclared fullname maps to the first.
    let mut id_of: Vec<NodeId> = Vec::with_capacity(decls.len());
    for d in &decls {
        let qname = format!("message:avro:{}", d.fullname);
        let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MESSAGE_TYPE, &qname);
        id_of.push(id);
        // Avro forbids redefining a name; a scanner must still not emit two
        // nodes with one id, so the first declaration wins.
        if seen.insert(&d.fullname, id).is_some() {
            continue;
        }
        let ns_field = if d.namespace.is_empty() {
            String::new()
        } else {
            format!(r#","namespace":"{}""#, json_str(&d.namespace))
        };
        out.nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Strong,
            cells: vec![
                position_cell(path, d.start_line, d.end_line),
                Cell {
                    kind: cell_type::ORIGIN,
                    payload: CellPayload::Json(format!(
                        r#"{{"provenance":"contract","source":"avro","decl":"{}"{ns_field}}}"#,
                        d.decl
                    )),
                },
            ],
        });
        let parent = d.parent.and_then(|p| id_of.get(p).copied());
        if let Some(p) = parent {
            out.edges.push(Edge {
                from: p,
                to: id,
                category: edge_category::DEFINES,
                confidence: Confidence::Strong,
            });
        }
        out.nav.record(
            id,
            &d.name,
            &qname,
            node_kind::MESSAGE_TYPE,
            Some(parent.unwrap_or(module_id)),
        );
        match d.decl {
            "enum" => out.enum_count += 1,
            "fixed" => out.fixed_count += 1,
            _ => out.message_count += 1,
        }
    }
    if !out.nodes.is_empty() {
        let last = u32::try_from(source.lines().count().saturating_sub(1)).unwrap_or(u32::MAX);
        out.module_cells.push(position_cell(path, 0, last));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> RepoId {
        RepoId(1)
    }

    fn module_id() -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "user")
    }

    fn qnames(out: &SchemaNodes) -> Vec<String> {
        out.nodes.iter().map(|n| out.nav.qname_by_id[&n.id].clone()).collect()
    }

    fn cell(n: &Node, kind: repo_graph_core::CellTypeId) -> &str {
        match n.cells.iter().find(|c| c.kind == kind).map(|c| &c.payload) {
            Some(CellPayload::Json(j)) => j,
            other => panic!("expected a Json cell, got {other:?}"),
        }
    }

    /// The exact bytes of `bench/substrate-gap/fixtures/xcut-grpc-grpc_calls/server/user.proto`.
    const USER_PROTO: &str = include_str!(
        "../../../../bench/substrate-gap/fixtures/xcut-grpc-grpc_calls/server/user.proto"
    );

    #[test]
    fn fixture_proto_yields_its_two_messages_and_nothing_referenced_only() {
        let out = extract_proto_messages(USER_PROTO, "user.proto", module_id(), repo());
        assert_eq!(
            qnames(&out),
            vec!["message:proto:user.GetUserRequest", "message:proto:user.User"],
            "ListUsersRequest / ListUsersResponse are referenced by an rpc, never declared"
        );
        assert_eq!(out.message_count, 2);
        assert_eq!(out.enum_count, 0);
        assert!(out.edges.is_empty(), "no nesting, no DEFINES");

        let user = &out.nodes[1];
        assert_eq!(out.nav.name_by_id[&user.id], "User", "nav name is the bare name");
        assert_eq!(out.nav.kind_by_id[&user.id], node_kind::MESSAGE_TYPE);
        assert_eq!(out.nav.parent_of[&user.id], module_id());
        assert_eq!(
            cell(user, cell_type::POSITION),
            r#"{"file":"user.proto","start_line":13,"end_line":16}"#
        );
        assert_eq!(
            cell(user, cell_type::ORIGIN),
            r#"{"provenance":"contract","source":"proto","decl":"message","package":"user"}"#
        );
    }

    #[test]
    fn nested_message_is_dot_qualified_and_parented_to_its_outer_message() {
        let src = "message Outer {\n  message Inner {}\n  Inner inner = 1;\n}\n";
        let out = extract_proto_messages(src, "o.proto", module_id(), repo());
        assert_eq!(qnames(&out), vec!["message:proto:Outer", "message:proto:Outer.Inner"]);
        let (outer, inner) = (out.nodes[0].id, out.nodes[1].id);
        assert_eq!(out.nav.name_by_id[&inner], "Inner");
        assert_eq!(out.nav.parent_of[&inner], outer);
        assert_eq!(out.nav.parent_of[&outer], module_id());
        assert_eq!(out.edges.len(), 1);
        assert_eq!((out.edges[0].from, out.edges[0].to), (outer, inner));
        assert_eq!(out.edges[0].category, edge_category::DEFINES);
        assert_eq!(
            cell(&out.nodes[0], cell_type::POSITION),
            r#"{"file":"o.proto","start_line":0,"end_line":3}"#
        );
        assert_eq!(
            cell(&out.nodes[1], cell_type::POSITION),
            r#"{"file":"o.proto","start_line":1,"end_line":1}"#
        );
        // No package -> the ORIGIN omits the key rather than writing null.
        assert!(!cell(&out.nodes[0], cell_type::ORIGIN).contains("package"));
    }

    #[test]
    fn enum_is_a_message_type_marked_as_an_enum() {
        let src = "package shop.v1;\n\nenum Status {\n  STATUS_UNKNOWN = 0;\n  ACTIVE = 1;\n}\n";
        let out = extract_proto_messages(src, "s.proto", module_id(), repo());
        assert_eq!(qnames(&out), vec!["message:proto:shop.v1.Status"]);
        assert_eq!(out.enum_count, 1);
        assert_eq!(out.message_count, 0);
        assert!(cell(&out.nodes[0], cell_type::ORIGIN).contains(r#""decl":"enum""#));
        assert!(cell(&out.nodes[0], cell_type::ORIGIN).contains(r#""package":"shop.v1""#));
    }

    #[test]
    fn commented_out_and_quoted_declarations_yield_nothing() {
        let src = "// message Ghost {\n/* message Spectre {\n} */\nmessage Real {\n  string note = 1 [default = \"message Fake { }\"];\n  string message = 2;\n}\n";
        let out = extract_proto_messages(src, "g.proto", module_id(), repo());
        assert_eq!(qnames(&out), vec!["message:proto:Real"]);
        assert_eq!(
            cell(&out.nodes[0], cell_type::POSITION),
            r#"{"file":"g.proto","start_line":3,"end_line":6}"#,
            "a brace inside a string literal must not close the body early"
        );
    }

    #[test]
    fn service_rpc_bodies_oneof_and_same_line_bodies_are_handled() {
        let src = "service S {\n  rpc Get (A) returns (B) { option (x) = { message: \"m\" }; }\n}\nmessage A { oneof pick { string x = 1; int32 y = 2; } }\nmessage B{}\nmessage A {}\n";
        let out = extract_proto_messages(src, "s.proto", module_id(), repo());
        assert_eq!(
            qnames(&out),
            vec!["message:proto:A", "message:proto:B"],
            "the service is not a message, and a redeclared A keeps its first node"
        );
        assert_eq!(
            cell(&out.nodes[0], cell_type::POSITION),
            r#"{"file":"s.proto","start_line":3,"end_line":3}"#
        );
    }

    // ---- Avro (A10.6) ----

    fn avro(src: &str) -> SchemaNodes {
        extract_avro_records(src, "o.avsc", module_id(), repo())
    }

    #[test]
    fn avro_record_with_namespace_is_a_message_type() {
        let src = "{\n  \"type\": \"record\",\n  \"name\": \"OrderPlaced\",\n  \"namespace\": \"com.shop\",\n  \"fields\": [{\"name\": \"id\", \"type\": \"string\"}]\n}\n";
        let out = avro(src);
        assert_eq!(qnames(&out), vec!["message:avro:com.shop.OrderPlaced"]);
        let n = &out.nodes[0];
        assert_eq!(out.nav.name_by_id[&n.id], "OrderPlaced", "nav name is the bare name");
        assert_eq!(out.nav.kind_by_id[&n.id], node_kind::MESSAGE_TYPE);
        assert_eq!(out.nav.parent_of[&n.id], module_id());
        assert_eq!(
            cell(n, cell_type::POSITION),
            r#"{"file":"o.avsc","start_line":0,"end_line":5}"#,
            "the record object's opening brace through its closing brace"
        );
        assert_eq!(
            cell(n, cell_type::ORIGIN),
            r#"{"provenance":"contract","source":"avro","decl":"record","namespace":"com.shop"}"#
        );
        assert_eq!((out.message_count, out.enum_count, out.fixed_count), (1, 0, 0));
        assert!(out.edges.is_empty());
        assert_eq!(out.module_cells.len(), 1, "the .avsc MODULE gets a whole-file POSITION");
    }

    #[test]
    fn avro_types_nested_in_a_union_are_emitted_under_the_inherited_namespace() {
        let src = r#"{"type":"record","name":"User","namespace":"com.shop","fields":[
  {"name":"email","type":["null","string"],"default":null},
  {"name":"address","type":["null",{"type":"record","name":"Address","fields":[
    {"name":"geo","type":{"type":"fixed","name":"Geo","size":16}}]}]},
  {"name":"tags","type":{"type":"array","items":{"type":"enum","name":"Tag","symbols":["A"]}}},
  {"name":"meta","type":{"type":"record","name":"Meta","namespace":"com.other","fields":[]},
   "default":{"type":"record","name":"NotASchema"}}
]}"#;
        let out = avro(src);
        assert_eq!(
            qnames(&out),
            vec![
                "message:avro:com.shop.User",
                "message:avro:com.shop.Address",
                "message:avro:com.shop.Geo",
                "message:avro:com.shop.Tag",
                "message:avro:com.other.Meta",
            ],
            "nested names inherit the namespace, never the outer name; a default value is data"
        );
        let ids: Vec<NodeId> = out.nodes.iter().map(|n| n.id).collect();
        let (user, address, geo) = (ids[0], ids[1], ids[2]);
        assert_eq!(out.nav.parent_of[&address], user);
        assert_eq!(out.nav.parent_of[&geo], address, "parent is the innermost enclosing record");
        let defines: Vec<(NodeId, NodeId)> = out
            .edges
            .iter()
            .filter(|e| e.category == edge_category::DEFINES)
            .map(|e| (e.from, e.to))
            .collect();
        assert_eq!(defines, vec![(user, address), (address, geo), (user, ids[3]), (user, ids[4])]);
        assert_eq!(
            cell(&out.nodes[1], cell_type::POSITION),
            r#"{"file":"o.avsc","start_line":2,"end_line":3}"#
        );
        assert_eq!((out.message_count, out.enum_count, out.fixed_count), (3, 1, 1));
        assert!(cell(&out.nodes[2], cell_type::ORIGIN).contains(r#""decl":"fixed""#));
    }

    #[test]
    fn malformed_avro_yields_nothing_and_does_not_panic() {
        for src in [
            "{\"type\": \"record\", \"name\": \"Broken\",",
            "{\"type\": \"record\" \"name\": \"NoComma\"}",
            "{\"type\": \"record\", \"name\": \"Trailing\"} garbage",
            "",
            "{\"name\": \"\\u00e9\\\"x",
        ] {
            let out = avro(src);
            assert!(out.nodes.is_empty(), "{src:?}");
            assert!(out.module_cells.is_empty(), "{src:?}");
        }
    }

    #[test]
    fn a_bare_type_string_schema_yields_nothing_and_a_dotted_name_is_a_fullname() {
        assert!(avro("\"string\"").nodes.is_empty());
        assert!(avro("[\"null\", \"com.shop.User\"]").nodes.is_empty(), "references declare nothing");
        let out = avro(
            r#"[{"type":"record","name":"a.b.Rec","namespace":"ignored","fields":[
                {"name":"x","type":{"type":"enum","name":"Kind","symbols":["K"]}}]},
               {"type":"enum","name":"Plain","symbols":["P"]}]"#,
        );
        assert_eq!(
            qnames(&out),
            vec!["message:avro:a.b.Rec", "message:avro:a.b.Kind", "message:avro:Plain"],
            "a dotted name overrides `namespace`; the null namespace has no prefix"
        );
        assert_eq!(out.nav.name_by_id[&out.nodes[0].id], "Rec");
        assert!(
            !cell(&out.nodes[2], cell_type::ORIGIN).contains("namespace"),
            "no namespace -> the ORIGIN omits the key"
        );
    }

    #[test]
    fn avro_redeclared_fullname_keeps_the_first_node_and_depth_is_bounded() {
        let src = r#"{"type":"record","name":"R","fields":[
            {"name":"a","type":{"type":"record","name":"R","fields":[]}}]}"#;
        let out = avro(src);
        assert_eq!(qnames(&out), vec!["message:avro:R"]);
        assert!(out.edges.is_empty(), "a redeclaration never DEFINES itself");

        // 40 nested records: the walk stops at AVRO_MAX_DEPTH, never overflows.
        let mut deep = String::from("\"int\"");
        for i in 0..40 {
            deep = format!(r#"{{"type":"record","name":"N{i}","fields":[{{"name":"f","type":{deep}}}]}}"#);
        }
        let n = avro(&deep).nodes.len();
        assert_eq!(n, AVRO_MAX_DEPTH + 1, "records at walk depth 0..=AVRO_MAX_DEPTH");
    }

    /// The exact bytes of `bench/substrate-gap/fixtures/avro-schema/user.avsc`,
    /// so the fixture's POSITION assertions are pinned here too.
    #[test]
    fn avro_fixture_positions_match_its_key() {
        let src = include_str!("../../../../bench/substrate-gap/fixtures/avro-schema/user.avsc");
        let out = extract_avro_records(src, "user.avsc", module_id(), repo());
        assert_eq!(
            qnames(&out),
            vec![
                "message:avro:com.shop.User",
                "message:avro:com.shop.Address",
                "message:avro:com.shop.Status"
            ]
        );
        assert_eq!(
            cell(&out.nodes[0], cell_type::POSITION),
            r#"{"file":"user.avsc","start_line":0,"end_line":22}"#
        );
        assert_eq!(
            cell(&out.nodes[1], cell_type::POSITION),
            r#"{"file":"user.avsc","start_line":10,"end_line":17}"#
        );
    }
}
