use std::collections::HashSet;

use repo_graph_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};
use repo_graph_doc::DocTag;
use tree_sitter::{Node as TsNode, Parser};

pub use repo_graph_code_domain::{
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
    let lang: tree_sitter::Language = tree_sitter_solidity::LANGUAGE.into();
    parser
        .set_language(&lang)
        .map_err(|e| ParseError::LanguageInit(e.to_string()))?;
    let tree = parser.parse(source, None).ok_or(ParseError::NoTree)?;
    let src = source.as_bytes();
    let root = tree.root_node();

    let mut acc = Acc::default();

    // Pre-pass: collect names of interfaces declared in this file so the
    // inheritance handler can distinguish IMPLEMENTS (→ interface) from
    // INHERITS_FROM (→ base contract).
    let local_interfaces = collect_local_interfaces(root, src);

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

    visit_top(
        root,
        src,
        file_rel_path,
        module_qname,
        module_id,
        repo,
        &local_interfaces,
        &mut acc,
    );

    let ns = &acc.natspec;
    if ns.tags > 0 {
        eprintln!(
            "[natspec] tags={} (param={} return={} notice={} dev={} other={}) nodes={} path={}",
            ns.tags, ns.param, ns.ret, ns.notice, ns.dev, ns.other, ns.nodes, file_rel_path
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
    natspec: NatspecStats,
}

/// Visit the file root's declarations. `module_qname` / `module_id` are the
/// file MODULE: every top-level declaration is DEFINED by it, and heritage refs
/// resolve against its import bindings.
#[allow(clippy::too_many_arguments)]
fn visit_top(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    module_qname: &str,
    module_id: NodeId,
    repo: RepoId,
    local_interfaces: &HashSet<String>,
    acc: &mut Acc,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "import_directive" => collect_import(child, src, module_qname, acc),
            "contract_declaration" => {
                visit_contract(child, src, file_rel, module_qname, module_id, module_id, repo, node_kind::CLASS, local_interfaces, acc);
            }
            "interface_declaration" => {
                visit_contract(child, src, file_rel, module_qname, module_id, module_id, repo, node_kind::INTERFACE, local_interfaces, acc);
            }
            "library_declaration" => {
                visit_contract(child, src, file_rel, module_qname, module_id, module_id, repo, node_kind::PACKAGE, local_interfaces, acc);
            }
            "enum_declaration" => {
                visit_enum(child, src, file_rel, module_qname, module_id, repo, acc);
            }
            "struct_declaration" => {
                visit_struct_decl(child, src, file_rel, module_qname, module_id, repo, acc);
            }
            _ => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn visit_contract(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    module_id: NodeId,
    repo: RepoId,
    kind: repo_graph_core::NodeKindId,
    local_interfaces: &HashSet<String>,
    acc: &mut Acc,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = text_of(name_node, src);
    let qname = format!("{parent_qname}::{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, kind, &qname);

    acc.nodes.push(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells(&node, src, file_rel, &mut acc.natspec),
    });
    acc.edges.push(Edge {
        from: parent_id,
        to: id,
        category: edge_category::DEFINES,
        confidence: Confidence::Strong,
    });
    acc.nav.record(id, name, &qname, kind, Some(parent_id));

    // `contract X is A, B`: each base is either an interface (→ IMPLEMENTS) or a
    // base contract (→ INHERITS_FROM). The `_class_heritage` rule is hidden, so
    // its `inheritance_specifier` children appear directly under the contract.
    collect_heritage(node, src, id, module_id, local_interfaces, acc);

    if let Some(body) = node.child_by_field_name("body") {
        let mut c = body.walk();
        for child in body.named_children(&mut c) {
            match child.kind() {
                "function_definition" => {
                    visit_function(child, src, file_rel, &qname, id, repo, acc);
                }
                "event_definition" => {
                    visit_event(child, src, file_rel, &qname, id, repo, acc);
                }
                "enum_declaration" => {
                    visit_enum(child, src, file_rel, &qname, id, repo, acc);
                }
                "struct_declaration" => {
                    visit_struct_decl(child, src, file_rel, &qname, id, repo, acc);
                }
                "state_variable_declaration" => {
                    visit_state_variable(child, src, file_rel, &qname, id, repo, acc);
                }
                _ => {}
            }
        }
    }
}

fn visit_function(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = text_of(name_node, src);
    let qname = format!("{parent_qname}::{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::METHOD, &qname);

    acc.nodes.push(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells(&node, src, file_rel, &mut acc.natspec),
    });
    acc.edges.push(Edge {
        from: parent_id,
        to: id,
        category: edge_category::DEFINES,
        confidence: Confidence::Strong,
    });
    acc.nav
        .record(id, name, &qname, node_kind::METHOD, Some(parent_id));

    if let Some(body) = node.child_by_field_name("body") {
        collect_calls_in(body, src, id, acc);
        collect_emits_in(body, src, parent_qname, id, repo, acc);
    }
}

/// Walk a function body for `emit Foo(...)` statements and wire an EVENT_FLOWS
/// edge from the emitting function (`from`) to the event node. The event is
/// declared in the same contract, so its NodeId is name-derived against the
/// contract qname (`parent_qname`) using the EVENT_EMITTER kind — matching the
/// node `visit_event` produces. Nested functions are not descended into (their
/// own emits are wired when they are visited).
fn collect_emits_in(
    node: TsNode,
    src: &[u8],
    contract_qname: &str,
    from: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if n.kind() == "emit_statement"
            && let Some(name_node) = n.child_by_field_name("name")
        {
            let event = text_of(unwrap_expression(name_node), src);
            if !event.is_empty() {
                let to_qname = format!("{contract_qname}::{event}");
                let to =
                    NodeId::from_parts(GRAPH_TYPE, repo, node_kind::EVENT_EMITTER, &to_qname);
                acc.edges.push(Edge {
                    from,
                    to,
                    category: edge_category::EVENT_FLOWS,
                    confidence: Confidence::Strong,
                });
            }
        }
        let mut cursor = n.walk();
        for child in n.named_children(&mut cursor) {
            if !matches!(child.kind(), "function_definition") {
                stack.push(child);
            }
        }
    }
}

fn visit_event(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = text_of(name_node, src);
    let qname = format!("{parent_qname}::{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::EVENT_EMITTER, &qname);

    acc.nodes.push(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells(&node, src, file_rel, &mut acc.natspec),
    });
    acc.edges.push(Edge {
        from: parent_id,
        to: id,
        category: edge_category::DEFINES,
        confidence: Confidence::Strong,
    });
    acc.nav
        .record(id, name, &qname, node_kind::EVENT_EMITTER, Some(parent_id));
}

fn visit_enum(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = text_of(name_node, src);
    let qname = format!("{parent_qname}::{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::ENUM, &qname);

    acc.nodes.push(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells(&node, src, file_rel, &mut acc.natspec),
    });
    acc.edges.push(Edge {
        from: parent_id,
        to: id,
        category: edge_category::DEFINES,
        confidence: Confidence::Strong,
    });
    acc.nav
        .record(id, name, &qname, node_kind::ENUM, Some(parent_id));
}

fn visit_struct_decl(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = text_of(name_node, src);
    let qname = format!("{parent_qname}::{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::STRUCT, &qname);

    acc.nodes.push(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells(&node, src, file_rel, &mut acc.natspec),
    });
    acc.edges.push(Edge {
        from: parent_id,
        to: id,
        category: edge_category::DEFINES,
        confidence: Confidence::Strong,
    });
    acc.nav
        .record(id, name, &qname, node_kind::STRUCT, Some(parent_id));
}

/// Emit a STATE_VAR node for a contract-level `<type> <visibility> <name>;`.
///
/// Noise gate: skip vars with NO leading doc whose value (initializer) is a
/// literal primitive or absent. Keep vars carrying a doc or a non-trivial
/// type / initializer (so e.g. `mapping(...) public x;` or `uint x = a + b;`
/// survive even without NatSpec).
fn visit_state_variable(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = text_of(name_node, src);

    let doc = repo_graph_doc::leading_doc(&node, src);
    // Noise gate: skip only when undocumented AND initialized to a literal
    // primitive (`uint256 x = 0;`). An UNINITIALIZED public state var
    // (`uint256 public feeBasisPoints;`) is meaningful storage — keep it even
    // without a leading doc. (glia-v5 G19)
    let trivial_value = node
        .child_by_field_name("value")
        .map(is_literal_primitive)
        .unwrap_or(false);
    if doc.is_none() && trivial_value {
        return;
    }

    let qname = format!("{parent_qname}::{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::STATE_VAR, &qname);

    acc.nodes.push(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells(&node, src, file_rel, &mut acc.natspec),
    });
    acc.edges.push(Edge {
        from: parent_id,
        to: id,
        category: edge_category::DEFINES,
        confidence: Confidence::Strong,
    });
    acc.nav
        .record(id, name, &qname, node_kind::STATE_VAR, Some(parent_id));
}

/// True if `value` is a literal primitive (number / string / bool).
fn is_literal_primitive(value: TsNode) -> bool {
    matches!(
        value.kind(),
        "number_literal"
            | "string_literal"
            | "string"
            | "hex_string_literal"
            | "unicode_string_literal"
            | "boolean_literal"
            | "true"
            | "false"
    )
}

/// Collect names of interfaces declared anywhere in this file (top level or
/// nested), used to classify inheritance bases as IMPLEMENTS vs INHERITS_FROM.
fn collect_local_interfaces(root: TsNode, src: &[u8]) -> HashSet<String> {
    let mut set = HashSet::new();
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        if n.kind() == "interface_declaration"
            && let Some(name_node) = n.child_by_field_name("name")
        {
            set.insert(text_of(name_node, src).to_string());
        }
        let mut cursor = n.walk();
        for child in n.named_children(&mut cursor) {
            stack.push(child);
        }
    }
    set
}

/// Parse a contract/interface `is A, B` heritage list. The hidden
/// `_class_heritage` rule splices its `inheritance_specifier` children directly
/// under the declaration node; each carries an `ancestor` `user_defined_type`.
///
/// Each base becomes an `UnresolvedRef` (A6.4): the parser names the base, the
/// graph crate's `resolve_refs` binds it to the real declaration (same file via
/// the module's symbols, cross-file via the unique-type fallback, since a
/// Solidity `import "./X.sol"` binds no names). An external base (`is IERC20`
/// from an OpenZeppelin import) stays unresolved instead of dangling.
/// `module_id` is the file MODULE the ref resolves from, never the enclosing
/// declaration.
fn collect_heritage(
    node: TsNode,
    src: &[u8],
    from_id: NodeId,
    module_id: NodeId,
    local_interfaces: &HashSet<String>,
    acc: &mut Acc,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() != "inheritance_specifier" {
            continue;
        }
        let Some(ancestor) = child.child_by_field_name("ancestor") else {
            continue;
        };
        let Some(base) = udt_name(ancestor, src) else {
            continue;
        };

        let category = if is_interface_base(base, local_interfaces) {
            edge_category::IMPLEMENTS
        } else {
            edge_category::INHERITS_FROM
        };
        acc.refs.push(UnresolvedRef {
            from: from_id,
            from_module: module_id,
            qualifier: CallQualifier::Bare(base.to_string()),
            category,
        });
    }
}

