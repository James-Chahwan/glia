use repo_graph_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};
use std::collections::HashSet;
use std::sync::OnceLock;
use tree_sitter::{Node as TsNode, Parser};

pub use repo_graph_code_domain::{
    CallQualifier, CallSite, CodeNav, FileParse, GRAPH_TYPE, ImportStmt, ImportTarget, ParseError,
    UnresolvedRef, cell_type, edge_category, node_kind,
};
use repo_graph_code_domain::endpoint::{
    self, ClientEndpoint, HitExtras, push_client_endpoint_with,
};

pub fn parse_file(
    source: &str,
    file_rel_path: &str,
    module_qname: &str,
    repo: RepoId,
) -> Result<FileParse, ParseError> {
    let mut parser = Parser::new();
    let lang: tree_sitter::Language = tree_sitter_java::LANGUAGE.into();
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

    // LB.2: top-level types are members of the PACKAGE (the directory), so
    // their qnames hang off the directory scope, not the file module. The
    // MODULE node above keeps `module_qname`: imports, TESTS pairing and the
    // local-module index all read file-module qnames.
    let scope = type_scope(module_qname);
    let mut top_level_types = 0usize;
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        match child.kind() {
            "import_declaration" => collect_import(child, src, module_qname, &mut acc),
            "class_declaration" | "interface_declaration" | "enum_declaration"
            | "record_declaration" => {
                top_level_types += usize::from(visit_type_decl(
                    child,
                    src,
                    file_rel_path,
                    scope,
                    module_id,
                    module_id,
                    repo,
                    &mut acc,
                ));
            }
            _ => {}
        }
    }
    if top_level_types > 0 && qname_debug() {
        eprintln!(
            "[qname] java: {top_level_types} top-level types scoped to {scope} (file stem dropped) file={file_rel_path}"
        );
    }

    scan_ktor_routes(source, repo, &mut acc);
    scan_webflux_routes(source, repo, &mut acc);
    scan_javalin_routes(source, repo, &mut acc);

    if acc.http_clients.any() {
        let c = acc.http_clients;
        eprintln!(
            "[java-http] clients jdk={} okhttp={} apache={} path={file_rel_path}",
            c.jdk, c.okhttp, c.apache
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

/// LB.2: a Java top-level type belongs to its package (its directory), not its
/// file, so drop the file-stem segment the engine's `path_to_qname` puts last:
/// `src::main::java::com::example::Foo` -> `src::main::java::com::example`, and
/// a file at the repo root (`Foo`) -> `""`. The directory, not the declared
/// `package`, is the scope on purpose: two services of one monorepo that both
/// declare `com.example.Application` must keep distinct NodeIds.
///
/// The public class `Foo` of `Foo.java` therefore shares its qname with the
/// file MODULE (different kind, different NodeId); `MergedGraph::pick_primary`
/// ranks the declaration over the container, so qname lookups land on the type.
fn type_scope(module_qname: &str) -> &str {
    module_qname.rsplit_once("::").map_or("", |(dir, _stem)| dir)
}

/// `scope::name`, or the bare `name` for the empty (repo-root) scope.
fn scoped(scope: &str, name: &str) -> String {
    if scope.is_empty() {
        name.to_string()
    } else {
        format!("{scope}::{name}")
    }
}

/// `GLIA_QNAME_DEBUG=1` turns on the per-file `[qname] java:` marker, read
/// once. Off by default: it would print for every Java file of a build.
///   `GLIA_QNAME_DEBUG=1 glia analyze <repo> 2>&1 | grep '\[qname\] java:'`
fn qname_debug() -> bool {
    static DEBUG: OnceLock<bool> = OnceLock::new();
    *DEBUG.get_or_init(|| {
        std::env::var("GLIA_QNAME_DEBUG").is_ok_and(|v| !v.is_empty() && v != "0")
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
    /// Dedups client ENDPOINT nodes across a file (Pattern A).
    endpoint_seen: HashSet<NodeId>,
    /// LA.22a: the imperative HTTP client libraries this file imports.
    http_libs: JavaHttpLibs,
    /// LA.22a: imperative-client call sites that became an ENDPOINT, per
    /// library, for the `[java-http] clients` marker.
    http_clients: JavaHttpClientCounts,
}

/// LA.22a: the imperative HTTP client libraries whose request shapes the
/// endpoint arms recognise. Each arm emits only for a library in scope.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum JavaHttpClient {
    /// `java.net.http`: `HttpRequest.newBuilder(…)…build()`.
    Jdk,
    /// `okhttp3`: `new Request.Builder().url(…)…build()`.
    OkHttp,
    /// Apache HttpClient 4 (`org.apache.http`) / 5 (`org.apache.hc`):
    /// `new HttpGet(url)` …, and 5's `SimpleRequestBuilder.get(url)` ….
    Apache,
}

/// LA.22a: which [`JavaHttpClient`] libraries the file imports. Set by
/// `collect_import`; `parse_file` walks every import before any type body, so
/// the flags are final before a method body is scanned.
#[derive(Default, Clone, Copy)]
struct JavaHttpLibs {
    jdk: bool,
    okhttp: bool,
    apache: bool,
}

impl JavaHttpLibs {
    /// Record one import path (`java.net.http.HttpRequest`, `okhttp3.*`, …).
    fn note_import(&mut self, path: &str) {
        self.jdk |= path.starts_with("java.net.http.");
        self.okhttp |= path.starts_with("okhttp3.");
        self.apache |= is_apache_http_package(path);
    }

    fn has(&self, client: JavaHttpClient) -> bool {
        match client {
            JavaHttpClient::Jdk => self.jdk,
            JavaHttpClient::OkHttp => self.okhttp,
            JavaHttpClient::Apache => self.apache,
        }
    }
}

/// LA.22a: per-file ENDPOINT emissions by [`JavaHttpClient`].
#[derive(Default, Clone, Copy)]
struct JavaHttpClientCounts {
    jdk: usize,
    okhttp: usize,
    apache: usize,
}

impl JavaHttpClientCounts {
    fn bump(&mut self, client: JavaHttpClient) {
        match client {
            JavaHttpClient::Jdk => self.jdk += 1,
            JavaHttpClient::OkHttp => self.okhttp += 1,
            JavaHttpClient::Apache => self.apache += 1,
        }
    }

    fn any(&self) -> bool {
        self.jdk + self.okhttp + self.apache > 0
    }
}

/// `org.apache.http.…` (HttpClient 4) or `org.apache.hc.…` (HttpClient 5).
fn is_apache_http_package(path: &str) -> bool {
    path.starts_with("org.apache.http.") || path.starts_with("org.apache.hc.")
}

/// Emit one type declaration and everything under it. `scope` is the qname the
/// type hangs off: the package scope ([`type_scope`]) for a top-level type,
/// the outer type's qname for a nested one. Returns whether a type node was
/// emitted (the `[qname] java:` marker counts them).
fn visit_type_decl(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    scope: &str,
    parent_id: NodeId,
    module_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) -> bool {
    let Some(name_node) = node.child_by_field_name("name") else {
        return false;
    };
    let name = text_of(name_node, src);
    let kind = match node.kind() {
        "class_declaration" | "record_declaration" => node_kind::CLASS,
        "interface_declaration" => node_kind::INTERFACE,
        "enum_declaration" => node_kind::ENUM,
        _ => return false,
    };
    let qname = scoped(scope, name);
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

    // G12.5: class heritage. `superclass` (extends) → INHERITS_FROM;
    // `interfaces` (super_interfaces → type_list) → IMPLEMENTS per interface.
    // Emitted as UnresolvedRefs (Bare(TypeName)) so the graph resolver binds them
    // to the uniquely-named class/interface across the repo (a direct name-derived
    // NodeId would target a phantom that mismatches the real qname-based id).
    if let Some(superclass) = node.child_by_field_name("superclass") {
        let mut sc_cursor = superclass.walk();
        for sc in superclass.named_children(&mut sc_cursor) {
            emit_heritage_ref(text_of(sc, src), edge_category::INHERITS_FROM, id, module_id, acc);
        }
    }
    if let Some(interfaces) = node.child_by_field_name("interfaces") {
        // `interfaces` is a `super_interfaces` wrapping a `type_list`.
        let mut if_cursor = interfaces.walk();
        for type_list in interfaces.named_children(&mut if_cursor) {
            let mut tl_cursor = type_list.walk();
            for iface in type_list.named_children(&mut tl_cursor) {
                emit_heritage_ref(text_of(iface, src), edge_category::IMPLEMENTS, id, module_id, acc);
            }
        }
    }

    // Persistence substrate: a JPA `@Entity` class is also a DATA_ENTITY; a Spring
    // Data repository (`extends JpaRepository<Entity, Id>`) ACCESSES_DATA the
    // parameterised entity. Both link through a name-derived DATA_ENTITY id so the
    // repository (which sees only the bare entity type name, possibly cross-file)
    // and the entity emitter agree on the same target node.
    if is_data_entity(&node, src) {
        emit_data_entity(name, id, repo, acc);
    }
    emit_repository_access(&node, src, id, repo, acc);

    // Walk body for methods + nested types.
    let Some(body) = node.child_by_field_name("body") else {
        return true;
    };
    // Pattern E: a Spring stereotype (@Service/@Component/@RestController/…) marks
    // this class as a DI-managed bean, so its constructor params are injected
    // dependencies. Field injection (@Autowired on a field) is gated per-field
    // below and does not require the class itself to be a stereotype.
    let is_bean = is_spring_bean(&node, src);
    // A4.4: Spring `@RequestMapping` / Micronaut `@Controller` / JAX-RS `@Path`
    // on the class is a PREFIX for every action method below, not a route the
    // methods own. Read it once here and compose it per method.
    let class_prefix = class_route_prefix(node, src);
    let mut composed = 0usize;
    let mut cursor = body.walk();
    for child in body.named_children(&mut cursor) {
        match child.kind() {
            "constructor_declaration" => {
                composed += visit_method(child, src, file_rel, &qname, id, repo, &class_prefix, acc);
                if is_bean {
                    emit_constructor_injects(child, src, id, module_id, acc);
                }
            }
            "method_declaration" => {
                composed += visit_method(child, src, file_rel, &qname, id, repo, &class_prefix, acc);
            }
            "field_declaration" => {
                visit_field_decl(child, src, file_rel, &qname, id, repo, acc);
                emit_field_inject(child, src, id, module_id, acc);
            }
            "class_declaration" | "interface_declaration" | "enum_declaration"
            | "record_declaration" => {
                visit_type_decl(child, src, file_rel, &qname, id, module_id, repo, acc);
            }
            _ => {}
        }
    }

    // The class's OWN annotations (its base route), with no prefix to compose
    // against — the prefix IS this annotation.
    check_route_annotations(node, src, file_rel, id, repo, "", acc);
    if composed > 0 && !class_prefix.is_empty() {
        eprintln!(
            "[java-routes] composed {composed} action routes under '{class_prefix}' in {file_rel}"
        );
    }
    true
}

fn visit_method(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    class_prefix: &str,
    acc: &mut Acc,
) -> usize {
    let Some(name_node) = node.child_by_field_name("name") else {
        return 0;
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
        collect_calls_in(body, src, id, repo, file_rel, acc);
    }

    // Route annotations on the method, composed onto the enclosing class prefix.
    check_route_annotations(node, src, file_rel, id, repo, class_prefix, acc)
}

/// G12.5: record an unresolved heritage reference (extends/implements) from a
/// class to a supertype name. The graph's `resolve_refs` binds the
/// `Bare(TypeName)` qualifier (for INHERITS_FROM / IMPLEMENTS) to the
/// uniquely-named class/interface across the repo and forms the concrete edge —
/// so we emit a REF, not a direct edge to a name-derived (phantom) NodeId.
fn emit_heritage_ref(
    raw: &str,
    category: repo_graph_core::EdgeCategoryId,
    from_id: NodeId,
    from_module: NodeId,
    acc: &mut Acc,
) {
    // Strip generic args (e.g. `Comparable<Foo>` → `Comparable`) and take the
    // trailing simple name (e.g. `pkg.Base` → `Base`).
    let base = raw.split('<').next().unwrap_or(raw).trim();
    let simple = base.rsplit(['.', ':']).next().unwrap_or(base).trim();
    if simple.is_empty() {
        return;
    }
    acc.refs.push(UnresolvedRef {
        from: from_id,
        from_module,
        qualifier: CallQualifier::Bare(simple.to_string()),
        category,
    });
}

/// Class-level annotations that mark a persistent data model → DATA_ENTITY.
/// `@Entity` is JPA; `@Document` is Spring Data Mongo.
const DATA_ENTITY_ANNOTATIONS: &[&str] = &["@Entity", "@Document"];

/// True if the class carries a persistence annotation (→ DATA_ENTITY projection).
fn is_data_entity(node: &TsNode, src: &[u8]) -> bool {
    modifiers_text(node, src)
        .map(|m| DATA_ENTITY_ANNOTATIONS.iter().any(|a| has_annotation(m, a)))
        .unwrap_or(false)
}

/// Stable, name-derived DATA_ENTITY id. Keyed on the entity's simple name only so
/// a repository referencing the bare type name (possibly from another file) and
/// the `@Entity` class that emits it resolve to the same node without needing the
/// entity's fully-qualified module path.
fn data_entity_id(simple_name: &str, repo: RepoId) -> NodeId {
    NodeId::from_parts(GRAPH_TYPE, repo, node_kind::DATA_ENTITY, simple_name)
}

/// Emit the DATA_ENTITY node projected from an `@Entity` class, plus a DEFINES
/// edge class→entity so the model is reachable from its declaring type.
fn emit_data_entity(name: &str, class_id: NodeId, repo: RepoId, acc: &mut Acc) {
    let entity_id = data_entity_id(name, repo);
    acc.nodes.push(Node {
        id: entity_id,
        repo,
        confidence: Confidence::Strong,
        cells: vec![Cell {
            kind: cell_type::CODE,
            payload: CellPayload::Text(name.to_string()),
        }],
    });
    acc.edges.push(Edge {
        from: class_id,
        to: entity_id,
        category: edge_category::DEFINES,
        confidence: Confidence::Strong,
    });
    acc.nav
        .record(entity_id, name, name, node_kind::DATA_ENTITY, Some(class_id));
}

/// Spring Data repository base interfaces whose first type parameter is the
/// managed entity (`interface FooRepo extends JpaRepository<Foo, Long>`).
const REPOSITORY_BASES: &[&str] = &[
    "Repository",
    "CrudRepository",
    "JpaRepository",
    "PagingAndSortingRepository",
    "JpaSpecificationExecutor",
    "ReactiveCrudRepository",
    "ReactiveSortingRepository",
    "R2dbcRepository",
    "MongoRepository",
    "ReactiveMongoRepository",
];

/// Detect `extends <RepositoryBase>< Entity, … >` on a class or interface and emit
/// an ACCESSES_DATA edge from the repository to the entity's DATA_ENTITY node.
/// `resolve_refs` does not fall back for ACCESSES_DATA, so we wire a direct edge
/// to the name-derived DATA_ENTITY id (matched by `emit_data_entity`).
fn emit_repository_access(node: &TsNode, src: &[u8], from_id: NodeId, repo: RepoId, acc: &mut Acc) {
    let mut c = node.walk();
    for child in node.named_children(&mut c) {
        // `superclass` (class extends), `interfaces`/`super_interfaces` (class
        // implements), `extends_interfaces` (interface extends).
        if matches!(
            child.kind(),
            "superclass" | "super_interfaces" | "extends_interfaces"
        ) {
            scan_repository_generics(child, src, from_id, repo, acc);
        }
    }
}

/// Walk a heritage clause for a `generic_type` whose base is a repository and emit
/// the ACCESSES_DATA edge to its first type argument (the entity).
fn scan_repository_generics(
    root: TsNode,
    src: &[u8],
    from_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        if n.kind() == "generic_type"
            && let Some(entity) = repository_entity(n, src)
        {
            acc.edges.push(Edge {
                from: from_id,
                to: data_entity_id(&entity, repo),
                category: edge_category::ACCESSES_DATA,
                confidence: Confidence::Medium,
            });
        }
        let mut cc = n.walk();
        for ch in n.named_children(&mut cc) {
            stack.push(ch);
        }
    }
}

/// If `gen` is `RepositoryBase<Entity, …>`, return the entity's simple name.
fn repository_entity(generic: TsNode, src: &[u8]) -> Option<String> {
    let mut base: Option<&str> = None;
    let mut targs: Option<TsNode> = None;
    let mut c = generic.walk();
    for child in generic.named_children(&mut c) {
        match child.kind() {
            "type_identifier" | "scoped_type_identifier" if base.is_none() => {
                base = Some(text_of(child, src));
            }
            "type_arguments" => targs = Some(child),
            _ => {}
        }
    }
    let base = base?;
    let base_simple = base.rsplit(['.', ':']).next().unwrap_or(base).trim();
    if !REPOSITORY_BASES.contains(&base_simple) {
        return None;
    }
    let targs = targs?;
    let mut tc = targs.walk();
    for arg in targs.named_children(&mut tc) {
        let t = text_of(arg, src);
        let simple = t
            .split('<')
            .next()
            .unwrap_or(t)
            .rsplit(['.', ':'])
            .next()
            .unwrap_or(t)
            .trim();
        if !simple.is_empty() {
            return Some(simple.to_string());
        }
    }
    None
}

/// G19: class-level constants / static fields. Emits a STATE_VAR node for each
/// declarator in a `static final` field, plus a DEFINES edge class→field.
/// Noise gate: skip when undocumented AND the initializer is a primitive literal.
fn visit_field_decl(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let text = text_of(node, src);
    // Only class-level constants: must be both `static` and `final`.
    if !(text.contains("static") && text.contains("final")) {
        return;
    }
    let has_doc = repo_graph_doc::leading_doc(&node, src).is_some();

    let mut cursor = node.walk();
    for declarator in node.children_by_field_name("declarator", &mut cursor) {
        let Some(name_node) = declarator.child_by_field_name("name") else {
            continue;
        };
        // Noise gate: undocumented + literal-primitive initializer → skip.
        if !has_doc {
            if let Some(value) = declarator.child_by_field_name("value") {
                if is_primitive_literal(value.kind()) {
                    continue;
                }
            }
        }
        let name = text_of(name_node, src);
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
}

/// True for Java primitive/atom literal initializer node kinds.
fn is_primitive_literal(kind: &str) -> bool {
    matches!(
        kind,
        "decimal_integer_literal"
            | "hex_integer_literal"
            | "octal_integer_literal"
            | "binary_integer_literal"
            | "decimal_floating_point_literal"
            | "hex_floating_point_literal"
            | "character_literal"
            | "string_literal"
            | "true"
            | "false"
            | "null_literal"
    )
}

/// Pattern E (dependency injection): Spring stereotype annotations that mark a
/// class as a DI-managed bean whose constructor params are injected beans.
const SPRING_STEREOTYPES: &[&str] = &[
    "@Service",
    "@Component",
    "@RestController",
    "@Controller",
    "@Repository",
    "@Configuration",
];

/// Field/constructor-level annotations that request injection of the annotated
/// member (`@Autowired` is Spring; `@Inject`/`@Resource` are JSR-330/JSR-250).
const INJECT_ANNOTATIONS: &[&str] = &["@Autowired", "@Inject", "@Resource"];

/// Text of a declaration's `modifiers` child (holds its annotations), if any.
fn modifiers_text<'a>(node: &TsNode<'a>, src: &'a [u8]) -> Option<&'a str> {
    let mut c = node.walk();
    for child in node.children(&mut c) {
        if child.kind() == "modifiers" {
            return Some(text_of(child, src));
        }
    }
    None
}

/// True if `mods` contains `ann` as a whole annotation token (so `@Component`
/// does not match `@ComponentScan`, and `@Service` matches `@Service(...)`).
fn has_annotation(mods: &str, ann: &str) -> bool {
    let mut from = 0;
    while let Some(rel) = mods[from..].find(ann) {
        let pos = from + rel;
        let after = mods[pos + ann.len()..].chars().next();
        match after {
            None => return true,
            Some(c) if !c.is_ascii_alphanumeric() && c != '_' => return true,
            _ => {}
        }
        from = pos + ann.len();
    }
    false
}

/// True if the class carries a Spring stereotype annotation (→ DI bean).
fn is_spring_bean(node: &TsNode, src: &[u8]) -> bool {
    modifiers_text(node, src)
        .map(|m| SPRING_STEREOTYPES.iter().any(|s| has_annotation(m, s)))
        .unwrap_or(false)
}

/// Types that are never DI beans — skip them as injected dependencies
/// (primitives are excluded structurally by node kind; this is the boxed /
/// value-type denylist for `type_identifier` nodes).
fn is_non_injectable_type(name: &str) -> bool {
    matches!(
        name,
        "String"
            | "CharSequence"
            | "Object"
            | "Integer"
            | "Long"
            | "Double"
            | "Float"
            | "Short"
            | "Byte"
            | "Boolean"
            | "Character"
            | "Number"
            | "BigDecimal"
            | "BigInteger"
    )
}

/// Simple type name of an injectable dependency, or `None` for primitives,
/// value types (String/boxed), generics (`List<T>`, `Optional<T>`) and arrays.
fn injectable_type_name<'a>(type_node: TsNode<'a>, src: &'a [u8]) -> Option<String> {
    match type_node.kind() {
        "type_identifier" => {
            let name = text_of(type_node, src);
            (!is_non_injectable_type(name)).then(|| name.to_string())
        }
        // `com.foo.Bar` → trailing simple name `Bar`.
        "scoped_type_identifier" => {
            let full = text_of(type_node, src);
            let simple = full.rsplit('.').next().unwrap_or(full).trim();
            (!simple.is_empty() && !is_non_injectable_type(simple)).then(|| simple.to_string())
        }
        _ => None,
    }
}

/// Record an INJECTS ref: the consumer class → the bare dependency TYPE name.
/// The graph resolver binds `Bare(TypeName)` to the uniquely-named class /
/// interface node across the repo and forms the CLASS→service INJECTS edge.
fn push_inject_ref(from: NodeId, from_module: NodeId, type_name: String, acc: &mut Acc) {
    acc.refs.push(UnresolvedRef {
        from,
        from_module,
        qualifier: CallQualifier::Bare(type_name),
        category: edge_category::INJECTS,
    });
}

/// Constructor injection: one INJECTS ref per bean-typed constructor parameter.
fn emit_constructor_injects(
    ctor: TsNode,
    src: &[u8],
    class_id: NodeId,
    module_id: NodeId,
    acc: &mut Acc,
) {
    let Some(params) = ctor.child_by_field_name("parameters") else {
        return;
    };
    let mut c = params.walk();
    for p in params.named_children(&mut c) {
        if p.kind() != "formal_parameter" {
            continue;
        }
        let Some(ty) = p.child_by_field_name("type") else {
            continue;
        };
        if let Some(name) = injectable_type_name(ty, src) {
            push_inject_ref(class_id, module_id, name, acc);
        }
    }
}

/// Field injection: `@Autowired private FooService foo;` → INJECTS FooService.
fn emit_field_inject(
    field: TsNode,
    src: &[u8],
    class_id: NodeId,
    module_id: NodeId,
    acc: &mut Acc,
) {
    let Some(mods) = modifiers_text(&field, src) else {
        return;
    };
    if !INJECT_ANNOTATIONS.iter().any(|a| has_annotation(mods, a)) {
        return;
    }
    let Some(ty) = field.child_by_field_name("type") else {
        return;
    };
    if let Some(name) = injectable_type_name(ty, src) {
        push_inject_ref(class_id, module_id, name, acc);
    }
}

/// The annotations attached to THIS declaration — its direct
/// `annotation` / `marker_annotation` children plus those inside its direct
/// `modifiers` child (tree-sitter-java puts them in either place). It
/// deliberately does not descend into the body, so a class no longer sees —
/// and re-emits — its own methods' route annotations.
///
/// Returns `(simple name, first string argument)`; a `scoped_identifier` name
/// (`@jakarta.ws.rs.Path`) is reduced to its last segment, and the marker form
/// (`@PostMapping`, no arguments) yields `None` for the argument.
fn own_annotations<'a>(node: TsNode<'a>, src: &'a [u8]) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "annotation" | "marker_annotation" => push_annotation(child, src, &mut out),
            "modifiers" => {
                let mut inner = child.walk();
                for ann in child.named_children(&mut inner) {
                    if matches!(ann.kind(), "annotation" | "marker_annotation") {
                        push_annotation(ann, src, &mut out);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn push_annotation<'a>(ann: TsNode<'a>, src: &'a [u8], out: &mut Vec<(String, Option<String>)>) {
    let Some(name_node) = ann.child_by_field_name("name") else {
        return;
    };
    let name = text_of(name_node, src)
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    let arg = ann
        .child_by_field_name("arguments")
        .and_then(|args| annotation_string_arg(args, src));
    out.push((name, arg));
}

/// The path literal of an `annotation_argument_list`. Prefers an
/// `element_value_pair` keyed `value` / `path` / `uri` / `uris` (so
/// `@GetMapping(produces = "application/json", path = "/x")` yields `/x`, not
/// the media type), then the first bare `string_literal`, and finally falls
/// back to the existing text scanner for shapes the AST does not spell out
/// (e.g. `uris = {"/a", "/b"}`).
fn annotation_string_arg(args: TsNode, src: &[u8]) -> Option<String> {
    let mut cursor = args.walk();
    let mut bare = None;
    for child in args.named_children(&mut cursor) {
        match child.kind() {
            "element_value_pair" => {
                let key = child
                    .child_by_field_name("key")
                    .map(|k| text_of(k, src))
                    .unwrap_or_default();
                if matches!(key, "value" | "path" | "uri" | "uris")
                    && let Some(v) = child.child_by_field_name("value")
                    && v.kind() == "string_literal"
                {
                    return Some(java_string_inner(v, src));
                }
            }
            "string_literal" if bare.is_none() => bare = Some(java_string_inner(child, src)),
            _ => {}
        }
    }
    bare.or_else(|| extract_annotation_string(text_of(args, src)))
}

/// The route prefix a type contributes to its action methods: the first of
/// Spring `@RequestMapping`, Micronaut `@Controller` or JAX-RS `@Path` that
/// carries a non-empty string argument. Empty when the type is not prefixed.
fn class_route_prefix(type_node: TsNode, src: &[u8]) -> String {
    for (name, arg) in own_annotations(type_node, src) {
        if matches!(name.as_str(), "RequestMapping" | "Controller" | "Path")
            && let Some(p) = arg
            && !p.is_empty()
        {
            return p;
        }
    }
    String::new()
}

/// Spring `@GetMapping`-style and Micronaut `@Get`-style verb annotations.
fn mapping_verb(name: &str) -> Option<&'static str> {
    Some(match name {
        "GetMapping" | "Get" => "GET",
        "PostMapping" | "Post" => "POST",
        "PutMapping" | "Put" => "PUT",
        "DeleteMapping" | "Delete" => "DELETE",
        "PatchMapping" | "Patch" => "PATCH",
        "Head" => "HEAD",
        "Options" => "OPTIONS",
        _ => return None,
    })
}

/// JAX-RS verb markers (`@GET`, `@POST`, …) — upper-case, so they never
/// collide with Micronaut's `@Get` / `@Post`.
fn jaxrs_verb(anns: &[(String, Option<String>)]) -> Option<&'static str> {
    anns.iter().find_map(|(n, _)| match n.as_str() {
        "GET" => Some("GET"),
        "POST" => Some("POST"),
        "PUT" => Some("PUT"),
        "DELETE" => Some("DELETE"),
        "PATCH" => Some("PATCH"),
        "HEAD" => Some("HEAD"),
        "OPTIONS" => Some("OPTIONS"),
        _ => None,
    })
}

/// Compose a class-level prefix with an action template. Spring, Micronaut and
/// JAX-RS all CONCATENATE — a leading `/` on the method template does not make
/// it absolute — so `@RequestMapping("/api/v1/users")` + `@GetMapping("/{id}")`
/// is `/api/v1/users/{id}`. `join_path` does the slash bookkeeping; `abs_path`
/// supplies the leading `/` that `join_path` deliberately does not force.
fn compose_route_path(class_prefix: &str, tmpl: &str) -> String {
    if tmpl.is_empty() {
        return endpoint::abs_path(class_prefix);
    }
    endpoint::abs_path(&endpoint::join_path(class_prefix, tmpl))
}

/// Emit the ROUTEs declared by THIS declaration's own annotations, composed
/// onto `class_prefix` (empty at class level, the enclosing type's prefix at
/// method level). Returns how many routes were emitted.
fn check_route_annotations(
    node: TsNode,
    src: &[u8],
    _file_rel: &str,
    handler_id: NodeId,
    repo: RepoId,
    class_prefix: &str,
    acc: &mut Acc,
) -> usize {
    let anns = own_annotations(node, src);
    let mut emitted = 0usize;
    // A marker annotation (`@PostMapping`) carries no template of its own: it
    // maps the class prefix itself. With no prefix either there is nothing to
    // name, so stay silent rather than invent a route.
    let mut emit = |verb: &str, tmpl: Option<&str>, acc: &mut Acc| {
        if tmpl.is_none() && class_prefix.is_empty() {
            return;
        }
        emit_route(
            verb,
            &compose_route_path(class_prefix, tmpl.unwrap_or_default()),
            handler_id,
            repo,
            acc,
        );
        emitted += 1;
    };

    for (name, arg) in &anns {
        // Spring @GetMapping("/x") / Micronaut @Get("/x").
        if let Some(verb) = mapping_verb(name) {
            emit(verb, arg.as_deref(), acc);
        }
        // Spring @RequestMapping — `method = RequestMethod.GET` is not read, so
        // it stays the ANY wildcard it has always been.
        // Micronaut @Controller("/api") — the class base route.
        if matches!(name.as_str(), "RequestMapping" | "Controller") {
            emit("ANY", arg.as_deref(), acc);
        }
    }

    // JAX-RS: @Path("/x") with the verb from a marker on the SAME declaration.
    let verb = jaxrs_verb(&anns);
    if let Some((_, arg)) = anns.iter().find(|(n, _)| n == "Path") {
        emit(verb.unwrap_or("ANY"), arg.as_deref(), acc);
    } else if let Some(verb) = verb {
        // A verb marker with no @Path maps the resource root itself.
        emit(verb, None, acc);
    }
    emitted
}

fn emit_route(method: &str, path: &str, handler_id: NodeId, repo: RepoId, acc: &mut Acc) {
    let route_name = format!("{method} {path}");
    let route_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::ROUTE, &route_name);
    acc.nodes.push(Node {
        id: route_id,
        repo,
        confidence: Confidence::Strong,
        cells: vec![Cell {
            kind: cell_type::ROUTE_METHOD,
            payload: CellPayload::Text(method.to_string()),
        }],
    });
    acc.edges.push(Edge {
        from: route_id,
        to: handler_id,
        category: edge_category::HANDLED_BY,
        confidence: Confidence::Strong,
    });
    acc.nav
        .record(route_id, &route_name, &route_name, node_kind::ROUTE, None);
}

fn scan_ktor_routes(source: &str, repo: RepoId, acc: &mut Acc) {
    // Ktor (Kotlin): `get("/path") { ... }`, `post("/path") { ... }`, etc.
    // File is Kotlin (tree-sitter-java rejects most of it so we rely on text).
    let methods: &[(&str, &str)] = &[
        ("get(\"", "GET"),
        ("post(\"", "POST"),
        ("put(\"", "PUT"),
        ("patch(\"", "PATCH"),
        ("delete(\"", "DELETE"),
        ("head(\"", "HEAD"),
        ("options(\"", "OPTIONS"),
    ];
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (needle, method) in methods {
        let mut search_from = 0;
        while let Some(rel) = source[search_from..].find(needle) {
            let pos = search_from + rel;
            // Require the needle to be a word-start so we don't match e.g.
            // `forget("...")` or `setget("...")`.
            let word_start = pos == 0 || {
                let prev = source.as_bytes()[pos - 1];
                !(prev.is_ascii_alphanumeric() || prev == b'_' || prev == b'.')
            };
            let start = pos + needle.len();
            if !word_start {
                search_from = start;
                continue;
            }
            let bytes = source.as_bytes();
            let mut j = start;
            while j < bytes.len() && bytes[j] != b'"' {
                if bytes[j] == b'\\' && j + 1 < bytes.len() {
                    j += 2;
                } else {
                    j += 1;
                }
            }
            if j >= bytes.len() {
                break;
            }
            let path = &source[start..j];
            // Ktor DSL expects routes to start with `/`. This filters out many
            // false positives (e.g., `get("count")`) at zero cost.
            if !path.starts_with('/') {
                search_from = j + 1;
                continue;
            }
            // Look ahead for opening `{` — Ktor route DSL always opens a block.
            let after_paren = source[j + 1..]
                .find(|c: char| !c.is_whitespace() && c != ')')
                .map(|o| source.as_bytes()[j + 1 + o]);
            if after_paren != Some(b'{') {
                search_from = j + 1;
                continue;
            }
            let key = format!("{method} {path}");
            if seen.insert(key.clone()) {
                emit_ktor_route(method, path, repo, acc);
            }
            search_from = j + 1;
        }
    }
}

fn scan_webflux_routes(source: &str, repo: RepoId, acc: &mut Acc) {
    // Spring WebFlux functional DSL: RouterFunctions.route().GET("/path", h).POST(...)
    let methods: &[(&str, &str)] = &[
        (".GET(\"", "GET"),
        (".POST(\"", "POST"),
        (".PUT(\"", "PUT"),
        (".PATCH(\"", "PATCH"),
        (".DELETE(\"", "DELETE"),
        (".HEAD(\"", "HEAD"),
        (".OPTIONS(\"", "OPTIONS"),
    ];
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let bytes = source.as_bytes();
    for (needle, method) in methods {
        let mut search_from = 0;
        while let Some(rel) = source[search_from..].find(needle) {
            let pos = search_from + rel;
            let start = pos + needle.len();
            let mut j = start;
            while j < bytes.len() && bytes[j] != b'"' {
                if bytes[j] == b'\\' && j + 1 < bytes.len() {
                    j += 2;
                } else {
                    j += 1;
                }
            }
            if j >= bytes.len() {
                break;
            }
            let path = &source[start..j];
            if !path.starts_with('/') {
                search_from = j + 1;
                continue;
            }
            let key = format!("{method} {path}");
            if seen.insert(key.clone()) {
                emit_ktor_route(method, path, repo, acc);
            }
            search_from = j + 1;
        }
    }
}

fn scan_javalin_routes(source: &str, repo: RepoId, acc: &mut Acc) {
    // Javalin: `app.get("/path", handler)` / `app.post("/path", ctx -> {...})`.
    // Distinct from Ktor (top-level `get("/path") { ... }`): Javalin always has
    // a receiver (`app.` / `router.`) and never a trailing `{` block — both
    // ruled out by the Ktor scanner above. Discriminator from `Map.get("k")`
    // is the path-`/` first-arg filter.
    let methods: &[(&str, &str)] = &[
        (".get(\"", "GET"),
        (".post(\"", "POST"),
        (".put(\"", "PUT"),
        (".patch(\"", "PATCH"),
        (".delete(\"", "DELETE"),
        (".head(\"", "HEAD"),
        (".options(\"", "OPTIONS"),
    ];
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let bytes = source.as_bytes();
    for (needle, method) in methods {
        let mut search_from = 0;
        while let Some(rel) = source[search_from..].find(needle) {
            let pos = search_from + rel;
            let start = pos + needle.len();
            let mut j = start;
            while j < bytes.len() && bytes[j] != b'"' {
                if bytes[j] == b'\\' && j + 1 < bytes.len() {
                    j += 2;
                } else {
                    j += 1;
                }
            }
            if j >= bytes.len() {
                break;
            }
            let path = &source[start..j];
            if !path.starts_with('/') {
                search_from = j + 1;
                continue;
            }
            // Must have a comma after the path (Javalin always takes a handler
            // as the second arg). Filters out single-arg `.get("/x")` fetcher
            // calls that happen to use a slash key.
            let after = source[j + 1..].trim_start();
            if !after.starts_with(',') {
                search_from = j + 1;
                continue;
            }
            let key = format!("{method} {path}");
            if seen.insert(key.clone()) {
                emit_ktor_route(method, path, repo, acc);
            }
            search_from = j + 1;
        }
    }
}

fn emit_ktor_route(method: &str, path: &str, repo: RepoId, acc: &mut Acc) {
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
}

fn extract_annotation_string(text: &str) -> Option<String> {
    let paren = text.find('(')?;
    let rest = &text[paren + 1..];
    // Find first quoted string: "..." or value = "..."
    let quote_start = rest.find('"')?;
    let after = &rest[quote_start + 1..];
    let quote_end = after.find('"')?;
    Some(after[..quote_end].to_string())
}

fn collect_import(node: TsNode, src: &[u8], from_module: &str, acc: &mut Acc) {
    // `import com.foo.bar.Baz;` or `import static com.foo.bar.Baz.method;`
    let text = text_of(node, src).trim().to_string();
    let path = text
        .trim_start_matches("import ")
        .trim_start_matches("static ")
        .trim_end_matches(';')
        .trim();
    acc.http_libs.note_import(path);

    if path.ends_with(".*") {
        // Wildcard import — module import
        let module_path = path.trim_end_matches(".*").replace('.', "::");
        acc.imports.push(ImportStmt {
            from_module: from_module.to_string(),
            target: ImportTarget::Module {
                path: module_path,
                alias: None,
            },
        });
    } else if let Some(last_dot) = path.rfind('.') {
        let module_part = &path[..last_dot];
        let name = &path[last_dot + 1..];
        acc.imports.push(ImportStmt {
            from_module: from_module.to_string(),
            target: ImportTarget::Symbol {
                module: module_part.replace('.', "::"),
                name: name.to_string(),
                alias: None,
                level: 0,
            },
        });
    }
}

fn collect_calls_in(
    node: TsNode,
    src: &[u8],
    from: NodeId,
    repo: RepoId,
    file_rel: &str,
    acc: &mut Acc,
) {
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if n.kind() == "method_invocation" {
            // Pattern A: client HTTP call (`rest.getForObject('/x', …)`,
            // `webClient.get().uri('/x')`) → ENDPOINT node so the
            // HttpStackResolver can pair it with a server ROUTE.
            try_detect_java_endpoint(n, src, from, repo, file_rel, acc);
            let qualifier = classify_method_invocation(n, src);
            acc.calls.push(CallSite { from, qualifier });
        } else if n.kind() == "object_creation_expression" {
            // LA.22a: Apache `new HttpGet(url)` is a request, not a call.
            try_detect_java_request_object(n, src, from, repo, file_rel, acc);
        }
        let mut cursor = n.walk();
        for child in n.named_children(&mut cursor) {
            if !matches!(
                child.kind(),
                "class_declaration"
                    | "lambda_expression"
                    | "method_declaration"
                    | "anonymous_class_body"
            ) {
                stack.push(child);
            }
        }
    }
}

