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
    /// LE.10a: proto messages / Avro records given a SCHEMA_FIELDS cell.
    /// Always 0 for a JSON Schema.
    pub schema_field_cells: usize,
    /// LE.10a: fields listed across those cells.
    pub schema_fields: usize,
}

/// The declaration scan only matches `Ident` / `Open` / `Close` / `Semi`;
/// every other token breaks a `message <Name> {` / `package <name> ;`
/// sequence. The field scan (LE.10a) also reads the literal and punctuation
/// tokens, so each keeps its text.
#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Open,
    Close,
    Semi,
    /// A string literal's text between its quotes, escapes as written.
    Str(String),
    /// A run of digits and the alphanumerics that follow (`12`, `0x1F`).
    Num(String),
    /// Any other byte: punctuation (`=`, `<`, `>`, `,`, `[`, `.`) or one byte
    /// of a non-ASCII char.
    Sym(u8),
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
                let text_start = i;
                while i < b.len() && b[i] != c && b[i] != b'\n' {
                    // An escape consumes the next byte too; a string never
                    // spans a raw newline, so stop before counting one twice.
                    if b[i] == b'\\' && b.get(i + 1).is_some_and(|n| *n != b'\n') {
                        i += 1;
                    }
                    i += 1;
                }
                // `i` sits on an ASCII quote / newline or at the end, so the
                // slice never splits a char.
                let text = source.get(text_start..i.min(b.len())).unwrap_or_default();
                out.push((Tok::Str(text.to_string()), start));
                // Consume the closing quote only: an unterminated string's
                // newline is left for the `\n` arm to count.
                if i < b.len() && b[i] == c {
                    i += 1;
                }
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
            _ if c.is_ascii_digit() => {
                // A whole run of digits is one token, so `= 12;` reads as a
                // number; only ASCII is consumed, so no char is split.
                let start = i;
                while i < b.len() && b[i].is_ascii_alphanumeric() {
                    i += 1;
                }
                out.push((Tok::Num(source.get(start..i).unwrap_or_default().to_string()), line));
            }
            _ => {
                out.push((Tok::Sym(c), line));
                i += 1;
            }
        }
    }
    out
}

/// Most fields (and, separately, reserved entries) one message / record's
/// SCHEMA_FIELDS cell lists; past it the cell says `"truncated":true`.
const SCHEMA_MAX_FIELDS: usize = 500;

/// One field declared directly in a proto `message` body, or in a `oneof`
/// inside it (LE.10a).
#[derive(Debug, PartialEq)]
pub struct ProtoField {
    pub name: String,
    /// The type as written minus a leading dot (`google.protobuf.Timestamp`,
    /// `shop.v1.Money`); a map is `map<K,V>` with the whitespace removed.
    pub type_name: String,
    /// The field number; `None` only when the literal does not parse.
    pub number: Option<u64>,
    /// `repeated` / `optional` / `required`, when written.
    pub label: Option<String>,
    /// The enclosing `oneof`'s name.
    pub oneof: Option<String>,
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
    /// A message's own fields in declaration order, `oneof` members included,
    /// capped at [`SCHEMA_MAX_FIELDS`]. A nested message's fields are its own;
    /// `option`, `extensions`, `extend` bodies and proto2 `group` bodies are
    /// not fields of the message. Always empty for an enum.
    pub fields: Vec<ProtoField>,
    /// `reserved` numbers, ranges (`9 to 11`, `40 to max`) and names, in
    /// declaration order, numbers written in decimal.
    pub reserved: Vec<String>,
    /// A cap cut `fields` or `reserved` short.
    pub truncated: bool,
}

/// One open brace in [`parse_proto_types`].
enum Frame {
    /// A `message` / `enum` body: an index into the decls.
    Decl(usize),
    /// A `oneof <name>` body directly inside message body `msg`.
    Oneof { msg: usize, name: String },
    /// Any other brace: a service, rpc options, `extend`, a proto2 `group`, an
    /// option's aggregate value.
    Other,
}

/// Statement keywords that can never open a field, even where the rest of
/// the statement reads like one (`option foo = 1;`).
const PROTO_NON_FIELD_KEYWORDS: [&str; 8] =
    ["option", "extensions", "extend", "reserved", "oneof", "message", "enum", "group"];

/// A proto integer literal: decimal, `0x` hex or leading-`0` octal.
fn parse_proto_int(s: &str) -> Option<u64> {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).ok()
    } else if s.len() > 1 && s.starts_with('0') {
        u64::from_str_radix(s.get(1..)?, 8).ok()
    } else {
        s.parse().ok()
    }
}

/// A number as decimal text, or as written when it does not parse.
fn proto_num_text(s: &str) -> String {
    parse_proto_int(s).map_or_else(|| s.to_string(), |n| n.to_string())
}

