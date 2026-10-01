use std::sync::OnceLock;

use glia_code_domain::{CodeNav, GRAPH_TYPE, node_kind};
use glia_core::{Confidence, Node, NodeId, RepoId};

use crate::anchor::{Anchor, line_of};
use crate::code_guard::LazyGuard;

pub struct GraphqlNodes {
    pub nodes: Vec<Node>,
    pub nav: CodeNav,
    /// A5.8: the line that minted each node (see `crate::anchor`).
    pub anchors: Vec<Anchor>,
}

/// LA.26: what a needle hit must show before it mints a GRAPHQL_OPERATION.
/// Every needle is shared with a non-GraphQL library, and the scan runs on
/// every code file of every language, so the text alone is no evidence.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Family {
    /// Apollo / urql hooks. TanStack Query exports the same hook names.
    Hook,
    /// Apollo / urql client methods. node-postgres and mqtt.js clients share
    /// the method names.
    ClientMethod,
    /// graphql-request's `request` function and client method. Every HTTP
    /// client (Guzzle, requests, ureq, reqwest) has one too.
    Request,
}

/// The first needle (table order) on a line is the one judged. The package
/// name graphql-request is not a needle: it names no call, so it only feeds
/// [`GqlContext`].
const OPERATION_PATTERNS: &[(&str, Family)] = &[
    ("useQuery(", Family::Hook),
    ("useMutation(", Family::Hook),
    ("useSubscription(", Family::Hook),
    ("useLazyQuery(", Family::Hook),
    ("client.query(", Family::ClientMethod),
    ("client.mutate(", Family::ClientMethod),
    ("client.subscribe(", Family::ClientMethod),
    ("request(", Family::Request),
];

/// GraphQL client packages, matched as a whole quoted module string. An entry
/// ending in `/` is a scope prefix (`'@urql/core'`); any other entry must be
/// followed by a closing quote or a `/` subpath, so `'urql-x'` is not urql.
const GRAPHQL_CLIENT_MODULES: &[&str] = &[
    "@apollo/client",
    "apollo-client",
    "@apollo/react-hooks",
    "react-apollo",
    "apollo-angular",
    "@vue/apollo-composable",
    "urql",
    "@urql/",
    "graphql-hooks",
    "react-relay",
    "relay-runtime",
    "graphql-tag",
    "graphql-request",
];

/// TanStack Query packages: their `useQuery` takes a query key and a fetcher,
/// never a GraphQL document.
const TANSTACK_MODULES: &[&str] = &["@tanstack/", "react-query", "vue-query", "svelte-query"];

/// The document tags [`extract_gql_operation_name`] reads, as file context.
const GQL_TAGS: &[&str] = &["gql`", "graphql`", "gql(", "graphql("];

/// Object keys that carry the GraphQL document in an options argument.
const DOCUMENT_KEYS: &[&[u8]] = &[b"query", b"mutation", b"subscription", b"document"];

/// How far past a needle's `(` [`first_arg_shape`] reads, in bytes.
const ARG_WINDOW: usize = 512;

/// File-level GraphQL evidence, computed once per file on its first needle
/// hit.
#[derive(Default, Clone, Copy)]
struct GqlContext {
    /// A GraphQL client package is imported (quoted module string).
    client_import: bool,
    /// The graphql-request package, or its `GraphQLClient(` constructor.
    graphql_request: bool,
    /// A gql / graphql document tag or call.
    gql_tag: bool,
    /// A TanStack Query package.
    tanstack: bool,
}

impl GqlContext {
    /// The true flags joined by `+`, for the `[graphql-ops]` marker.
    fn label(self) -> String {
        let flags: Vec<&str> = [
            (self.client_import, "client"),
            (self.graphql_request, "graphql-request"),
            (self.gql_tag, "gql-tag"),
            (self.tanstack, "tanstack"),
        ]
        .into_iter()
        .filter_map(|(on, name)| on.then_some(name))
        .collect();
        if flags.is_empty() {
            "none".to_string()
        } else {
            flags.join("+")
        }
    }
}

/// The first argument of a needle call, read from the source after its `(`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ArgShape {
    /// `{ query: Q }`, `{ mutation }`, `{ document: D, ... }`.
    QueryObject,
    /// Any other object literal (`{ queryKey, queryFn }`, `{ text, values }`).
    OtherObject,
    /// An identifier or an inline tag (`GET_USERS`, `gql` + template).
    Ident,
    /// A string or template literal.
    Str,
    /// An array literal (a TanStack query key).
    Array,
    /// No argument.
    Empty,
    /// Anything else, or nothing within [`ARG_WINDOW`].
    Other,
}

/// Rejected needle hits by family, plus the kept count, for the
/// `[graphql-ops]` marker. `context` is set on the file's first counted hit.
#[derive(Default)]
struct NeedleTally {
    kept: usize,
    hook: usize,
    client: usize,
    request: usize,
    context: Option<GqlContext>,
}

impl NeedleTally {
    fn record(&mut self, family: Family, kept: bool) {
        if kept {
            self.kept += 1;
            return;
        }
        match family {
            Family::Hook => self.hook += 1,
            Family::ClientMethod => self.client += 1,
            Family::Request => self.request += 1,
        }
    }

    /// The marker line, or `None` when the file had no needle hit.
    fn marker(&self) -> Option<String> {
        let ctx = self.context?;
        Some(format!(
            "[graphql-ops] kept={} rejected={} hook={} client={} request={} context={}",
            self.kept,
            self.hook + self.client + self.request,
            self.hook,
            self.client,
            self.request,
            ctx.label()
        ))
    }
}

/// `GLIA_GRAPHQL_DEBUG=1` turns on the `[graphql-ops]` marker, read once (the
/// `queues::debug_enabled` pattern).
fn graphql_debug() -> bool {
    static DEBUG: OnceLock<bool> = OnceLock::new();
    *DEBUG.get_or_init(|| {
        std::env::var("GLIA_GRAPHQL_DEBUG").is_ok_and(|v| !v.is_empty() && v != "0")
    })
}

fn is_ident_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'$'
}

/// True when the byte before offset `at` is an identifier byte, so a match
/// there is the tail of a longer name (`handle_request(`).
fn ident_byte_before(b: &[u8], at: usize) -> bool {
    at.checked_sub(1)
        .and_then(|p| b.get(p))
        .is_some_and(|&c| is_ident_byte(c))
}

/// `module` as a whole quoted module string starting at `start` (the byte
/// after the opening quote). See [`GRAPHQL_CLIENT_MODULES`] for the rule.
fn quoted_module_at(b: &[u8], start: usize, module: &str) -> bool {
    let m = module.as_bytes();
    let Some(end) = start.checked_add(m.len()) else {
        return false;
    };
    if b.get(start..end) != Some(m) {
        return false;
    }
    m.last() == Some(&b'/') || matches!(b.get(end), Some(b'\'' | b'"' | b'/'))
}

/// True when `needle` occurs at the start of `source` or after a
/// non-identifier byte.
fn occurs_bounded(source: &str, needle: &str) -> bool {
    let b = source.as_bytes();
    source
        .match_indices(needle)
        .any(|(at, _)| !ident_byte_before(b, at))
}

fn gql_context(source: &str) -> GqlContext {
    let b = source.as_bytes();
    let mut ctx = GqlContext::default();
    for (i, &c) in b.iter().enumerate() {
        if c != b'\'' && c != b'"' {
            continue;
        }
        let start = i + 1;
        ctx.client_import = ctx.client_import
            || GRAPHQL_CLIENT_MODULES
                .iter()
                .any(|m| quoted_module_at(b, start, m));
        ctx.graphql_request =
            ctx.graphql_request || quoted_module_at(b, start, "graphql-request");
        ctx.tanstack = ctx.tanstack
            || TANSTACK_MODULES
                .iter()
                .any(|m| quoted_module_at(b, start, m));
    }
    ctx.graphql_request = ctx.graphql_request || occurs_bounded(source, "GraphQLClient(");
    ctx.gql_tag = GQL_TAGS.iter().any(|t| occurs_bounded(source, t));
    ctx
}

/// Classify the first argument of the call whose `(` ends at byte
/// `after_paren`. Reads bytes, so a multi-line call works and no `str` is
/// ever sliced mid-character.
fn first_arg_shape(source: &str, after_paren: usize) -> ArgShape {
    let b = source.as_bytes();
    let end = after_paren.saturating_add(ARG_WINDOW).min(b.len());
    let w = b.get(after_paren..end).unwrap_or_default();
    let skip_ws = |mut i: usize| {
        while w.get(i).is_some_and(|c| c.is_ascii_whitespace()) {
            i += 1;
        }
        i
    };
    let i = skip_ws(0);
    match w.get(i) {
        Some(b'{') => {
            let key_start = skip_ws(i + 1);
            let mut key_end = key_start;
            while w.get(key_end).is_some_and(|&c| is_ident_byte(c)) {
                key_end += 1;
            }
            let key = w.get(key_start..key_end).unwrap_or_default();
            let next = w.get(skip_ws(key_end));
            if DOCUMENT_KEYS.contains(&key) && matches!(next, Some(b':' | b',' | b'}')) {
                ArgShape::QueryObject
            } else {
                ArgShape::OtherObject
            }
        }
        Some(&c) if c.is_ascii_alphabetic() || c == b'_' || c == b'$' => ArgShape::Ident,
        Some(b'\'' | b'"' | b'`') => ArgShape::Str,
        Some(b'[') => ArgShape::Array,
        Some(b')') => ArgShape::Empty,
        _ => ArgShape::Other,
    }
}

/// Does this needle hit carry GraphQL evidence? `ident_before` is whether the
/// needle is the tail of a longer identifier.
fn accept(family: Family, arg: ArgShape, ctx: GqlContext, ident_before: bool) -> bool {
    match family {
        // An Apollo hook given a document constant or inline tag, a urql hook
        // given a `query` option; never a TanStack file that imports no
        // GraphQL client.
        Family::Hook => {
            matches!(arg, ArgShape::Ident | ArgShape::QueryObject)
                && (ctx.client_import || !ctx.tanstack)
        }
        // A document key in the options object, or an identifier argument
        // in a file that imports a GraphQL client or builds a document.
        Family::ClientMethod => {
            arg == ArgShape::QueryObject
                || (arg == ArgShape::Ident && (ctx.client_import || ctx.gql_tag))
        }
        // graphql-request's `request` or `client.request`, only in a file
        // that uses graphql-request.
        Family::Request => ctx.graphql_request && !ident_before,
    }
}

/// The needle lines that carry GraphQL evidence, as (operation name, 0-indexed
/// line) in source order, and the tally for the `[graphql-ops]` marker.
/// CJ.1b: a needle occurrence `guard` refuses (it starts in a Rust / Python
/// string literal or comment) carries no evidence.
fn scan_operation_needles(
    source: &str,
    guard: &mut LazyGuard<'_>,
) -> (Vec<(String, u32)>, NeedleTally) {
    let mut hits = Vec::new();
    let mut tally = NeedleTally::default();
    let b = source.as_bytes();
    let mut next_line_start = 0usize;
    for (line_no, line) in source.split_inclusive('\n').enumerate() {
        let line_start = next_line_start;
        next_line_start += line.len();
        let trimmed = line.trim();
        if !OPERATION_PATTERNS.iter().any(|(p, _)| trimmed.contains(p)) {
            continue;
        }
        // A10.9: `api.user.list.useQuery()` is a tRPC hook, not a GraphQL
        // operation. With no gql tag in the file the fallback name would
        // mint `graphql_op:useQuery`, which the GraphQL resolver then pairs
        // to any `Query` resolver by substring.
        if crate::trpc::is_trpc_client_line(trimmed) {
            continue;
        }
        let ctx = *tally.context.get_or_insert_with(|| gql_context(source));
        // Needles in table order; the first one with evidence mints the line.
        // A rejected needle does not end the line: in
        // `useQuery(['users'], () => request(API, DOC))` the hook is TanStack's
        // but the `request` inside it is graphql-request's.
        for &(pattern, family) in OPERATION_PATTERNS {
            if !trimmed.contains(pattern) {
                continue;
            }
            let kept = line.match_indices(pattern).any(|(at, _)| {
                let hit = line_start + at;
                if !guard.admits(hit) {
                    return false;
                }
                let arg = first_arg_shape(source, hit + pattern.len());
                accept(family, arg, ctx, ident_byte_before(b, hit))
            });
            tally.record(family, kept);
            if kept {
                let op_name = extract_gql_operation_name(source, trimmed)
                    .unwrap_or_else(|| pattern.trim_end_matches('(').to_string());
                hits.push((op_name, u32::try_from(line_no).unwrap_or(u32::MAX)));
                break;
            }
        }
    }
    (hits, tally)
}

/// LA.38: the GraphQL root types, the only type-level resolver nouns a
/// decorator or class can name. `@Resolver(` / `@ResolveField(` /
/// `@strawberry.type` are decorator names, and an object type
/// (`class Recipe`) is never a resolver.
const ROOT_TYPES: &[&str] = &["Query", "Mutation", "Subscription"];

/// LA.38: NestJS / TypeGraphQL decorators that name the root type they sit
/// on. Read only at the start of a line of a TypeScript-family file that
/// imports a GraphQL server package ([`decorator_scan`]).
const ROOT_DECORATORS: &[(&str, &str)] = &[
    ("@Query(", "Query"),
    ("@Mutation(", "Mutation"),
    ("@Subscription(", "Subscription"),
];

/// LA.38: every needle the pre-LA.38 scan minted a noun from, anywhere in any
/// file. Counted only, for the `[graphql-decorators]` census.
const DECORATOR_NEEDLES: &[&str] = &[
    "@Query(",
    "@Mutation(",
    "@Subscription(",
    "@Resolver(",
    "@ResolveField(",
    "@strawberry.type",
    "@strawberry.mutation",
    "ObjectType):",
    "graphene.ObjectType",
];

/// The graphene needles: they sit in a class base list, not at a line start.
const GRAPHENE_NEEDLES: &[&str] = &["ObjectType):", "graphene.ObjectType"];

/// LA.38: packages whose import (a whole quoted module string, LA.26's
/// [`quoted_module_at`] rule) makes a TypeScript-family file a GraphQL server.
const TS_SERVER_MODULES: &[(&str, GqlLib)] = &[
    ("@nestjs/graphql", GqlLib::NestGraphql),
    ("type-graphql", GqlLib::TypeGraphql),
];

/// LA.38: Python GraphQL server packages, imported by a line that starts with
/// `import <pkg>` or `from <pkg>`.
const PY_SERVER_PACKAGES: &[(&str, GqlLib)] = &[
    ("strawberry", GqlLib::Strawberry),
    ("graphene", GqlLib::Graphene),
];

/// LA.38: the languages whose GraphQL server libraries spell resolvers with
/// the decorators and classes read here. Java / Kotlin / C# / PHP / Rust / Go
/// / Dart servers use other spellings (`@QueryMapping`, `@DgsQuery`,
/// HotChocolate attributes), and a Java `@Query(` is Spring Data / Micronaut
/// Data / Room.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DecoratorFamily {
    /// NestJS / TypeGraphQL decorators.
    Ts,
    /// strawberry / graphene root classes and their fields (CB.14).
    Py,
}

/// LA.38: the GraphQL server package a file imports.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum GqlLib {
    NestGraphql,
    TypeGraphql,
    Strawberry,
    Graphene,
}

