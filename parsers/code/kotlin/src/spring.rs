//! Spring / Micronaut / JAX-RS / JPA annotation needles on Kotlin (A14.4).
//!
//! Until A14.2's routing flip, `.kt` files went through the Java parser, and a
//! Java-parseable Kotlin class header (`@RestController class X {`) got its
//! annotation routes, stereotype DI and `@Entity` projection from there. This
//! module restores all of it on the Kotlin AST and adds the Kotlin-only DI
//! idiom the Java grammar never saw: the primary constructor.
//!
//! Everything a Java declaration and a Kotlin one must agree on comes from
//! code-domain, never from a copy, because the engine builds both languages
//! as ONE graph (the JVM family):
//!
//! - ROUTE names: `endpoint::jvm_route_prefix` + `endpoint::jvm_annotation_routes`,
//!   the A4.4 recipe the Java parser calls too. A class `@RequestMapping("/api")`
//!   is an `ANY /api` route AND the prefix every action method composes onto,
//!   so `@GetMapping("/users")` beneath it is `GET /api/users` in either
//!   language, and a client ENDPOINT pairs with it either way.
//! - DATA_ENTITY ids: `jvm::data_entity_qname` over `jvm::DATA_ENTITY_ANNOTATIONS`
//!   (`@Document` → `nosql` wins over `@Entity` → `sql`), keyed on the model's
//!   simple name, so a Kotlin repository reaches a Java entity and back.
//! - Repository bases: `jvm::REPOSITORY_BASES`.
//! - The DI value-type denylist: `jvm::is_non_injectable_type`, plus
//!   `jvm::is_kotlin_value_type` for `Int` / `Unit` / `List` / ….
//!
//! # What is emitted (parsers extract; the graph crate resolves)
//!
//! - ROUTE `{VERB} {path}` (Confidence::Strong, a ROUTE_METHOD cell) and a
//!   direct HANDLED_BY edge route → the annotated METHOD (or the CLASS, for the
//!   class's own `@RequestMapping` / `@Controller` / `@Path`). The handler is a
//!   node of this very file, so the edge needs no resolution.
//! - INJECTS as an [`UnresolvedRef`] `Bare(TypeName)` from the consuming type:
//!   `resolve_refs` binds it to the real class, in either language. Sources:
//!   every primary- and secondary-constructor parameter of a Spring stereotype
//!   (`@Service` …), of any constructor carrying `@Inject` / `@Autowired` /
//!   `@Resource` (`class Repo @Inject constructor(private val api: Api)`), and
//!   every property carrying one of those (`@Autowired lateinit var x: X`).
//!   The gate is strict: a `data class` has a primary constructor too, and is
//!   no DI container.
//! - DATA_ENTITY `data_entity:<flavor>:<Name>` (+ DEFINES class → entity) for an
//!   `@Entity` / `@Document` class, and a Medium ACCESSES_DATA edge from a type
//!   whose supertypes name a Spring Data base (`: JpaRepository<User, Long>`)
//!   to the entity's id — direct, like the Java parser, because `resolve_refs`
//!   has no ACCESSES_DATA fallback and the id is name-derived.
//!
//! # Grammar shapes (tree-sitter-kotlin-ng 1.1.0, measured)
//!
//! - A declaration's annotations sit in its `modifiers` child as `annotation`
//!   nodes: `annotation > user_type` for a marker (`@Service`),
//!   `annotation > constructor_invocation > [user_type, value_arguments]` with
//!   arguments (`@GetMapping("/x")`), an optional leading `use_site_target`
//!   (`@field:Inject`), and several parts in one node for `@[A B("x")]`.
//! - `value_argument` is `[identifier, '=', expr]` when named, else `[expr]`;
//!   an array argument is a `collection_literal` (`path = ["/a", "/b"]`).
//! - `primary_constructor > [modifiers?, class_parameters > class_parameter*]`;
//!   a `class_parameter` is `[modifiers?, val|var?, identifier, type, default?]`.
//!   `secondary_constructor > [modifiers?, function_value_parameters > parameter*]`.
//! - Supertypes: `delegation_specifiers > delegation_specifier > user_type`
//!   (an interface, with `type_arguments`) or `> constructor_invocation >
//!   user_type` (a superclass call).
//! - Trap: an annotated `object` declared straight after a class with NO body
//!   (`class A @Inject constructor(..)` then `@Path("/r") object R {}`) parses
//!   the annotation as a detached `annotated_expression`; the object then has
//!   no `modifiers` and its prefix is lost. With a body (`{}`) on the class, or
//!   for a `class` instead of an `object`, the annotations attach normally.
//!
//! # fired_on
//!
//! `[kotlin/spring] stereotypes=S routes=R composed=C injects=J entities=E repos=P repo=<label>`,
//! printed once per repo holding Kotlin by [`crate::trace`]:
//! `glia analyze <repo> 2>&1 | grep '\[kotlin/spring\]'`. The counts are what
//! this module's detectors did while THIS process parsed (process-global
//! counters, taken by the marker, like `di_stats`' shape tokens): a file served
//! from the engine's parse cache ran no detector and is not counted, so a gap
//! on a warm cache means "cached", not "broken". The INJECTS refs also reach
//! the shared `[di]` line as ` kotlin=N` with `kotlin-ctor` / `kotlin-field`
//! shape tokens.