/// One named type at `toks[i]`: `Name` or `.pkg.Name` (dot dropped).
/// Returns it and the index after it.
fn proto_named_type(toks: &[Tok], i: usize) -> Option<(String, usize)> {
    let i = if toks.get(i) == Some(&Tok::Sym(b'.')) { i + 1 } else { i };
    let Tok::Ident(t) = toks.get(i)? else {
        return None;
    };
    Some((t.clone(), i + 1))
}

/// A field's type at `toks[i]`: a named type, or `map < K , V >`. A map's
/// key and value are never maps themselves (proto forbids it), so this does
/// not recurse. Returns the rendered type and the index after it.
fn proto_type(toks: &[Tok], i: usize) -> Option<(String, usize)> {
    let (t, next) = proto_named_type(toks, i)?;
    if t != "map" || toks.get(next) != Some(&Tok::Sym(b'<')) {
        return Some((t, next));
    }
    let (k, after_k) = proto_named_type(toks, next + 1)?;
    (toks.get(after_k)? == &Tok::Sym(b',')).then_some(())?;
    let (v, after_v) = proto_named_type(toks, after_k + 1)?;
    (toks.get(after_v)? == &Tok::Sym(b'>')).then_some(())?;
    Some((format!("map<{k},{v}>"), after_v + 1))
}

/// `[label] <type> <name> = <number> ...` (options and anything after the
/// number are ignored), else `None`.
fn proto_field(stmt: &[Tok], oneof: Option<&str>) -> Option<ProtoField> {
    let label = match stmt.first()? {
        Tok::Ident(l) if matches!(l.as_str(), "repeated" | "optional" | "required") => Some(l.clone()),
        _ => None,
    };
    let at = usize::from(label.is_some());
    if let Some(Tok::Ident(kw)) = stmt.get(at)
        && PROTO_NON_FIELD_KEYWORDS.contains(&kw.as_str())
    {
        return None;
    }
    let (type_name, next) = proto_type(stmt, at)?;
    let Tok::Ident(name) = stmt.get(next)? else {
        return None;
    };
    if name.contains('.') || stmt.get(next + 1)? != &Tok::Sym(b'=') {
        return None;
    }
    let Tok::Num(number) = stmt.get(next + 2)? else {
        return None;
    };
    Some(ProtoField {
        name: name.clone(),
        type_name,
        number: parse_proto_int(number),
        label,
        oneof: oneof.map(str::to_string),
    })
}

/// The entries of a `reserved` statement (after the keyword): numbers,
/// `a to b` / `a to max` ranges and names, quoted (proto2 / proto3) or bare
/// (editions). An entry of any other shape is skipped.
fn proto_reserved(rest: &[Tok]) -> Vec<String> {
    rest.split(|t| *t == Tok::Sym(b','))
        .filter_map(|entry| match entry {
            [Tok::Str(s)] | [Tok::Ident(s)] => Some(s.clone()),
            [Tok::Num(n)] => Some(proto_num_text(n)),
            [Tok::Num(a), Tok::Ident(to), Tok::Num(b)] if to == "to" => {
                Some(format!("{} to {}", proto_num_text(a), proto_num_text(b)))
            }
            [Tok::Num(a), Tok::Ident(to), Tok::Ident(max)] if to == "to" && max == "max" => {
                Some(format!("{} to max", proto_num_text(a)))
            }
            _ => None,
        })
        .collect()
}

/// Fold one `;`-terminated statement of a message (or `oneof`) body into its
/// declaration: a field, a `reserved` list, or nothing.
fn record_proto_statement(stmt: &[Tok], oneof: Option<&str>, d: &mut ProtoTypeDecl) {
    if let [Tok::Ident(kw), rest @ ..] = stmt
        && kw == "reserved"
    {
        if oneof.is_none() {
            for r in proto_reserved(rest) {
                if d.reserved.len() >= SCHEMA_MAX_FIELDS {
                    d.truncated = true;
                    break;
                }
                d.reserved.push(r);
            }
        }
        return;
    }
    if let Some(f) = proto_field(stmt, oneof) {
        if d.fields.len() >= SCHEMA_MAX_FIELDS {
            d.truncated = true;
        } else {
            d.fields.push(f);
        }
    }
}

