use std::collections::{HashMap, HashSet};

use glia_code_domain::endpoint::{
    ClientEndpoint, HitExtras, canonical_http_path, client_url_split, push_client_endpoint_with,
    route_qname,
};
use glia_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};
use tree_sitter::{Node as TsNode, Parser};

pub use glia_code_domain::{
    CallQualifier, CallSite, CodeNav, FileParse, GRAPH_TYPE, ImportStmt, ImportTarget, ParseError,
    UnresolvedRef, cell_type, edge_category, node_kind,
};

pub fn parse_file(
    source: &str,
    file_rel_path: &str,
    module_qname: &str,
    repo: RepoId,
) -> Result<FileParse, ParseError> {
    let mut parser = Parser::new();
    let lang: tree_sitter::Language = tree_sitter_clojure_orchard::LANGUAGE.into();
    parser
        .set_language(&lang)
        .map_err(|e| ParseError::LanguageInit(e.to_string()))?;
    let tree = parser.parse(source, None).ok_or(ParseError::NoTree)?;
    let src = source.as_bytes();
    let root = tree.root_node();

    let mut acc = Acc::default();

    let module_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MODULE, module_qname);
    acc.nodes.push(Node {
        id: module_id,
        repo,
        confidence: Confidence::Strong,
        cells: file_cells(&root, src, file_rel_path),
    });
    let module_simple = module_qname.rsplit("::").next().unwrap_or(module_qname);
    acc.nav
        .record(module_id, module_simple, module_qname, node_kind::MODULE, None);

    visit_top(root, src, file_rel_path, module_qname, module_id, repo, &mut acc);
    scan_clojure_routes(source, repo, &mut acc);
    let endpoints = acc.clj_http_hits + acc.hato_hits;
    if endpoints > 0 {
        eprintln!(
            "[clj-http] endpoints={endpoints} (clj-http={} hato={}) path={file_rel_path}",
            acc.clj_http_hits, acc.hato_hits
        );
    }
    if acc.docstrings_attached + acc.ns_docstrings > 0 {
        eprintln!(
            "[doc] clojure docstrings attached={} ns={} path={file_rel_path}",
            acc.docstrings_attached, acc.ns_docstrings
        );
    }

    Ok(FileParse {
        nodes: acc.nodes,
        edges: acc.edges,
        imports: acc.imports,
        calls: acc.calls,
        refs: acc.refs,
        nav: acc.nav,
        properties: Default::default(),
    })
}

#[derive(Default)]
struct Acc {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    imports: Vec<ImportStmt>,
    calls: Vec<CallSite>,
    refs: Vec<UnresolvedRef>,
    nav: CodeNav,
    /// LA.22c: `alias -> namespace` from this file's require vectors
    /// (`[clj-http.client :as client]`). An `Acc` lives for one `parse_file`,
    /// so two files that alias `client` differently never see each other's.
    ns_aliases: HashMap<String, String>,
    /// LA.22c: dedups ENDPOINT nodes across the file (the sink's `seen`).
    endpoint_seen: HashSet<NodeId>,
    /// LA.22c: client call sites that emitted an ENDPOINT, per library, for
    /// the `[clj-http]` marker.
    clj_http_hits: usize,
    hato_hits: usize,
    /// LA.7b: FUNCTION / INTERFACE nodes whose DOC came from a docstring (or
    /// `:doc` metadata), and file MODULEs that took an `ns` docstring, for the
    /// `[doc]` marker.
    docstrings_attached: usize,
    ns_docstrings: usize,
}

fn visit_top(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        visit_form(child, src, file_rel, parent_qname, parent_id, repo, acc);
    }
}

fn visit_form(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    if node.kind() != "list_lit" {
        return;
    }
    let Some(head) = first_symbol(node, src) else {
        return;
    };

    match head {
        "ns" => {
            collect_ns(node, src, parent_qname, acc);
            attach_ns_docstring(node, src, parent_id, acc);
        }
        "def" | "defn" | "defn-" | "defmacro" => {
            visit_defn(node, src, file_rel, parent_qname, parent_id, repo, acc);
        }
        "defprotocol" => {
            visit_defprotocol(node, src, file_rel, parent_qname, parent_id, repo, acc);
        }
        "defrecord" | "deftype" => {
            visit_defrecord(node, src, file_rel, parent_qname, parent_id, repo, acc);
        }
        "require" => collect_require(node, src, parent_qname, acc),
        _ => {}
    }
}

fn first_symbol<'a>(list: TsNode<'a>, src: &'a [u8]) -> Option<&'a str> {
    let mut cursor = list.walk();
    for child in list.named_children(&mut cursor) {
        if child.kind() == "sym_lit" {
            return Some(text_of(child, src));
        }
    }
    None
}

fn first_kwd<'a>(list: TsNode<'a>, src: &'a [u8]) -> Option<&'a str> {
    let mut cursor = list.walk();
    for child in list.named_children(&mut cursor) {
        if child.kind() == "kwd_lit" {
            return Some(text_of(child, src));
        }
    }
    None
}

fn second_symbol<'a>(list: TsNode<'a>, src: &'a [u8]) -> Option<&'a str> {
    let mut cursor = list.walk();
    let mut seen_first = false;
    for child in list.named_children(&mut cursor) {
        if child.kind() == "sym_lit" {
            if seen_first {
                return Some(text_of(child, src));
            }
            seen_first = true;
        }
    }
    None
}

fn visit_defn(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let Some(name) = second_symbol(node, src) else {
        return;
    };
    let qname = format!("{parent_qname}::{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::FUNCTION, &qname);

    let form = if first_symbol(node, src) == Some("def") {
        DocForm::Def
    } else {
        DocForm::Fn
    };
    let doc = form_docstring(node, src, form);
    acc.docstrings_attached += usize::from(doc.is_some());
    acc.nodes.push(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells(&node, src, file_rel, doc),
    });
    acc.edges.push(Edge {
        from: parent_id,
        to: id,
        category: edge_category::DEFINES,
        confidence: Confidence::Strong,
        cells: Vec::new(),
    });
    acc.nav
        .record(id, name, &qname, node_kind::FUNCTION, Some(parent_id));

    collect_calls_in(node, src, id, acc);
    collect_client_endpoints_in(node, src, id, repo, file_rel, acc);
}

fn visit_defprotocol(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let Some(name) = second_symbol(node, src) else {
        return;
    };
    let qname = format!("{parent_qname}::{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::INTERFACE, &qname);

    let doc = form_docstring(node, src, DocForm::Protocol);
    acc.docstrings_attached += usize::from(doc.is_some());
    acc.nodes.push(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells(&node, src, file_rel, doc),
    });
    acc.edges.push(Edge {
        from: parent_id,
        to: id,
        category: edge_category::DEFINES,
        confidence: Confidence::Strong,
        cells: Vec::new(),
    });
    acc.nav
        .record(id, name, &qname, node_kind::INTERFACE, Some(parent_id));

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "list_lit"
            && let Some(method_name) = first_symbol(child, src)
        {
            let mq = format!("{qname}::{method_name}");
            let mid = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::METHOD, &mq);
            acc.nodes.push(Node {
                id: mid,
                repo,
                confidence: Confidence::Strong,
                cells: entity_cells(&child, src, file_rel, None),
            });
            acc.edges.push(Edge {
                from: id,
                to: mid,
                category: edge_category::DEFINES,
                confidence: Confidence::Strong,
                cells: Vec::new(),
            });
            acc.nav
                .record(mid, method_name, &mq, node_kind::METHOD, Some(id));
        }
    }
}