fn classify_method_invocation(node: TsNode, src: &[u8]) -> CallQualifier {
    let name = node
        .child_by_field_name("name")
        .map(|n| text_of(n, src))
        .unwrap_or("");
    if let Some(obj) = node.child_by_field_name("object") {
        let obj_text = text_of(obj, src);
        if obj_text == "this" {
            CallQualifier::SelfMethod(name.to_string())
        } else if obj.kind() == "identifier" {
            CallQualifier::Attribute {
                base: obj_text.to_string(),
                name: name.to_string(),
            }
        } else {
            CallQualifier::ComplexReceiver {
                receiver: obj_text.to_string(),
                name: name.to_string(),
            }
        }
    } else {
        // No receiver: in Java an unqualified call `foo()` is an implicit
        // `this.foo()` — a method of the enclosing type (or an inherited one).
        // Java has no module-level free functions, so a bare `method_invocation`
        // never resolves against module top-level symbols; classify it as
        // `SelfMethod` so `resolve_calls` binds it against the enclosing class's
        // methods (`class_methods[<enclosing class>]`).
        CallQualifier::SelfMethod(name.to_string())
    }
}

const HTTP_VERBS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

/// Pattern A: detect a Spring client HTTP call and emit a shared ENDPOINT node
/// (+ CALLS edge from the enclosing method `from`). Three idioms:
///
///   RestTemplate:  `rest.getForObject(url, C)` / `.postForObject(url, r, C)` …
///                  verb from the method name; `.exchange(url, HttpMethod.GET, …)`
///                  / `.execute(…)` take the verb from the `HttpMethod.<VERB>` arg.
///   WebClient:     `webClient.get().uri(url)…` — fires on the `.uri(url)` call;
///                  verb walked back down the fluent chain (`.get()`/`.post()`/
///                  `.method(HttpMethod.GET)`).
///
/// LA.22a adds the imperative clients, each only when the file imports the
/// library (see [`JavaHttpLibs`]):
///
///   java.net.http: `HttpRequest.newBuilder(…)…build()` and
///   OkHttp:        `new Request.Builder()…build()` — fire on the `.build()`
///                  that finishes the chain, via [`builder_request_parts`], so
///                  a request is one ENDPOINT however many setters it has.
///                  Every other arm skips a chain rooted at one of these
///                  builders: its `.uri(…)` / `.put(body)` are setters of the
///                  request the `build` arm emits.
///   Apache 5:      `SimpleRequestBuilder.get(url)` / `ClassicRequestBuilder
///                  .post(url)` — verb from the method name. (Apache's
///                  `new HttpGet(url)` is [`try_detect_java_request_object`].)
///
/// URL is the first call argument: a plain `"…"` literal (→ Strong) or a `+`
/// concatenation whose non-literal parts become `${…}` wildcards (→ Medium),
/// either one optionally wrapped in `URI.create` & co (see
/// [`url_string_from_arg`]). The `url_to_path` filter (path must start `/`)
/// rules out `map.put("k", v)` &c.
fn try_detect_java_endpoint(
    n: TsNode,
    src: &[u8],
    from: NodeId,
    repo: RepoId,
    file_rel: &str,
    acc: &mut Acc,
) {
    let name = n
        .child_by_field_name("name")
        .map(|x| text_of(x, src))
        .unwrap_or("");
    let args = n.child_by_field_name("arguments");
    let obj = n.child_by_field_name("object");
    let root = obj.and_then(|o| request_builder_root(o, src));

    if let Some(client) = root {
        if name == "build"
            && acc.http_libs.has(client)
            && let Some((method, url_arg)) = builder_request_parts(n, client, src)
        {
            emit_java_client_endpoint(
                n,
                method,
                url_arg,
                Some(client),
                src,
                from,
                repo,
                file_rel,
                acc,
            );
        }
        return;
    }

    if acc.http_libs.apache
        && let Some(o) = obj
        && is_apache5_request_builder(text_of(o, src))
        && let Some(verb) = lower_verb(name)
    {
        if let Some(url_arg) = first_arg(args) {
            emit_java_client_endpoint(
                n,
                verb,
                url_arg,
                Some(JavaHttpClient::Apache),
                src,
                from,
                repo,
                file_rel,
                acc,
            );
        }
        return;
    }

    let method = if name == "uri" {
        // WebClient: verb comes from the fluent chain the `.uri(…)` hangs off.
        let Some(obj) = obj else {
            return;
        };
        let Some(v) = webclient_verb(obj, src) else {
            return;
        };
        v
    } else if name == "exchange" || name == "execute" {
        // RestTemplate low-level: verb is the `HttpMethod.<VERB>` argument.
        let Some(v) = http_method_arg_verb(args, src) else {
            return;
        };
        v
    } else if let Some(v) = rest_template_verb(name) {
        v.to_string()
    } else {
        return;
    };

    // URL is the first argument.
    let Some(url_arg) = first_arg(args) else {
        return;
    };
    emit_java_client_endpoint(n, method, url_arg, None, src, from, repo, file_rel, acc);
}

