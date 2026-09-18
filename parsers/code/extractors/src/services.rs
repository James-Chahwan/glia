//! Cross-language SERVICE classification + CONTAINS edges.
//!
//! SERVICE is the service-LAYER stereotype of a code unit: a DI bean, a
//! controller, a hosted worker, an OTP process. It is carried on the
//! declaration itself as the ROLE cell LB.3's build-time fold writes
//! (`graph::roles`, read through `roles_in`). Deployable services are a
//! different question with a different answer: `glia arch` keys them by
//! PROJECT root (`ServiceKeying`, `engine/src/arch.rs`). The two are not
//! interchangeable — a monorepo has a handful of deployables and hundreds of
//! service-layer classes.
//!
//! Promotes a CLASS / STRUCT node (an Elixir `defmodule` PACKAGE) to also emit
//! a parallel SERVICE overlay (same qname, kind=SERVICE → different NodeId)
//! when the declaration matches a recognised service shape. The fold then
//! merges the overlay into the declaration, so no SERVICE twin survives a
//! build:
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
//! LA.21a: six more languages, each rule named so the per-file marker counts
//! it. The controller rules also accept a `*Controller` class whose parent's
//! name ends in `Controller` (an app's own base controller), because the
//! framework base is then one file away:
//!
//!   - C#: `[ApiController]` on the class, or a base `ControllerBase` /
//!     `Controller` (`aspnet_controller`); a base `BackgroundService` /
//!     `IHostedService` (`hosted_service`); `class X : IX` — the class
//!     implements the interface named `I` + its own name, the .NET DI
//!     registration convention (`iface_service`); a base `Hub` / `Hub<T>`
//!     (`signalr_hub`).
//!   - PHP: `extends Controller` / `extends AbstractController` /
//!     `#[AsController]` (`controller`); a class named `*Service` in a
//!     namespace with a `Services` / `Service` segment
//!     (`services_namespace`).
//!   - Ruby: `< ApplicationController` / `< ActionController::Base` /
//!     `< ActionController::API` (`rails_controller`); a class named
//!     `*Service` whose body defines `call` / `self.call` / `perform`
//!     (`service_object`).
//!   - Scala: `extends` / `with` `AbstractController` / `BaseController` /
//!     `InjectedController` (`play_controller`); a class named
//!     `*Service` under `@Singleton` or with an `@Inject` constructor
//!     (`di_service`).
//!   - Elixir: a `defmodule` whose OWN body (not a nested `def` / `quote`)
//!     says `use GenServer` / `Agent` / `Supervisor` /
//!     `DynamicSupervisor` (`otp_process`), `use Phoenix.Controller` /
//!     `Phoenix.LiveView` / `Phoenix.Channel` or
//!     `use <App>Web, :controller` / `:live_view` / `:channel`
//!     (`phoenix`), `use Oban.Worker` (`oban_worker`). The module is a
//!     PACKAGE, so only for Elixir the owner index admits PACKAGE.
//!   - Dart: `@injectable` / `@Injectable()` / `@lazySingleton` /
//!     `@LazySingleton()` / `@singleton` / `@Singleton()` directly above
//!     the class (`injectable`); `extends GetxService` /
//!     `extends GetxController` (`getx`).
//!
//! Every rule is a framework convention (HEURISTIC tier); a name is emitted
//! only when the per-file nav holds a declaration with that name, so a scan
//! never mints a phantom. Each file the six languages classify prints
//! `[services] <lang> classified=N rules=<rule>:<n>,... module=<module qname>`
//! on stderr.
//!
//! For each detected class, emits CONTAINS edges from the new SERVICE node to
//! every METHOD / FUNCTION child of the matching declaration, looked up via
//! the per-file `CodeNav`. The declaration is left intact; the fold drops the
//! CONTAINS edges that duplicate its DEFINES and moves the rest onto it.
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
    module_id: NodeId,
    repo: RepoId,
) -> ServicesOut {
    let mut out = ServicesOut {
        nodes: Vec::new(),
        edges: Vec::new(),
        nav: CodeNav::default(),
    };

    // An Elixir `defmodule` is a PACKAGE: the unit its `use` lines classify.
    // Other languages' PACKAGEs (namespaces, Ruby modules) are never owners.
    let admits_package = lang == "elixir";
    let qname_to_class: HashMap<&str, NodeId> = fp_nav
        .qname_by_id
        .iter()
        .filter_map(|(id, qn)| {
            let kind = fp_nav.kind_by_id.get(id).copied()?;
            if kind == node_kind::CLASS
                || kind == node_kind::STRUCT
                || (admits_package && kind == node_kind::PACKAGE)
            {
                Some((qn.as_str(), *id))
            } else {
                None
            }
        })
        .collect();

    // LA.21a: the rule-carrying detectors. Empty for every other language.
    let hits: Vec<RuleHit> = match lang {
        "csharp" => detect_csharp_services(source),
        "php" => detect_php_services(source),
        "ruby" => detect_ruby_services(source),
        "scala" => detect_scala_services(source),
        "elixir" => detect_elixir_services(source),
        "dart" => detect_dart_services(source),
        _ => Vec::new(),
    };

    let names = match lang {
        "go" => detect_go_services(source),
        "python" => detect_python_services(source),
        "rust" => detect_rust_services(source),
        "java" => detect_java_services(source),
        "typescript" | "react" | "angular" | "vue" => detect_ts_services(source),
        _ => hit_names(&hits),
    };

    let mut seen: HashSet<NodeId> = HashSet::new();
    let mut seen_edges: HashSet<(NodeId, NodeId)> = HashSet::new();
    // How many declarations each name classified — the marker's rule tally.
    let mut owners_of: HashMap<String, usize> = HashMap::new();

    for name in names {
        // Sorted: two same-leaf declarations in one file emit in qname order,
        // not in HashMap order.
        let mut candidates: Vec<(&str, NodeId)> = qname_to_class
            .iter()
            .filter(|(qn, _)| matches_class_qname(qn, &name))
            .map(|(qn, id)| (*qn, *id))
            .collect();
        candidates.sort_unstable_by_key(|&(qn, _)| qn);
        if !hits.is_empty() && !candidates.is_empty() {
            owners_of.insert(name.clone(), candidates.len());
        }
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

    if !owners_of.is_empty() {
        let module = fp_nav
            .qname_by_id
            .get(&module_id)
            .map_or("", String::as_str);
        eprintln!(
            "{}",
            services_marker(
                lang,
                out.nodes.len(),
                &rule_tally(&hits, &owners_of),
                module
            )
        );
    }

    out
}

/// One classification by a rule-carrying detector: the declared name and the
/// rule that matched it.
type RuleHit = (String, &'static str);

/// The distinct names in `hits`, sorted.
fn hit_names(hits: &[RuleHit]) -> Vec<String> {
    let mut names: Vec<String> = hits.iter().map(|(n, _)| n.clone()).collect();
    names.sort();
    names.dedup();
    names
}

/// Per rule, the declarations it classified, in first-hit order (hits come in
/// source order, so the tally reads top to bottom). A name the nav does not
/// hold classified nothing and is not counted.
fn rule_tally(hits: &[RuleHit], owners_of: &HashMap<String, usize>) -> Vec<(&'static str, usize)> {
    let mut tally: Vec<(&'static str, usize)> = Vec::new();
    let mut counted: HashSet<(&str, &str)> = HashSet::new();
    for (name, rule) in hits {
        let Some(&owners) = owners_of.get(name) else {
            continue;
        };
        if !counted.insert((name.as_str(), *rule)) {
            continue;
        }
        match tally.iter_mut().find(|(r, _)| r == rule) {
            Some((_, n)) => *n += owners,
            None => tally.push((*rule, owners)),
        }
    }
    tally
}

/// The LA.21a fired_on line:
/// `[services] <lang> classified=N rules=<rule>:<n>,... module=<module qname>`.
fn services_marker(
    lang: &str,
    classified: usize,
    tally: &[(&'static str, usize)],
    module: &str,
) -> String {
    let rules: Vec<String> = tally.iter().map(|(r, n)| format!("{r}:{n}")).collect();
    format!(
        "[services] {lang} classified={classified} rules={} module={module}",
        rules.join(",")
    )
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
// LA.21a — rule-carrying detectors: C# / PHP / Ruby / Scala / Elixir / Dart.
// Each returns `(declared name, rule)` in source order.
// ----------------------------------------------------------------------------

/// A controller-named class whose parent is also controller-named: the app's
/// own base controller, which carries the framework base one file away.
fn controller_by_parent(name: &str, parent: &str) -> bool {
    name.ends_with("Controller") && parent.ends_with("Controller")
}

fn detect_csharp_services(source: &str) -> Vec<RuleHit> {
    const MODIFIERS: &[&str] = &[
        "public",
        "private",
        "protected",
        "internal",
        "static",
        "sealed",
        "abstract",
        "partial",
        "unsafe",
        "new",
        "file",
    ];
    let mut out = Vec::new();
    for d in class_decls(source, DecoStyle::Bracket, MODIFIERS, true) {
        let bases = csharp_bases(&d.header);
        let has = |b: &str| bases.iter().any(|x| x == b);
        let mut push = |rule| out.push((d.name.clone(), rule));
        if d.decos.iter().any(|a| a == "ApiController")
            || has("ControllerBase")
            || has("Controller")
            || bases.iter().any(|b| controller_by_parent(&d.name, b))
        {
            push("aspnet_controller");
        }
        if has("BackgroundService") || has("IHostedService") {
            push("hosted_service");
        }
        if has(&format!("I{}", d.name)) {
            push("iface_service");
        }
        if has("Hub") {
            push("signalr_hub");
        }
    }
    out
}

/// A C# declaration's base list: the depth-0 names after its `:` and before
/// any `where` clause, generic arguments and namespace qualification dropped.
fn csharp_bases(header: &str) -> Vec<String> {
    let toks = header_tokens(header, true);
    let Some(colon) = toks.iter().position(|t| t == ":") else {
        return Vec::new();
    };
    if toks[..colon].iter().any(|t| t == "where") {
        return Vec::new();
    }
    toks[colon + 1..]
        .iter()
        .take_while(|t| *t != "where")
        .filter(|t| *t != "," && *t != ":")
        .map(|t| last_segment(t).to_string())
        .collect()
}

fn detect_php_services(source: &str) -> Vec<RuleHit> {
    const MODIFIERS: &[&str] = &["final", "abstract", "readonly"];
    let mut out = Vec::new();
    for d in class_decls(source, DecoStyle::HashBracket, MODIFIERS, false) {
        let toks = header_tokens(&d.header, false);
        let parent = words_after(&toks, &["extends"]);
        let mut push = |rule| out.push((d.name.clone(), rule));
        if d.decos.iter().any(|a| a == "AsController")
            || parent
                .iter()
                .any(|p| p == "Controller" || p == "AbstractController")
            || parent.iter().any(|p| controller_by_parent(&d.name, p))
        {
            push("controller");
        }
        if d.name.ends_with("Service")
            && php_namespace_before(source, d.line_start)
                .is_some_and(|ns| ns.split('\\').any(|s| s == "Services" || s == "Service"))
        {
            push("services_namespace");
        }
    }
    out
}

/// The namespace in force at `pos`: the last `namespace X;` / `namespace X {`
/// line before it.
fn php_namespace_before(source: &str, pos: usize) -> Option<&str> {
    let mut found = None;
    for line in source[..pos.min(source.len())].lines() {
        if let Some(rest) = line.trim_start().strip_prefix("namespace ") {
            let end = rest
                .find(|c: char| c == ';' || c == '{' || c.is_whitespace())
                .unwrap_or(rest.len());
            if end > 0 {
                found = Some(&rest[..end]);
            }
        }
    }
    found
}

fn detect_ruby_services(source: &str) -> Vec<RuleHit> {
    let mut out = Vec::new();
    let lines: Vec<&str> = source.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let Some(rest) = line.trim_start().strip_prefix("class ") else {
            continue;
        };
        let rest = rest.trim_start();
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == ':'))
            .unwrap_or(rest.len());
        let name = rest[..end].rsplit("::").next().unwrap_or("");
        // `class << self` has no constant path; `class foo` is not a class.
        if !name.starts_with(|c: char| c.is_ascii_uppercase()) {
            continue;
        }
        let parent = rest[end..].trim_start().strip_prefix('<').map(|p| {
            let p = p.trim_start();
            let e = p
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == ':'))
                .unwrap_or(p.len());
            p[..e].trim_start_matches("::")
        });
        if let Some(parent) = parent
            && (matches!(
                parent,
                "ApplicationController" | "ActionController::Base" | "ActionController::API"
            ) || controller_by_parent(name, parent))
        {
            out.push((name.to_string(), "rails_controller"));
        }
        if name.ends_with("Service") && ruby_body_defines_entry(&lines, i) {
            out.push((name.to_string(), "service_object"));
        }
    }
    out
}

/// True when the class opened on `lines[at]` defines `call`, `self.call` or
/// `perform` in its body: the lines indented deeper than the `class` line,
/// up to the first one that is not.
fn ruby_body_defines_entry(lines: &[&str], at: usize) -> bool {
    let indent = |l: &str| l.len() - l.trim_start().len();
    let class_indent = indent(lines[at]);
    for line in &lines[at + 1..] {
        let t = line.trim_start();
        if t.is_empty() {
            continue;
        }
        if indent(line) <= class_indent {
            return false;
        }
        if let Some(rest) = t.strip_prefix("def ") {
            let rest = rest.trim_start();
            for entry in ["self.call", "call", "perform"] {
                if let Some(after) = rest.strip_prefix(entry)
                    && !after.starts_with(|c: char| c.is_ascii_alphanumeric() || "_?!.".contains(c))
                {
                    return true;
                }
            }
        }
    }
    false
}

fn detect_scala_services(source: &str) -> Vec<RuleHit> {
    const MODIFIERS: &[&str] = &[
        "final",
        "abstract",
        "sealed",
        "case",
        "private",
        "protected",
        "implicit",
        "open",
    ];
    let mut out = Vec::new();
    for d in class_decls(source, DecoStyle::At, MODIFIERS, false) {
        let toks = header_tokens(&d.header, false);
        let parents = words_after(&toks, &["extends", "with"]);
        let mut push = |rule| out.push((d.name.clone(), rule));
        if parents.iter().any(|p| {
            matches!(
                p.as_str(),
                "AbstractController" | "BaseController" | "InjectedController"
            ) || controller_by_parent(&d.name, p)
        }) {
            push("play_controller");
        }
        let injected = toks
            .iter()
            .any(|t| t == "@Inject" || t.ends_with(".Inject"))
            || d.decos.iter().any(|a| a == "Inject");
        if d.name.ends_with("Service") && (injected || d.decos.iter().any(|a| a == "Singleton")) {
            push("di_service");
        }
    }
    out
}

fn detect_dart_services(source: &str) -> Vec<RuleHit> {
    const MODIFIERS: &[&str] = &["abstract", "base", "final", "interface", "sealed", "mixin"];
    const DI: &[&str] = &[
        "injectable",
        "Injectable",
        "lazySingleton",
        "LazySingleton",
        "singleton",
        "Singleton",
    ];
    let mut out = Vec::new();
    for d in class_decls(source, DecoStyle::At, MODIFIERS, true) {
        if d.decos.iter().any(|a| DI.contains(&a.as_str())) {
            out.push((d.name.clone(), "injectable"));
        }
        let toks = header_tokens(&d.header, true);
        if words_after(&toks, &["extends"])
            .iter()
            .any(|p| p == "GetxService" || p == "GetxController")
        {
            out.push((d.name.clone(), "getx"));
        }
    }
    out
}

/// Elixir: classify each `defmodule` by the `use` lines of its OWN body.
///
/// A `use` belongs to the innermost `defmodule` still open, and only when it
/// sits at that module's body depth — a `use` inside a `def`, a `quote` or a
/// nested module does not classify the outer one (Phoenix's `MyAppWeb`
/// module says `use Phoenix.Controller` inside `quote` for every controller).
/// Depth counts `do` / `fn` openers and `end` closers over the source with
/// strings, charlists, sigils and comments blanked, so none of those move it;
/// `do:` / `end:` keyword keys and `:do` / `:end` atoms are not keywords.
fn detect_elixir_services(source: &str) -> Vec<RuleHit> {
    let code = elixir_code_only(source);
    let b = code.as_slice();
    let word_at = |s: usize, e: usize| std::str::from_utf8(&b[s..e]).unwrap_or("");
    let mut out = Vec::new();
    let mut depth = 0usize;
    // (module name as written, depth of its body)
    let mut open: Vec<(String, usize)> = Vec::new();
    let mut pending: Option<String> = None;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if !(c.is_ascii_alphabetic() || c == b'_') {
            i += 1;
            continue;
        }
        let start = i;
        while i < b.len() && (b[i].is_ascii_alphanumeric() || matches!(b[i], b'_' | b'?' | b'!')) {
            i += 1;
        }
        let prev = start.checked_sub(1).map(|p| b[p]);
        if matches!(prev, Some(b'.' | b':' | b'@' | b'&')) {
            continue;
        }
        let keyword_key = b.get(i) == Some(&b':') && b.get(i + 1) != Some(&b':');
        match word_at(start, i) {
            "defmodule" => {
                let (alias, _) = elixir_alias(b, i);
                pending = (!alias.is_empty()).then(|| alias.to_string());
            }
            "do" if keyword_key => pending = None,
            "do" => {
                depth += 1;
                if let Some(m) = pending.take() {
                    open.push((m, depth));
                }
            }
            "fn" if !keyword_key => depth += 1,
            "end" if !keyword_key => {
                depth = depth.saturating_sub(1);
                while open.last().is_some_and(|(_, d)| *d > depth) {
                    open.pop();
                }
            }
            "use" => {
                if let Some((module, body)) = open.last()
                    && *body == depth
                    && let Some(rule) = elixir_use_rule(b, i)
                {
                    out.push((module.clone(), rule));
                }
            }
            _ => {}
        }
    }
    out
}

