use std::sync::OnceLock;

use glia_code_domain::{CodeNav, GRAPH_TYPE, node_kind};
use glia_core::{Confidence, Node, NodeId, RepoId};

use crate::anchor::{Anchor, line_of};

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
fn scan_operation_needles(source: &str) -> (Vec<(String, u32)>, NeedleTally) {
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
    /// strawberry / graphene root classes.
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
    /// Methods read under a field decorator.
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

pub fn extract_graphql_operation_nodes(
    source: &str,
    module_id: NodeId,
    repo: RepoId,
) -> GraphqlNodes {
    let mut nodes = Vec::new();
    let mut nav = CodeNav::default();
    let mut anchors = Vec::new();
    let mut seen = std::collections::HashSet::new();

    let (hits, tally) = scan_operation_needles(source);
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

    for (name, at) in extract_gql_template_operations(source) {
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
///   `class Query(graphene.ObjectType):`, names that root type.
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
    if family == DecoratorFamily::Ts {
        let fields = decorator_method_names(source);
        census.fields = fields.len();
        names.extend(fields);
    }
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
/// start with `@Query(` / `@Mutation(` / `@Subscription(`.
fn ts_root_decorators(lines: &[&str]) -> Vec<(&'static str, u32)> {
    let mut roots = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim_start();
        if let Some(&(_, root)) = ROOT_DECORATORS.iter().find(|(d, _)| t.starts_with(d)) {
            push_root(&mut roots, root, i);
        }
    }
    roots
}

/// LA.38: the root types Python classes declare, anchored at the class line.
/// strawberry: `@strawberry.type` (bare or called), then within 4 lines, past
/// blank lines and further decorators, `class <Root>`. graphene:
/// `class <Root>(...)` whose base list holds the token `ObjectType`.
fn py_root_classes(lines: &[&str], lib: GqlLib) -> Vec<(&'static str, u32)> {
    let mut roots = Vec::new();
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
                    push_root(&mut roots, root, j);
                }
            }
            GqlLib::Graphene => {
                if let Some(root) = graphene_root(t) {
                    push_root(&mut roots, root, i);
                }
            }
            GqlLib::NestGraphql | GqlLib::TypeGraphql => {}
        }
    }
    roots
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
/// its `` gql` `` tag.
fn extract_gql_template_operations(source: &str) -> Vec<(String, usize)> {
    let mut ops = Vec::new();
    let mut search_from = 0;
    while let Some(idx) = source[search_from..].find("gql`") {
        let tag = search_from + idx;
        let abs = tag + 4;
        if let Some(name) = extract_operation_from_body(&source[abs..]) {
            ops.push((name, tag));
        }
        search_from = abs;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> RepoId {
        RepoId(1)
    }
    fn module_id() -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "test")
    }

    #[test]
    fn detects_use_query() {
        let source = "const { data } = useQuery(GET_USERS);";
        let result = extract_graphql_operation_nodes(source, module_id(), repo());
        assert!(!result.nodes.is_empty());
    }

    #[test]
    fn trpc_hook_lines_mint_no_graphql_operation() {
        for source in [
            "const { data } = trpc.user.list.useQuery();",
            "const m = api.post.create.useMutation();",
        ] {
            let result = extract_graphql_operation_nodes(source, module_id(), repo());
            assert!(
                result.nodes.is_empty(),
                "{source} -> {:?}",
                result.nav.qname_by_id.values().collect::<Vec<_>>()
            );
        }
        // The Apollo shapes beside them still count.
        let mixed = "const a = trpc.user.list.useQuery();\nconst { data } = useQuery(GET_USERS);";
        let result = extract_graphql_operation_nodes(mixed, module_id(), repo());
        assert!(result.nav.qname_by_id.values().any(|q| q == "graphql_op:useQuery"));
        let apollo = "const res = await client.query({ query: GET_USERS });";
        let result = extract_graphql_operation_nodes(apollo, module_id(), repo());
        assert!(result.nav.qname_by_id.values().any(|q| q == "graphql_op:client.query"));
    }

    #[test]
    fn extracts_gql_template_name() {
        let source = r#"const GET_USERS = gql`query GetUsers { users { id name } }`;"#;
        let result = extract_graphql_operation_nodes(source, module_id(), repo());
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
    fn anchors_operations_at_their_minting_line() {
        let src = "const GET_USER = gql`\n  query getUser { u }\n`;\n\nexport function P() {\n  const { data } = useQuery(GET_USER);\n}\nconst OTHER = gql`query listUsers { u }`;";
        let out = extract_graphql_operation_nodes(src, module_id(), repo());
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
        assert_eq!(anchor_line(&out, "graphql_resolver:Query"), Some(3));
        // LA.38: `@Resolver(` is a decorator name, never a node.
        assert_eq!(anchor_line(&out, "graphql_resolver:Resolver"), None);

        let sdl = "const typeDefs = `\ntype Query {\n  getUser(id: ID!): User\n  listUsers: [User]\n}\n`;";
        let out = extract_graphql_resolver_nodes(sdl, "typescript", module_id(), repo());
        assert_eq!(anchor_line(&out, "graphql_resolver:getUser"), Some(2));
        assert_eq!(anchor_line(&out, "graphql_resolver:listUsers"), Some(3));
        assert_eq!(anchor_line(&out, "graphql_resolver:Query"), Some(1));
    }

    fn op_qnames(source: &str) -> Vec<String> {
        let out = extract_graphql_operation_nodes(source, module_id(), repo());
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
        let (hits, tally) = scan_operation_needles(mixed);
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
        let (hits, tally) = scan_operation_needles(rows);
        assert!(hits.is_empty());
        assert_eq!(
            tally.marker().as_deref(),
            Some("[graphql-ops] kept=0 rejected=3 hook=1 client=2 request=0 context=tanstack")
        );

        let users = "import { request, gql } from 'graphql-request';\nconst Q = gql(DOC);\nawait request(url, Q);";
        let (hits, tally) = scan_operation_needles(users);
        assert_eq!(hits, vec![("request".to_string(), 2)]);
        assert_eq!(
            tally.marker().as_deref(),
            Some("[graphql-ops] kept=1 rejected=0 hook=0 client=0 request=0 context=client+graphql-request+gql-tag")
        );

        // No needle hit, no marker line.
        assert_eq!(scan_operation_needles("const x = 1;").1.marker(), None);
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
        assert_eq!(anchor_line(&out, "graphql_resolver:Query"), Some(3));
        assert_eq!(anchor_line(&out, "graphql_resolver:Subscription"), Some(5));
        assert_eq!(anchor_line(&out, "graphql_resolver:author"), Some(8));
    }

    #[test]
    fn strawberry_and_graphene_root_classes() {
        let strawberry = "import strawberry\n\n@strawberry.type\nclass Recipe:\n    title: str\n\n@strawberry.type\nclass Query:\n    @strawberry.field\n    def recipe(self) -> Recipe: ...\n\n@strawberry.type\nclass Mutation:\n    @strawberry.mutation\n    def add(self) -> Recipe: ...";
        // HEAD: strawberry.type + strawberry.mutation.
        assert_resolvers(strawberry, "python", &["Mutation", "Query"]);
        let out = extract_graphql_resolver_nodes(strawberry, "python", module_id(), repo());
        assert_eq!(anchor_line(&out, "graphql_resolver:Query"), Some(7));
        assert_eq!(anchor_line(&out, "graphql_resolver:Mutation"), Some(12));
        // HEAD: ObjectType.
        let graphene = "import graphene\nclass User(graphene.ObjectType):\n    name = graphene.String()\nclass Query(graphene.ObjectType):\n    user = graphene.Field(User)";
        assert_resolvers(graphene, "python", &["Query"]);
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
}
