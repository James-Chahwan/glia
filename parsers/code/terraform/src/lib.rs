use std::collections::{HashMap, HashSet};

use glia_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};
use tree_sitter::{Node as TsNode, Parser};

pub use glia_code_domain::{
    CallQualifier, CallSite, CodeNav, FileParse, GRAPH_TYPE, ImportStmt, ImportTarget, ParseError,
    UnresolvedRef, cell_type, edge_category, infra, node_kind,
};

/// The INFRA_RESOURCE qname for a Terraform `resource "<ty>" "<name>"` block.
///
/// A13.5: this is the SAME shape `parsers/code/extractors/src/iac.rs` emits for
/// k8s / compose / Dockerfile resources, so `IacResolver` — which pairs on the
/// verbatim qname — can finally join a terraform-declared service to the k8s
/// Service another repo declares. The old shape was `<module_qname>::<ty>.<name>`,
/// which embedded the file's module path and therefore never joined anything,
/// not even terraform-to-terraform.
///
/// Both the emitter (`visit_block`) and the `collect_resource_addresses`
/// pre-pass call THIS function: if they ever disagree, every INFRA_REFERENCES /
/// DEPENDS_ON edge silently dangles.
fn resource_qname(ty: &str, name: &str) -> String {
    infra::qname(infra::canonical_kind(ty), name)
}

pub fn parse_file(
    source: &str,
    file_rel_path: &str,
    module_qname: &str,
    repo: RepoId,
) -> Result<FileParse, ParseError> {
    let mut parser = Parser::new();
    let lang: tree_sitter::Language = tree_sitter_hcl::LANGUAGE.into();
    parser
        .set_language(&lang)
        .map_err(|e| ParseError::LanguageInit(e.to_string()))?;
    let tree = parser.parse(source, None).ok_or(ParseError::NoTree)?;
    let src = source.as_bytes();
    let root = tree.root_node();

    let mut acc = Acc::default();

    // Pre-pass: map each resource's Terraform address ("aws_vpc.main") to its
    // NodeId so intra-file references resolve directly to concrete resource nodes.
    collect_resource_addresses(root, src, repo, &mut acc.resource_ids);

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
    /// Terraform resource address ("aws_vpc.main") → resource NodeId.
    resource_ids: HashMap<String, NodeId>,
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
        match child.kind() {
            "block" => visit_block(child, src, file_rel, parent_qname, parent_id, repo, acc),
            "body" => {
                let mut c2 = child.walk();
                for gc in child.named_children(&mut c2) {
                    if gc.kind() == "block" {
                        visit_block(gc, src, file_rel, parent_qname, parent_id, repo, acc);
                    }
                }
            }
            _ => {}
        }
    }
}