/// The alias after `from` (`MyApp.Cache`), skipping blanks; and where it ends.
fn elixir_alias(b: &[u8], from: usize) -> (&str, usize) {
    let mut s = from;
    while s < b.len() && matches!(b[s], b' ' | b'\t') {
        s += 1;
    }
    let mut e = s;
    while e < b.len() && (b[e].is_ascii_alphanumeric() || matches!(b[e], b'_' | b'.')) {
        e += 1;
    }
    let alias = std::str::from_utf8(&b[s..e]).unwrap_or("");
    if alias.starts_with(|c: char| c.is_ascii_uppercase() || c == '_') {
        (alias.trim_end_matches('.'), e)
    } else {
        ("", from)
    }
}

/// The rule a `use <Alias>[, :atom]` names, reading from just past `use`.
fn elixir_use_rule(b: &[u8], from: usize) -> Option<&'static str> {
    let (alias, end) = elixir_alias(b, from);
    let mut k = end;
    while k < b.len() && matches!(b[k], b' ' | b'\t') {
        k += 1;
    }
    let mut atom = "";
    if b.get(k) == Some(&b',') {
        k += 1;
        while k < b.len() && matches!(b[k], b' ' | b'\t') {
            k += 1;
        }
        if b.get(k) == Some(&b':') {
            let s = k + 1;
            let mut e = s;
            while e < b.len() && (b[e].is_ascii_alphanumeric() || b[e] == b'_') {
                e += 1;
            }
            atom = std::str::from_utf8(&b[s..e]).unwrap_or("");
        }
    }
    match alias {
        "GenServer" | "Agent" | "Supervisor" | "DynamicSupervisor" => Some("otp_process"),
        "Phoenix.Controller" | "Phoenix.LiveView" | "Phoenix.Channel" => Some("phoenix"),
        "Oban.Worker" => Some("oban_worker"),
        a if a.ends_with("Web") && matches!(atom, "controller" | "live_view" | "channel") => {
            Some("phoenix")
        }
        _ => None,
    }
}

