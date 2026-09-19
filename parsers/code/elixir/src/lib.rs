use glia_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};
use tree_sitter::{Node as TsNode, Parser};

pub use glia_code_domain::{
    CallQualifier, CallSite, CodeNav, FileParse, GRAPH_TYPE, ImportStmt, ImportTarget, ParseError,
    UnresolvedRef, cell_type, edge_category, node_kind,
};
use glia_code_domain::endpoint::{ClientEndpoint, join_scope, push_client_endpoint, url_to_path};
use glia_code_domain::line_of;

pub fn parse_file(
    source: &str,
    file_rel_path: &str,
    module_qname: &str,
    repo: RepoId,
) -> Result<FileParse, ParseError> {
    let mut parser = Parser::new();
    let lang: tree_sitter::Language = tree_sitter_elixir::LANGUAGE.into();
    parser
        .set_language(&lang)
        .map_err(|e| ParseError::LanguageInit(e.to_string()))?;
    let tree = parser.parse(source, None).ok_or(ParseError::NoTree)?;
    let src = source.as_bytes();
    let root = tree.root_node();

    let mut acc = Acc::default();
    // The file-stem MODULE qname (e.g. `controller`). Elixir `import`/`alias`/`use`
    // forms live *inside* a `defmodule`, so the walk's `parent_qname` at that point
    // is the enclosing PACKAGE qname (`controller::MyAppWeb.UserController`), which
    // `resolve_imports_python` never finds in `module_by_qname` (MODULE nodes only)
    // → every import early-continues. Stash the file MODULE qname so `collect_import`
    // emits `from_module` = the file stem, matching what the graph registers.
    acc.module_qname = module_qname.to_string();

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

    scan_phoenix_routes(source, repo, module_id, &mut acc);
    if acc.doc_attached + acc.doc_moduledoc + acc.doc_hidden > 0 {
        eprintln!(
            "[doc] elixir attrs attached={} moduledoc={} hidden={} path={}",
            acc.doc_attached, acc.doc_moduledoc, acc.doc_hidden, file_rel_path
        );
    }
    if acc.endpoint_hits > 0 {
        eprintln!(
            "[elixir-http-client] {} endpoints in {}",
            acc.endpoint_hits, file_rel_path
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
    /// File-stem MODULE qname; the `from_module` every import resolves against.
    module_qname: String,
    /// Per-file dedup for the shared ENDPOINT nodes client calls mint.
    endpoint_seen: std::collections::HashSet<NodeId>,
    /// Client HTTP call sites emitted from this file (drives the fired-on marker).
    endpoint_hits: usize,
    /// `@doc` texts attached to FUNCTION nodes in this file (LA.7a marker).
    doc_attached: usize,
    /// `@moduledoc` texts attached to PACKAGE / INTERFACE nodes (LA.7a marker).
    doc_moduledoc: usize,
    /// Nodes whose doc an explicit `@doc false` / `@moduledoc false` hid.
    doc_hidden: usize,
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
        visit_node(child, src, file_rel, parent_qname, parent_id, repo, acc);
    }
}

fn visit_node(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    if node.kind() != "call" {
        return;
    }
    let target = node.child_by_field_name("target").map(|n| text_of(n, src));
    let Some(target_name) = target else { return };

    match target_name {
        "defmodule" => visit_defmodule(node, src, file_rel, parent_qname, parent_id, repo, acc),
        // Outside a `defmodule` body there is no pending `@doc` to carry, so a
        // top-level `def` only ever gets the leading-comment fallback.
        "def" | "defp" => visit_def(
            node,
            src,
            file_rel,
            parent_qname,
            parent_id,
            repo,
            DocChoice::Leading,
            acc,
        ),
        "defprotocol" => visit_defprotocol(node, src, file_rel, parent_qname, parent_id, repo, acc),
        "defstruct" => visit_defstruct(node, src, file_rel, parent_qname, parent_id, repo, acc),
        "import" | "alias" | "use" => collect_import(node, src, acc),
        _ => {}
    }
}

fn find_args(node: TsNode) -> Option<TsNode> {
    let mut c = node.walk();
    node.named_children(&mut c).find(|ch| ch.kind() == "arguments")
}

fn visit_defmodule(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let Some(args) = find_args(node) else {
        return;
    };
    let name = first_arg_text(args, src);
    if name.is_empty() {
        return;
    }
    // Elixir modules are dotted (`MyApp.Repo`). Keep the full dotted path in the
    // qname (for uniqueness), but record the node's *name* as the last segment
    // (`Repo`) — that is how Elixir refers to the module after `alias MyApp.Repo`,
    // and it is the short name the graph's import tail fallback matches against.
    let short = name.rsplit('.').next().unwrap_or(&name).to_string();
    let qname = format!("{parent_qname}::{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::PACKAGE, &qname);

    // Elixir documentation is a module attribute in the body, not a comment
    // above the node: `@moduledoc` sits INSIDE the `do_block`, after the node
    // the PACKAGE is built from, so it is pre-scanned before the push.
    let body = do_block_of(node);
    let doc = body.map_or(DocChoice::Leading, |b| moduledoc_choice(b, src));
    acc.count_doc(&doc, true);
    acc.nodes.push(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells_with_doc(&node, src, file_rel, doc),
    });
    acc.edges.push(Edge {
        from: parent_id,
        to: id,
        category: edge_category::CONTAINS,
        confidence: Confidence::Strong,
        cells: Vec::new(),
    });
    acc.nav
        .record(id, &short, &qname, node_kind::PACKAGE, Some(parent_id));

    if let Some(body) = body {
        visit_module_body(body, src, file_rel, &qname, id, repo, acc);
    }
}

/// The `do_block` child of a `defmodule` / `defprotocol` call.
fn do_block_of(node: TsNode) -> Option<TsNode> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|c| c.kind() == "do_block")
}

