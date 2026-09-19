//! Kotlin client ENDPOINTs and Android components (A14.6).
//!
//! - **Retrofit interfaces.** `interface UserApi { @GET("users") suspend fun
//!   listUsers(): List<User> }` declares the requests the app SENDS: each
//!   verb annotation on a body-less INTERFACE method is a client ENDPOINT
//!   (+ CALLS from the method), minted through
//!   `code_domain::endpoint::push_client_endpoint_with` — the one qname
//!   recipe (`endpoint:{METHOD}:{path}`) the HttpStackResolver indexes, so
//!   `listUsers -> GET /api/users` pairs with the server's route across
//!   repos. Ktorfit's `@GET("x")` interfaces read the same.
//! - **Spring clients** (`restTemplate.getForObject("/x", …)`,
//!   `webClient.post().uri("/x")`) are detected by the call walker
//!   ([`crate::calls`]); they share this module's ENDPOINT sink
//!   ([`emit_client_endpoint`]) and URL reconstruction ([`client_url`]).
//! - **Android components.** A class whose superclass is an Android framework
//!   entry type (`AppCompatActivity`, `Fragment`, `ViewModel`, `Service`, …)
//!   in a file importing `android.*` / `androidx.*`, or that carries a Hilt
//!   entry marker (`@AndroidEntryPoint`, `@HiltViewModel`, `@HiltAndroidApp`),
//!   is started by the framework, not by a call: it carries a ROLE cell
//!   `{"roles":["COMPONENT"]}` on its CLASS node — the LB.3a canonical shape
//!   (never a same-qname COMPONENT twin), which `roles_in` reads and the
//!   engine's liveness treats as an entrypoint, so an Activity and everything
//!   it reaches is no longer flagged dead.
//!
//! # Retrofit vs JAX-RS
//!
//! `@GET` means opposite things in the two frameworks: JAX-RS marks a SERVER
//! method (its path is a separate `@Path`), Retrofit a CLIENT one (its path is
//! the annotation's own argument). A method is a Retrofit mapping only when
//! ALL hold: its type is an INTERFACE, it has no `function_body`, it carries
//! no `@Path` of its own (Retrofit's `@Path("id")` is a PARAMETER annotation,
//! in `parameter_modifiers`, never among the function's `modifiers`), and the
//! verb annotation has a string argument (`@GET("x")`; a JAX-RS `@GET` has
//! none, and a Retrofit `@GET` with no path is a dynamic `@Url` call with
//! nothing to name). `@HTTP(method = "DELETE", path = "x")` counts too. A
//! Retrofit method mints no ROUTE ([`crate::spring::on_method`] is skipped
//! for it), so a client interface never pairs with itself.
//!
//! # Paths
//!
//! A Retrofit template is relative to the builder's `baseUrl` (`@GET("users")`
//! requests `<base>/users`), so it gains its leading `/` here — the
//! HttpStackResolver can never pair `users` with `/users`. The query is
//! dropped (`users?sort=desc` is `/users`); `{id}` templates are kept for the
//! resolver's placeholder fold. An absolute template (`@GET("https://x/y")`)
//! replaces the base: its literal authority rides on ENDPOINT_HIT as `host`.
//! A Kotlin string template (`"$BASE/users"`, `"/u/${id}"`) has each
//! interpolation rewritten to `${…}`, the convention every client parser uses,
//! and is Medium rather than Strong. The builder's own `baseUrl(…)` is not
//! read: nothing ties a `Retrofit.Builder` to the interface it `create`s
//! without types.
//!
//! # Not covered
//!
//! Classes that extend an app's own base (`class Main : BaseActivity()`), since
//! the base is matched by name, not by resolved hierarchy; WorkManager workers
//! and `@Composable` functions; OkHttp and Ktor-client calls.
//!
//! # fired_on
//!
//! `[kotlin/retrofit] endpoints=E components=C spring_clients=K repo=<label>`,
//! once per repo holding Kotlin, from [`crate::trace`]:
//! `glia analyze <repo> 2>&1 | grep '\[kotlin/retrofit\]'`. `E` counts
//! Retrofit ENDPOINT emissions (one per mapping), `C` Android component
//! classes, `K` Spring client ENDPOINT emissions. Like the `[kotlin/spring]`
//! line these are what the detectors did while THIS process parsed: a file
//! served from the engine's parse cache is not counted.

use std::sync::atomic::{AtomicUsize, Ordering};