fn visit_block(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let labels = collect_labels(node, src);
    if labels.is_empty() {
        return;
    }

    let block_type = labels[0].as_str();
    match block_type {
        "resource" if labels.len() >= 3 => {
            // NAME stays the Terraform address (`aws_vpc.main`) — it is what a
            // reader recognises. Only the qname (= node identity) is canonical.
            let name = format!("{}.{}", labels[1], labels[2]);
            let qname = resource_qname(&labels[1], &labels[2]);
            let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::INFRA_RESOURCE, &qname);
            let mut cells = entity_cells(&node, src, file_rel);
            // DF.14: after entity_cells, so the first CODE cell stays the
            // block's source slice.
            cells.extend(pubsub_cell(node, src, &labels[1]));
            acc.nodes.push(Node {
                id,
                repo,
                confidence: Confidence::Strong,
                cells,
            });
            acc.edges.push(Edge {
                from: parent_id,
                to: id,
                category: edge_category::DEFINES,
                confidence: Confidence::Strong,
                cells: Vec::new(),
            });
            acc.nav
                .record(id, &name, &qname, node_kind::INFRA_RESOURCE, Some(parent_id));
            emit_resource_edges(node, src, id, &name, acc);
        }
        "module" if labels.len() >= 2 => {
            let name = &labels[1];
            let qname = format!("{parent_qname}::{name}");
            let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::PACKAGE, &qname);
            acc.nodes.push(Node {
                id,
                repo,
                confidence: Confidence::Strong,
                cells: entity_cells(&node, src, file_rel),
            });
            acc.edges.push(Edge {
                from: parent_id,
                to: id,
                category: edge_category::CONTAINS,
                confidence: Confidence::Strong,
                cells: Vec::new(),
            });
            acc.nav
                .record(id, name, &qname, node_kind::PACKAGE, Some(parent_id));

            if let Some(source_attr) = find_attribute(node, src, "source") {
                acc.imports.push(ImportStmt {
                    from_module: parent_qname.to_string(),
                    target: ImportTarget::Module {
                        path: source_attr,
                        alias: None,
                    },
                    line: line_at(node),
                });
            }
        }
        "variable" if labels.len() >= 2 => {
            let name = format!("var.{}", labels[1]);
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
                cells: Vec::new(),
            });
            acc.nav
                .record(id, &name, &qname, node_kind::FUNCTION, Some(parent_id));
        }
        "output" if labels.len() >= 2 => {
            let name = format!("output.{}", labels[1]);
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
                cells: Vec::new(),
            });
            acc.nav
                .record(id, &name, &qname, node_kind::FUNCTION, Some(parent_id));
        }
        "data" if labels.len() >= 3 => {
            let name = format!("data.{}.{}", labels[1], labels[2]);
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
                cells: Vec::new(),
            });
            acc.nav
                .record(id, &name, &qname, node_kind::STRUCT, Some(parent_id));
        }
        _ => {}
    }
}

fn collect_labels(node: TsNode, src: &[u8]) -> Vec<String> {
    let mut labels = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "identifier" => labels.push(text_of(child, src).to_string()),
            "string_lit" => {
                let mut c = child.walk();
                for gc in child.named_children(&mut c) {
                    if gc.kind() == "template_literal" {
                        labels.push(text_of(gc, src).to_string());
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    labels
}

fn find_attribute(node: TsNode, src: &[u8], attr_name: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "body" {
            let mut c2 = child.walk();
            for gc in child.named_children(&mut c2) {
                if gc.kind() == "attribute" {
                    let key = gc.named_child(0).map(|n| text_of(n, src)).unwrap_or("");
                    if key == attr_name {
                        let val_text = gc.named_child(1).map(|n| text_of(n, src)).unwrap_or("");
                        return Some(val_text.trim_matches('"').to_string());
                    }
                }
            }
        }
    }
    None
}

/// The value expression of the block's top-level attribute `attr_name`.
fn attribute_value<'t>(node: TsNode<'t>, src: &[u8], attr_name: &str) -> Option<TsNode<'t>> {
    let mut cursor = node.walk();
    let body = node.named_children(&mut cursor).find(|c| c.kind() == "body")?;
    let mut c2 = body.walk();
    let attr = body.named_children(&mut c2).find(|a| {
        a.kind() == "attribute" && a.named_child(0).map(|k| text_of(k, src)) == Some(attr_name)
    })?;
    attr.named_child(1)
}

// ---- DF.14: GCP Pub/Sub bindings -------------------------------------------

/// The Terraform resource type of a Pub/Sub topic.
const PUBSUB_TOPIC: &str = "google_pubsub_topic";
/// The Terraform resource type of a Pub/Sub subscription.
const PUBSUB_SUBSCRIPTION: &str = "google_pubsub_subscription";