/// Extract the trailing identifier of a `user_defined_type` (handles dotted
/// `Lib.Type` by taking the last segment).
fn udt_name<'a>(node: TsNode<'a>, src: &'a [u8]) -> Option<&'a str> {
    let mut last = None;
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "identifier" {
            last = Some(text_of(child, src));
        }
    }
    // Fall back to the raw text for grammars that expose the name inline.
    last.or_else(|| {
        let t = text_of(node, src);
        t.rsplit('.').next().filter(|s| !s.is_empty())
    })
}

/// A base is treated as an interface if its name resolves to a locally-declared
/// interface, OR by convention starts with `I` + an uppercase letter (e.g.
/// `IERC20`). Otherwise it's a base contract → INHERITS_FROM.
fn is_interface_base(base: &str, local_interfaces: &HashSet<String>) -> bool {
    if local_interfaces.contains(base) {
        return true;
    }
    let mut chars = base.chars();
    matches!(chars.next(), Some('I')) && matches!(chars.next(), Some(c) if c.is_uppercase())
}

fn collect_import(node: TsNode, src: &[u8], from_module: &str, acc: &mut Acc) {
    let text = text_of(node, src).trim().to_string();
    let path = text
        .trim_start_matches("import ")
        .trim_end_matches(';')
        .trim()
        .trim_matches('"');
    acc.imports.push(ImportStmt {
        from_module: from_module.to_string(),
        target: ImportTarget::Module {
            path: path.to_string(),
            alias: None,
        },
    });
}