use std::sync::atomic::{AtomicUsize, Ordering};

use glia_code_domain::di_stats::{self, DiShape};
use glia_code_domain::{CallQualifier, UnresolvedRef, endpoint, jvm};
use glia_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId};
use tree_sitter::Node as TsNode;

use crate::{
    Acc, File, GRAPH_TYPE, cell_type, edge_category, line_at, named_child_of_kind, node_kind,
    text_of,
};

/// Stereotypes that make a type a DI-managed bean, as simple names — the Java
/// parser's `SPRING_STEREOTYPES`, plus Hilt's two class-level entry markers
/// (A14.6): a `@HiltViewModel` / `@AndroidEntryPoint` class is built by the
/// Hilt graph, so its constructor parameters are injected even without an
/// explicit `@Inject constructor`.
const SPRING_STEREOTYPES: &[&str] = &[
    "Service",
    "Component",
    "RestController",
    "Controller",
    "Repository",
    "Configuration",
    "HiltViewModel",
    "AndroidEntryPoint",
];

/// Constructor / property annotations that request injection — the Java
/// parser's `INJECT_ANNOTATIONS` (Spring, JSR-330, JSR-250).
const INJECT_ANNOTATIONS: &[&str] = &["Autowired", "Inject", "Resource"];

/// Named `value_argument`s that carry a route path.
const PATH_KEYS: &[&str] = &["value", "path", "uri", "uris"];

/// What one file's Spring pass did, for the `[kotlin/spring]` marker.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SpringCounts {
    /// Types carrying a Spring stereotype.
    pub(crate) stereotypes: usize,
    /// Annotation ROUTEs emitted (one per HANDLED_BY edge).
    pub(crate) routes: usize,
    /// Of `routes`, the method routes composed onto a class prefix.
    pub(crate) composed: usize,
    /// INJECTS refs pushed.
    pub(crate) injects: usize,
    /// DATA_ENTITY nodes projected from `@Entity` / `@Document` classes.
    pub(crate) entities: usize,
    /// ACCESSES_DATA edges from Spring Data repositories.
    pub(crate) repos: usize,
}

impl SpringCounts {
    fn as_array(self) -> [usize; 6] {
        [
            self.stereotypes,
            self.routes,
            self.composed,
            self.injects,
            self.entities,
            self.repos,
        ]
    }

    fn from_array(a: [usize; 6]) -> Self {
        let [stereotypes, routes, composed, injects, entities, repos] = a;
        Self {
            stereotypes,
            routes,
            composed,
            injects,
            entities,
            repos,
        }
    }
}

/// The process-global bank the marker reads, in [`SpringCounts::as_array`]
/// order. Diagnostics only: nothing here reaches the graph.
static COUNTS: [AtomicUsize; 6] = [const { AtomicUsize::new(0) }; 6];

/// Add one file's counts to the bank (end of `parse_file`).
pub(crate) fn publish(counts: SpringCounts) {
    for (slot, n) in COUNTS.iter().zip(counts.as_array()) {
        if n > 0 {
            slot.fetch_add(n, Ordering::Relaxed);
        }
    }
}

/// Read and zero the bank, so the next repo's line starts clean.
pub(crate) fn take() -> SpringCounts {
    SpringCounts::from_array(COUNTS.each_ref().map(|c| c.swap(0, Ordering::Relaxed)))
}

/// The `[kotlin/spring]` marker line.
pub(crate) fn marker(c: SpringCounts, repo_label: &str) -> String {
    format!(
        "[kotlin/spring] stereotypes={} routes={} composed={} injects={} entities={} repos={} repo={repo_label}",
        c.stereotypes, c.routes, c.composed, c.injects, c.entities, c.repos
    )
}

/// What a type's own annotations make of it, for the members walked after it.
pub(crate) struct TypeSpring {
    /// The route prefix its action methods compose onto (`""` when none).
    pub(crate) prefix: String,
    /// A Spring stereotype: every constructor parameter is injected.
    pub(crate) is_bean: bool,
}

