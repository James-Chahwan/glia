//! Declared message / payload schema types (A10.5).
//!
//! A `.proto` is the contract every service that speaks it shares, and the
//! `message User {}` block is what each language's generated `User` class is
//! derived from. `grpc.rs` owns the proto `service` / `rpc` path (A5.1); this
//! module adds the `message` and `enum` declarations from the same file, as
//! `node_kind::MESSAGE_TYPE` nodes, so a polyglot stack has one node per
//! declared type to share.
//!
//! qname convention: `message:<flavor>:<qualified name>`, flavor `proto`,
//! `avro` (A10.6) or `jsonschema` (A10.12 / LA.16). Each qualified name is
//! written the way its own format writes it: protobuf's
//! `<package>.<Outer>.<Inner>`, and Avro's fullname `<namespace>.<Name>`, where
//! a nested named type inherits the enclosing namespace rather than nesting
//! under the outer record (the name Avro codegen and other schemas use). A
//! JSON Schema has no namespace: its root is named by `title`, `$id` or the
//! file stem, and each `$defs` / `definitions` member is `<root>.<def>`.

use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, cell_type, edge_category, node_kind};
use repo_graph_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};

/// Everything the schema declarations in one file contribute to the graph.
#[derive(Default)]
pub struct SchemaNodes {
    pub nodes: Vec<Node>,
    /// Outer message / record DEFINES each nested declaration.
    pub edges: Vec<Edge>,
    pub nav: CodeNav,
    /// proto `message` / Avro `record` declarations emitted (nested included);
    /// for a JSON Schema, the root type (0 or 1).
    pub message_count: usize,
    /// `enum` declarations emitted (nested ones included).
    pub enum_count: usize,
    /// Avro `fixed` declarations emitted. Always 0 for proto.
    pub fixed_count: usize,
    /// JSON Schema `$defs` / `definitions` members emitted. Always 0 for proto
    /// and Avro.
    pub def_count: usize,
    /// Cells for the file's own MODULE node: an `.avsc`'s or a JSON Schema's
    /// whole-file POSITION. Empty for proto, whose module POSITION comes from
    /// `grpc.rs`.
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

// ---------------------------------------------------------------------------
// JSON Schema `.json` (A10.12 / LA.16)
// ---------------------------------------------------------------------------

/// How much of a `.json` [`sniff_json_schema`] scans for its markers.
const JSON_SCHEMA_SNIFF_BYTES: usize = 8 * 1024;
/// Most MESSAGE_TYPE nodes (the root plus `$defs` members) taken from one file.
const JSON_SCHEMA_MAX_TYPES: usize = 256;
/// Root keys that make a document an API contract rather than a JSON Schema.
/// A Swagger document's `definitions` belong to the contract route
/// (`contracts.rs`), never to this one, whatever order the router checks in.
const CONTRACT_ROOT_KEYS: [&str; 3] = ["openapi", "swagger", "asyncapi"];

/// The largest char boundary at or below `at`, so a byte cap never splits a
/// multi-byte char.
fn floor_char_boundary(s: &str, at: usize) -> usize {
    let mut i = at.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// What follows each occurrence of `quoted` in KEY position (the quoted key,
/// optional whitespace, then `:`), leading whitespace trimmed. The key rule of
/// `contracts::key_pos`, kept local: this gate reads a different file family.
fn key_values<'a>(hay: &'a str, quoted: &'a str) -> impl Iterator<Item = &'a str> + 'a {
    hay.match_indices(quoted).filter_map(move |(i, _)| {
        let rest = hay.get(i + quoted.len()..)?.trim_start();
        rest.strip_prefix(':').map(str::trim_start)
    })
}

/// Cheap content gate for a `.json`: could it be a JSON Schema? No parse.
/// Within the first [`JSON_SCHEMA_SNIFF_BYTES`] of an object document, either
/// a `"$schema"` key whose string value names `json-schema.org`, or a
/// `"type": "object"` key together with a `"properties"` key.
///
/// A gate, not a validator: a data file whose NESTED object happens to look
/// like a schema is admitted, and [`extract_json_schema_types`], which reads
/// only the root and its `$defs`, then emits nothing. A false admit costs one
/// parse, never a wrong node.
pub fn sniff_json_schema(text: &str) -> bool {
    let body = text.trim_start_matches('\u{feff}');
    if !body.trim_start().starts_with('{') {
        return false; // an array, JSONL or a comment header is never a schema
    }
    let head = &body[..floor_char_boundary(body, JSON_SCHEMA_SNIFF_BYTES)];
    let names_meta_schema = key_values(head, "\"$schema\"").any(|v| {
        v.strip_prefix('"')
            .and_then(|s| s.split('"').next())
            .is_some_and(|url| url.contains("json-schema.org"))
    });
    names_meta_schema
        || (key_values(head, "\"type\"").any(|v| v.starts_with("\"object\""))
            && key_values(head, "\"properties\"").next().is_some())
}

/// A declared type name: `[A-Za-z_][A-Za-z0-9_.-]{0,127}`. A `title` such as
/// "Order created" fails it and falls through to `$id` / the file stem rather
/// than minting a qname with a space in it.
fn is_type_name(s: &str) -> bool {
    let mut bytes = s.bytes();
    bytes.next().is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
        && s.len() <= 128
        && bytes.all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
}

/// `s` without a trailing `suffix`, compared ASCII case-insensitively.
fn strip_suffix_ci<'a>(s: &'a str, suffix: &str) -> &'a str {
    s.len()
        .checked_sub(suffix.len())
        .and_then(|cut| Some((s.get(..cut)?, s.get(cut..)?)))
        .filter(|(_, tail)| tail.eq_ignore_ascii_case(suffix))
        .map_or(s, |(head, _)| head)
}

