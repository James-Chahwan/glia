//! Declared message / payload schema types (A10.5).
//!
//! A `.proto` is the contract every service that speaks it shares, and the
//! `message User {}` block is what each language's generated `User` class is
//! derived from. `grpc.rs` owns the proto `service` / `rpc` path (A5.1); this
//! module adds the `message` and `enum` declarations from the same file, as
//! `node_kind::MESSAGE_TYPE` nodes, so a polyglot stack has one node per
//! declared type to share.
//!
//! qname convention: `message:<flavor>:<qualified name>`, flavor `proto`
//! today (`avro` / `jsonschema` reserved for A10.12). The proto qualified name
//! is written the way protobuf writes it: `<package>.<Outer>.<Inner>`.

use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, cell_type, edge_category, node_kind};
use repo_graph_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};

/// Everything the schema declarations in one file contribute to the graph.
#[derive(Default)]
pub struct SchemaNodes {
    pub nodes: Vec<Node>,
    /// Outer message DEFINES each nested message / enum.
    pub edges: Vec<Edge>,
    pub nav: CodeNav,
    /// `message` declarations emitted (nested ones included).
    pub message_count: usize,
    /// `enum` declarations emitted (nested ones included).
    pub enum_count: usize,
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
                Cell {
                    kind: cell_type::POSITION,
                    payload: CellPayload::Json(format!(
                        r#"{{"file":"{}","start_line":{},"end_line":{}}}"#,
                        json_str(path),
                        d.start_line,
                        d.end_line
                    )),
                },
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
}