fn visit_defrecord(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let Some(name) = second_symbol(node, src) else {
        return;
    };
    let qname = format!("{parent_qname}::{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::STRUCT, &qname);

    acc.nodes.push(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells(&node, src, file_rel, None),
    });
    acc.edges.push(Edge {
        from: parent_id,
        to: id,
        category: edge_category::DEFINES,
        confidence: Confidence::Strong,
        cells: Vec::new(),
    });
    acc.nav
        .record(id, name, &qname, node_kind::STRUCT, Some(parent_id));
}

fn collect_ns(node: TsNode, src: &[u8], from_module: &str, acc: &mut Acc) {
    // `(ns app.core (:require [app.util :as util] ...))`
    //
    // The `:require` (and `:use`) clauses are nested `list_lit`s whose HEAD is a
    // keyword (`kwd_lit` = `:require`), not a symbol — so `first_symbol` never
    // matches them, and the `:require` keyword is never a direct child of the ns
    // form. Detect the clause by its leading keyword, then reuse
    // `collect_require` to walk the `[dep :as alias]` vectors inside.
    //
    // `from_module` is the MODULE node's qname (the file stem the engine passes,
    // e.g. `core`) — NOT the dotted ns name — so the emitted `ImportStmt`
    // resolves against `module_by_qname` in the graph builder.
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "list_lit"
            && matches!(first_kwd(child, src), Some(":require") | Some(":use"))
        {
            collect_require(child, src, from_module, acc);
        }
    }
}

// ============================================================================
// Clojure docstrings (LA.7b)
// ============================================================================
//
// A Clojure docstring is not a comment: it is the string literal after the
// name INSIDE the defining form, so `glia_doc::leading_doc` (which walks
// preceding comment siblings) never sees it. Clojure's own precedence, lowest
// first: `^{:doc ".."}` metadata on the name symbol, then the docstring, then
// an attr-map `{:doc ".."}` right after it (defn / defmacro / ns only). A
// form the parser does not visit (defmulti, ...) keeps its docstring dropped:
// this adds DOC cells, never nodes.

/// Which docstring grammar a defining form follows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DocForm {
    /// `(def name "doc"? init?)`: the string is a docstring only when an init
    /// follows it. A lone `(def x "s")` binds the string as x's VALUE.
    Def,
    /// `(defn name "doc"? {attr-map}? [params] body)`, and defn- / defmacro.
    Fn,
    /// `(defprotocol Name "doc"? opts* method-sigs*)`.
    Protocol,
    /// `(ns name "doc"? {attr-map}? references*)`.
    Ns,
}

/// The docstring of a defining form, cleaned to the one-line shape
/// `glia_doc::leading_doc` produces. `None` when the form has none, so
/// the caller falls back to the comment above it.
fn form_docstring(list: TsNode, src: &[u8], form: DocForm) -> Option<String> {
    let vals: Vec<TsNode> = values(list).collect();
    let name = vals.get(1).filter(|n| n.kind() == "sym_lit")?;
    // def / defn need a form after the string or map (an init, an arg
    // vector); ns and defprotocol may end on their docstring.
    let followed = |i: usize| match form {
        DocForm::Def | DocForm::Fn => vals.len() > i + 1,
        DocForm::Protocol | DocForm::Ns => true,
    };
    let mut doc = name_meta_doc(*name, src);
    let mut next = 2;
    if let Some(s) = vals.get(next).filter(|n| n.kind() == "str_lit")
        && followed(next)
    {
        doc = str_content(*s, src);
        next += 1;
    }
    if matches!(form, DocForm::Fn | DocForm::Ns)
        && let Some(m) = vals.get(next).filter(|n| n.kind() == "map_lit")
        && followed(next)
        && let Some(d) = map_doc(*m, src)
    {
        doc = Some(d);
    }
    doc.map(clean_docstring).filter(|d| !d.is_empty())
}

/// `:doc` from reader metadata on the name symbol: `^{:doc "x"} name` (or
/// the old `#^{...}`). The metadata is part of the `sym_lit` node.
fn name_meta_doc<'a>(sym: TsNode<'a>, src: &'a [u8]) -> Option<&'a str> {
    let mut cursor = sym.walk();
    let metas: Vec<TsNode> = sym
        .named_children(&mut cursor)
        .filter(|c| matches!(c.kind(), "meta_lit" | "old_meta_lit"))
        .collect();
    metas.into_iter().find_map(|m| {
        let map = m.child_by_field_name("value").filter(|v| v.kind() == "map_lit")?;
        map_doc(map, src)
    })
}

/// The raw text of a string-valued `:doc` key in a map literal.
fn map_doc<'a>(map: TsNode<'a>, src: &'a [u8]) -> Option<&'a str> {
    let kv: Vec<TsNode> = values(map).collect();
    kv.chunks_exact(2).find_map(|pair| {
        if pair[0].kind() == "kwd_lit"
            && text_of(pair[0], src) == ":doc"
            && pair[1].kind() == "str_lit"
        {
            str_content(pair[1], src)
        } else {
            None
        }
    })
}

/// Decode a docstring's escapes (`\"`, `\\`, `\uXXXX`; `\n` `\t` `\r` `\f`
/// `\b` are whitespace), collapse every whitespace run (the docstring's
/// continuation-line indent included) to one space, and cap at
/// [`glia_doc::DOC_MAX`] bytes on a char boundary.
fn clean_docstring(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n' | 't' | 'r' | 'f' | 'b') => out.push(' '),
            Some('u') => {
                let hex: String = chars.clone().take(4).collect();
                match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                    Some(ch) if hex.len() == 4 => {
                        out.push(ch);
                        chars.nth(3);
                    }
                    _ => out.push_str("\\u"),
                }
            }
            Some(e) => out.push(e),
            None => out.push('\\'),
        }
    }
    let joined = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.len() <= glia_doc::DOC_MAX {
        return joined;
    }
    let mut end = glia_doc::DOC_MAX;
    while !joined.is_char_boundary(end) {
        end -= 1;
    }
    joined[..end].trim_end().to_string()
}

/// `(ns app.core "doc" ...)` documents the file: the docstring goes on the
/// file MODULE, which `parse_file` pushes first. `module_id` is the
/// `parent_id` of a top-level form, and the id check keeps the cell off any
/// other node. A second `ns` form in one file never overwrites the first's.
fn attach_ns_docstring(ns: TsNode, src: &[u8], module_id: NodeId, acc: &mut Acc) {
    let Some(doc) = form_docstring(ns, src, DocForm::Ns) else {
        return;
    };
    let Some(module) = acc.nodes.first_mut().filter(|n| n.id == module_id) else {
        return;
    };
    if module.cells.iter().any(|c| c.kind == cell_type::DOC) {
        return;
    }
    module.cells.push(Cell {
        kind: cell_type::DOC,
        payload: CellPayload::Text(doc),
    });
    acc.ns_docstrings += 1;
}