use glia_code_domain::endpoint::{self, ClientEndpoint, HitExtras};
use glia_core::{Cell, CellPayload, Confidence, NodeId};
use tree_sitter::Node as TsNode;

use crate::{
    Acc, File, ImportTarget, cell_type, named_child_of_kind, node_kind, spring, text_of,
};

/// Android framework types a component class extends, as simple names. All
/// are classes the framework instantiates; each counts only in a file that
/// imports `android.*` / `androidx.*` (an in-house `Service()` base elsewhere
/// is not Android's).
const ANDROID_BASES: &[&str] = &[
    "Activity",
    "AppCompatActivity",
    "ComponentActivity",
    "FragmentActivity",
    "Fragment",
    "DialogFragment",
    "ViewModel",
    "AndroidViewModel",
    "Service",
    "BroadcastReceiver",
    "ContentProvider",
    "Application",
];

/// Hilt annotations that mark a class as a framework entry point, anywhere.
const HILT_ENTRY_ANNOTATIONS: &[&str] = &["AndroidEntryPoint", "HiltViewModel", "HiltAndroidApp"];

/// What one file's client / component detectors did, for the
/// `[kotlin/retrofit]` marker.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ClientCounts {
    /// Retrofit ENDPOINT emissions (one per mapping, with its CALLS edge).
    pub(crate) endpoints: usize,
    /// Classes classified as Android components.
    pub(crate) components: usize,
    /// Spring RestTemplate / WebClient ENDPOINT emissions ([`crate::calls`]).
    pub(crate) spring_clients: usize,
}

/// The process-global bank the marker reads: endpoints, components,
/// spring_clients. Diagnostics only: nothing here reaches the graph.
static COUNTS: [AtomicUsize; 3] = [const { AtomicUsize::new(0) }; 3];

/// Add one file's counts to the bank (end of `parse_file`).
pub(crate) fn publish(c: ClientCounts) {
    for (slot, n) in COUNTS
        .iter()
        .zip([c.endpoints, c.components, c.spring_clients])
    {
        if n > 0 {
            slot.fetch_add(n, Ordering::Relaxed);
        }
    }
}

/// Read and zero the bank, so the next repo's line starts clean.
pub(crate) fn take() -> ClientCounts {
    let [endpoints, components, spring_clients] =
        COUNTS.each_ref().map(|c| c.swap(0, Ordering::Relaxed));
    ClientCounts {
        endpoints,
        components,
        spring_clients,
    }
}

/// The `[kotlin/retrofit]` marker line.
pub(crate) fn marker(c: ClientCounts, repo_label: &str) -> String {
    format!(
        "[kotlin/retrofit] endpoints={} components={} spring_clients={} repo={repo_label}",
        c.endpoints, c.components, c.spring_clients
    )
}

/// A member function of an INTERFACE: when it is a Retrofit mapping (see the
/// module doc for the gate), emit one ENDPOINT per verb annotation with a
/// CALLS edge from the method `from`, and return `true` so the caller mints
/// no ROUTE for it. `false` for anything else.
pub(crate) fn on_interface_fn(fun: TsNode, from: NodeId, file: &File, acc: &mut Acc) -> bool {
    let mappings = retrofit_mappings(fun, file.src);
    if mappings.is_empty() {
        return false;
    }
    for (verb, tmpl, ann) in mappings {
        let (tmpl, templated) = placeholders(&tmpl);
        let (host, path) = if tmpl.contains("://") {
            match endpoint::client_url_split(&tmpl) {
                (host, Some(path)) => (host, path),
                (_, None) => continue,
            }
        } else {
            (None, base_relative_path(&tmpl))
        };
        emit_client_endpoint(ann, verb, path, host.as_deref(), !templated, from, file, acc);
        acc.clients.endpoints += 1;
    }
    true
}

/// `(verb, template, annotation)` for every Retrofit request mapping on a
/// function, or none when it fails the Retrofit gate.
fn retrofit_mappings<'a>(fun: TsNode<'a>, src: &[u8]) -> Vec<(&'static str, String, TsNode<'a>)> {
    if named_child_of_kind(fun, &["function_body"]).is_some() {
        return Vec::new();
    }
    let parts = spring::own_annotation_parts(fun, src);
    if parts.iter().any(|p| p.name == "Path") {
        return Vec::new();
    }
    let mut out = Vec::new();
    for part in &parts {
        let Some(args) = part.args else {
            continue;
        };
        let mapping = if part.name == "HTTP" {
            spring::named_string_arg(args, "method", src)
                .and_then(|m| endpoint::jaxrs_verb(&m.to_ascii_uppercase()))
                .zip(spring::named_string_arg(args, "path", src))
        } else {
            endpoint::jaxrs_verb(&part.name).zip(spring::annotation_path_arg(args, src))
        };
        if let Some((verb, tmpl)) = mapping.filter(|(_, t)| !t.trim().is_empty()) {
            out.push((verb, tmpl, part.node));
        }
    }
    out
}