/// The `package` and every `message` / `enum` declaration of a `.proto`, in
/// source order of their opening keyword, each message with its fields and
/// reserved entries (LE.10a). Parsers extract; ids come later.
pub fn parse_proto_types(source: &str) -> (Option<String>, Vec<ProtoTypeDecl>) {
    let toks = tokenize(source);
    let mut package: Option<String> = None;
    let mut decls: Vec<ProtoTypeDecl> = Vec::new();
    // One frame per open brace.
    let mut frames: Vec<Frame> = Vec::new();
    // The tokens of the statement being read in a message / oneof body.
    let mut stmt: Vec<Tok> = Vec::new();
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
                    let parent = match frames.last() {
                        Some(Frame::Decl(p)) => Some(decls[*p].local_name.clone()),
                        _ => None,
                    };
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
                        fields: Vec::new(),
                        reserved: Vec::new(),
                        truncated: false,
                    });
                    frames.push(Frame::Decl(decls.len() - 1));
                    stmt.clear();
                    i += 3;
                    continue;
                }
            }
            _ => {}
        }
        // A token read into a statement belongs to this message body, and to
        // this oneof when inside one.
        let target = match frames.last() {
            Some(Frame::Decl(d)) if !decls[*d].is_enum => Some((*d, None)),
            Some(Frame::Oneof { msg, name }) => Some((*msg, Some(name.as_str()))),
            _ => None,
        };
        match tok {
            Tok::Open => {
                let frame = match (frames.last(), stmt.as_slice()) {
                    (Some(Frame::Decl(msg)), [Tok::Ident(kw), Tok::Ident(name)])
                        if target.is_some() && kw == "oneof" && !name.contains('.') =>
                    {
                        Frame::Oneof { msg: *msg, name: name.clone() }
                    }
                    _ => Frame::Other,
                };
                // An option's aggregate value (`= {`) is part of its statement;
                // any other brace in a message body opens a block that ends it.
                if target.is_some() && !matches!(stmt.last(), Some(Tok::Sym(b'=' | b':' | b',' | b'['))) {
                    stmt.clear();
                }
                frames.push(frame);
            }
            Tok::Close => match frames.pop() {
                Some(Frame::Decl(d)) => {
                    decls[d].end_line = *line;
                    stmt.clear();
                }
                Some(Frame::Oneof { .. }) => stmt.clear(),
                Some(Frame::Other) | None => {}
            },
            Tok::Semi => {
                if let Some((msg, oneof)) = target {
                    record_proto_statement(&stmt, oneof, &mut decls[msg]);
                    stmt.clear();
                }
            }
            t => {
                if target.is_some() {
                    stmt.push(t.clone());
                }
            }
        }
        i += 1;
    }
    // An unterminated body ends at the file's last token.
    for frame in frames {
        if let Frame::Decl(d) = frame {
            decls[d].end_line = last_line;
        }
    }
    (package, decls)
}

fn json_str(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// `s` as a JSON string literal, quotes included and control characters
/// escaped (which [`json_str`] does not do).
fn json_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| String::from("\"\""))
}