/// `order-created.schema.json` -> `order-created`: `.json`, then `.schema`.
fn schema_stem(s: &str) -> &str {
    strip_suffix_ci(strip_suffix_ci(s, ".json"), ".schema")
}

/// The root's name, first match wins: a valid `title`; else the last path
/// segment of `$id` (fragment and query dropped, `.schema.json` / `.json`
/// stripped) when it is a valid name; else the file stem, `.schema` stripped.
fn json_schema_root_name(root: &Json, path: &str) -> Option<String> {
    if let Some(title) = root.get("title").and_then(Json::as_str)
        && is_type_name(title)
    {
        return Some(title.to_string());
    }
    if let Some(id) = root.get("$id").and_then(Json::as_str) {
        let id = id.split(['#', '?']).next().unwrap_or(id).trim_end_matches('/');
        let seg = schema_stem(id.rsplit('/').next().unwrap_or(id));
        if is_type_name(seg) {
            return Some(seg.to_string());
        }
    }
    let stem = schema_stem(path.rsplit(['/', '\\']).next().unwrap_or(path));
    (!stem.is_empty()).then(|| stem.to_string())
}

/// A schema that declares an object type: `"type": "object"`, or an
/// object-valued `"properties"`.
fn is_object_type(v: &Json) -> bool {
    matches!(v, Json::Obj { .. })
        && (v.get("type").and_then(Json::as_str) == Some("object")
            || matches!(v.get("properties"), Some(Json::Obj { .. })))
}