/// Shared sink of every Java client arm: resolve `url_arg` to a path, and emit
/// the ENDPOINT (+ CALLS from `from`) positioned at `site`, the node that
/// fired. `client` names the LA.22a library to count for the marker (`None`
/// for the Spring arms). Returns whether an endpoint was emitted; a URL that
/// is not a literal / concatenation, or whose path does not start `/`, emits
/// nothing.
#[allow(clippy::too_many_arguments)]
fn emit_java_client_endpoint(
    site: TsNode,
    method: String,
    url_arg: TsNode,
    client: Option<JavaHttpClient>,
    src: &[u8],
    from: NodeId,
    repo: RepoId,
    file_rel: &str,
    acc: &mut Acc,
) -> bool {
    let Some((raw, strong)) = url_string_from_arg(url_arg, src) else {
        return false;
    };
    // Path is the ENDPOINT's identity; the authority rides on ENDPOINT_HIT as
    // `host` (A11.5), and only when it is literal (`"http://" + h + "/x"` has
    // none).
    let (host, path) = endpoint::client_url_split(&raw);
    let Some(path) = path else {
        return false;
    };

    let pos = site.start_position();
    let ep = ClientEndpoint {
        method,
        path,
        file: file_rel.to_string(),
        line: pos.row + 1,
        col: pos.column + 1,
        confidence: if strong {
            Confidence::Strong
        } else {
            Confidence::Medium
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
    if let Some(c) = client {
        acc.http_clients.bump(c);
    }
    true
}

/// LA.22a: Apache HttpClient's `new HttpGet(url)` & co — a request object
/// whose class names the verb and whose first constructor argument is the URL.
/// Gated on the file importing `org.apache.http` / `org.apache.hc`, or on the
/// class being spelled fully qualified.
fn try_detect_java_request_object(
    n: TsNode,
    src: &[u8],
    from: NodeId,
    repo: RepoId,
    file_rel: &str,
    acc: &mut Acc,
) {
    let Some(ty) = n.child_by_field_name("type").map(|t| text_of(t, src)) else {
        return;
    };
    if !acc.http_libs.apache && !is_apache_http_package(ty) {
        return;
    }
    let class = ty.rsplit('.').next().unwrap_or(ty);
    let Some(method) = apache_request_class_verb(class) else {
        return;
    };
    let Some(url_arg) = first_arg(n.child_by_field_name("arguments")) else {
        return;
    };
    emit_java_client_endpoint(
        n,
        method.to_string(),
        url_arg,
        Some(JavaHttpClient::Apache),
        src,
        from,
        repo,
        file_rel,
        acc,
    );
}

/// Apache HttpClient 4/5 request class → verb (`HttpGet` → `GET`, …).
fn apache_request_class_verb(class: &str) -> Option<&'static str> {
    match class {
        "HttpGet" => Some("GET"),
        "HttpPost" => Some("POST"),
        "HttpPut" => Some("PUT"),
        "HttpPatch" => Some("PATCH"),
        "HttpDelete" => Some("DELETE"),
        "HttpHead" => Some("HEAD"),
        "HttpOptions" => Some("OPTIONS"),
        _ => None,
    }
}

/// Apache HttpClient 5's static request builders (`SimpleRequestBuilder.get(url)`,
/// `ClassicRequestBuilder.post(url)`), bare or fully qualified.
fn is_apache5_request_builder(receiver: &str) -> bool {
    let simple = receiver.rsplit('.').next().unwrap_or(receiver);
    matches!(simple, "SimpleRequestBuilder" | "ClassicRequestBuilder")
}

/// A lower-case verb method name (`get`, `post`, …) → its upper-case verb.
fn lower_verb(name: &str) -> Option<String> {
    let up = name.to_ascii_uppercase();
    (name == up.to_ascii_lowercase() && HTTP_VERBS.contains(&up.as_str())).then_some(up)
}

/// LA.22a: the builder a receiver chain hangs off — walk the `object` field of
/// each `method_invocation` down to the chain's first expression. `Jdk` when it
/// is `HttpRequest.newBuilder(…)`, `OkHttp` when it is `new Request.Builder()`.
/// No import gate: it decides which arm OWNS a chain; the arms that emit gate.
fn request_builder_root(obj: TsNode, src: &[u8]) -> Option<JavaHttpClient> {
    let mut cur = obj;
    loop {
        match cur.kind() {
            "method_invocation" => {
                if is_jdk_new_builder(cur, src) {
                    return Some(JavaHttpClient::Jdk);
                }
                cur = cur.child_by_field_name("object")?;
            }
            "object_creation_expression" => {
                let ty = cur
                    .child_by_field_name("type")
                    .map(|t| text_of(t, src))
                    .unwrap_or("");
                return matches!(ty, "Request.Builder" | "okhttp3.Request.Builder")
                    .then_some(JavaHttpClient::OkHttp);
            }
            _ => return None,
        }
    }
}

/// `HttpRequest.newBuilder(…)` (java.net.http), bare or fully qualified.
fn is_jdk_new_builder(inv: TsNode, src: &[u8]) -> bool {
    let name = inv
        .child_by_field_name("name")
        .map(|x| text_of(x, src))
        .unwrap_or("");
    name == "newBuilder"
        && inv
            .child_by_field_name("object")
            .is_some_and(|o| matches!(text_of(o, src), "HttpRequest" | "java.net.http.HttpRequest"))
}

/// LA.22a: `(verb, url argument)` of the request a JDK / OkHttp `.build()`
/// finishes. `chain_top` is the `build` invocation; its receiver chain is
/// walked down to the builder root. The OUTERMOST setter wins, as the builder
/// keeps the last value set, so the URL and verb are taken only while unset.
///
///   Jdk:    URL from `.uri(u)`, else `newBuilder(u)`; verb from `.GET()` /
///           `.POST(b)` / `.PUT(b)` / `.DELETE()` / `.HEAD()`.
///   OkHttp: URL from `.url(u)`; verb from `.get()` / `.post(b)` / `.put(b)` /
///           `.patch(b)` / `.delete()` / `.delete(b)` / `.head()`.
///   Both:   `.method("PATCH", b)` — a string-literal verb; any other verb
///           argument leaves the verb unknown and emits nothing. Unset → GET,
///           each builder's own default. No URL → nothing.
fn builder_request_parts<'a>(
    chain_top: TsNode<'a>,
    client: JavaHttpClient,
    src: &[u8],
) -> Option<(String, TsNode<'a>)> {
    let mut url: Option<TsNode<'a>> = None;
    // Outer None = unset; Some(None) = set, but not to a literal verb.
    let mut verb: Option<Option<String>> = None;
    let mut cur = chain_top.child_by_field_name("object")?;
    while cur.kind() == "method_invocation" {
        let name = cur
            .child_by_field_name("name")
            .map(|x| text_of(x, src))
            .unwrap_or("");
        let args = cur.child_by_field_name("arguments");
        let (url_setter, verb_setter) = match client {
            JavaHttpClient::Jdk => (
                name == "uri",
                matches!(name, "GET" | "POST" | "PUT" | "DELETE" | "HEAD"),
            ),
            JavaHttpClient::OkHttp => (
                name == "url",
                matches!(name, "get" | "post" | "put" | "patch" | "delete" | "head"),
            ),
            JavaHttpClient::Apache => return None,
        };
        let is_root = client == JavaHttpClient::Jdk && is_jdk_new_builder(cur, src);
        if url_setter || is_root {
            url = url.or_else(|| first_arg(args));
        } else if verb_setter {
            verb.get_or_insert_with(|| Some(name.to_ascii_uppercase()));
        } else if name == "method" {
            verb.get_or_insert_with(|| literal_verb_arg(args, src));
        }
        if is_root {
            break;
        }
        cur = cur.child_by_field_name("object")?;
    }
    let verb = verb.unwrap_or_else(|| Some("GET".to_string()))?;
    Some((verb, url?))
}