/// The SCHEMA_FIELDS cell (LE.10a): `{"format":<format>,"fields":[...]}` with
/// the pre-rendered field objects in declaration order, then
/// `"reserved":[...]` when there are any, then `"truncated":true` when a cap
/// cut a list short. LE.10b writes the same shape for contract ops, so LE.10c
/// compares one shape.
fn schema_fields_cell(format: &str, fields: &[String], reserved: &[String], truncated: bool) -> Cell {
    let mut j = format!(r#"{{"format":"{format}","fields":[{}]"#, fields.join(","));
    if !reserved.is_empty() {
        let reserved: Vec<String> = reserved.iter().map(|r| json_string(r)).collect();
        j.push_str(&format!(r#","reserved":[{}]"#, reserved.join(",")));
    }
    if truncated {
        j.push_str(r#","truncated":true"#);
    }
    j.push('}');
    Cell { kind: cell_type::SCHEMA_FIELDS, payload: CellPayload::Json(j) }
}

/// `{"name":..,"type":..,"number":N,"label":..,"oneof":..}`, each optional
/// key only when present.
fn proto_field_json(f: &ProtoField) -> String {
    let mut j = format!(r#"{{"name":{},"type":{}"#, json_string(&f.name), json_string(&f.type_name));
    if let Some(n) = f.number {
        j.push_str(&format!(r#","number":{n}"#));
    }
    if let Some(label) = &f.label {
        j.push_str(&format!(r#","label":{}"#, json_string(label)));
    }
    if let Some(oneof) = &f.oneof {
        j.push_str(&format!(r#","oneof":{}"#, json_string(oneof)));
    }
    j.push('}');
    j
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
/// which also keeps `tag_synthetic_provenance` from re-tagging it. A message
/// (never an enum) also carries a SCHEMA_FIELDS cell listing its own fields
/// and reserved entries (LE.10a), empty `fields` included.
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
        let mut cells = vec![
            position_cell(path, d.start_line, d.end_line),
            Cell {
                kind: cell_type::ORIGIN,
                payload: CellPayload::Json(format!(
                    r#"{{"provenance":"contract","source":"proto","decl":"{decl}"{pkg_field}}}"#
                )),
            },
        ];
        if !d.is_enum {
            let fields: Vec<String> = d.fields.iter().map(proto_field_json).collect();
            cells.push(schema_fields_cell("proto", &fields, &d.reserved, d.truncated));
            out.schema_field_cells += 1;
            out.schema_fields += fields.len();
        }
        out.nodes.push(Node { id, repo, confidence: Confidence::Strong, cells });
        let parent = d.parent.as_ref().and_then(|p| ids.get(p).copied());
        if let Some(p) = parent {
            out.edges.push(Edge {
                from: p,
                to: id,
                category: edge_category::DEFINES,
                confidence: Confidence::Strong,
                cells: Vec::new(),
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

/// One field of an Avro record (LE.10a).
struct AvroField {
    name: String,
    /// The type rendered by [`avro_type`]; `None` when it is not a schema.
    type_name: Option<String>,
    /// The field declares a `default` (its value is not kept).
    has_default: bool,
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
    /// A record's `fields`, in declaration order, capped at
    /// [`SCHEMA_MAX_FIELDS`] (the bool: the cap cut them short). `None` for an
    /// enum or a fixed.
    fields: Option<(Vec<AvroField>, bool)>,
}

/// Avro's primitive type names; any other bare type string names a type.
const AVRO_PRIMITIVES: [&str; 8] = ["null", "boolean", "int", "long", "float", "double", "bytes", "string"];

/// A named type's `(namespace, bare name)`. A dotted `name` is already a
/// fullname, and its namespace overrides both the `namespace` attribute and
/// the enclosing `ns`. `None` without a non-empty name.
fn avro_name<'a>(v: &'a Json, ns: &'a str) -> Option<(&'a str, &'a str)> {
    let raw = v.get("name").and_then(Json::as_str)?;
    let (namespace, name) = match raw.rsplit_once('.') {
        Some((n, bare)) => (n, bare),
        None => (v.get("namespace").and_then(Json::as_str).unwrap_or(ns), raw),
    };
    (!name.is_empty()).then_some((namespace, name))
}

/// `<namespace>.<name>`, or `<name>` in the null namespace.
fn avro_fullname(namespace: &str, name: &str) -> String {
    if namespace.is_empty() {
        name.to_string()
    } else {
        format!("{namespace}.{name}")
    }
}

/// A type string: a primitive as is, a named-type reference as its fullname
/// (a bare name resolves in the enclosing namespace `ns`).
fn avro_type_ref(s: &str, ns: &str) -> String {
    if AVRO_PRIMITIVES.contains(&s) || s.contains('.') {
        s.to_string()
    } else {
        avro_fullname(ns, s)
    }
}

/// A field's type, rendered canonically: a primitive by name; a union as its
/// members joined by `|` in declared order (`null|string`); `array<T>`;
/// `map<T>`; a named or inline record / enum / fixed as its fullname; a
/// `logicalType` appended in parentheses (`long(timestamp-millis)`). `None`
/// when a position holds no schema, or past [`AVRO_MAX_DEPTH`].
fn avro_type(v: &Json, ns: &str, depth: usize) -> Option<String> {
    if depth > AVRO_MAX_DEPTH {
        return None;
    }
    match v {
        Json::Str(s) => Some(avro_type_ref(s, ns)),
        Json::Arr(members) => {
            let members: Option<Vec<String>> = members.iter().map(|m| avro_type(m, ns, depth + 1)).collect();
            Some(members?.join("|"))
        }
        Json::Obj { .. } => {
            let base = match v.get("type")? {
                Json::Str(t) => match t.as_str() {
                    "record" | "enum" | "fixed" => {
                        let (namespace, name) = avro_name(v, ns)?;
                        avro_fullname(namespace, name)
                    }
                    "array" => format!("array<{}>", avro_type(v.get("items")?, ns, depth + 1)?),
                    "map" => format!("map<{}>", avro_type(v.get("values")?, ns, depth + 1)?),
                    other => avro_type_ref(other, ns),
                },
                inner @ (Json::Obj { .. } | Json::Arr(_)) => avro_type(inner, ns, depth + 1)?,
                Json::Other => return None,
            };
            Some(match v.get("logicalType").and_then(Json::as_str) {
                Some(logical) => format!("{base}({logical})"),
                None => base,
            })
        }
        Json::Other => None,
    }
}

/// A record's named `fields`, `ns` being the record's own namespace (the one
/// its fields' nested names inherit). The bool: the cap cut the list short.
fn avro_fields(fields: Option<&Json>, ns: &str, depth: usize) -> (Vec<AvroField>, bool) {
    let Some(Json::Arr(items)) = fields else {
        return (Vec::new(), false);
    };
    let mut out = Vec::new();
    for f in items {
        let Some(name) = f.get("name").and_then(Json::as_str) else {
            continue;
        };
        if out.len() >= SCHEMA_MAX_FIELDS {
            return (out, true);
        }
        out.push(AvroField {
            name: name.to_string(),
            type_name: f.get("type").and_then(|t| avro_type(t, ns, depth + 1)),
            has_default: f.get("default").is_some(),
        });
    }
    (out, false)
}

/// `{"name":..,"type":..,"default":true}`, each optional key only when present.
fn avro_field_json(f: &AvroField) -> String {
    let mut j = format!(r#"{{"name":{}"#, json_string(&f.name));
    if let Some(t) = &f.type_name {
        j.push_str(&format!(r#","type":{}"#, json_string(t)));
    }
    if f.has_default {
        j.push_str(r#","default":true"#);
    }
    j.push('}');
    j
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
    if let (Some(decl), Some(_)) = (decl, v.get("name").and_then(Json::as_str)) {
        let Some((namespace, name)) = avro_name(v, ns) else {
            return;
        };
        out.push(AvroDecl {
            fullname: avro_fullname(namespace, name),
            name: name.to_string(),
            namespace: namespace.to_string(),
            decl,
            start_line,
            end_line,
            parent,
            fields: (decl == "record").then(|| avro_fields(v.get("fields"), namespace, depth)),
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
/// `provenance: contract` cell; a record also carries a SCHEMA_FIELDS cell
/// listing its fields (LE.10a). Malformed JSON, or a schema that is only a
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
        let mut cells = vec![
            position_cell(path, d.start_line, d.end_line),
            Cell {
                kind: cell_type::ORIGIN,
                payload: CellPayload::Json(format!(
                    r#"{{"provenance":"contract","source":"avro","decl":"{}"{ns_field}}}"#,
                    d.decl
                )),
            },
        ];
        if let Some((fields, truncated)) = &d.fields {
            let fields: Vec<String> = fields.iter().map(avro_field_json).collect();
            cells.push(schema_fields_cell("avro", &fields, &[], *truncated));
            out.schema_field_cells += 1;
            out.schema_fields += fields.len();
        }
        out.nodes.push(Node { id, repo, confidence: Confidence::Strong, cells });
        let parent = d.parent.and_then(|p| id_of.get(p).copied());
        if let Some(p) = parent {
            out.edges.push(Edge {
                from: p,
                to: id,
                category: edge_category::DEFINES,
                confidence: Confidence::Strong,
                cells: Vec::new(),
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
                cells: Vec::new(),
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

    // ---- SCHEMA_FIELDS on proto messages (LE.10a) ----

    fn fields_of<'a>(out: &'a SchemaNodes, qname: &str) -> Option<&'a str> {
        let n = out.nodes.iter().find(|n| out.nav.qname_by_id[&n.id] == qname)?;
        n.cells.iter().find(|c| c.kind == cell_type::SCHEMA_FIELDS).map(|c| match &c.payload {
            CellPayload::Json(j) => j.as_str(),
            other => panic!("expected a Json cell, got {other:?}"),
        })
    }

    #[test]
    fn proto_scalar_repeated_map_oneof_fields() {
        let src = "syntax = \"proto3\";\npackage shop.v1;\nimport \"google/protobuf/timestamp.proto\";\n\nmessage Order {\n  option deprecated = true;\n  int64 total_cents = 2;\n  repeated string sku = 3;\n  map<string, string> labels = 4;\n  oneof payer {\n    string card_token = 5;\n    .shop.v1.Money wallet = 6 [deprecated = true];\n  }\n  google.protobuf.Timestamp at = 0x10;\n  optional string note = 017 [(validate.rules).string = { min_len: 1 }];\n  map<string, .shop.v1.Money> balances = 9;\n  extensions 100 to 199;\n  option foo = 1;\n}\n";
        let out = extract_proto_messages(src, "o.proto", module_id(), repo());
        assert_eq!(
            fields_of(&out, "message:proto:shop.v1.Order"),
            Some(concat!(
                r#"{"format":"proto","fields":["#,
                r#"{"name":"total_cents","type":"int64","number":2},"#,
                r#"{"name":"sku","type":"string","number":3,"label":"repeated"},"#,
                r#"{"name":"labels","type":"map<string,string>","number":4},"#,
                r#"{"name":"card_token","type":"string","number":5,"oneof":"payer"},"#,
                r#"{"name":"wallet","type":"shop.v1.Money","number":6,"oneof":"payer"},"#,
                r#"{"name":"at","type":"google.protobuf.Timestamp","number":16},"#,
                r#"{"name":"note","type":"string","number":15,"label":"optional"},"#,
                r#"{"name":"balances","type":"map<string,shop.v1.Money>","number":9}"#,
                r#"]}"#
            )),
            "declaration order; oneof members carry it; a leading dot is dropped; hex / octal numbers \
             decoded; an option's aggregate value stays inside its field; option / extensions are not fields"
        );
        assert_eq!((out.schema_field_cells, out.schema_fields), (1, 8));
        // The POSITION / ORIGIN cells are unchanged, SCHEMA_FIELDS comes last.
        let kinds: Vec<_> = out.nodes[0].cells.iter().map(|c| c.kind).collect();
        assert_eq!(kinds, vec![cell_type::POSITION, cell_type::ORIGIN, cell_type::SCHEMA_FIELDS]);
    }

    #[test]
    fn proto_reserved_captured() {
        let src = "message M {\n  reserved 2, 15, 9 to 11, 40 to max;\n  reserved \"foo\", \"bar\";\n  reserved baz;\n  string keep = 1;\n  reserved 0x10;\n}\nmessage Empty {}\nenum E { E_A = 0; reserved 1; }\n";
        let out = extract_proto_messages(src, "r.proto", module_id(), repo());
        assert_eq!(
            fields_of(&out, "message:proto:M"),
            Some(concat!(
                r#"{"format":"proto","fields":[{"name":"keep","type":"string","number":1}],"#,
                r#""reserved":["2","15","9 to 11","40 to max","foo","bar","baz","16"]}"#
            )),
            "numbers (decimal), ranges and quoted / bare names, in declaration order"
        );
        assert_eq!(
            fields_of(&out, "message:proto:Empty"),
            Some(r#"{"format":"proto","fields":[]}"#),
            "a message with no fields says so; no reserved key when there is none"
        );
        assert_eq!(fields_of(&out, "message:proto:E"), None, "an enum declares values, not fields");
        assert_eq!((out.schema_field_cells, out.schema_fields), (2, 1));
    }

    #[test]
    fn proto_nested_message_fields_stay_nested() {
        let src = "message Outer {\n  string id = 1;\n  message Inner {\n    int32 depth = 1;\n    enum Kind { KIND_A = 0; }\n    Kind kind = 2;\n  }\n  Inner inner = 2;\n  extend Base { int32 ext = 100; }\n  repeated group Legacy = 3 { optional int32 old = 4; }\n  int64 after = 5;\n}\n";
        let out = extract_proto_messages(src, "n.proto", module_id(), repo());
        assert_eq!(
            qnames(&out),
            vec!["message:proto:Outer", "message:proto:Outer.Inner", "message:proto:Outer.Inner.Kind"]
        );
        assert_eq!(
            fields_of(&out, "message:proto:Outer"),
            Some(concat!(
                r#"{"format":"proto","fields":["#,
                r#"{"name":"id","type":"string","number":1},"#,
                r#"{"name":"inner","type":"Inner","number":2},"#,
                r#"{"name":"after","type":"int64","number":5}]}"#
            )),
            "the nested message's fields, an extend body and a group body are not Outer's"
        );
        assert_eq!(
            fields_of(&out, "message:proto:Outer.Inner"),
            Some(concat!(
                r#"{"format":"proto","fields":["#,
                r#"{"name":"depth","type":"int32","number":1},"#,
                r#"{"name":"kind","type":"Kind","number":2}]}"#
            ))
        );
        assert_eq!(fields_of(&out, "message:proto:Outer.Inner.Kind"), None);
    }

    #[test]
    fn proto_comment_and_string_braces_ignored() {
        let src = "message Real {\n  // string ghost = 9;\n  /* int32 spectre = 10; } */\n  string note = 1 [default = \"a } b; int32 fake = 3;\"];\n  string message = 2;\n  string apostrophe = 3 [json_name = 'it\\'s'];\n}\n";
        let out = extract_proto_messages(src, "c.proto", module_id(), repo());
        assert_eq!(qnames(&out), vec!["message:proto:Real"]);
        assert_eq!(
            fields_of(&out, "message:proto:Real"),
            Some(concat!(
                r#"{"format":"proto","fields":["#,
                r#"{"name":"note","type":"string","number":1},"#,
                r#"{"name":"message","type":"string","number":2},"#,
                r#"{"name":"apostrophe","type":"string","number":3}]}"#
            )),
            "commented-out fields and braces / semicolons inside strings are not fields"
        );
        assert_eq!(
            cell(&out.nodes[0], cell_type::POSITION),
            r#"{"file":"c.proto","start_line":0,"end_line":6}"#
        );

        // An unterminated string ends at its line and leaves the newline to be
        // counted, so every later line number holds.
        let src = "message A {\n  string s = 1 [default = \"oops\n];\n}\nmessage B {\n}\n";
        let out = extract_proto_messages(src, "u.proto", module_id(), repo());
        assert_eq!(qnames(&out), vec!["message:proto:A", "message:proto:B"]);
        assert_eq!(
            cell(&out.nodes[1], cell_type::POSITION),
            r#"{"file":"u.proto","start_line":4,"end_line":5}"#
        );
    }

    /// The exact bytes of both copies in the proto-field-drift fixture, so its
    /// SCHEMA_FIELDS expectations are pinned here too.
    #[test]
    fn proto_field_drift_fixture_payloads() {
        let producer = include_str!(
            "../../../../bench/substrate-gap/fixtures/proto-field-drift/producer/proto/orders.proto"
        );
        let consumer = include_str!(
            "../../../../bench/substrate-gap/fixtures/proto-field-drift/consumer/proto/orders.proto"
        );
        let p = extract_proto_messages(producer, "proto/orders.proto", module_id(), repo());
        let c = extract_proto_messages(consumer, "proto/orders.proto", module_id(), repo());
        assert_eq!(
            fields_of(&p, "message:proto:shop.v1.OrderCreated"),
            Some(concat!(
                r#"{"format":"proto","fields":["#,
                r#"{"name":"order_id","type":"string","number":1},"#,
                r#"{"name":"total_cents","type":"int64","number":2},"#,
                r#"{"name":"sku","type":"string","number":3,"label":"repeated"},"#,
                r#"{"name":"labels","type":"map<string,string>","number":4},"#,
                r#"{"name":"card_token","type":"string","number":5,"oneof":"payer"},"#,
                r#"{"name":"wallet_id","type":"string","number":6,"oneof":"payer"}]}"#
            ))
        );
        assert_eq!(
            fields_of(&c, "message:proto:shop.v1.OrderCreated"),
            Some(concat!(
                r#"{"format":"proto","fields":["#,
                r#"{"name":"order_id","type":"string","number":1},"#,
                r#"{"name":"total_cents","type":"int32","number":2},"#,
                r#"{"name":"sku","type":"string","number":3,"label":"repeated"}]}"#
            ))
        );
        assert_eq!((p.schema_fields + c.schema_fields, p.schema_field_cells + c.schema_field_cells), (9, 2));
    }

    #[test]
    fn field_cap_500() {
        let body: String = (1..=600).map(|i| format!("  string f{i} = {i};\n")).collect();
        let reserved: Vec<String> = (1000..1600).map(|i| i.to_string()).collect();
        let src = format!("message Big {{\n{body}  reserved {};\n}}\n", reserved.join(", "));
        let out = extract_proto_messages(&src, "b.proto", module_id(), repo());
        let j = fields_of(&out, "message:proto:Big").unwrap_or_default();
        assert_eq!(j.matches(r#""name":"#).count(), SCHEMA_MAX_FIELDS);
        assert!(j.contains(r#""name":"f500""#) && !j.contains(r#""name":"f501""#), "the first 500 kept");
        assert!(j.contains(r#""1499""#) && !j.contains(r#""1500""#), "reserved capped the same way");
        assert!(j.ends_with(r#","truncated":true}"#), "{}", &j[j.len() - 40..]);
        assert_eq!(out.schema_fields, SCHEMA_MAX_FIELDS);

        // A pathological `map<map<map<...` statement is not a field, and the
        // type scan never recurses into it.
        let deep = format!("message D {{ {} string> x = 1; }}", "map<".repeat(100_000));
        let out = extract_proto_messages(&deep, "d.proto", module_id(), repo());
        assert_eq!(fields_of(&out, "message:proto:D"), Some(r#"{"format":"proto","fields":[]}"#));

        let exact: String = (1..=500).map(|i| format!("  string f{i} = {i};\n")).collect();
        let out = extract_proto_messages(&format!("message Full {{\n{exact}}}\n"), "f.proto", module_id(), repo());
        assert!(!fields_of(&out, "message:proto:Full").unwrap_or_default().contains("truncated"));

        let fields: Vec<String> = (1..=600).map(|i| format!(r#"{{"name":"f{i}","type":"int"}}"#)).collect();
        let big = format!(r#"{{"type":"record","name":"Big","fields":[{}]}}"#, fields.join(","));
        let out = avro(&big);
        let j = fields_of(&out, "message:avro:Big").unwrap_or_default();
        assert_eq!(j.matches(r#""name":"#).count(), SCHEMA_MAX_FIELDS);
        assert!(j.ends_with(r#"{"name":"f500","type":"int"}],"truncated":true}"#));
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

    #[test]
    fn avro_union_array_map_default() {
        let src = r#"{"type":"record","name":"OrderPlaced","namespace":"com.shop","fields":[
  {"name":"coupon","type":["null","string"],"default":null},
  {"name":"skus","type":{"type":"array","items":"string"}},
  {"name":"labels","type":{"type":"map","values":"long"},"default":{}},
  {"name":"at","type":{"type":"long","logicalType":"timestamp-millis"}},
  {"name":"money","type":"Money"},
  {"name":"ext","type":"com.other.Ext"},
  {"name":"price","type":{"type":"bytes","logicalType":"decimal","precision":9,"scale":2}},
  {"name":"bad","type":7},
  {"type":"string"}
]}"#;
        let out = avro(src);
        assert_eq!(
            fields_of(&out, "message:avro:com.shop.OrderPlaced"),
            Some(concat!(
                r#"{"format":"avro","fields":["#,
                r#"{"name":"coupon","type":"null|string","default":true},"#,
                r#"{"name":"skus","type":"array<string>"},"#,
                r#"{"name":"labels","type":"map<long>","default":true},"#,
                r#"{"name":"at","type":"long(timestamp-millis)"},"#,
                r#"{"name":"money","type":"com.shop.Money"},"#,
                r#"{"name":"ext","type":"com.other.Ext"},"#,
                r#"{"name":"price","type":"bytes(decimal)"},"#,
                r#"{"name":"bad"}]}"#
            )),
            "a default is flagged, never stored; a bare named reference resolves in the record's \
             namespace; a non-schema type is left out; a field with no name is skipped"
        );
        assert_eq!((out.schema_field_cells, out.schema_fields), (1, 8));
    }

    #[test]
    fn avro_nested_record_type_is_fullname() {
        let src = r#"{"type":"record","name":"User","namespace":"com.shop","fields":[
  {"name":"address","type":["null",{"type":"record","name":"Address","fields":[{"name":"city","type":"string"}]}]},
  {"name":"status","type":{"type":"enum","name":"Status","symbols":["A"]}},
  {"name":"geo","type":{"type":"fixed","name":"com.geo.Hash","size":16}},
  {"name":"tags","type":{"type":"array","items":{"type":"record","name":"Tag","namespace":"com.meta","fields":[]}}}
]}"#;
        let out = avro(src);
        assert_eq!(
            fields_of(&out, "message:avro:com.shop.User"),
            Some(concat!(
                r#"{"format":"avro","fields":["#,
                r#"{"name":"address","type":"null|com.shop.Address"},"#,
                r#"{"name":"status","type":"com.shop.Status"},"#,
                r#"{"name":"geo","type":"com.geo.Hash"},"#,
                r#"{"name":"tags","type":"array<com.meta.Tag>"}]}"#
            )),
            "an inline named type is its fullname, the way the node that declares it is named"
        );
        assert_eq!(
            fields_of(&out, "message:avro:com.shop.Address"),
            Some(r#"{"format":"avro","fields":[{"name":"city","type":"string"}]}"#),
            "the nested record's fields are its own cell"
        );
        assert_eq!(fields_of(&out, "message:avro:com.meta.Tag"), Some(r#"{"format":"avro","fields":[]}"#));
        assert_eq!(fields_of(&out, "message:avro:com.shop.Status"), None, "an enum has no fields");
        assert_eq!(fields_of(&out, "message:avro:com.geo.Hash"), None, "a fixed has no fields");
        assert_eq!((out.schema_field_cells, out.schema_fields), (3, 5));
    }

    /// The exact bytes of both copies in the avro-field-drift fixture.
    #[test]
    fn avro_field_drift_fixture_payloads() {
        let producer = include_str!(
            "../../../../bench/substrate-gap/fixtures/avro-field-drift/producer/schemas/order.avsc"
        );
        let consumer = include_str!(
            "../../../../bench/substrate-gap/fixtures/avro-field-drift/consumer/schemas/order.avsc"
        );
        assert_eq!(
            fields_of(&avro(producer), "message:avro:com.shop.OrderPlaced"),
            Some(concat!(
                r#"{"format":"avro","fields":["#,
                r#"{"name":"orderId","type":"string"},"#,
                r#"{"name":"totalCents","type":"long"},"#,
                r#"{"name":"coupon","type":"null|string","default":true}]}"#
            ))
        );
        assert_eq!(
            fields_of(&avro(consumer), "message:avro:com.shop.OrderPlaced"),
            Some(concat!(
                r#"{"format":"avro","fields":["#,
                r#"{"name":"orderId","type":"string"},"#,
                r#"{"name":"totalCents","type":"int"},"#,
                r#"{"name":"channel","type":"string"}]}"#
            ))
        );
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