/// A base-URL-relative Retrofit template as an absolute request path: the
/// query dropped, a leading `/` added. A `${…}` base placeholder is kept in
/// front, the shape the resolver's BaseFold tier reads.
fn base_relative_path(tmpl: &str) -> String {
    let (path, _) = endpoint::normalise_client_path(tmpl.trim());
    if path.starts_with("${") {
        path
    } else {
        endpoint::abs_path(&path)
    }
}

/// A class declaration's Android component classification: a ROLE COMPONENT
/// cell on its CLASS node `id` when it extends an [`ANDROID_BASES`] type in a
/// file importing Android, or carries a [`HILT_ENTRY_ANNOTATIONS`] marker.
/// An interface, enum or `object` is never a component.
pub(crate) fn on_type(
    node: TsNode,
    kind: glia_core::NodeKindId,
    id: NodeId,
    file: &File,
    acc: &mut Acc,
) {
    if kind != node_kind::CLASS || node.kind() != "class_declaration" {
        return;
    }
    let hilt = spring::own_annotations(node, file.src)
        .iter()
        .any(|(n, _)| HILT_ENTRY_ANNOTATIONS.contains(&n.as_str()));
    let is_component = hilt || (imports_android(acc) && extends_android_base(node, file.src));
    if !is_component {
        return;
    }
    let Some(class) = acc.nodes.iter_mut().rev().find(|n| n.id == id) else {
        return;
    };
    if class.cells.iter().any(|c| c.kind == cell_type::ROLE) {
        return;
    }
    class.cells.push(component_role_cell());
    acc.clients.components += 1;
}

/// The LB.3a ROLE cell naming the COMPONENT role, in `roles_in`'s payload
/// shape: `{"roles":["COMPONENT"]}`.
fn component_role_cell() -> Cell {
    Cell {
        kind: cell_type::ROLE,
        payload: CellPayload::Json(format!(
            "{{\"roles\":[\"{}\"]}}",
            node_kind::name(node_kind::COMPONENT)
        )),
    }
}

/// Whether the file imports anything under `android` / `androidx`. Imports
/// precede every declaration in a Kotlin file, so they are all collected by
/// the time a class is visited.
fn imports_android(acc: &Acc) -> bool {
    acc.imports.iter().any(|i| {
        let path = match &i.target {
            ImportTarget::Symbol { module, .. } => module.as_str(),
            ImportTarget::Module { path, .. } => path.as_str(),
        };
        matches!(path.split("::").next(), Some("android" | "androidx"))
    })
}

/// Whether a supertype of `node` is named in [`ANDROID_BASES`] — a superclass
/// call (`: AppCompatActivity()`) or a call-less supertype.
fn extends_android_base(node: TsNode, src: &[u8]) -> bool {
    let Some(specs) = named_child_of_kind(node, &["delegation_specifiers"]) else {
        return false;
    };
    let mut cursor = specs.walk();
    specs
        .named_children(&mut cursor)
        .filter(|s| s.kind() == "delegation_specifier")
        .filter_map(|s| {
            named_child_of_kind(s, &["user_type"]).or_else(|| {
                named_child_of_kind(s, &["constructor_invocation"])
                    .and_then(|ci| named_child_of_kind(ci, &["user_type"]))
            })
        })
        .any(|ty| {
            let mut c = ty.walk();
            ty.named_children(&mut c)
                .filter(|p| p.kind() == "identifier")
                .last()
                .is_some_and(|name| ANDROID_BASES.contains(&text_of(name, src)))
        })
}

/// Emit one client ENDPOINT (+ CALLS from `from`) positioned at `site`, the
/// node that declared or made the request, through the shared code-domain
/// sink. `line` is 1-based (`row + 1`) and `col` 1-based, like every other
/// client parser's ENDPOINT_HIT.
#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_client_endpoint(
    site: TsNode,
    method: &str,
    path: String,
    host: Option<&str>,
    strong: bool,
    from: NodeId,
    file: &File,
    acc: &mut Acc,
) {
    let pos = site.start_position();
    let ep = ClientEndpoint {
        method: method.to_string(),
        path,
        file: file.rel.to_string(),
        line: pos.row + 1,
        col: pos.column + 1,
        confidence: if strong {
            Confidence::Strong
        } else {
            Confidence::Medium
        },
    };
    let extras = HitExtras {
        host,
        ..HitExtras::default()
    };
    endpoint::push_client_endpoint_with(
        file.repo,
        &ep,
        extras,
        from,
        &mut acc.nodes,
        &mut acc.edges,
        &mut acc.nav,
        &mut acc.seen_endpoints,
    );
}