/// `source` with every string, charlist, heredoc, sigil, char literal and
/// comment byte replaced by a space (newlines kept), so a keyword scan over
/// the result sees code only. Offsets are unchanged.
fn elixir_code_only(source: &str) -> Vec<u8> {
    let src = source.as_bytes();
    let mut out = src.to_vec();
    let blank = |out: &mut Vec<u8>, from: usize, to: usize| {
        for byte in &mut out[from..to.min(src.len())] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
    };
    let mut i = 0;
    while i < src.len() {
        let c = src[i];
        let prev_ident = i > 0 && (src[i - 1].is_ascii_alphanumeric() || src[i - 1] == b'_');
        let end = match c {
            b'#' => src[i..]
                .iter()
                .position(|&x| x == b'\n')
                .map_or(src.len(), |p| i + p),
            b'"' | b'\'' => elixir_quoted_end(src, i),
            b'?' if !prev_ident && i + 1 < src.len() => {
                // `?a`, `?\n`, `?#`: a char literal is one (escaped) char.
                let mut e = i + 1;
                if src[e] == b'\\' {
                    e += 1;
                }
                e + 1
                    + (e..src.len())
                        .skip(1)
                        .take_while(|&k| src[k] & 0xC0 == 0x80)
                        .count()
            }
            b'~' if i + 1 < src.len() && src[i + 1].is_ascii_alphabetic() => {
                let mut d = i + 1;
                while d < src.len() && src[d].is_ascii_alphabetic() {
                    d += 1;
                }
                match src.get(d) {
                    Some(b'"' | b'\'') => elixir_quoted_end(src, d),
                    Some(&open) if b"/|([{<".contains(&open) => {
                        let close = match open {
                            b'(' => b')',
                            b'[' => b']',
                            b'{' => b'}',
                            b'<' => b'>',
                            other => other,
                        };
                        let mut e = d + 1;
                        while e < src.len() && src[e] != close {
                            e += if src[e] == b'\\' { 2 } else { 1 };
                        }
                        e + 1
                    }
                    _ => i + 1,
                }
            }
            _ => {
                i += 1;
                continue;
            }
        };
        blank(&mut out, i, end);
        i = end.max(i + 1);
    }
    out
}

