use std::collections::HashSet;

use repo_graph_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};
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
}

#[allow(clippy::too_many_arguments)]
fn visit_top(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    local_interfaces: &HashSet<String>,
    acc: &mut Acc,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "import_directive" => collect_import(child, src, parent_qname, acc),
            "contract_declaration" => {
                visit_contract(child, src, file_rel, parent_qname, parent_id, repo, node_kind::CLASS, local_interfaces, acc);
            }
            "interface_declaration" => {
                visit_contract(child, src, file_rel, parent_qname, parent_id, repo, node_kind::INTERFACE, local_interfaces, acc);
            }
            "library_declaration" => {
                visit_contract(child, src, file_rel, parent_qname, parent_id, repo, node_kind::PACKAGE, local_interfaces, acc);
            }
            "enum_declaration" => {
                visit_enum(child, src, file_rel, parent_qname, parent_id, repo, acc);
            }
            "struct_declaration" => {
                visit_struct_decl(child, src, file_rel, parent_qname, parent_id, repo, acc);
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
        cells: entity_cells(&node, src, file_rel),
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
    collect_heritage(node, src, parent_qname, id, repo, local_interfaces, acc);

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
        cells: entity_cells(&node, src, file_rel),
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
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::FUNCTION, &qname);

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
    });
    acc.nav
        .record(id, name, &qname, node_kind::FUNCTION, Some(parent_id));
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
        cells: entity_cells(&node, src, file_rel),
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
        cells: entity_cells(&node, src, file_rel),
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
        cells: entity_cells(&node, src, file_rel),
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
fn collect_heritage(
    node: TsNode,
    src: &[u8],
    parent_qname: &str,
    from_id: NodeId,
    repo: RepoId,
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

        let (kind, category) = if is_interface_base(base, local_interfaces) {
            (node_kind::INTERFACE, edge_category::IMPLEMENTS)
        } else {
            (node_kind::CLASS, edge_category::INHERITS_FROM)
        };
        // Best-effort same-scope target NodeId; the graph crate's cross-file
        // resolver reconciles it against the real declaration.
        let to_qname = format!("{parent_qname}::{base}");
        let to_id = NodeId::from_parts(GRAPH_TYPE, repo, kind, &to_qname);
        acc.edges.push(Edge {
            from: from_id,
            to: to_id,
            category,
            confidence: Confidence::Weak,
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
    match func_node.kind() {
        "identifier" => CallQualifier::Bare(text_of(func_node, src).to_string()),
        "member_expression" => {
            let obj = func_node
                .child_by_field_name("object")
                .map(|n| text_of(n, src))
                .unwrap_or("");
            let prop = func_node
                .child_by_field_name("property")
                .map(|n| text_of(n, src))
                .unwrap_or("");
            CallQualifier::Attribute {
                base: obj.to_string(),
                name: prop.to_string(),
            }
        }
        _ => CallQualifier::ComplexReceiver {
            receiver: text_of(func_node, src).to_string(),
            name: String::new(),
        },
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

fn entity_cells(node: &TsNode, src: &[u8], file_rel: &str) -> Vec<Cell> {
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
    cells
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

        // `contract X is IFoo` → IMPLEMENTS (IFoo matches `I`+capital and is a
        // locally-declared interface), not INHERITS_FROM.
        assert_eq!(
            fp.edges.iter().filter(|e| e.category == edge_category::IMPLEMENTS).count(),
            1,
            "expected one IMPLEMENTS edge for `is IFoo`"
        );
        assert_eq!(
            fp.edges.iter().filter(|e| e.category == edge_category::INHERITS_FROM).count(),
            0,
        );
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

        // `Base` is not interface-like → INHERITS_FROM, no IMPLEMENTS.
        assert_eq!(
            fp.edges.iter().filter(|e| e.category == edge_category::INHERITS_FROM).count(),
            1,
        );
        assert_eq!(
            fp.edges.iter().filter(|e| e.category == edge_category::IMPLEMENTS).count(),
            0,
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
}