/// The URL a client call's argument spells, as `(url, strong)`: a string
/// literal (Strong unless it interpolates), or a `+` concatenation with a
/// literal part (Medium), every non-literal operand and interpolation
/// becoming `${…}`. `None` for anything else (a variable, a call).
pub(crate) fn client_url(arg: TsNode, src: &[u8]) -> Option<(String, bool)> {
    match arg.kind() {
        "string_literal" | "multiline_string_literal" => {
            let (url, templated) = placeholders(&spring::literal_content(arg, src));
            Some((url, !templated))
        }
        "binary_expression" => {
            let mut out = String::new();
            let mut saw_literal = false;
            concat_parts(arg, src, &mut out, &mut saw_literal)?;
            saw_literal.then_some((out, false))
        }
        "parenthesized_expression" => client_url(arg.named_child(0)?, src),
        _ => None,
    }
}

/// Flatten a `+` chain (left-nested `binary_expression`s) into `out`. `None`
/// when an operator is not `+`.
fn concat_parts(node: TsNode, src: &[u8], out: &mut String, saw_literal: &mut bool) -> Option<()> {
    match node.kind() {
        "binary_expression" => {
            let mut cursor = node.walk();
            let op_is_plus = node
                .children(&mut cursor)
                .any(|c| !c.is_named() && c.kind() == "+");
            if !op_is_plus {
                return None;
            }
            let mut cursor = node.walk();
            let operands: Vec<TsNode> = node.named_children(&mut cursor).collect();
            for operand in operands {
                concat_parts(operand, src, out, saw_literal)?;
            }
        }
        "string_literal" => {
            out.push_str(&placeholders(&spring::literal_content(node, src)).0);
            *saw_literal = true;
        }
        _ => out.push_str("${…}"),
    }
    Some(())
}