/// DF.14: the Pub/Sub facts of a `google_pubsub_topic` /
/// `google_pubsub_subscription` resource block, as one JSON CODE cell (the
/// A2.8 queue-node convention; no verbatim source slice, so CD.7c stores it
/// inline). A subscriber names its SUBSCRIPTION, never its topic, and this is
/// where the binding is declared; `QueueStackResolver` reads the cells
/// (`graph/src/resolvers/queue.rs`, rule `binding`).
///
/// - topic: `{"pubsub":"topic","name":<n>}`
/// - subscription with a literal topic (`orders`, or the full
///   `projects/<p>/topics/orders`, whose project may be interpolated):
///   `{"pubsub":"subscription","name":<n>,"topic":<t>}`
/// - subscription referencing `google_pubsub_topic.<r>.name` or `.id`:
///   `{"pubsub":"subscription","name":<n>,"topic_ref":"google_pubsub_topic.<r>"}`
///
/// A name that is not a literal Pub/Sub id (interpolation, a variable, a
/// module output) records nothing: no guess.
fn pubsub_cell(node: TsNode, src: &[u8], ty: &str) -> Option<Cell> {
    let name = || attribute_value(node, src, "name").and_then(|v| string_literal(v, src));
    // Written by hand in a fixed field order: every value is a checked
    // `is_pubsub_id` or a Terraform address (identifier characters only), so
    // nothing needs escaping, and the bytes do not depend on whether a build
    // unifies serde_json's `preserve_order` feature.
    let json = match ty {
        PUBSUB_TOPIC => format!(r#"{{"pubsub":"topic","name":"{}"}}"#, name()?),
        PUBSUB_SUBSCRIPTION => {
            let name = name()?;
            let topic = attribute_value(node, src, "topic")?;
            match literal_topic(topic, src) {
                Some(t) => format!(r#"{{"pubsub":"subscription","name":"{name}","topic":"{t}"}}"#),
                None => {
                    let r = topic_resource_ref(topic, src)?;
                    format!(r#"{{"pubsub":"subscription","name":"{name}","topic_ref":"{r}"}}"#)
                }
            }
        }
        _ => return None,
    };
    Some(Cell {
        kind: cell_type::CODE,
        payload: CellPayload::Json(json),
    })
}

/// A Pub/Sub topic or subscription id: non-empty, only the characters GCP
/// allows (letters, digits, `-` `_` `.` `~` `+` `%`). That excludes every
/// quote, escape, `/` and template character, so the cell's JSON strings
/// never need escaping (the graph crate reads them with a tight scan).
fn is_pubsub_id(s: &str) -> bool {
    !s.is_empty()
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.~+%".contains(&b))
}

/// `expr` as a plain string literal holding a Pub/Sub id; `None` for an
/// interpolated template, a reference, or any other expression.
fn string_literal(expr: TsNode, src: &[u8]) -> Option<String> {
    let lit = only_named_child(expr, "literal_value")?;
    let s = only_named_child(lit, "string_lit")?;
    let mut cursor = s.walk();
    let parts: Vec<TsNode> = s
        .named_children(&mut cursor)
        .filter(|c| !matches!(c.kind(), "quoted_template_start" | "quoted_template_end"))
        .collect();
    let [part] = parts.as_slice() else { return None };
    let text = text_of(*part, src);
    (part.kind() == "template_literal" && is_pubsub_id(text)).then(|| text.to_string())
}

/// The topic id a subscription's `topic` string names: a bare id, or the
/// tail of `projects/<p>/topics/<t>`. The project segment may be an
/// interpolation (`projects/${var.project}/topics/orders`) as long as the
/// topic id after the last `/topics/` is literal.
fn literal_topic(expr: TsNode, src: &[u8]) -> Option<String> {
    if let Some(s) = string_literal(expr, src) {
        return Some(s);
    }
    let string = match only_named_child(expr, "literal_value") {
        Some(lit) => only_named_child(lit, "string_lit")?,
        None => only_named_child(only_named_child(expr, "template_expr")?, "quoted_template")?,
    };
    // Only a full topic path names a topic: `"${local.prefix}/topics/x"` or
    // `"a/topics/b"` is some other string.
    if !text_of(string, src).trim_start_matches('"').starts_with("projects/") {
        return None;
    }
    let mut cursor = string.walk();
    let parts: Vec<TsNode> = string.named_children(&mut cursor).collect();
    // The last content part, right before the closing quote, must be literal.
    let [.., last, end] = parts.as_slice() else { return None };
    if end.kind() != "quoted_template_end" || last.kind() != "template_literal" {
        return None;
    }
    let (_, id) = text_of(*last, src).rsplit_once("/topics/")?;
    is_pubsub_id(id).then(|| id.to_string())
}

/// `google_pubsub_topic.<r>.name` / `.id` -> `google_pubsub_topic.<r>`: the
/// Terraform address of the topic resource the subscription references.
fn topic_resource_ref(expr: TsNode, src: &[u8]) -> Option<String> {
    if expr.kind() != "expression" || expr.named_child_count() != 3 {
        return None;
    }
    let address = ref_address_of(expr, src)?;
    let (ty, _) = address.split_once('.')?;
    let attr = expr.named_child(2).filter(|a| a.kind() == "get_attr")?;
    let field = attr.named_child(0).map(|n| text_of(n, src))?;
    (ty == PUBSUB_TOPIC && matches!(field, "name" | "id")).then_some(address)
}

/// `node`'s single named child, when it has exactly one and it is `kind`.
fn only_named_child<'t>(node: TsNode<'t>, kind: &str) -> Option<TsNode<'t>> {
    if node.named_child_count() != 1 {
        return None;
    }
    node.named_child(0).filter(|c| c.kind() == kind)
}

/// Pre-pass mirroring `visit_top`/`visit_block` traversal to record every
/// resource's Terraform address ("aws_vpc.main") → NodeId, so later reference
/// scanning can bind intra-file references to concrete resource nodes.
fn collect_resource_addresses(
    node: TsNode,
    src: &[u8],
    repo: RepoId,
    out: &mut HashMap<String, NodeId>,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "block" => {
                let labels = collect_labels(child, src);
                if labels.len() >= 3 && labels[0] == "resource" {
                    let address = format!("{}.{}", labels[1], labels[2]);
                    let qname = resource_qname(&labels[1], &labels[2]);
                    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::INFRA_RESOURCE, &qname);
                    out.insert(address, id);
                }
            }
            "body" => collect_resource_addresses(child, src, repo, out),
            _ => {}
        }
    }
}