/// Everything a type declaration's own annotations and supertypes emit: its
/// class-level routes, its DATA_ENTITY, its repository ACCESSES_DATA and its
/// primary-constructor INJECTS. Returns what its members need.
pub(crate) fn on_type(node: TsNode, name: &str, id: NodeId, file: &File, acc: &mut Acc) -> TypeSpring {
    let anns = own_annotations(node, file.src);
    let is_bean = anns
        .iter()
        .any(|(n, _)| SPRING_STEREOTYPES.contains(&n.as_str()));
    if is_bean {
        acc.spring.stereotypes += 1;
    }

    // The class's own @RequestMapping / @Controller / @Path is a route of its
    // own (`ANY /api`), handled by the class, with nothing to compose onto.
    for (verb, path) in endpoint::jvm_annotation_routes(&anns, "") {
        emit_route(verb, &path, id, file, acc);
    }

    if let Some(flavor) = jvm::DATA_ENTITY_ANNOTATIONS
        .iter()
        .find(|(ann, _)| anns.iter().any(|(n, _)| n == ann))
        .map(|(_, flavor)| *flavor)
    {
        emit_data_entity(flavor, name, id, file, acc);
    }
    emit_repository_access(node, id, file, acc);

    if let Some(ctor) = named_child_of_kind(node, &["primary_constructor"])
        && (is_bean || requests_injection(ctor, file.src))
        && let Some(params) = named_child_of_kind(ctor, &["class_parameters"])
    {
        let mut cursor = params.walk();
        for param in params.named_children(&mut cursor) {
            if param.kind() == "class_parameter" {
                inject_from_param(param, id, DiShape::KotlinCtor, file, acc);
            }
        }
    }

    TypeSpring {
        prefix: endpoint::jvm_route_prefix(&anns),
        is_bean,
    }
}

/// A member function's own route annotations, composed onto its type's prefix.
pub(crate) fn on_method(node: TsNode, id: NodeId, prefix: &str, file: &File, acc: &mut Acc) {
    let routes = endpoint::jvm_annotation_routes(&own_annotations(node, file.src), prefix);
    if !prefix.is_empty() {
        acc.spring.composed += routes.len();
    }
    for (verb, path) in routes {
        emit_route(verb, &path, id, file, acc);
    }
}

/// `@Autowired lateinit var repo: UserRepo` → INJECTS from the owning type.
pub(crate) fn on_property(node: TsNode, owner: NodeId, file: &File, acc: &mut Acc) {
    if !requests_injection(node, file.src) {
        return;
    }
    let Some(var) = named_child_of_kind(node, &["variable_declaration"]) else {
        return;
    };
    if let Some(name) = declared_type(var).and_then(|t| injectable_type(t, file.src)) {
        push_inject(owner, name, DiShape::KotlinField, line_at(var), file, acc);
    }
}

/// `constructor(a: Alpha)` inside a bean's body, or any secondary constructor
/// carrying an inject annotation: every parameter is injected.
pub(crate) fn on_secondary_ctor(
    node: TsNode,
    owner: NodeId,
    owner_is_bean: bool,
    file: &File,
    acc: &mut Acc,
) {
    if !(owner_is_bean || requests_injection(node, file.src)) {
        return;
    }
    let Some(params) = named_child_of_kind(node, &["function_value_parameters"]) else {
        return;
    };
    let mut cursor = params.walk();
    for param in params.named_children(&mut cursor) {
        if param.kind() == "parameter" {
            inject_from_param(param, owner, DiShape::KotlinCtor, file, acc);
        }
    }
}

/// Whether `node`'s own annotations request injection.
fn requests_injection(node: TsNode, src: &[u8]) -> bool {
    own_annotations(node, src)
        .iter()
        .any(|(n, _)| INJECT_ANNOTATIONS.contains(&n.as_str()))
}

/// One constructor parameter (`class_parameter` / `parameter`) → INJECTS its
/// bean type, unless it is a value type.
fn inject_from_param(param: TsNode, owner: NodeId, shape: DiShape, file: &File, acc: &mut Acc) {
    if let Some(name) = declared_type(param).and_then(|t| injectable_type(t, file.src)) {
        push_inject(owner, name, shape, line_at(param), file, acc);
    }
}

/// `line` is the injecting declaration's 0-based row (LC.3b).
fn push_inject(
    owner: NodeId,
    type_name: String,
    shape: DiShape,
    line: u32,
    file: &File,
    acc: &mut Acc,
) {
    acc.refs.push(UnresolvedRef {
        from: owner,
        from_module: file.module_id,
        qualifier: CallQualifier::Bare(type_name),
        category: edge_category::INJECTS,
        line,
    });
    acc.spring.injects += 1;
    di_stats::record(shape);
}