fn extract_require_from_vec<'a>(vec_node: TsNode<'a>, src: &'a [u8]) -> String {
    let mut cursor = vec_node.walk();
    for child in vec_node.named_children(&mut cursor) {
        if child.kind() == "sym_lit" {
            return text_of(child, src).to_string();
        }
    }
    String::new()
}

fn collect_require(node: TsNode, src: &[u8], from_module: &str, acc: &mut Acc) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "quoting_lit" {
            // `(require '[clj-http.client :as client])`: the alias only. The
            // quoted form has never emitted an import, and LA.22c does not
            // change that.
            if let Some(vec) = values(child).find(|v| v.kind() == "vec_lit") {
                record_alias(vec, src, acc);
            }
        }
        if child.kind() == "vec_lit" {
            record_alias(child, src, acc);
            let req = extract_require_from_vec(child, src);
            if !req.is_empty() {
                acc.imports.push(ImportStmt {
                    from_module: from_module.to_string(),
                    target: ImportTarget::Module {
                        path: req,
                        alias: None,
                    },
                    line: line_at(child),
                });
            }
        }
        if child.kind() == "sym_lit" && text_of(child, src) != "require" {
            let sym = text_of(child, src);
            acc.imports.push(ImportStmt {
                from_module: from_module.to_string(),
                target: ImportTarget::Module {
                    path: sym.to_string(),
                    alias: None,
                },
                line: line_at(child),
            });
        }
    }
}

fn collect_calls_in(node: TsNode, src: &[u8], from: NodeId, acc: &mut Acc) {
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if n.kind() == "list_lit"
            && let Some(head) = first_symbol(n, src)
            && !matches!(head, "def" | "defn" | "defn-" | "defmacro" | "let" | "if" | "when" | "do" | "fn" | "loop" | "cond" | "case" | "ns" | "require" | "defprotocol" | "defrecord" | "deftype")
        {
            if head.contains('/') {
                let parts: Vec<&str> = head.splitn(2, '/').collect();
                acc.calls.push(CallSite {
                    from,
                    qualifier: CallQualifier::Attribute {
                        base: parts[0].to_string(),
                        name: parts[1].to_string(),
                    },
                    line: line_at(n),
                });
            } else if let Some(stripped) = head.strip_prefix('.') {
                acc.calls.push(CallSite {
                    from,
                    qualifier: CallQualifier::SelfMethod(stripped.to_string()),
                    line: line_at(n),
                });
            } else {
                acc.calls.push(CallSite {
                    from,
                    qualifier: CallQualifier::Bare(head.to_string()),
                    line: line_at(n),
                });
            }
        }
        let mut cursor = n.walk();
        for child in n.named_children(&mut cursor) {
            if child.kind() != "list_lit" || first_symbol(child, src).is_none_or(|s| !matches!(s, "defn" | "defn-" | "fn" | "defmacro")) {
                stack.push(child);
            }
        }
    }
}

/// The 0-based row a node starts on: the `line` of the `CallSite` /
/// `UnresolvedRef` / `ImportStmt` it asserts (LC.3b, POSITION convention).
fn line_at(n: TsNode) -> u32 {
    u32::try_from(n.start_position().row).unwrap_or(u32::MAX)
}

fn text_of<'a>(node: TsNode<'a>, src: &'a [u8]) -> &'a str {
    node.utf8_text(src).unwrap_or("")
}

/// The `value` children of a form: its elements, without comments or `#_`
/// discarded forms (tree-sitter-clojure leaves those unlabelled).
fn values<'a>(node: TsNode<'a>) -> impl Iterator<Item = TsNode<'a>> {
    let mut cursor = node.walk();
    node.children_by_field_name("value", &mut cursor)
        .collect::<Vec<_>>()
        .into_iter()
}

/// `[clj-http.client :as client]` records `client -> clj-http.client`.
fn record_alias(vec: TsNode, src: &[u8], acc: &mut Acc) {
    let vals: Vec<TsNode> = values(vec).collect();
    let Some(ns) = vals.first().filter(|n| n.kind() == "sym_lit") else {
        return;
    };
    for pair in vals.windows(2) {
        if pair[0].kind() == "kwd_lit"
            && text_of(pair[0], src) == ":as"
            && pair[1].kind() == "sym_lit"
        {
            acc.ns_aliases.insert(
                text_of(pair[1], src).to_string(),
                text_of(*ns, src).to_string(),
            );
        }
    }
}

// ============================================================================
// Clojure HTTP clients (LA.22c): clj-http, hato
// ============================================================================
//
// `(client/get "http://api/users" opts)` with `client` bound to
// `clj-http.client` (or `hato.client`) by the file's require, the
// fully-qualified `(clj-http.client/get ...)`, and the map form
// `(client/request {:method :get :url "..."})` become ENDPOINT nodes through
// the shared code-domain sink, with a CALLS edge from the enclosing
// top-level form's node. A `client/get` whose alias is not bound to one of
// these namespaces stays a plain call.

#[derive(Clone, Copy)]
enum CljClient {
    CljHttp,
    Hato,
}

/// The client library a namespace names. clj-http-lite is clj-http's
/// drop-in fork (same functions, same arguments), so it counts as clj-http.
fn clj_client(ns: &str) -> Option<CljClient> {
    match ns {
        "clj-http.client" | "clj-http.lite.client" => Some(CljClient::CljHttp),
        "hato.client" => Some(CljClient::Hato),
        _ => None,
    }
}

/// Upper-case verb for a client function / `:method` keyword name.
fn clj_http_verb(name: &str) -> Option<&'static str> {
    match name {
        "get" => Some("GET"),
        "post" => Some("POST"),
        "put" => Some("PUT"),
        "patch" => Some("PATCH"),
        "delete" => Some("DELETE"),
        "head" => Some("HEAD"),
        "options" => Some("OPTIONS"),
        _ => None,
    }
}

/// `(namespace, name)` of a form's head symbol, read from the symbol's own
/// `namespace` / `name` fields so reader metadata on it never leaks in:
/// `(client/get ...)` -> `(Some("client"), "get")`, `(str ...)` ->
/// `(None, "str")`. None when the head is not a symbol.
fn head_sym<'a>(list: TsNode<'a>, src: &'a [u8]) -> Option<(Option<&'a str>, &'a str)> {
    let head = list
        .child_by_field_name("value")
        .filter(|h| h.kind() == "sym_lit")?;
    let name = text_of(head.child_by_field_name("name")?, src);
    let ns = head
        .child_by_field_name("namespace")
        .map(|n| text_of(n, src));
    Some((ns, name))
}