/// Walk a module body in source order, carrying a pending `@doc` to the `def`
/// it documents. A `@doc` sets the pending doc; `def` / `defp` consumes it;
/// comments and other module attributes (`@spec`, `@impl`, `@decorate`,
/// `@moduledoc`, `@typedoc`, ...) keep it; any other statement clears it, so a
/// stray `@doc` never lands on an unrelated function. A nested `defmodule` is
/// such a statement, and its own body starts with nothing pending.
///
/// Multi-clause functions (`def get(1)` / `def get(n)`) share one qname and
/// `merge_parses` appends cells by id, so once an attribute has decided a
/// function's doc, its later clauses emit no DOC at all.
fn visit_module_body(
    body: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let mut pending: Option<DocAttr> = None;
    let mut decided: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut cursor = body.walk();
    for child in body.named_children(&mut cursor) {
        if child.kind() == "comment" {
            continue;
        }
        if let Some(attr) = doc_attr_text(child, src) {
            if matches!(attr, DocAttr::Doc(_) | DocAttr::Hidden) {
                pending = Some(attr);
            }
            continue;
        }
        if module_attribute(child) {
            continue;
        }
        if let Some(name) = def_call_name(child, src) {
            let doc = match pending.take() {
                Some(DocAttr::Doc(text)) => {
                    decided.insert(name);
                    DocChoice::Text(text)
                }
                Some(DocAttr::Hidden) => {
                    decided.insert(name);
                    DocChoice::Hidden
                }
                Some(DocAttr::ModuleDoc(_) | DocAttr::ModuleHidden) | None => {
                    if decided.contains(&name) {
                        DocChoice::Omit
                    } else {
                        DocChoice::Leading
                    }
                }
            };
            visit_def(child, src, file_rel, parent_qname, parent_id, repo, doc, acc);
            continue;
        }
        pending = None;
        visit_node(child, src, file_rel, parent_qname, parent_id, repo, acc);
    }
}

/// `doc` is decided by the caller: a module body passes the pending `@doc`
/// (authoritative text, or `Hidden` for `@doc false`); everywhere else passes
/// `Leading`, the comment-above fallback.
#[allow(clippy::too_many_arguments)]
fn visit_def(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    doc: DocChoice,
    acc: &mut Acc,
) {
    let Some(args) = find_args(node) else {
        return;
    };
    let name = extract_def_name(args, src);
    if name.is_empty() {
        return;
    }
    let qname = format!("{parent_qname}::{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::FUNCTION, &qname);

    acc.count_doc(&doc, false);
    acc.nodes.push(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells_with_doc(&node, src, file_rel, doc),
    });
    acc.edges.push(Edge {
        from: parent_id,
        to: id,
        category: edge_category::DEFINES,
        confidence: Confidence::Strong,
        cells: Vec::new(),
    });
    acc.nav
        .record(id, &name, &qname, node_kind::FUNCTION, Some(parent_id));

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "do_block" {
            collect_calls_in(child, src, id, acc);
        }
    }
    // Scan the WHOLE `def` call, not just its `do_block`: a one-liner
    // `def f(x), do: HTTPoison.get(url)` carries its body in the `do:` keyword
    // argument instead, and the head itself can never contain a client call.
    let hits = collect_client_endpoints_in(node, src, id, repo, file_rel, acc);
    acc.endpoint_hits += hits;
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
    let Some(args) = find_args(node) else {
        return;
    };
    let name = first_arg_text(args, src);
    if name.is_empty() {
        return;
    }
    let qname = format!("{parent_qname}::{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::INTERFACE, &qname);

    // Same `@moduledoc` pre-scan as `visit_defmodule`; the protocol body's
    // other children (function heads) are not walked.
    let doc = do_block_of(node).map_or(DocChoice::Leading, |b| moduledoc_choice(b, src));
    acc.count_doc(&doc, true);
    acc.nodes.push(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells_with_doc(&node, src, file_rel, doc),
    });
    acc.edges.push(Edge {
        from: parent_id,
        to: id,
        category: edge_category::DEFINES,
        confidence: Confidence::Strong,
        cells: Vec::new(),
    });
    acc.nav
        .record(id, &name, &qname, node_kind::INTERFACE, Some(parent_id));
}

fn visit_defstruct(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let qname = format!("{parent_qname}::__struct__");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::STRUCT, &qname);

    acc.nodes.push(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells(&node, src, file_rel),
    });
    acc.edges.push(Edge {
        from: parent_id,
        to: id,
        category: edge_category::DEFINES,
        confidence: Confidence::Strong,
        cells: Vec::new(),
    });
    acc.nav
        .record(id, "__struct__", &qname, node_kind::STRUCT, Some(parent_id));
}

fn collect_import(node: TsNode, src: &[u8], acc: &mut Acc) {
    let Some(args) = find_args(node) else {
        return;
    };
    // `import Plug.Conn` / `alias MyApp.Repo` / `use Phoenix.Controller`. The first
    // argument is the dotted module path; the graph's tail fallback splits it on
    // `.` and binds the final segment (`Conn`/`Repo`) to a unique in-repo module by
    // short name. External modules (e.g. `Plug.Conn`) have no in-repo target, so
    // they resolve to no edge — only in-repo aliases (e.g. an app's own `MyApp.Repo`)
    // produce an IMPORTS edge.
    let path = first_arg_text(args, src);
    if !path.is_empty() {
        acc.imports.push(ImportStmt {
            from_module: acc.module_qname.clone(),
            target: ImportTarget::Module {
                path,
                alias: None,
            },
            line: line_at(node),
        });
    }
}

fn first_arg_text(args: TsNode, src: &[u8]) -> String {
    let mut cursor = args.walk();
    for child in args.named_children(&mut cursor) {
        match child.kind() {
            "alias" | "identifier" | "atom" => {
                return text_of(child, src).trim().to_string();
            }
            _ => {
                let text = text_of(child, src).trim().to_string();
                if !text.is_empty() {
                    return text;
                }
            }
        }
    }
    String::new()
}

fn extract_def_name(args: TsNode, src: &[u8]) -> String {
    let mut cursor = args.walk();
    for child in args.named_children(&mut cursor) {
        match child.kind() {
            "identifier" => return text_of(child, src).to_string(),
            "call" => {
                if let Some(target) = child.child_by_field_name("target") {
                    return text_of(target, src).to_string();
                }
            }
            _ => {}
        }
    }
    String::new()
}