impl GqlLib {
    fn label(self) -> &'static str {
        match self {
            GqlLib::NestGraphql => "@nestjs/graphql",
            GqlLib::TypeGraphql => "type-graphql",
            GqlLib::Strawberry => "strawberry",
            GqlLib::Graphene => "graphene",
        }
    }
}

/// LA.38: what one file's decorator needles came to, for the
/// `[graphql-decorators]` marker. Each needle occurrence lands in at most one
/// rejection count: a file of another language (`rej_lang`), a file with no
/// GraphQL server import (`rej_import`), or an occurrence off decorator /
/// class position (`rej_position`: a comment, a string, mid-line).
#[derive(Default, Debug)]
struct DecoratorCensus {
    /// Occurrences of [`DECORATOR_NEEDLES`].
    hits: usize,
    lib: Option<GqlLib>,
    /// Root nouns kept, in line order.
    roots: Vec<&'static str>,
    /// Field resolvers read: TypeScript methods under a field decorator,
    /// Python root-class fields (CB.14).
    fields: usize,
    rej_lang: usize,
    rej_import: usize,
    rej_position: usize,
}

impl DecoratorCensus {
    /// The marker line, or `None` for a file with no decorator needle.
    fn marker(&self, lang: &str) -> Option<String> {
        (self.hits > 0).then(|| {
            format!(
                "[graphql-decorators] lang={lang} import={} roots={} fields={} rejected lang={} import={} position={}",
                self.lib.map_or("none", GqlLib::label),
                self.roots.join(","),
                self.fields,
                self.rej_lang,
                self.rej_import,
                self.rej_position
            )
        })
    }
}

/// Decorators whose *following method* is the actual resolver field (e.g. the
/// `getUser` in NestJS `@Query() async getUser()`). The root nouns only
/// recover "Query", never the field a client operation is keyed by, so
/// GRAPHQL_CALLS never pairs.
const RESOLVER_FIELD_DECORATORS: &[&str] =
    &["@Query(", "@Mutation(", "@Subscription(", "@ResolveField("];

/// GraphQL SDL root type openers: the root is a resolver, and so is each
/// field of its block. [`sdl_resolvers`] reads them at the start of a line,
/// also after `extend `.
const SDL_RESOLVER_TYPES: &[&str] = &["type Query {", "type Mutation {", "type Subscription {"];

/// LA.27: template tags whose body is a GraphQL document.
const GQL_TEMPLATE_TAGS: &[&str] = &["gql`", "graphql`"];

/// LA.27: calls whose first argument, when it is a literal, is GraphQL:
/// graphql-tag / Ariadne / gql, graphql-js `buildSchema`, graph-gophers
/// `MustParseSchema` / `ParseSchema`, graphql-ruby `from_definition`.
const GQL_LITERAL_CALLS: &[&str] = &[
    "gql(",
    "graphql(",
    "buildSchema(",
    "MustParseSchema(",
    "ParseSchema(",
    "from_definition(",
];

/// LA.27 (James): the only variable / key names whose literal is read as SDL
/// with no other marker. SDL kept in a differently named variable is not
/// read; `engine::coverage` declares that as a caveat row.
const SDL_VARIABLES: &[&str] = &["typeDefs", "type_defs"];

/// LA.27: the magic comment that marks the literal after it as GraphQL.
const GQL_MAGIC_COMMENT: &str = "/* GraphQL */";

/// LA.27: heredoc openers whose body is GraphQL (graphql-ruby). The body ends
/// at the line whose trimmed text is the delimiter.
const GQL_HEREDOCS: &[(&str, &str)] = &[
    ("<<~GRAPHQL", "GRAPHQL"),
    ("<<-GRAPHQL", "GRAPHQL"),
    ("<<GRAPHQL", "GRAPHQL"),
    ("<<~GQL", "GQL"),
    ("<<-GQL", "GQL"),
    ("<<GQL", "GQL"),
];

/// LA.27: a leading SDL comment that marks a template or triple-quoted
/// literal as GraphQL: Apollo Server 4 opens `typeDefs` templates with it.
const GQL_HASH_MARKS: &[&str] = &["#graphql", "# graphql"];

/// Keywords that precede a method name and must not be mistaken for one.
const METHOD_MODIFIERS: &[&str] = &[
    "async",
    "public",
    "private",
    "protected",
    "static",
    "readonly",
    "get",
    "set",
    "constructor",
    "return",
    "if",
    "for",
    "while",
    "await",
    "function",
];

/// Client-side GRAPHQL_OPERATION nodes: the operation needles
/// ([`scan_operation_needles`]) and the named operations of `` gql` ``
/// templates ([`extract_gql_template_operations`]).
///
/// `path` selects CJ.1b's literal / comment guard: in a Rust or Python file an
/// operation needle or `` gql` `` tag that starts in a string literal or
/// comment mints nothing (`""` = no guard). Resolver / SDL extraction is never
/// guarded: an SDL schema legitimately lives in a string literal. fired_on,
/// once per call that refused one:
/// `[code-guard] graphql_op lang=<rust|python> dropped=<n> path=<path>`.
pub fn extract_graphql_operation_nodes(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
) -> GraphqlNodes {
    let mut nodes = Vec::new();
    let mut nav = CodeNav::default();
    let mut anchors = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut guard = LazyGuard::new(path, source);

    let (hits, tally) = scan_operation_needles(source, &mut guard);
    if graphql_debug()
        && let Some(marker) = tally.marker()
    {
        eprintln!("{marker}");
    }
    for (op_name, line) in hits {
        if seen.insert(op_name.clone()) {
            let qname = format!("graphql_op:{op_name}");
            let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::GRAPHQL_OPERATION, &qname);
            nodes.push(Node {
                id,
                repo,
                confidence: Confidence::Medium,
                cells: vec![],
            });
            nav.record(id, &op_name, &qname, node_kind::GRAPHQL_OPERATION, Some(module_id));
            anchors.push(Anchor { node: id, line });
        }
    }

    let templates = extract_gql_template_operations(source, &mut guard);
    guard.report("graphql_op");
    for (name, at) in templates {
        if seen.insert(name.clone()) {
            let qname = format!("graphql_op:{name}");
            let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::GRAPHQL_OPERATION, &qname);
            nodes.push(Node {
                id,
                repo,
                confidence: Confidence::Strong,
                cells: vec![],
            });
            nav.record(id, &name, &qname, node_kind::GRAPHQL_OPERATION, Some(module_id));
            anchors.push(Anchor { node: id, line: line_of(source, at) });
        }
    }

    GraphqlNodes { nodes, nav, anchors }
}

/// CODE mode, for every code file of every language
/// (`engine::extract::apply_cross_cutting_extractors`, `lang` from
/// `detect_language`): the decorator family ([`decorator_scan`], LA.38) and
/// SDL only inside a GraphQL-marked literal ([`gql_literal_regions`], LA.27).
/// A comment or an unmarked string that holds `type Query {` mints nothing,
/// and neither does a decorator needle outside a TypeScript / Python file
/// that imports a GraphQL server package. A `.graphql` / `.gql` body goes
/// through [`extract_graphql_sdl_file_nodes`] instead.
pub fn extract_graphql_resolver_nodes(
    source: &str,
    lang: &str,
    module_id: NodeId,
    repo: RepoId,
) -> GraphqlNodes {
    // (name, 0-indexed line the name was read from): root nouns and the
    // methods under field decorators, then the roots and fields of SDL held
    // in marked literals, in source order. Fields carry the operation name a
    // client `gql query getUser` pairs against.
    let (mut names, census) = decorator_scan(source, lang);
    let embedded = embedded_sdl(source);
    if graphql_debug() {
        for marker in [census.marker(lang), embedded.marker()].into_iter().flatten() {
            eprintln!("{marker}");
        }
    }
    names.extend(embedded.hits.into_iter().map(|h| (h.name, h.line)));
    names.sort_by_key(|&(_, line)| line);

    resolver_nodes(names, module_id, repo)
}

/// LA.38: the GraphQL decorator family of one code file, as (name, 0-indexed
/// line) pairs in line order, plus the census for the `[graphql-decorators]`
/// marker. It runs only for a TypeScript-family or Python file
/// ([`decorator_family`]) that imports a GraphQL server package
/// ([`graphql_server_lib`]), and reads only at decorator / class position:
/// - TypeScript (NestJS / TypeGraphQL): a line starting with `@Query(` /
///   `@Mutation(` / `@Subscription(` names that root type, and the method
///   under a field decorator ([`decorator_method_names`]) is a field;
/// - Python: `@strawberry.type` on `class Query:`, or graphene
///   `class Query(graphene.ObjectType):`, names that root type, and the
///   root class's field resolvers ([`py_field_resolvers`], CB.14) are fields
///   under their schema names.
///
/// Every noun is a root type: `@Resolver(`, `@ResolveField(`,
/// `@strawberry.type`, `@strawberry.mutation` and `ObjectType` name nothing.
fn decorator_scan(source: &str, lang: &str) -> (Vec<(String, u32)>, DecoratorCensus) {
    let mut census = DecoratorCensus {
        hits: DECORATOR_NEEDLES.iter().map(|n| source.matches(n).count()).sum(),
        ..DecoratorCensus::default()
    };
    let Some(family) = decorator_family(lang) else {
        census.rej_lang = census.hits;
        return (Vec::new(), census);
    };
    let Some(lib) = graphql_server_lib(source, family) else {
        census.rej_import = census.hits;
        return (Vec::new(), census);
    };
    census.lib = Some(lib);
    let lines: Vec<&str> = source.lines().collect();
    census.rej_position = misplaced_needles(&lines);
    let roots = match family {
        DecoratorFamily::Ts => ts_root_decorators(&lines),
        DecoratorFamily::Py => py_root_classes(&lines, lib),
    };
    census.roots = roots.iter().map(|&(root, _)| root).collect();
    let mut names: Vec<(String, u32)> = roots
        .into_iter()
        .map(|(root, line)| (root.to_string(), line))
        .collect();
    let fields = match family {
        DecoratorFamily::Ts => decorator_method_names(source),
        DecoratorFamily::Py => py_field_resolvers(&lines, lib),
    };
    census.fields = fields.len();
    names.extend(fields);
    names.sort_by_key(|&(_, line)| line);
    (names, census)
}

/// LA.38: the decorator family of a `detect_language` tag (`.ts` / `.tsx` /
/// `.js` / `.jsx` are `typescript`, `.component.ts` is `angular`).
fn decorator_family(lang: &str) -> Option<DecoratorFamily> {
    match lang {
        "typescript" | "react" | "angular" | "vue" => Some(DecoratorFamily::Ts),
        "python" => Some(DecoratorFamily::Py),
        _ => None,
    }
}

/// LA.38: the GraphQL server package the file imports. TypeScript: a quoted
/// `@nestjs/graphql` / `type-graphql` module string, so a multi-line import
/// counts and a REST controller importing `Query` from `@nestjs/common` does
/// not. Python: a line starting with `import` / `from` and the package name
/// as a whole token (`import strawberry as sb`, `from graphene import ...`).
fn graphql_server_lib(source: &str, family: DecoratorFamily) -> Option<GqlLib> {
    match family {
        DecoratorFamily::Ts => {
            let b = source.as_bytes();
            b.iter()
                .enumerate()
                .filter(|&(_, &c)| c == b'\'' || c == b'"')
                .find_map(|(i, _)| {
                    TS_SERVER_MODULES
                        .iter()
                        .find(|(module, _)| quoted_module_at(b, i + 1, module))
                        .map(|&(_, lib)| lib)
                })
        }
        DecoratorFamily::Py => source.lines().find_map(|line| {
            let t = line.trim_start();
            let rest = t
                .strip_prefix("import ")
                .or_else(|| t.strip_prefix("from "))?
                .trim_start();
            PY_SERVER_PACKAGES
                .iter()
                .find(|(package, _)| {
                    rest.strip_prefix(package)
                        .is_some_and(|after| !after.bytes().next().is_some_and(is_ident_byte))
                })
                .map(|&(_, lib)| lib)
        }),
    }
}

/// LA.38: needle occurrences off decorator / class position: not at the
/// start of their line (a graphene needle also counts inside a `class` line's
/// base list).
fn misplaced_needles(lines: &[&str]) -> usize {
    lines
        .iter()
        .map(|line| {
            let trimmed = line.trim_start();
            let lead = line.len() - trimmed.len();
            let class_line = trimmed.starts_with("class ");
            DECORATOR_NEEDLES
                .iter()
                .map(|&needle| {
                    let positioned = |at: usize| {
                        at == lead || (class_line && GRAPHENE_NEEDLES.contains(&needle))
                    };
                    line.match_indices(needle)
                        .filter(|&(at, _)| !positioned(at))
                        .count()
                })
                .sum::<usize>()
        })
        .sum()
}

fn line_u32(i: usize) -> u32 {
    u32::try_from(i).unwrap_or(u32::MAX)
}

/// Push `root` unless the file already has it: one node per root type,
/// anchored at its first qualifying line.
fn push_root(roots: &mut Vec<(&'static str, u32)>, root: &'static str, line: usize) {
    if !roots.iter().any(|&(r, _)| r == root) {
        roots.push((root, line_u32(line)));
    }
}

/// LA.38: the root types NestJS / TypeGraphQL decorators name, from lines that
/// start with `@Query(` / `@Mutation(` / `@Subscription(`. CB.4: a root noun is
/// type-level, so it anchors at its class declaration line
/// ([`class_line_before`]), inside the class and outside every method: the
/// TypeScript parser's span of a decorated method opens at its first
/// decorator, so the `@Query(` line itself now lies inside the field method.
fn ts_root_decorators(lines: &[&str]) -> Vec<(&'static str, u32)> {
    let mut roots = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim_start();
        if let Some(&(_, root)) = ROOT_DECORATORS.iter().find(|(d, _)| t.starts_with(d)) {
            push_root(&mut roots, root, class_line_before(lines, i));
        }
    }
    roots
}

/// CB.4: the nearest line at or above `i` that declares a class
/// ([`is_class_line`]), or `i` when none does (a decorator outside any class).
/// A line scan: this extractor reads text, not an AST.
fn class_line_before(lines: &[&str], i: usize) -> usize {
    lines
        .get(..=i)
        .and_then(|above| above.iter().rposition(|l| is_class_line(l)))
        .unwrap_or(i)
}

/// A TypeScript class declaration line: trimmed, past any of `export `,
/// `default `, `abstract ` (in that order), it starts with `class `.
fn is_class_line(line: &str) -> bool {
    let mut t = line.trim_start();
    for word in ["export ", "default ", "abstract "] {
        if let Some(rest) = t.strip_prefix(word) {
            t = rest.trim_start();
        }
    }
    t.starts_with("class ")
}

/// LA.38: the root types Python classes declare, anchored at the class line
/// of each root's first declaration ([`py_root_class_sites`]).
fn py_root_classes(lines: &[&str], lib: GqlLib) -> Vec<(&'static str, u32)> {
    let mut roots = Vec::new();
    for (root, line) in py_root_class_sites(lines, lib) {
        push_root(&mut roots, root, line);
    }
    roots
}