/// Walk one top-level form for client calls. Pre-order, so the first call
/// site of a `(method, path)` owns the ENDPOINT_HIT cell. A call is a
/// `(...)` list or a `#(...)` anonymous fn, whose head is its first element
/// too. Descends into anonymous `(fn ...)` / `#(...)` bodies (they run as
/// part of the enclosing form), but never into `(comment ...)` or a quoted
/// `'(...)`, which are not evaluated.
fn collect_client_endpoints_in(
    node: TsNode,
    src: &[u8],
    from: NodeId,
    repo: RepoId,
    file_rel: &str,
    acc: &mut Acc,
) {
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if n.kind() == "quoting_lit" {
            continue;
        }
        if matches!(n.kind(), "list_lit" | "anon_fn_lit")
            && let Some((ns, name)) = head_sym(n, src)
        {
            if ns.is_none() && name == "comment" {
                continue;
            }
            if let Some(lib) =
                ns.and_then(|ns| clj_client(acc.ns_aliases.get(ns).map_or(ns, String::as_str)))
            {
                try_detect_clj_endpoint(n, src, lib, name, from, repo, file_rel, acc);
            }
        }
        let children: Vec<TsNode> = values(n).collect();
        stack.extend(children.into_iter().rev());
    }
}

/// One clj-http / hato call: `(verb url opts?)` or `(request {:method ..
/// :url ..})`. Emits nothing when the URL is not a literal the sink can split
/// into a path (a bare var, or a `(str base "/x")` whose base is unknown).
#[allow(clippy::too_many_arguments)]
fn try_detect_clj_endpoint(
    list: TsNode,
    src: &[u8],
    lib: CljClient,
    name: &str,
    from: NodeId,
    repo: RepoId,
    file_rel: &str,
    acc: &mut Acc,
) -> Option<()> {
    let args: Vec<TsNode> = values(list).skip(1).collect();
    let (verb, url) = if name == "request" {
        let map = args.first().filter(|a| a.kind() == "map_lit")?;
        let kv: Vec<TsNode> = values(*map).collect();
        let (mut verb, mut url) = (None, None);
        for pair in kv.chunks_exact(2) {
            match text_of(pair[0], src) {
                ":method" | ":request-method" if pair[1].kind() == "kwd_lit" => {
                    verb = clj_http_verb(text_of(pair[1], src).trim_start_matches(':'));
                }
                ":url" => url = Some(pair[1]),
                _ => {}
            }
        }
        (verb?, url?)
    } else {
        (clj_http_verb(name)?, *args.first()?)
    };
    let (raw, interpolated) = clj_url_arg(url, src)?;
    let (host, path) = client_url_split(&raw);
    let pos = list.start_position();
    let ep = ClientEndpoint {
        method: verb.to_string(),
        path: path?,
        file: file_rel.to_string(),
        line: pos.row + 1,
        col: pos.column + 1,
        confidence: if interpolated {
            Confidence::Medium
        } else {
            Confidence::Strong
        },
    };
    let extras = HitExtras {
        host: host.as_deref(),
        ..HitExtras::default()
    };
    push_client_endpoint_with(
        repo,
        &ep,
        extras,
        from,
        &mut acc.nodes,
        &mut acc.edges,
        &mut acc.nav,
        &mut acc.endpoint_seen,
    );
    match lib {
        CljClient::CljHttp => acc.clj_http_hits += 1,
        CljClient::Hato => acc.hato_hits += 1,
    }
    Some(())
}

/// The placeholder every parser writes for an interpolated URL segment, so
/// `normalise_http_path` collapses it the same way it does a TS template.
const INTERP: &str = "${…}";