/// A `.method("PATCH", body)` first argument, when it is a string literal
/// naming an HTTP verb.
fn literal_verb_arg(args: Option<TsNode>, src: &[u8]) -> Option<String> {
    let a = first_arg(args)?;
    if a.kind() != "string_literal" {
        return None;
    }
    let up = java_string_inner(a, src).to_ascii_uppercase();
    HTTP_VERBS.contains(&up.as_str()).then_some(up)
}

/// Map a RestTemplate convenience-method name to its HTTP verb. `put`/`delete`
/// are guarded downstream by the `url_to_path` path filter (so `map.put("k", …)`
/// never survives), the `*For*` families are self-describing.
fn rest_template_verb(name: &str) -> Option<&'static str> {
    if name.starts_with("getFor") {
        Some("GET")
    } else if name.starts_with("postFor") {
        Some("POST")
    } else if name.starts_with("patchFor") {
        Some("PATCH")
    } else if name.starts_with("headFor") {
        Some("HEAD")
    } else if name.starts_with("optionsFor") {
        Some("OPTIONS")
    } else if name == "put" {
        Some("PUT")
    } else if name == "delete" {
        Some("DELETE")
    } else {
        None
    }
}

/// Verb from a `HttpMethod.<VERB>` argument (used by `.exchange`/`.execute` and
/// WebClient's `.method(HttpMethod.GET)`).
fn http_method_arg_verb(args: Option<TsNode>, src: &[u8]) -> Option<String> {
    let a = args?;
    let mut c = a.walk();
    for arg in a.named_children(&mut c) {
        let t = text_of(arg, src);
        if let Some(idx) = t.find("HttpMethod.") {
            let after = &t[idx + "HttpMethod.".len()..];
            let verb: String = after.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
            let up = verb.to_ascii_uppercase();
            if HTTP_VERBS.contains(&up.as_str()) {
                return Some(up);
            }
        }
    }
    None
}