/// LA.38: every Python root class declaration, as (root, 0-indexed class
/// line) in line order. strawberry: `@strawberry.type` (bare or called), then
/// within 4 lines, past blank lines and further decorators, `class <Root>`.
/// graphene: `class <Root>(...)` whose base list holds the token `ObjectType`.
fn py_root_class_sites(lines: &[&str], lib: GqlLib) -> Vec<(&'static str, usize)> {
    let mut sites = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        match lib {
            GqlLib::Strawberry => {
                let Some(after) = t.strip_prefix("@strawberry.type") else {
                    continue;
                };
                if !(after.is_empty() || after.starts_with(['(', ' ', '\t', '#'])) {
                    continue;
                }
                let class_line = lines
                    .iter()
                    .enumerate()
                    .skip(i + 1)
                    .take(4)
                    .find(|(_, l)| {
                        let t = l.trim();
                        !t.is_empty() && !t.starts_with('@')
                    });
                if let Some((j, l)) = class_line
                    && let Some((root, _)) = class_root(l.trim())
                {
                    sites.push((root, j));
                }
            }
            GqlLib::Graphene => {
                if let Some(root) = graphene_root(t) {
                    sites.push((root, i));
                }
            }
            GqlLib::NestGraphql | GqlLib::TypeGraphql => {}
        }
    }
    sites
}

/// The root type a trimmed `class <Ident>` line declares, and the text after
/// the identifier. `None` for any other class.
fn class_root(t: &str) -> Option<(&'static str, &str)> {
    let rest = t.strip_prefix("class ")?.trim_start();
    let end = rest
        .bytes()
        .position(|c| !is_ident_byte(c))
        .unwrap_or(rest.len());
    let ident = rest.get(..end)?;
    let root = ROOT_TYPES.iter().copied().find(|r| *r == ident)?;
    Some((root, rest.get(end..)?))
}

/// A graphene root: `class Query(graphene.ObjectType):` or
/// `class Query(ObjectType):`, the token `ObjectType` in the base list up to
/// the first `)` on the line.
fn graphene_root(t: &str) -> Option<&'static str> {
    let (root, after) = class_root(t)?;
    let bases = after.trim_start().strip_prefix('(')?;
    let bases = bases.get(..bases.find(')')?)?;
    let b = bases.as_bytes();
    let token = "ObjectType";
    bases
        .match_indices(token)
        .any(|(at, _)| {
            !ident_byte_before(b, at) && !b.get(at + token.len()).is_some_and(|&c| is_ident_byte(c))
        })
        .then_some(root)
}

/// CB.14: strawberry decorators and attribute calls that declare a field.
const STRAWBERRY_FIELD_CALLS: &[&str] = &[
    "strawberry.field",
    "strawberry.mutation",
    "strawberry.subscription",
];

/// CB.14: graphene field types a root class mounts as an attribute, called
/// bare (`String(`) or as `graphene.<Type>(`. Any callee whose last segment
/// is `Field` (`graphene.Field(`, a mutation's `CreateUser.Field()`,
/// `relay.Node.Field()`) or ends in `ConnectionField` (graphene's relay
/// `ConnectionField` and the graphene-django / graphene-sqlalchemy
/// subclasses) is a field too, whatever its prefix.
const GRAPHENE_FIELD_TYPES: &[&str] = &[
    "Field",
    "List",
    "NonNull",
    "String",
    "Int",
    "Float",
    "Boolean",
    "ID",
    "JSONString",
    "DateTime",
    "Date",
    "Time",
    "Decimal",
    "UUID",
    "BigInt",
    "Base64",
    "Dynamic",
];

/// CB.14: the schema-config spellings that turn camelCasing off, compared
/// with whitespace removed: strawberry's `StrawberryConfig(auto_camel_case=
/// False)` and graphene's `Schema(..., auto_camelcase=False)`.
const PY_CAMEL_OFF: &[&str] = &["auto_camel_case=False", "auto_camelcase=False"];

/// CB.14: how many lines a multi-line decorator or field call is read for
/// its `name=` keyword before the scan gives up on an unbalanced bracket.
const PY_CALL_WINDOW: usize = 32;

/// CB.14: the field resolvers of a Python file's root classes
/// ([`py_root_class_sites`]), as (schema field name, 0-indexed anchor line)
/// in line order. Only a class body's first-level members count
/// ([`py_class_members`]); an object type (`class User`) mints nothing.
/// - strawberry: a `@strawberry.field` / `.mutation` / `.subscription`
///   member (bare or called) followed, past further decorators, by
///   `def <name>(` or `async def <name>(`, anchored at that `def` line (the
///   Python parser's method POSITION opens at `def`, not at its decorators,
///   so the anchor lands inside the method); and an attribute
///   `<name>[: T] = strawberry.field(...)`, anchored at its line.
/// - graphene: an attribute `<name> = <field type>(...)`
///   ([`graphene_field_call`]), anchored at the block's
///   `def resolve_<name>(` when there is one (so HANDLED_BY lands on the
///   resolver method), else at the attribute line.
///
/// The name is the schema field ([`schema_name`]): an explicit `name="..."`
/// keyword, else the camelCased Python name unless the file turns camelCasing
/// off ([`PY_CAMEL_OFF`]).
fn py_field_resolvers(lines: &[&str], lib: GqlLib) -> Vec<(String, u32)> {
    let camel = !lines.iter().any(|l| {
        let compact: String = l.chars().filter(|c| !c.is_whitespace()).collect();
        PY_CAMEL_OFF.iter().any(|off| compact.contains(off))
    });
    let mut class_lines: Vec<usize> = py_root_class_sites(lines, lib)
        .into_iter()
        .map(|(_, line)| line)
        .collect();
    class_lines.sort_unstable();
    class_lines.dedup();
    let mut fields = Vec::new();
    for class_line in class_lines {
        let members = py_class_members(lines, class_line);
        match lib {
            GqlLib::Strawberry => strawberry_fields(lines, &members, camel, &mut fields),
            GqlLib::Graphene => graphene_fields(lines, &members, camel, &mut fields),
            GqlLib::NestGraphql | GqlLib::TypeGraphql => {}
        }
    }
    fields.sort_by_key(|&(_, line)| line);
    fields
}

/// CB.14: strawberry field methods and attributes among a root class's
/// first-level `members`.
fn strawberry_fields(
    lines: &[&str],
    members: &[usize],
    camel: bool,
    out: &mut Vec<(String, u32)>,
) {
    for (m, &i) in members.iter().enumerate() {
        let Some(t) = lines.get(i).map(|l| l.trim()) else {
            continue;
        };
        if let Some(after) = t.strip_prefix('@') {
            let Some(rest) = strawberry_call(after) else {
                continue;
            };
            if !(rest.is_empty() || rest.starts_with(['(', ' ', '\t', '#'])) {
                continue;
            }
            // Past stacked decorators, the next member is the method.
            let def = members
                .get(m + 1..)
                .unwrap_or_default()
                .iter()
                .filter_map(|&j| Some((j, lines.get(j)?.trim())))
                .find(|(_, l)| !l.starts_with('@'));
            let Some((j, name)) = def.and_then(|(j, l)| Some((j, py_def_name(l)?))) else {
                continue;
            };
            let explicit = call_name_kwarg(lines, i, rest);
            out.push((schema_name(&name, explicit, camel), line_u32(j)));
        } else if let Some((name, rhs)) = py_assignment(t)
            && let Some(rest) = strawberry_call(rhs)
            && rest.trim_start().starts_with('(')
        {
            let explicit = call_name_kwarg(lines, i, rest.trim_start());
            out.push((schema_name(name, explicit, camel), line_u32(i)));
        }
    }
}

/// The text after a [`STRAWBERRY_FIELD_CALLS`] prefix, when `s` starts with
/// one as a whole dotted name (`strawberry.field_x` is not one).
fn strawberry_call(s: &str) -> Option<&str> {
    STRAWBERRY_FIELD_CALLS.iter().find_map(|call| {
        s.strip_prefix(call)
            .filter(|rest| !rest.bytes().next().is_some_and(|c| is_ident_byte(c) || c == b'.'))
    })
}

/// CB.14: graphene field attributes among a root class's first-level
/// `members`, each anchored at its `resolve_<name>` method when the block
/// has one.
fn graphene_fields(lines: &[&str], members: &[usize], camel: bool, out: &mut Vec<(String, u32)>) {
    let resolvers: Vec<(String, usize)> = members
        .iter()
        .filter_map(|&j| {
            let name = py_def_name(lines.get(j)?.trim())?;
            Some((name.strip_prefix("resolve_")?.to_string(), j))
        })
        .collect();
    for &i in members {
        let Some(t) = lines.get(i).map(|l| l.trim()) else {
            continue;
        };
        let Some((name, rhs)) = py_assignment(t) else {
            continue;
        };
        let Some((callee, call)) = py_callee(rhs) else {
            continue;
        };
        if !graphene_field_call(callee) {
            continue;
        }
        let explicit = call_name_kwarg(lines, i, call);
        let anchor = resolvers
            .iter()
            .find(|(field, _)| field == name)
            .map_or(i, |&(_, j)| j);
        out.push((schema_name(name, explicit, camel), line_u32(anchor)));
    }
}

/// CB.14: a dotted callee graphene mounts as a field ([`GRAPHENE_FIELD_TYPES`]).
fn graphene_field_call(callee: &str) -> bool {
    let (prefix, last) = callee.rsplit_once('.').unwrap_or(("", callee));
    last == "Field"
        || last.ends_with("ConnectionField")
        || ((prefix.is_empty() || prefix == "graphene") && GRAPHENE_FIELD_TYPES.contains(&last))
}

/// CB.14: the schema name of a Python field: the explicit `name=` when
/// given, else [`camel_case`] of the Python name, or the Python name itself
/// when the file turns camelCasing off.
fn schema_name(py_name: &str, explicit: Option<String>, camel: bool) -> String {
    match explicit {
        Some(name) => name,
        None if camel => camel_case(py_name),
        None => py_name.to_string(),
    }
}

/// CB.14: strawberry's and graphene's `to_camel_case`, which are the same
/// function: split on `_`, keep the first component as written, capitalise
/// every later one (first char upper, the rest lower, Python's
/// `str.capitalize`) and write an empty one as `_`. So `current_user` ->
/// `currentUser`, `field_2` -> `field2`, `user_ID` -> `userId`, `_private` ->
/// `Private`, `a__b` -> `a_B`.
fn camel_case(s: &str) -> String {
    let mut parts = s.split('_');
    let mut out = parts.next().unwrap_or_default().to_string();
    for part in parts {
        let mut chars = part.chars();
        match chars.next() {
            None => out.push('_'),
            Some(first) => {
                out.extend(first.to_uppercase());
                out.extend(chars.flat_map(char::to_lowercase));
            }
        }
    }
    out
}

/// CB.14: the 0-indexed lines of a class body's first-level members: the
/// lines after `class_line` indented deeper than it, up to the first
/// non-blank, non-comment line at or above its indent, at the indent of the
/// body's first statement. Continuation lines of a bracketed expression and
/// the inside of a triple-quoted string neither end the body nor count. A
/// line scan: indent is the width of the leading whitespace, tabs and spaces
/// alike.
fn py_class_members(lines: &[&str], class_line: usize) -> Vec<usize> {
    let indent = |l: &str| l.len() - l.trim_start().len();
    let Some(class_indent) = lines.get(class_line).map(|l| indent(l)) else {
        return Vec::new();
    };
    let mut members = Vec::new();
    let mut member_indent = None;
    let mut depth = 0i32;
    let mut in_triple: Option<&str> = None;
    for (j, l) in lines.iter().enumerate().skip(class_line + 1) {
        if let Some(quote) = in_triple {
            if let Some(at) = l.find(quote) {
                in_triple = None;
                depth = (depth + py_bracket_delta(l.get(at + quote.len()..).unwrap_or_default())).max(0);
            }
            continue;
        }
        if depth > 0 {
            depth = (depth + py_bracket_delta(l)).max(0);
            in_triple = open_triple_quote(l);
            continue;
        }
        let t = l.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let ind = indent(l);
        if ind <= class_indent {
            break;
        }
        if *member_indent.get_or_insert(ind) == ind {
            members.push(j);
        }
        depth = py_bracket_delta(l).max(0);
        in_triple = open_triple_quote(l);
    }
    members
}

/// The triple quote a line leaves open: an odd count of `"""` (or `'''`).
fn open_triple_quote(line: &str) -> Option<&'static str> {
    ["\"\"\"", "'''"]
        .into_iter()
        .find(|q| line.matches(q).count() % 2 == 1)
}