/// Reconstruct a URL argument: a string literal is itself (Strong);
/// `(str "http://api/users/" id)` is `http://api/users/${…}` and
/// `(format "http://api/users/%s" id)` the same (Medium). Returns
/// `(text, interpolated)`; None for anything else (a var, a call).
fn clj_url_arg(node: TsNode, src: &[u8]) -> Option<(String, bool)> {
    match node.kind() {
        "str_lit" => Some((str_content(node, src)?.to_string(), false)),
        "list_lit" => {
            let (ns, name) = head_sym(node, src)?;
            if ns.is_some_and(|ns| ns != "clojure.core") {
                return None;
            }
            match name {
                "str" => Some(str_concat(node, src)),
                "format" => {
                    let fmt = values(node).nth(1).filter(|f| f.kind() == "str_lit")?;
                    Some(format_template(str_content(fmt, src)?))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// A `str_lit`'s text without its quotes.
fn str_content<'a>(node: TsNode<'a>, src: &'a [u8]) -> Option<&'a str> {
    text_of(node, src).strip_prefix('"')?.strip_suffix('"')
}

/// `(str a "/x" b)`: literal strings and numbers verbatim, anything else one
/// `${…}` (adjacent ones collapse, `(str host port "/x")` is `${…}/x`).
fn str_concat(list: TsNode, src: &[u8]) -> (String, bool) {
    let mut out = String::new();
    let mut interpolated = false;
    for arg in values(list).skip(1) {
        match arg.kind() {
            "str_lit" => out.push_str(str_content(arg, src).unwrap_or("")),
            "num_lit" => out.push_str(text_of(arg, src)),
            _ => {
                if !out.ends_with(INTERP) {
                    out.push_str(INTERP);
                }
                interpolated = true;
            }
        }
    }
    (out, interpolated)
}

/// A `format` template with every conversion (`%s`, `%d`, `%05d`, `%1$s`)
/// as `${…}`; `%%` is a literal `%`.
fn format_template(fmt: &str) -> (String, bool) {
    let mut out = String::with_capacity(fmt.len());
    let mut interpolated = false;
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        if chars.peek() == Some(&'%') {
            chars.next();
            out.push('%');
            continue;
        }
        // flags, width, precision, argument index, then the conversion letter
        while chars.peek().is_some_and(|c| {
            c.is_ascii_digit() || matches!(c, '-' | '#' | '+' | ' ' | ',' | '(' | '.' | '$')
        }) {
            chars.next();
        }
        if chars.next().is_some() {
            out.push_str(INTERP);
            interpolated = true;
        }
    }
    (out, interpolated)
}

// ============================================================================
// Clojure route extraction (v0.4.11a R-clojure)
// ============================================================================
//
// Compojure:   (GET "/users" [] handler)      → GET /users
//              (POST "/users/:id" [] handler) → POST /users/:id
// Reitit:      ["/users" {:get list :post mk}] → GET /users, POST /users
//
// Text scan only — tree-sitter-clojure lacks semantic linking, so we match
// syntactic patterns directly. Clojure's macro system means any
// `(SYMBOL "literal" ...)` looks the same at parse time; gating on
// upper-case HTTP verbs keeps precision high.

fn scan_clojure_routes(source: &str, repo: RepoId, acc: &mut Acc) {
    let mut seen = std::collections::HashSet::new();

    // Compojure: `(GET "..."`, `(POST "..."`, etc.
    for method in ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "ANY"] {
        let needle = format!("({method} \"");
        let mut idx = 0;
        while let Some(pos) = source[idx..].find(&needle) {
            let start = idx + pos + needle.len();
            let after = &source[start..];
            if let Some(end) = after.find('"') {
                let path = &after[..end];
                if !path.is_empty() {
                    emit_clojure_route(method, path, repo, acc, &mut seen);
                }
            }
            idx = start;
        }
    }

    // Reitit: `["/path" {:get ... :post ...}]` (may span lines). LA.22c: the
    // string is route data only when `[` is the nearest non-whitespace byte
    // before it and `{` the nearest after its closing quote, and a method
    // counts only as a whole `:<method>` keyword, so a client call
    // `[r (client/get (str base "/users") {:headers ...})]` is not a route.
    // Every probe is a byte read, never a str slice at a computed offset, so
    // multibyte text anywhere in the file cannot panic the scan.
    let bytes = source.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"'
            && prev_non_ws(bytes, i) == Some(b'[')
            && let Some(end) = source[i + 1..].find('"')
        {
            let close = i + 1 + end;
            let path = &source[i + 1..close];
            if path.starts_with('/')
                && let Some(open) = next_non_ws(bytes, close + 1).filter(|&j| bytes[j] == b'{')
                && let Some(len) = source[open + 1..].find('}')
            {
                let block = &source[open + 1..open + 1 + len];
                for method in ["get", "post", "put", "patch", "delete", "head", "options"] {
                    if has_keyword(block, method) {
                        emit_clojure_route(
                            &method.to_ascii_uppercase(),
                            path,
                            repo,
                            acc,
                            &mut seen,
                        );
                    }
                }
            }
            i = close + 1;
            continue;
        }
        i += 1;
    }
}

/// Clojure whitespace: ASCII whitespace and `,`.
fn is_clj_ws(b: u8) -> bool {
    b.is_ascii_whitespace() || b == b','
}

/// The nearest non-whitespace byte before `i` (crossing newlines).
fn prev_non_ws(bytes: &[u8], i: usize) -> Option<u8> {
    bytes[..i].iter().rev().copied().find(|&b| !is_clj_ws(b))
}

/// Index of the nearest non-whitespace byte at or after `i`.
fn next_non_ws(bytes: &[u8], i: usize) -> Option<usize> {
    bytes
        .get(i..)?
        .iter()
        .position(|&b| !is_clj_ws(b))
        .map(|p| i + p)
}

/// A byte that continues a Clojure symbol / keyword token.
fn is_token_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b >= 0x80 || b"-_?!*+<>=.'/:#$%&|".contains(&b)
}

/// `block` holds the keyword `:<name>` as a whole token: `:head` matches in
/// `{:head h}` but not in `:headers`, `::head` or `:x/head`.
fn has_keyword(block: &str, name: &str) -> bool {
    let bytes = block.as_bytes();
    let kw = format!(":{name}");
    block.match_indices(&kw).any(|(p, _)| {
        let before_ok = p == 0 || !is_token_byte(bytes[p - 1]);
        let after_ok = bytes.get(p + kw.len()).is_none_or(|&b| !is_token_byte(b));
        before_ok && after_ok
    })
}

fn emit_clojure_route(
    method: &str,
    path: &str,
    repo: RepoId,
    acc: &mut Acc,
    seen: &mut std::collections::HashSet<(String, String)>,
) {
    // LB.5: keyed on the canonical path, so compojure's relative `"bolts"` and
    // a slashed `"/bolts"` for the same verb are one node, `GET /bolts`.
    let path = canonical_http_path(path);
    let key = (method.to_string(), path.to_string());
    if !seen.insert(key) {
        return;
    }
    let route_name = route_qname(method, &path);
    let route_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::ROUTE, &route_name);
    acc.nodes.push(Node {
        id: route_id,
        repo,
        confidence: Confidence::Medium,
        cells: vec![Cell {
            kind: cell_type::ROUTE_METHOD,
            payload: CellPayload::Text(method.to_string()),
        }],
    });
    acc.nav
        .record(route_id, &route_name, &route_name, node_kind::ROUTE, None);
}

fn file_cells(root: &TsNode, src: &[u8], file_rel: &str) -> Vec<Cell> {
    vec![
        Cell {
            kind: cell_type::CODE,
            payload: CellPayload::Text(text_of(*root, src).to_string()),
        },
        Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(glia_doc::position_json(root, file_rel)),
        },
    ]
}

/// CODE + POSITION, and a DOC cell: the form's own docstring (`doc`, from
/// [`form_docstring`]) wins; the `;;` comment above the form is the fallback.
fn entity_cells(node: &TsNode, src: &[u8], file_rel: &str, doc: Option<String>) -> Vec<Cell> {
    let mut cells = vec![
        Cell {
            kind: cell_type::CODE,
            payload: CellPayload::Text(text_of(*node, src).to_string()),
        },
        Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(glia_doc::position_json(node, file_rel)),
        },
    ];
    if let Some(doc) = doc.or_else(|| glia_doc::leading_doc(node, src)) {
        cells.push(Cell {
            kind: cell_type::DOC,
            payload: CellPayload::Text(doc),
        });
    }
    cells
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> RepoId {
        RepoId(1)
    }

    #[test]
    fn defns_and_protocol() {
        let source = r#"
(defprotocol Greeter
  (greet [this name]))

(defn hello [name]
  (str "Hello " name))

(defn- internal-fn []
  (println "private"))
"#;
        let fp = parse_file(source, "src/greeter.clj", "src::greeter", repo()).unwrap();
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::INTERFACE).count(), 1);
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::FUNCTION).count(), 2);
    }

    #[test]
    fn defrecord() {
        let source = r#"
(defrecord User [name email])
"#;
        let fp = parse_file(source, "src/user.clj", "src::user", repo()).unwrap();
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::STRUCT).count(), 1);
    }

    #[test]
    fn calls_detected() {
        let source = r#"
(defn process [x]
  (validate x)
  (db/save x))
"#;
        let fp = parse_file(source, "src/proc.clj", "src::proc", repo()).unwrap();
        assert!(fp.calls.iter().any(|c| matches!(&c.qualifier, CallQualifier::Bare(n) if n == "validate")));
        assert!(fp.calls.iter().any(|c| matches!(&c.qualifier, CallQualifier::Attribute { base, name } if base == "db" && name == "save")));
    }

    #[test]
    fn ns_require_emits_import() {
        // `app.core` requires `app.util`. The engine passes the file-stem qname
        // (`core`) as module_qname, so from_module must equal `core` to resolve
        // against module_by_qname, and the require target path (`app.util`) must
        // be emitted so the graph's tail fallback binds it to the `util` module.
        let source = r#"
(ns app.core
  (:require [app.util :as util]
            [clojure.string :as str]))

(defn run [x]
  (util/process x))
"#;
        let fp = parse_file(source, "core.clj", "core", repo()).unwrap();
        assert!(
            fp.imports.iter().any(|i| i.from_module == "core"
                && matches!(&i.target, ImportTarget::Module { path, .. } if path == "app.util")),
            "expected import core -> app.util, got {:?}",
            fp.imports
                .iter()
                .map(|i| (&i.from_module, &i.target))
                .collect::<Vec<_>>()
        );
        // Every ns-require import carries the file-stem from_module, not the
        // dotted ns name — otherwise resolve_imports_python early-continues.
        assert!(fp.imports.iter().all(|i| i.from_module == "core"));
    }

    fn route_id(method: &str, path: &str) -> NodeId {
        NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::ROUTE,
            &format!("{method} {path}"),
        )
    }

    #[test]
    fn compojure_routes_emit() {
        let source = r#"
(defroutes app-routes
  (GET "/users" [] (list-users))
  (POST "/users" [] (create-user))
  (DELETE "/users/:id" [id] (delete-user id)))
"#;
        let fp = parse_file(source, "src/routes.clj", "src::routes", repo()).unwrap();
        assert!(fp.nodes.iter().any(|n| n.id == route_id("GET", "/users")));
        assert!(fp.nodes.iter().any(|n| n.id == route_id("POST", "/users")));
        assert!(fp.nodes.iter().any(|n| n.id == route_id("DELETE", "/users/:id")));
    }

    /// LB.5 — compojure's relative `"bolts"` is `GET /bolts`, and a relative
    /// plus a slashed registration of one verb+path is one node.
    #[test]
    fn relative_compojure_path_is_canonical() {
        let source = r#"
(defroutes app-routes
  (GET "bolts" [] (list-bolts))
  (GET "/bolts" [] (list-bolts))
  (POST "bolts" [] (make-bolt)))
"#;
        let fp = parse_file(source, "src/routes.clj", "src::routes", repo()).unwrap();
        let get = route_id("GET", "/bolts");
        assert_eq!(fp.nodes.iter().filter(|n| n.id == get).count(), 1);
        assert!(fp.nodes.iter().any(|n| n.id == route_id("POST", "/bolts")));
        assert!(!fp.nodes.iter().any(|n| n.id == route_id("GET", "bolts")));
        assert_eq!(
            fp.nav.name_by_id.get(&get).map(String::as_str),
            Some("GET /bolts")
        );
    }

    #[test]
    fn reitit_routes_emit() {
        let source = r#"
(def routes
  [["/users" {:get list-users :post create-user}]
   ["/users/:id" {:delete delete-user}]])
"#;
        let fp = parse_file(source, "src/api.clj", "src::api", repo()).unwrap();
        assert!(fp.nodes.iter().any(|n| n.id == route_id("GET", "/users")));
        assert!(fp.nodes.iter().any(|n| n.id == route_id("POST", "/users")));
        assert!(fp.nodes.iter().any(|n| n.id == route_id("DELETE", "/users/:id")));
    }

    // ---- LA.22c: clj-http / hato clients, reitit boundary rules ----

    /// The matrix probe's client file (matrix/clojure/http_client).
    const PROBE: &str = r#"(ns app.api-client
  (:require [clj-http.client :as client]))