/// The declared type of a parameter / property: its first type-shaped child.
fn declared_type(node: TsNode) -> Option<TsNode> {
    named_child_of_kind(
        node,
        &[
            "user_type",
            "nullable_type",
            "function_type",
            "parenthesized_type",
            "not_nullable_type",
        ],
    )
}

/// The simple bean-type name a declared type injects, or `None` for a
/// function type, a value type (`String`, `Int`, …) or a collection. `T?` is
/// its `T`; generics are stripped (`Repo<User>` injects `Repo`, and
/// `List<Handler>` reduces to the denylisted `List`).
fn injectable_type(ty: TsNode, src: &[u8]) -> Option<String> {
    let ty = match ty.kind() {
        "user_type" => ty,
        "nullable_type" => named_child_of_kind(ty, &["user_type"])?,
        _ => return None,
    };
    let name = simple_type_name(ty, src);
    (!name.is_empty() && !jvm::is_non_injectable_type(&name) && !jvm::is_kotlin_value_type(&name))
        .then_some(name)
}

/// The last `identifier` of a `user_type` — `jakarta.persistence.Entity` is
/// `Entity`, `JpaRepository<User, Long>` is `JpaRepository`.
fn simple_type_name(user_type: TsNode, src: &[u8]) -> String {
    let mut cursor = user_type.walk();
    user_type
        .named_children(&mut cursor)
        .filter(|c| c.kind() == "identifier")
        .last()
        .map(|c| text_of(c, src).trim().to_string())
        .unwrap_or_default()
}

/// The annotations attached to THIS declaration (its `modifiers` child only,
/// never its body) as `(simple name, path argument)` — the shape
/// `endpoint::jvm_annotation_routes` reads, and the Java parser's
/// `own_annotations` produces.
pub(crate) fn own_annotations(node: TsNode, src: &[u8]) -> Vec<(String, Option<String>)> {
    own_annotation_parts(node, src)
        .into_iter()
        .map(|p| (p.name, p.args.and_then(|args| annotation_path_arg(args, src))))
        .collect()
}

/// One annotation part attached to a declaration: `@GET("/x")` is name `GET`
/// with its `value_arguments`, `@Service` is name `Service` with none.
pub(crate) struct AnnotationPart<'a> {
    /// The annotation's simple name (`retrofit2.http.GET` is `GET`).
    pub(crate) name: String,
    /// Its `value_arguments`, when it has any.
    pub(crate) args: Option<TsNode<'a>>,
    /// The `annotation` node it belongs to (its source position).
    pub(crate) node: TsNode<'a>,
}

/// The annotation parts of THIS declaration's `modifiers` child, in source
/// order: the nodes [`own_annotations`] reads, for a caller that needs a named
/// argument or a position (A14.6's Retrofit mappings).
pub(crate) fn own_annotation_parts<'a>(node: TsNode<'a>, src: &[u8]) -> Vec<AnnotationPart<'a>> {
    let mut out = Vec::new();
    let Some(mods) = named_child_of_kind(node, &["modifiers"]) else {
        return out;
    };
    let mut cursor = mods.walk();
    for ann in mods.named_children(&mut cursor) {
        if ann.kind() != "annotation" {
            continue;
        }
        // `@[A B("x")]` holds several parts; `use_site_target` is skipped.
        let mut parts = ann.walk();
        for part in ann.named_children(&mut parts) {
            match part.kind() {
                "user_type" => out.push(AnnotationPart {
                    name: simple_type_name(part, src),
                    args: None,
                    node: ann,
                }),
                "constructor_invocation" => {
                    let Some(ty) = named_child_of_kind(part, &["user_type"]) else {
                        continue;
                    };
                    out.push(AnnotationPart {
                        name: simple_type_name(ty, src),
                        args: named_child_of_kind(part, &["value_arguments"]),
                        node: ann,
                    });
                }
                _ => {}
            }
        }
    }
    out
}

/// The string value of the named argument `key` of an annotation's argument
/// list (`method` of `@HTTP(method = "DELETE", path = "x")`), read like a path
/// argument ([`annotation_path_arg`]'s literal / array / template rules).
pub(crate) fn named_string_arg(args: TsNode, key: &str, src: &[u8]) -> Option<String> {
    let mut cursor = args.walk();
    for arg in args.named_children(&mut cursor) {
        if arg.kind() != "value_argument" {
            continue;
        }
        let mut ac = arg.walk();
        let named: Vec<TsNode> = arg.named_children(&mut ac).collect();
        let has_key = {
            let mut kc = arg.walk();
            arg.children(&mut kc).any(|c| !c.is_named() && c.kind() == "=")
        };
        if has_key
            && named.len() >= 2
            && named.first().is_some_and(|k| text_of(*k, src) == key)
            && let Some(value) = named.last()
        {
            return string_value(*value, src);
        }
    }
    None
}