/// Walk a WebClient fluent chain (the object a `.uri(…)` hangs off) down to the
/// verb call: `.get()`/`.post()`/… or `.method(HttpMethod.GET)`.
fn webclient_verb(obj: TsNode, src: &[u8]) -> Option<String> {
    let mut cur = obj;
    loop {
        if cur.kind() != "method_invocation" {
            return None;
        }
        let nm = cur
            .child_by_field_name("name")
            .map(|x| text_of(x, src))
            .unwrap_or("");
        let up = nm.to_ascii_uppercase();
        if HTTP_VERBS.contains(&up.as_str()) {
            return Some(up);
        }
        if nm == "method"
            && let Some(v) = http_method_arg_verb(cur.child_by_field_name("arguments"), src)
        {
            return Some(v);
        }
        cur = cur.child_by_field_name("object")?;
    }
}

/// First named child (first positional argument) of an `arguments` node.
fn first_arg<'a>(args: Option<TsNode<'a>>) -> Option<TsNode<'a>> {
    let a = args?;
    let mut c = a.walk();
    a.named_children(&mut c).next()
}

/// Extract the URL string from a call's first argument. Returns `(path, strong)`
/// where `strong` is true for a plain string literal. A `+` concatenation is
/// reconstructed with `${…}` in place of every non-literal operand so an
/// interpolated URL (`"/users/" + id`) normalises like a template path
/// (`/users/${…}` → `/users/{}`); `strong` is false for that case.
///
/// LA.22a: a single-argument URL wrapper — `URI.create(x)`, `new URI(x)`,
/// `HttpUrl.parse(x)`, `HttpUrl.get(x)` — is unwrapped first, so `x` is judged
/// by the same literal / concatenation rule.
fn url_string_from_arg(arg: TsNode, src: &[u8]) -> Option<(String, bool)> {
    let arg = unwrap_url_wrappers(arg, src);
    if arg.kind() == "string_literal" {
        return Some((java_string_inner(arg, src), true));
    }
    if arg.kind() == "binary_expression" {
        // Only string concatenation (`+`) reconstructs to a path.
        let mut out = String::new();
        let mut saw_literal = false;
        let mut c = arg.walk();
        for part in arg.named_children(&mut c) {
            if part.kind() == "string_literal" {
                out.push_str(&java_string_inner(part, src));
                saw_literal = true;
            } else {
                out.push_str("${…}");
            }
        }
        if saw_literal {
            return Some((out, false));
        }
    }
    None
}

/// Peel URL wrappers off a URL argument: `URI.create(x)`, `new URI(x)`
/// (java.net), `HttpUrl.parse(x)` / `HttpUrl.get(x)` (OkHttp), bare or fully
/// qualified. Only the one-argument forms: `new URI(scheme, host, path, …)`
/// has no single URL argument. Anything else is returned as is.
fn unwrap_url_wrappers<'a>(arg: TsNode<'a>, src: &[u8]) -> TsNode<'a> {
    let mut cur = arg;
    loop {
        let (wrapper, args) = match cur.kind() {
            "method_invocation" => {
                let recv = cur
                    .child_by_field_name("object")
                    .map(|o| text_of(o, src))
                    .unwrap_or("");
                let name = cur
                    .child_by_field_name("name")
                    .map(|x| text_of(x, src))
                    .unwrap_or("");
                let w = match recv {
                    "URI" | "java.net.URI" => name == "create",
                    "HttpUrl" | "okhttp3.HttpUrl" => matches!(name, "parse" | "get"),
                    _ => false,
                };
                (w, cur.child_by_field_name("arguments"))
            }
            "object_creation_expression" => {
                let ty = cur
                    .child_by_field_name("type")
                    .map(|t| text_of(t, src))
                    .unwrap_or("");
                (
                    matches!(ty, "URI" | "java.net.URI"),
                    cur.child_by_field_name("arguments"),
                )
            }
            _ => return cur,
        };
        let Some(a) = args.filter(|_| wrapper) else {
            return cur;
        };
        let mut c = a.walk();
        let mut named = a.named_children(&mut c);
        match (named.next(), named.next()) {
            (Some(only), None) => cur = only,
            _ => return cur,
        }
    }
}