fn collect_calls_in(node: TsNode, src: &[u8], from: NodeId, acc: &mut Acc) {
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if n.kind() == "call_expression"
            && let Some(func) = n.child_by_field_name("function")
        {
            let qualifier = classify_call(func, src);
            acc.calls.push(CallSite { from, qualifier });
        }
        let mut cursor = n.walk();
        for child in n.named_children(&mut cursor) {
            if !matches!(child.kind(), "function_definition") {
                stack.push(child);
            }
        }
    }
}

fn classify_call(func_node: TsNode, src: &[u8]) -> CallQualifier {
    // tree-sitter-solidity wraps the `function:` field of a `call_expression`
    // in an `expression` node, e.g. `function: (expression (identifier))` or
    // `function: (expression (member_expression ...))`. Unwrap a single-child
    // `expression` so we classify against the real callee node.
    let func_node = unwrap_expression(func_node);
    match func_node.kind() {
        // A bare identifier call inside a contract method is an internal call
        // to a sibling function of the SAME contract (Solidity functions are
        // METHOD-kind, contract-scoped). Emit SelfMethod so the graph's
        // resolve_calls binds it against the enclosing contract's methods
        // (module_symbols would never contain a contract method, so `Bare`
        // could not resolve). Free/global builtins (`require`, `keccak256`,
        // type conversions) simply fall through to unresolved — harmless.
        "identifier" => CallQualifier::SelfMethod(text_of(func_node, src).to_string()),
        "member_expression" => {
            let obj_node = func_node
                .child_by_field_name("object")
                .map(unwrap_expression);
            let obj = obj_node.map(|n| text_of(n, src)).unwrap_or("");
            let prop = func_node
                .child_by_field_name("property")
                .map(|n| text_of(n, src))
                .unwrap_or("");
            // `this.m()` / `super.m()` target a method of the enclosing
            // contract → SelfMethod, same as a bare internal call.
            if obj == "this" || obj == "super" {
                CallQualifier::SelfMethod(prop.to_string())
            } else {
                CallQualifier::Attribute {
                    base: obj.to_string(),
                    name: prop.to_string(),
                }
            }
        }
        _ => CallQualifier::ComplexReceiver {
            receiver: text_of(func_node, src).to_string(),
            name: String::new(),
        },
    }
}