/// The path of an annotation's argument list: a `value` / `path` / `uri` /
/// `uris` named argument first, else the first positional one. Any other
/// named argument (`produces = [..]`, `method = [..]`) is never read, so a
/// media type cannot become a route prefix.
pub(crate) fn annotation_path_arg(args: TsNode, src: &[u8]) -> Option<String> {
    let mut positional = None;
    let mut cursor = args.walk();
    for arg in args.named_children(&mut cursor) {
        if arg.kind() != "value_argument" {
            continue;
        }
        let mut ac = arg.walk();
        let named: Vec<TsNode> = arg.named_children(&mut ac).collect();
        let has_key = {
            let mut kc = arg.walk();
            arg.children(&mut kc).any(|c| !c.is_named() && c.kind() == "=")
        };
        let Some(value) = named.last().copied() else {
            continue;
        };
        if has_key {
            let key = named.first().map(|k| text_of(*k, src)).unwrap_or_default();
            if PATH_KEYS.contains(&key)
                && let Some(path) = string_value(value, src)
            {
                return Some(path);
            }
        } else if positional.is_none() {
            positional = string_value(value, src);
        }
    }
    positional
}

/// A path argument's string: a literal's content, the first literal of an
/// array (`["/a", "/b"]` → `/a`, as the Java parser reads `{"/a", "/b"}`), or —
/// for an expression such as `BASE + "/x"` — the first quoted string in its
/// text, the Java parser's fallback. A template (`"$BASE/x"`) keeps its raw
/// text.
fn string_value(value: TsNode, src: &[u8]) -> Option<String> {
    match value.kind() {
        "string_literal" | "multiline_string_literal" => Some(literal_content(value, src)),
        "collection_literal" => {
            let mut cursor = value.walk();
            let first = value
                .named_children(&mut cursor)
                .find(|c| matches!(c.kind(), "string_literal" | "multiline_string_literal"));
            first.map(|c| literal_content(c, src))
        }
        _ => first_quoted(text_of(value, src)),
    }
}

/// The text between a string literal's quotes.
pub(crate) fn literal_content(lit: TsNode, src: &[u8]) -> String {
    let t = text_of(lit, src);
    let inner = t
        .strip_prefix("\"\"\"")
        .and_then(|r| r.strip_suffix("\"\"\""))
        .or_else(|| t.strip_prefix('"').and_then(|r| r.strip_suffix('"')))
        .unwrap_or(t);
    inner.to_string()
}

/// The first `"…"` in `text`.
fn first_quoted(text: &str) -> Option<String> {
    let start = text.find('"')? + 1;
    let len = text[start..].find('"')?;
    Some(text[start..start + len].to_string())
}

/// One annotation ROUTE: the node once per `METHOD path` (shared with the
/// text scans in `routes.rs`, which run after and so never duplicate it) and
/// a HANDLED_BY edge per handler. Same NodeId, confidence and cell as the
/// Java parser's `emit_route`.
fn emit_route(method: &str, path: &str, handler: NodeId, file: &File, acc: &mut Acc) {
    let route_name = format!("{method} {path}");
    let route_id = NodeId::from_parts(GRAPH_TYPE, file.repo, node_kind::ROUTE, &route_name);
    if acc.routes_seen.insert(route_name.clone()) {
        acc.nodes.push(Node {
            id: route_id,
            repo: file.repo,
            confidence: Confidence::Strong,
            cells: vec![Cell {
                kind: cell_type::ROUTE_METHOD,
                payload: CellPayload::Text(method.to_string()),
            }],
        });
        acc.nav
            .record(route_id, &route_name, &route_name, node_kind::ROUTE, None);
    }
    acc.edges.push(Edge {
        from: route_id,
        to: handler,
        category: edge_category::HANDLED_BY,
        confidence: Confidence::Strong,
        cells: Vec::new(),
    });
    acc.spring.routes += 1;
}

/// The DATA_ENTITY an `@Entity` / `@Document` class projects, and DEFINES
/// class → entity. The Java parser's `emit_data_entity` shape: node name = the
/// class's simple name, one CODE cell holding it, the flavored model-keyed id.
fn emit_data_entity(flavor: &str, name: &str, class_id: NodeId, file: &File, acc: &mut Acc) {
    let qname = jvm::data_entity_qname(flavor, name);
    let entity_id = NodeId::from_parts(GRAPH_TYPE, file.repo, node_kind::DATA_ENTITY, &qname);
    acc.nodes.push(Node {
        id: entity_id,
        repo: file.repo,
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
        cells: Vec::new(),
    });
    acc.nav
        .record(entity_id, name, &qname, node_kind::DATA_ENTITY, Some(class_id));
    acc.spring.entities += 1;
}

