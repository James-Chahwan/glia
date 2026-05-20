//! Cross-language SERVICE classification + CONTAINS edges.
//!
//! Promotes a CLASS / STRUCT node to also emit a parallel SERVICE node (same
//! qname, kind=SERVICE → different NodeId) when the class matches a recognised
//! service shape:
//!
//!   - Go: `type Foo struct { ... }` with name ending in `Service` / `Server`,
//!         OR ≥3 method-receivers across the module + a `New<Name>` constructor.
//!   - Python: class decorated with `@dataclass` + ≥1 def, OR an explicit
//!             `# @service` opt-in line above the class declaration.
//!   - Rust: `impl Foo { ... }` with ≥2 `pub fn` and ≥1 `async fn`.
//!   - Java/Kotlin: class annotated with `@Service` or `@RestController`.
//!   - TypeScript: class decorated with `@Injectable(...)` (NestJS;
//!                 Angular's case is handled by `angular.rs` for the SERVICE
//!                 *node*, but CONTAINS edges to its methods land here so the
//!                 logic stays in one place).
//!
//! For each detected class, emits CONTAINS edges from the new SERVICE node to
//! every METHOD child of the matching CLASS / STRUCT node, looked up via the
//! per-file `CodeNav`. The CLASS / STRUCT node is left intact; SERVICE is a
//! parallel classification that downstream consumers (e.g. neuropil's hex
//! containment) walk via CONTAINS to find the service's methods.
//!
//! Implementation notes:
//!   - Pattern detection is regex/text-based, not AST. Matches the style of
//!     the rest of the extractors crate.
//!   - Per-class child lookup is O(1) via a transient qname→NodeId map built
//!     from the supplied nav.
//!   - The same logical class may be matched by multiple language paths
//!     (e.g. Angular `@Injectable` + NestJS `@Injectable`); the deterministic
//!     NodeId + edge dedupe set keeps the output unique.

use std::collections::{HashMap, HashSet};

use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, edge_category, node_kind};
use repo_graph_core::{Confidence, Edge, Node, NodeId, RepoId};

pub struct ServicesOut {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub nav: CodeNav,
}

/// Scan `source` for service patterns appropriate to `lang` and emit SERVICE
/// nodes + CONTAINS edges to the methods owned by each matching class.
///
/// `fp_nav` is the per-file nav after the language parser has run; it provides
/// the CLASS / STRUCT qname→NodeId mapping and children_of so this extractor
/// doesn't have to re-walk the AST.
pub fn extract_service_nodes(
    source: &str,
    lang: &str,
    fp_nav: &CodeNav,
    _module_id: NodeId,
    repo: RepoId,
) -> ServicesOut {
    let mut out = ServicesOut {
        nodes: Vec::new(),
        edges: Vec::new(),
        nav: CodeNav::default(),
    };

    let qname_to_class: HashMap<&str, NodeId> = fp_nav
        .qname_by_id
        .iter()
        .filter_map(|(id, qn)| {
            let kind = fp_nav.kind_by_id.get(id).copied()?;
            if kind == node_kind::CLASS || kind == node_kind::STRUCT {
                Some((qn.as_str(), *id))
            } else {
                None
            }
        })
        .collect();

    let names = match lang {
        "go" => detect_go_services(source),
        "python" => detect_python_services(source),
        "rust" => detect_rust_services(source),
        "java" => detect_java_services(source),
        "typescript" | "react" | "angular" | "vue" => detect_ts_services(source),
        _ => Vec::new(),
    };

    let mut seen: HashSet<NodeId> = HashSet::new();
    let mut seen_edges: HashSet<(NodeId, NodeId)> = HashSet::new();

    for name in names {
        let candidates: Vec<(&str, NodeId)> = qname_to_class
            .iter()
            .filter(|(qn, _)| matches_class_qname(qn, &name))
            .map(|(qn, id)| (*qn, *id))
            .collect();
        for (qname, class_id) in candidates {
            let service_id =
                NodeId::from_parts(GRAPH_TYPE, repo, node_kind::SERVICE, qname);
            if seen.insert(service_id) {
                out.nodes.push(Node {
                    id: service_id,
                    repo,
                    confidence: Confidence::Medium,
                    cells: Vec::new(),
                });
                let parent = fp_nav.parent_of.get(&class_id).copied();
                out.nav
                    .record(service_id, &name, qname, node_kind::SERVICE, parent);
            }

            if let Some(children) = fp_nav.children_of.get(&class_id) {
                for &child_id in children {
                    let child_kind = fp_nav.kind_by_id.get(&child_id).copied();
                    if child_kind == Some(node_kind::METHOD)
                        || child_kind == Some(node_kind::FUNCTION)
                    {
                        if seen_edges.insert((service_id, child_id)) {
                            out.edges.push(Edge {
                                from: service_id,
                                to: child_id,
                                category: edge_category::CONTAINS,
                                confidence: Confidence::Medium,
                            });
                        }
                    }
                }
            }
        }
    }

    out
}