/// Unwrap a single-child `expression` wrapper node (the solidity grammar boxes
/// most sub-expressions in an `expression` rule). Returns the inner node when
/// `node` is an `expression` with exactly one named child; otherwise `node`.
fn unwrap_expression(node: TsNode) -> TsNode {
    if node.kind() == "expression" && node.named_child_count() == 1 {
        node.named_child(0).unwrap_or(node)
    } else {
        node
    }
}

fn text_of<'a>(node: TsNode<'a>, src: &'a [u8]) -> &'a str {
    node.utf8_text(src).unwrap_or("")
}

fn file_cells(root: &TsNode, src: &[u8], file_rel: &str) -> Vec<Cell> {
    vec![
        Cell {
            kind: cell_type::CODE,
            payload: CellPayload::Text(text_of(*root, src).to_string()),
        },
        Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(repo_graph_doc::position_json(root, file_rel)),
        },
    ]
}

fn entity_cells(
    node: &TsNode,
    src: &[u8],
    file_rel: &str,
    natspec: &mut NatspecStats,
) -> Vec<Cell> {
    let mut cells = vec![
        Cell {
            kind: cell_type::CODE,
            payload: CellPayload::Text(text_of(*node, src).to_string()),
        },
        Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(repo_graph_doc::position_json(node, file_rel)),
        },
    ];
    if let Some(doc) = repo_graph_doc::leading_doc(node, src) {
        cells.push(Cell {
            kind: cell_type::DOC,
            payload: CellPayload::Text(doc),
        });
    }
    if let Some(cell) = natspec_cell(node, src, natspec) {
        cells.push(cell);
    }
    cells
}

/// Per-file NatSpec tallies behind the `[natspec]` marker.
#[derive(Default)]
struct NatspecStats {
    tags: usize,
    param: usize,
    ret: usize,
    notice: usize,
    dev: usize,
    other: usize,
    nodes: usize,
}

impl NatspecStats {
    fn record(&mut self, tags: &[DocTag]) {
        self.nodes += 1;
        self.tags += tags.len();
        for t in tags {
            match t.tag.as_str() {
                "param" => self.param += 1,
                "return" => self.ret += 1,
                "notice" => self.notice += 1,
                "dev" => self.dev += 1,
                _ => self.other += 1,
            }
        }
    }
}

/// DOC_TAGS payload. Field order is the wire order (`style`, `tags`; each tag
/// `tag`, `name` only when bound, `text`) — a derived struct keeps it, where a
/// `serde_json::Map` would sort the keys.
#[derive(serde::Serialize)]
struct NatspecPayload<'a> {
    style: &'static str,
    tags: Vec<NatspecTagOut<'a>>,
}

#[derive(serde::Serialize)]
struct NatspecTagOut<'a> {
    tag: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
    text: &'a str,
}

/// NatSpec comment forms: `///` lines and `/** … */` blocks. Plain `//` and
/// `/* */` comments are not NatSpec, so a `// section` note above a function
/// never becomes an `@notice`.
fn is_natspec_comment(raw: &str) -> bool {
    let t = raw.trim_start();
    t.starts_with("///") || (t.starts_with("/**") && !t.starts_with("/**/"))
}