/// Scan a resource block's body for references to other resources and emit
/// direct edges: `depends_on = [...]` → DEPENDS_ON, any other attribute
/// referencing a resource address (e.g. `vpc_id = aws_vpc.main.id`) →
/// INFRA_REFERENCES. Resolution is intra-file, by resource address.
fn emit_resource_edges(node: TsNode, src: &[u8], from: NodeId, self_address: &str, acc: &mut Acc) {
    let mut seen: HashSet<(NodeId, u32)> = HashSet::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() != "body" {
            continue;
        }
        let mut c2 = child.walk();
        for attr in child.named_children(&mut c2) {
            if attr.kind() != "attribute" {
                continue;
            }
            let key = attr.named_child(0).map(|n| text_of(n, src)).unwrap_or("");
            let value = match attr.named_child(1) {
                Some(v) => v,
                None => continue,
            };
            let category = if key == "depends_on" {
                edge_category::DEPENDS_ON
            } else {
                edge_category::INFRA_REFERENCES
            };

            let mut addresses = Vec::new();
            collect_ref_addresses(value, src, &mut addresses);
            for address in addresses {
                if address == self_address {
                    continue;
                }
                if let Some(&to) = acc.resource_ids.get(&address)
                    && seen.insert((to, category.0))
                {
                    acc.edges.push(Edge {
                        from,
                        to,
                        category,
                        confidence: Confidence::Strong,
                        cells: Vec::new(),
                    });
                }
            }
        }
    }
}

/// Walk an expression subtree, collecting the "type.name" address of every
/// resource-style reference (an `expression` beginning with
/// `variable_expr` + `get_attr`, e.g. `aws_vpc.main` in `aws_vpc.main.id`).
fn collect_ref_addresses(node: TsNode, src: &[u8], out: &mut Vec<String>) {
    if node.kind() == "expression"
        && let Some(address) = ref_address_of(node, src)
    {
        out.push(address);
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_ref_addresses(child, src, out);
    }
}