/// POSITION (the object's own braces) + ORIGIN for one JSON Schema type.
fn json_schema_node(v: &Json, id: NodeId, repo: RepoId, path: &str, decl: &str) -> Node {
    let (start_line, end_line) = match v {
        Json::Obj { start_line, end_line, .. } => (*start_line, *end_line),
        _ => (0, 0),
    };
    // serde_json escapes control characters too, which `json_str` does not.
    let id_field = v
        .get("$id")
        .and_then(Json::as_str)
        .and_then(|s| serde_json::to_string(s).ok())
        .map(|s| format!(r#","id":{s}"#))
        .unwrap_or_default();
    Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: vec![
            position_cell(path, start_line, end_line),
            Cell {
                kind: cell_type::ORIGIN,
                payload: CellPayload::Json(format!(
                    r#"{{"provenance":"contract","source":"jsonschema","decl":"{decl}"{id_field}}}"#
                )),
            },
        ],
    }
}

/// One MESSAGE_TYPE node per object type a JSON Schema file declares: the
/// root, when it is an object schema, as `message:jsonschema:<root>`, and each
/// object-schema member of its `$defs` / `definitions` (one level, document
/// order) as `message:jsonschema:<root>.<def>`. Same shape as
/// [`extract_avro_records`]: POSITION spanning the object's braces, an ORIGIN
/// `provenance: contract` cell, the root DEFINES each def and parents it in
/// nav (a def under a non-type root is parented to the file's MODULE), and the
/// MODULE gets a whole-file POSITION. Only the root and its `$defs` are read,
/// so a data file whose nested object looks like a schema yields nothing;
/// malformed JSON and API contracts yield nothing either. `message_count`
/// counts the root, `def_count` the defs; a repeated qname keeps the first.
pub fn extract_json_schema_types(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
) -> SchemaNodes {
    let mut out = SchemaNodes::default();
    // A BOM sits before the opening brace on line 0, so trimming it moves no line.
    let body = source.trim_start_matches('\u{feff}');
    if serde_json::from_str::<serde::de::IgnoredAny>(body).is_err() {
        return out;
    }
    let mut reader = JsonReader { src: body, i: 0, line: 0 };
    let Some(root @ Json::Obj { .. }) = reader.value(0) else {
        return out;
    };
    if CONTRACT_ROOT_KEYS.iter().any(|k| root.get(k).is_some()) {
        return out;
    }
    let Some(root_name) = json_schema_root_name(&root, path) else {
        return out;
    };
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    let root_id = if is_object_type(&root) {
        let qname = format!("message:jsonschema:{root_name}");
        let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MESSAGE_TYPE, &qname);
        out.nodes.push(json_schema_node(&root, id, repo, path, "object"));
        out.nav.record(id, &root_name, &qname, node_kind::MESSAGE_TYPE, Some(module_id));
        out.message_count += 1;
        seen.insert(qname);
        Some(id)
    } else {
        None
    };

    let Json::Obj { members, .. } = &root else {
        return out;
    };
    let def_blocks = members
        .iter()
        .filter(|(k, _)| k == "$defs" || k == "definitions")
        .filter_map(|(_, v)| match v {
            Json::Obj { members, .. } => Some(members),
            _ => None,
        });
    for (name, def) in def_blocks.flatten() {
        if out.nodes.len() >= JSON_SCHEMA_MAX_TYPES {
            break;
        }
        if !is_type_name(name) || !is_object_type(def) {
            continue;
        }
        let qname = format!("message:jsonschema:{root_name}.{name}");
        if !seen.insert(qname.clone()) {
            continue;
        }
        let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MESSAGE_TYPE, &qname);
        out.nodes.push(json_schema_node(def, id, repo, path, "def"));
        if let Some(r) = root_id {
            out.edges.push(Edge {
                from: r,
                to: id,
                category: edge_category::DEFINES,
                confidence: Confidence::Strong,
            });
        }
        out.nav.record(id, name, &qname, node_kind::MESSAGE_TYPE, Some(root_id.unwrap_or(module_id)));
        out.def_count += 1;
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

    // ---- JSON Schema (A10.12 / LA.16) ----

    /// The exact bytes of the substrate fixture's shared schema, so its
    /// POSITION / ORIGIN assertions are pinned here too.
    const ORDER_CREATED: &str = include_str!(
        "../../../../bench/substrate-gap/fixtures/xschema-jsonschema-shared/orders/schemas/order-created.schema.json"
    );
    const REFUND: &str = include_str!(
        "../../../../bench/substrate-gap/fixtures/xschema-jsonschema-shared/orders/schemas/refund.json"
    );
    const SETTINGS: &str = include_str!(
        "../../../../bench/substrate-gap/fixtures/xschema-jsonschema-shared/billing/testdata/settings.json"
    );
    const FIXTURE_KEY: &str =
        include_str!("../../../../bench/substrate-gap/fixtures/xschema-jsonschema-shared/key.json");

    fn jsonschema(src: &str, path: &str) -> SchemaNodes {
        extract_json_schema_types(src, path, module_id(), repo())
    }

    #[test]
    fn sniff_json_schema_accepts_draft_and_object_shapes() {
        for (what, src) in [
            (
                "draft-07 $schema, not even an object type",
                r##"{"$schema": "http://json-schema.org/draft-07/schema#", "type": "string"}"##,
            ),
            ("2020-12 $schema", ORDER_CREATED),
            ("type object + properties, no $schema", REFUND),
            ("BOM and leading whitespace", "\u{feff}\n  {\"type\" : \"object\", \"properties\" : {}}"),
            ("escaped slashes in the meta-schema URL", r#"{"$schema":"https:\/\/json-schema.org\/draft\/2019-09\/schema"}"#),
        ] {
            assert!(sniff_json_schema(src), "{what}: {src:?}");
        }
        // The walk test's rejected candidates, the fixture's own key.json and
        // the near misses.
        for (what, src) in [
            ("openapi.json", r#"{"openapi":"3.0.3","paths":{"/users":{"get":{}}}}"#),
            ("tsconfig.json", r#"{"compilerOptions":{"paths":{"@app/*":["src/app/*"]}}}"#),
            (
                "package-lock.json",
                r#"{"name":"shop","lockfileVersion":3,"packages":{"":{"dependencies":{"swagger-ui":"5"}}}}"#,
            ),
            ("a JSON array", r#"[{"type":"object","properties":{}}]"#),
            ("the substrate key.json", FIXTURE_KEY),
            ("schemastore $schema", r#"{"$schema":"https://json.schemastore.org/tsconfig","compilerOptions":{}}"#),
            ("type object without properties", r#"{"type":"object","required":[]}"#),
            ("the keys only as values", r#"{"kind":"type","note":"properties","t":"object"}"#),
            ("type not object", r#"{"type":"module","properties":{"a":1}}"#),
            ("empty", ""),
        ] {
            assert!(!sniff_json_schema(src), "{what}: {src:?}");
        }
        // The markers past the 8 KiB head are not read, and the cut never
        // splits the multi-byte char it lands in.
        let pad = "é".repeat(JSON_SCHEMA_SNIFF_BYTES);
        // `{"pa":"` is 7 bytes, so the 8 KiB cut lands inside an `é`.
        let late = format!(r#"{{"pa":"{pad}","type":"object","properties":{{}}}}"#);
        assert!(!sniff_json_schema(&late));
    }

    #[test]
    fn json_schema_root_and_defs_are_message_types() {
        let path = "schemas/order-created.schema.json";
        let out = jsonschema(ORDER_CREATED, path);
        assert_eq!(
            qnames(&out),
            vec!["message:jsonschema:OrderCreated", "message:jsonschema:OrderCreated.Address"],
            "the root and its one $defs entry; properties are not declared types"
        );
        let (root, address) = (&out.nodes[0], &out.nodes[1]);
        assert_eq!(out.nav.name_by_id[&root.id], "OrderCreated");
        assert_eq!(out.nav.name_by_id[&address.id], "Address", "nav name is the bare def name");
        assert_eq!(out.nav.kind_by_id[&address.id], node_kind::MESSAGE_TYPE);
        assert_eq!(out.nav.parent_of[&root.id], module_id());
        assert_eq!(out.nav.parent_of[&address.id], root.id);
        assert_eq!(out.edges.len(), 1);
        assert_eq!((out.edges[0].from, out.edges[0].to), (root.id, address.id));
        assert_eq!(out.edges[0].category, edge_category::DEFINES);
        assert_eq!(
            cell(root, cell_type::POSITION),
            r#"{"file":"schemas/order-created.schema.json","start_line":0,"end_line":17}"#
        );
        assert_eq!(
            cell(address, cell_type::POSITION),
            r#"{"file":"schemas/order-created.schema.json","start_line":12,"end_line":15}"#
        );
        assert_eq!(
            cell(root, cell_type::ORIGIN),
            r#"{"provenance":"contract","source":"jsonschema","decl":"object","id":"https://schemas.example.com/order-created.schema.json"}"#
        );
        assert_eq!(
            cell(address, cell_type::ORIGIN),
            r#"{"provenance":"contract","source":"jsonschema","decl":"def"}"#,
            "a def without its own $id carries no id"
        );
        assert_eq!((out.message_count, out.def_count), (1, 1));
        assert_eq!((out.enum_count, out.fixed_count), (0, 0));
        assert_eq!(out.module_cells.len(), 1, "the schema file's MODULE gets a whole-file POSITION");
        assert!(matches!(&out.module_cells[0].payload,
            CellPayload::Json(j) if j == r#"{"file":"schemas/order-created.schema.json","start_line":0,"end_line":17}"#));

        let refund = jsonschema(REFUND, "schemas/refund.json");
        assert_eq!(qnames(&refund), vec!["message:jsonschema:RefundIssued"]);
        assert_eq!((refund.message_count, refund.def_count), (1, 0));
    }

    #[test]
    fn title_id_stem_naming_precedence() {
        let name = |src: &str, path: &str| qnames(&jsonschema(src, path));
        assert_eq!(
            name(r#"{"title":"Order","$id":"https://x/y/other.json","type":"object"}"#, "a/b.json"),
            ["message:jsonschema:Order"],
            "a valid title wins"
        );
        assert_eq!(
            name(r##"{"title":"Order placed","$id":"https://x/y/order-placed.schema.json#","type":"object"}"##, "a/b.json"),
            ["message:jsonschema:order-placed"],
            "a spaced title falls to the $id's last segment, suffix and fragment dropped"
        );
        assert_eq!(
            name(r#"{"$id":"urn:example:order","properties":{"a":{}}}"#, "schemas/Refund.Schema.JSON"),
            ["message:jsonschema:Refund"],
            "an $id with no valid segment falls to the file stem, suffixes case-insensitive"
        );
        assert_eq!(
            name(r#"{"type":"object"}"#, "schemas/user.schema.json"),
            ["message:jsonschema:user"],
            "no title, no $id: the stem without .schema"
        );
        assert_eq!(
            name(r#"{"title":"9lives","type":"object"}"#, "cat.json"),
            ["message:jsonschema:cat"],
            "a title starting with a digit is not a name"
        );
    }

    #[test]
    fn nested_lookalike_yields_nothing() {
        for (what, src) in [
            ("the fixture's settings.json", SETTINGS),
            ("an array of schemas", r#"[{"type":"object","properties":{}}]"#),
            (
                "a Swagger document's definitions are the contract route's",
                r#"{"swagger":"2.0","definitions":{"User":{"type":"object","properties":{}}}}"#,
            ),
            (
                "an OpenAPI document's component schemas",
                r#"{"openapi":"3.1.0","type":"object","properties":{},"components":{"schemas":{}}}"#,
            ),
            ("a root that is not an object type", r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"string"}"#),
            (
                "a def that is not an object schema, or not a name",
                r##"{"$defs":{"Id":{"type":"string"},"bad name":{"type":"object"},"Ref":{"$ref":"#/x"}}}"##,
            ),
        ] {
            let out = jsonschema(src, "testdata/settings.json");
            assert!(out.nodes.is_empty(), "{what}: {:?}", qnames(&out));
            assert!(out.module_cells.is_empty(), "{what}");
        }
    }

    #[test]
    fn defs_bundle_parents_to_the_module_first_wins_and_is_capped() {
        // draft-07 `definitions` and 2020-12 `$defs` in one bundle whose root
        // declares no type of its own: each def parents to the MODULE, no
        // DEFINES, and a name repeated across the two blocks keeps the first.
        let src = "{\n  \"definitions\": {\n    \"Money\": {\"type\": \"object\"}\n  },\n  \"$defs\": {\n    \"Money\": {\"properties\": {}},\n    \"Sku\": {\"properties\": {}}\n  }\n}\n";
        let out = jsonschema(src, "schemas/common.schema.json");
        assert_eq!(qnames(&out), vec!["message:jsonschema:common.Money", "message:jsonschema:common.Sku"]);
        assert!(out.edges.is_empty(), "no root type, no DEFINES");
        for n in &out.nodes {
            assert_eq!(out.nav.parent_of[&n.id], module_id());
        }
        assert_eq!(
            cell(&out.nodes[0], cell_type::POSITION),
            r#"{"file":"schemas/common.schema.json","start_line":2,"end_line":2}"#,
            "the first Money, not the redeclared one"
        );
        assert_eq!((out.message_count, out.def_count), (0, 2));

        let defs: Vec<String> = (0..300).map(|i| format!(r#""T{i}":{{"type":"object"}}"#)).collect();
        let big = format!(r#"{{"type":"object","$defs":{{{}}}}}"#, defs.join(","));
        let out = jsonschema(&big, "big.json");
        assert_eq!(out.nodes.len(), JSON_SCHEMA_MAX_TYPES, "the root plus 255 defs");
        assert_eq!((out.message_count, out.def_count), (1, JSON_SCHEMA_MAX_TYPES - 1));
    }

    #[test]
    fn malformed_json_yields_nothing_and_does_not_panic() {
        for src in [
            "{\"type\": \"object\", \"properties\": {",
            "{\"type\": \"object\" \"properties\": {}}",
            "{\"type\": \"object\", \"properties\": {}} trailing",
            "",
            "\u{feff}",
            "{\"title\": \"\\u00e9\\\"x",
            "{\"type\":\"object\",\"title\":\"é\u{1F600}\",\"properties\":{}",
        ] {
            let out = jsonschema(src, "x.json");
            assert!(out.nodes.is_empty(), "{src:?}");
            assert!(out.module_cells.is_empty(), "{src:?}");
        }
        // Control characters in a decoded $id stay valid JSON in the ORIGIN.
        let out = jsonschema(r#"{"$id":"a\nb\"c","title":"T","type":"object"}"#, "t.json");
        let origin = cell(&out.nodes[0], cell_type::ORIGIN);
        assert!(serde_json::from_str::<serde::de::IgnoredAny>(origin).is_ok(), "{origin}");
        assert!(origin.ends_with(r#","id":"a\nb\"c"}"#), "{origin}");
    }
}