/// `interface UserRepository : JpaRepository<User, Long>` → ACCESSES_DATA from
/// the repository to `data_entity:sql:User` (Mongo bases → `nosql`).
fn emit_repository_access(node: TsNode, from: NodeId, file: &File, acc: &mut Acc) {
    let Some(specs) = named_child_of_kind(node, &["delegation_specifiers"]) else {
        return;
    };
    let mut cursor = specs.walk();
    for spec in specs.named_children(&mut cursor) {
        if spec.kind() != "delegation_specifier" {
            continue;
        }
        let base = named_child_of_kind(spec, &["user_type"]).or_else(|| {
            named_child_of_kind(spec, &["constructor_invocation"])
                .and_then(|ci| named_child_of_kind(ci, &["user_type"]))
        });
        let Some(base) = base else {
            continue;
        };
        let Some(flavor) = jvm::repository_flavor(&simple_type_name(base, file.src)) else {
            continue;
        };
        let Some(entity) = first_type_argument(base, file.src) else {
            continue;
        };
        let to = NodeId::from_parts(
            GRAPH_TYPE,
            file.repo,
            node_kind::DATA_ENTITY,
            &jvm::data_entity_qname(flavor, &entity),
        );
        acc.edges.push(Edge {
            from,
            to,
            category: edge_category::ACCESSES_DATA,
            confidence: Confidence::Medium,
            cells: Vec::new(),
        });
        acc.spring.repos += 1;
    }
}

