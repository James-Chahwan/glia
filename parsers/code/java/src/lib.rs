use repo_graph_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use tree_sitter::{Node as TsNode, Parser};

pub use repo_graph_code_domain::{
    CallQualifier, CallSite, CodeNav, FileParse, GRAPH_TYPE, ImportStmt, ImportTarget, ParseError,
    UnresolvedRef, cell_type, edge_category, node_kind,
};
use repo_graph_code_domain::endpoint::{
    self, ClientEndpoint, HitExtras, push_client_endpoint_with,
};
use repo_graph_code_domain::jvm;

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

    // LA.30b fired_on: `glia analyze <repo> 2>&1 | grep '\[java-enums\]'` — per
    // file that declares an enum or produced a constant reference.
    let (member_uses, member_refs) = resolve_member_refs(module_id, &mut acc);
    if acc.enums.enums > 0 || member_uses + member_refs > 0 {
        let e = acc.enums;
        eprintln!(
            "[java-enums] enums={} constants={} methods={} const_bodies={} member_uses={member_uses} member_refs={member_refs} file={file_rel_path}",
            e.enums, e.constants, e.methods, e.const_bodies
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
    // LA.22b fired_on: `glia analyze <repo> 2>&1 | grep '\[java-http\] declarative'`.
    if acc.declarative.ifaces > 0 {
        let d = acc.declarative;
        eprintln!(
            "[java-http] declarative feign={} exchange={} microprofile={} retrofit={} routes_suppressed={} path={file_rel_path}",
            d.feign, d.exchange, d.microprofile, d.retrofit, d.routes_suppressed
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
    /// LA.22b: declarative client interfaces, for the `[java-http]
    /// declarative` marker.
    declarative: DeclarativeCounts,
    /// LA.30b: the constants of every enum declared in this file, by enum
    /// qname: constant name -> its ATTRIBUTE id. Filled by a pre-scan of the
    /// enum body before any member is walked, so a constant body or method
    /// may name a constant declared after it. Lookup only, never iterated.
    enum_consts: HashMap<String, HashMap<String, NodeId>>,
    /// LA.30b: simple name -> qnames of the enums declared in this file (two
    /// nested enums may share a simple name; such a base is ambiguous).
    enum_names: HashMap<String, Vec<String>>,
    /// LA.30b: constant / member references recorded in method bodies, in
    /// walk order, deduped per `(from, base, name)`; resolved by
    /// [`resolve_member_refs`] once every type of the file is visited.
    member_refs: Vec<MemberRef>,
    member_ref_seen: HashSet<(NodeId, Option<String>, String)>,
    /// LA.30b: per-file tallies for the `[java-enums]` marker.
    enums: EnumCounts,
}

/// LA.30b: a reference to a (possibly) enum constant, recorded while a method
/// body is walked and resolved at the end of [`parse_file`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct MemberRef {
    /// The METHOD whose body holds the reference.
    from: NodeId,
    /// `X` of `X.Y`; `None` for a bare constant name inside its own enum.
    base: Option<String>,
    name: String,
    /// The enclosing enum's qname, for a bare reference.
    enum_ctx: Option<String>,
}

/// LA.30b: per-file tallies for the `[java-enums]` marker.
#[derive(Default, Clone, Copy)]
struct EnumCounts {
    enums: usize,
    constants: usize,
    /// METHOD emissions inside an enum: body methods, constructors (every
    /// overload counts) and constant-body methods.
    methods: usize,
    /// Constants that carry a class body.
    const_bodies: usize,
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
    /// LA.22b: `retrofit2.http` — the gate for a Retrofit interface, whose
    /// `@GET("…")` would otherwise read as a JAX-RS marker.
    retrofit: bool,
    /// LA.22b: `feign.*` — the gate for a Feign-native interface
    /// (`@RequestLine`, no `@FeignClient`).
    feign: bool,
}

impl JavaHttpLibs {
    /// Record one import path (`java.net.http.HttpRequest`, `okhttp3.*`, …).
    fn note_import(&mut self, path: &str) {
        self.jdk |= path.starts_with("java.net.http.");
        self.okhttp |= path.starts_with("okhttp3.");
        self.apache |= is_apache_http_package(path);
        self.retrofit |= path.starts_with("retrofit2.http.");
        self.feign |= path.starts_with("feign.");
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

/// LA.22b: the declarative HTTP client framework an interface is written for.
/// Its mapping annotations describe requests the interface SENDS, so they
/// become client ENDPOINTs, never server ROUTEs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ClientFlavour {
    /// Spring Cloud OpenFeign `@FeignClient` (Spring MVC mapping annotations),
    /// or Feign-native `@RequestLine` in a file importing `feign.*`.
    Feign,
    /// Spring 6 HTTP interface: `@HttpExchange` / `@GetExchange` & co.
    Exchange,
    /// MicroProfile Rest Client: `@RegisterRestClient` + JAX-RS `@GET`/`@Path`.
    MicroProfile,
    /// Retrofit: `retrofit2.http` `@GET("…")` & co, path in the annotation.
    Retrofit,
}

/// LA.22b: a declarative client interface, read once from its own annotations
/// by [`client_iface_of`]. Every method mapping composes onto `prefix`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ClientIface {
    /// The path every method's template hangs off: the base URL's path, then
    /// Feign's `path` / `@RequestMapping`, `@HttpExchange`'s url, or JAX-RS
    /// `@Path`, joined in that order. Empty for Retrofit (its base URL lives
    /// on the `Retrofit.Builder`, not the interface).
    prefix: String,
    /// The literal authority of the base URL (`@FeignClient(url)`,
    /// `@RegisterRestClient(baseUri)`, an absolute `@HttpExchange` url), for
    /// ENDPOINT_HIT's `host`.
    host: Option<String>,
    flavour: ClientFlavour,
}

/// LA.22b: per-file tallies for the `[java-http] declarative` marker.
#[derive(Default, Clone, Copy)]
struct DeclarativeCounts {
    /// Client interfaces found (the marker prints when this is non-zero).
    ifaces: usize,
    /// Client ENDPOINT emissions, per [`ClientFlavour`].
    feign: usize,
    exchange: usize,
    microprofile: usize,
    retrofit: usize,
    /// Mappings `check_route_annotations` would have minted as server ROUTEs
    /// on these interfaces (and now does not).
    routes_suppressed: usize,
}

impl DeclarativeCounts {
    fn bump(&mut self, flavour: ClientFlavour, n: usize) {
        match flavour {
            ClientFlavour::Feign => self.feign += n,
            ClientFlavour::Exchange => self.exchange += n,
            ClientFlavour::MicroProfile => self.microprofile += n,
            ClientFlavour::Retrofit => self.retrofit += n,
        }
    }
}

/// Spring 6 HTTP-interface verb annotations. Kept apart from `endpoint::mapping_verb`
/// so the server path (`check_route_annotations`) never reads them.
fn exchange_verb(name: &str) -> Option<&'static str> {
    Some(match name {
        "GetExchange" => "GET",
        "PostExchange" => "POST",
        "PutExchange" => "PUT",
        "PatchExchange" => "PATCH",
        "DeleteExchange" => "DELETE",
        _ => return None,
    })
}

/// The upper-case verb annotations JAX-RS (`@GET`) and Retrofit
/// (`@GET("users")`) share. The two are told apart by the argument, which
/// Retrofit's carries and JAX-RS's never does.
fn upper_verb_annotation(name: &str) -> Option<&'static str> {
    HTTP_VERBS.iter().copied().find(|v| *v == name)
}

/// The first HTTP verb named in `text`, case-insensitively, as a whole word:
/// `RequestMethod.POST`, `{RequestMethod.GET}`, `"get"`.
fn verb_in_text(text: &str) -> Option<&'static str> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .find_map(|tok| HTTP_VERBS.iter().copied().find(|v| v.eq_ignore_ascii_case(tok)))
}

/// Methods declared directly in a type body.
fn body_methods<'a>(type_node: TsNode<'a>) -> Vec<TsNode<'a>> {
    let Some(body) = type_node.child_by_field_name("body") else {
        return Vec::new();
    };
    let mut cursor = body.walk();
    body.named_children(&mut cursor)
        .filter(|c| c.kind() == "method_declaration")
        .collect()
}

/// A base-URL attribute (`@FeignClient(url)`, `@RegisterRestClient(baseUri)`)
/// as `(host, path)`. Feign accepts a bare `host:port` with no scheme, so a
/// scheme-less value is read as an authority, not as a path. A `${…}`
/// property placeholder names neither: `client_url_split` drops the
/// non-literal authority, and it has no path.
fn base_url_parts(raw: &str) -> (Option<String>, String) {
    let raw = raw.trim();
    let full = if raw.contains("://") {
        raw.to_string()
    } else {
        format!("http://{raw}")
    };
    let (host, path) = endpoint::client_url_split(&full);
    (host, path.unwrap_or_default())
}