fn collect_calls_in(node: TsNode, src: &[u8], from: NodeId, acc: &mut Acc) {
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if n.kind() == "call"
            && let Some(target) = n.child_by_field_name("target")
        {
            match target.kind() {
                "identifier" => {
                    let name = text_of(target, src);
                    if !matches!(name, "def" | "defp" | "defmodule" | "defprotocol" | "defstruct" | "import" | "alias" | "use" | "if" | "case" | "cond" | "do" | "end") {
                        acc.calls.push(CallSite {
                            from,
                            qualifier: CallQualifier::Bare(name.to_string()),
                            line: line_at(n),
                        });
                    }
                }
                "dot" => {
                    let text = text_of(target, src);
                    if let Some(pos) = text.rfind('.') {
                        acc.calls.push(CallSite {
                            from,
                            qualifier: CallQualifier::Attribute {
                                base: text[..pos].to_string(),
                                name: text[pos + 1..].to_string(),
                            },
                            line: line_at(n),
                        });
                    }
                }
                _ => {}
            }
        }
        let mut cursor = n.walk();
        for child in n.named_children(&mut cursor) {
            if !matches!(child.kind(), "anonymous_function") {
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

// ---------------------------------------------------------------------------
// Client HTTP calls (HTTPoison / Tesla / Req / Finch) -> shared ENDPOINT nodes
// ---------------------------------------------------------------------------

/// Elixir HTTP client modules. Matched on the FIRST dotted segment, so
/// `HTTPoison.Base.get/1` counts as `HTTPoison`.
const ELIXIR_HTTP_CLIENTS: &[&str] = &["HTTPoison", "Tesla", "Req", "Finch"];

const ELIXIR_HTTP_VERBS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

/// Upper-case `s` if it names an HTTP verb. Tolerates the leading `:` of an
/// atom (`:get`) and the trailing `!`/`?` of a bang/query function (`get!`).
fn http_verb(s: &str) -> Option<String> {
    let up = s
        .trim()
        .trim_start_matches(':')
        .trim_end_matches(['!', '?'])
        .to_ascii_uppercase();
    ELIXIR_HTTP_VERBS.contains(&up.as_str()).then_some(up)
}

/// Reconstruct a `string` literal's text. An `#{…}` interpolation becomes
/// `${…}` so `normalise_http_path` collapses that segment exactly as it does
/// for a TypeScript template path. Returns `(text, had_interpolation)`.
fn string_text(node: TsNode, src: &[u8]) -> (String, bool) {
    let mut out = String::new();
    let mut interpolated = false;
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "interpolation" {
            out.push_str("${…}");
            interpolated = true;
        } else {
            out.push_str(text_of(child, src));
        }
    }
    (out, interpolated)
}

/// `(text, interpolated)` of the first DIRECT string-literal argument. Direct
/// only, so a string buried in a nested call argument is never mistaken for
/// the URL — and Tesla's client-first form `Tesla.get(client, "/x")` lands on
/// `"/x"` for free.
fn first_string_arg(args: TsNode, src: &[u8]) -> Option<(String, bool)> {
    let mut cursor = args.walk();
    args.named_children(&mut cursor)
        .find(|ch| ch.kind() == "string")
        .map(|ch| string_text(ch, src))
}

/// `(text, interpolated)` of `<key>: "…"` among the call's keyword arguments —
/// how `Req.get!(url: "…")` carries its URL.
fn keyword_string_arg(args: TsNode, key: &str, src: &[u8]) -> Option<(String, bool)> {
    let mut cursor = args.walk();
    for child in args.named_children(&mut cursor) {
        if child.kind() != "keywords" {
            continue;
        }
        let mut pairs = child.walk();
        for pair in child.named_children(&mut pairs) {
            let (Some(k), Some(v)) = (
                pair.child_by_field_name("key"),
                pair.child_by_field_name("value"),
            ) else {
                continue;
            };
            // The `keyword` node's text is `"url: "` — colon AND trailing
            // space — so trim the whitespace BEFORE the colon.
            if v.kind() == "string" && text_of(k, src).trim().trim_end_matches(':') == key {
                return Some(string_text(v, src));
            }
        }
    }
    None
}

/// `(verb, raw_url, interpolated)` for a `call` node that is an Elixir HTTP
/// client call, else None. Four shapes:
///   HTTPoison `HTTPoison.get("…")`            — verb is the function name
///   Tesla     `Tesla.post(client(), "…", b)`  — client-first, URL is arg 2
///   Req       `Req.get!(url: "…")`            — URL is the `url:` keyword
///   Finch     `Finch.build(:get, "…")`        — verb is the leading atom
fn client_call_candidate(n: TsNode, src: &[u8]) -> Option<(String, String, bool)> {
    let target = n.child_by_field_name("target")?;
    if target.kind() != "dot" {
        return None;
    }
    let module = text_of(target.child_by_field_name("left")?, src);
    let func = text_of(target.child_by_field_name("right")?, src);
    if !ELIXIR_HTTP_CLIENTS.contains(&module.split('.').next().unwrap_or(module)) {
        return None;
    }
    let args = find_args(n)?;
    if func == "build" {
        // `Finch.build(:get, url, …)` — the verb is the leading atom argument.
        let mut cursor = args.walk();
        let atom = args
            .named_children(&mut cursor)
            .find(|ch| ch.kind() == "atom")?;
        let verb = http_verb(text_of(atom, src))?;
        let (raw, interpolated) = first_string_arg(args, src)?;
        return Some((verb, raw, interpolated));
    }
    let verb = http_verb(func)?;
    let (raw, interpolated) =
        first_string_arg(args, src).or_else(|| keyword_string_arg(args, "url", src))?;
    Some((verb, raw, interpolated))
}

/// Outbound HTTP call sites inside a `def`/`defp` become shared ENDPOINT nodes
/// (+ a CALLS edge from the enclosing function) so `HttpStackResolver` can pair
/// them with a server ROUTE. Returns the number of call sites emitted.
fn collect_client_endpoints_in(
    body: TsNode,
    src: &[u8],
    from: NodeId,
    repo: RepoId,
    file_rel: &str,
    acc: &mut Acc,
) -> usize {
    let mut hits = 0;
    let mut stack = vec![body];
    while let Some(n) = stack.pop() {
        if n.kind() == "call"
            && let Some((method, raw, interpolated)) = client_call_candidate(n, src)
            && let Some(path) = url_to_path(&raw)
        {
            let pos = n.start_position();
            // An interpolated path is Medium (the concrete segment is unknown);
            // a plain literal is Strong — the same rule scala/swift use.
            let ep = ClientEndpoint {
                method,
                path,
                file: file_rel.to_string(),
                line: pos.row + 1,
                col: pos.column + 1,
                confidence: if interpolated {
                    Confidence::Medium
                } else {
                    Confidence::Strong
                },
            };
            push_client_endpoint(
                repo,
                &ep,
                from,
                &mut acc.nodes,
                &mut acc.edges,
                &mut acc.nav,
                &mut acc.endpoint_seen,
            );
            hits += 1;
        }
        let mut cursor = n.walk();
        for child in n.named_children(&mut cursor) {
            stack.push(child);
        }
    }
    hits
}

fn scan_phoenix_routes(source: &str, repo: RepoId, module_id: NodeId, acc: &mut Acc) {
    let bytes = source.as_bytes();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut scope_stack: Vec<String> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        // Match word at i, then dispatch.
        if !is_word_start(bytes, i) {
            i += 1;
            continue;
        }
        let word_end = word_end(bytes, i);
        let word = &source[i..word_end];
        match word {
            "scope" => {
                let rest = &source[word_end..];
                let prefix = first_quoted(rest).unwrap_or_default();
                scope_stack.push(prefix);
                // Advance past the end of the line or next do (rough).
                if let Some(off) = rest.find(" do") {
                    i = word_end + off + 3;
                    continue;
                }
                i = word_end;
            }
            "end" => {
                // End *might* close a scope — we can't tell precisely without a full parse.
                // Pop lazily: only if non-empty.
                if !scope_stack.is_empty() {
                    scope_stack.pop();
                }
                i = word_end;
            }
            "get" | "post" | "put" | "patch" | "delete" | "head" | "options" => {
                let method = word.to_ascii_uppercase();
                let rest = &source[word_end..];
                if let Some(path) = first_quoted(rest) {
                    let full = join_scope(&scope_stack, &path);
                    // Handler action is the last atom on the route line, e.g.
                    // `get "/users", UserController, :index` -> `index`.
                    let action = route_action(rest);
                    let route_name = format!("{method} {full}");
                    if seen.insert(route_name.clone()) {
                        emit_phoenix_route(
                            &method,
                            &full,
                            action.as_deref(),
                            repo,
                            module_id,
                            line_of(source, i),
                            acc,
                        );
                    }
                }
                i = word_end;
            }
            "resources" => {
                let rest = &source[word_end..];
                if let Some(path) = first_quoted(rest) {
                    let full = join_scope(&scope_stack, &path);
                    for m in ["GET", "POST", "PUT", "PATCH", "DELETE"] {
                        let route_name = format!("{m} {full}");
                        if seen.insert(route_name.clone()) {
                            // `resources` derives standard RESTful actions from the
                            // controller implicitly; no explicit action to link.
                            emit_phoenix_route(m, &full, None, repo, module_id, line_of(source, i), acc);
                        }
                    }
                }
                i = word_end;
            }
            _ => {
                i = word_end;
            }
        }
    }
}

fn is_word_start(bytes: &[u8], i: usize) -> bool {
    let c = bytes[i];
    if !(c.is_ascii_alphabetic() || c == b'_') {
        return false;
    }
    if i == 0 {
        return true;
    }
    let p = bytes[i - 1];
    !(p.is_ascii_alphanumeric() || p == b'_' || p == b'.' || p == b':')
}

fn word_end(bytes: &[u8], start: usize) -> usize {
    let mut i = start;
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
        i += 1;
    }
    i
}