/// The net bracket depth one line of Python opens, ignoring brackets inside a
/// one-line string literal and after a `#` comment.
fn py_bracket_delta(line: &str) -> i32 {
    let b = line.as_bytes();
    let mut depth = 0i32;
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        match c {
            b'#' => break,
            b'\'' | b'"' => i = skip_py_string(b, i),
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    depth
}

/// The offset of the quote closing the string literal that opens at `at`
/// (a backslash escapes the next byte), or the last offset when it does not
/// close.
fn skip_py_string(b: &[u8], at: usize) -> usize {
    let Some(&quote) = b.get(at) else {
        return at;
    };
    let mut i = at + 1;
    while let Some(&c) = b.get(i) {
        if c == b'\\' {
            i += 2;
            continue;
        }
        if c == quote {
            return i;
        }
        i += 1;
    }
    b.len().saturating_sub(1)
}

/// `def <name>(` or `async def <name>(` on a trimmed line: the name.
fn py_def_name(t: &str) -> Option<String> {
    let t = t
        .strip_prefix("async")
        .filter(|rest| rest.starts_with([' ', '\t']))
        .map_or(t, str::trim_start);
    let rest = t.strip_prefix("def")?;
    if !rest.starts_with([' ', '\t']) {
        return None;
    }
    let (name, after) = py_ident(rest.trim_start())?;
    after.trim_start().starts_with('(').then(|| name.to_string())
}

/// The Python identifier `s` starts with, and the text after it.
fn py_ident(s: &str) -> Option<(&str, &str)> {
    let end = s
        .bytes()
        .position(|c| !(c.is_ascii_alphanumeric() || c == b'_'))
        .unwrap_or(s.len());
    if end == 0 || s.bytes().next().is_some_and(|c| c.is_ascii_digit()) {
        return None;
    }
    Some((s.get(..end)?, s.get(end..)?))
}

/// A trimmed assignment `<name> = <rhs>` or `<name>: <T> = <rhs>`: the name
/// and the trimmed right-hand side. `==` is a comparison, not one.
fn py_assignment(t: &str) -> Option<(&str, &str)> {
    let (name, rest) = py_ident(t)?;
    let rest = rest.trim_start();
    let b = rest.as_bytes();
    let eq = if rest.starts_with(':') {
        // The annotation runs to the first `=` outside its brackets.
        let mut depth = 0i32;
        let mut found = None;
        for (i, &c) in b.iter().enumerate() {
            match c {
                b'[' | b'(' | b'{' => depth += 1,
                b']' | b')' | b'}' => depth -= 1,
                b'=' if depth == 0 => {
                    found = Some(i);
                    break;
                }
                _ => {}
            }
        }
        found?
    } else if rest.starts_with('=') {
        0
    } else {
        return None;
    };
    if b.get(eq + 1) == Some(&b'=') {
        return None;
    }
    Some((name, rest.get(eq + 1..)?.trim_start()))
}

/// A call on a trimmed right-hand side: the dotted callee and the text from
/// its `(` on.
fn py_callee(rhs: &str) -> Option<(&str, &str)> {
    let end = rhs
        .bytes()
        .position(|c| !(c.is_ascii_alphanumeric() || c == b'_' || c == b'.'))
        .unwrap_or(rhs.len());
    let callee = rhs.get(..end)?;
    let call = rhs.get(end..)?.trim_start();
    let valid = callee
        .split('.')
        .all(|seg| py_ident(seg).is_some_and(|(_, after)| after.is_empty()));
    (valid && call.starts_with('(')).then_some((callee, call))
}

/// CB.14: the quoted `name="..."` keyword of the call on line `i` whose text
/// from `(` on is `first`, read across its continuation lines (at most
/// [`PY_CALL_WINDOW`]). `None` when there is no call, no such keyword, or its
/// value is no GraphQL name.
fn call_name_kwarg(lines: &[&str], i: usize, first: &str) -> Option<String> {
    if !first.starts_with('(') {
        return None;
    }
    let mut text = first.to_string();
    let mut depth = py_bracket_delta(first);
    for l in lines.iter().skip(i + 1).take(PY_CALL_WINDOW) {
        if depth <= 0 {
            break;
        }
        text.push('\n');
        text.push_str(l);
        depth += py_bracket_delta(l);
    }
    py_name_kwarg(&text)
}

/// The `name=` keyword argument of the call `call` opens (it starts at `(`),
/// read at the call's own bracket depth, when its value is a quoted GraphQL
/// name: `(User, name="me")` -> `me`; a `name=` inside a nested call, a
/// comment or a string, `first_name=`, and `name=some_var` are not it.
fn py_name_kwarg(call: &str) -> Option<String> {
    let b = call.as_bytes();
    let mut depth = 0i32;
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        match c {
            b'#' => {
                i = b.iter().skip(i).position(|&x| x == b'\n').map_or(b.len(), |p| i + p);
                continue;
            }
            b'\'' | b'"' => i = skip_py_string(b, i),
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth <= 0 {
                    return None;
                }
            }
            _ if depth == 1 && is_ident_byte(c) && !ident_byte_before(b, i) => {
                let (ident, after) = py_ident(call.get(i..)?).unwrap_or(("", ""));
                if ident == "name"
                    && let Some(value) = after.trim_start().strip_prefix('=')
                    && !value.starts_with('=')
                {
                    // The call's own `name=`: its quoted literal, or no
                    // explicit name at all (`name=some_var`).
                    return quoted_literal(value.trim_start())
                        .filter(|l| py_ident(l).is_some_and(|(_, rest)| rest.is_empty()))
                        .map(str::to_string);
                }
                i += ident.len().max(1);
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The body of the one-line quoted literal `s` opens (`"me"` / `'me'`).
fn quoted_literal(s: &str) -> Option<&str> {
    let b = s.as_bytes();
    let quote = b.first().copied().filter(|&q| q == b'"' || q == b'\'')?;
    let close = skip_py_string(b, 0);
    (close > 0 && b.get(close) == Some(&quote))
        .then(|| s.get(1..close))
        .flatten()
}

/// WHOLE-FILE mode for a routed `.graphql` / `.gql` schema
/// (`engine::route::parse_repo_files`, LA.27): the whole text is SDL, so every
/// root type block and its fields count. No decorator patterns: those are
/// code, and never occur in SDL.
pub fn extract_graphql_sdl_file_nodes(
    source: &str,
    module_id: NodeId,
    repo: RepoId,
) -> GraphqlNodes {
    let names = sdl_resolvers(source, 0)
        .into_iter()
        .map(|h| (h.name, h.line))
        .collect();
    resolver_nodes(names, module_id, repo)
}

/// One GRAPHQL_RESOLVER per distinct name, anchored at the line of its first
/// occurrence in `names`.
fn resolver_nodes(names: Vec<(String, u32)>, module_id: NodeId, repo: RepoId) -> GraphqlNodes {
    let mut nodes = Vec::new();
    let mut nav = CodeNav::default();
    let mut anchors = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (resolver_name, line) in names {
        if seen.insert(resolver_name.clone()) {
            let qname = format!("graphql_resolver:{resolver_name}");
            let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::GRAPHQL_RESOLVER, &qname);
            nodes.push(Node {
                id,
                repo,
                confidence: Confidence::Medium,
                cells: vec![],
            });
            nav.record(id, &resolver_name, &qname, node_kind::GRAPHQL_RESOLVER, Some(module_id));
            anchors.push(Anchor { node: id, line });
        }
    }
    GraphqlNodes { nodes, nav, anchors }
}

/// The method following a `@Query()` / `@Mutation()` / `@Subscription()` /
/// `@ResolveField()` decorator, with the 0-indexed line it was read from (the
/// method declaration line, not the decorator's).
fn decorator_method_names(source: &str) -> Vec<(String, u32)> {
    let mut names = Vec::new();
    let lines: Vec<&str> = source.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if !RESOLVER_FIELD_DECORATORS
            .iter()
            .any(|d| trimmed.starts_with(d))
        {
            continue;
        }
        // The method usually sits on a following line; allow same-line
        // (`@ResolveField() name() {}`) and skip stacked decorators.
        for (j, candidate) in lines.iter().enumerate().skip(i).take(4) {
            let t = candidate.trim();
            if t.is_empty() || t.starts_with('@') {
                continue;
            }
            if let Some(name) = method_name_from_line(t) {
                names.push((name, line_u32(j)));
                break;
            }
        }
    }
    names
}

/// One resolver name read from SDL: a root type (`Query`) or a field of one.
struct SdlHit {
    name: String,
    /// 0-indexed source line.
    line: u32,
    root: bool,
}

/// The root type a trimmed SDL line opens (`type Query {`, also after
/// `extend `), by name.
fn sdl_root(trimmed: &str) -> Option<&'static str> {
    let t = trimmed
        .strip_prefix("extend ")
        .map_or(trimmed, str::trim_start);
    SDL_RESOLVER_TYPES
        .iter()
        .copied()
        .find(|opener| t.starts_with(opener))
        .map(|opener| opener.trim_start_matches("type ").trim_end_matches(" {"))
}

/// The SDL block scan: each root type opened at the start of a line, and each
/// field inside its block up to the line that starts with `}`. `text` is SDL
/// (a whole schema file, or one marked literal's body); its first line is
/// source line `base_line`, so anchors stay source lines. A root opener in
/// the middle of a line never counts.
fn sdl_resolvers(text: &str, base_line: u32) -> Vec<SdlHit> {
    let mut hits = Vec::new();
    let mut in_root = false;
    for (i, line) in text.lines().enumerate() {
        let line_no = base_line.saturating_add(u32::try_from(i).unwrap_or(u32::MAX));
        let trimmed = line.trim();
        if in_root {
            if trimmed.starts_with('}') {
                in_root = false;
            } else if let Some(field) = sdl_field_name(trimmed) {
                hits.push(SdlHit {
                    name: field,
                    line: line_no,
                    root: false,
                });
            }
            continue;
        }
        if let Some(root) = sdl_root(trimmed) {
            hits.push(SdlHit {
                name: root.to_string(),
                line: line_no,
                root: true,
            });
            in_root = true;
        }
    }
    hits
}

/// SDL read from one code file's GraphQL-marked literals, plus the counts the
/// `[graphql-sdl-code]` marker reports.
#[derive(Default)]
struct EmbeddedSdl {
    hits: Vec<SdlHit>,
    literals: usize,
    /// Root type openers outside every marked literal: what the pre-LA.27
    /// scan minted from (comments, unmarked strings).
    unmarked_roots: usize,
}

impl EmbeddedSdl {
    /// The marker line, or `None` for a file with no marked literal and no
    /// root opener.
    fn marker(&self) -> Option<String> {
        if self.literals == 0 && self.unmarked_roots == 0 {
            return None;
        }
        let mut roots: Vec<&str> = Vec::new();
        for h in self.hits.iter().filter(|h| h.root) {
            if !roots.contains(&h.name.as_str()) {
                roots.push(&h.name);
            }
        }
        Some(format!(
            "[graphql-sdl-code] literals={} roots={} fields={} unmarked_roots={}",
            self.literals,
            roots.join(","),
            self.hits.iter().filter(|h| !h.root).count(),
            self.unmarked_roots
        ))
    }
}

fn embedded_sdl(source: &str) -> EmbeddedSdl {
    let regions = gql_literal_regions(source);
    let mut out = EmbeddedSdl {
        literals: regions.len(),
        ..EmbeddedSdl::default()
    };
    for &(start, end) in &regions {
        if let Some(body) = source.get(start..end) {
            out.hits.extend(sdl_resolvers(body, line_of(source, start)));
        }
    }
    out.unmarked_roots = SDL_RESOLVER_TYPES
        .iter()
        .flat_map(|opener| source.match_indices(opener))
        .filter(|&(at, _)| !regions.iter().any(|&(s, e)| s <= at && at < e))
        .count();
    out
}

fn skip_ascii_ws(b: &[u8], mut i: usize) -> usize {
    while b.get(i).is_some_and(|c| c.is_ascii_whitespace()) {
        i += 1;
    }
    i
}