/// Rewrite each Kotlin string-template interpolation (`$name`, `${expr}`) in
/// a literal's content to `${…}`. Returns the text and whether anything was
/// rewritten. `\$` is a literal dollar.
fn placeholders(content: &str) -> (String, bool) {
    let mut out = String::with_capacity(content.len());
    let mut changed = false;
    let mut chars = content.char_indices().peekable();
    while let Some((_, c)) = chars.next() {
        match c {
            '\\' if chars.peek().is_some_and(|(_, n)| *n == '$') => {
                chars.next();
                out.push('$');
            }
            '$' if chars.peek().is_some_and(|(_, n)| *n == '{') => {
                let mut depth = 0usize;
                for (_, n) in chars.by_ref() {
                    match n {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                out.push_str("${…}");
                changed = true;
            }
            '$' if chars
                .peek()
                .is_some_and(|(_, n)| n.is_alphabetic() || *n == '_') =>
            {
                while chars
                    .peek()
                    .is_some_and(|(_, n)| n.is_alphanumeric() || *n == '_')
                {
                    chars.next();
                }
                out.push_str("${…}");
                changed = true;
            }
            _ => out.push(c),
        }
    }
    (out, changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FileParse, edge_category, parse_file};
    use glia_core::RepoId;

    fn repo() -> RepoId {
        RepoId(1)
    }

    fn parse(source: &str) -> FileParse {
        parse_file(source, "api.kt", "api", repo()).unwrap()
    }

    /// `(endpoint qname, caller qname)` of every CALLS edge into an ENDPOINT,
    /// sorted.
    fn endpoint_calls(fp: &FileParse) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = fp
            .edges
            .iter()
            .filter(|e| e.category == edge_category::CALLS)
            .filter(|e| fp.nav.kind_by_id.get(&e.to) == Some(&node_kind::ENDPOINT))
            .map(|e| {
                (
                    fp.nav.qname_by_id.get(&e.to).cloned().unwrap_or_default(),
                    fp.nav.qname_by_id.get(&e.from).cloned().unwrap_or_default(),
                )
            })
            .collect();
        out.sort();
        out
    }

    fn hit_json(fp: &FileParse, qname: &str) -> serde_json::Value {
        let id = NodeId::from_parts(crate::GRAPH_TYPE, repo(), node_kind::ENDPOINT, qname);
        let node = fp.nodes.iter().find(|n| n.id == id).expect("endpoint node");
        let Some(Cell {
            payload: CellPayload::Json(s),
            ..
        }) = node.cells.iter().find(|c| c.kind == cell_type::ENDPOINT_HIT)
        else {
            panic!("no ENDPOINT_HIT on {qname}");
        };
        serde_json::from_str(s).unwrap()
    }

    fn roles_of(fp: &FileParse, qname: &str) -> Vec<String> {
        let id = NodeId::from_parts(crate::GRAPH_TYPE, repo(), node_kind::CLASS, qname);
        let node = fp.nodes.iter().find(|n| n.id == id).expect("class node");
        node.cells
            .iter()
            .filter(|c| c.kind == cell_type::ROLE)
            .map(|c| match &c.payload {
                CellPayload::Json(s) | CellPayload::Text(s) => s.clone(),
                CellPayload::Bytes(_) => String::new(),
            })
            .collect()
    }

    #[test]
    fn retrofit_get_on_interface_emits_endpoint() {
        // The kotlin-retrofit fixture's client: two mappings, each an
        // ENDPOINT called by its interface method, and no ROUTE at all.
        let fp = parse(
            r#"
package com.acme.client

import retrofit2.http.GET
import retrofit2.http.POST

interface UserApi {
    @GET("/api/users")
    suspend fun listUsers(): List<String>

    @POST("/api/users")
    suspend fun createUser(name: String): String

    @HTTP(method = "DELETE", path = "/api/users/{id}", hasBody = true)
    fun purge(@Path("id") id: Long): Call<Unit>
}
"#,
        );
        assert_eq!(
            endpoint_calls(&fp),
            vec![
                ("endpoint:DELETE:/api/users/{id}".to_string(), "UserApi::purge".to_string()),
                ("endpoint:GET:/api/users".to_string(), "UserApi::listUsers".to_string()),
                ("endpoint:POST:/api/users".to_string(), "UserApi::createUser".to_string()),
            ]
        );
        assert!(
            !fp.nav.kind_by_id.values().any(|k| *k == node_kind::ROUTE),
            "a client interface mints no ROUTE"
        );
        // ENDPOINT_HIT: 1-based line of the annotation, Strong, no raw / host.
        let hit = hit_json(&fp, "endpoint:GET:/api/users");
        assert_eq!(hit["line"], 8);
        assert_eq!(hit["col"], 5);
        assert_eq!(hit["confidence"], "strong");
        assert_eq!(hit["file"], "api.kt");
        assert!(hit.get("raw").is_none() && hit.get("host").is_none(), "{hit}");
        let clients = crate::parse_all(
            "interface A {\n    @GET(\"/a\")\n    fun a(): String\n}\n",
            "a.kt",
            "a",
            repo(),
        )
        .unwrap()
        .clients;
        assert_eq!(clients, ClientCounts { endpoints: 1, ..ClientCounts::default() });
    }

    #[test]
    fn retrofit_relative_path_gets_leading_slash() {
        let fp = parse(
            r#"
interface Api {
    @GET("users/{id}?expand=true")
    fun user(@Path("id") id: Long): User

    @GET(value = "https://api.example.com/v2/orders")
    fun orders(): List<Order>

    @PUT("$BASE/items")
    fun put(): Unit

    @GET
    fun dynamic(@Url url: String): String
}
"#,
        );
        assert_eq!(
            endpoint_calls(&fp),
            vec![
                ("endpoint:GET:/users/{id}".to_string(), "Api::user".to_string()),
                ("endpoint:GET:/v2/orders".to_string(), "Api::orders".to_string()),
                ("endpoint:PUT:${…}/items".to_string(), "Api::put".to_string()),
            ]
        );
        // Relative: normalised before the ENDPOINT is built, so no `raw`.
        let user = hit_json(&fp, "endpoint:GET:/users/{id}");
        assert!(user.get("raw").is_none(), "{user}");
        // Absolute: the literal authority rides as `host`.
        assert_eq!(hit_json(&fp, "endpoint:GET:/v2/orders")["host"], "api.example.com");
        // A template interpolation is Medium.
        assert_eq!(hit_json(&fp, "endpoint:PUT:${…}/items")["confidence"], "medium");
    }

    #[test]
    fn spring_jaxrs_get_with_path_is_not_a_retrofit_endpoint() {
        // JAX-RS: `@GET` + `@Path` on a resource interface is a SERVER route;
        // a string-argument `@GET` beside a function-level `@Path` is not
        // Retrofit either. Neither may mint a client ENDPOINT.
        let fp = parse(
            r#"
@Path("/items")
interface ItemResource {
    @GET
    @Path("/{id}")
    fun one(): String

    @GET("/weird")
    @Path("/x")
    fun odd(): String
}
"#,
        );
        assert!(endpoint_calls(&fp).is_empty(), "{:?}", endpoint_calls(&fp));
        // The JAX-RS route is still there.
        let route = NodeId::from_parts(crate::GRAPH_TYPE, repo(), node_kind::ROUTE, "GET /items/{id}");
        assert!(fp.nodes.iter().any(|n| n.id == route));
    }

    #[test]
    fn interface_method_with_body_is_not_an_endpoint() {
        let fp = parse(
            r#"
interface Api {
    @GET("/users")
    fun users(): List<String> = listOf()
}

class NotAnInterface {
    @GET("/orders")
    fun orders(): List<String> = listOf()
}
"#,
        );
        assert!(endpoint_calls(&fp).is_empty(), "{:?}", endpoint_calls(&fp));
    }

    #[test]
    fn activity_subclass_is_component() {
        let fp = parse(
            r#"
package com.acme.app

import android.os.Bundle
import androidx.appcompat.app.AppCompatActivity

class MainActivity : AppCompatActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
    }
}

class Plain : Helper()

interface Screen : Activity
"#,
        );
        assert_eq!(roles_of(&fp, "MainActivity"), vec![r#"{"roles":["COMPONENT"]}"#]);
        assert!(roles_of(&fp, "Plain").is_empty());
        // No same-qname COMPONENT twin: the CLASS carries the role (LB.3a).
        assert!(!fp.nav.kind_by_id.values().any(|k| *k == node_kind::COMPONENT));
        // Control: the same base without an Android import is not Android's.
        let backend = parse("class Worker : Service() {\n    fun run() {}\n}\n");
        assert!(roles_of(&backend, "Worker").is_empty());
        let counts = crate::parse_all(
            "import android.app.Service\n\nclass Sync : Service() {\n    fun run() {}\n}\n",
            "s.kt",
            "s",
            repo(),
        )
        .unwrap()
        .clients;
        assert_eq!(counts.components, 1);
    }

    #[test]
    fn hilt_viewmodel_is_component_and_bean() {
        // `@HiltViewModel` classifies the class (no Android import needed)
        // AND makes it a DI bean: its primary constructor injects even
        // without `@Inject constructor`.
        let fp = parse(
            r#"
@HiltViewModel
class UsersViewModel(private val repo: UserRepository) : ViewModel() {
    fun load() = repo.all()
}

@AndroidEntryPoint
class HomeFragment : Fragment()
"#,
        );
        assert_eq!(roles_of(&fp, "UsersViewModel"), vec![r#"{"roles":["COMPONENT"]}"#]);
        assert_eq!(roles_of(&fp, "HomeFragment"), vec![r#"{"roles":["COMPONENT"]}"#]);
        let vm = NodeId::from_parts(crate::GRAPH_TYPE, repo(), node_kind::CLASS, "UsersViewModel");
        assert!(fp.refs.iter().any(|r| r.from == vm
            && r.category == edge_category::INJECTS
            && r.qualifier == glia_code_domain::CallQualifier::Bare("UserRepository".into())));
    }

    #[test]
    fn client_url_reconstructs_templates_and_concatenation() {
        assert_eq!(placeholders("/users/$id/x"), ("/users/${…}/x".to_string(), true));
        assert_eq!(placeholders("/o/${o.id}"), ("/o/${…}".to_string(), true));
        assert_eq!(placeholders("/a/\\$b"), ("/a/$b".to_string(), false));
        assert_eq!(placeholders("/cost/$"), ("/cost/$".to_string(), false));
        assert_eq!(base_relative_path("users?a=1"), "/users");
        assert_eq!(base_relative_path("${…}/users"), "${…}/users");
    }

    #[test]
    fn marker_line_shape() {
        let c = ClientCounts {
            endpoints: 2,
            components: 0,
            spring_clients: 1,
        };
        assert_eq!(
            marker(c, "fixtures/kotlin-retrofit/client"),
            "[kotlin/retrofit] endpoints=2 components=0 spring_clients=1 repo=fixtures/kotlin-retrofit/client"
        );
    }
}