/// `qname` is full `module::...::Class`. Match by last segment.
fn matches_class_qname(qname: &str, name: &str) -> bool {
    qname.rsplit("::").next().is_some_and(|leaf| leaf == name)
}

// ----------------------------------------------------------------------------
// Per-language pattern detection — returns simple class names.
// ----------------------------------------------------------------------------

fn detect_go_services(source: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();

    let mut types: Vec<String> = Vec::new();
    for line in source.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("type ") {
            let name_end = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(rest.len());
            if name_end > 0 {
                let name = &rest[..name_end];
                if rest[name_end..].trim_start().starts_with("struct")
                    || rest[name_end..].trim_start().starts_with("interface")
                {
                    types.push(name.to_string());
                }
            }
        }
    }

    let mut receiver_counts: HashMap<String, usize> = HashMap::new();
    for line in source.lines() {
        let t = line.trim_start();
        if !t.starts_with("func ") {
            continue;
        }
        if let Some(open) = t.find('(')
            && let Some(close_rel) = t[open + 1..].find(')')
        {
            let receiver = t[open + 1..open + 1 + close_rel].trim();
            if !receiver.is_empty() {
                let inner = receiver.split_whitespace().last().unwrap_or("");
                let typename = inner.trim_start_matches('*');
                if types.iter().any(|x| x == typename) {
                    *receiver_counts.entry(typename.to_string()).or_insert(0) += 1;
                }
            }
        }
    }

    let has_constructor = |name: &str| -> bool {
        let needle = format!("func New{name}(");
        source.contains(&needle)
    };

    for t in &types {
        let suffix_match = t.ends_with("Service") || t.ends_with("Server");
        let receiver_match = receiver_counts.get(t).copied().unwrap_or(0) >= 3
            && has_constructor(t);
        if suffix_match || receiver_match {
            out.push(t.clone());
        }
    }
    out.sort();
    out.dedup();
    out
}

fn detect_python_services(source: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let lines: Vec<&str> = source.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim_start();
        if !t.starts_with("class ") {
            continue;
        }
        let mut decor_dataclass = false;
        let mut decor_service = false;
        let mut j = i;
        while j > 0 {
            j -= 1;
            let p = lines[j].trim_start();
            if p.is_empty() {
                continue;
            }
            if p.starts_with("@dataclass") || p.starts_with("@dataclasses.dataclass") {
                decor_dataclass = true;
                continue;
            }
            if p.starts_with("@") {
                continue;
            }
            if p.starts_with("# @service") {
                decor_service = true;
                break;
            }
            break;
        }
        if !(decor_dataclass || decor_service) {
            continue;
        }

        let rest = &t["class ".len()..];
        let name_end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        if name_end == 0 {
            continue;
        }
        let name = &rest[..name_end];

        let mut has_method = false;
        let class_indent = line.len() - line.trim_start().len();
        for body_line in &lines[i + 1..] {
            let bt = body_line.trim_start();
            if bt.is_empty() || bt.starts_with('#') {
                continue;
            }
            let indent = body_line.len() - body_line.trim_start().len();
            if indent <= class_indent {
                break;
            }
            if bt.starts_with("def ") || bt.starts_with("async def ") {
                has_method = true;
                break;
            }
        }

        if has_method || decor_service {
            out.push(name.to_string());
        }
    }
    out.sort();
    out.dedup();
    out
}