/// A path-prefix attribute (Feign `path`, `@HttpExchange` url, `@Path`,
/// `@RequestMapping`) as `(host, path)`. An absolute URL splits like a base
/// URL; a `${…}` / `#{…}` placeholder contributes nothing; anything else is
/// the path itself.
fn prefix_parts(raw: &str) -> (Option<String>, String) {
    let raw = raw.trim();
    if raw.contains("://") {
        return base_url_parts(raw);
    }
    if raw.contains("${") || raw.contains("#{") {
        return (None, String::new());
    }
    (None, raw.to_string())
}

/// LA.22b: whether `node` is a declarative HTTP client interface, and its
/// base. Keys on explicit CLIENT markers only — `@FeignClient`,
/// `@HttpExchange` (on the type or, since Spring allows omitting it there, an
/// `*Exchange` on any method), `@RegisterRestClient`, a Retrofit verb WITH a
/// path argument in a file importing `retrofit2.http`, or a Feign-native
/// `@RequestLine` in a file importing `feign.*`. Never on "is an interface":
/// an OpenAPI `interfaceOnly` server contract (`interface UsersApi {
/// @GetMapping(…) }` implemented by a `@RestController`) is a real route.
fn client_iface_of(node: TsNode, src: &[u8], libs: JavaHttpLibs) -> Option<ClientIface> {
    if node.kind() != "interface_declaration" {
        return None;
    }
    let anns = own_annotation_nodes(node, src);
    let find = |want: &str| anns.iter().find(|(n, _)| n == want).map(|(_, a)| *a);
    let arg = |want: &str| find(want).and_then(|a| ann_path(a, &["value", "path"], src));
    let join = |a: String, b: String| endpoint::join_path(&a, &b);

    if let Some(feign) = find("FeignClient") {
        let (host, url_path) = ann_pair(feign, "url", src)
            .map(|u| base_url_parts(&u))
            .unwrap_or_default();
        let path = ann_pair(feign, "path", src).map(|p| prefix_parts(&p).1).unwrap_or_default();
        // Feign honours a type-level Spring `@RequestMapping` as a prefix too.
        let mapping = arg("RequestMapping").map(|p| prefix_parts(&p).1).unwrap_or_default();
        return Some(ClientIface {
            prefix: join(join(url_path, path), mapping),
            host,
            flavour: ClientFlavour::Feign,
        });
    }

    let methods = body_methods(node);
    let any_method = |pred: &dyn Fn(&str, TsNode) -> bool| {
        methods.iter().any(|m| {
            own_annotation_nodes(*m, src)
                .into_iter()
                .any(|(n, a)| pred(&n, a))
        })
    };

    let type_exchange = find("HttpExchange");
    if type_exchange.is_some()
        || any_method(&|n, _| n == "HttpExchange" || exchange_verb(n).is_some())
    {
        let (host, prefix) = type_exchange
            .and_then(|a| ann_path(a, &["value", "url"], src))
            .map(|p| prefix_parts(&p))
            .unwrap_or_default();
        return Some(ClientIface {
            prefix,
            host,
            flavour: ClientFlavour::Exchange,
        });
    }

    if let Some(rest_client) = find("RegisterRestClient") {
        let (host, base_path) = ann_pair(rest_client, "baseUri", src)
            .map(|u| base_url_parts(&u))
            .unwrap_or_default();
        let path = arg("Path").map(|p| prefix_parts(&p).1).unwrap_or_default();
        return Some(ClientIface {
            prefix: join(base_path, path),
            host,
            flavour: ClientFlavour::MicroProfile,
        });
    }

    if libs.retrofit
        && any_method(&|n, a| {
            (upper_verb_annotation(n).is_some() && ann_path(a, &["value"], src).is_some())
                || (n == "HTTP" && ann_pair(a, "method", src).is_some())
        })
    {
        return Some(ClientIface {
            prefix: String::new(),
            host: None,
            flavour: ClientFlavour::Retrofit,
        });
    }

    if libs.feign && any_method(&|n, _| n == "RequestLine") {
        return Some(ClientIface {
            prefix: String::new(),
            host: None,
            flavour: ClientFlavour::Feign,
        });
    }
    None
}

/// LA.22b: `(verb, template, annotation)` for every request mapping a client
/// interface method declares, read the way `iface`'s framework reads it.
/// `template` is `None` for a marker annotation that maps the prefix itself.
fn client_mappings<'a>(
    method: TsNode<'a>,
    src: &[u8],
    flavour: ClientFlavour,
) -> Vec<(String, Option<String>, TsNode<'a>)> {
    let anns = own_annotation_nodes(method, src);
    let mut out = Vec::new();
    match flavour {
        ClientFlavour::Feign => {
            for (name, ann) in &anns {
                if name.ends_with("Mapping")
                    && let Some(verb) = endpoint::mapping_verb(name)
                {
                    out.push((verb.to_string(), ann_path(*ann, &["value", "path"], src), *ann));
                } else if name == "RequestMapping" {
                    // Feign's SpringMvcContract defaults a method-less
                    // `@RequestMapping` to GET.
                    let verb = ann_pair_text(*ann, "method", src)
                        .and_then(verb_in_text)
                        .unwrap_or("GET");
                    out.push((verb.to_string(), ann_path(*ann, &["value", "path"], src), *ann));
                } else if name == "RequestLine"
                    && let Some(line) = ann_path(*ann, &["value"], src)
                {
                    // Feign-native: `@RequestLine("GET /users/{id}")`.
                    let mut parts = line.split_whitespace();
                    if let (Some(verb), Some(path)) = (parts.next(), parts.next())
                        && let Some(verb) = verb_in_text(verb)
                    {
                        out.push((verb.to_string(), Some(path.to_string()), *ann));
                    }
                }
            }
        }
        ClientFlavour::Exchange => {
            for (name, ann) in &anns {
                if let Some(verb) = exchange_verb(name) {
                    out.push((verb.to_string(), ann_path(*ann, &["value", "url"], src), *ann));
                } else if name == "HttpExchange"
                    && let Some(verb) = ann_pair_text(*ann, "method", src).and_then(verb_in_text)
                {
                    out.push((verb.to_string(), ann_path(*ann, &["value", "url"], src), *ann));
                }
            }
        }
        ClientFlavour::MicroProfile => {
            let verb = anns.iter().find_map(|(n, a)| {
                let verb = upper_verb_annotation(n)?;
                Some((verb, *a))
            });
            if let Some((verb, ann)) = verb {
                let path = anns
                    .iter()
                    .find(|(n, _)| n == "Path")
                    .and_then(|(_, a)| ann_path(*a, &["value"], src));
                out.push((verb.to_string(), path, ann));
            }
        }
        ClientFlavour::Retrofit => {
            for (name, ann) in &anns {
                if let Some(verb) = upper_verb_annotation(name) {
                    // `@GET` with no path is a dynamic `@Url` call: nothing
                    // to name.
                    if let Some(path) = ann_path(*ann, &["value"], src) {
                        out.push((verb.to_string(), Some(path), *ann));
                    }
                } else if name == "HTTP"
                    && let Some(verb) = ann_pair_text(*ann, "method", src).and_then(verb_in_text)
                    && let Some(path) = ann_pair(*ann, "path", src)
                {
                    out.push((verb.to_string(), Some(path), *ann));
                }
            }
        }
    }
    out
}