/// End (exclusive) of the string / charlist / heredoc whose quote is at
/// `open`. An interpolation `#{...}` is skipped whole, braces counted.
fn elixir_quoted_end(src: &[u8], open: usize) -> usize {
    let q = src[open];
    let heredoc = src.get(open + 1) == Some(&q) && src.get(open + 2) == Some(&q);
    let mut i = open + if heredoc { 3 } else { 1 };
    while i < src.len() {
        match src[i] {
            b'\\' => i += 2,
            b'#' if src.get(i + 1) == Some(&b'{') => {
                let mut depth = 0usize;
                while i < src.len() {
                    match src[i] {
                        b'{' => depth += 1,
                        b'}' => {
                            depth = depth.saturating_sub(1);
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                i += 1;
            }
            c if c == q => {
                if !heredoc {
                    return i + 1;
                }
                if src.get(i + 1) == Some(&q) && src.get(i + 2) == Some(&q) {
                    return i + 3;
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    src.len()
}

// ----------------------------------------------------------------------------
// LA.21a — class declarations + their attributes, for C# / PHP / Scala / Dart.
// ----------------------------------------------------------------------------

/// How a language writes the attributes / annotations on a declaration.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DecoStyle {
    /// C# `[ApiController]`, `[Route("api/[controller]"), Authorize]`.
    Bracket,
    /// PHP 8 `#[AsController]` (a bare `#` starts a comment).
    HashBracket,
    /// Scala / Dart `@Singleton`, `@Injectable(as: Repo)`.
    At,
}

/// One `class` declaration and what decorates it.
struct ClassDecl {
    name: String,
    /// Attribute / annotation names on the declaration (above it, or inline
    /// before it): last path segment, a C# `Attribute` suffix dropped.
    decos: Vec<String>,
    /// The declaration after its name, up to the body (see [`decl_header`]).
    header: String,
    /// Byte offset of the declaration's first token.
    line_start: usize,
}

/// Every `[modifiers] class Name` statement in `source`, with the attributes
/// written before it. `angle`: the language's generics use `<` `>`. Attributes accumulate across blank and comment lines
/// and are dropped by any other statement, so a method's `[HttpGet]` never
/// reaches the next class.
fn class_decls(source: &str, style: DecoStyle, modifiers: &[&str], angle: bool) -> Vec<ClassDecl> {
    let b = source.as_bytes();
    let mut out = Vec::new();
    let mut pending: Vec<String> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let rest = &source[i..];
        let line_end = rest.find('\n').map_or(b.len(), |p| i + p);
        if rest.starts_with("//")
            || (style == DecoStyle::HashBracket && rest.starts_with('#') && !rest.starts_with("#["))
        {
            i = line_end;
            continue;
        }
        if rest.starts_with("/*") {
            i = rest.find("*/").map_or(b.len(), |p| i + p + 2);
            continue;
        }
        if let Some((names, end)) = parse_deco(source, i, style) {
            pending.extend(names);
            i = end;
            continue;
        }
        let decos = std::mem::take(&mut pending);
        if let Some((name, name_end)) = class_decl_at(&source[i..line_end], modifiers) {
            out.push(ClassDecl {
                name: name.to_string(),
                decos,
                header: decl_header(source, i + name_end, angle),
                line_start: i,
            });
        }
        i = line_end;
    }
    out
}

/// The attribute / annotation group at `at`, if one starts there: its names
/// and the offset just past it.
fn parse_deco(source: &str, at: usize, style: DecoStyle) -> Option<(Vec<String>, usize)> {
    let b = source.as_bytes();
    match style {
        DecoStyle::Bracket | DecoStyle::HashBracket => {
            let open = if style == DecoStyle::Bracket {
                at
            } else {
                at + 1
            };
            if style == DecoStyle::HashBracket && b.get(at) != Some(&b'#') {
                return None;
            }
            if b.get(open) != Some(&b'[') {
                return None;
            }
            let close = matching_close(b, open)?;
            let names = split_depth0(&source[open + 1..close])
                .into_iter()
                .filter_map(|part| {
                    let part = part.trim();
                    // `[assembly: X]` / `[return: X]` target prefixes.
                    let part = match part.split_once(':') {
                        Some((t, r))
                            if !r.starts_with(':') && t.chars().all(char::is_alphabetic) =>
                        {
                            r.trim_start()
                        }
                        _ => part,
                    };
                    let end = part
                        .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.' || c == '\\'))
                        .unwrap_or(part.len());
                    let name = last_segment(&part[..end]);
                    let name = name
                        .strip_suffix("Attribute")
                        .filter(|n| !n.is_empty())
                        .unwrap_or(name);
                    (!name.is_empty()).then(|| name.to_string())
                })
                .collect();
            Some((names, close + 1))
        }
        DecoStyle::At => {
            if b.get(at) != Some(&b'@') || !b.get(at + 1).is_some_and(u8::is_ascii_alphabetic) {
                return None;
            }
            let mut e = at + 1;
            while e < b.len() && (b[e].is_ascii_alphanumeric() || matches!(b[e], b'_' | b'.')) {
                e += 1;
            }
            let name = last_segment(&source[at + 1..e]).to_string();
            let end = if b.get(e) == Some(&b'(') {
                matching_close(b, e)? + 1
            } else {
                e
            };
            Some((vec![name], end))
        }
    }
}

/// `text` (one statement's first line) as `[modifiers] class Name ...`: the
/// name and the offset just past it.
fn class_decl_at<'a>(text: &'a str, modifiers: &[&str]) -> Option<(&'a str, usize)> {
    let mut rest = text;
    'strip: loop {
        for m in modifiers {
            if let Some(after) = rest.strip_prefix(m)
                && after.starts_with(|c: char| c.is_whitespace())
            {
                rest = after.trim_start();
                continue 'strip;
            }
        }
        break;
    }
    let after = rest.strip_prefix("class")?;
    if !after.starts_with(|c: char| c.is_whitespace()) {
        return None;
    }
    let name_at = after.trim_start();
    let end = name_at
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(name_at.len());
    if end == 0 || !name_at.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
        return None;
    }
    let offset = text.len() - name_at.len() + end;
    Some((&name_at[..end], offset))
}

/// The declaration text from `from` (just past the name) to its body: the
/// first depth-0 `{` or `;`, or a depth-0 line break the next line does not
/// continue (`: Base`, `extends`, `with`, `implements`, `where`, or a line
/// ending in `:` / `,` / a heritage keyword). `angle` counts `<` `>` as
/// brackets (generics in C# / PHP / Dart; Scala's are `[` `]`).
fn decl_header(source: &str, from: usize, angle: bool) -> String {
    const CONT: &[&str] = &["extends", "implements", "with", "where"];
    let b = source.as_bytes();
    let mut depth = 0usize;
    let mut k = from;
    while k < b.len() && k - from < 4096 {
        match b[k] {
            b'"' | b'\'' => {
                k = quoted_end(b, k);
                continue;
            }
            b'(' | b'[' => depth += 1,
            b'<' if angle => depth += 1,
            b')' | b']' => depth = depth.saturating_sub(1),
            b'>' if angle => depth = depth.saturating_sub(1),
            b'{' | b';' if depth == 0 => break,
            b'\n' if depth == 0 => {
                let so_far = source[from..k].trim_end();
                let next = source[k..].trim_start();
                let ends_cont = so_far.ends_with(':')
                    || so_far.ends_with(',')
                    || CONT.iter().any(|w| {
                        so_far.ends_with(w)
                            && !so_far[..so_far.len() - w.len()]
                                .ends_with(|c: char| c.is_alphanumeric() || c == '_')
                    });
                let starts_cont = next.starts_with(':')
                    || next.starts_with(',')
                    || next.starts_with('(')
                    || CONT.iter().any(|w| {
                        next.strip_prefix(w)
                            .is_some_and(|r| r.starts_with(|c: char| c.is_whitespace()))
                    });
                if !(ends_cont || starts_cont) {
                    break;
                }
            }
            _ => {}
        }
        k += 1;
    }
    let mut k = k.min(b.len());
    while !source.is_char_boundary(k) {
        k -= 1;
    }
    source[from..k].to_string()
}

/// Depth-0 tokens of a declaration header: (qualified) identifiers, `@Name`
/// annotations, and the punctuation `:` and `,`. Anything inside brackets or
/// quotes is skipped.
fn header_tokens(header: &str, angle: bool) -> Vec<String> {
    let b = header.as_bytes();
    let mut toks = Vec::new();
    let mut depth = 0usize;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match c {
            b'"' | b'\'' => {
                i = quoted_end(b, i);
                continue;
            }
            b'(' | b'[' | b'{' => depth += 1,
            b'<' if angle => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b'>' if angle => depth = depth.saturating_sub(1),
            b':' | b',' if depth == 0 => toks.push((c as char).to_string()),
            _ if depth == 0
                && (c.is_ascii_alphabetic() || matches!(c, b'_' | b'$' | b'\\' | b'@')) =>
            {
                let s = i;
                i += 1;
                while i < b.len()
                    && (b[i].is_ascii_alphanumeric() || matches!(b[i], b'_' | b'$' | b'\\' | b'.'))
                {
                    i += 1;
                }
                toks.push(header[s..i].to_string());
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    toks
}

/// The last-segment names that follow any of `keywords` in `toks`, including
/// a comma-separated list after one (`implements A, B`).
fn words_after(toks: &[String], keywords: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        if keywords.contains(&toks[i].as_str()) {
            i += 1;
            while i < toks.len() {
                out.push(last_segment(&toks[i]).to_string());
                if toks.get(i + 1).is_some_and(|t| t == ",") {
                    i += 2;
                } else {
                    break;
                }
            }
        }
        i += 1;
    }
    out
}

/// `A.B.C` / `A\B\C` / `\A\B` → the last non-empty segment.
fn last_segment(s: &str) -> &str {
    s.rsplit(['.', '\\'])
        .find(|seg| !seg.is_empty())
        .unwrap_or("")
}

/// The offset of the bracket closing the one at `open`, skipping quoted text.
fn matching_close(b: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = open;
    while i < b.len() {
        match b[i] {
            b'"' | b'\'' => {
                i = quoted_end(b, i);
                continue;
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// End (exclusive) of the quoted literal opening at `open`; backslash escapes.
fn quoted_end(b: &[u8], open: usize) -> usize {
    let q = b[open];
    let mut i = open + 1;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            c if c == q => return i + 1,
            b'\n' => return i,
            _ => i += 1,
        }
    }
    b.len()
}

/// `s` split at its depth-0 commas.
fn split_depth0(s: &str) -> Vec<&str> {
    let b = s.as_bytes();
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'"' | b'\'' => {
                i = quoted_end(b, i);
                continue;
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    parts.push(&s[start..]);
    parts
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

    // ------------------------------------------------------------------
    // LA.21a — C# / PHP / Ruby / Scala / Elixir / Dart.
    // ------------------------------------------------------------------

    /// `(name, rule)` pairs, sorted, for order-free comparison.
    fn pairs(hits: Vec<RuleHit>) -> Vec<(String, &'static str)> {
        let mut v = hits;
        v.sort();
        v
    }

    fn p(name: &str, rule: &'static str) -> (String, &'static str) {
        (name.to_string(), rule)
    }

    #[test]
    fn csharp_every_rule_fires() {
        let src = r#"
using Microsoft.AspNetCore.Mvc;

namespace Shop.Api
{
    [ApiController]
    [Route("api/[controller]")]
    public class UsersController : ApiBase
    {
        [HttpGet("{id}")]
        public string Get(int id) { return "u"; }
    }

    public class OrdersController
        : ControllerBase
    {
    }

    public sealed class HomeController : Microsoft.AspNetCore.Mvc.Controller { }

    public class AdminController : BaseApiController { }

    public class BillingService : IBillingService, IDisposable { }

    public class Worker(ILogger<Worker> log) : BackgroundService { }

    internal class Poller : IHostedService { }

    public class ChatHub : Hub<IChatClient> { }

    public class NotifyHub : Hub { }
}
"#;
        assert_eq!(
            pairs(detect_csharp_services(src)),
            vec![
                p("AdminController", "aspnet_controller"),
                p("BillingService", "iface_service"),
                p("ChatHub", "signalr_hub"),
                p("HomeController", "aspnet_controller"),
                p("NotifyHub", "signalr_hub"),
                p("OrdersController", "aspnet_controller"),
                p("Poller", "hosted_service"),
                p("UsersController", "aspnet_controller"),
                p("Worker", "hosted_service"),
            ]
        );
    }

    #[test]
    fn csharp_plain_base_generic_constraint_and_method_attribute_do_not_classify() {
        let src = r#"
public class Foo : Bar
{
    [HttpGet]
    public void Run() {}
}

[Serializable]
public class Repo<T> where T : Controller { }

public class Point : IComparable<Point> { }

[HttpPost]
public void Stray() {}
public class Plain { }
"#;
        assert_eq!(detect_csharp_services(src), Vec::<RuleHit>::new());
    }

    #[test]
    fn php_every_rule_fires() {
        let src = r#"<?php
namespace App\Http\Controllers;

use App\Http\Controllers\Controller;

class UserController extends Controller
{
    public function show($id) { return $id; }
}

final class ReportController extends \Symfony\Bundle\FrameworkBundle\Controller\AbstractController {}

#[AsController]
class Health
{
}

class AdminController extends BaseController {}

namespace App\Services;

class InvoiceService
{
    public function send() {}
}
"#;
        assert_eq!(
            pairs(detect_php_services(src)),
            vec![
                p("AdminController", "controller"),
                p("Health", "controller"),
                p("InvoiceService", "services_namespace"),
                p("ReportController", "controller"),
                p("UserController", "controller"),
            ]
        );
    }

    #[test]
    fn php_service_outside_a_services_namespace_is_not_classified() {
        let src = r#"<?php
namespace App\Billing;

# an old-style comment
class InvoiceService
{
    public function send() {}
}

class Invoice extends Model {}
"#;
        assert_eq!(detect_php_services(src), Vec::<RuleHit>::new());
    }

    #[test]
    fn ruby_every_rule_fires() {
        let src = r#"
class UsersController < ApplicationController
  def show
  end
end

class Api::V1::ItemsController < ActionController::API
end

class LegacyController < ::ActionController::Base; end

class Api::V1::OrdersController < Api::V1::BaseController
end

class SignupService
  def call
  end
end

module Billing
  class ChargeService
    def self.call(order)
    end
  end
end

class ReindexService
  def perform(id)
  end
end
"#;
        assert_eq!(
            pairs(detect_ruby_services(src)),
            vec![
                p("ChargeService", "service_object"),
                p("ItemsController", "rails_controller"),
                p("LegacyController", "rails_controller"),
                p("OrdersController", "rails_controller"),
                p("ReindexService", "service_object"),
                p("SignupService", "service_object"),
                p("UsersController", "rails_controller"),
            ]
        );
    }

    #[test]
    fn ruby_service_without_an_entry_point_is_not_classified() {
        let src = r#"
class FooService
  def caller
  end

  def perform_later
  end
end

class Other
  def call
  end
end

class << self
end
"#;
        assert_eq!(detect_ruby_services(src), Vec::<RuleHit>::new());
    }

    #[test]
    fn scala_every_rule_fires() {
        let src = r#"
package controllers

import javax.inject._
import play.api.mvc._

@Singleton
class UserController @Inject()(cc: ControllerComponents) extends AbstractController(cc) {
  def show(id: Long) = Action { Ok("u") }
}

class HomeController @Inject()(
    val controllerComponents: ControllerComponents
) extends BaseController {
}

class PingController extends InjectedController

class HealthController @Inject()(cc: ControllerComponents) extends Foo(cc) with BaseController

@Singleton
class UserService {
  def find(id: Long): Int = 1
}

class BillingService @Inject()(repo: Repo) {
}
"#;
        assert_eq!(
            pairs(detect_scala_services(src)),
            vec![
                p("BillingService", "di_service"),
                p("HealthController", "play_controller"),
                p("HomeController", "play_controller"),
                p("PingController", "play_controller"),
                p("UserController", "play_controller"),
                p("UserService", "di_service"),
            ]
        );
    }

    #[test]
    fn scala_plain_service_is_not_classified() {
        let src = r#"
class PricingService {
  def quote(): Int = 1
}

@Singleton
class Cache {
}
"#;
        assert_eq!(detect_scala_services(src), Vec::<RuleHit>::new());
    }

    #[test]
    fn elixir_every_rule_fires() {
        let src = r#"
defmodule MyApp.Cache do
  use GenServer, restart: :transient
  def init(state), do: {:ok, state}
end

defmodule MyApp.Tree do
  use Supervisor
end

defmodule MyAppWeb.UserController do
  use MyAppWeb, :controller
  def show(conn, _params), do: conn
end

defmodule MyAppWeb.PageLive do
  use MyAppWeb, :live_view
end

defmodule MyAppWeb.Legacy do
  use Phoenix.Controller
end

defmodule MyApp.Mailer.Job do
  use Oban.Worker, queue: :mail
end
"#;
        assert_eq!(
            pairs(detect_elixir_services(src)),
            vec![
                p("MyApp.Cache", "otp_process"),
                p("MyApp.Mailer.Job", "oban_worker"),
                p("MyApp.Tree", "otp_process"),
                p("MyAppWeb.Legacy", "phoenix"),
                p("MyAppWeb.PageLive", "phoenix"),
                p("MyAppWeb.UserController", "phoenix"),
            ]
        );
    }

    #[test]
    fn elixir_schema_module_is_not_classified() {
        let src = r#"
defmodule MyApp.User do
  use Ecto.Schema
  schema "users" do
    field :name, :string
  end
end
"#;
        assert_eq!(detect_elixir_services(src), Vec::<RuleHit>::new());
    }

    /// The second module's `use GenServer` must not classify the first; a
    /// `use` inside `quote` (Phoenix's `MyAppWeb.controller/0`) classifies
    /// nothing; strings, charlists, sigils, char literals, comments, `do:` and
    /// `:end` atoms never move the depth.
    #[test]
    fn elixir_use_belongs_to_its_own_module_body() {
        let src = r#"
defmodule MyApp.First do
  @moduledoc """
  Does not end here. do
  """
  def a, do: :end
  def b(x) do
    s = "end #{x} do"
    c = 'end'
    r = ~r/do|end/
    q = ?e
    # end end end
    Enum.map(x, fn y -> y end)
  end
end

defmodule MyApp.Second do
  use GenServer
end

defmodule MyAppWeb do
  def controller do
    quote do
      use Phoenix.Controller
    end
  end
end

defmodule MyApp.Outer do
  defmodule Inner do
    use Agent
  end
  def x, do: 1
end
"#;
        assert_eq!(
            pairs(detect_elixir_services(src)),
            vec![p("Inner", "otp_process"), p("MyApp.Second", "otp_process")]
        );
    }

    #[test]
    fn dart_every_rule_fires() {
        let src = r#"
import 'package:injectable/injectable.dart';

@lazySingleton
class AuthService {
  Future<void> login() async {}
}

@LazySingleton(as: AuthRepo)
class AuthRepoImpl implements AuthRepo {}

@injectable
class Api {}

@Injectable()
class Clock {}

@singleton
class Db {}

@Singleton()
class Prefs {}

class HomeController extends GetxController {}

class SessionService extends GetxService {}
"#;
        assert_eq!(
            pairs(detect_dart_services(src)),
            vec![
                p("Api", "injectable"),
                p("AuthRepoImpl", "injectable"),
                p("AuthService", "injectable"),
                p("Clock", "injectable"),
                p("Db", "injectable"),
                p("HomeController", "getx"),
                p("Prefs", "injectable"),
                p("SessionService", "getx"),
            ]
        );
    }

    #[test]
    fn dart_override_is_not_a_di_annotation() {
        let src = r#"
class CartService extends ChangeNotifier {
  @override
  void dispose() {}
}

@override
void stray() {}
class Plain {}
"#;
        assert_eq!(detect_dart_services(src), Vec::<RuleHit>::new());
    }

    /// Nav with one owner of `kind` at `qname` plus FUNCTION / METHOD children.
    fn nav_with_owner(
        kind: repo_graph_core::NodeKindId,
        qname: &str,
        children: &[(&str, repo_graph_core::NodeKindId)],
    ) -> (CodeNav, NodeId, NodeId, Vec<NodeId>) {
        let mut nav = CodeNav::default();
        let module_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "worker");
        nav.record(module_id, "worker", "worker", node_kind::MODULE, None);
        let owner = NodeId::from_parts(GRAPH_TYPE, repo(), kind, qname);
        let short = qname.rsplit("::").next().unwrap_or(qname);
        nav.record(owner, short, qname, kind, Some(module_id));
        let kids = children
            .iter()
            .map(|(n, k)| {
                let q = format!("{qname}::{n}");
                let id = NodeId::from_parts(GRAPH_TYPE, repo(), *k, &q);
                nav.record(id, n, &q, *k, Some(owner));
                id
            })
            .collect();
        (nav, module_id, owner, kids)
    }

    #[test]
    fn elixir_package_owner_gets_a_service_overlay_with_contains() {
        let src = "defmodule MyApp.Cache do\n  use GenServer\n  def init(s), do: {:ok, s}\nend\n";
        let (nav, module_id, _, kids) = nav_with_owner(
            node_kind::PACKAGE,
            "worker::MyApp.Cache",
            &[
                ("init", node_kind::FUNCTION),
                ("start_link", node_kind::FUNCTION),
            ],
        );
        let out = extract_service_nodes(src, "elixir", &nav, module_id, repo());
        assert_eq!(out.nodes.len(), 1);
        assert_eq!(
            out.nodes[0].id,
            NodeId::from_parts(
                GRAPH_TYPE,
                repo(),
                node_kind::SERVICE,
                "worker::MyApp.Cache"
            )
        );
        assert_eq!(out.nav.kind_by_id[&out.nodes[0].id], node_kind::SERVICE);
        let to: HashSet<NodeId> = out.edges.iter().map(|e| e.to).collect();
        assert_eq!(to, kids.into_iter().collect::<HashSet<NodeId>>());
        assert!(
            out.edges
                .iter()
                .all(|e| e.category == edge_category::CONTAINS)
        );
    }

    /// PACKAGE is an owner for Elixir only: a Ruby `module` never is.
    #[test]
    fn package_owner_is_elixir_only() {
        let src = "class SignupService\n  def call\n  end\nend\n";
        let (nav, module_id, _, _) = nav_with_owner(node_kind::PACKAGE, "m::SignupService", &[]);
        let out = extract_service_nodes(src, "ruby", &nav, module_id, repo());
        assert!(out.nodes.is_empty(), "got {:?}", out.nodes);
    }

    /// A detector name with no declaration in the nav emits nothing and is not
    /// counted; the marker tallies rules in first-hit order.
    #[test]
    fn csharp_emits_only_for_nav_declarations_and_tallies_rules() {
        let src = r#"
[ApiController]
public class UsersController : ControllerBase { public string Get() => "u"; }
public class BillingService : IBillingService { }
public class Worker : BackgroundService { }
public class Ghost : BackgroundService { }
"#;
        let mut nav = CodeNav::default();
        let module_id =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "UsersController");
        nav.record(
            module_id,
            "UsersController",
            "UsersController",
            node_kind::MODULE,
            None,
        );
        for c in ["UsersController", "BillingService", "Worker"] {
            let q = format!("Shop::Api::{c}");
            let id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, &q);
            nav.record(id, c, &q, node_kind::CLASS, Some(module_id));
        }
        let out = extract_service_nodes(src, "csharp", &nav, module_id, repo());
        assert_eq!(out.nodes.len(), 3, "Ghost has no declaration");

        let hits = detect_csharp_services(src);
        let owners: HashMap<String, usize> = ["UsersController", "BillingService", "Worker"]
            .iter()
            .map(|n| (n.to_string(), 1))
            .collect();
        let tally = rule_tally(&hits, &owners);
        assert_eq!(
            services_marker("csharp", 3, &tally, "UsersController"),
            "[services] csharp classified=3 \
             rules=aspnet_controller:1,iface_service:1,hosted_service:1 module=UsersController"
        );
    }
}