/// The simple name of a `user_type`'s first type argument (`User` of
/// `JpaRepository<com.acme.User, Long>`).
fn first_type_argument(user_type: TsNode, src: &[u8]) -> Option<String> {
    let targs = named_child_of_kind(user_type, &["type_arguments"])?;
    let mut cursor = targs.walk();
    let first = targs
        .named_children(&mut cursor)
        .find(|c| c.kind() == "type_projection")?;
    let ty = named_child_of_kind(first, &["user_type", "nullable_type"])?;
    let ty = if ty.kind() == "nullable_type" {
        named_child_of_kind(ty, &["user_type"])?
    } else {
        ty
    };
    let name = simple_type_name(ty, src);
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FileParse, parse_file};
    use glia_core::RepoId;

    fn repo() -> RepoId {
        RepoId(1)
    }

    fn id_of(fp: &FileParse, kind: glia_core::NodeKindId, qname: &str) -> NodeId {
        let id = NodeId::from_parts(GRAPH_TYPE, repo(), kind, qname);
        assert!(
            fp.nodes.iter().any(|n| n.id == id),
            "no {qname} node; nav: {:?}",
            fp.nav.qname_by_id.values().collect::<Vec<_>>()
        );
        id
    }

    fn route_id(name: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ROUTE, name)
    }

    /// `(route name, handler qname)` of every HANDLED_BY edge, sorted.
    fn handled_by(fp: &FileParse) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = fp
            .edges
            .iter()
            .filter(|e| e.category == edge_category::HANDLED_BY)
            .map(|e| {
                (
                    fp.nav.name_by_id.get(&e.from).cloned().unwrap_or_default(),
                    fp.nav.qname_by_id.get(&e.to).cloned().unwrap_or_default(),
                )
            })
            .collect();
        out.sort();
        out
    }

    /// `(from qname, injected type)` of every INJECTS ref, sorted.
    fn injects(fp: &FileParse) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = fp
            .refs
            .iter()
            .filter(|r| r.category == edge_category::INJECTS)
            .map(|r| {
                let from = fp.nav.qname_by_id.get(&r.from).cloned().unwrap_or_default();
                let CallQualifier::Bare(t) = &r.qualifier else {
                    panic!("INJECTS must be Bare: {:?}", r.qualifier);
                };
                (from, t.clone())
            })
            .collect();
        out.sort();
        out
    }

    fn counts(source: &str) -> SpringCounts {
        crate::parse_counting(source, "x.kt", "x", repo()).unwrap().1
    }

    #[test]
    fn getmapping_on_kotlin_method_emits_route_and_handled_by() {
        let source = r#"
package com.example.web

@RestController
class UserController(private val userService: UserService) {
    @GetMapping("/users")
    fun list(): List<String> = userService.findAll()

    @PostMapping(value = ["/users"], produces = ["application/json"])
    fun create(): String = "ok"

    fun helper() {}
}
"#;
        let fp = parse_file(source, "web.kt", "web", repo()).unwrap();
        assert_eq!(
            handled_by(&fp),
            vec![
                ("GET /users".to_string(), "UserController::list".to_string()),
                ("POST /users".to_string(), "UserController::create".to_string()),
            ]
        );
        // The Java parser's ROUTE: same id, Strong, one ROUTE_METHOD cell.
        let node = fp.nodes.iter().find(|n| n.id == route_id("GET /users")).unwrap();
        assert_eq!(node.confidence, Confidence::Strong);
        assert!(matches!(
            node.cells.as_slice(),
            [Cell { kind, payload: CellPayload::Text(m) }] if *kind == cell_type::ROUTE_METHOD && m == "GET"
        ));
        let c = counts(source);
        assert_eq!((c.stereotypes, c.routes, c.composed, c.injects), (1, 2, 0, 1));
    }

    #[test]
    fn class_request_mapping_is_a_route_and_a_prefix_like_java() {
        // kotlin-flip-guard's web.kt: the class route `ANY /api` handled by the
        // class, and the action route composed onto it, as A4.4 does for Java.
        let source = r#"
package com.acme.web

@RestController
@RequestMapping("/api")
class UserController {
    @GetMapping("/users")
    fun list(): List<String> = listOf()

    @DeleteMapping
    fun purge() {}
}
"#;
        let fp = parse_file(source, "web.kt", "com::acme::web::web", repo()).unwrap();
        assert_eq!(
            handled_by(&fp),
            vec![
                ("ANY /api".to_string(), "com::acme::web::UserController".to_string()),
                ("DELETE /api".to_string(), "com::acme::web::UserController::purge".to_string()),
                ("GET /api/users".to_string(), "com::acme::web::UserController::list".to_string()),
            ]
        );
        let c = counts(source);
        assert_eq!((c.routes, c.composed), (3, 2));
    }

    #[test]
    fn micronaut_get_annotation_route() {
        let source = r#"
package app

@Controller("/hello")
class HelloController {
    @Get("/{name}")
    fun greet(name: String): String = "hi $name"
}

@Path("/items")
class ItemResource {
    @GET
    @Path("/{id}")
    fun one(): String = "x"

    @POST
    fun add() {}
}
"#;
        let fp = parse_file(source, "app/Hello.kt", "app::Hello", repo()).unwrap();
        assert_eq!(
            handled_by(&fp),
            vec![
                ("ANY /hello".to_string(), "app::HelloController".to_string()),
                ("ANY /items".to_string(), "app::ItemResource".to_string()),
                ("GET /hello/{name}".to_string(), "app::HelloController::greet".to_string()),
                ("GET /items/{id}".to_string(), "app::ItemResource::one".to_string()),
                ("POST /items".to_string(), "app::ItemResource::add".to_string()),
            ]
        );
    }

    #[test]
    fn restcontroller_primary_constructor_emits_injects() {
        let source = r#"
package com.example.web

@RestController
class UserController(
    private val userService: UserService,
    private val audit: com.acme.audit.AuditLog?,
    val repo: Repo<User>,
) {
    @GetMapping("/users")
    fun list(): List<String> = userService.findAll()
}

@Service
class UserService(private val repo: UserRepository)
"#;
        let fp = parse_file(source, "web.kt", "web", repo()).unwrap();
        assert_eq!(
            injects(&fp),
            vec![
                ("UserController".to_string(), "AuditLog".to_string()),
                ("UserController".to_string(), "Repo".to_string()),
                ("UserController".to_string(), "UserService".to_string()),
                ("UserService".to_string(), "UserRepository".to_string()),
            ]
        );
        // Every ref names its file's MODULE for import-aware resolution.
        let module = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "web");
        assert!(fp.refs.iter().all(|r| r.from_module == module));
    }

    #[test]
    fn inject_constructor_without_stereotype_still_injects() {
        let source = r#"
package p

class Repo @Inject constructor(private val api: UserApi, private val clock: Clock) {
    fun all() = api.list()
}

class Plain(private val api: UserApi)

data class UserDto(val id: Long, val owner: Owner)

class Legacy {
    @Autowired
    constructor(api: UserApi, n: Int)
}
"#;
        let fp = parse_file(source, "p/Repo.kt", "p::Repo", repo()).unwrap();
        // The @Inject primary constructor and the @Autowired secondary one
        // inject; a plain class and a data class are no DI containers.
        assert_eq!(
            injects(&fp),
            vec![
                ("p::Legacy".to_string(), "UserApi".to_string()),
                ("p::Repo".to_string(), "Clock".to_string()),
                ("p::Repo".to_string(), "UserApi".to_string()),
            ]
        );
        assert_eq!(counts(source).stereotypes, 0);
    }

    #[test]
    fn value_type_ctor_params_are_not_injected() {
        let source = r#"
@Service
class Svc(
    private val name: String,
    private val n: Int,
    val flags: List<String>,
    val lookup: Map<String, Long>,
    val onDone: () -> Unit,
    val count: Long?,
    val repo: UserRepository,
) {
    constructor(n: Int, other: Other) : this("x", n, listOf(), mapOf(), {}, null, other.repo)
}
"#;
        let fp = parse_file(source, "svc.kt", "svc", repo()).unwrap();
        assert_eq!(
            injects(&fp),
            vec![
                ("Svc".to_string(), "Other".to_string()),
                ("Svc".to_string(), "UserRepository".to_string()),
            ]
        );
    }

    #[test]
    fn autowired_property_injects() {
        let source = r#"
@Component
class Jobs {
    @Autowired
    lateinit var repo: com.acme.JobRepository

    @field:Inject
    lateinit var clock: Clock

    @Autowired
    private lateinit var name: String

    lateinit var notInjected: Mailer
}

class NotABean {
    @Resource
    private var mailer: Mailer? = null
}
"#;
        let fp = parse_file(source, "jobs.kt", "jobs", repo()).unwrap();
        assert_eq!(
            injects(&fp),
            vec![
                ("Jobs".to_string(), "Clock".to_string()),
                ("Jobs".to_string(), "JobRepository".to_string()),
                ("NotABean".to_string(), "Mailer".to_string()),
            ]
        );
    }

    #[test]
    fn kotlin_jpa_entity_and_repository_share_one_id() {
        let model = r#"
package com.acme.model

@Entity
class User {
    @Id
    var id: Long? = null
}

@Document(collection = "events")
@Entity
data class Event(val id: String)
"#;
        let repo_src = r#"
package com.acme.repo

interface UserRepository : JpaRepository<com.acme.model.User, Long>

interface EventRepository : MongoRepository<Event, String>, Custom

abstract class Impl : BaseThing(), CoroutineCrudRepository<User, Long>

interface NotARepo : Comparable<User>
"#;
        let m = parse_file(model, "model/User.kt", "model::User", repo()).unwrap();
        let r = parse_file(repo_src, "repo/Repos.kt", "repo::Repos", repo()).unwrap();
        // `@Document` wins over `@Entity`; the qname is A13.2's flavored,
        // model-keyed one, the same string the Java parser mints.
        let user = id_of(&m, node_kind::DATA_ENTITY, "data_entity:sql:User");
        let event = id_of(&m, node_kind::DATA_ENTITY, "data_entity:nosql:Event");
        assert_eq!(m.nav.name_by_id.get(&user).map(String::as_str), Some("User"));
        let class = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, "model::User");
        assert!(m.edges.iter().any(|e| e.from == class
            && e.to == user
            && e.category == edge_category::DEFINES));
        let mut access: Vec<(String, NodeId)> = r
            .edges
            .iter()
            .filter(|e| e.category == edge_category::ACCESSES_DATA)
            .map(|e| (r.nav.qname_by_id.get(&e.from).cloned().unwrap_or_default(), e.to))
            .collect();
        access.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            access,
            vec![
                ("repo::EventRepository".to_string(), event),
                ("repo::Impl".to_string(), user),
                ("repo::UserRepository".to_string(), user),
            ]
        );
        assert!(
            r.edges
                .iter()
                .filter(|e| e.category == edge_category::ACCESSES_DATA)
                .all(|e| e.confidence == Confidence::Medium)
        );
        let (mc, rc) = (counts(model), counts(repo_src));
        assert_eq!((mc.entities, mc.repos, rc.entities, rc.repos), (2, 0, 0, 3));
        // An entity class is not a bean: no stereotype, no INJECTS.
        assert!(m.refs.is_empty());
    }

    #[test]
    fn annotation_route_suppresses_the_same_text_scan_route() {
        // A path the Javalin scan also matches stays ONE node — the annotation
        // one, with its handler.
        let source = r#"
@RestController
class C {
    @GetMapping("/users")
    fun list() = app.get("/users", handler)
}
"#;
        let fp = parse_file(source, "c.kt", "c", repo()).unwrap();
        let id = route_id("GET /users");
        assert_eq!(fp.nodes.iter().filter(|n| n.id == id).count(), 1);
        let node = fp.nodes.iter().find(|n| n.id == id).unwrap();
        assert_eq!(node.confidence, Confidence::Strong);
    }

    #[test]
    fn marker_line_shape() {
        let c = SpringCounts {
            stereotypes: 2,
            routes: 1,
            composed: 0,
            injects: 1,
            entities: 0,
            repos: 0,
        };
        assert_eq!(
            marker(c, "fixtures/kotlin-spring"),
            "[kotlin/spring] stereotypes=2 routes=1 composed=0 injects=1 entities=0 repos=0 repo=fixtures/kotlin-spring"
        );
        assert_eq!(SpringCounts::from_array(c.as_array()), c);
    }
}