/// The NatSpec above `node` as a DOC_TAGS Json cell — the structured twin of
/// the flat DOC string, which stays untouched. Tag names are bound against the
/// AST so the data stays honest: `@param x` keeps `x` as its name only when `x`
/// is a declared parameter, `@return y` only when `y` is a NAMED return
/// variable (an unnamed return's whole line is its description), otherwise the
/// word stays in the text. `@inheritdoc Base` keeps `Base` — the base
/// contract / interface, usually declared in another file.
fn natspec_cell(node: &TsNode, src: &[u8], stats: &mut NatspecStats) -> Option<Cell> {
    let lines = repo_graph_doc::leading_doc_lines_where(node, src, is_natspec_comment)?;
    let mut tags = repo_graph_doc::split_doc_tags(&lines, "notice");
    if tags.is_empty() {
        return None;
    }
    let params = param_names(node, src);
    let returns = named_returns(node, src);
    for t in &mut tags {
        let bound = match t.tag.as_str() {
            "param" => t.name.as_deref().is_some_and(|n| params.contains(&n)),
            "return" => t.name.as_deref().is_some_and(|n| returns.contains(&n)),
            _ => true,
        };
        if !bound {
            t.unbind_name();
        }
    }
    let payload = NatspecPayload {
        style: "natspec",
        tags: tags
            .iter()
            .map(|t| NatspecTagOut {
                tag: &t.tag,
                name: t.name.as_deref(),
                text: &t.text,
            })
            .collect(),
    };
    let json = serde_json::to_string(&payload).ok()?;
    stats.record(&tags);
    Some(Cell {
        kind: cell_type::DOC_TAGS,
        payload: CellPayload::Json(json),
    })
}

/// Declared parameter names of a function / modifier / constructor / event /
/// error (its direct `parameter` / `event_parameter` / `error_parameter`
/// children). Return variables live under `return_type`, not here.
fn param_names<'a>(node: &TsNode<'a>, src: &'a [u8]) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if matches!(child.kind(), "parameter" | "event_parameter" | "error_parameter")
            && let Some(n) = child.child_by_field_name("name")
        {
            out.push(text_of(n, src));
        }
    }
    out
}