(def base "http://api")

(defn list-users []
  (:body (client/get "http://api/users" {:as :json})))

(defn create-user [user]
  (client/post "http://api/users" {:form-params user :content-type :json}))

(defn user-count [token]
  (let [r (client/get (str base "/users") {:headers {"Authorization" token}})]
    (count (:body r))))
"#;

    fn kind_count(fp: &FileParse, kind: glia_core::NodeKindId) -> usize {
        fp.nav.kind_by_id.values().filter(|k| **k == kind).count()
    }

    fn endpoint_names(fp: &FileParse) -> Vec<String> {
        let mut v: Vec<String> = fp
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::ENDPOINT)
            .filter_map(|(id, _)| fp.nav.name_by_id.get(id).cloned())
            .collect();
        v.sort();
        v
    }

    fn fn_id(module: &str, name: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, &format!("{module}::{name}"))
    }

    fn ep_id(method: &str, path: &str) -> NodeId {
        glia_code_domain::endpoint::endpoint_id(repo(), method, path)
    }

    fn has_calls(fp: &FileParse, from: NodeId, to: NodeId) -> bool {
        fp.edges
            .iter()
            .any(|e| e.from == from && e.to == to && e.category == edge_category::CALLS)
    }

    fn hit_json(fp: &FileParse, id: NodeId) -> String {
        fp.nodes
            .iter()
            .filter(|n| n.id == id)
            .flat_map(|n| n.cells.iter())
            .find_map(|c| match (&c.payload, c.kind == cell_type::ENDPOINT_HIT) {
                (CellPayload::Json(s), true) => Some(s.clone()),
                _ => None,
            })
            .unwrap_or_default()
    }

    #[test]
    fn clj_http_calls_emit_endpoints() {
        let fp = parse_file(PROBE, "api_client.clj", "api_client", repo()).unwrap();
        // user-count's `(str base "/users")` is `${…}/users`: no scheme, no
        // leading `/`, so client_url_split yields no path and no third node.
        assert_eq!(endpoint_names(&fp), vec!["GET /users", "POST /users"]);
        let get = ep_id("GET", "/users");
        let post = ep_id("POST", "/users");
        assert!(has_calls(&fp, fn_id("api_client", "list-users"), get));
        assert!(has_calls(&fp, fn_id("api_client", "create-user"), post));
        assert!(!fp.edges.iter().any(|e| e.from == fn_id("api_client", "user-count")
            && e.category == edge_category::CALLS));
        let hit = hit_json(&fp, get);
        assert!(hit.contains(r#""method":"GET""#), "{hit}");
        assert!(hit.contains(r#""file":"api_client.clj""#), "{hit}");
        assert!(hit.contains(r#""line":7"#), "{hit}");
        assert!(hit.contains(r#""host":"api""#), "{hit}");
        assert!(hit.contains(r#""confidence":"strong""#), "{hit}");
        assert!(hit_json(&fp, post).contains(r#""method":"POST""#));
    }

    #[test]
    fn client_call_with_headers_is_not_a_route() {
        let fp = parse_file(PROBE, "api_client.clj", "api_client", repo()).unwrap();
        assert_eq!(kind_count(&fp, node_kind::ROUTE), 0, "{:?}", fp.nav.name_by_id);
        assert!(!fp.nodes.iter().any(|n| n.id == route_id("HEAD", "/users")));
    }

    #[test]
    fn hato_verbs_and_request_map_emit_endpoints() {
        let source = r#"
(ns app.items
  (:require [hato.client :as hc]))

(defn put-item [id body]
  (hc/put (str "http://svc:8080/items/" id) {:body body}))

(defn drop-items []
  (hc/request {:method :delete :url "https://api.example.com/items"}))

(defn probe []
  (hc/head "/health"))
"#;
        let fp = parse_file(source, "src/items.clj", "src::items", repo()).unwrap();
        assert_eq!(
            endpoint_names(&fp),
            vec!["DELETE /items", "HEAD /health", "PUT /items/${…}"]
        );
        let put = ep_id("PUT", "/items/${…}");
        assert!(has_calls(&fp, fn_id("src::items", "put-item"), put));
        let hit = hit_json(&fp, put);
        assert!(hit.contains(r#""confidence":"medium""#), "{hit}");
        assert!(hit.contains(r#""host":"svc:8080""#), "{hit}");
        let del = hit_json(&fp, ep_id("DELETE", "/items"));
        assert!(del.contains(r#""host":"api.example.com""#), "{del}");
        assert!(has_calls(&fp, fn_id("src::items", "drop-items"), ep_id("DELETE", "/items")));
    }

    #[test]
    fn clj_http_request_map_and_format_url() {
        let source = r#"
(ns app.users
  (:require [clj-http.client :as http]))

(defn patch-user [id m]
  (http/request {:url (format "http://api/users/%s" id)
                 :request-method :patch
                 :form-params m}))

(defn all-users [ids]
  (mapv (fn [id] (http/get (str "http://api/users/" id "/profile"))) ids))

(defn pinged []
  (run! #(http/put (str "http://api/ping/" %)) [1 2])
  @(http/options "http://api/ping" {:async? true}))
"#;
        let fp = parse_file(source, "src/users.clj", "src::users", repo()).unwrap();
        assert_eq!(
            endpoint_names(&fp),
            vec![
                "GET /users/${…}/profile",
                "OPTIONS /ping",
                "PATCH /users/${…}",
                "PUT /ping/${…}"
            ]
        );
        // An anonymous `(fn ...)` belongs to its enclosing defn.
        assert!(has_calls(
            &fp,
            fn_id("src::users", "all-users"),
            ep_id("GET", "/users/${…}/profile")
        ));
    }

    #[test]
    fn fully_qualified_and_quoted_require_forms() {
        let source = r#"
(require '[hato.client :as hc])

(defn a [] (clj-http.client/get "http://api/a"))
(defn b [] (hc/post "http://api/b" {}))
"#;
        let fp = parse_file(source, "src/fq.clj", "src::fq", repo()).unwrap();
        assert_eq!(endpoint_names(&fp), vec!["GET /a", "POST /b"]);
    }

    #[test]
    fn unbound_client_alias_stays_a_plain_call() {
        // `client` is not bound to clj-http / hato here: a plain call, no ENDPOINT.
        let source = r#"
(ns app.other
  (:require [app.client :as client]))

(defn f [] (client/get "http://api/users"))
(defn g [] (svc/post "http://api/users" {}))
"#;
        let fp = parse_file(source, "src/other.clj", "src::other", repo()).unwrap();
        assert!(endpoint_names(&fp).is_empty(), "{:?}", endpoint_names(&fp));
        assert!(fp.calls.iter().any(|c| matches!(&c.qualifier,
            CallQualifier::Attribute { base, name } if base == "client" && name == "get")));
    }

    #[test]
    fn client_alias_is_per_file() {
        let a = r#"(ns a (:require [clj-http.client :as client]))
(defn f [] (client/get "http://api/users"))
"#;
        let b = r#"(ns b (:require [my.rpc :as client]))
(defn f [] (client/get "http://api/users"))
"#;
        let fa = parse_file(a, "a.clj", "a", repo()).unwrap();
        let fb = parse_file(b, "b.clj", "b", repo()).unwrap();
        assert_eq!(endpoint_names(&fa), vec!["GET /users"]);
        assert!(endpoint_names(&fb).is_empty());
    }

    #[test]
    fn discarded_and_comment_forms_emit_nothing() {
        let source = r#"
(ns app.scratch (:require [clj-http.client :as client]))

(defn f []
  #_(client/get "http://api/a")
  (comment (client/get "http://api/b"))
  (client/get base-url))
"#;
        let fp = parse_file(source, "src/scratch.clj", "src::scratch", repo()).unwrap();
        assert!(endpoint_names(&fp).is_empty(), "{:?}", endpoint_names(&fp));
    }

    #[test]
    fn reitit_multiline_route_vector_still_emits() {
        // `nearest non-whitespace byte` crosses newlines and Clojure's `,`.
        let source = r#"(def routes
  [
   "/users"
   {:get list-users
    :post create-user}]
  ["/ping" ,
   {:head ping}]
  ["/items"
   {:put put-item}])
"#;
        let fp = parse_file(source, "src/api.clj", "src::api", repo()).unwrap();
        assert!(fp.nodes.iter().any(|n| n.id == route_id("GET", "/users")));
        assert!(fp.nodes.iter().any(|n| n.id == route_id("POST", "/users")));
        assert!(fp.nodes.iter().any(|n| n.id == route_id("HEAD", "/ping")));
        assert!(fp.nodes.iter().any(|n| n.id == route_id("PUT", "/items")));
    }

    #[test]
    fn reitit_method_keyword_needs_a_token_boundary() {
        let source = r#"
(def routes
  [["/a" {:headers x :getter y :post? z :put! w :delete-all v}]
   ["/b" {:ns/get x ::get y}]])
"#;
        let fp = parse_file(source, "src/api.clj", "src::api", repo()).unwrap();
        assert_eq!(kind_count(&fp, node_kind::ROUTE), 0, "{:?}", fp.nav.name_by_id);
    }

    #[test]
    fn reitit_needs_bracket_before_and_brace_after_the_path() {
        let source = r#"
(def xs [(f "/a") {:get x}])
(def ys [x "/b" {:get x}])
(def zs ["/c" :name {:get x}])
"#;
        let fp = parse_file(source, "src/api.clj", "src::api", repo()).unwrap();
        assert_eq!(kind_count(&fp, node_kind::ROUTE), 0, "{:?}", fp.nav.name_by_id);
    }

    // ---- LA.7b: docstrings ----

    /// The DOC cell of the node `(kind, qname)`, if any.
    fn doc_of(fp: &FileParse, kind: glia_core::NodeKindId, qname: &str) -> Option<String> {
        let id = NodeId::from_parts(GRAPH_TYPE, repo(), kind, qname);
        let node = fp.nodes.iter().find(|n| n.id == id)?;
        node.cells.iter().find_map(|c| match (&c.payload, c.kind == cell_type::DOC) {
            (CellPayload::Text(s), true) => Some(s.clone()),
            _ => None,
        })
    }

    /// The fixture file (bench/substrate-gap/fixtures/clojure-docs).
    const DOCS: &str = r#"(ns myapp.core
  "Core namespace docs.")

(defprotocol Greeter
  "Things that greet."
  (greet-all [this]))

(defn greet
  "Returns a greeting for name."
  [name]
  (str "hi " name))

(def banner "not a doc")

;; leading comment
(defn plain [x] x)
"#;

    #[test]
    fn defn_docstring_becomes_doc_cell() {
        let fp = parse_file(DOCS, "src/core.clj", "src::core", repo()).unwrap();
        assert_eq!(
            doc_of(&fp, node_kind::FUNCTION, "src::core::greet").as_deref(),
            Some("Returns a greeting for name.")
        );
    }

    #[test]
    fn ns_docstring_lands_on_the_file_module() {
        let fp = parse_file(DOCS, "src/core.clj", "src::core", repo()).unwrap();
        assert_eq!(
            doc_of(&fp, node_kind::MODULE, "src::core").as_deref(),
            Some("Core namespace docs.")
        );
        // Exactly one DOC on the module.
        let module = &fp.nodes[0];
        assert_eq!(module.cells.iter().filter(|c| c.kind == cell_type::DOC).count(), 1);
    }

    #[test]
    fn defprotocol_docstring_lands_on_the_interface() {
        let fp = parse_file(DOCS, "src/core.clj", "src::core", repo()).unwrap();
        assert_eq!(
            doc_of(&fp, node_kind::INTERFACE, "src::core::Greeter").as_deref(),
            Some("Things that greet.")
        );
        // The method signature is not handed the protocol's docstring.
        assert_eq!(doc_of(&fp, node_kind::METHOD, "src::core::Greeter::greet-all"), None);
    }

    #[test]
    fn def_string_value_is_not_a_docstring() {
        let fp = parse_file(DOCS, "src/core.clj", "src::core", repo()).unwrap();
        assert_eq!(doc_of(&fp, node_kind::FUNCTION, "src::core::banner"), None);
    }

    #[test]
    fn def_docstring_needs_an_init_after_it() {
        let source = "(def answer \"The answer.\" 42)\n(def label \"just a value\")\n";
        let fp = parse_file(source, "src/v.clj", "src::v", repo()).unwrap();
        assert_eq!(
            doc_of(&fp, node_kind::FUNCTION, "src::v::answer").as_deref(),
            Some("The answer.")
        );
        assert_eq!(doc_of(&fp, node_kind::FUNCTION, "src::v::label"), None);
    }

    #[test]
    fn leading_semicolon_comment_is_the_fallback_without_its_marker() {
        let fp = parse_file(DOCS, "src/core.clj", "src::core", repo()).unwrap();
        assert_eq!(
            doc_of(&fp, node_kind::FUNCTION, "src::core::plain").as_deref(),
            Some("leading comment")
        );
    }

    #[test]
    fn defn_private_and_defmacro_docstrings() {
        let source = r#"
(defn- helper
  "Private helper."
  [x] x)

(defmacro unless
  "Inverted when."
  [test & body]
  `(when-not ~test ~@body))
"#;
        let fp = parse_file(source, "src/m.clj", "src::m", repo()).unwrap();
        assert_eq!(
            doc_of(&fp, node_kind::FUNCTION, "src::m::helper").as_deref(),
            Some("Private helper.")
        );
        assert_eq!(
            doc_of(&fp, node_kind::FUNCTION, "src::m::unless").as_deref(),
            Some("Inverted when.")
        );
    }

    #[test]
    fn docstring_wins_over_a_leading_comment() {
        let source = ";; the comment\n(defn f\n  \"The docstring.\"\n  [x] x)\n";
        let fp = parse_file(source, "src/w.clj", "src::w", repo()).unwrap();
        assert_eq!(
            doc_of(&fp, node_kind::FUNCTION, "src::w::f").as_deref(),
            Some("The docstring.")
        );
    }

    #[test]
    fn doc_metadata_on_the_name_and_attr_map() {
        // `^{:doc}` on the name symbol. The name keeps its metadata text in the
        // qname today (`second_symbol` reads the whole sym_lit), so find the
        // one FUNCTION node by kind.
        let source = "(defn ^{:doc \"Meta doc.\"} f [x] x)\n";
        let fp = parse_file(source, "src/a.clj", "src::a", repo()).unwrap();
        let fns: Vec<&Node> = fp
            .nodes
            .iter()
            .filter(|n| fp.nav.kind_by_id.get(&n.id) == Some(&node_kind::FUNCTION))
            .collect();
        assert_eq!(fns.len(), 1);
        let qname = fp.nav.qname_by_id.get(&fns[0].id).cloned().unwrap_or_default();
        assert_eq!(doc_of(&fp, node_kind::FUNCTION, &qname).as_deref(), Some("Meta doc."));

        // attr-map after the name, and after a docstring (the attr-map wins,
        // as in `defn`'s own metadata merge).
        let source = r#"
(defn g {:doc "Attr doc." :added "1.0"} [x] x)
(defn h "String doc." {:doc "Attr wins."} [x] x)
(defn k {:added "1.0"} [x] x)
"#;
        let fp = parse_file(source, "src/b.clj", "src::b", repo()).unwrap();
        assert_eq!(doc_of(&fp, node_kind::FUNCTION, "src::b::g").as_deref(), Some("Attr doc."));
        assert_eq!(doc_of(&fp, node_kind::FUNCTION, "src::b::h").as_deref(), Some("Attr wins."));
        assert_eq!(doc_of(&fp, node_kind::FUNCTION, "src::b::k"), None);

        // ns with `^{:doc}` metadata on its name (the older clojure.core style).
        let source = "(ns ^{:doc \"Set ops.\" :author \"x\"} app.sets)\n";
        let fp = parse_file(source, "src/sets.clj", "src::sets", repo()).unwrap();
        assert_eq!(
            doc_of(&fp, node_kind::MODULE, "src::sets").as_deref(),
            Some("Set ops.")
        );
    }

    #[test]
    fn docstring_escapes_and_whitespace_are_cleaned() {
        let source = "(defn f\n  \"Line one.\n   Says \\\"hi\\\" \\u00e9\\tend.\"\n  [x] x)\n";
        let fp = parse_file(source, "src/e.clj", "src::e", repo()).unwrap();
        assert_eq!(
            doc_of(&fp, node_kind::FUNCTION, "src::e::f").as_deref(),
            Some("Line one. Says \"hi\" é end.")
        );
        // Capped at DOC_MAX on a char boundary.
        let long = "é".repeat(glia_doc::DOC_MAX);
        let source = format!("(defn g \"{long}\" [x] x)\n");
        let fp = parse_file(&source, "src/e.clj", "src::e", repo()).unwrap();
        let doc = doc_of(&fp, node_kind::FUNCTION, "src::e::g").unwrap_or_default();
        assert!(doc.len() <= glia_doc::DOC_MAX);
        assert_eq!(doc.chars().count(), glia_doc::DOC_MAX / 2);
    }

    #[test]
    fn empty_docstring_falls_back_to_the_comment() {
        let source = ";; kept\n(defn f \"\" [x] x)\n";
        let fp = parse_file(source, "src/z.clj", "src::z", repo()).unwrap();
        assert_eq!(doc_of(&fp, node_kind::FUNCTION, "src::z::f").as_deref(), Some("kept"));
    }

    #[test]
    fn route_scan_survives_multibyte_text_before_a_string() {
        // HEAD sliced `&source[i - 32..i]` before every `"`; put a multibyte
        // char across that cut, for a plain string and for a route vector.
        let head = "(def s \"é…\")\n";
        let tail = "[\"/users\" {:get list-users}]";
        let ell = head.find('…').unwrap();
        let q = head.len() + 1; // the `"` after `[`
        let pad = " ".repeat(ell + 1 + 32 - q);
        let source = format!("{head}{pad}{tail}\n(def t \"é…\")\n");
        let quote = source.find("\"/users").unwrap();
        assert!(!source.is_char_boundary(quote - 32), "cut must land inside `…`");
        let fp = parse_file(&source, "src/api.clj", "src::api", repo()).unwrap();
        assert!(fp.nodes.iter().any(|n| n.id == route_id("GET", "/users")));
        assert_eq!(kind_count(&fp, node_kind::ROUTE), 1);
    }
}