/// LA.22b: emit the client ENDPOINTs a declarative interface method maps
/// (+ CALLS from the method `from`), composed onto the interface prefix. No
/// ROUTE and no HANDLED_BY. Returns how many endpoints were emitted.
fn emit_client_mappings(
    method: TsNode,
    src: &[u8],
    iface: &ClientIface,
    from: NodeId,
    repo: RepoId,
    file_rel: &str,
    acc: &mut Acc,
) -> usize {
    let mut emitted = 0usize;
    for (verb, tmpl, ann) in client_mappings(method, src, iface.flavour) {
        // A marker annotation with no prefix names nothing (the same rule the
        // server path applies).
        if tmpl.is_none() && iface.prefix.is_empty() {
            continue;
        }
        let tmpl = tmpl.unwrap_or_default();
        let (host, raw) = if tmpl.contains("://") {
            // An absolute template (Retrofit `@GET("https://…")`) replaces
            // the base URL outright.
            base_url_parts(&tmpl)
        } else {
            (
                iface.host.clone(),
                endpoint::compose_route_path(&iface.prefix, &tmpl),
            )
        };
        // Drops a query / fragment (`users?sort=desc`); `raw` is absolute.
        let Some(path) = endpoint::url_to_path(&raw) else {
            continue;
        };
        let pos = ann.start_position();
        let ep = ClientEndpoint {
            method: verb,
            path,
            file: file_rel.to_string(),
            line: pos.row + 1,
            col: pos.column + 1,
            confidence: Confidence::Strong,
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
        emitted += 1;
    }
    acc.declarative.bump(iface.flavour, emitted);
    emitted
}

/// How many ROUTEs `check_route_annotations` would mint for `node`, without
/// minting them: the `routes_suppressed` tally of a client interface.
fn would_mint_routes(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    handler_id: NodeId,
    repo: RepoId,
    class_prefix: &str,
) -> usize {
    let mut scratch = Acc::default();
    check_route_annotations(node, src, file_rel, handler_id, repo, class_prefix, &mut scratch)
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

    // Persistence substrate: a JPA `@Entity` / Mongo `@Document` class is also a
    // DATA_ENTITY; a Spring Data repository (`extends JpaRepository<Entity, Id>`)
    // ACCESSES_DATA the parameterised entity. Both link through a name-derived,
    // flavor-prefixed DATA_ENTITY id so the repository (which sees only the bare
    // entity type name, possibly cross-file) and the entity emitter agree on the
    // same target node.
    if let Some(flavor) = data_entity_flavor(&node, src) {
        emit_data_entity(flavor, name, id, repo, acc);
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
    // LA.22b: a declarative CLIENT interface (Feign, Spring HTTP interface,
    // MicroProfile, Retrofit) maps requests it SENDS: its methods emit
    // ENDPOINTs, and neither they nor the type itself mint ROUTEs.
    let client = client_iface_of(node, src, acc.http_libs);
    if client.is_some() {
        acc.declarative.ifaces += 1;
    }
    let owner = MemberOwner {
        qname: &qname,
        id,
        module_id,
        class_prefix: &class_prefix,
        client: client.as_ref(),
        is_bean,
        enum_ctx: None,
    };
    let composed = if node.kind() == "enum_declaration" {
        // LA.30b: an enum body is `enum_body` (constants, then an optional
        // `enum_body_declarations`), not a `class_body`.
        acc.enums.enums += 1;
        acc.enum_names
            .entry(name.to_string())
            .or_default()
            .push(qname.clone());
        visit_enum_body(
            body,
            src,
            file_rel,
            repo,
            &MemberOwner {
                enum_ctx: Some(&qname),
                ..owner
            },
            acc,
        )
    } else {
        let mut composed = 0usize;
        let mut cursor = body.walk();
        for child in body.named_children(&mut cursor) {
            composed += visit_type_member(child, src, file_rel, repo, &owner, acc);
        }
        composed
    };

    // The class's OWN annotations (its base route), with no prefix to compose
    // against — the prefix IS this annotation. A client interface's type-level
    // `@RequestMapping` / `@Path` is its request prefix, not a route.
    if client.is_some() {
        acc.declarative.routes_suppressed += would_mint_routes(node, src, file_rel, id, repo, "");
    } else {
        check_route_annotations(node, src, file_rel, id, repo, "", acc);
    }
    if composed > 0 && !class_prefix.is_empty() {
        eprintln!(
            "[java-routes] composed {composed} action routes under '{class_prefix}' in {file_rel}"
        );
    }
    true
}

/// The type whose body members [`visit_type_member`] walks: everything a
/// member needs from its owner, read once per type by [`visit_type_decl`].
#[derive(Clone, Copy)]
struct MemberOwner<'a> {
    qname: &'a str,
    id: NodeId,
    module_id: NodeId,
    /// A4.4: the class-level route prefix every action method composes onto.
    class_prefix: &'a str,
    /// LA.22b: the declarative client interface this type is, if any.
    client: Option<&'a ClientIface>,
    /// Pattern E: a Spring stereotype, so constructor params are injected.
    is_bean: bool,
    /// LA.30b: the enclosing enum's qname when the owner IS an enum, so its
    /// methods record bare references to its constants.
    enum_ctx: Option<&'a str>,
}

/// One member of a type body: a constructor or method (METHOD), a field
/// (STATE_VAR / INJECTS) or a nested type. Returns the action routes composed
/// under the owner's class prefix. A class / interface / record body calls it
/// per `class_body` child; an enum per `enum_body_declarations` child (LA.30b).
fn visit_type_member(
    child: TsNode,
    src: &[u8],
    file_rel: &str,
    repo: RepoId,
    owner: &MemberOwner,
    acc: &mut Acc,
) -> usize {
    match child.kind() {
        "constructor_declaration" => {
            let composed = visit_method(
                child,
                src,
                file_rel,
                owner.qname,
                owner.id,
                repo,
                owner.class_prefix,
                owner.client,
                owner.enum_ctx,
                acc,
            );
            if owner.is_bean {
                emit_constructor_injects(child, src, owner.id, owner.module_id, acc);
            }
            composed
        }
        "method_declaration" => visit_method(
            child,
            src,
            file_rel,
            owner.qname,
            owner.id,
            repo,
            owner.class_prefix,
            owner.client,
            owner.enum_ctx,
            acc,
        ),
        "field_declaration" => {
            visit_field_decl(child, src, file_rel, owner.qname, owner.id, repo, acc);
            emit_field_inject(child, src, owner.id, owner.module_id, acc);
            0
        }
        "class_declaration"
        | "interface_declaration"
        | "enum_declaration"
        | "record_declaration" => {
            visit_type_decl(
                child,
                src,
                file_rel,
                owner.qname,
                owner.id,
                owner.module_id,
                repo,
                acc,
            );
            0
        }
        _ => 0,
    }
}

/// LA.30b: walk an `enum_body` (tree-sitter-java: `'{' commaSep(enum_constant)
/// ','? enum_body_declarations? '}'`). Every constant is registered first, so
/// a constant body or a method may name a constant declared after it; then
/// each constant is emitted ([`visit_enum_constant`]) and every
/// `enum_body_declarations` member is walked exactly like a class member.
fn visit_enum_body(
    body: TsNode,
    src: &[u8],
    file_rel: &str,
    repo: RepoId,
    owner: &MemberOwner,
    acc: &mut Acc,
) -> usize {
    let mut cursor = body.walk();
    let children: Vec<TsNode> = body.named_children(&mut cursor).collect();
    for c in children.iter().filter(|c| c.kind() == "enum_constant") {
        if let Some(n) = c.child_by_field_name("name") {
            let name = text_of(n, src);
            let (_, id) = enum_constant_id(owner.qname, name, repo);
            acc.enum_consts
                .entry(owner.qname.to_string())
                .or_default()
                .insert(name.to_string(), id);
        }
    }
    let mut composed = 0usize;
    for child in children {
        match child.kind() {
            "enum_constant" => visit_enum_constant(child, src, file_rel, owner, repo, acc),
            "enum_body_declarations" => {
                let mut dc = child.walk();
                for member in child.named_children(&mut dc) {
                    composed += visit_type_member(member, src, file_rel, repo, owner, acc);
                }
            }
            _ => {}
        }
    }
    composed
}

/// `{enum}::{NAME}` and its ATTRIBUTE id.
fn enum_constant_id(enum_qname: &str, name: &str, repo: RepoId) -> (String, NodeId) {
    let qname = format!("{enum_qname}::{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::ATTRIBUTE, &qname);
    (qname, id)
}

/// LA.30b: one enum constant -> an ATTRIBUTE under its ENUM (HAS_ATTRIBUTE),
/// the Python / Rust-variant (LA.3) shape. A constant with a class body
/// (`GREEN { @Override String label() {..} }`) is an anonymous subclass: its
/// methods hang under the constant, never under the ENUM, where they would
/// shadow the enum's own same-named method in `class_methods`. The
/// constant's `arguments` select a constructor; they are not calls.
fn visit_enum_constant(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    owner: &MemberOwner,
    repo: RepoId,
    acc: &mut Acc,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = text_of(name_node, src);
    let (qname, id) = enum_constant_id(owner.qname, name, repo);
    acc.nodes.push(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells(&node, src, file_rel),
    });
    acc.edges.push(Edge {
        from: owner.id,
        to: id,
        category: edge_category::HAS_ATTRIBUTE,
        confidence: Confidence::Strong,
    });
    acc.nav
        .record(id, name, &qname, node_kind::ATTRIBUTE, Some(owner.id));
    acc.enums.constants += 1;

    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    acc.enums.const_bodies += 1;
    let mut cursor = body.walk();
    for child in body.named_children(&mut cursor) {
        if child.kind() == "method_declaration" {
            visit_method(
                child,
                src,
                file_rel,
                &qname,
                id,
                repo,
                "",
                None,
                owner.enum_ctx,
                acc,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn visit_method(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    class_prefix: &str,
    client: Option<&ClientIface>,
    enum_ctx: Option<&str>,
    acc: &mut Acc,
) -> usize {
    let Some(name_node) = node.child_by_field_name("name") else {
        return 0;
    };
    let name = text_of(name_node, src);
    let qname = format!("{parent_qname}::{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::METHOD, &qname);
    if enum_ctx.is_some() {
        acc.enums.methods += 1;
    }

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
        // LA.30b: inside an enum, a bare name that is one of its constants is
        // a reference to it, unless the method declares a same-named local.
        let scope = enum_ctx.map(|eq| EnumRefScope {
            enum_qname: eq,
            consts: acc
                .enum_consts
                .get(eq)
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default(),
            shadowed: declared_names(node, src),
        });
        collect_calls_in(body, src, id, repo, file_rel, scope.as_ref(), acc);
    }

    // LA.22b: on a client interface the mappings are requests this method
    // sends — ENDPOINTs with a CALLS edge from it, never ROUTEs.
    if let Some(iface) = client {
        acc.declarative.routes_suppressed +=
            would_mint_routes(node, src, file_rel, id, repo, class_prefix);
        emit_client_mappings(node, src, iface, id, repo, file_rel, acc);
        return 0;
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

/// The DATA_ENTITY flavor of a class carrying a persistence annotation, or
/// `None` when it carries none (no DATA_ENTITY projection). The annotation
/// table (`@Document` before `@Entity`) is the JVM family's, shared with the
/// Kotlin parser (A14.4).
fn data_entity_flavor(node: &TsNode, src: &[u8]) -> Option<&'static str> {
    let mods = modifiers_text(node, src)?;
    jvm::DATA_ENTITY_ANNOTATIONS
        .iter()
        .find(|(ann, _)| has_annotation(mods, &format!("@{ann}")))
        .map(|(_, flavor)| *flavor)
}

/// The DATA_ENTITY qname `data_entity:<flavor>:<simple_name>` — the JVM
/// family's recipe (`jvm::data_entity_qname`), so a Kotlin repository and a
/// Java entity of one model name the same node.
fn data_entity_qname(flavor: &str, simple_name: &str) -> String {
    jvm::data_entity_qname(flavor, simple_name)
}

/// Stable, name-derived DATA_ENTITY id — the one constructor both the entity
/// emitter and the repository edge use. Keyed on the flavor and the entity's
/// simple name only, so a repository referencing the bare type name (possibly
/// from another file) and the annotated class that emits it resolve to the same
/// node without needing the entity's fully-qualified module path.
fn data_entity_id(flavor: &str, simple_name: &str, repo: RepoId) -> NodeId {
    NodeId::from_parts(
        GRAPH_TYPE,
        repo,
        node_kind::DATA_ENTITY,
        &data_entity_qname(flavor, simple_name),
    )
}

/// Emit the DATA_ENTITY node projected from an `@Entity` / `@Document` class,
/// plus a DEFINES edge class→entity so the model is reachable from its declaring
/// type. The node's name is the class's simple name; its qname carries the flavor.
fn emit_data_entity(flavor: &str, name: &str, class_id: NodeId, repo: RepoId, acc: &mut Acc) {
    let entity_id = data_entity_id(flavor, name, repo);
    let qname = data_entity_qname(flavor, name);
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
        .record(entity_id, name, &qname, node_kind::DATA_ENTITY, Some(class_id));
}

/// Detect `extends <RepositoryBase>< Entity, … >` on a class or interface and emit
/// an ACCESSES_DATA edge from the repository to the entity's DATA_ENTITY node.
/// `resolve_refs` does not fall back for ACCESSES_DATA, so we wire a direct edge
/// to the name-derived, flavor-prefixed DATA_ENTITY id (matched by
/// `emit_data_entity`), the flavor read off the repository base.
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
            && let Some((entity, flavor)) = repository_entity(n, src)
        {
            acc.edges.push(Edge {
                from: from_id,
                to: data_entity_id(flavor, &entity, repo),
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

/// If `gen` is `RepositoryBase<Entity, …>`, return the entity's simple name and
/// the DATA_ENTITY flavor the base implies.
fn repository_entity(generic: TsNode, src: &[u8]) -> Option<(String, &'static str)> {
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
    // The Spring Data bases (and the flavor each implies) are the JVM
    // family's table, `jvm::REPOSITORY_BASES`, shared with the Kotlin parser.
    let flavor = jvm::repository_flavor(base_simple)?;
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
            return Some((simple.to_string(), flavor));
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
/// value-type denylist for `type_identifier` nodes). The JVM family's list,
/// shared with the Kotlin parser (A14.4).
fn is_non_injectable_type(name: &str) -> bool {
    jvm::is_non_injectable_type(name)
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
    own_annotation_nodes(node, src)
        .into_iter()
        .map(|(name, ann)| {
            let arg = ann
                .child_by_field_name("arguments")
                .and_then(|args| annotation_string_arg(args, src));
            (name, arg)
        })
        .collect()
}

/// A declaration's own annotations as `(simple name, annotation node)`, in
/// source order — the walk [`own_annotations`] reads, for callers (LA.22b)
/// that need a named element (`url = …`, `method = …`) rather than the path.
fn own_annotation_nodes<'a>(node: TsNode<'a>, src: &[u8]) -> Vec<(String, TsNode<'a>)> {
    let mut out = Vec::new();
    let mut push = |ann: TsNode<'a>| {
        if let Some(name_node) = ann.child_by_field_name("name") {
            let name = text_of(name_node, src)
                .rsplit('.')
                .next()
                .unwrap_or_default()
                .trim()
                .to_string();
            out.push((name, ann));
        }
    };
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "annotation" | "marker_annotation" => push(child),
            "modifiers" => {
                let mut inner = child.walk();
                for ann in child.named_children(&mut inner) {
                    if matches!(ann.kind(), "annotation" | "marker_annotation") {
                        push(ann);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// The first string literal of an annotation element value: a plain
/// `"…"`, or the first entry of a `{"…", …}` array.
fn element_string(value: TsNode, src: &[u8]) -> Option<String> {
    match value.kind() {
        "string_literal" => Some(java_string_inner(value, src)),
        "element_value_array_initializer" => {
            let mut cursor = value.walk();
            value
                .named_children(&mut cursor)
                .find(|c| c.kind() == "string_literal")
                .map(|c| java_string_inner(c, src))
        }
        _ => None,
    }
}

/// The value node of `key = …` in an annotation's argument list.
fn ann_pair_value<'a>(ann: TsNode<'a>, key: &str, src: &[u8]) -> Option<TsNode<'a>> {
    let args = ann.child_by_field_name("arguments")?;
    let mut cursor = args.walk();
    args.named_children(&mut cursor)
        .filter(|c| c.kind() == "element_value_pair")
        .find(|c| c.child_by_field_name("key").is_some_and(|k| text_of(k, src) == key))
        .and_then(|c| c.child_by_field_name("value"))
}

/// The string of `key = "…"` in an annotation's argument list.
fn ann_pair(ann: TsNode, key: &str, src: &[u8]) -> Option<String> {
    ann_pair_value(ann, key, src).and_then(|v| element_string(v, src))
}

/// The source text of `key = …` (`RequestMethod.POST`, `"GET"`).
fn ann_pair_text<'a>(ann: TsNode<'a>, key: &str, src: &'a [u8]) -> Option<&'a str> {
    ann_pair_value(ann, key, src).map(|v| text_of(v, src))
}

/// An annotation's path argument: the first of `keys` spelled out
/// (`url = "/x"`), else the bare positional value (`@GetExchange("/x")`).
/// Unlike [`annotation_string_arg`] there is no text-scan fallback, so
/// `@FeignClient(name = "users")` yields no path.
fn ann_path(ann: TsNode, keys: &[&str], src: &[u8]) -> Option<String> {
    if let Some(v) = keys.iter().find_map(|k| ann_pair(ann, k, src)) {
        return Some(v);
    }
    let args = ann.child_by_field_name("arguments")?;
    let mut cursor = args.walk();
    args.named_children(&mut cursor)
        .find(|c| matches!(c.kind(), "string_literal" | "element_value_array_initializer"))
        .and_then(|c| element_string(c, src))
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

/// The route prefix a type contributes to its action methods (Spring
/// `@RequestMapping`, Micronaut `@Controller`, JAX-RS `@Path`). The recipe is
/// the JVM family's, `endpoint::jvm_route_prefix`, shared with the Kotlin
/// parser (A14.4) so both languages compose alike.
fn class_route_prefix(type_node: TsNode, src: &[u8]) -> String {
    endpoint::jvm_route_prefix(&own_annotations(type_node, src))
}

/// Emit the ROUTEs declared by THIS declaration's own annotations, composed
/// onto `class_prefix` (empty at class level, the enclosing type's prefix at
/// method level). Returns how many routes were emitted. Which annotations map
/// which `{VERB} {path}` is `endpoint::jvm_annotation_routes` — the one ROUTE
/// recipe the Java and Kotlin parsers share (A14.4).
fn check_route_annotations(
    node: TsNode,
    src: &[u8],
    _file_rel: &str,
    handler_id: NodeId,
    repo: RepoId,
    class_prefix: &str,
    acc: &mut Acc,
) -> usize {
    let routes = endpoint::jvm_annotation_routes(&own_annotations(node, src), class_prefix);
    for (verb, path) in &routes {
        emit_route(verb, path, handler_id, repo, acc);
    }
    routes.len()
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
    enum_scope: Option<&EnumRefScope>,
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
        } else if n.kind() == "field_access" {
            // LA.30b: `X.Y` with an identifier object and an Upper-initial
            // field is a candidate constant reference (`Color.GREEN`);
            // `resolve_member_refs` keeps it only for an enum of this file or
            // a single-type-imported base.
            if let (Some(obj), Some(field)) = (
                n.child_by_field_name("object"),
                n.child_by_field_name("field"),
            ) && obj.kind() == "identifier"
                && field.kind() == "identifier"
                && text_of(field, src).starts_with(|c: char| c.is_ascii_uppercase())
            {
                push_member_ref(
                    acc,
                    MemberRef {
                        from,
                        base: Some(text_of(obj, src).to_string()),
                        name: text_of(field, src).to_string(),
                        enum_ctx: None,
                    },
                );
            }
        } else if n.kind() == "identifier"
            && let Some(scope) = enum_scope
        {
            // LA.30b: a bare constant name inside its own enum (`return RED;`,
            // `this == RED`, `case BLUE:`), in value position and not
            // shadowed by a local of the method.
            let text = text_of(n, src);
            if scope.consts.contains(text)
                && !scope.shadowed.contains(text)
                && is_value_identifier(n)
            {
                push_member_ref(
                    acc,
                    MemberRef {
                        from,
                        base: None,
                        name: text.to_string(),
                        enum_ctx: Some(scope.enum_qname.to_string()),
                    },
                );
            }
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

/// LA.30b: what a method body inside an enum needs to record bare references
/// to that enum's constants.
struct EnumRefScope<'a> {
    enum_qname: &'a str,
    /// The enum's constant names (registered before any member is walked).
    consts: HashSet<String>,
    /// Names the method declares (parameters, locals, lambda / catch / for /
    /// pattern variables): a same-named identifier is the local, not the
    /// constant, so the whole name is suppressed for this method.
    shadowed: HashSet<String>,
}

/// Record `r` once per `(from, base, name)`, in walk order.
fn push_member_ref(acc: &mut Acc, r: MemberRef) {
    if acc
        .member_ref_seen
        .insert((r.from, r.base.clone(), r.name.clone()))
    {
        acc.member_refs.push(r);
    }
}

/// LA.30b: every name `method` declares: formal / catch / spread parameters,
/// local variables, enhanced-for, resource, lambda and pattern variables. One
/// pre-walk of the whole method (parameters included), so a declaration after
/// the use still shadows it — a conservative suppression, never a false edge.
fn declared_names(method: TsNode, src: &[u8]) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut stack = vec![method];
    while let Some(n) = stack.pop() {
        if n.kind() == "identifier"
            && let Some(p) = n.parent()
        {
            let declares = match p.kind() {
                "variable_declarator"
                | "formal_parameter"
                | "catch_formal_parameter"
                | "enhanced_for_statement"
                | "resource"
                | "instanceof_expression" => p.child_by_field_name("name") == Some(n),
                "lambda_expression" => p.child_by_field_name("parameters") == Some(n),
                "inferred_parameters" | "type_pattern" | "record_pattern_component" => true,
                _ => false,
            };
            if declares {
                out.insert(text_of(n, src).to_string());
            }
        }
        let mut c = n.walk();
        for ch in n.named_children(&mut c) {
            stack.push(ch);
        }
    }
    out
}

/// LA.30b: is this `identifier` read as a value? Not when it names something
/// instead: a method (`name` of a method_invocation), a field (`field` of a
/// field_access), a declaration (`name` of a declarator / parameter), a
/// label, an annotation or annotation key, or a method reference's method.
/// A `switch_label` constant (`case RED:`) IS a value.
fn is_value_identifier(n: TsNode) -> bool {
    let Some(p) = n.parent() else {
        return false;
    };
    match p.kind() {
        "field_access" | "method_invocation" => p.child_by_field_name("object") == Some(n),
        "labeled_statement"
        | "break_statement"
        | "continue_statement"
        | "scoped_identifier"
        | "inferred_parameters"
        | "type_pattern"
        | "record_pattern_component"
        | "record_pattern" => false,
        "element_value_pair" => p.child_by_field_name("key") != Some(n),
        "method_reference" => p.named_child(0) == Some(n),
        "lambda_expression" => p.child_by_field_name("parameters") != Some(n),
        _ => p.child_by_field_name("name") != Some(n),
    }
}

/// LA.30b: turn the recorded [`MemberRef`]s into edges once every type of the
/// file is visited (so a reference to an enum declared further down binds):
///   - bare `RED` inside its enum -> direct USES to that constant;
///   - `X.Y` with `X` the simple name of exactly one enum of this file -> a
///     direct USES to its constant `Y` (dropped when `Y` is not a constant);
///   - `X.Y` with `X` a single-type import of this file -> an UnresolvedRef
///     USES `Attribute{X, Y}`, which the graph crate binds when `X` resolves
///     to an ENUM with an ATTRIBUTE `Y` (LA.30a); a CLASS base stays in
///     `unresolved_refs`;
///   - anything else (`Integer.MAX_VALUE`, a wildcard-imported or same-package
///     unimported type) is dropped, keeping it out of the persisted refs.
///
/// Returns `(direct USES edges, UnresolvedRefs)` for the `[java-enums]` marker.
fn resolve_member_refs(module_id: NodeId, acc: &mut Acc) -> (usize, usize) {
    let imported: HashSet<String> = acc
        .imports
        .iter()
        .filter_map(|i| match &i.target {
            ImportTarget::Symbol { name, .. } => Some(name.clone()),
            ImportTarget::Module { .. } => None,
        })
        .collect();
    let refs = std::mem::take(&mut acc.member_refs);
    let mut edge_seen: HashSet<(NodeId, NodeId)> = HashSet::new();
    let (mut uses, mut unresolved) = (0usize, 0usize);
    for r in refs {
        let target = match (&r.base, &r.enum_ctx) {
            (None, Some(eq)) => acc
                .enum_consts
                .get(eq)
                .and_then(|m| m.get(&r.name))
                .copied(),
            (Some(base), _) => match acc.enum_names.get(base).map(Vec::as_slice) {
                Some([eq]) => acc
                    .enum_consts
                    .get(eq)
                    .and_then(|m| m.get(&r.name))
                    .copied(),
                // A same-file enum name (or an ambiguous one) never falls
                // through to the import table.
                Some(_) => None,
                None => {
                    if imported.contains(base) {
                        acc.refs.push(UnresolvedRef {
                            from: r.from,
                            from_module: module_id,
                            qualifier: CallQualifier::Attribute {
                                base: base.clone(),
                                name: r.name.clone(),
                            },
                            category: edge_category::USES,
                        });
                        unresolved += 1;
                    }
                    None
                }
            },
            (None, None) => None,
        };
        if let Some(to) = target
            && edge_seen.insert((r.from, to))
        {
            acc.edges.push(Edge {
                from: r.from,
                to,
                category: edge_category::USES,
                confidence: Confidence::Strong,
            });
            uses += 1;
        }
    }
    (uses, unresolved)
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
        // LA.30b: one ATTRIBUTE per constant.
        assert_eq!(
            fp.nav
                .kind_by_id
                .values()
                .filter(|k| **k == node_kind::ATTRIBUTE)
                .count(),
            3
        );
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

        // @Entity → DATA_ENTITY node named User, qname carrying the `sql` flavor.
        let entity_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::DATA_ENTITY,
            "data_entity:sql:User",
        );
        assert!(
            fp.nodes.iter().any(|n| n.id == entity_id),
            "expected DATA_ENTITY data_entity:sql:User node"
        );
        assert_eq!(fp.nav.name_by_id.get(&entity_id).map(String::as_str), Some("User"));
        assert_eq!(
            fp.nav.qname_by_id.get(&entity_id).map(String::as_str),
            Some("data_entity:sql:User")
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
    fn document_annotation_uses_nosql_flavor() {
        // A13.2: a Spring Data Mongo `@Document` is a `nosql` entity, and a
        // `MongoRepository<Session, …>` must target the SAME id the entity
        // emitter minted — else the ACCESSES_DATA edge dangles.
        let source = r#"
package com.example;

import org.springframework.data.mongodb.core.mapping.Document;
import org.springframework.data.mongodb.repository.MongoRepository;

@Document
class Session {
    private String id;
}

interface SessionRepo extends MongoRepository<Session, String> {
}
"#;
        let fp = parse_file(source, "SessionRepo.java", "com::example::SessionRepo", repo()).unwrap();
        let entity_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::DATA_ENTITY,
            "data_entity:nosql:Session",
        );
        let entities: Vec<&NodeId> = fp
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::DATA_ENTITY)
            .map(|(id, _)| id)
            .collect();
        assert_eq!(entities, vec![&entity_id], "exactly one DATA_ENTITY, nosql-flavored");
        assert!(fp.nodes.iter().any(|n| n.id == entity_id));
        assert_eq!(fp.nav.name_by_id.get(&entity_id).map(String::as_str), Some("Session"));
        let repo_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::INTERFACE,
            "com::example::SessionRepo",
        );
        let access: Vec<&Edge> = fp
            .edges
            .iter()
            .filter(|e| e.category == edge_category::ACCESSES_DATA)
            .collect();
        assert_eq!(access.len(), 1, "one ACCESSES_DATA edge: {access:?}");
        assert!(
            access[0].from == repo_id && access[0].to == entity_id,
            "expected ACCESSES_DATA SessionRepo -> data_entity:nosql:Session: {access:?}"
        );

        // A class carrying both annotations is a Mongo document.
        let both = r#"
package com.example;

@Entity
@Document
class Audit {
}
"#;
        let fp = parse_file(both, "Audit.java", "com::example::Audit", repo()).unwrap();
        let audit_id =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::DATA_ENTITY, "data_entity:nosql:Audit");
        assert!(
            fp.nodes.iter().any(|n| n.id == audit_id),
            "@Entity + @Document must mint the nosql id"
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

    // ---- LA.22b: declarative HTTP client interfaces ------------------------

    /// Whether a CALLS edge runs from the METHOD `method_qname` to the
    /// ENDPOINT `endpoint_qname`.
    fn method_calls_endpoint(fp: &FileParse, method_qname: &str, endpoint_qname: &str) -> bool {
        let from = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::METHOD, method_qname);
        let to = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ENDPOINT, endpoint_qname);
        fp.edges
            .iter()
            .any(|e| e.from == from && e.to == to && e.category == edge_category::CALLS)
    }

    fn no_routes_or_handled_by(fp: &FileParse) {
        assert_eq!(
            qnames_of(fp, node_kind::ROUTE),
            Vec::<&str>::new(),
            "a client interface must mint no server ROUTE"
        );
        assert!(
            !fp.edges.iter().any(|e| e.category == edge_category::HANDLED_BY),
            "a client interface method handles no route"
        );
    }

    /// The fixture's `UserClient.java`: the Feign `@GetMapping` is a request
    /// the interface sends, and the `@HttpExchange` prefix composes onto
    /// `@GetExchange`.
    #[test]
    fn feign_and_exchange_interfaces_emit_endpoints_not_routes() {
        let source = r#"
package com.example.client;

import org.springframework.cloud.openfeign.FeignClient;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PathVariable;
import org.springframework.web.service.annotation.GetExchange;
import org.springframework.web.service.annotation.HttpExchange;

@FeignClient(name = "users", url = "http://users-svc")
public interface UserClient {
    @GetMapping("/feign/users/{id}")
    String getUser(@PathVariable("id") long id);
}

@HttpExchange("/exchange")
interface ExchangeClient {
    @GetExchange("/users/{id}")
    String get(@PathVariable long id);
}
"#;
        let fp = parse_file(source, "UserClient.java", "UserClient", repo()).unwrap();
        assert_eq!(
            qnames_of(&fp, node_kind::ENDPOINT),
            vec!["endpoint:GET:/exchange/users/{id}", "endpoint:GET:/feign/users/{id}"]
        );
        no_routes_or_handled_by(&fp);
        assert!(method_calls_endpoint(&fp, "UserClient::getUser", "endpoint:GET:/feign/users/{id}"));
        assert!(method_calls_endpoint(&fp, "ExchangeClient::get", "endpoint:GET:/exchange/users/{id}"));
        let feign = endpoint_hit(&fp, "endpoint:GET:/feign/users/{id}");
        assert!(feign.ends_with(r#","host":"users-svc"}"#), "{feign}");
        assert!(feign.contains(r#""line":12,"col":5"#), "positioned at the annotation: {feign}");
        let exchange = endpoint_hit(&fp, "endpoint:GET:/exchange/users/{id}");
        assert!(!exchange.contains("host"), "{exchange}");
    }

    /// Feign composes url path + `path` + type-level `@RequestMapping` +
    /// the method mapping; a method-less `@RequestMapping` is GET, and
    /// `method = RequestMethod.X` is read. A `${…}` url names no host.
    #[test]
    fn feign_client_composes_url_path_and_request_mapping() {
        let source = r#"
package com.example.client;

@FeignClient(name = "users", url = "http://users-svc:8080/base", path = "/api")
@RequestMapping("/v1")
public interface UserClient {
    @GetMapping("/users/{id}")
    String getUser(@PathVariable("id") long id);

    @PostMapping(value = "/users", consumes = "application/json")
    String create(String body);

    @RequestMapping(value = "/users/{id}", method = RequestMethod.DELETE)
    void remove(long id);

    @RequestMapping(path = "/users")
    List<String> all();
}

@FeignClient(name = "orders", url = "${orders.url}")
interface OrderClient {
    @PutMapping("/orders/{id}")
    void put(long id);
}
"#;
        let fp = parse_file(source, "UserClient.java", "UserClient", repo()).unwrap();
        assert_eq!(
            qnames_of(&fp, node_kind::ENDPOINT),
            vec![
                "endpoint:DELETE:/base/api/v1/users/{id}",
                "endpoint:GET:/base/api/v1/users",
                "endpoint:GET:/base/api/v1/users/{id}",
                "endpoint:POST:/base/api/v1/users",
                "endpoint:PUT:/orders/{id}",
            ]
        );
        no_routes_or_handled_by(&fp);
        let hit = endpoint_hit(&fp, "endpoint:POST:/base/api/v1/users");
        assert!(hit.ends_with(r#","host":"users-svc:8080"}"#), "{hit}");
        let orders = endpoint_hit(&fp, "endpoint:PUT:/orders/{id}");
        assert!(!orders.contains("host"), "a property placeholder names no host: {orders}");
    }

    /// Feign-native `@RequestLine("VERB /path")`, with no `@FeignClient`, in a
    /// file importing `feign.*`; the query template is dropped. Without the
    /// import the same interface is left alone.
    #[test]
    fn feign_request_line_emits_endpoints() {
        let source = r#"
import feign.Param;
import feign.RequestLine;

interface GitHub {
    @RequestLine("GET /repos/{owner}/{repo}/contributors")
    List<Contributor> contributors(@Param("owner") String owner, @Param("repo") String repo);

    @RequestLine("POST /repos/{owner}/{repo}/issues?draft={draft}")
    void createIssue(Issue issue, @Param("owner") String owner, @Param("repo") String repo);
}
"#;
        let fp = parse_file(source, "GitHub.java", "GitHub", repo()).unwrap();
        assert_eq!(
            qnames_of(&fp, node_kind::ENDPOINT),
            vec![
                "endpoint:GET:/repos/{owner}/{repo}/contributors",
                "endpoint:POST:/repos/{owner}/{repo}/issues",
            ]
        );
        assert!(method_calls_endpoint(
            &fp,
            "GitHub::contributors",
            "endpoint:GET:/repos/{owner}/{repo}/contributors"
        ));
        no_routes_or_handled_by(&fp);

        let unimported = source.replace("import feign.Param;\nimport feign.RequestLine;\n", "");
        let fp = parse_file(&unimported, "GitHub.java", "GitHub", repo()).unwrap();
        assert!(qnames_of(&fp, node_kind::ENDPOINT).is_empty());
    }

    /// Spring HTTP interface: the type-level `@HttpExchange(url = …)` prefixes
    /// every `*Exchange`; a marker `@PostExchange` maps the prefix itself; a
    /// method-level `@HttpExchange(method = …)` is read; and an interface
    /// with no type-level `@HttpExchange` is still a client.
    #[test]
    fn http_exchange_interface_composes_its_prefix() {
        let source = r#"
@HttpExchange(url = "/api", accept = "application/json")
public interface ExchangeClient {
    @GetExchange("/users/{id}")
    String get(@PathVariable long id);

    @PostExchange
    String create(@RequestBody String body);

    @HttpExchange(method = "PUT", url = "/users/{id}")
    void put(@PathVariable long id, @RequestBody String body);
}

interface ItemClient {
    @DeleteExchange(url = "/items/{id}")
    void remove(@PathVariable long id);
}
"#;
        let fp = parse_file(source, "ExchangeClient.java", "ExchangeClient", repo()).unwrap();
        assert_eq!(
            qnames_of(&fp, node_kind::ENDPOINT),
            vec![
                "endpoint:DELETE:/items/{id}",
                "endpoint:GET:/api/users/{id}",
                "endpoint:POST:/api",
                "endpoint:PUT:/api/users/{id}",
            ]
        );
        assert!(method_calls_endpoint(&fp, "ItemClient::remove", "endpoint:DELETE:/items/{id}"));
        no_routes_or_handled_by(&fp);
    }

    /// Retrofit: relative paths become absolute, the query is dropped,
    /// `@HTTP(method, path)` is read, an absolute URL carries its host, and a
    /// dynamic `@GET` + `@Url` names nothing.
    #[test]
    fn retrofit_interface_relative_paths_become_absolute() {
        let source = r#"
package com.example.client;

import retrofit2.Call;
import retrofit2.http.*;

public interface RepoApi {
    @POST("repos")
    Call<Void> create(@Body Object b);

    @GET("users/{user}/repos?sort=desc")
    Call<List<Repo>> list(@Path("user") String user);

    @HTTP(method = "DELETE", path = "repos/{id}", hasBody = true)
    Call<Void> remove(@Path("id") long id, @Body Object reason);

    @GET("https://api.github.com/meta")
    Call<Meta> meta();

    @GET
    Call<String> fetch(@Url String url);
}
"#;
        let fp = parse_file(source, "RepoApi.java", "RepoApi", repo()).unwrap();
        assert_eq!(
            qnames_of(&fp, node_kind::ENDPOINT),
            vec![
                "endpoint:DELETE:/repos/{id}",
                "endpoint:GET:/meta",
                "endpoint:GET:/users/{user}/repos",
                "endpoint:POST:/repos",
            ]
        );
        assert!(method_calls_endpoint(&fp, "RepoApi::create", "endpoint:POST:/repos"));
        let meta = endpoint_hit(&fp, "endpoint:GET:/meta");
        assert!(meta.ends_with(r#","host":"api.github.com"}"#), "{meta}");
        no_routes_or_handled_by(&fp);

        // The same interface without the retrofit2.http import is not a client.
        let unimported = source.replace("import retrofit2.http.*;\n", "");
        let fp = parse_file(&unimported, "RepoApi.java", "RepoApi", repo()).unwrap();
        assert!(qnames_of(&fp, node_kind::ENDPOINT).is_empty());
    }

    /// MicroProfile Rest Client: `@RegisterRestClient` + the interface's JAX-RS
    /// `@Path` prefix every `@GET` / `@POST`; `baseUri` contributes its path
    /// and its host.
    #[test]
    fn microprofile_register_rest_client_emits_endpoints() {
        let source = r#"
@RegisterRestClient(configKey = "users-api")
@Path("/users")
public interface UsersClient {
    @GET
    @Path("/{id}")
    User get(@PathParam("id") long id);

    @POST
    User create(User u);
}

@RegisterRestClient(baseUri = "http://orders:9000/api")
@Path("/orders")
interface OrdersClient {
    @GET
    List<Order> all();
}
"#;
        let fp = parse_file(source, "UsersClient.java", "UsersClient", repo()).unwrap();
        assert_eq!(
            qnames_of(&fp, node_kind::ENDPOINT),
            vec![
                "endpoint:GET:/api/orders",
                "endpoint:GET:/users/{id}",
                "endpoint:POST:/users",
            ]
        );
        assert!(method_calls_endpoint(&fp, "UsersClient::get", "endpoint:GET:/users/{id}"));
        let orders = endpoint_hit(&fp, "endpoint:GET:/api/orders");
        assert!(orders.ends_with(r#","host":"orders:9000"}"#), "{orders}");
        no_routes_or_handled_by(&fp);
    }

    /// Regression guard: an OpenAPI-generator `interfaceOnly` server contract
    /// (Spring or JAX-RS mappings on an interface a controller implements)
    /// carries NO client marker, so it keeps its ROUTEs and mints no ENDPOINT.
    #[test]
    fn interface_only_server_controller_still_emits_routes() {
        let source = r#"
package com.example.api;

@RequestMapping("/api")
public interface UsersApi {
    @GetMapping("/users")
    List<User> list();
}

@Path("/items")
interface ItemsResource {
    @GET
    @Path("/{id}")
    Item get(@PathParam("id") long id);
}

@RestController
public class UsersController implements UsersApi {
    public List<User> list() { return List.of(); }
}
"#;
        let fp = parse_file(source, "UsersApi.java", "com::example::api::UsersApi", repo()).unwrap();
        assert_eq!(
            qnames_of(&fp, node_kind::ROUTE),
            vec!["ANY /api", "ANY /items", "GET /api/users", "GET /items/{id}"]
        );
        assert!(qnames_of(&fp, node_kind::ENDPOINT).is_empty());
        let route = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ROUTE, "GET /api/users");
        let handler =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::METHOD, "com::example::api::UsersApi::list");
        assert!(fp.edges.iter().any(|e| e.from == route
            && e.to == handler
            && e.category == edge_category::HANDLED_BY));
    }

    /// The `routes_suppressed` tally is exactly what the server path would
    /// have minted: the type-level `@RequestMapping` plus each method mapping.
    #[test]
    fn suppressed_routes_are_what_the_server_path_would_mint() {
        let source = r#"
@FeignClient(name = "users")
@RequestMapping("/v1")
interface UserClient {
    @GetMapping("/users")
    List<String> all();

    @PostMapping("/users")
    String create(String body);
}
"#;
        let tree = {
            let mut parser = Parser::new();
            let lang: tree_sitter::Language = tree_sitter_java::LANGUAGE.into();
            parser.set_language(&lang).unwrap();
            parser.parse(source, None).unwrap()
        };
        let src = source.as_bytes();
        let iface = tree.root_node().named_child(0).unwrap();
        let client = client_iface_of(iface, src, JavaHttpLibs::default()).unwrap();
        assert_eq!(client.flavour, ClientFlavour::Feign);
        assert_eq!(client.prefix, "/v1");
        assert_eq!(client.host, None);
        let id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::INTERFACE, "UserClient");
        let type_level = would_mint_routes(iface, src, "U.java", id, repo(), "");
        let methods: usize = body_methods(iface)
            .into_iter()
            .map(|m| would_mint_routes(m, src, "U.java", id, repo(), "/v1"))
            .sum();
        assert_eq!((type_level, methods), (1, 2));
    }

    // ---- LA.30b: enum constants, enum-body members, constant references ----

    const COLOR_SRC: &str = r#"
package com.shop;

public enum Color {
    RED,
    GREEN("g") {
        @Override
        public String label() { return "green!"; }
    },
    BLUE;

    /** The colour a caller gets when it names none. */
    public static final Color DEFAULT = RED;

    private final String code;

    Color() { this.code = ""; }

    Color(String c) { this.code = c; }

    public String label() { return code; }

    public static Color pick() { return RED; }

    public boolean warm() { return isRed(); }

    private boolean isRed() { return this == RED; }

    public int rank() {
        switch (this) {
            case BLUE: return 2;
            default: return 0;
        }
    }
}
"#;
    /// LB.2: the public enum of `Color.java` shares the file MODULE's qname.
    const COLOR: &str = "src::main::java::com::shop::Color";

    fn parse_color() -> FileParse {
        parse_file(
            COLOR_SRC,
            "src/main/java/com/shop/Color.java",
            COLOR,
            repo(),
        )
        .unwrap()
    }

    fn nid(kind: repo_graph_core::NodeKindId, qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), kind, qname)
    }

    fn has_edge(
        fp: &FileParse,
        from: NodeId,
        to: NodeId,
        cat: repo_graph_core::EdgeCategoryId,
    ) -> bool {
        fp.edges
            .iter()
            .any(|e| e.from == from && e.to == to && e.category == cat)
    }

    /// Every in-parse USES edge as `(from qname, to qname)`, sorted.
    fn uses_edges(fp: &FileParse) -> Vec<(String, String)> {
        let q = |id: &NodeId| fp.nav.qname_by_id.get(id).cloned().unwrap_or_default();
        let mut out: Vec<(String, String)> = fp
            .edges
            .iter()
            .filter(|e| e.category == edge_category::USES)
            .map(|e| (q(&e.from), q(&e.to)))
            .collect();
        out.sort();
        out
    }

    fn uses_refs(fp: &FileParse) -> Vec<&UnresolvedRef> {
        fp.refs
            .iter()
            .filter(|r| r.category == edge_category::USES)
            .collect()
    }

    #[test]
    fn enum_constants_are_attributes() {
        let fp = parse_color();
        let enum_id = nid(node_kind::ENUM, COLOR);
        assert_eq!(
            qnames_of(&fp, node_kind::ATTRIBUTE),
            vec![
                format!("{COLOR}::BLUE"),
                format!("{COLOR}::GREEN"),
                format!("{COLOR}::RED")
            ]
        );
        for c in ["RED", "GREEN", "BLUE"] {
            let cid = nid(node_kind::ATTRIBUTE, &format!("{COLOR}::{c}"));
            assert!(
                has_edge(&fp, enum_id, cid, edge_category::HAS_ATTRIBUTE),
                "HAS_ATTRIBUTE {c}"
            );
            assert_eq!(
                fp.nav.parent_of.get(&cid),
                Some(&enum_id),
                "{c} hangs off the ENUM"
            );
            let node = fp
                .nodes
                .iter()
                .find(|n| n.id == cid)
                .expect("constant node");
            assert!(
                node.cells.iter().any(|c| c.kind == cell_type::POSITION),
                "{c} has a POSITION"
            );
        }
        assert_eq!(
            fp.edges
                .iter()
                .filter(|e| e.category == edge_category::HAS_ATTRIBUTE)
                .count(),
            3
        );
    }

    #[test]
    fn enum_body_declarations_are_walked() {
        let fp = parse_color();
        let enum_id = nid(node_kind::ENUM, COLOR);
        for m in ["Color", "label", "pick", "warm", "isRed", "rank"] {
            let mid = nid(node_kind::METHOD, &format!("{COLOR}::{m}"));
            assert!(fp.nodes.iter().any(|n| n.id == mid), "METHOD {m} missing");
            assert_eq!(
                fp.nav.parent_of.get(&mid),
                Some(&enum_id),
                "{m} is a member of the ENUM"
            );
            assert!(
                has_edge(&fp, enum_id, mid, edge_category::DEFINES),
                "DEFINES {m}"
            );
        }
        // The documented `static final` field is a STATE_VAR of the enum.
        assert_eq!(
            qnames_of(&fp, node_kind::STATE_VAR),
            vec![format!("{COLOR}::DEFAULT")]
        );
        // `warm` calls `isRed` unqualified: a SelfMethod the graph binds
        // against the ENUM (LA.30a).
        let warm = nid(node_kind::METHOD, &format!("{COLOR}::warm"));
        assert!(fp.calls.iter().any(
            |c| c.from == warm && c.qualifier == CallQualifier::SelfMethod("isRed".to_string())
        ));
    }

    #[test]
    fn constant_body_methods_hang_off_the_constant() {
        let fp = parse_color();
        let enum_id = nid(node_kind::ENUM, COLOR);
        let green = nid(node_kind::ATTRIBUTE, &format!("{COLOR}::GREEN"));
        let body_label = nid(node_kind::METHOD, &format!("{COLOR}::GREEN::label"));
        assert!(
            fp.nodes.iter().any(|n| n.id == body_label),
            "the override is a METHOD"
        );
        assert_eq!(fp.nav.parent_of.get(&body_label), Some(&green));
        assert!(has_edge(&fp, green, body_label, edge_category::DEFINES));
        assert!(!has_edge(&fp, enum_id, body_label, edge_category::DEFINES));
        // The enum's own `label` stays the ENUM's member; the override never
        // replaces it.
        let own_label = nid(node_kind::METHOD, &format!("{COLOR}::label"));
        assert_eq!(fp.nav.parent_of.get(&own_label), Some(&enum_id));
        // `GREEN("g")` selects a constructor; it is not a call.
        assert!(!fp.calls.iter().any(|c| matches!(&c.qualifier,
            CallQualifier::Bare(n) | CallQualifier::SelfMethod(n) if n == "Color")));
    }

    #[test]
    fn bare_constant_reference_is_a_uses_edge() {
        let fp = parse_color();
        assert_eq!(
            uses_edges(&fp),
            vec![
                (format!("{COLOR}::isRed"), format!("{COLOR}::RED")),
                (format!("{COLOR}::pick"), format!("{COLOR}::RED")),
                (format!("{COLOR}::rank"), format!("{COLOR}::BLUE")),
            ],
            "exactly the constants each method names: `return RED`, `this == RED`, `case BLUE:`"
        );
        assert!(
            uses_refs(&fp).is_empty(),
            "same-file constants resolve in the parse"
        );
    }

    #[test]
    fn local_named_like_a_constant_is_not_a_reference() {
        let source = r#"
package com.shop;

enum Level {
    LOW, HIGH;

    int param(int HIGH) { return HIGH; }

    int local() { int LOW = 3; return LOW; }

    int lambda() { return java.util.List.of(1).stream().mapToInt(LOW -> LOW).sum(); }

    void label() { LOW: for (;;) { break LOW; } }

    int named() { return HIGH.ordinal(); }
}
"#;
        let module = "src::main::java::com::shop::Level";
        let fp = parse_file(source, "src/main/java/com/shop/Level.java", module, repo()).unwrap();
        assert_eq!(
            uses_edges(&fp),
            vec![(format!("{module}::named"), format!("{module}::HIGH"))],
            "a parameter, a local, a lambda parameter and a label shadow the constant; \
             `HIGH.ordinal()` reads it"
        );
    }

    #[test]
    fn qualified_reference_to_imported_enum_is_a_uses_ref() {
        let source = r#"
package com.shop.web;

import com.shop.Color;

public class Picker {
    public Color choose() { return Color.pick(); }

    public Color green() { return Color.GREEN; }

    public String lbl(Color c) { return c.label(); }
}
"#;
        let module = "src::main::java::com::shop::web::Picker";
        let fp = parse_file(
            source,
            "src/main/java/com/shop/web/Picker.java",
            module,
            repo(),
        )
        .unwrap();
        let green = nid(node_kind::METHOD, &format!("{module}::green"));
        let refs = uses_refs(&fp);
        assert_eq!(refs.len(), 1, "only `Color.GREEN`: {refs:?}");
        assert_eq!(refs[0].from, green);
        assert_eq!(refs[0].from_module, nid(node_kind::MODULE, module));
        assert_eq!(
            refs[0].qualifier,
            CallQualifier::Attribute {
                base: "Color".to_string(),
                name: "GREEN".to_string()
            }
        );
        assert!(
            uses_edges(&fp).is_empty(),
            "an imported enum is bound by the graph crate"
        );
        // `Color.pick()` stays a call site (LA.30a binds it), never a member ref.
        let choose = nid(node_kind::METHOD, &format!("{module}::choose"));
        assert!(fp.calls.iter().any(|c| c.from == choose
            && c.qualifier
                == CallQualifier::Attribute {
                    base: "Color".to_string(),
                    name: "pick".to_string()
                }));
    }

    #[test]
    fn qualified_reference_to_same_file_enum_is_a_direct_edge() {
        let source = r#"
package com.shop;

public class Shop {
    enum Size { S, M }

    Size small() { return Size.S; }

    Size missing() { return Size.XL; }

    Tier gold() { return Tier.GOLD; }
}

enum Tier { GOLD, SILVER }
"#;
        let module = "src::main::java::com::shop::Shop";
        let fp = parse_file(source, "src/main/java/com/shop/Shop.java", module, repo()).unwrap();
        assert_eq!(
            uses_edges(&fp),
            vec![
                (
                    format!("{module}::gold"),
                    "src::main::java::com::shop::Tier::GOLD".to_string()
                ),
                (format!("{module}::small"), format!("{module}::Size::S")),
            ],
            "a nested enum and one declared further down the file both bind in the parse; \
             `Size.XL` names no constant and is dropped"
        );
        assert!(uses_refs(&fp).is_empty());
    }

    #[test]
    fn unimported_uppercase_field_access_emits_nothing() {
        let source = r#"
package com.shop;

import org.springframework.http.*;

public class Limits {
    int max() { return Integer.MAX_VALUE; }

    Object ok() { return HttpStatus.OK; }

    Object out() { return System.out; }
}
"#;
        let module = "src::main::java::com::shop::Limits";
        let fp = parse_file(source, "src/main/java/com/shop/Limits.java", module, repo()).unwrap();
        assert!(
            uses_refs(&fp).is_empty(),
            "no import binds Integer / HttpStatus: {:?}",
            fp.refs
        );
        assert!(uses_edges(&fp).is_empty());
    }
}