fn detect_rust_services(source: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let bytes = source.as_bytes();
    let mut i = 0;
    while i + 5 <= bytes.len() {
        if &bytes[i..i + 5] == b"impl " {
            let line_start = source[..i]
                .rfind('\n')
                .map(|p| p + 1)
                .unwrap_or(0);
            let prefix = source[line_start..i].trim();
            if !prefix.is_empty() {
                i += 1;
                continue;
            }
            let rest = &source[i + 5..];
            let mut idx = 0;
            let rb = rest.as_bytes();
            while idx < rb.len()
                && !rb[idx].is_ascii_whitespace()
                && rb[idx] != b'{'
                && rb[idx] != b'<'
            {
                idx += 1;
            }
            let target = rest[..idx].trim();
            if target.is_empty() || target.contains(" for ") {
                i += 1;
                continue;
            }
            if let Some(body_start) = rest.find('{') {
                let mut depth = 1i32;
                let body_bytes = &rest.as_bytes()[body_start + 1..];
                let mut k = 0;
                while k < body_bytes.len() && depth > 0 {
                    match body_bytes[k] {
                        b'{' => depth += 1,
                        b'}' => depth -= 1,
                        _ => {}
                    }
                    k += 1;
                }
                let body = &rest[body_start + 1..body_start + k];
                let pub_fns = body.matches("pub fn ").count()
                    + body.matches("pub async fn ").count();
                let async_fns = body.matches("async fn ").count();
                if pub_fns >= 2 && async_fns >= 1 {
                    out.push(target.to_string());
                }
                i += 5 + body_start + k;
                continue;
            }
        }
        i += 1;
    }
    out.sort();
    out.dedup();
    out
}