/// Inner text of a Java `string_literal` node (strip the surrounding quotes).
fn java_string_inner(node: TsNode, src: &[u8]) -> String {
    text_of(node, src).trim_matches('"').to_string()
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

    /// Every qname the parse recorded for `kind`, sorted.
    fn qnames_of(fp: &FileParse, kind: repo_graph_core::NodeKindId) -> Vec<&str> {
        let mut out: Vec<&str> = fp
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == kind)
            .filter_map(|(id, _)| fp.nav.qname_by_id.get(id).map(|s| s.as_str()))
            .collect();
        out.sort_unstable();
        out
    }

    /// LB.2 — top-level types hang off the package (directory) scope, never the
    /// file module: no doubled `InvoiceService::InvoiceService` segment. The
    /// module qname is the engine's own (`path_to_qname` of the repo path).
    #[test]
    fn top_level_types_are_package_scoped() {
        let source = r#"
package com.example.billing;

import java.util.List;

public class InvoiceService {
    public int total(int x) { return round(x) + 1; }
    private int round(int x) { return x; }
    public static class Row {}
}

class LineItem {
    void touch() {}
}
"#;
        let module = "src::main::java::com::example::billing::InvoiceService";
        let fp = parse_file(
            source,
            "src/main/java/com/example/billing/InvoiceService.java",
            module,
            repo(),
        )
        .unwrap();
        assert_eq!(
            qnames_of(&fp, node_kind::CLASS),
            vec![
                "src::main::java::com::example::billing::InvoiceService",
                "src::main::java::com::example::billing::InvoiceService::Row",
                "src::main::java::com::example::billing::LineItem",
            ],
            "public, nested and secondary top-level classes"
        );
        assert_eq!(
            qnames_of(&fp, node_kind::METHOD),
            vec![
                "src::main::java::com::example::billing::InvoiceService::round",
                "src::main::java::com::example::billing::InvoiceService::total",
                "src::main::java::com::example::billing::LineItem::touch",
            ]
        );
        assert!(
            !fp.nav.qname_by_id.values().any(|q| q.contains("InvoiceService::InvoiceService")),
            "no qname doubles the file stem: {:?}",
            fp.nav.qname_by_id.values().collect::<Vec<_>>()
        );

        // The file MODULE is unchanged: it keeps the full module qname, so the
        // public class shares that qname under a different kind and NodeId.
        assert_eq!(qnames_of(&fp, node_kind::MODULE), vec![module]);
        let module_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, module);
        let class_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, module);
        assert_ne!(module_id, class_id);
        assert!(fp.nodes.iter().any(|n| n.id == class_id), "the CLASS node is emitted");
        assert!(
            fp.edges.iter().any(|e| e.from == module_id
                && e.to == class_id
                && e.category == edge_category::DEFINES),
            "the file module still DEFINES its public class"
        );
        // Imports still originate from the file module's qname.
        assert_eq!(fp.imports.len(), 1);
        assert_eq!(fp.imports[0].from_module, module);
        // A nested type hangs off its outer type, not the package.
        let row_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::CLASS,
            "src::main::java::com::example::billing::InvoiceService::Row",
        );
        assert_eq!(fp.nav.parent_of.get(&row_id), Some(&class_id));
    }

    /// LB.2 — a file at the repo root has an empty package scope: its type's
    /// qname is the bare type name.
    #[test]
    fn root_level_file_types_have_bare_qnames() {
        let source = "public class App {\n    public void run() {}\n}\n";
        let fp = parse_file(source, "App.java", "App", repo()).unwrap();
        assert_eq!(qnames_of(&fp, node_kind::CLASS), vec!["App"]);
        assert_eq!(qnames_of(&fp, node_kind::METHOD), vec!["App::run"]);
        assert_eq!(qnames_of(&fp, node_kind::MODULE), vec!["App"]);
        assert_eq!(type_scope("App"), "");
        assert_eq!(type_scope("tools::Tool"), "tools");
        assert_eq!(scoped("", "App"), "App");
        assert_eq!(scoped("tools", "Tool"), "tools::Tool");
    }

    #[test]
    fn classes_and_methods() {
        let source = r#"
package com.example;

public class UserService {
    public User getUser(String id) {
        return db.find(id);
    }

    private void validate(User u) {}
}
"#;
        let fp = parse_file(source, "src/main/java/UserService.java", "com::example::UserService", repo()).unwrap();
        let names: Vec<&str> = fp.nav.name_by_id.values().map(|s| s.as_str()).collect();
        assert!(names.contains(&"UserService"));
        assert!(names.contains(&"getUser"));
        assert!(names.contains(&"validate"));
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::CLASS).count(), 1);
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::METHOD).count(), 2);
    }

    #[test]
    fn interfaces_and_enums() {
        let source = r#"
package com.example;

public interface Drawable {
    void draw();
}

public enum Color {
    RED, GREEN, BLUE;
}
"#;
        let fp = parse_file(source, "src/main/java/Types.java", "com::example::Types", repo()).unwrap();
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::INTERFACE).count(), 1);
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::ENUM).count(), 1);
    }

    #[test]
    fn implements_and_state_var() {
        // G12.5: `implements IFoo` emits an IMPLEMENTS edge (class → interface).
        // G19: a documented `static final int FEE = 250;` emits a STATE_VAR.
        let source = r#"
package com.example;

public class X extends Base implements IFoo, IBar {
    /** The processing fee in cents. */
    public static final int FEE = 250;

    public static final int RAW = 7;
}
"#;
        let fp = parse_file(source, "src/main/java/X.java", "com::example::X", repo()).unwrap();

        // STATE_VAR: only the documented FEE survives the noise gate.
        let state_vars: Vec<&str> = fp
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::STATE_VAR)
            .filter_map(|(id, _)| fp.nav.name_by_id.get(id).map(|s| s.as_str()))
            .collect();
        assert_eq!(state_vars, vec!["FEE"]);

        // IMPLEMENTS refs: one per interface (IFoo, IBar). Heritage is emitted as
        // UnresolvedRefs (Bare qualifier) that the graph resolver later binds.
        let implements: Vec<&UnresolvedRef> = fp
            .refs
            .iter()
            .filter(|r| r.category == edge_category::IMPLEMENTS)
            .collect();
        assert_eq!(implements.len(), 2);
        assert!(
            implements
                .iter()
                .any(|r| r.qualifier == CallQualifier::Bare("IFoo".to_string()))
                && implements
                    .iter()
                    .any(|r| r.qualifier == CallQualifier::Bare("IBar".to_string())),
            "IMPLEMENTS refs must carry Bare(IFoo)/Bare(IBar): {implements:?}"
        );
        // extends Base → INHERITS_FROM ref (Bare(Base)).
        let inherits: Vec<&UnresolvedRef> = fp
            .refs
            .iter()
            .filter(|r| r.category == edge_category::INHERITS_FROM)
            .collect();
        assert_eq!(inherits.len(), 1);
        assert_eq!(
            inherits[0].qualifier,
            CallQualifier::Bare("Base".to_string())
        );
        // The heritage ref must originate from the class node and carry the
        // enclosing module id (so the resolver can scope the lookup).
        let x_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, "com::example::X");
        let module_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "com::example::X");
        assert!(inherits[0].from == x_id && inherits[0].from_module == module_id);
    }

    #[test]
    fn jpa_entity_and_repository_access() {
        // java-spring-accessdata fixture: an `@Entity` class projects a DATA_ENTITY
        // node; a `JpaRepository<User, Long>` interface gets an ACCESSES_DATA edge
        // to that entity (direct, name-derived — resolve_refs has no ACCESSES_DATA
        // fallback).
        let source = r#"
package com.example;

import javax.persistence.Entity;
import org.springframework.data.jpa.repository.JpaRepository;
import org.springframework.stereotype.Repository;

@Entity
class User {
    @Id
    private Long id;
    private String name;
}

@Repository
interface UserRepository extends JpaRepository<User, Long> {
    User findByName(String name);
}
"#;
        let fp = parse_file(source, "UserRepository.java", "com::example::UserRepository", repo()).unwrap();

        // @Entity → DATA_ENTITY node named User.
        let entity_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::DATA_ENTITY, "User");
        assert!(
            fp.nodes.iter().any(|n| n.id == entity_id),
            "expected DATA_ENTITY User node"
        );
        assert_eq!(
            fp.nav
                .kind_by_id
                .values()
                .filter(|k| **k == node_kind::DATA_ENTITY)
                .count(),
            1,
            "exactly one DATA_ENTITY"
        );

        // UserRepository (an interface) ACCESSES_DATA the User entity.
        let repo_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::INTERFACE,
            "com::example::UserRepository",
        );
        assert!(
            fp.edges.iter().any(|e| e.from == repo_id
                && e.to == entity_id
                && e.category == edge_category::ACCESSES_DATA),
            "expected ACCESSES_DATA UserRepository -> User: {:?}",
            fp.edges
        );
    }

    #[test]
    fn plain_class_and_generic_field_emit_no_entity_or_access() {
        // No @Entity, no repository base → no DATA_ENTITY, no ACCESSES_DATA noise
        // (a bare `List<User>` field must not be mistaken for a repository).
        let source = r#"
package com.example;

class Basket {
    private List<User> items;
}
"#;
        let fp = parse_file(source, "Basket.java", "com::example::Basket", repo()).unwrap();
        assert!(
            !fp.nav.kind_by_id.values().any(|k| *k == node_kind::DATA_ENTITY),
            "plain class must not emit a DATA_ENTITY"
        );
        assert!(
            !fp.edges
                .iter()
                .any(|e| e.category == edge_category::ACCESSES_DATA),
            "no repository base → no ACCESSES_DATA edge"
        );
    }

    #[test]
    fn imports() {
        let source = r#"
package com.example;

import com.example.models.User;
import java.util.*;
import static org.junit.Assert.assertEquals;
"#;
        let fp = parse_file(source, "src/main/java/App.java", "com::example::App", repo()).unwrap();
        assert_eq!(fp.imports.len(), 3);
    }

    #[test]
    fn spring_routes() {
        let source = r#"
package com.example;

public class UserController {
    @GetMapping("/users")
    public List<User> list() { return null; }

    @PostMapping("/users")
    public User create() { return null; }
}
"#;
        let fp = parse_file(source, "src/main/java/UserController.java", "com::example::UserController", repo()).unwrap();
        let routes: Vec<_> = fp
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::ROUTE)
            .filter_map(|(id, _)| fp.nav.name_by_id.get(id).map(|s| s.as_str()))
            .collect();
        assert!(routes.contains(&"GET /users"));
        assert!(routes.contains(&"POST /users"));
    }

    #[test]
    fn micronaut_routes() {
        let source = r#"
package com.example;

@Controller("/api")
public class ThingsController {
    @Get("/things")
    public Thing list() { return null; }

    @Post("/things")
    public Thing create() { return null; }

    @Put("/things/{id}")
    public Thing update() { return null; }

    @Delete("/things/{id}")
    public void destroy() {}
}
"#;
        let fp = parse_file(source, "ThingsController.java", "com::example::ThingsController", repo()).unwrap();
        let routes: Vec<&str> = fp
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::ROUTE)
            .filter_map(|(id, _)| fp.nav.name_by_id.get(id).map(|s| s.as_str()))
            .collect();
        assert!(routes.contains(&"GET /api/things"));
        assert!(routes.contains(&"PUT /api/things/{id}"));
        assert!(routes.contains(&"DELETE /api/things/{id}"));
        assert!(routes.contains(&"ANY /api"));
    }

    const SPRING_CLASS_PREFIXED: &str = r#"
package com.example;

@RestController
@RequestMapping("/api/v1/users")
public class UserController {
    @GetMapping("/{id}")
    public String getUser(String id) { return "user " + id; }