/// Offset of the first unescaped `close` at or after `from`. A one-line
/// literal gives up at a newline.
fn find_close(b: &[u8], from: usize, close: &[u8], one_line: bool) -> Option<usize> {
    let mut i = from;
    while let Some(&c) = b.get(i) {
        if c == b'\\' {
            i += 2;
            continue;
        }
        if one_line && c == b'\n' {
            return None;
        }
        if b.get(i..).is_some_and(|rest| rest.starts_with(close)) {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// The body of the literal that opens at byte `at`, as `(start, end)`: a
/// backtick template (multi-line), a `"""` / `'''` block, or a one-line `"` /
/// `'` string. `None` when no literal opens there or it never closes. Both
/// bounds sit on ASCII delimiters, so slicing the source there is char-safe.
fn literal_body(b: &[u8], at: usize) -> Option<(usize, usize)> {
    let q = *b.get(at)?;
    match q {
        b'`' => find_close(b, at + 1, b"`", false).map(|end| (at + 1, end)),
        b'"' | b'\'' => {
            let triple = [q, q, q];
            if b.get(at..at + 3) == Some(&triple[..]) {
                find_close(b, at + 3, &triple, false).map(|end| (at + 3, end))
            } else {
                find_close(b, at + 1, &[q], true).map(|end| (at + 1, end))
            }
        }
        _ => None,
    }
}

/// `at` past an optional `/* ... */` comment and the whitespace around it.
fn skip_block_comment(source: &str, at: usize) -> usize {
    let b = source.as_bytes();
    let i = skip_ascii_ws(b, at);
    if b.get(i..).is_some_and(|rest| rest.starts_with(b"/*"))
        && let Some(close) = source.get(i..).and_then(|rest| rest.find("*/"))
    {
        return skip_ascii_ws(b, i + close + 2);
    }
    i
}

/// Where the value of a `typeDefs` / `type_defs` binding whose name ends at
/// `after` starts: after `=`, after an object key's `:`, or after a
/// `: <type> =` annotation. `None` for any other use of the name.
fn binding_value(source: &str, after: usize) -> Option<usize> {
    let b = source.as_bytes();
    let i = skip_ascii_ws(b, after);
    let assign = |eq: usize| {
        (b.get(eq) == Some(&b'=') && !matches!(b.get(eq + 1), Some(b'=' | b'>')))
            .then(|| skip_block_comment(source, eq + 1))
    };
    match b.get(i)? {
        b'=' => assign(i),
        b':' if b.get(i + 1) != Some(&b':') => {
            let value = skip_block_comment(source, i + 1);
            if literal_body(b, value).is_some() {
                return Some(value);
            }
            // `typeDefs: string = ...`: step over the annotation on this line.
            let mut j = value;
            while let Some(&c) = b.get(j) {
                if c == b'=' {
                    return assign(j);
                }
                if !(is_ident_byte(c) || b" \t.<>[]|,?&".contains(&c)) {
                    return None;
                }
                j += 1;
            }
            None
        }
        _ => None,
    }
}

/// The bodies of the GraphQL-marked literals in a code file, as byte ranges
/// sorted by start and deduplicated. LA.27: this is the only place code-mode
/// SDL is read from. A tag, call, name or heredoc opener must follow a
/// non-identifier byte (`buildSchema(` is not `rebuildSchema(`):
/// - a `` gql` `` / `` graphql` `` template tag, not after a `.`;
/// - `/* GraphQL */` before a literal;
/// - a literal first argument of `gql(` / `graphql(` / `buildSchema(` /
///   `MustParseSchema(` / `ParseSchema(` / `from_definition(`;
/// - a literal bound to a `typeDefs` / `type_defs` variable or key (a tag or
///   call in that position is caught by the rules above);
/// - a GRAPHQL / GQL heredoc, to the line whose trimmed text is the delimiter;
/// - a template or triple-quoted literal whose body opens with `#graphql`.
///
/// An unterminated literal yields no region.
fn gql_literal_regions(source: &str) -> Vec<(usize, usize)> {
    let b = source.as_bytes();
    let bounded = |needle: &'static str| {
        source
            .match_indices(needle)
            .map(|(at, _)| at)
            .filter(move |&at| !ident_byte_before(b, at))
    };
    let mut regions: Vec<(usize, usize)> = Vec::new();

    for &tag in GQL_TEMPLATE_TAGS {
        // Not after a `.` either: `` `.gql` `` / `` `.graphql` `` in a comment
        // is a file extension closing a code span, not a tag. The backtick is
        // the tag's last byte.
        regions.extend(
            bounded(tag)
                .filter(|&at| at.checked_sub(1).and_then(|p| b.get(p)) != Some(&b'.'))
                .filter_map(|at| literal_body(b, at + tag.len() - 1)),
        );
    }
    // A literal after a marker, past whitespace.
    let literal_after = |end: usize| literal_body(b, skip_ascii_ws(b, end));
    regions.extend(
        source
            .match_indices(GQL_MAGIC_COMMENT)
            .filter_map(|(at, _)| literal_after(at + GQL_MAGIC_COMMENT.len())),
    );
    for &call in GQL_LITERAL_CALLS {
        regions.extend(bounded(call).filter_map(|at| literal_after(at + call.len())));
    }
    for &name in SDL_VARIABLES {
        regions.extend(
            bounded(name)
                .filter(|&at| !b.get(at + name.len()).is_some_and(|&c| is_ident_byte(c)))
                .filter_map(|at| binding_value(source, at + name.len()))
                .filter_map(|value| literal_body(b, value)),
        );
    }
    for &(opener, delimiter) in GQL_HEREDOCS {
        regions.extend(
            bounded(opener)
                .map(|at| at + opener.len())
                .filter(|&end| !b.get(end).is_some_and(|&c| is_ident_byte(c)))
                .filter_map(|end| heredoc_body(source, end, delimiter)),
        );
    }
    for &mark in GQL_HASH_MARKS {
        regions.extend(source.match_indices(mark).filter_map(|(at, _)| {
            let mut i = at;
            while i > 0 && b.get(i - 1).is_some_and(|c| c.is_ascii_whitespace()) {
                i -= 1;
            }
            let opener = match i.checked_sub(1).and_then(|p| b.get(p))? {
                b'`' => i - 1,
                q @ (b'"' | b'\'') if i >= 3 && b.get(i - 3..i) == Some(&[*q, *q, *q][..]) => i - 3,
                _ => return None,
            };
            literal_body(b, opener)
        }));
    }

    regions.sort_unstable();
    regions.dedup();
    regions
}

/// The body of a heredoc whose opener ends at `after_opener`: from the next
/// line to the line whose trimmed text is `delimiter`.
fn heredoc_body(source: &str, after_opener: usize, delimiter: &str) -> Option<(usize, usize)> {
    let start = after_opener + source.get(after_opener..)?.find('\n')? + 1;
    let mut line_start = start;
    for line in source.get(start..)?.split_inclusive('\n') {
        if line.trim() == delimiter {
            return Some((start, line_start));
        }
        line_start += line.len();
    }
    None
}

/// Extract the method identifier from a TS/JS method declaration line, e.g.
/// `async getUser(@Args('id') id: string) {` → `getUser`.
fn method_name_from_line(line: &str) -> Option<String> {
    let before_paren = line.split('(').next()?.trim();
    if before_paren.is_empty() {
        return None;
    }
    let token = before_paren
        .split_whitespace()
        .last()?
        .trim_start_matches('*')
        .trim_end_matches('?');
    if token.is_empty() || METHOD_MODIFIERS.contains(&token) || !is_ident(token) {
        return None;
    }
    Some(token.to_string())
}

/// Extract the field name from an SDL field line, e.g. `getUser(id: ID!): User`
/// or `getUser: User` → `getUser`.
fn sdl_field_name(line: &str) -> Option<String> {
    if line.starts_with('#') {
        return None;
    }
    let name: String = line
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if name.is_empty() || !is_ident(&name) {
        None
    } else {
        Some(name)
    }
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_alphanumeric() || c == '_')
}

fn extract_gql_operation_name(source: &str, _line: &str) -> Option<String> {
    for tag in ["gql`", "gql(`", "gql(\"", "graphql(`", "graphql(\""] {
        if let Some(idx) = source.find(tag) {
            let after = &source[idx + tag.len()..];
            return extract_operation_from_body(after);
        }
    }
    None
}

/// Every named operation in a `` gql` `` template, with the byte offset of
/// its `` gql` `` tag. CJ.1b: a tag `guard` refuses (it starts in a Rust /
/// Python literal or comment) is no template.
fn extract_gql_template_operations(
    source: &str,
    guard: &mut LazyGuard<'_>,
) -> Vec<(String, usize)> {
    let mut ops = Vec::new();
    let mut search_from = 0;
    while let Some(idx) = source[search_from..].find("gql`") {
        let tag = search_from + idx;
        let abs = tag + 4;
        search_from = abs;
        if !guard.admits(tag) {
            continue;
        }
        if let Some(name) = extract_operation_from_body(&source[abs..]) {
            ops.push((name, tag));
        }
    }
    ops
}

fn extract_operation_from_body(body: &str) -> Option<String> {
    let trimmed = body.trim();
    for keyword in ["query ", "mutation ", "subscription "] {
        if let Some(rest) = trimmed.strip_prefix(keyword) {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    None
}

// ============================================================================
// CB.24 — the base URL a GraphQL client is built with
// ============================================================================

/// GraphQL client constructors whose options object names the base URL under
/// `uri` / `url`: `(callee, called with new)`. Apollo's `ApolloClient`,
/// `HttpLink` and `createHttpLink`; urql's `createClient` and `Client`.
const GQL_CLIENT_CTORS: &[(&str, bool)] = &[
    ("ApolloClient", true),
    ("HttpLink", true),
    ("createHttpLink", false),
    ("createClient", false),
    ("Client", true),
];

/// graphql-request's client, whose FIRST positional argument is the URL.
const GQL_REQUEST_CTOR: &str = "GraphQLClient";

/// The option keys a GraphQL client constructor reads its base URL from.
const GQL_URL_KEYS: &[&str] = &["uri", "url"];

/// CB.24: the literal authority (`host[:port]`) of every GraphQL client base
/// URL `source` builds, with the 0-based row of its literal, in source order.
///
/// A site is a [`GQL_CLIENT_CTORS`] call whose first argument is an object
/// literal holding a top-level `uri:` / `url:` key, or `new GraphQLClient(..)`
/// with a first positional argument, whose value is ONE string literal (a
/// template without `${`) with a literal `scheme://authority`
/// (`code_domain::endpoint::client_url_split`). A relative `/graphql`, an
/// env var, a concatenation or an interpolated host names no service and
/// yields nothing. `createClient` and `Client` are everyday names (a Redis
/// client is built as `createClient({ url })` too), so nothing is read from a
/// file that imports no GraphQL client package ([`gql_context`]).
pub fn graphql_client_hosts(source: &str) -> Vec<(String, u32)> {
    let named = GQL_CLIENT_CTORS.iter().any(|(c, _)| source.contains(c))
        || source.contains(GQL_REQUEST_CTOR);
    if !named {
        return Vec::new();
    }
    let ctx = gql_context(source);
    if !ctx.client_import && !ctx.graphql_request {
        return Vec::new();
    }
    let mut sites = Vec::new();
    for &(callee, new) in GQL_CLIENT_CTORS {
        sites.extend(client_option_urls(source, callee, new, GQL_URL_KEYS));
    }
    sites.extend(client_positional_urls(source, GQL_REQUEST_CTOR));
    client_sites_in_order(source, sites)
}

/// CB.24: `(literal offset, authority)` of every `callee(` call (`new
/// callee(` when `new`, a TS type argument list allowed before the paren)
/// whose first argument is an object literal holding one of `keys` at its top
/// level, bound to a single absolute-URL literal. Shared with the tRPC link
/// scan (`trpc::trpc_client_hosts`).
pub(crate) fn client_option_urls(
    source: &str,
    callee: &str,
    new: bool,
    keys: &[&str],
) -> Vec<(usize, String)> {
    let b = source.as_bytes();
    let mut out = Vec::new();
    for (at, _) in source.match_indices(callee) {
        if ident_byte_before(b, at) || in_line_comment(b, at) || (new && !preceded_by_new(b, at)) {
            continue;
        }
        let Some(open) = call_open(b, at + callee.len()) else {
            continue;
        };
        let arg = skip_ascii_ws(b, open + 1);
        if b.get(arg) != Some(&b'{') {
            continue;
        }
        let Some(close) = matching_close(b, arg) else {
            continue;
        };
        if let Some(site) = top_level_url(source, arg, close, keys) {
            out.push(site);
        }
    }
    out
}

/// CB.24: `(literal offset, authority)` of every `new callee(` call whose
/// first positional argument is a single absolute-URL literal.
fn client_positional_urls(source: &str, callee: &str) -> Vec<(usize, String)> {
    let b = source.as_bytes();
    source
        .match_indices(callee)
        .filter(|&(at, _)| !ident_byte_before(b, at) && !in_line_comment(b, at) && preceded_by_new(b, at))
        .filter_map(|(at, _)| {
            let open = call_open(b, at + callee.len())?;
            single_url_literal(source, skip_ascii_ws(b, open + 1))
        })
        .collect()
}

/// CB.24: sites in source order, one per literal, as `(authority, 0-based row)`.
pub(crate) fn client_sites_in_order(source: &str, mut sites: Vec<(usize, String)>) -> Vec<(String, u32)> {
    sites.sort_by_key(|&(at, _)| at);
    sites.dedup_by_key(|&mut (at, _)| at);
    sites
        .into_iter()
        .map(|(at, host)| (host, line_of(source, at)))
        .collect()
}

/// True when `at` sits on a line that is a comment (`//`, `/*` or a `*`
/// continuation line) up to it.
fn in_line_comment(b: &[u8], at: usize) -> bool {
    let start = b[..at.min(b.len())]
        .iter()
        .rposition(|&c| c == b'\n')
        .map_or(0, |i| i + 1);
    let head = skip_ascii_ws(b, start);
    head < at && matches!(b.get(head), Some(b'/' | b'*'))
}

/// True when the token before `at` (across whitespace, at least one byte of
/// it) is the keyword `new`.
fn preceded_by_new(b: &[u8], at: usize) -> bool {
    let end = b[..at.min(b.len())]
        .iter()
        .rposition(|c| !c.is_ascii_whitespace())
        .map_or(0, |i| i + 1);
    end < at
        && end >= 3
        && b.get(end - 3..end) == Some(b"new")
        && !ident_byte_before(b, end - 3)
}

/// The `(` of a call whose callee ends at `from`: whitespace and one TS type
/// argument list (`<NormalizedCacheObject>`) may come first.
fn call_open(b: &[u8], from: usize) -> Option<usize> {
    let mut i = skip_ascii_ws(b, from);
    if b.get(i) == Some(&b'<') {
        let mut depth = 0usize;
        loop {
            match *b.get(i)? {
                b'<' => depth += 1,
                b'>' => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        break;
                    }
                }
                b'(' | b')' | b';' | b'{' | b'}' => return None,
                _ => {}
            }
            i += 1;
        }
        i = skip_ascii_ws(b, i + 1);
    }
    (b.get(i) == Some(&b'(')).then_some(i)
}

/// The index just past the string literal whose opening quote is at `at`
/// (`'`, `"` or a backtick template), or `None` when it is unterminated. A
/// quoted string ends at the line's end; a template may span lines.
fn skip_js_literal(b: &[u8], at: usize) -> Option<usize> {
    let quote = *b.get(at)?;
    let mut i = at + 1;
    while let Some(&c) = b.get(i) {
        match c {
            b'\\' => i += 2,
            b'\n' if quote != b'`' => return None,
            _ if c == quote => return Some(i + 1),
            _ => i += 1,
        }
    }
    None
}

/// The index of the bracket closing the one opened at `open`, over any mix
/// of `()[]{}`, skipping string literals and comments.
fn matching_close(b: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = open;
    while let Some(&c) = b.get(i) {
        match c {
            b'"' | b'\'' | b'`' => {
                i = skip_js_literal(b, i)?;
                continue;
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                i = b[i..].iter().position(|&x| x == b'\n').map_or(b.len(), |p| i + p);
                continue;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i = find_close(b, i + 2, b"*/", false).map_or(b.len(), |p| p + 2);
                continue;
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth = depth.checked_sub(1)?;
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

/// The first top-level `key: <literal>` of the object literal spanning
/// `open..=close` whose key is one of `keys`: `(literal offset, authority)`.
/// A key is an identifier right after the `{` or a `,`; a matching key bound
/// to anything but one absolute-URL literal ends the search with `None`.
fn top_level_url(source: &str, open: usize, close: usize, keys: &[&str]) -> Option<(usize, String)> {
    let b = source.as_bytes();
    let mut depth = 0usize;
    let mut expect_key = true;
    let mut i = open + 1;
    while i < close {
        let c = b[i];
        match c {
            b'"' | b'\'' | b'`' => {
                i = skip_js_literal(b, i)?;
                expect_key = false;
                continue;
            }
            b'/' if matches!(b.get(i + 1), Some(b'/' | b'*')) => {
                i = if b[i + 1] == b'/' {
                    b[i..].iter().position(|&x| x == b'\n').map_or(close, |p| i + p)
                } else {
                    find_close(b, i + 2, b"*/", false).map_or(close, |p| p + 2)
                };
                continue;
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => {
                expect_key = true;
                i += 1;
                continue;
            }
            _ if c.is_ascii_whitespace() => {
                i += 1;
                continue;
            }
            _ if depth == 0 && expect_key && is_ident_byte(c) => {
                let end = i + b[i..close].iter().take_while(|&&x| is_ident_byte(x)).count();
                let colon = skip_ascii_ws(b, end);
                if keys.contains(&&source[i..end]) && b.get(colon) == Some(&b':') {
                    return single_url_literal(source, skip_ascii_ws(b, colon + 1));
                }
                i = end;
                expect_key = false;
                continue;
            }
            _ => {}
        }
        expect_key = false;
        i += 1;
    }
    None
}

/// `(offset, authority)` when a single string literal starts at `at` (a
/// template without `${`), is followed by `,` / `}` / `)` (so not a
/// concatenation or a method call), and has a literal `scheme://authority`.
fn single_url_literal(source: &str, at: usize) -> Option<(usize, String)> {
    let b = source.as_bytes();
    let quote = *b.get(at)?;
    if !matches!(quote, b'"' | b'\'' | b'`') {
        return None;
    }
    let end = skip_js_literal(b, at)?;
    let body = source.get(at + 1..end - 1)?;
    if quote == b'`' && body.contains("${") {
        return None;
    }
    if !matches!(b.get(skip_ascii_ws(b, end)), Some(b',' | b'}' | b')')) {
        return None;
    }
    let host = glia_code_domain::endpoint::client_url_split(body).0?;
    Some((at, host))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> RepoId {
        RepoId(1)
    }
    fn module_id() -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "test")
    }

    fn hosts(v: &[(&str, u32)]) -> Vec<(String, u32)> {
        v.iter().map(|&(h, l)| (h.to_string(), l)).collect()
    }

    /// CB.24: Apollo's client options name the base URL under `uri`; a TS
    /// type argument list may sit before the paren.
    #[test]
    fn apollo_client_uri() {
        let src = "import { ApolloClient, InMemoryCache } from \"@apollo/client\";\n\nexport const client = new ApolloClient({ uri: \"http://users-svc/graphql\", cache: new InMemoryCache() });\n";
        assert_eq!(graphql_client_hosts(src), hosts(&[("users-svc", 2)]));
        let generic = "import { ApolloClient } from '@apollo/client/core';\nconst c = new ApolloClient<NormalizedCacheObject>({\n  cache,\n  uri: 'https://users.internal:4000/graphql',\n});\n";
        assert_eq!(graphql_client_hosts(generic), hosts(&[("users.internal:4000", 3)]));
    }

    /// CB.24: `new HttpLink({ uri })` and `createHttpLink({ uri })`; a link
    /// nested in the client's options is one site, read once.
    #[test]
    fn http_link_uri() {
        let src = "import { ApolloClient, HttpLink, createHttpLink } from '@apollo/client';\nconst a = new ApolloClient({\n  link: new HttpLink({ uri: `http://users-svc:4000/graphql` }),\n  cache,\n});\nconst b = createHttpLink({ uri: \"http://catalog-svc/graphql\" });\n";
        assert_eq!(
            graphql_client_hosts(src),
            hosts(&[("users-svc:4000", 2), ("catalog-svc", 5)])
        );
    }

    /// CB.24: urql's `createClient({ url })` / `new Client({ url })`; the
    /// same call in a file with no GraphQL client import (a Redis client)
    /// names nothing.
    #[test]
    fn urql_create_client_url() {
        let src = "import { createClient, cacheExchange, fetchExchange } from 'urql';\nexport const client = createClient({\n  url: 'http://catalog-svc/graphql',\n  exchanges: [cacheExchange, fetchExchange],\n});\n";
        assert_eq!(graphql_client_hosts(src), hosts(&[("catalog-svc", 2)]));
        let core = "import { Client } from '@urql/core';\nconst c = new Client({ exchanges: [], url: \"http://users-svc/graphql\" });\n";
        assert_eq!(graphql_client_hosts(core), hosts(&[("users-svc", 1)]));
        let redis = "import { createClient } from 'redis';\nconst r = createClient({ url: 'redis://cache:6379' });\n";
        assert!(graphql_client_hosts(redis).is_empty());
    }

    /// CB.24: graphql-request's `new GraphQLClient(url, opts)`.
    #[test]
    fn graphql_request_ctor() {
        let src = "import { GraphQLClient } from 'graphql-request';\n\nconst gql = new GraphQLClient(\"http://users-svc/graphql\", { headers: {} });\n";
        assert_eq!(graphql_client_hosts(src), hosts(&[("users-svc", 2)]));
    }

    /// CB.24: a relative path, an env var, an interpolated or concatenated
    /// URL, a shorthand key, a nested (non-top-level) key and a commented-out
    /// client name no host.
    #[test]
    fn relative_or_dynamic_url_is_none() {
        for body in [
            "new ApolloClient({ uri: '/graphql' })",
            "new ApolloClient({ uri: process.env.GRAPHQL_URL })",
            "new ApolloClient({ uri: `http://${host}/graphql` })",
            "new ApolloClient({ uri: 'http://' + host + '/graphql' })",
            "new ApolloClient({ uri })",
            "new ApolloClient({ cache, headers: { uri: 'http://users-svc/graphql' } })",
            "// new ApolloClient({ uri: 'http://users-svc/graphql' })",
            "ApolloClient({ uri: 'http://users-svc/graphql' })",
            "new GraphQLClient(endpoint)",
        ] {
            let src = format!("import {{ ApolloClient }} from '@apollo/client';\n{body}\n");
            assert!(graphql_client_hosts(&src).is_empty(), "{body}");
        }
    }

    #[test]
    fn detects_use_query() {
        let source = "const { data } = useQuery(GET_USERS);";
        let result = extract_graphql_operation_nodes(source, "", module_id(), repo());
        assert!(!result.nodes.is_empty());
    }

    #[test]
    fn trpc_hook_lines_mint_no_graphql_operation() {
        for source in [
            "const { data } = trpc.user.list.useQuery();",
            "const m = api.post.create.useMutation();",
        ] {
            let result = extract_graphql_operation_nodes(source, "", module_id(), repo());
            assert!(
                result.nodes.is_empty(),
                "{source} -> {:?}",
                result.nav.qname_by_id.values().collect::<Vec<_>>()
            );
        }
        // The Apollo shapes beside them still count.
        let mixed = "const a = trpc.user.list.useQuery();\nconst { data } = useQuery(GET_USERS);";
        let result = extract_graphql_operation_nodes(mixed, "", module_id(), repo());
        assert!(result.nav.qname_by_id.values().any(|q| q == "graphql_op:useQuery"));
        let apollo = "const res = await client.query({ query: GET_USERS });";
        let result = extract_graphql_operation_nodes(apollo, "", module_id(), repo());
        assert!(result.nav.qname_by_id.values().any(|q| q == "graphql_op:client.query"));
    }

    /// CJ.1b: in a `.rs` / `.py` file an operation needle or `` gql` `` tag
    /// inside a string literal or comment mints nothing; the same text read
    /// as TypeScript (no guard) mints as before. A Python needle in code
    /// stays, anchored on its own line, not on a literal one above it.
    #[test]
    fn literal_operations_mint_nothing_in_rust_and_python() {
        let hook = "const { data } = useQuery(GET_USERS);";
        let template = r#"const GET_USERS = gql`query GetUsers { u }`;"#;
        let rust = format!(
            "/// `useQuery(GET_USERS)` and `gql` documents, as the scanner reads them.\n\
             fn samples() {{\n\
             \x20   let hook = {hook:?};\n\
             \x20   let doc = r#\"{template}\"#;\n\
             \x20   let _ = (hook, doc);\n\
             }}\n"
        );
        let qnames = |out: &GraphqlNodes| {
            let mut q: Vec<String> = out.nav.qname_by_id.values().cloned().collect();
            q.sort();
            q
        };
        let rs = extract_graphql_operation_nodes(&rust, "scanner/src/channels.rs", module_id(), repo());
        assert!(rs.nodes.is_empty() && rs.anchors.is_empty(), "{:?}", qnames(&rs));

        // Unguarded (TypeScript): each shape mints as it did before CJ.1b.
        let ts_hook = extract_graphql_operation_nodes(hook, "x.ts", module_id(), repo());
        assert_eq!(qnames(&ts_hook), vec!["graphql_op:useQuery"]);
        let ts_doc = extract_graphql_operation_nodes(template, "x.ts", module_id(), repo());
        assert_eq!(qnames(&ts_doc), vec!["graphql_op:GetUsers"]);
        // The Rust file's text read as TypeScript still mints from its literals.
        assert!(!extract_graphql_operation_nodes(&rust, "x.ts", module_id(), repo()).nodes.is_empty());

        let py = "from gql import gql\n\
                  QUERY = gql(\"query GetUsers { u }\")\n\
                  # client.query(QUERY)\n\
                  SAMPLE = \"client.query(QUERY)\"\n\
                  def run(client):\n\
                  \x20   return client.query(QUERY)\n";
        let out = extract_graphql_operation_nodes(py, "client/ops.py", module_id(), repo());
        assert_eq!(qnames(&out), vec!["graphql_op:GetUsers"]);
        assert_eq!(out.anchors.iter().map(|a| a.line).collect::<Vec<_>>(), vec![5]);
        let unguarded = extract_graphql_operation_nodes(py, "client/ops.ts", module_id(), repo());
        assert_eq!(unguarded.anchors.iter().map(|a| a.line).collect::<Vec<_>>(), vec![2]);
    }

    #[test]
    fn extracts_gql_template_name() {
        let source = r#"const GET_USERS = gql`query GetUsers { users { id name } }`;"#;
        let result = extract_graphql_operation_nodes(source, "", module_id(), repo());
        assert!(result.nav.qname_by_id.values().any(|q| q == "graphql_op:GetUsers"));
    }

    #[test]
    fn detects_resolver_decorator() {
        let source = "import { Query } from '@nestjs/graphql';\n@Query()\nasync users() { return []; }";
        let result = extract_graphql_resolver_nodes(source, "typescript", module_id(), repo());
        assert!(!result.nodes.is_empty());
        assert!(result.nav.qname_by_id.values().any(|q| q.starts_with("graphql_resolver:")));
    }

    #[test]
    fn detects_schema_type() {
        // LA.27: a bare SDL document is a `.graphql` body, read whole.
        let source = "type Query {\n  users: [User]\n}";
        let result = extract_graphql_sdl_file_nodes(source, module_id(), repo());
        assert!(result.nav.qname_by_id.values().any(|q| q == "graphql_resolver:Query"));
    }

    #[test]
    fn extracts_decorator_method_field_name() {
        // NestJS: the resolver field is the method under @Query(), not "Query".
        let source = "import { Query } from '@nestjs/graphql';\n  @Query(() => User)\n  async getUser(@Args('id') id: string) {\n    return this.userService.findOne(id);\n  }";
        let result = extract_graphql_resolver_nodes(source, "typescript", module_id(), repo());
        // The client op `gql query getUser` pairs against this field name.
        assert!(
            result
                .nav
                .qname_by_id
                .values()
                .any(|q| q == "graphql_resolver:getUser"),
            "expected field-named resolver getUser, got {:?}",
            result.nav.qname_by_id.values().collect::<Vec<_>>()
        );
        // The root noun the decorator sits on is retained.
        assert!(result
            .nav
            .qname_by_id
            .values()
            .any(|q| q == "graphql_resolver:Query"));
    }

    #[test]
    fn extracts_sdl_type_field_names() {
        let source = "type Query {\n  getUser(id: ID!): User\n  listUsers: [User]\n}";
        let result = extract_graphql_sdl_file_nodes(source, module_id(), repo());
        let qnames: Vec<&String> = result.nav.qname_by_id.values().collect();
        assert!(qnames.iter().any(|q| *q == "graphql_resolver:getUser"));
        assert!(qnames.iter().any(|q| *q == "graphql_resolver:listUsers"));
    }

    #[test]
    fn method_name_skips_modifiers() {
        assert_eq!(
            method_name_from_line("async getUser(@Args('id') id: string) {"),
            Some("getUser".to_string())
        );
        assert_eq!(method_name_from_line("constructor(private x: Y) {"), None);
    }

    fn anchor_line(out: &GraphqlNodes, qname: &str) -> Option<u32> {
        let id = out
            .nav
            .qname_by_id
            .iter()
            .find(|(_, q)| q.as_str() == qname)
            .map(|(id, _)| *id)?;
        out.anchors.iter().find(|a| a.node == id).map(|a| a.line)
    }

    #[test]
    fn root_noun_anchors_at_its_class_line() {
        // CB.4: a NestJS resolver whose first method carries `@Query(...)`.
        let src = "import { Resolver, Query } from \"@nestjs/graphql\";\n\n@Resolver()\n@Injectable()\nexport class OrdersGateway {\n  /** Records a shipped order. */\n  @OnEvent(\"order.shipped\")\n  onShipped() {}\n\n  @Query(() => String)\n  order() {\n    return \"x\";\n  }\n}";
        let out = extract_graphql_resolver_nodes(src, "typescript", module_id(), repo());
        assert_eq!(
            anchor_line(&out, "graphql_resolver:Query"),
            Some(4),
            "the `export class` line"
        );
        assert_eq!(
            anchor_line(&out, "graphql_resolver:order"),
            Some(10),
            "a field keeps its method line"
        );
        // Every class declaration spelling anchors the root at row 1.
        for header in [
            "class R {",
            "export class R {",
            "export default class R {",
            "abstract class R {",
            "export abstract class R {",
        ] {
            let src = format!(
                "import {{ Query }} from '@nestjs/graphql';\n{header}\n  @Query(() => X)\n  q() {{}}\n}}"
            );
            let out = extract_graphql_resolver_nodes(&src, "typescript", module_id(), repo());
            assert_eq!(
                anchor_line(&out, "graphql_resolver:Query"),
                Some(1),
                "{header}"
            );
        }
        // No class above: the decorator line, as before.
        let bare =
            "import { Query } from '@nestjs/graphql';\n@Query(() => [User])\nasync users() {}";
        let out = extract_graphql_resolver_nodes(bare, "typescript", module_id(), repo());
        assert_eq!(anchor_line(&out, "graphql_resolver:Query"), Some(1));
        // `classify` is not a class keyword.
        assert!(!is_class_line("  classify(x) {"));
    }

    #[test]
    fn anchors_operations_at_their_minting_line() {
        let src = "const GET_USER = gql`\n  query getUser { u }\n`;\n\nexport function P() {\n  const { data } = useQuery(GET_USER);\n}\nconst OTHER = gql`query listUsers { u }`;";
        let out = extract_graphql_operation_nodes(src, "", module_id(), repo());
        // The useQuery( line mints getUser (the file's first gql tag).
        assert_eq!(anchor_line(&out, "graphql_op:getUser"), Some(5));
        // A template-only operation anchors at its gql` tag.
        assert_eq!(anchor_line(&out, "graphql_op:listUsers"), Some(7));
        assert_eq!(out.anchors.len(), out.nodes.len(), "one anchor per node");
    }

    #[test]
    fn anchors_resolvers_at_the_method_and_sdl_lines() {
        let src = "import { Resolver, Query } from '@nestjs/graphql';\n@Resolver('User')\nexport class R {\n  @Query(() => User)\n  async getUser(id: string) {\n    return 1;\n  }\n}";
        let out = extract_graphql_resolver_nodes(src, "typescript", module_id(), repo());
        assert_eq!(anchor_line(&out, "graphql_resolver:getUser"), Some(4));
        // CB.4: the root noun anchors at its class line, not at `@Query(`.
        assert_eq!(anchor_line(&out, "graphql_resolver:Query"), Some(2));
        // LA.38: `@Resolver(` is a decorator name, never a node.
        assert_eq!(anchor_line(&out, "graphql_resolver:Resolver"), None);

        let sdl = "const typeDefs = `\ntype Query {\n  getUser(id: ID!): User\n  listUsers: [User]\n}\n`;";
        let out = extract_graphql_resolver_nodes(sdl, "typescript", module_id(), repo());
        assert_eq!(anchor_line(&out, "graphql_resolver:getUser"), Some(2));
        assert_eq!(anchor_line(&out, "graphql_resolver:listUsers"), Some(3));
        assert_eq!(anchor_line(&out, "graphql_resolver:Query"), Some(1));
    }

    fn op_qnames(source: &str) -> Vec<String> {
        let out = extract_graphql_operation_nodes(source, "", module_id(), repo());
        let mut qnames: Vec<String> = out.nav.qname_by_id.values().cloned().collect();
        qnames.sort();
        qnames
    }

    fn assert_no_ops(sources: &[&str]) {
        for source in sources {
            assert!(op_qnames(source).is_empty(), "{source} -> {:?}", op_qnames(source));
        }
    }

    fn assert_op(source: &str, qname: &str) {
        let qnames = op_qnames(source);
        assert!(qnames.iter().any(|q| q == qname), "{source} -> {qnames:?}, want {qname}");
    }

    #[test]
    fn request_needle_needs_a_graphql_request_client() {
        assert_no_ops(&[
            "$this->http->request('GET', '/users');",
            "def handle_request(self, request):\n    return requests.request(\"GET\", request.url)",
            "agent.request(\"GET\", url).call()",
        ]);
        assert_op(
            "import { request } from 'graphql-request';\nawait request(API, LIST_USERS);",
            "graphql_op:request",
        );
        assert_op(
            "import { GraphQLClient } from 'graphql-request';\nconst c = new GraphQLClient(url);\nawait c.request(DOC);",
            "graphql_op:request",
        );
        // The constructor alone is graphql-request context.
        assert_op("const c = new GraphQLClient(url);\nawait c.request(DOC);", "graphql_op:request");
        // A longer name ending in `request` is not graphql-request's call.
        assert_no_ops(&["import { request } from 'graphql-request';\nhandle_request(x);"]);
        // TanStack's hook is rejected, but graphql-request's call inside it
        // on the same line still mints.
        let mixed = "import { useQuery } from '@tanstack/react-query';\nimport { request } from 'graphql-request';\nuseQuery(['users'], () => request(API, USERS));";
        assert_eq!(op_qnames(mixed), vec!["graphql_op:request".to_string()]);
        let (hits, tally) = scan_operation_needles(mixed, &mut LazyGuard::new("", mixed));
        assert_eq!(hits, vec![("request".to_string(), 2)]);
        assert_eq!((tally.kept, tally.hook), (1, 1));
    }

    #[test]
    fn package_name_is_context_not_an_operation() {
        assert_no_ops(&[
            "// built on graphql-request",
            "import { GraphQLClient } from \"graphql-request\";",
        ]);
    }

    #[test]
    fn client_methods_need_a_graphql_argument() {
        assert_no_ops(&[
            "client.subscribe('sensors/temp');",
            "client.query('SELECT 1')",
            "client.query(sql, params)",
            "client.query({ text: q, values })",
        ]);
        assert_op("client.mutate({\n  mutation: ADD_USER,\n})", "graphql_op:client.mutate");
        assert_op(
            "import { createClient } from 'urql';\nclient.query(USERS, {}).toPromise();",
            "graphql_op:client.query",
        );
        // A module name that only starts with a client package is not one.
        assert_no_ops(&["import x from 'urql-x';\nclient.query(USERS, {});"]);
    }

    #[test]
    fn hooks_need_a_graphql_argument() {
        assert_no_ops(&[
            "useQuery({ queryKey: ['todos'], queryFn })",
            "useQuery(['todos'], fetchTodos)",
            "useQuery('todos', fetchTodos)",
            "import { useQuery } from '@tanstack/react-query';\nuseQuery(todoOptions);",
        ]);
        assert_op("useQuery({ query: TodosQuery })", "graphql_op:useQuery");
        // Named GetUsers, not a fresh name: glia parses its own source, and
        // this test string mints the op it names there too.
        assert_op("useQuery(gql`query GetUsers { a }`)", "graphql_op:GetUsers");
        // Apollo beside TanStack: the GraphQL client import wins.
        assert_op(
            "import { useQuery } from '@apollo/client';\nimport { useQueries } from '@tanstack/react-query';\nuseQuery(GET_USERS);",
            "graphql_op:useQuery",
        );
    }

    #[test]
    fn argument_scan_is_byte_safe_across_multibyte_text() {
        // The 512-byte window ends mid-character inside the é run, and a
        // multibyte byte sits right before a needle; neither may panic.
        let long = format!("client.query({{ {}: 1 }})", "é".repeat(400));
        let before = "héllo€useQuery(GET_ÜSERS);\nconst s = \"ü\";request(x)";
        assert_no_ops(&[long.as_str()]);
        assert_op(before, "graphql_op:useQuery");
    }

    #[test]
    fn marker_counts_rejections_per_family() {
        let rows = "import { useQuery } from '@tanstack/react-query';\nclient.query('SELECT 1');\nclient.subscribe('t');\nuseQuery({ queryKey: ['k'] });";
        let (hits, tally) = scan_operation_needles(rows, &mut LazyGuard::new("", rows));
        assert!(hits.is_empty());
        assert_eq!(
            tally.marker().as_deref(),
            Some("[graphql-ops] kept=0 rejected=3 hook=1 client=2 request=0 context=tanstack")
        );

        let users = "import { request, gql } from 'graphql-request';\nconst Q = gql(DOC);\nawait request(url, Q);";
        let (hits, tally) = scan_operation_needles(users, &mut LazyGuard::new("", users));
        assert_eq!(hits, vec![("request".to_string(), 2)]);
        assert_eq!(
            tally.marker().as_deref(),
            Some("[graphql-ops] kept=1 rejected=0 hook=0 client=0 request=0 context=client+graphql-request+gql-tag")
        );

        // No needle hit, no marker line.
        assert_eq!(scan_operation_needles("const x = 1;", &mut LazyGuard::new("", "const x = 1;")).1.marker(), None);
    }

    // LA.27. Every source below is a one-line Rust string (`\n` escapes), so
    // glia's own build of this file reads no SDL line from it.

    /// Resolver qnames minted from `source` in code mode, sorted.
    fn resolver_qnames(source: &str, lang: &str) -> Vec<String> {
        let out = extract_graphql_resolver_nodes(source, lang, module_id(), repo());
        let mut qnames: Vec<String> = out.nav.qname_by_id.values().cloned().collect();
        qnames.sort();
        qnames
    }

    fn assert_resolvers(source: &str, lang: &str, want: &[&str]) {
        let got = resolver_qnames(source, lang);
        let want: Vec<String> = want.iter().map(|n| format!("graphql_resolver:{n}")).collect();
        assert_eq!(got, want, "{source}");
    }

    #[test]
    fn sdl_in_a_rust_comment_mints_nothing() {
        // engine/src/route.rs's A10.4 comment, verbatim: the glia self-build
        // minted graphql_resolver:Query from it, HANDLED_BY parse_repo_files.
        let source = "fn parse_repo_files() {\n        // A10.4: a `.graphql` / `.gql` schema reaches the SDL field scan that\n        // embedded `type Query {` blocks already get, so a schema-first\n        // service has resolvers for its clients' operations to pair with.\n        if lang == \"graphql\" {}\n}\n";
        assert_resolvers(source, "rust", &[]);
        assert_eq!(
            embedded_sdl(source).marker().as_deref(),
            Some("[graphql-sdl-code] literals=0 roots= fields=0 unmarked_roots=1")
        );
    }

    #[test]
    fn sdl_in_an_unmarked_string_mints_nothing() {
        assert_resolvers(r#"let sdl = "type Query {\n  getUser(id: ID!): User\n}\n";"#, "rust", &[]);
        // A multi-line raw string: the block scan read it line by line.
        let raw = "#[test]\nfn schema_routes() {\n    let sdl = r#\"\ntype Subscription {\n  orderShipped: Order\n}\n\"#;\n    assert!(!sdl.is_empty());\n}\n";
        assert_resolvers(raw, "rust", &[]);
        assert_eq!(
            embedded_sdl(raw).marker().as_deref(),
            Some("[graphql-sdl-code] literals=0 roots= fields=0 unmarked_roots=1")
        );
    }

    #[test]
    fn sdl_in_a_python_comment_mints_nothing() {
        let source = "# How the resolver scan works: a line like\n#   type Query {\n# opens a root type block.\ndef scan(text):\n    return text.splitlines()\n";
        assert_resolvers(source, "python", &[]);
    }

    #[test]
    fn gql_tagged_sdl_mints_root_and_fields() {
        // The tag marks the literal whatever the variable is called.
        let source = "import { gql } from \"graphql-tag\";\n\nexport const schema = gql`\n  type Query {\n    listOrders: [Order]\n  }\n\n  type Order {\n    id: ID!\n  }\n`;\n";
        let out = extract_graphql_resolver_nodes(source, "typescript", module_id(), repo());
        assert_eq!(anchor_line(&out, "graphql_resolver:Query"), Some(3));
        assert_eq!(anchor_line(&out, "graphql_resolver:listOrders"), Some(4));
        assert_eq!(out.nodes.len(), 2, "`type Order {{` is an object type, never a resolver");
        assert_eq!(
            embedded_sdl(source).marker().as_deref(),
            Some("[graphql-sdl-code] literals=1 roots=Query fields=1 unmarked_roots=0")
        );
    }

    #[test]
    fn ariadne_gql_call_mints_fields() {
        let source = "from ariadne import gql\n\nsdl = gql(\"\"\"\n    type Mutation {\n        placeOrder(sku: String!): Order\n    }\n\"\"\")\n";
        assert_resolvers(source, "python", &["Mutation", "placeOrder"]);
        let out = extract_graphql_resolver_nodes(source, "python", module_id(), repo());
        assert_eq!(anchor_line(&out, "graphql_resolver:placeOrder"), Some(4));
    }

    #[test]
    fn go_must_parse_schema_literal_mints_fields() {
        let source = "func Schema() *graphql.Schema {\n\treturn graphql.MustParseSchema(`\n\ttype Query {\n\t\tuser(id: ID!): User\n\t}\n`, &resolver{})\n}\n";
        assert_resolvers(source, "go", &["Query", "user"]);
        // A schema string passed by name is not a marked literal.
        assert_resolvers("var s = `\ntype Query {\n  user: User\n}\n`\nvar _ = graphql.MustParseSchema(s, &r{})\n", "go", &[]);
        // `rebuildSchema(` is not `buildSchema(`.
        assert_resolvers("rebuildSchema(`\ntype Query {\n  user: User\n}\n`)\n", "go", &[]);
    }

    #[test]
    fn ruby_graphql_heredoc_mints_fields() {
        let source = "class Schema\n  DEFINITION = <<~GRAPHQL\n    type Query {\n      posts: [Post]\n    }\n  GRAPHQL\nend\nSchema2 = GraphQL::Schema.from_definition(<<-GQL)\n  extend type Mutation {\n    deletePost(id: ID!): Boolean\n  }\nGQL\n";
        assert_resolvers(source, "ruby", &["Mutation", "Query", "deletePost", "posts"]);
        let out = extract_graphql_resolver_nodes(source, "ruby", module_id(), repo());
        assert_eq!(anchor_line(&out, "graphql_resolver:posts"), Some(3));
        assert_eq!(anchor_line(&out, "graphql_resolver:deletePost"), Some(9));
        // No terminator line: no region, and no panic.
        assert_resolvers("q = <<~GRAPHQL\n  type Query {\n    posts: [Post]\n  }\n", "ruby", &[]);
    }

    #[test]
    fn graphql_magic_comment_template_mints_fields() {
        let source = "const schema = /* GraphQL */ `\n  type Mutation {\n    addBook(title: String): Book\n  }\n`;\n";
        assert_resolvers(source, "typescript", &["Mutation", "addBook"]);
    }

    #[test]
    fn hash_graphql_template_mints_fields() {
        // Apollo Server 4: `schema` is a name no binding rule knows.
        let source = "const schema = `#graphql\n  type Query {\n    books: [Book]\n  }\n`;\n";
        assert_resolvers(source, "typescript", &["Query", "books"]);
        let py = "SCHEMA = \"\"\"\n# graphql\ntype Query {\n  books: [Book]\n}\n\"\"\"\n";
        assert_resolvers(py, "python", &["Query", "books"]);
    }

    #[test]
    fn typedefs_bindings_mint_fields() {
        for (source, lang) in [
            ("export const typeDefs: string = `\ntype Query {\n  me: User\n}\n`;\n", "typescript"),
            ("type_defs = \"\"\"\ntype Query {\n    me: User\n}\n\"\"\"\n", "python"),
            ("type_defs: str = \"\"\"\ntype Query {\n    me: User\n}\n\"\"\"\n", "python"),
            ("new ApolloServer({\n  typeDefs: `\n    type Query {\n      me: User\n    }\n  `,\n  resolvers,\n});\n", "typescript"),
        ] {
            assert_resolvers(source, lang, &["Query", "me"]);
        }
        // The same template in a differently named variable is not read
        // (the coverage caveat row declares this), nor is a longer name.
        for source in [
            "const schema = `\ntype Query {\n  me: User\n}\n`;\n",
            "const typeDefsV2 = `\ntype Query {\n  me: User\n}\n`;\n",
            "if (typeDefs == `\ntype Query {\n  me: User\n}\n`) {}\n",
        ] {
            assert_resolvers(source, "typescript", &[]);
        }
    }

    #[test]
    fn literal_regions_are_char_safe_and_skip_unterminated_literals() {
        let source = "const a = gql`é type Query {`;\nconst b = gql`\ntype Query {\n  ünïcode: String\n  ok: Int\n}\n";
        // `a` closes on its line; `b` never closes, so only `a` is a region.
        let regions = gql_literal_regions(source);
        assert_eq!(regions.len(), 1);
        let (s, e) = regions[0];
        assert_eq!(source.get(s..e), Some("é type Query {"));
        // A root opener mid-line is not a line start, and `b` is unread.
        assert_resolvers(source, "typescript", &[]);
        // Unterminated openers of every shape end the scan cleanly.
        for tail in ["gql`", "gql(\"", "gql(\"\"\"", "typeDefs = '", "/* GraphQL */", "<<~GRAPHQL"] {
            assert!(gql_literal_regions(tail).is_empty(), "{tail}");
        }
    }

    #[test]
    fn sdl_file_mode_scans_the_whole_file() {
        let sdl = "# schema root\ntype Query {\n  me: User\n}\n\nextend type Mutation {\n  logout: Boolean\n}\n\ntype User {\n  id: ID!\n}\n";
        let out = extract_graphql_sdl_file_nodes(sdl, module_id(), repo());
        let mut qnames: Vec<&str> = out.nav.qname_by_id.values().map(String::as_str).collect();
        qnames.sort_unstable();
        assert_eq!(
            qnames,
            [
                "graphql_resolver:Mutation",
                "graphql_resolver:Query",
                "graphql_resolver:logout",
                "graphql_resolver:me"
            ]
        );
        assert_eq!(anchor_line(&out, "graphql_resolver:Query"), Some(1));
        assert_eq!(anchor_line(&out, "graphql_resolver:logout"), Some(6));
        // The same bare document in a code file is not a marked literal.
        assert_resolvers(sdl, "typescript", &[]);
    }

    // LA.38. Every source below is a one-line Rust string (`\n` escapes), and
    // glia's own build reads this file as Rust, so neither mints a resolver.

    const JPA_REPOSITORY: &str = "import org.springframework.data.jpa.repository.Query;\n@Query(\"SELECT p FROM Pet p\")\nList<Pet> findPets();";
    const REST_CONTROLLER: &str = "import { Controller, Get, Query } from '@nestjs/common';\n@Get()\nasync list(\n  @Query() filter: Dto,\n) {}";
    const COMMENT_AND_STRING: &str = "import { Query } from '@nestjs/graphql';\n// use @Mutation(() => X) once writes land\nconst doc = \"@Subscription(\";\n@Query(() => [User])\nasync users() {}";

    #[test]
    fn decorator_family_needs_a_graphql_language() {
        // Spring Data JPA's @Query is JPQL (HEAD: Query + findPets).
        assert_resolvers(JPA_REPOSITORY, "java", &[]);
        // A needle table (HEAD: Resolver, ResolveField, ObjectType,
        // strawberry.type).
        let table = "pub const NOUNS: &[&str] = &[\"@Resolver(\", \"@ResolveField(\", \"ObjectType):\", \"@strawberry.type\"];";
        assert_resolvers(table, "rust", &[]);
        // HEAD: Query.
        assert_resolvers("// @Query(() => X)\nfunc f() {}", "go", &[]);
        // The same decorator in a GraphQL server file of the TypeScript family.
        let nest = "import { Query } from '@nestjs/graphql';\n@Query(() => [User])\nasync users() {}";
        for lang in ["typescript", "angular", "vue", "react"] {
            assert_resolvers(nest, lang, &["Query", "users"]);
        }
        assert_resolvers(nest, "kotlin", &[]);
    }

    #[test]
    fn decorator_family_needs_a_graphql_import() {
        // @nestjs/common's query-string parameter decorator (HEAD: Query).
        assert_resolvers(REST_CONTROLLER, "typescript", &[]);
        let graphql = "import { Query } from '@nestjs/graphql';\n@Query(() => [User])\nasync users() {}";
        assert_resolvers(graphql, "typescript", &["Query", "users"]);
        // A multi-line import, and TypeGraphQL.
        let multi_line = "import {\n  Args,\n  Query,\n} from \"@nestjs/graphql\";\n@Query(() => [User])\nasync users() {}";
        assert_resolvers(multi_line, "typescript", &["Query", "users"]);
        let type_graphql = "import { Query, Resolver } from 'type-graphql';\n@Query(() => [User])\nasync users() {}";
        assert_resolvers(type_graphql, "typescript", &["Query", "users"]);
        // A package that only starts with the name is not it.
        let lookalike = "import { Query } from '@nestjs/graphql-x';\n@Query(() => [User])\nasync users() {}";
        assert_resolvers(lookalike, "typescript", &[]);
        // Python: strawberry-like package names are not strawberry.
        let fields = "from strawberryfields import ops\n@strawberry.type\nclass Query:\n    x: int";
        assert_resolvers(fields, "python", &[]);
    }

    #[test]
    fn decorators_count_only_at_decorator_position() {
        // HEAD: Mutation + Query + Subscription + users.
        assert_resolvers(COMMENT_AND_STRING, "typescript", &["Query", "users"]);
        // A JSDoc line starts with `*`, not with the decorator.
        let jsdoc = "import { Query } from '@nestjs/graphql';\n/**\n * @Mutation(() => X) lands later\n */\nconst x = 1;";
        assert_resolvers(jsdoc, "typescript", &[]);
    }

    #[test]
    fn only_root_types_are_nouns() {
        // HEAD adds Resolver and ResolveField.
        let source = "import { Resolver, ResolveField, Query, Subscription } from '@nestjs/graphql';\n@Resolver(() => Recipe)\nexport class R {\n  @Query(() => [Recipe])\n  recipes() {}\n  @Subscription(() => Recipe)\n  added() {}\n  @ResolveField()\n  author() {}\n}";
        assert_resolvers(source, "typescript", &["Query", "Subscription", "added", "author", "recipes"]);
        let out = extract_graphql_resolver_nodes(source, "typescript", module_id(), repo());
        // CB.4: both root nouns anchor at `export class R {`; fields keep
        // their method lines.
        assert_eq!(anchor_line(&out, "graphql_resolver:Query"), Some(2));
        assert_eq!(anchor_line(&out, "graphql_resolver:Subscription"), Some(2));
        assert_eq!(anchor_line(&out, "graphql_resolver:author"), Some(8));
    }

    #[test]
    fn strawberry_and_graphene_root_classes() {
        let strawberry = "import strawberry\n\n@strawberry.type\nclass Recipe:\n    title: str\n\n@strawberry.type\nclass Query:\n    @strawberry.field\n    def recipe(self) -> Recipe: ...\n\n@strawberry.type\nclass Mutation:\n    @strawberry.mutation\n    def add(self) -> Recipe: ...";
        // HEAD: strawberry.type + strawberry.mutation. CB.14: the root
        // classes' fields too.
        assert_resolvers(strawberry, "python", &["Mutation", "Query", "add", "recipe"]);
        let out = extract_graphql_resolver_nodes(strawberry, "python", module_id(), repo());
        assert_eq!(anchor_line(&out, "graphql_resolver:Query"), Some(7));
        assert_eq!(anchor_line(&out, "graphql_resolver:Mutation"), Some(12));
        // HEAD: ObjectType.
        let graphene = "import graphene\nclass User(graphene.ObjectType):\n    name = graphene.String()\nclass Query(graphene.ObjectType):\n    user = graphene.Field(User)";
        assert_resolvers(graphene, "python", &["Query", "user"]);
        let out = extract_graphql_resolver_nodes(graphene, "python", module_id(), repo());
        assert_eq!(anchor_line(&out, "graphql_resolver:Query"), Some(3));
        // Without its import the strawberry source mints nothing.
        let unimported = strawberry.trim_start_matches("import strawberry\n");
        assert_resolvers(unimported, "python", &[]);
        // A called decorator past a stacked one, a submodule import, a bare
        // ObjectType base beside a mixin; `MyObjectType` is not the token.
        let shapes = "from strawberry.types import Info\n@strawberry.type(description=\"root\")\n@other\nclass Subscription:\n    pass\n";
        assert_resolvers(shapes, "python", &["Subscription"]);
        let bare = "from graphene import ObjectType\nclass Query(ObjectType, Mixin):\n    pass\nclass Mutation(MyObjectType):\n    pass\n";
        assert_resolvers(bare, "python", &["Query"]);
    }

    #[test]
    fn census_counts_every_rejection() {
        let census = |source: &str, lang: &str| decorator_scan(source, lang).1;
        let java = census(JPA_REPOSITORY, "java");
        assert_eq!((java.hits, java.rej_lang, java.rej_import, java.rej_position), (1, 1, 0, 0));
        let rest = census(REST_CONTROLLER, "typescript");
        assert_eq!((rest.hits, rest.rej_lang, rest.rej_import, rest.rej_position), (1, 0, 1, 0));
        let mixed = census(COMMENT_AND_STRING, "typescript");
        assert_eq!((mixed.hits, mixed.rej_lang, mixed.rej_import, mixed.rej_position), (3, 0, 0, 2));
        assert_eq!((mixed.roots.as_slice(), mixed.fields), (&["Query"][..], 1));

        assert_eq!(
            java.marker("java").as_deref(),
            Some("[graphql-decorators] lang=java import=none roots= fields=0 rejected lang=1 import=0 position=0")
        );
        assert_eq!(
            rest.marker("typescript").as_deref(),
            Some("[graphql-decorators] lang=typescript import=none roots= fields=0 rejected lang=0 import=1 position=0")
        );
        let resolver = "import { Args, Mutation, Query, Resolver } from \"@nestjs/graphql\";\n\n@Resolver(() => Recipe)\nexport class RecipesResolver {\n  @Query(() => [Recipe])\n  async recipes() {\n    return [];\n  }\n\n  @Mutation(() => Recipe)\n  async addRecipe(@Args(\"title\") title: string) {\n    return { title };\n  }\n}";
        assert_eq!(
            census(resolver, "typescript").marker("typescript").as_deref(),
            Some("[graphql-decorators] lang=typescript import=@nestjs/graphql roots=Query,Mutation fields=2 rejected lang=0 import=0 position=0")
        );
        let py = "import strawberry\n@strawberry.type\nclass Query:\n    x: int";
        assert_eq!(
            census(py, "python").marker("python").as_deref(),
            Some("[graphql-decorators] lang=python import=strawberry roots=Query fields=0 rejected lang=0 import=0 position=0")
        );
        // No needle, no marker line.
        assert_eq!(census("const x = 1;", "typescript").marker("typescript"), None);
    }

    // CB.14. Python root-class field resolvers, named by their schema field.

    #[test]
    fn strawberry_field_methods() {
        // A nested decorated function inside a resolver body is not a
        // first-level member, and a stacked decorator is skipped.
        let source = "import strawberry\n\n@strawberry.type\nclass Query:\n    @strawberry.field\n    def user(self, id: strawberry.ID) -> User:\n        @strawberry.field\n        def inner(self) -> int:\n            return 1\n        return User()\n\n    @strawberry.field\n    @cached\n    async def current_user(self) -> User:\n        return User()\n\n    # a comment at the member indent\n    def helper(self) -> None: ...\n";
        assert_resolvers(source, "python", &["Query", "currentUser", "user"]);
        let out = extract_graphql_resolver_nodes(source, "python", module_id(), repo());
        assert_eq!(anchor_line(&out, "graphql_resolver:user"), Some(5), "the `def` line");
        assert_eq!(anchor_line(&out, "graphql_resolver:currentUser"), Some(13), "past `@cached`");
        assert_eq!(anchor_line(&out, "graphql_resolver:Query"), Some(3));
        // Mutation and Subscription roots read the same way.
        let roots = "import strawberry\n@strawberry.type\nclass Mutation:\n    @strawberry.mutation\n    def add_book(self, title: str) -> Book: ...\n@strawberry.type\nclass Subscription:\n    @strawberry.subscription\n    async def book_added(self) -> AsyncGenerator[Book, None]: ...\n";
        assert_resolvers(roots, "python", &["Mutation", "Subscription", "addBook", "bookAdded"]);
        // A decorator whose next member is no method names nothing.
        let dangling = "import strawberry\n@strawberry.type\nclass Query:\n    @strawberry.field\n    x: int\n";
        assert_resolvers(dangling, "python", &["Query"]);
    }

    #[test]
    fn strawberry_explicit_name() {
        let source = "import strawberry\n@strawberry.type\nclass Query:\n    @strawberry.field(name=\"me\")\n    def current_user(self) -> User: ...\n";
        assert_resolvers(source, "python", &["Query", "me"]);
        // A multi-line call, a bracket inside a string, single quotes.
        let multi = "import strawberry\n@strawberry.type\nclass Query:\n    @strawberry.field(\n        description=\"the (viewer\",\n        name='viewer',\n    )\n    def who_am_i(self) -> User: ...\n";
        assert_resolvers(multi, "python", &["Query", "viewer"]);
        let out = extract_graphql_resolver_nodes(multi, "python", module_id(), repo());
        assert_eq!(anchor_line(&out, "graphql_resolver:viewer"), Some(7));
        // A `name=` of a nested call, in a string or a comment, a longer
        // keyword and a non-literal value are not the field's name.
        let decoys = "import strawberry\n@strawberry.type\nclass Mutation:\n    @strawberry.mutation(permission_classes=[P(name=\"x\")], description=\"name='y'\")\n    def add_book(self) -> Book: ...\n    @strawberry.mutation(graphql_name=\"z\")  # name=\"w\"\n    def drop_book(self) -> Book: ...\n    @strawberry.mutation(name=NAME)\n    def lend_book(self) -> Book: ...\n";
        assert_resolvers(decoys, "python", &["Mutation", "addBook", "dropBook", "lendBook"]);
        assert_eq!(py_name_kwarg("(User, name=\"me\")").as_deref(), Some("me"));
        assert_eq!(py_name_kwarg("(name == \"x\", name=\"y\")").as_deref(), Some("y"));
        assert_eq!(py_name_kwarg("(name=\"not-a-name\")"), None);
        assert_eq!(py_name_kwarg("(name=\"unterminated"), None);
    }

    #[test]
    fn strawberry_attribute_field() {
        let source = "import strawberry\n@strawberry.type\nclass Query:\n    users: list[User] = strawberry.field(resolver=get_users)\n    top_user: User = strawberry.field(resolver=get_top, name=\"best\")\n    all_books = strawberry.field(resolver=get_books)\n    count: int = 0\n    title: str\n    flag: bool = strawberry.field_x()\n";
        assert_resolvers(source, "python", &["Query", "allBooks", "best", "users"]);
        let out = extract_graphql_resolver_nodes(source, "python", module_id(), repo());
        assert_eq!(anchor_line(&out, "graphql_resolver:users"), Some(3));
        assert_eq!(anchor_line(&out, "graphql_resolver:best"), Some(4));
    }

    const GRAPHENE_SCHEMA: &str = "import graphene\n\n\nclass User(graphene.ObjectType):\n    id = graphene.ID()\n    name = graphene.String()\n\n\nclass Query(graphene.ObjectType):\n    all_users = graphene.List(User)\n    user_by_id = graphene.Field(User, id=graphene.ID(required=True))\n\n    def resolve_all_users(root, info):\n        return []\n\n    def resolve_user_by_id(root, info, id):\n        return None\n\n\nschema = graphene.Schema(query=Query)\n";

    #[test]
    fn graphene_fields_anchor_at_resolve() {
        // bench/substrate-gap/fixtures/py-graphene-fields/server/schema.py.
        assert_resolvers(GRAPHENE_SCHEMA, "python", &["Query", "allUsers", "userById"]);
        let out = extract_graphql_resolver_nodes(GRAPHENE_SCHEMA, "python", module_id(), repo());
        assert_eq!(anchor_line(&out, "graphql_resolver:allUsers"), Some(12), "resolve_all_users");
        assert_eq!(anchor_line(&out, "graphql_resolver:userById"), Some(15), "resolve_user_by_id");
        assert_eq!(
            decorator_scan(GRAPHENE_SCHEMA, "python").1.marker("python").as_deref(),
            Some("[graphql-decorators] lang=python import=graphene roots=Query fields=2 rejected lang=0 import=0 position=0")
        );
        // No resolve_ method: the attribute line. Bare types, relay fields,
        // a multi-line call with an explicit name, and a non-field call.
        let shapes = "from graphene import ObjectType, String, List, relay\nclass Query(ObjectType):\n    node = relay.Node.Field()\n    version = String()\n    all_posts = relay.ConnectionField(PostConnection)\n    viewer = graphene.Field(\n        User,\n        name=\"me\",\n    )\n    cache = make_cache()\n    enum = graphene.Enum('E', [])\n\n    @staticmethod\n    async def resolve_version(root, info):\n        return \"1\"\n";
        assert_resolvers(shapes, "python", &["Query", "allPosts", "me", "node", "version"]);
        let out = extract_graphql_resolver_nodes(shapes, "python", module_id(), repo());
        assert_eq!(anchor_line(&out, "graphql_resolver:node"), Some(2));
        assert_eq!(anchor_line(&out, "graphql_resolver:version"), Some(13), "async resolve_version");
        assert_eq!(anchor_line(&out, "graphql_resolver:me"), Some(5));
    }

    #[test]
    fn graphene_mutation_field() {
        let source = "import graphene\n\nclass CreateUser(graphene.Mutation):\n    class Arguments:\n        name = graphene.String()\n\n    ok = graphene.Boolean()\n\n    def mutate(root, info, name):\n        return CreateUser(ok=True)\n\n\nclass Mutation(graphene.ObjectType):\n    create_user = CreateUser.Field()\n";
        assert_resolvers(source, "python", &["Mutation", "createUser"]);
        let out = extract_graphql_resolver_nodes(source, "python", module_id(), repo());
        assert_eq!(anchor_line(&out, "graphql_resolver:createUser"), Some(13));
    }

    #[test]
    fn auto_camel_case_off() {
        let strawberry = "import strawberry\nfrom strawberry.schema.config import StrawberryConfig\n@strawberry.type\nclass Query:\n    @strawberry.field\n    def current_user(self) -> User: ...\n    @strawberry.field(name=\"allUsers\")\n    def all_users(self) -> list[User]: ...\n\nschema = strawberry.Schema(query=Query, config=StrawberryConfig(auto_camel_case=False))\n";
        assert_resolvers(strawberry, "python", &["Query", "allUsers", "current_user"]);
        let graphene = "import graphene\nclass Query(graphene.ObjectType):\n    all_users = graphene.List(User)\n\nschema = graphene.Schema(query=Query, auto_camelcase = False)\n";
        assert_resolvers(graphene, "python", &["Query", "all_users"]);
    }

    #[test]
    fn non_root_class_fields_ignored() {
        let graphene = "import graphene\nclass User(graphene.ObjectType):\n    name = graphene.String()\n    posts = graphene.List(Post)\n\n    def resolve_posts(root, info):\n        return []\n";
        assert_resolvers(graphene, "python", &[]);
        let strawberry = "import strawberry\n@strawberry.type\nclass User:\n    @strawberry.field\n    def full_name(self) -> str: ...\n";
        assert_resolvers(strawberry, "python", &[]);
    }

    #[test]
    fn root_class_body_bounds() {
        // The root body ends at the next line at the class's indent.
        let after = "import graphene\nclass Query(graphene.ObjectType):\n    me = graphene.Field(User)\nclass Other(graphene.ObjectType):\n    them = graphene.Field(User)\nthose = graphene.Field(User)\n";
        assert_resolvers(after, "python", &["Query", "me"]);
        // A triple-quoted string and a bracketed continuation at column 0
        // stay inside the body.
        let docstring = "import graphene\nclass Query(graphene.ObjectType):\n    \"\"\"Root.\n\nMore text at column 0.\n\"\"\"\n    items = graphene.List(\n        Item,\n)\n    total = graphene.Int()\n";
        assert_resolvers(docstring, "python", &["Query", "items", "total"]);
    }

    #[test]
    fn camel_case_follows_the_libraries() {
        for (py, schema) in [
            ("current_user", "currentUser"),
            ("user_by_id", "userById"),
            ("field_2", "field2"),
            ("user_ID", "userId"),
            ("_private", "Private"),
            ("a__b", "a_B"),
            ("trailing_", "trailing_"),
            ("already", "already"),
            ("camelCase", "camelCase"),
        ] {
            assert_eq!(camel_case(py), schema, "{py}");
        }
        assert_eq!(schema_name("current_user", Some("me".into()), true), "me");
        assert_eq!(schema_name("current_user", None, false), "current_user");
    }
}