/// Names of a function's NAMED return variables (`returns (bool ok)` -> `ok`).
fn named_returns<'a>(node: &TsNode<'a>, src: &'a [u8]) -> Vec<&'a str> {
    let Some(ret) = node.child_by_field_name("return_type") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cursor = ret.walk();
    for child in ret.named_children(&mut cursor) {
        if child.kind() == "parameter"
            && let Some(n) = child.child_by_field_name("name")
        {
            out.push(text_of(n, src));
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

    #[test]
    fn contract_and_functions() {
        let source = r#"
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

contract Token {
    function transfer(address to, uint256 amount) public returns (bool) {
        return true;
    }

    event Transfer(address indexed from, address indexed to, uint256 value);
}
"#;
        let fp = parse_file(source, "contracts/Token.sol", "contracts::Token", repo()).unwrap();
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::CLASS).count(), 1);
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::METHOD).count(), 1);
    }

    #[test]
    fn interface_and_library() {
        let source = r#"
interface IERC20 {
    function totalSupply() external view returns (uint256);
}

library SafeMath {
    function add(uint256 a, uint256 b) internal pure returns (uint256) {
        return a + b;
    }
}
"#;
        let fp = parse_file(source, "contracts/Lib.sol", "contracts::Lib", repo()).unwrap();
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::INTERFACE).count(), 1);
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::PACKAGE).count(), 1);
    }

    #[test]
    fn state_var_with_doc_and_implements_edge() {
        let source = r#"
interface IFoo {
    function foo() external;
}

contract X is IFoo {
    /// @notice Platform fee
    uint256 public feeBasisPoints;
}
"#;
        let fp = parse_file(source, "contracts/X.sol", "contracts::X", repo()).unwrap();

        // STATE_VAR node `feeBasisPoints` surfaced (has a leading NatSpec doc).
        let var_id = fp
            .nav
            .kind_by_id
            .iter()
            .find(|(_, k)| **k == node_kind::STATE_VAR)
            .map(|(id, _)| *id)
            .expect("expected a STATE_VAR node");
        assert_eq!(fp.nav.name_by_id.get(&var_id).map(String::as_str), Some("feeBasisPoints"));

        // The DOC cell carries the NatSpec.
        let var_node = fp.nodes.iter().find(|n| n.id == var_id).unwrap();
        assert!(
            var_node
                .cells
                .iter()
                .any(|c| c.kind == cell_type::DOC),
            "STATE_VAR should carry a DOC cell"
        );

        // `contract X is IFoo` → an IMPLEMENTS ref (IFoo matches `I`+capital and
        // is a locally-declared interface), not INHERITS_FROM. The parser emits
        // no heritage edge of its own: resolve_refs binds the ref.
        assert_eq!(
            fp.refs
                .iter()
                .filter(|r| r.category == edge_category::IMPLEMENTS
                    && matches!(&r.qualifier, CallQualifier::Bare(n) if n == "IFoo"))
                .count(),
            1,
            "expected one IMPLEMENTS ref for `is IFoo`, got {:?}",
            fp.refs
        );
        assert_eq!(
            fp.refs.iter().filter(|r| r.category == edge_category::INHERITS_FROM).count(),
            0,
        );
        assert!(
            !fp.edges.iter().any(|e| e.category == edge_category::IMPLEMENTS
                || e.category == edge_category::INHERITS_FROM),
            "heritage must not be a parser-fabricated edge"
        );
        // The ref resolves from the file MODULE, out of the contract `X`.
        let module_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "contracts::X");
        let x_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, "contracts::X::X");
        assert!(fp.refs.iter().any(|r| r.from == x_id && r.from_module == module_id));
    }

    #[test]
    fn state_var_noise_gate_skips_undocumented_literal() {
        let source = r#"
contract Y is Base {
    uint256 public count = 0;
    /// @dev keeps the treasury
    address public treasury;
}
"#;
        let fp = parse_file(source, "contracts/Y.sol", "contracts::Y", repo()).unwrap();
        let state_vars: Vec<_> = fp
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::STATE_VAR)
            .filter_map(|(id, _)| fp.nav.name_by_id.get(id).map(String::as_str))
            .collect();
        // Solidity keeps state variables (the contract's storage interface);
        // the noise gate only drops an undocumented var with a *detected*
        // literal initializer, which is conservative here, so both survive.
        // `treasury` (documented, uninitialized) is the must-keep case.
        assert!(state_vars.contains(&"treasury"));

        // `Base` is not interface-like → an INHERITS_FROM ref, no IMPLEMENTS.
        assert_eq!(
            fp.refs
                .iter()
                .filter(|r| r.category == edge_category::INHERITS_FROM
                    && matches!(&r.qualifier, CallQualifier::Bare(n) if n == "Base"))
                .count(),
            1,
        );
        assert_eq!(
            fp.refs.iter().filter(|r| r.category == edge_category::IMPLEMENTS).count(),
            0,
        );
    }

    #[test]
    fn internal_call_emits_self_method() {
        // `deposit()` calls sibling `_credit()` — a bare internal call inside a
        // contract. The parser must emit a SelfMethod CallSite so the graph
        // resolves it against the enclosing contract's methods.
        let source = r#"
contract Bank {
    mapping(address => uint256) balances;

    function deposit(uint256 amount) public {
        _credit(msg.sender, amount);
        this.deposit(amount);
    }

    function _credit(address to, uint256 amount) internal {
        balances[to] += amount;
    }
}
"#;
        let fp = parse_file(source, "contracts/Bank.sol", "contracts::Bank", repo()).unwrap();

        // Bare internal call `_credit(...)` → SelfMethod("_credit").
        assert!(
            fp.calls
                .iter()
                .any(|c| matches!(&c.qualifier, CallQualifier::SelfMethod(n) if n == "_credit")),
            "expected a SelfMethod call to _credit, got {:?}",
            fp.calls
        );

        // `this.deposit(...)` also maps to SelfMethod (same-contract dispatch).
        assert!(
            fp.calls
                .iter()
                .any(|c| matches!(&c.qualifier, CallQualifier::SelfMethod(n) if n == "deposit")),
            "expected a SelfMethod call to deposit via `this.`, got {:?}",
            fp.calls
        );

        // The call originates from `deposit`'s NodeId, not `_credit`'s.
        let deposit_id = fp
            .nav
            .name_by_id
            .iter()
            .find(|(_, n)| n.as_str() == "deposit")
            .map(|(id, _)| *id)
            .expect("deposit method node");
        assert!(
            fp.calls
                .iter()
                .any(|c| c.from == deposit_id
                    && matches!(&c.qualifier, CallQualifier::SelfMethod(n) if n == "_credit")),
            "the _credit call should originate from deposit"
        );

        // No ComplexReceiver leaked through the expression-unwrap.
        assert!(
            !fp.calls
                .iter()
                .any(|c| matches!(c.qualifier, CallQualifier::ComplexReceiver { .. })),
            "no call should classify as ComplexReceiver: {:?}",
            fp.calls
        );
    }

    #[test]
    fn event_emitter_node_and_event_flows_edge() {
        // `event BidPlaced(...)` is an EVENT_EMITTER (not a FUNCTION), and the
        // `bid()` function that does `emit BidPlaced(...)` wires an EVENT_FLOWS
        // edge bid -> BidPlaced.
        let source = r#"
contract Auction {
    uint256 public highestBid;

    event BidPlaced(address indexed bidder, uint256 amount);

    function bid(uint256 amount) public {
        highestBid = amount;
        emit BidPlaced(msg.sender, amount);
    }
}
"#;
        let fp = parse_file(source, "Auction.sol", "Auction", repo()).unwrap();

        // The event surfaces as EVENT_EMITTER named `BidPlaced` (never FUNCTION).
        let event_id = fp
            .nav
            .kind_by_id
            .iter()
            .find(|(_, k)| **k == node_kind::EVENT_EMITTER)
            .map(|(id, _)| *id)
            .expect("expected an EVENT_EMITTER node");
        assert_eq!(
            fp.nav.name_by_id.get(&event_id).map(String::as_str),
            Some("BidPlaced")
        );
        assert_eq!(
            fp.nav
                .kind_by_id
                .values()
                .filter(|k| **k == node_kind::FUNCTION)
                .count(),
            0,
            "the event must not be emitted as FUNCTION"
        );

        // `bid` is a METHOD; `emit BidPlaced(...)` wires bid -> BidPlaced.
        let bid_id = fp
            .nav
            .name_by_id
            .iter()
            .find(|(_, n)| n.as_str() == "bid")
            .map(|(id, _)| *id)
            .expect("bid method node");
        assert!(
            fp.edges.iter().any(|e| e.from == bid_id
                && e.to == event_id
                && e.category == edge_category::EVENT_FLOWS),
            "expected an EVENT_FLOWS edge bid -> BidPlaced, got {:?}",
            fp.edges
        );
    }

    #[test]
    fn imports() {
        let source = r#"
import "./IERC20.sol";
import "@openzeppelin/contracts/token/ERC20/ERC20.sol";
"#;
        let fp = parse_file(source, "contracts/Token.sol", "contracts::Token", repo()).unwrap();
        assert_eq!(fp.imports.len(), 2);
    }

    /// The payload of `cell` on the non-MODULE node named `name`, if any (the
    /// module of `contracts/Vault.sol` is also called `Vault`).
    fn cell_on(fp: &FileParse, name: &str, cell: repo_graph_core::CellTypeId) -> Option<String> {
        let id = fp
            .nav
            .name_by_id
            .iter()
            .find(|(id, n)| {
                n.as_str() == name && fp.nav.kind_by_id.get(*id) != Some(&node_kind::MODULE)
            })
            .map(|(id, _)| *id)?;
        let node = fp.nodes.iter().find(|n| n.id == id)?;
        node.cells.iter().find(|c| c.kind == cell).map(|c| match &c.payload {
            CellPayload::Text(s) | CellPayload::Json(s) => s.clone(),
            other => format!("{other:?}"),
        })
    }

    const VAULT: &str = r#"// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