    @PostMapping
    public String createUser(String body) { return "created"; }
}
"#;

    #[test]
    fn spring_class_request_mapping_composes() {
        let fp = parse_file(
            SPRING_CLASS_PREFIXED,
            "server/UserController.java",
            "com::example::UserController",
            repo(),
        )
        .unwrap();
        let routes: Vec<&str> = fp
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::ROUTE)
            .filter_map(|(id, _)| fp.nav.name_by_id.get(id).map(|s| s.as_str()))
            .collect();
        assert!(
            routes.contains(&"GET /api/v1/users/{id}"),
            "class @RequestMapping must compose onto @GetMapping: {routes:?}"
        );
        assert!(
            routes.contains(&"POST /api/v1/users"),
            "a bare @PostMapping marker must inherit the class prefix: {routes:?}"
        );
        assert!(routes.contains(&"ANY /api/v1/users"), "class base route: {routes:?}");
        assert!(
            !routes.contains(&"GET /{id}"),
            "the uncomposed relative template must not survive: {routes:?}"
        );
        assert!(
            !routes.iter().any(|r| r.starts_with("POST created")),
            "the marker form must not scan forward into the method body: {routes:?}"
        );
    }

    #[test]
    fn spring_class_scan_does_not_duplicate_action_routes() {
        let fp = parse_file(
            SPRING_CLASS_PREFIXED,
            "server/UserController.java",
            "com::example::UserController",
            repo(),
        )
        .unwrap();
        let route_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::ROUTE,
            "GET /api/v1/users/{id}",
        );
        let handled: Vec<&str> = fp
            .edges
            .iter()
            .filter(|e| e.category == edge_category::HANDLED_BY && e.from == route_id)
            .filter_map(|e| fp.nav.name_by_id.get(&e.to).map(|s| s.as_str()))
            .collect();
        assert_eq!(
            handled,
            vec!["getUser"],
            "the class scan must not also claim the class itself as a handler"
        );
    }

    #[test]
    fn ktor_routes() {
        let source = r#"
fun Application.module() {
    routing {
        get("/users") {
            call.respond(listOf<String>())
        }
        post("/users") {
            call.respond("ok")
        }
        route("/admin") {
            delete("/users/{id}") { call.respond("ok") }
        }
    }
}
"#;
        let fp = parse_file(source, "Application.kt", "com::example::Application", repo()).unwrap();
        let routes: Vec<&str> = fp
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::ROUTE)
            .filter_map(|(id, _)| fp.nav.name_by_id.get(id).map(|s| s.as_str()))
            .collect();
        assert!(routes.contains(&"GET /users"));
        assert!(routes.contains(&"POST /users"));
        assert!(routes.contains(&"DELETE /users/{id}"));
    }

    #[test]
    fn webflux_functional_routes() {
        let source = r#"
@Configuration
public class RouterConfig {
    @Bean
    public RouterFunction<ServerResponse> routes(UserHandler handler) {
        return RouterFunctions.route()
            .GET("/users", handler::list)
            .POST("/users", handler::create)
            .DELETE("/users/{id}", handler::destroy)
            .build();
    }
}
"#;
        let fp = parse_file(source, "RouterConfig.java", "com::example::RouterConfig", repo()).unwrap();
        let routes: Vec<&str> = fp
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::ROUTE)
            .filter_map(|(id, _)| fp.nav.name_by_id.get(id).map(|s| s.as_str()))
            .collect();
        assert!(routes.contains(&"GET /users"));
        assert!(routes.contains(&"POST /users"));
        assert!(routes.contains(&"DELETE /users/{id}"));
    }

    #[test]
    fn javalin_routes() {
        let source = r#"
import io.javalin.Javalin;

public class App {
    public static void main(String[] args) {
        Javalin app = Javalin.create();
        app.get("/health", ctx -> ctx.result("ok"));
        app.post("/users", UserHandler::create);
        app.put("/users/{id}", UserHandler::update);
        app.delete("/users/{id}", UserHandler::destroy);
    }
}
"#;
        let fp = parse_file(source, "App.java", "com::example::App", repo()).unwrap();
        let routes: Vec<&str> = fp
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::ROUTE)
            .filter_map(|(id, _)| fp.nav.name_by_id.get(id).map(|s| s.as_str()))
            .collect();
        assert!(routes.contains(&"GET /health"));
        assert!(routes.contains(&"POST /users"));
        assert!(routes.contains(&"PUT /users/{id}"));
        assert!(routes.contains(&"DELETE /users/{id}"));
    }

    #[test]
    fn javalin_skips_map_get_with_path_key() {
        // `cache.get("/users")` shape: path-`/` filter alone would let it
        // through; the comma-after-path filter rejects it (single-arg call).
        let source = r#"
public class Svc {
    public String load() {
        return cache.get("/users");
    }
}
"#;
        let fp = parse_file(source, "Svc.java", "com::example::Svc", repo()).unwrap();
        let has_route = fp.nav.kind_by_id.values().any(|k| *k == node_kind::ROUTE);
        assert!(!has_route, "single-arg `.get(\"/key\")` must not emit a route");
    }

    #[test]
    fn javalin_skips_non_path_first_arg() {
        let source = r#"
public class Svc {
    public String load() {
        return cache.get("user-id", fallback);
    }
}
"#;
        let fp = parse_file(source, "Svc.java", "com::example::Svc", repo()).unwrap();
        let has_route = fp.nav.kind_by_id.values().any(|k| *k == node_kind::ROUTE);
        assert!(!has_route, "non-`/` first arg must not emit a route");
    }

    #[test]
    fn rest_template_client_call_emits_endpoint_not_route() {
        // Pattern A: RestTemplate client calls in a method → ENDPOINT nodes (not
        // phantom server ROUTEs), each with a CALLS edge from the enclosing
        // method. `"/users/" + id` concatenation → `/users/${…}` (Medium);
        // `.exchange(url, HttpMethod.GET, …)` takes its verb from the arg.
        let source = r#"
package com.example.client;

import org.springframework.web.client.RestTemplate;

public class ApiClient {
    private final RestTemplate rest = new RestTemplate();

    public String fetchUser(String id) {
        return rest.getForObject("/users/" + id, String.class);
    }

    public String createUser(String body) {
        return rest.postForObject("/users", body, String.class);
    }

    public ResponseEntity<String> raw() {
        return rest.exchange("http://api/users", HttpMethod.DELETE, null, String.class);
    }
}
"#;
        let fp = parse_file(source, "client/ApiClient.java", "com::example::client::ApiClient", repo()).unwrap();

        let ep_get =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ENDPOINT, "endpoint:GET:/users/${…}");
        let ep_post =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ENDPOINT, "endpoint:POST:/users");
        let ep_del =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ENDPOINT, "endpoint:DELETE:/users");

        assert!(fp.nodes.iter().any(|n| n.id == ep_get), "expected GET /users/${{…}} ENDPOINT");
        assert!(fp.nodes.iter().any(|n| n.id == ep_post), "expected POST /users ENDPOINT");
        assert!(
            fp.nodes.iter().any(|n| n.id == ep_del),
            "expected DELETE /users ENDPOINT (verb from HttpMethod arg, host stripped)"
        );

        // No phantom ROUTE nodes for the client calls.
        assert!(
            !fp.nav.kind_by_id.values().any(|k| *k == node_kind::ROUTE),
            "client RestTemplate calls must not become server ROUTEs"
        );

        // CALLS edge from the enclosing method into each endpoint.
        assert!(
            fp.edges.iter().any(|e| e.to == ep_get && e.category == edge_category::CALLS),
            "expected CALLS edge into the GET endpoint"
        );
        assert!(
            fp.edges.iter().any(|e| e.to == ep_post && e.category == edge_category::CALLS),
            "expected CALLS edge into the POST endpoint"
        );
    }

    /// A11.5 — a RestTemplate call against an absolute URL puts its authority
    /// on the ENDPOINT_HIT cell as `host`; a URI-template authority
    /// (`"http://{host}/orders"`, filled from uriVariables) names no service,
    /// so no `host`.
    #[test]
    fn rest_template_endpoint_carries_the_url_authority_as_host() {
        let source = r#"
public class ApiClient {
    public String fetch() {
        return rest.getForObject("http://svc:8080/x", String.class);
    }
    public String orders(String host) {
        return rest.getForObject("http://{host}/orders", String.class, host);
    }
}
"#;
        let fp = parse_file(source, "ApiClient.java", "com::example::ApiClient", repo()).unwrap();
        let hit = |qname: &str| -> String {
            let id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ENDPOINT, qname);
            let node = fp.nodes.iter().find(|n| n.id == id).expect("ENDPOINT node");
            match &node.cells[0].payload {
                CellPayload::Json(j) if node.cells[0].kind == cell_type::ENDPOINT_HIT => j.clone(),
                other => panic!("not an ENDPOINT_HIT json cell: {other:?}"),
            }
        };
        let x = hit("endpoint:GET:/x");
        assert!(
            x.ends_with(r#","confidence":"strong","host":"svc:8080"}"#),
            "{x}"
        );
        let orders = hit("endpoint:GET:/orders");
        assert!(!orders.contains("host"), "{orders}");
    }

    #[test]
    fn webclient_uri_call_emits_endpoint() {
        // Pattern A: WebClient fluent `webClient.get().uri('/x')` → verb from the
        // `.get()`/`.post()` in the chain, path from `.uri(...)`.
        let source = r#"
public class Client {
    public Mono<String> getUser(String id) {
        return webClient.get().uri("/users/{id}").retrieve().bodyToMono(String.class);
    }
    public Mono<Void> createOrder() {
        return webClient.post().uri("/orders").retrieve().bodyToMono(Void.class);
    }
}
"#;
        let fp = parse_file(source, "Client.java", "com::example::Client", repo()).unwrap();
        let ep_get =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ENDPOINT, "endpoint:GET:/users/{id}");
        let ep_post =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ENDPOINT, "endpoint:POST:/orders");
        assert!(fp.nodes.iter().any(|n| n.id == ep_get), "expected WebClient GET /users/{{id}}");
        assert!(fp.nodes.iter().any(|n| n.id == ep_post), "expected WebClient POST /orders");
        assert!(
            fp.edges.iter().any(|e| e.to == ep_post && e.category == edge_category::CALLS),
            "expected CALLS edge into the WebClient POST endpoint"
        );
    }

    #[test]
    fn map_put_is_not_an_endpoint() {
        // `.put`/`.delete` map onto verbs but the url_to_path filter (path must
        // start with `/`) rejects a non-path first arg like a map key.
        let source = r#"
public class Cache {
    public void store() {
        map.put("some-key", value);
    }
}
"#;
        let fp = parse_file(source, "Cache.java", "com::example::Cache", repo()).unwrap();
        assert!(
            !fp.nav.kind_by_id.values().any(|k| *k == node_kind::ENDPOINT),
            "map.put(\"key\", …) must not emit an ENDPOINT"
        );
    }

    /// ENDPOINT qnames in a parse, sorted.
    fn endpoint_qnames(fp: &FileParse) -> Vec<String> {
        let mut out: Vec<String> = fp
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::ENDPOINT)
            .filter_map(|(id, _)| fp.nav.qname_by_id.get(id).cloned())
            .collect();
        out.sort();
        out
    }

    fn endpoint_hit(fp: &FileParse, qname: &str) -> String {
        let id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ENDPOINT, qname);
        let node = fp.nodes.iter().find(|n| n.id == id).expect("ENDPOINT node");
        match &node.cells[0].payload {
            CellPayload::Json(j) if node.cells[0].kind == cell_type::ENDPOINT_HIT => j.clone(),
            other => panic!("not an ENDPOINT_HIT json cell: {other:?}"),
        }
    }

    /// LA.22a — java.net.http: `.uri(URI.create(…)).GET().build()`,
    /// `newBuilder(URI.create(…)).POST(…)` and `.method("PATCH", …)`, each one
    /// ENDPOINT at its `.build()` with the URL authority as `host`.
    #[test]
    fn jdk_http_request_builders_emit_one_endpoint_each() {
        let source = r#"
import java.net.URI;
import java.net.http.HttpRequest;

public class Clients {
    public void a() {
        HttpRequest r = HttpRequest.newBuilder().uri(URI.create("http://users-svc/users")).GET().build();
    }
    public void b() {
        HttpRequest r = HttpRequest.newBuilder(URI.create("http://orders-svc/orders"))
            .POST(HttpRequest.BodyPublishers.ofString("{}")).build();
    }
    public void c() {
        HttpRequest r = HttpRequest.newBuilder(new URI("/carts/" + id))
            .method("PATCH", HttpRequest.BodyPublishers.noBody()).build();
    }
}
"#;
        let fp = parse_file(source, "Clients.java", "com::example::Clients", repo()).unwrap();
        assert_eq!(
            endpoint_qnames(&fp),
            [
                "endpoint:GET:/users",
                "endpoint:PATCH:/carts/${…}",
                "endpoint:POST:/orders"
            ]
        );
        assert!(endpoint_hit(&fp, "endpoint:GET:/users").contains(r#""host":"users-svc""#));
        assert!(endpoint_hit(&fp, "endpoint:POST:/orders").contains(r#""host":"orders-svc""#));
        assert!(
            endpoint_hit(&fp, "endpoint:PATCH:/carts/${…}").contains(r#""confidence":"medium""#)
        );
        let ep = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::ENDPOINT,
            "endpoint:GET:/users",
        );
        assert_eq!(
            fp.edges
                .iter()
                .filter(|e| e.to == ep && e.category == edge_category::CALLS)
                .count(),
            1,
            "one CALLS edge per request, from the enclosing method"
        );
    }

    /// LA.22a — the verb set BEFORE `.uri(…)` must not also fire the WebClient
    /// `.uri` arm: exactly one ENDPOINT, one CALLS edge.
    #[test]
    fn jdk_verb_before_uri_is_one_endpoint() {
        let source = r#"
import java.net.URI;
import java.net.http.HttpRequest;

public class Clients {
    public void a() {
        HttpRequest r = HttpRequest.newBuilder().DELETE().uri(URI.create("http://svc/items/7")).build();
    }
}
"#;
        let fp = parse_file(source, "Clients.java", "com::example::Clients", repo()).unwrap();
        assert_eq!(endpoint_qnames(&fp), ["endpoint:DELETE:/items/7"]);
        assert_eq!(
            fp.edges
                .iter()
                .filter(|e| e.category == edge_category::CALLS
                    && fp.nav.kind_by_id.get(&e.to) == Some(&node_kind::ENDPOINT))
                .count(),
            1
        );
    }

    /// LA.22a — the java.net.http arm is gated on the import: the same chain in
    /// a file that does not import `java.net.http` emits nothing.
    #[test]
    fn jdk_builder_without_the_import_is_not_an_endpoint() {
        let source = r#"
public class Clients {
    public void a() {
        Object r = HttpRequest.newBuilder().uri(URI.create("http://svc/users")).GET().build();
    }
}
"#;
        let fp = parse_file(source, "Clients.java", "com::example::Clients", repo()).unwrap();
        assert!(
            endpoint_qnames(&fp).is_empty(),
            "{:?}",
            endpoint_qnames(&fp)
        );
    }

    /// LA.22a — OkHttp: `new Request.Builder().url(…).build()` defaults to GET;
    /// `.post(body)` sets POST; `HttpUrl.parse(…)` is unwrapped; the `.put(body)`
    /// setter never reaches the RestTemplate `put` arm.
    #[test]
    fn okhttp_request_builder_emits_endpoint() {
        let source = r#"
import okhttp3.HttpUrl;
import okhttp3.Request;
import okhttp3.RequestBody;

public class Clients {
    public void a() {
        Request r = new Request.Builder().url("http://users-svc/accounts").build();
    }
    public void b(RequestBody body) {
        Request r = new Request.Builder().url(HttpUrl.parse("/orders")).post(body).build();
    }
    public void c() {
        Request r = new Request.Builder().url("/carts").put(RequestBody.create("/x", null)).build();
    }
}
"#;
        let fp = parse_file(source, "Clients.java", "com::example::Clients", repo()).unwrap();
        assert_eq!(
            endpoint_qnames(&fp),
            [
                "endpoint:GET:/accounts",
                "endpoint:POST:/orders",
                "endpoint:PUT:/carts"
            ]
        );
        assert!(endpoint_hit(&fp, "endpoint:GET:/accounts").contains(r#""host":"users-svc""#));
    }

    /// LA.22a — Apache HttpClient 4 `new HttpDelete(url)` and HttpClient 5's
    /// `SimpleRequestBuilder.post(url)` / `ClassicRequestBuilder.put(url)`.
    #[test]
    fn apache_request_classes_and_builders_emit_endpoints() {
        let source = r#"
import org.apache.http.client.methods.HttpDelete;
import org.apache.hc.client5.http.async.methods.SimpleRequestBuilder;
import org.apache.hc.core5.http.io.support.ClassicRequestBuilder;

public class Clients {
    public void a() {
        HttpDelete del = new HttpDelete("http://users-svc/invoices");
    }
    public void b() {
        var req = SimpleRequestBuilder.post("http://billing/charges").build();
    }
    public void c() {
        var req = ClassicRequestBuilder.put(URI.create("/limits")).build();
    }
}
"#;
        let fp = parse_file(source, "Clients.java", "com::example::Clients", repo()).unwrap();
        assert_eq!(
            endpoint_qnames(&fp),
            [
                "endpoint:DELETE:/invoices",
                "endpoint:POST:/charges",
                "endpoint:PUT:/limits"
            ]
        );
        assert!(endpoint_hit(&fp, "endpoint:DELETE:/invoices").contains(r#""host":"users-svc""#));
    }

    /// LA.22a negatives: an unrelated `new HashMap<>()`, a `StringBuilder`'s
    /// `.build()`-alike and a non-Apache `new HttpGet(…)` (no import) emit
    /// nothing; nor does `new URI(scheme, host, path, frag)`.
    #[test]
    fn unrelated_objects_and_builders_are_not_endpoints() {
        let source = r#"
import java.net.URI;
import java.net.http.HttpRequest;
import okhttp3.Request;

public class Clients {
    public void a() {
        Map<String, String> m = new HashMap<>();
        String s = new StringBuilder().append("/users").build();
        Object g = new HttpGet("/users");
        Object q = Other.newBuilder().uri("/users").build();
        Request r = new Request.Builder().build();
        HttpRequest h = HttpRequest.newBuilder().method(verb, null).uri(URI.create("/x")).build();
        URI u = new URI("http", "svc", "/users", null);
    }
}
"#;
        let fp = parse_file(source, "Clients.java", "com::example::Clients", repo()).unwrap();
        assert!(
            endpoint_qnames(&fp).is_empty(),
            "{:?}",
            endpoint_qnames(&fp)
        );
    }

    /// LA.22a — `URI.create(…)` unwrapping also reaches the existing Spring
    /// arms: `rest.getForObject(URI.create("/users"), …)`.
    #[test]
    fn uri_create_is_unwrapped_for_rest_template() {
        let source = r#"
public class ApiClient {
    public String fetch() {
        return rest.getForObject(URI.create("http://svc/users"), String.class);
    }
}
"#;
        let fp = parse_file(source, "ApiClient.java", "com::example::ApiClient", repo()).unwrap();
        assert_eq!(endpoint_qnames(&fp), ["endpoint:GET:/users"]);
    }

    #[test]
    fn spring_di_emits_injects_refs() {
        // Pattern E: a @RestController bean injects UserService via its
        // constructor and FooService via an @Autowired field. Each emits an
        // INJECTS UnresolvedRef with a Bare(TypeName) qualifier from the
        // consumer class. Primitives (int) and value types (String) are skipped.
        let source = r#"
package com.example;

import org.springframework.beans.factory.annotation.Autowired;
import org.springframework.stereotype.Service;
import org.springframework.web.bind.annotation.RestController;

@Service
class UserService {
    public String find() { return "u"; }
}

@RestController
class UserController {
    @Autowired
    private FooService foo;

    private final UserService userService;

    @Autowired
    public UserController(UserService userService, int count, String name) {
        this.userService = userService;
    }
}
"#;
        let fp = parse_file(source, "UserController.java", "com::example::UserController", repo()).unwrap();

        let injects: Vec<&UnresolvedRef> = fp
            .refs
            .iter()
            .filter(|r| r.category == edge_category::INJECTS)
            .collect();

        // The consumer is UserController.
        let controller_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::CLASS,
            "com::example::UserController",
        );

        // Constructor injection of the bean-typed param.
        assert!(
            injects.iter().any(|r| r.from == controller_id
                && r.qualifier == CallQualifier::Bare("UserService".to_string())),
            "expected INJECTS UserService from UserController constructor"
        );
        // Field injection via @Autowired.
        assert!(
            injects.iter().any(|r| r.from == controller_id
                && r.qualifier == CallQualifier::Bare("FooService".to_string())),
            "expected INJECTS FooService from @Autowired field"
        );
        // Primitive (int) and value type (String) constructor params are skipped.
        assert!(
            !injects
                .iter()
                .any(|r| matches!(&r.qualifier, CallQualifier::Bare(n) if n == "String" || n == "count")),
            "primitives / value types must not be injected"
        );
        assert_eq!(injects.len(), 2, "exactly two DI dependencies: {injects:?}");
    }

    #[test]
    fn plain_data_class_emits_no_injects() {
        // No stereotype, no @Autowired → not a DI consumer, no INJECTS noise.
        let source = r#"
package com.example;

class Point {
    private final int x;
    public Point(Helper helper, int x) { this.x = x; }
}
"#;
        let fp = parse_file(source, "Point.java", "com::example::Point", repo()).unwrap();
        assert!(
            !fp.refs.iter().any(|r| r.category == edge_category::INJECTS),
            "plain data class must not emit INJECTS refs"
        );
    }

    #[test]
    fn bare_intra_class_call_is_self_method() {
        // java-spring-calls fixture: `compute()` calls `helper()` with no
        // receiver. An unqualified Java call is an implicit `this.helper()`, so
        // the parser must emit `SelfMethod("helper")` (not `Bare`) — only
        // `SelfMethod` resolves against the enclosing class's methods in the
        // graph's resolve_calls (a class method is not a module top-level def).
        let source = r#"
package com.example;

public class App {
    public int compute(int x) {
        return helper(x) + 1;
    }

    public int helper(int x) {
        return x * 2;
    }
}
"#;
        let fp = parse_file(source, "App.java", "com::example::App", repo()).unwrap();
        let compute_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::METHOD,
            "com::example::App::compute",
        );
        assert!(
            fp.calls.iter().any(|c| c.from == compute_id
                && c.qualifier == CallQualifier::SelfMethod("helper".to_string())),
            "expected SelfMethod(\"helper\") CallSite from compute(): {:?}",
            fp.calls
        );
        // A bare call must not be emitted as Bare (would not resolve to the
        // sibling class method).
        assert!(
            !fp.calls
                .iter()
                .any(|c| c.qualifier == CallQualifier::Bare("helper".to_string())),
            "bare intra-class call must be SelfMethod, not Bare"
        );
    }

    #[test]
    fn this_calls() {
        let source = r#"
package com.example;

public class Service {
    public void handle() {
        this.validate();
        helper.process();
    }
    private void validate() {}
}
"#;
        let fp = parse_file(source, "src/main/java/Service.java", "com::example::Service", repo()).unwrap();
        let self_calls: Vec<_> = fp
            .calls
            .iter()
            .filter(|c| matches!(&c.qualifier, CallQualifier::SelfMethod(_)))
            .collect();
        assert_eq!(self_calls.len(), 1);
        let attr_calls: Vec<_> = fp
            .calls
            .iter()
            .filter(|c| matches!(&c.qualifier, CallQualifier::Attribute { .. }))
            .collect();
        assert_eq!(attr_calls.len(), 1);
    }
}