fn first_quoted(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'\n' {
            return None;
        }
        if c == b'"' {
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j] != b'"' {
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

/// `line` is the route line's 0-based row: the site of its HANDLED_BY ref
/// (LC.3b).
fn emit_phoenix_route(
    method: &str,
    path: &str,
    action: Option<&str>,
    repo: RepoId,
    module_id: NodeId,
    line: u32,
    acc: &mut Acc,
) {
    let route_name = format!("{method} {path}");
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

    // HANDLED_BY: link the ROUTE to its controller action function. The action
    // is a bare `def` in the controller module; resolve_refs binds it via the
    // global-by-name fallback (Elixir modules are MODULE/PACKAGE nodes, not
    // CLASS, so the Attribute/class-method path can't resolve them).
    if let Some(action) = action {
        if !action.is_empty() {
            acc.refs.push(UnresolvedRef {
                from: route_id,
                from_module: module_id,
                qualifier: CallQualifier::Bare(action.to_string()),
                category: edge_category::HANDLED_BY,
                line,
            });
        }
    }
}

/// Extract the controller action from a Phoenix route line. Given the text
/// following the verb (e.g. `"/users", UserController, :index`), returns the
/// trailing atom's name (`index`). Returns `None` when no `:atom` action is
/// present on the same line (only scans up to the first newline).
fn route_action(rest: &str) -> Option<String> {
    let line = rest.split('\n').next().unwrap_or(rest);
    let bytes = line.as_bytes();
    // Skip past the quoted path so a `:param` inside it (e.g. "/users/:id")
    // isn't mistaken for the action atom.
    let mut i = 0;
    while i < bytes.len() && bytes[i] != b'"' {
        i += 1;
    }
    if i < bytes.len() {
        i += 1; // opening quote
        while i < bytes.len() && bytes[i] != b'"' {
            if bytes[i] == b'\\' && i + 1 < bytes.len() {
                i += 2;
            } else {
                i += 1;
            }
        }
        if i < bytes.len() {
            i += 1; // closing quote
        }
    }
    let mut last: Option<String> = None;
    while i < bytes.len() {
        if bytes[i] == b':' && i + 1 < bytes.len() {
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            if j > start {
                last = Some(line[start..j].to_string());
            }
            i = j;
        } else {
            i += 1;
        }
    }
    last
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

fn entity_cells(node: &TsNode, src: &[u8], file_rel: &str) -> Vec<Cell> {
    entity_cells_with_doc(node, src, file_rel, DocChoice::Leading)
}

/// CODE + POSITION, plus the DOC cell `doc` selects.
fn entity_cells_with_doc(node: &TsNode, src: &[u8], file_rel: &str, doc: DocChoice) -> Vec<Cell> {
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
    let text = match doc {
        DocChoice::Leading => glia_doc::leading_doc(node, src),
        DocChoice::Text(t) if !t.is_empty() => Some(t),
        DocChoice::Text(_) | DocChoice::Hidden | DocChoice::Omit => None,
    };
    if let Some(doc) = text {
        cells.push(Cell {
            kind: cell_type::DOC,
            payload: CellPayload::Text(doc),
        });
    }
    cells
}

// ---------------------------------------------------------------------------
// Documentation attributes (`@doc` / `@moduledoc`) -> DOC cells (LA.7a)
// ---------------------------------------------------------------------------
//
// Elixir documentation is not a comment: it is a module attribute in the body,
// which `glia_doc::leading_doc` (a preceding-comment walk) never sees.
// tree-sitter-elixir 0.3 parses `@doc "x"` as
// `unary_operator(operator: "@", operand: call(target: identifier "doc",
// arguments(string | sigil | boolean)))`.

/// One documentation attribute, as written.
#[derive(Debug, Clone, PartialEq, Eq)]
enum DocAttr {
    /// `@doc "..."` / heredoc / `~S"""..."""`, cleaned.
    Doc(String),
    /// `@moduledoc "..."`, cleaned.
    ModuleDoc(String),
    /// `@doc false`: the author hid the function.
    Hidden,
    /// `@moduledoc false`: the author hid the module.
    ModuleHidden,
}

/// Where a node's DOC cell comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
enum DocChoice {
    /// No attribute decided it: the leading-comment walk (comment-only defs).
    Leading,
    /// An attribute's text, authoritative over any comment. Empty = no DOC.
    Text(String),
    /// `@doc false` / `@moduledoc false`: no DOC, not even from a comment.
    Hidden,
    /// A later clause of a function whose doc an attribute already decided on
    /// the first clause: no DOC, so the merged node carries exactly one.
    Omit,
}

impl Acc {
    /// Tally an attached / hidden doc for the fired-on marker.
    fn count_doc(&mut self, doc: &DocChoice, module: bool) {
        match doc {
            DocChoice::Text(t) if !t.is_empty() => {
                if module {
                    self.doc_moduledoc += 1;
                } else {
                    self.doc_attached += 1;
                }
            }
            DocChoice::Hidden => self.doc_hidden += 1,
            DocChoice::Leading | DocChoice::Text(_) | DocChoice::Omit => {}
        }
    }
}

/// The `call` operand of a module attribute (`@name ...`), if `node` is one.
fn attribute_call(node: TsNode) -> Option<TsNode> {
    if node.kind() != "unary_operator" {
        return None;
    }
    let op = node.child_by_field_name("operator")?;
    if op.kind() != "@" {
        return None;
    }
    let operand = node.child_by_field_name("operand")?;
    (operand.kind() == "call").then_some(operand)
}

/// Any `@attr ...` statement (`@spec`, `@impl`, `@decorate`, `@typedoc`, ...).
fn module_attribute(node: TsNode) -> bool {
    attribute_call(node).is_some()
}

/// A `@doc` / `@moduledoc` attribute's value. `None` for other attributes and
/// for doc forms that carry no text (`@doc since: "1.0"` metadata).
fn doc_attr_text(node: TsNode, src: &[u8]) -> Option<DocAttr> {
    let call = attribute_call(node)?;
    let target = call.child_by_field_name("target")?;
    if target.kind() != "identifier" {
        return None;
    }
    let module = match text_of(target, src) {
        "doc" => false,
        "moduledoc" => true,
        _ => return None,
    };
    let args = find_args(call)?;
    let mut cursor = args.walk();
    let value = args.named_children(&mut cursor).next()?;
    match value.kind() {
        "string" | "sigil" => {
            let text = clean_doc(&quoted_text(value, src));
            Some(if module {
                DocAttr::ModuleDoc(text)
            } else {
                DocAttr::Doc(text)
            })
        }
        "boolean" if text_of(value, src) == "false" => Some(if module {
            DocAttr::ModuleHidden
        } else {
            DocAttr::Hidden
        }),
        _ => None,
    }
}

/// The literal text of a `string` / `sigil` (single-line or `"""` heredoc):
/// its `quoted_content` runs, escapes decoded, interpolations kept verbatim.
fn quoted_text(node: TsNode, src: &[u8]) -> String {
    let mut out = String::new();
    let mut cursor = node.walk();
    for part in node.named_children(&mut cursor) {
        match part.kind() {
            "quoted_content" | "interpolation" => out.push_str(text_of(part, src)),
            "escape_sequence" => {
                let esc = text_of(part, src);
                match esc {
                    "\\n" | "\\t" | "\\r" => out.push(' '),
                    _ => match esc.strip_prefix('\\') {
                        Some(rest) if rest.chars().count() == 1 => out.push_str(rest),
                        _ => out.push_str(esc),
                    },
                }
            }
            _ => {}
        }
    }
    out
}

/// Dedent, trim and collapse a doc body to one line (the shape
/// `glia_doc::leading_doc` produces: lines joined by single spaces), then
/// cap at [`glia_doc::DOC_MAX`] on a char boundary. Collapsing every
/// whitespace run subsumes the heredoc dedent: each line's common indent goes
/// with the rest of its leading whitespace.
fn clean_doc(raw: &str) -> String {
    let joined = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.len() <= glia_doc::DOC_MAX {
        return joined;
    }
    let mut end = glia_doc::DOC_MAX;
    while !joined.is_char_boundary(end) {
        end -= 1;
    }
    joined[..end].trim_end().to_string()
}

/// The function name of a `def` / `defp` call, if `node` is one.
fn def_call_name(node: TsNode, src: &[u8]) -> Option<String> {
    if node.kind() != "call" {
        return None;
    }
    let target = node.child_by_field_name("target")?;
    if !matches!(text_of(target, src), "def" | "defp") {
        return None;
    }
    let name = extract_def_name(find_args(node)?, src);
    (!name.is_empty()).then_some(name)
}

/// The DOC choice for a `defmodule` / `defprotocol` node, from the
/// `@moduledoc` among its body's direct children (the last one wins, as in
/// Elixir). No `@moduledoc` keeps the leading-comment fallback. Nested module
/// bodies are not entered, so their `@moduledoc` never leaks outward.
fn moduledoc_choice(body: TsNode, src: &[u8]) -> DocChoice {
    let mut choice = DocChoice::Leading;
    let mut cursor = body.walk();
    for child in body.named_children(&mut cursor) {
        match doc_attr_text(child, src) {
            Some(DocAttr::ModuleDoc(text)) => choice = DocChoice::Text(text),
            Some(DocAttr::ModuleHidden) => choice = DocChoice::Hidden,
            Some(DocAttr::Doc(_) | DocAttr::Hidden) | None => {}
        }
    }
    choice
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> RepoId {
        RepoId(1)
    }

    /// Every DOC text on the node(s) named `name` of `kind`, across all the
    /// `Node` entries sharing its id (multi-clause functions push one per clause).
    fn docs_of(fp: &FileParse, kind: glia_core::NodeKindId, name: &str) -> Vec<String> {
        let ids: Vec<NodeId> = fp
            .nav
            .name_by_id
            .iter()
            .filter(|(id, n)| n.as_str() == name && fp.nav.kind_by_id.get(*id) == Some(&kind))
            .map(|(id, _)| *id)
            .collect();
        assert!(!ids.is_empty(), "no {kind:?} node named {name}");
        fp.nodes
            .iter()
            .filter(|n| ids.contains(&n.id))
            .flat_map(|n| n.cells.iter())
            .filter(|c| c.kind == cell_type::DOC)
            .filter_map(|c| match &c.payload {
                CellPayload::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    /// The LA.7a fixture file (bench/substrate-gap/fixtures/elixir-docs).
    const ACCOUNTS: &str = r#"defmodule MyApp.Accounts do
  @moduledoc """
  Account management context.
  """

  @doc """
  Fetches a user by id.
  """
  @spec get_user(integer) :: map
  def get_user(id), do: %{id: id}

  @doc "Creates a user."
  def create_user(attrs), do: attrs

  # Hidden helper comment.
  @doc false
  def internal(x), do: x

  # Plain comment above.
  def comment_only(x), do: x
end
"#;

    #[test]
    fn doc_heredoc_attaches_through_spec() {
        let fp = parse_file(ACCOUNTS, "lib/accounts.ex", "lib::accounts", repo()).unwrap();
        assert_eq!(docs_of(&fp, node_kind::FUNCTION, "get_user"), vec!["Fetches a user by id."]);
    }

    #[test]
    fn doc_single_line_string() {
        let fp = parse_file(ACCOUNTS, "lib/accounts.ex", "lib::accounts", repo()).unwrap();
        assert_eq!(docs_of(&fp, node_kind::FUNCTION, "create_user"), vec!["Creates a user."]);
    }

    #[test]
    fn moduledoc_on_package() {
        let fp = parse_file(ACCOUNTS, "lib/accounts.ex", "lib::accounts", repo()).unwrap();
        assert_eq!(
            docs_of(&fp, node_kind::PACKAGE, "Accounts"),
            vec!["Account management context."]
        );
    }

    #[test]
    fn doc_false_hides_even_the_comment_above() {
        let fp = parse_file(ACCOUNTS, "lib/accounts.ex", "lib::accounts", repo()).unwrap();
        assert!(docs_of(&fp, node_kind::FUNCTION, "internal").is_empty());
        // Control: the leading-comment path still documents a comment-only def.
        assert_eq!(docs_of(&fp, node_kind::FUNCTION, "comment_only"), vec!["Plain comment above."]);

        // A comment BETWEEN `@doc false` and the def is directly above the def,
        // so `leading_doc` would pick it up; `@doc false` must still win.
        let source = r#"
defmodule M do
  @doc false
  # Would be picked up by the comment walk.
  def hidden(x), do: x
end
"#;
        let fp = parse_file(source, "lib/m.ex", "lib::m", repo()).unwrap();
        assert!(docs_of(&fp, node_kind::FUNCTION, "hidden").is_empty());
    }

    #[test]
    fn moduledoc_false_hides_package_comment() {
        let source = r#"
# Comment above the module.
defmodule M do
  @moduledoc false
  def f(x), do: x
end
"#;
        let fp = parse_file(source, "lib/m.ex", "lib::m", repo()).unwrap();
        assert!(docs_of(&fp, node_kind::PACKAGE, "M").is_empty());
    }

    #[test]
    fn doc_survives_spec_impl_and_comments() {
        let source = r#"
defmodule M do
  @doc "Handles a call."
  # a comment between
  @impl true
  @spec handle(term) :: :ok
  def handle(_), do: :ok
end
"#;
        let fp = parse_file(source, "lib/m.ex", "lib::m", repo()).unwrap();
        assert_eq!(docs_of(&fp, node_kind::FUNCTION, "handle"), vec!["Handles a call."]);
    }

    #[test]
    fn stray_doc_before_a_statement_attaches_to_nothing() {
        let source = r#"
defmodule M do
  @doc "Stray."
  alias Foo.Bar
  def after_alias(x), do: x

  @doc "Also stray."
  defmodule Inner do
    def inner(x), do: x
  end

  def after_inner(x), do: x
end
"#;
        let fp = parse_file(source, "lib/m.ex", "lib::m", repo()).unwrap();
        assert!(docs_of(&fp, node_kind::FUNCTION, "after_alias").is_empty());
        assert!(docs_of(&fp, node_kind::FUNCTION, "inner").is_empty());
        assert!(docs_of(&fp, node_kind::FUNCTION, "after_inner").is_empty());
        assert!(docs_of(&fp, node_kind::PACKAGE, "Inner").is_empty());
    }

    #[test]
    fn multi_clause_function_gets_one_doc() {
        let source = r#"
defmodule M do
  @doc "Gets a thing."
  def get(1), do: :one
  # Second clause comment.
  def get(n), do: n
end
"#;
        let fp = parse_file(source, "lib/m.ex", "lib::m", repo()).unwrap();
        assert_eq!(docs_of(&fp, node_kind::FUNCTION, "get"), vec!["Gets a thing."]);
    }

    #[test]
    fn nested_module_moduledoc_stays_on_its_own_package() {
        let source = r#"
defmodule Outer do
  defmodule Inner do
    @moduledoc "Inner docs."
  end
end
"#;
        let fp = parse_file(source, "lib/o.ex", "lib::o", repo()).unwrap();
        assert!(docs_of(&fp, node_kind::PACKAGE, "Outer").is_empty());
        assert_eq!(docs_of(&fp, node_kind::PACKAGE, "Inner"), vec!["Inner docs."]);
    }

    #[test]
    fn protocol_moduledoc_on_interface() {
        let source = r#"
defprotocol Size do
  @moduledoc """
  Computes the size of a data structure.
  """
  def size(data)
end
"#;
        let fp = parse_file(source, "lib/size.ex", "lib::size", repo()).unwrap();
        assert_eq!(
            docs_of(&fp, node_kind::INTERFACE, "Size"),
            vec!["Computes the size of a data structure."]
        );
    }

    #[test]
    fn sigil_heredoc_escapes_and_dedent() {
        let source = r#"
defmodule M do
  @doc ~S"""
      Indented   first line.
        Deeper second line, café.
  """
  def a(x), do: x

  @doc "Say \"hi\" to #{name}."
  def b(name), do: name
end
"#;
        let fp = parse_file(source, "lib/m.ex", "lib::m", repo()).unwrap();
        assert_eq!(
            docs_of(&fp, node_kind::FUNCTION, "a"),
            vec!["Indented first line. Deeper second line, café."]
        );
        assert_eq!(docs_of(&fp, node_kind::FUNCTION, "b"), vec!["Say \"hi\" to #{name}."]);
    }

    #[test]
    fn long_utf8_doc_capped_on_char_boundary() {
        let body = "é".repeat(glia_doc::DOC_MAX);
        let source = format!("defmodule M do\n  @doc \"{body}\"\n  def a(x), do: x\nend\n");
        let fp = parse_file(&source, "lib/m.ex", "lib::m", repo()).unwrap();
        let docs = docs_of(&fp, node_kind::FUNCTION, "a");
        assert_eq!(docs.len(), 1);
        assert!(docs[0].len() <= glia_doc::DOC_MAX);
        assert!(docs[0].chars().all(|c| c == 'é'));
        assert_eq!(docs[0].chars().count(), glia_doc::DOC_MAX / 2);
    }

    #[test]
    fn doc_metadata_keyword_form_is_not_text() {
        // `@doc since: "1.0"` carries metadata, not text: it neither documents
        // the def nor clears the real `@doc` above it.
        let source = r#"
defmodule M do
  @doc "Real doc."
  @doc since: "1.0"
  def a(x), do: x
end
"#;
        let fp = parse_file(source, "lib/m.ex", "lib::m", repo()).unwrap();
        assert_eq!(docs_of(&fp, node_kind::FUNCTION, "a"), vec!["Real doc."]);
    }

    #[test]
    fn doc_attr_counters_match_the_fixture_marker() {
        let lang: tree_sitter::Language = tree_sitter_elixir::LANGUAGE.into();
        let mut parser = Parser::new();
        parser.set_language(&lang).unwrap();
        let tree = parser.parse(ACCOUNTS, None).unwrap();
        let mut acc = Acc::default();
        let module_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "lib::accounts");
        visit_top(
            tree.root_node(),
            ACCOUNTS.as_bytes(),
            "lib/accounts.ex",
            "lib::accounts",
            module_id,
            repo(),
            &mut acc,
        );
        assert_eq!(
            (acc.doc_attached, acc.doc_moduledoc, acc.doc_hidden),
            (2, 1, 1),
            "marker: [doc] elixir attrs attached=2 moduledoc=1 hidden=1"
        );
    }

    #[test]
    fn module_and_functions() {
        let source = r#"
defmodule MyApp.Users do
  def get_user(id) do
    Repo.get(User, id)
  end

  defp validate(user) do
    :ok
  end
end
"#;
        let fp = parse_file(source, "lib/users.ex", "lib::users", repo()).unwrap();
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::PACKAGE).count(), 1);
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::FUNCTION).count(), 2);
    }

    #[test]
    fn imports() {
        let source = r#"
defmodule MyApp.Web do
  import Plug.Conn
  alias MyApp.Repo
  use Phoenix.Controller
end
"#;
        let fp = parse_file(source, "lib/web.ex", "lib::web", repo()).unwrap();
        assert_eq!(fp.imports.len(), 3);

        // Regression: `import`/`alias`/`use` live *inside* the `defmodule`, but the
        // emitted `from_module` must be the file-stem MODULE qname (`lib::web`) — the
        // key the graph registers in `module_by_qname` — NOT the enclosing PACKAGE
        // qname (`lib::web::MyApp.Web`). Otherwise `resolve_imports_python`
        // early-continues and no elixir import ever resolves.
        assert!(
            fp.imports.iter().all(|i| i.from_module == "lib::web"),
            "from_module must be the file MODULE qname, got: {:?}",
            fp.imports.iter().map(|i| &i.from_module).collect::<Vec<_>>()
        );

        // The dotted alias path is preserved so the graph's tail fallback can split
        // it to the short module name it binds against.
        let paths: Vec<&str> = fp
            .imports
            .iter()
            .filter_map(|i| match &i.target {
                ImportTarget::Module { path, .. } => Some(path.as_str()),
                _ => None,
            })
            .collect();
        assert!(paths.contains(&"Plug.Conn"), "paths: {paths:?}");
        assert!(paths.contains(&"MyApp.Repo"), "paths: {paths:?}");
    }

    #[test]
    fn defmodule_recorded_by_short_name() {
        // `defmodule MyApp.Repo` records short name `Repo` (how Elixir refers to it
        // after `alias MyApp.Repo`) so the graph's import tail fallback binds an
        // in-repo alias by its unique short name — while the full dotted path stays
        // in the qname for uniqueness.
        let source = r#"
defmodule MyApp.Repo do
  def all(q), do: q
end
"#;
        let fp = parse_file(source, "lib/repo.ex", "lib::repo", repo()).unwrap();
        let pkg = fp
            .nav
            .name_by_id
            .iter()
            .find(|(id, _)| fp.nav.kind_by_id.get(*id) == Some(&node_kind::PACKAGE))
            .expect("defmodule package node");
        assert_eq!(pkg.1, "Repo", "short name should be the last dotted segment");
        assert_eq!(
            fp.nav.qname_by_id.get(pkg.0).map(String::as_str),
            Some("lib::repo::MyApp.Repo"),
            "qname keeps the full dotted path"
        );
    }

    #[test]
    fn phoenix_routes_basic() {
        let source = r#"
defmodule MyAppWeb.Router do
  use MyAppWeb, :router

  scope "/api", MyAppWeb do
    get "/users", UserController, :index
    post "/users", UserController, :create
    put "/users/:id", UserController, :update
    delete "/users/:id", UserController, :delete
    resources "/posts", PostController
  end
end
"#;
        let fp = parse_file(source, "lib/router.ex", "lib::router", repo()).unwrap();
        let route_names: Vec<&str> = fp
            .nav
            .name_by_id
            .iter()
            .filter(|(id, _)| fp.nav.kind_by_id.get(*id) == Some(&node_kind::ROUTE))
            .map(|(_, n)| n.as_str())
            .collect();
        assert!(route_names.contains(&"GET /api/users"));
        assert!(route_names.contains(&"POST /api/users"));
        assert!(route_names.contains(&"PUT /api/users/:id"));
        assert!(route_names.contains(&"DELETE /api/users/:id"));
        assert!(route_names.contains(&"GET /api/posts"));
        assert!(route_names.contains(&"POST /api/posts"));
    }

    #[test]
    fn calls_detected() {
        let source = r#"
defmodule MyApp.Service do
  def run(data) do
    validate(data)
    Repo.insert(data)
  end
end
"#;
        let fp = parse_file(source, "lib/service.ex", "lib::service", repo()).unwrap();
        assert!(fp.calls.iter().any(|c| matches!(&c.qualifier, CallQualifier::Bare(n) if n == "validate")));
        assert!(fp.calls.iter().any(|c| matches!(&c.qualifier, CallQualifier::Attribute { base, name } if base == "Repo" && name == "insert")));
    }

    #[test]
    fn handled_by_refs_link_route_to_action() {
        let source = r#"
defmodule MyAppWeb.Router do
  use MyAppWeb, :router

  scope "/api", MyAppWeb do
    get "/users", UserController, :index
    post "/users", UserController, :create
    put "/users/:id", UserController, :update
  end
end
"#;
        let fp = parse_file(source, "lib/router.ex", "lib::router", repo()).unwrap();

        // One HANDLED_BY ref per explicit-verb route, qualifier = Bare(action).
        let actions: Vec<&str> = fp
            .refs
            .iter()
            .filter(|r| r.category == edge_category::HANDLED_BY)
            .filter_map(|r| match &r.qualifier {
                CallQualifier::Bare(n) => Some(n.as_str()),
                _ => None,
            })
            .collect();
        assert!(actions.contains(&"index"), "actions: {actions:?}");
        assert!(actions.contains(&"create"), "actions: {actions:?}");
        // `:id` inside the path must not be captured as the action.
        assert!(actions.contains(&"update"), "actions: {actions:?}");
        assert!(!actions.contains(&"id"), "path param leaked: {actions:?}");

        // The ref's `from` must be the ROUTE node id (GET /api/users).
        let route_id = fp
            .nav
            .name_by_id
            .iter()
            .find(|(id, n)| {
                n.as_str() == "GET /api/users"
                    && fp.nav.kind_by_id.get(*id) == Some(&node_kind::ROUTE)
            })
            .map(|(id, _)| *id)
            .expect("route node exists");
        assert!(
            fp.refs.iter().any(|r| r.from == route_id
                && r.category == edge_category::HANDLED_BY
                && matches!(&r.qualifier, CallQualifier::Bare(n) if n == "index")),
            "expected HANDLED_BY from GET /api/users -> index"
        );
    }

    // --- client HTTP calls (HTTPoison / Tesla / Req / Finch) -----------------

    /// Every ENDPOINT display name the parse produced, sorted.
    fn endpoint_names(fp: &FileParse) -> Vec<String> {
        let mut v: Vec<String> = fp
            .nav
            .name_by_id
            .iter()
            .filter(|(id, _)| fp.nav.kind_by_id.get(*id) == Some(&node_kind::ENDPOINT))
            .map(|(_, n)| n.clone())
            .collect();
        v.sort();
        v
    }

    /// True if a CALLS edge runs from the FUNCTION named `from` to the ENDPOINT
    /// named `to`.
    fn has_calls_edge(fp: &FileParse, from: &str, to: &str) -> bool {
        let id_of = |name: &str, kind| {
            fp.nav
                .name_by_id
                .iter()
                .find(|(id, n)| n.as_str() == name && fp.nav.kind_by_id.get(*id) == Some(&kind))
                .map(|(id, _)| *id)
        };
        let (Some(f), Some(t)) = (
            id_of(from, node_kind::FUNCTION),
            id_of(to, node_kind::ENDPOINT),
        ) else {
            return false;
        };
        fp.edges
            .iter()
            .any(|e| e.from == f && e.to == t && e.category == edge_category::CALLS)
    }

    #[test]
    fn httpoison_get_emits_endpoint() {
        let source = r#"
defmodule ApiClient do
  def fetch_user(id) do
    HTTPoison.get("http://users-svc/api/users/#{id}")
  end
end
"#;
        let fp = parse_file(source, "lib/api_client.ex", "lib::api_client", repo()).unwrap();
        // Host stripped by `url_to_path`; `#{id}` reconstructed as `${…}` so the
        // graph's `normalise_http_path` collapses it to `{}` and it pairs with a
        // server route `/api/users/{id}`.
        assert_eq!(endpoint_names(&fp), vec!["GET /api/users/${…}".to_string()]);
        assert!(
            has_calls_edge(&fp, "fetch_user", "GET /api/users/${…}"),
            "expected CALLS from the enclosing def to the ENDPOINT"
        );
        // An interpolated path is Medium — the concrete segment is unknown.
        let ep = fp
            .nodes
            .iter()
            .find(|n| fp.nav.kind_by_id.get(&n.id) == Some(&node_kind::ENDPOINT))
            .expect("endpoint node");
        assert_eq!(ep.confidence, Confidence::Medium);
        assert!(
            ep.cells.iter().any(|c| c.kind == cell_type::ENDPOINT_HIT),
            "ENDPOINT must carry an ENDPOINT_HIT cell"
        );
    }

    #[test]
    fn tesla_client_first_arg_form_emits_endpoint() {
        let source = r#"
defmodule ApiClient do
  def create_user(body) do
    Tesla.post(client(), "/api/users", body)
  end

  defp client, do: Tesla.client([])
end
"#;
        let fp = parse_file(source, "lib/api_client.ex", "lib::api_client", repo()).unwrap();
        // The URL is the SECOND argument: `first_string_arg` scans DIRECT
        // arguments for the first string literal, so `client()` is skipped.
        assert_eq!(endpoint_names(&fp), vec!["POST /api/users".to_string()]);
        assert!(has_calls_edge(&fp, "create_user", "POST /api/users"));
        // `Tesla.client([])` is not a verb -> no endpoint from the private helper.
        let ep = fp
            .nodes
            .iter()
            .find(|n| fp.nav.kind_by_id.get(&n.id) == Some(&node_kind::ENDPOINT))
            .expect("endpoint node");
        assert_eq!(ep.confidence, Confidence::Strong);
    }

    #[test]
    fn req_keyword_url_and_finch_atom_verb_emit_endpoints() {
        let source = r#"
defmodule ApiClient do
  def list_orders do
    Req.get!(url: "http://orders-svc/api/orders")
  end

  def remove_user(id) do
    Finch.build(:delete, "http://users-svc/api/users/#{id}")
  end
end
"#;
        let fp = parse_file(source, "lib/api_client.ex", "lib::api_client", repo()).unwrap();
        assert_eq!(
            endpoint_names(&fp),
            vec![
                "DELETE /api/users/${…}".to_string(),
                "GET /api/orders".to_string(),
            ]
        );
        // Req's URL rides a `url:` keyword, whose `keyword` node text is
        // `"url: "` — colon and trailing space.
        assert!(has_calls_edge(&fp, "list_orders", "GET /api/orders"));
        // Finch's verb is the leading atom, not the function name (`build`).
        assert!(has_calls_edge(&fp, "remove_user", "DELETE /api/users/${…}"));
    }

    #[test]
    fn elixir_client_calls_emit_no_route() {
        // `scan_phoenix_routes` is a raw-source WORD scanner: a bare `get "/x"`
        // mints a ROUTE. Every client call here is qualified (`HTTPoison.get`),
        // and `is_word_start` rejects a word preceded by `.` or `:`, so none of
        // them may be mistaken for a Phoenix route declaration.
        let source = r#"
defmodule ApiClient do
  def fetch_user(id) do
    HTTPoison.get("http://users-svc/api/users/#{id}")
  end

  def create_user(body) do
    Tesla.post(client(), "/api/users", body)
  end

  def list_orders do
    Req.get!(url: "http://orders-svc/api/orders")
  end

  def remove_user(id) do
    Finch.build(:delete, "http://users-svc/api/users/#{id}")
  end

  defp client, do: Tesla.client([])
end
"#;
        let fp = parse_file(source, "lib/api_client.ex", "lib::api_client", repo()).unwrap();
        let routes: Vec<&String> = fp
            .nav
            .name_by_id
            .iter()
            .filter(|(id, _)| fp.nav.kind_by_id.get(*id) == Some(&node_kind::ROUTE))
            .map(|(_, n)| n)
            .collect();
        assert!(routes.is_empty(), "phantom ROUTEs minted: {routes:?}");
        assert_eq!(endpoint_names(&fp).len(), 4, "{:?}", endpoint_names(&fp));
    }

    #[test]
    fn elixir_non_url_string_is_dropped() {
        let source = r#"
defmodule ApiClient do
  # Non-HTTP receiver: `Cache` is not a known client module.
  def cached(id), do: Cache.get("user." <> id)

  # Known client module, but the argument is not a URL path.
  def bad_url, do: HTTPoison.get("users-svc")

  # Known client module, unknown verb.
  def started, do: HTTPoison.start()
end
"#;
        let fp = parse_file(source, "lib/api_client.ex", "lib::api_client", repo()).unwrap();
        assert!(
            endpoint_names(&fp).is_empty(),
            "no endpoint may be minted: {:?}",
            endpoint_names(&fp)
        );
    }
}