/// @title Vault
/// @author Kina
/// @notice Holds deposits for members.
contract Vault {
    /// @notice Platform fee in basis points.
    uint256 public feeBasisPoints;

    /**
     * @notice Deposit funds for a member.
     * @dev Reverts when amount is zero.
     * @param member The member credited.
     * @param amount The amount in wei.
     * @return ok True on success.
     */
    function deposit(address member, uint256 amount) public returns (bool ok) {
        return true;
    }

    /// @notice Withdraw.
    /// @param amount How much.
    /// @return remaining Balance left.
    /// @inheritdoc IVault
    /// @custom:security non-reentrant
    function withdraw(uint256 amount) external returns (uint256 remaining) {
        return 0;
    }

    /// @return True always.
    function ping() external pure returns (bool) {
        return true;
    }
}
"#;

    #[test]
    fn natspec_doc_tags_named_return_param_and_block_comment() {
        let fp = parse_file(VAULT, "contracts/Vault.sol", "contracts::Vault", repo()).unwrap();
        assert_eq!(
            cell_on(&fp, "deposit", cell_type::DOC_TAGS).as_deref(),
            Some(concat!(
                r#"{"style":"natspec","tags":["#,
                r#"{"tag":"notice","text":"Deposit funds for a member."},"#,
                r#"{"tag":"dev","text":"Reverts when amount is zero."},"#,
                r#"{"tag":"param","name":"member","text":"The member credited."},"#,
                r#"{"tag":"param","name":"amount","text":"The amount in wei."},"#,
                r#"{"tag":"return","name":"ok","text":"True on success."}]}"#,
            ))
        );
        // The flat DOC string is untouched.
        assert_eq!(
            cell_on(&fp, "deposit", cell_type::DOC).as_deref(),
            Some(
                "@notice Deposit funds for a member. @dev Reverts when amount is zero. \
                 @param member The member credited. @param amount The amount in wei. \
                 @return ok True on success."
            )
        );
    }

    #[test]
    fn natspec_inheritdoc_custom_and_unnamed_return() {
        let fp = parse_file(VAULT, "contracts/Vault.sol", "contracts::Vault", repo()).unwrap();
        assert_eq!(
            cell_on(&fp, "withdraw", cell_type::DOC_TAGS).as_deref(),
            Some(concat!(
                r#"{"style":"natspec","tags":["#,
                r#"{"tag":"notice","text":"Withdraw."},"#,
                r#"{"tag":"param","name":"amount","text":"How much."},"#,
                r#"{"tag":"return","name":"remaining","text":"Balance left."},"#,
                r#"{"tag":"inheritdoc","name":"IVault","text":""},"#,
                r#"{"tag":"custom:security","text":"non-reentrant"}]}"#,
            ))
        );
        // `returns (bool)` is unnamed: `True` is description, not a name.
        assert_eq!(
            cell_on(&fp, "ping", cell_type::DOC_TAGS).as_deref(),
            Some(r#"{"style":"natspec","tags":[{"tag":"return","text":"True always."}]}"#)
        );
    }

    #[test]
    fn natspec_contract_level_title_author_and_state_var() {
        let fp = parse_file(VAULT, "contracts/Vault.sol", "contracts::Vault", repo()).unwrap();
        assert_eq!(
            cell_on(&fp, "Vault", cell_type::DOC_TAGS).as_deref(),
            Some(concat!(
                r#"{"style":"natspec","tags":["#,
                r#"{"tag":"title","text":"Vault"},"#,
                r#"{"tag":"author","text":"Kina"},"#,
                r#"{"tag":"notice","text":"Holds deposits for members."}]}"#,
            ))
        );
        assert_eq!(
            cell_on(&fp, "feeBasisPoints", cell_type::DOC_TAGS).as_deref(),
            Some(r#"{"style":"natspec","tags":[{"tag":"notice","text":"Platform fee in basis points."}]}"#)
        );
        // The module node carries no DOC_TAGS: a file is not a NatSpec target
        // (and its SPDX line is boilerplate).
        let module = fp
            .nodes
            .iter()
            .find(|n| fp.nav.kind_by_id.get(&n.id) == Some(&node_kind::MODULE))
            .expect("module node");
        assert!(module.cells.iter().all(|c| c.kind != cell_type::DOC_TAGS));
    }

    #[test]
    fn natspec_param_not_in_signature_stays_text() {
        let source = r#"
contract C {
    /// @param amout Misspelt, so not a parameter.
    /// @param to Recipient.
    function send(address to, uint256 amount) external {}
}
"#;
        let fp = parse_file(source, "C.sol", "C", repo()).unwrap();
        assert_eq!(
            cell_on(&fp, "send", cell_type::DOC_TAGS).as_deref(),
            Some(concat!(
                r#"{"style":"natspec","tags":["#,
                r#"{"tag":"param","text":"amout Misspelt, so not a parameter."},"#,
                r#"{"tag":"param","name":"to","text":"Recipient."}]}"#,
            ))
        );
    }

    #[test]
    fn natspec_block_and_triple_slash_forms_give_identical_tags() {
        let block = r#"
contract C {
    /**
     * Pays out.
     * @param who The payee,
     *   possibly a contract.
     * @return paid Amount paid.
     */
    function pay(address who) external returns (uint256 paid) {}
}
"#;
        let lines = r#"
contract C {
    /// Pays out.
    /// @param who The payee,
    ///   possibly a contract.
    /// @return paid Amount paid.
    function pay(address who) external returns (uint256 paid) {}
}
"#;
        let a = parse_file(block, "C.sol", "C", repo()).unwrap();
        let b = parse_file(lines, "C.sol", "C", repo()).unwrap();
        let want = concat!(
            r#"{"style":"natspec","tags":["#,
            r#"{"tag":"notice","text":"Pays out."},"#,
            r#"{"tag":"param","name":"who","text":"The payee, possibly a contract."},"#,
            r#"{"tag":"return","name":"paid","text":"Amount paid."}]}"#,
        );
        assert_eq!(cell_on(&a, "pay", cell_type::DOC_TAGS).as_deref(), Some(want));
        assert_eq!(cell_on(&b, "pay", cell_type::DOC_TAGS).as_deref(), Some(want));
    }

    #[test]
    fn natspec_plain_comments_are_not_natspec() {
        let source = r#"
contract C {
    // SC-02 mutual proposal events
    event ResolutionProposed(uint256 indexed tradeId, address by);

    /// @notice Raised on a dispute.
    /// @param tradeId The trade.
    // (divider)
    event DisputeRaised(uint256 indexed tradeId);

    /* not natspec either */
    function f() external {}
}
"#;
        let fp = parse_file(source, "C.sol", "C", repo()).unwrap();
        // DOC still carries the plain comment; DOC_TAGS does not claim it.
        assert!(cell_on(&fp, "ResolutionProposed", cell_type::DOC).is_some());
        assert_eq!(cell_on(&fp, "ResolutionProposed", cell_type::DOC_TAGS), None);
        assert_eq!(cell_on(&fp, "f", cell_type::DOC_TAGS), None);
        // An event's @param binds against its event parameters; the `//`
        // divider between the NatSpec and the event is stepped over.
        assert_eq!(
            cell_on(&fp, "DisputeRaised", cell_type::DOC_TAGS).as_deref(),
            Some(concat!(
                r#"{"style":"natspec","tags":["#,
                r#"{"tag":"notice","text":"Raised on a dispute."},"#,
                r#"{"tag":"param","name":"tradeId","text":"The trade."}]}"#,
            ))
        );
    }
}