fn detect_java_services(source: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let needles = ["@Service", "@RestController", "@Controller"];
    for needle in needles {
        let mut from = 0;
        while let Some(rel) = source[from..].find(needle) {
            let pos = from + rel;
            let after = pos + needle.len();
            from = after;
            if let Some(name) = find_next_java_class_or_kotlin(&source[after..]) {
                out.push(name);
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn find_next_java_class_or_kotlin(s: &str) -> Option<String> {
    for line in s.lines() {
        let t = line.trim_start();
        if t.is_empty() || t.starts_with("//") || t.starts_with("/*") || t.starts_with("*") {
            continue;
        }
        if t.starts_with('@') {
            continue;
        }
        let stripped = t
            .trim_start_matches("public ")
            .trim_start_matches("final ")
            .trim_start_matches("abstract ")
            .trim_start_matches("open ")
            .trim_start_matches("sealed ")
            .trim_start_matches("data ");
        let rest = stripped.strip_prefix("class ")?;
        let name_end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        if name_end == 0 {
            return None;
        }
        return Some(rest[..name_end].to_string());
    }
    None
}

fn detect_ts_services(source: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut from = 0;
    while let Some(rel) = source[from..].find("@Injectable(") {
        let pos = from + rel;
        let arg_start = pos + "@Injectable(".len();
        let close = match find_balanced_paren(&source[arg_start..]) {
            Some(off) => arg_start + off,
            None => {
                from = arg_start;
                continue;
            }
        };
        if let Some(name) = find_next_ts_class_name(&source[close + 1..]) {
            out.push(name);
        }
        from = close + 1;
    }
    out.sort();
    out.dedup();
    out
}

fn find_balanced_paren(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut depth = 1i32;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 && c == b')' {
                    return Some(i);
                }
            }
            b'\'' | b'"' | b'`' => {
                let delim = c;
                i += 1;
                while i < bytes.len() && bytes[i] != delim {
                    if bytes[i] == b'\\' && i + 1 < bytes.len() {
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn find_next_ts_class_name(s: &str) -> Option<String> {
    for line in s.lines() {
        let t = line.trim_start();
        if t.is_empty()
            || t.starts_with("//")
            || t.starts_with("/*")
            || t.starts_with("*")
            || t.starts_with(')')
        {
            continue;
        }
        if t.starts_with('@') {
            continue;
        }
        let stripped = t
            .trim_start_matches("export ")
            .trim_start_matches("default ")
            .trim_start_matches("abstract ");
        let rest = stripped.strip_prefix("class ")?;
        let name_end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$'))
            .unwrap_or(rest.len());
        if name_end == 0 {
            return None;
        }
        return Some(rest[..name_end].to_string());
    }
    None
}

// ----------------------------------------------------------------------------
// Tests
// ----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use repo_graph_code_domain::node_kind;

    fn repo() -> RepoId {
        RepoId(1)
    }

    fn nav_with_class(qname: &str, methods: &[&str]) -> (CodeNav, NodeId, Vec<NodeId>) {
        let mut nav = CodeNav::default();
        let module_id =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "m");
        nav.record(module_id, "m", "m", node_kind::MODULE, None);
        let class_id =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, qname);
        nav.record(
            class_id,
            qname.rsplit("::").next().unwrap_or(qname),
            qname,
            node_kind::CLASS,
            Some(module_id),
        );
        let mut method_ids = Vec::new();
        for m in methods {
            let mq = format!("{qname}::{m}");
            let id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::METHOD, &mq);
            nav.record(id, m, &mq, node_kind::METHOD, Some(class_id));
            method_ids.push(id);
        }
        (nav, class_id, method_ids)
    }

    #[test]
    fn ts_injectable_promotes_to_service_with_contains_edges() {
        let src = r#"
@Injectable({ providedIn: 'root' })
export class UserService {
  login() {}
  logout() {}
}
"#;
        let (nav, _class, methods) = nav_with_class("m::UserService", &["login", "logout"]);
        let module_id =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "m");
        let out = extract_service_nodes(src, "typescript", &nav, module_id, repo());
        assert_eq!(out.nodes.len(), 1, "got nodes: {:?}", out.nodes);
        assert_eq!(out.nodes[0].id.0, NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::SERVICE, "m::UserService").0);
        assert_eq!(out.edges.len(), 2);
        let to_set: std::collections::HashSet<NodeId> =
            out.edges.iter().map(|e| e.to).collect();
        for m in methods {
            assert!(to_set.contains(&m));
        }
        for e in &out.edges {
            assert_eq!(e.category, edge_category::CONTAINS);
        }
    }

    #[test]
    fn go_service_by_name_suffix() {
        let src = r#"
package svc

type UserService struct {
    db *DB
}

func NewUserService(db *DB) *UserService {
    return &UserService{db: db}
}

func (u *UserService) Login() {}
"#;
        let names = detect_go_services(src);
        assert!(names.contains(&"UserService".to_string()), "got: {names:?}");
    }

    #[test]
    fn go_service_by_receiver_count() {
        let src = r#"
package svc

type Foo struct{}

func NewFoo() *Foo { return &Foo{} }
func (f *Foo) A() {}
func (f *Foo) B() {}
func (f *Foo) C() {}
"#;
        let names = detect_go_services(src);
        assert!(names.contains(&"Foo".to_string()), "got: {names:?}");
    }

    #[test]
    fn go_struct_without_constructor_not_promoted() {
        let src = r#"
package svc

type Bag struct{}

func (b *Bag) X() {}
func (b *Bag) Y() {}
"#;
        let names = detect_go_services(src);
        assert!(!names.contains(&"Bag".to_string()), "got: {names:?}");
    }

    #[test]
    fn python_dataclass_with_methods() {
        let src = r#"
@dataclass
class UserRepo:
    db: DB
    def find(self): pass
    def save(self): pass
"#;
        let names = detect_python_services(src);
        assert!(names.contains(&"UserRepo".to_string()), "got: {names:?}");
    }

    #[test]
    fn python_plain_class_not_promoted() {
        let src = r#"
class Plain:
    def x(self): pass
"#;
        let names = detect_python_services(src);
        assert!(!names.contains(&"Plain".to_string()), "got: {names:?}");
    }

    #[test]
    fn python_service_opt_in_comment() {
        let src = r#"
# @service
class Manual:
    pass
"#;
        let names = detect_python_services(src);
        assert!(names.contains(&"Manual".to_string()), "got: {names:?}");
    }

    #[test]
    fn rust_impl_with_async_pub_fns() {
        let src = r#"
pub struct AuthSvc;

impl AuthSvc {
    pub async fn login(&self) {}
    pub fn logout(&self) {}
}
"#;
        let names = detect_rust_services(src);
        assert!(names.contains(&"AuthSvc".to_string()), "got: {names:?}");
    }

    #[test]
    fn rust_trait_impl_skipped() {
        let src = r#"
impl Display for Foo {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result { Ok(()) }
}
"#;
        let names = detect_rust_services(src);
        assert!(names.is_empty(), "got: {names:?}");
    }

    #[test]
    fn java_service_annotation() {
        let src = r#"
@Service
public class UserService {
    public void login() {}
}
"#;
        let names = detect_java_services(src);
        assert!(names.contains(&"UserService".to_string()), "got: {names:?}");
    }

    #[test]
    fn java_restcontroller_annotation() {
        let src = r#"
@RestController
@RequestMapping("/users")
public class UserController { }
"#;
        let names = detect_java_services(src);
        assert!(names.contains(&"UserController".to_string()), "got: {names:?}");
    }
}