/// If `expr`'s first named children are `variable_expr` then `get_attr`,
/// return the resource address "<variable_expr>.<get_attr>".
fn ref_address_of(expr: TsNode, src: &[u8]) -> Option<String> {
    let first = expr.named_child(0)?;
    if first.kind() != "variable_expr" {
        return None;
    }
    let second = expr.named_child(1)?;
    if second.kind() != "get_attr" {
        return None;
    }
    let ty = text_of(first, src).trim();
    let name = second.named_child(0).map(|n| text_of(n, src)).unwrap_or("");
    if ty.is_empty() || name.is_empty() {
        return None;
    }
    Some(format!("{ty}.{name}"))
}

/// The 0-based row a node starts on: the `line` of the `CallSite` /
/// `UnresolvedRef` / `ImportStmt` it asserts (LC.3b, POSITION convention).
fn line_at(n: TsNode) -> u32 {
    u32::try_from(n.start_position().row).unwrap_or(u32::MAX)
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
            payload: CellPayload::Text(format!(
                "{}:{}-{}",
                file_rel,
                root.start_position().row + 1,
                root.end_position().row + 1,
            )),
        },
    ]
}

fn entity_cells(node: &TsNode, src: &[u8], file_rel: &str) -> Vec<Cell> {
    vec![
        Cell {
            kind: cell_type::CODE,
            payload: CellPayload::Text(text_of(*node, src).to_string()),
        },
        Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Text(format!(
                "{}:{}-{}",
                file_rel,
                node.start_position().row + 1,
                node.end_position().row + 1,
            )),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> RepoId {
        RepoId(1)
    }

    #[test]
    fn resources_and_variables() {
        let source = r#"
resource "aws_instance" "web" {
  ami           = "ami-12345"
  instance_type = "t2.micro"
}

variable "region" {
  default = "us-east-1"
}

output "instance_id" {
  value = aws_instance.web.id
}
"#;
        let fp = parse_file(source, "main.tf", "main", repo()).unwrap();
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::INFRA_RESOURCE).count(), 1);
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::STRUCT).count(), 0);
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::FUNCTION).count(), 2);
    }

    /// `address` is the Terraform address, e.g. "aws_vpc.main".
    fn node_id(address: &str) -> NodeId {
        let (ty, name) = address.split_once('.').expect("ty.name");
        let qname = resource_qname(ty, name);
        NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::INFRA_RESOURCE, &qname)
    }

    #[test]
    fn infra_references_between_resources() {
        let source = r#"
resource "aws_vpc" "main" {
  cidr_block = "10.0.0.0/16"
}

resource "aws_subnet" "web" {
  vpc_id     = aws_vpc.main.id
  cidr_block = "10.0.1.0/24"
}

resource "aws_instance" "app" {
  ami       = "ami-123456"
  subnet_id = aws_subnet.web.id
}
"#;
        let fp = parse_file(source, "main.tf", "main", repo()).unwrap();

        // All three resources are INFRA_RESOURCE nodes.
        assert_eq!(
            fp.nav.kind_by_id.values().filter(|k| **k == node_kind::INFRA_RESOURCE).count(),
            3
        );

        let subnet = node_id("aws_subnet.web");
        let vpc = node_id("aws_vpc.main");
        let instance = node_id("aws_instance.app");

        let has_ref = |from: NodeId, to: NodeId| {
            fp.edges.iter().any(|e| {
                e.from == from && e.to == to && e.category == edge_category::INFRA_REFERENCES
            })
        };
        assert!(has_ref(subnet, vpc), "aws_subnet -> aws_vpc via vpc_id");
        assert!(has_ref(instance, subnet), "aws_instance -> aws_subnet via subnet_id");

        // No spurious references (e.g. from string literals or self).
        assert_eq!(
            fp.edges.iter().filter(|e| e.category == edge_category::INFRA_REFERENCES).count(),
            2
        );
    }

    #[test]
    fn depends_on_edges() {
        let source = r#"
resource "aws_iam_role" "app" {
  name = "app-role"
}

resource "aws_s3_bucket" "data" {
  bucket = "my-app-data"
}

resource "aws_instance" "worker" {
  ami = "ami-123456"

  depends_on = [
    aws_iam_role.app,
    aws_s3_bucket.data,
  ]
}
"#;
        let fp = parse_file(source, "main.tf", "main", repo()).unwrap();

        let worker = node_id("aws_instance.worker");
        let role = node_id("aws_iam_role.app");
        let bucket = node_id("aws_s3_bucket.data");

        let has_dep = |to: NodeId| {
            fp.edges
                .iter()
                .any(|e| e.from == worker && e.to == to && e.category == edge_category::DEPENDS_ON)
        };
        assert!(has_dep(role), "worker depends_on aws_iam_role.app");
        assert!(has_dep(bucket), "worker depends_on aws_s3_bucket.data");
        assert_eq!(
            fp.edges.iter().filter(|e| e.category == edge_category::DEPENDS_ON).count(),
            2
        );
    }

    #[test]
    fn resource_qname_is_canonical_infra_shape() {
        let source = r#"
resource "aws_ecs_service" "api" {
  name = "api"
}

resource "aws_s3_bucket" "data" {
  bucket = "my-app-data"
}
"#;
        let fp = parse_file(source, "infra/main.tf", "infra::main", repo()).unwrap();
        let qnames: Vec<&String> = fp.nav.qname_by_id.values().collect();
        // Aliased type: joins the k8s/compose `service` kind emitted by iac.rs.
        assert!(
            qnames.iter().any(|q| *q == "infra:service:api"),
            "aws_ecs_service.api -> infra:service:api, got {qnames:?}"
        );
        // Unaliased type: uniform shape, no false k8s join.
        assert!(
            qnames.iter().any(|q| *q == "infra:aws_s3_bucket:data"),
            "aws_s3_bucket.data -> infra:aws_s3_bucket:data, got {qnames:?}"
        );
        // The module path must NOT leak into the resource identity any more —
        // that is what stopped terraform joining anything cross-repo.
        assert!(
            !qnames.iter().any(|q| q.contains("infra::main::")),
            "module path leaked into an INFRA_RESOURCE qname: {qnames:?}"
        );
        // NAME is unchanged: the Terraform address a reader recognises.
        let names: Vec<&String> = fp.nav.name_by_id.values().collect();
        assert!(names.iter().any(|n| *n == "aws_ecs_service.api"), "{names:?}");
    }

    #[test]
    fn modules_with_source() {
        let source = r#"
module "vpc" {
  source = "./modules/vpc"
}
"#;
        let fp = parse_file(source, "main.tf", "main", repo()).unwrap();
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::PACKAGE).count(), 1);
        assert_eq!(fp.imports.len(), 1);
    }

    /// DF.14: the JSON CODE cells of every Pub/Sub resource in `fp`, by
    /// Terraform address, in source order (`None` = the block has no cell).
    fn pubsub_cells(fp: &FileParse) -> Vec<(String, Option<String>)> {
        let mut out = Vec::new();
        for n in &fp.nodes {
            let Some(name) = fp.nav.name_by_id.get(&n.id) else { continue };
            if !name.starts_with("google_pubsub_") {
                continue;
            }
            let json = n.cells.iter().find_map(|c| match &c.payload {
                CellPayload::Json(j) if c.kind == cell_type::CODE => Some(j.clone()),
                _ => None,
            });
            out.push((name.clone(), json));
        }
        out
    }

    #[test]
    fn pubsub_topic_and_subscription_cells() {
        let source = r#"
resource "google_pubsub_topic" "orders" {
  name = "orders"
}

resource "google_pubsub_topic" "dynamic" {
  name = "${var.env}-orders"
}

resource "google_pubsub_subscription" "by_name" {
  name  = "orders-worker"
  topic = google_pubsub_topic.orders.name
}

resource "google_pubsub_subscription" "by_id" {
  name  = "orders-audit"
  topic = google_pubsub_topic.orders.id
}

resource "google_pubsub_subscription" "literal" {
  name  = "orders-mail"
  topic = "orders"
}

resource "google_pubsub_subscription" "full_path" {
  name  = "orders-bi"
  topic = "projects/my-project/topics/orders"
}

resource "google_pubsub_subscription" "templated_project" {
  name  = "orders-ops"
  topic = "projects/${var.project}/topics/orders"
}

resource "google_pubsub_subscription" "interpolated_name" {
  name  = "${var.env}-worker"
  topic = google_pubsub_topic.orders.name
}

resource "google_pubsub_subscription" "var_topic" {
  name  = "orders-x"
  topic = var.topic
}

resource "google_pubsub_subscription" "templated_topic" {
  name  = "orders-y"
  topic = "projects/p/topics/${var.topic}"
}

resource "google_pubsub_subscription" "other_attr" {
  name  = "orders-z"
  topic = google_pubsub_topic.orders.labels
}
"#;
        let fp = parse_file(source, "infra/pubsub.tf", "infra::pubsub", repo()).unwrap();
        let sub = |name: &str, field: &str, value: &str| {
            Some(format!(r#"{{"pubsub":"subscription","name":"{name}","{field}":"{value}"}}"#))
        };
        let got = pubsub_cells(&fp);
        let want: Vec<(String, Option<String>)> = vec![
            ("google_pubsub_topic.orders".into(), Some(r#"{"pubsub":"topic","name":"orders"}"#.into())),
            ("google_pubsub_topic.dynamic".into(), None),
            ("google_pubsub_subscription.by_name".into(), sub("orders-worker", "topic_ref", "google_pubsub_topic.orders")),
            ("google_pubsub_subscription.by_id".into(), sub("orders-audit", "topic_ref", "google_pubsub_topic.orders")),
            ("google_pubsub_subscription.literal".into(), sub("orders-mail", "topic", "orders")),
            ("google_pubsub_subscription.full_path".into(), sub("orders-bi", "topic", "orders")),
            ("google_pubsub_subscription.templated_project".into(), sub("orders-ops", "topic", "orders")),
            ("google_pubsub_subscription.interpolated_name".into(), None),
            ("google_pubsub_subscription.var_topic".into(), None),
            ("google_pubsub_subscription.templated_topic".into(), None),
            ("google_pubsub_subscription.other_attr".into(), None),
        ];
        assert_eq!(got, want);

        // The block's source slice stays the FIRST CODE cell; the JSON cell
        // comes after entity_cells' CODE + POSITION.
        let topic = fp
            .nodes
            .iter()
            .find(|n| fp.nav.name_by_id.get(&n.id).map(String::as_str) == Some("google_pubsub_topic.orders"))
            .unwrap();
        let kinds: Vec<_> = topic.cells.iter().map(|c| c.kind).collect();
        assert_eq!(kinds, [cell_type::CODE, cell_type::POSITION, cell_type::CODE]);
        assert!(matches!(&topic.cells[0].payload, CellPayload::Text(t) if t.starts_with("resource \"google_pubsub_topic\"")));

        // A non-Pub/Sub resource gains nothing.
        let other = parse_file("resource \"aws_sqs_queue\" \"q\" {\n  name = \"orders\"\n}\n", "q.tf", "q", repo()).unwrap();
        assert!(other.nodes.iter().all(|n| n.cells.iter().all(|c| !matches!(c.payload, CellPayload::Json(_)))));
    }

    #[test]
    fn data_source() {
        let source = r#"
data "aws_ami" "ubuntu" {
  most_recent = true
}
"#;
        let fp = parse_file(source, "data.tf", "data", repo()).unwrap();
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::STRUCT).count(), 1);
    }
}
