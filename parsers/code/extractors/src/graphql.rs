use std::sync::OnceLock;

use glia_code_domain::{CodeNav, GRAPH_TYPE, cell_type, node_kind};
use glia_core::{Cell, CellPayload, Confidence, Node, NodeId, RepoId};

use crate::anchor::{Anchor, line_of};
use crate::cli::{CodeMap, Script};
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
    /// DH.8: set on the first kept hit of a Dart file that imports
    /// package:graphql ([`dart_imports_graphql`]).
    dart: Option<DartDocTally>,
}

/// DH.8: how the kept needles of a package:graphql Dart file were named, for
/// the `[graphql-ops] dart-document` marker: from a same-file constant
/// (`gql(listOrders)`), from an inline literal (`gql(r'''query X {..}''')`),
/// or neither (the needle's fallback name).
#[derive(Default, Clone, Copy)]
struct DartDocTally {
    constant: usize,
    inline: usize,
    unread: usize,
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

    /// DH.8: the `[graphql-ops] dart-document` line, or `None` when the file
    /// is no package:graphql Dart file with a kept needle.
    fn dart_marker(&self) -> Option<String> {
        let d = self.dart?;
        Some(format!(
            "[graphql-ops] dart-document const={} inline={} unread={}",
            d.constant, d.inline, d.unread
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
/// line, the document the name was read from) in source order, and the tally
/// for the `[graphql-ops]` marker. DT.11a: a line named by its needle text
/// read no document (`None`).
/// CJ.1b: a needle occurrence `guard` refuses (it starts in a Rust / Python
/// string literal or comment) carries no evidence.
///
/// A kept line is named by [`extract_gql_operation_name`], else by its
/// needle. DH.8: in a Dart file that imports package:graphql (or
/// graphql_flutter) the kept call's own `gql(..)` document names it first
/// ([`dart_document_name`]): `client.query(QueryOptions(document:
/// gql(listOrders)))` is the operation its `listOrders` constant declares.
fn scan_operation_needles<'s>(
    source: &'s str,
    guard: &mut LazyGuard<'_>,
) -> (Vec<(String, u32, Option<&'s str>)>, NeedleTally) {
    let mut hits = Vec::new();
    let mut tally = NeedleTally::default();
    let b = source.as_bytes();
    // DH.8: whether the file's package:graphql import was read, on its first
    // kept hit.
    let mut dart_checked = false;
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
            let kept = line.match_indices(pattern).map(|(at, _)| line_start + at).find(|&hit| {
                if !guard.admits(hit) {
                    return false;
                }
                let arg = first_arg_shape(source, hit + pattern.len());
                accept(family, arg, ctx, ident_byte_before(b, hit))
            });
            tally.record(family, kept.is_some());
            if let Some(hit) = kept {
                if !dart_checked {
                    dart_checked = true;
                    tally.dart = dart_imports_graphql(source).then(DartDocTally::default);
                }
                // The needle's `(` is its last byte.
                let document = tally.dart.as_mut().map(|d| {
                    let found = dart_document_name(source, hit + pattern.len() - 1);
                    match found.as_ref().map(|(_, how, _)| *how) {
                        Some(DartDoc::Constant) => d.constant += 1,
                        Some(DartDoc::Inline) => d.inline += 1,
                        None => d.unread += 1,
                    }
                    found.map(|(name, _, doc)| (name, Some(doc)))
                });
                let (op_name, doc) = document
                    .flatten()
                    .or_else(|| extract_gql_operation_name(source))
                    .unwrap_or_else(|| (pattern.trim_end_matches('(').to_string(), None));
                hits.push((op_name, u32::try_from(line_no).unwrap_or(u32::MAX), doc));
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
/// the decorators and classes read here. Java / Kotlin / PHP / Go / Dart
/// servers use other spellings (`@QueryMapping`, `@DgsQuery`), and a Java
/// `@Query(` is Spring Data / Micronaut Data / Room.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DecoratorFamily {
    /// NestJS / TypeGraphQL decorators.
    Ts,
    /// strawberry / graphene root classes and their fields (CB.14).
    Py,
    /// HotChocolate root classes and their public members (CL.6b).
    CSharp,
    /// async-graphql `#[Object]` / `#[Subscription]` root impls and their
    /// methods (DG.4).
    Rust,
    /// Caliban `RootResolver(..)` case classes and their parameters (DG.4).
    Scala,
    /// graphql-ruby root classes and their `field` calls (DH.8).
    Ruby,
    /// Absinthe `query do` / `mutation do` / `subscription do` blocks and
    /// their `field`s (DH.8).
    Elixir,
}

/// LA.38: the GraphQL server package a file imports.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum GqlLib {
    NestGraphql,
    TypeGraphql,
    Strawberry,
    Graphene,
    HotChocolate,
    AsyncGraphql,
    Caliban,
    GraphqlRuby,
    Absinthe,
}

impl GqlLib {
    fn label(self) -> &'static str {
        match self {
            GqlLib::NestGraphql => "@nestjs/graphql",
            GqlLib::TypeGraphql => "type-graphql",
            GqlLib::Strawberry => "strawberry",
            GqlLib::Graphene => "graphene",
            GqlLib::HotChocolate => "hotchocolate",
            GqlLib::AsyncGraphql => "async-graphql",
            GqlLib::Caliban => "caliban",
            GqlLib::GraphqlRuby => "graphql-ruby",
            GqlLib::Absinthe => "absinthe",
        }
    }
}

/// LA.38: what one file's decorator needles came to, for the
/// `[graphql-decorators]` marker. Each needle occurrence lands in at most one
/// rejection count: a file of another language (`rej_lang`), a file with no
/// GraphQL server import (`rej_import`), or an occurrence off decorator /
/// class position (`rej_position`: a comment, a string, mid-line). A C# file
/// counts its root markers ([`CS_ROOT_NEEDLES`]) instead, `rej_position`
/// being those in a comment, string or preprocessor line (CL.6b); a Rust
/// file its root attributes ([`rs_root_attr_sites`]) and a Scala file its
/// `RootResolver` calls ([`scala_root_resolver_sites`]), `rej_position`
/// being those in a comment or string (DG.4); a Ruby file its root-named
/// `class` declarations ([`ruby_root_class_sites`]) and an Elixir file its
/// root block openers ([`absinthe_root_sites`]), the same way (DH.8).
#[derive(Default, Debug)]
struct DecoratorCensus {
    /// Occurrences of [`DECORATOR_NEEDLES`], or of the language's root
    /// markers in a C# / Rust / Scala / Ruby / Elixir file.
    hits: usize,
    lib: Option<GqlLib>,
    /// Root nouns kept, in line order.
    roots: Vec<&'static str>,
    /// Field resolvers read: TypeScript methods under a field decorator,
    /// Python root-class fields (CB.14), C# root-class members (CL.6b), Rust
    /// root-impl methods and Scala root case-class parameters (DG.4),
    /// graphql-ruby root-class and Absinthe root-block fields (DH.8).
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
/// ([`scan_operation_needles`]), the operations of `` gql` `` templates
/// ([`extract_gql_template_operations`]) and the literal documents of Go
/// machinebox / C# GraphQL.Client requests ([`client_request_operations`],
/// CL.6a).
///
/// DT.11a: every node minted from a document carries that document as its
/// CODE cell ([`document_cells`]), the one carrier of the document: the
/// GraphQL resolver pairs the operation by the root fields it selects
/// ([`operation_root_fields`]). A needle named by its needle text read no
/// document and carries none.
///
/// `path` selects CJ.1b's literal / comment guard: in a Rust or Python file an
/// operation needle, `` gql` `` tag or client request that starts in a string
/// literal or comment mints nothing (`""` = no guard). Resolver / SDL
/// extraction is never guarded: an SDL schema legitimately lives in a string
/// literal. fired_on, once per call that refused one:
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
    if graphql_debug() {
        for marker in [tally.marker(), tally.dart_marker()].into_iter().flatten() {
            eprintln!("{marker}");
        }
    }
    for (op_name, line, doc) in hits {
        if seen.insert(op_name.clone()) {
            let qname = format!("graphql_op:{op_name}");
            let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::GRAPHQL_OPERATION, &qname);
            nodes.push(Node {
                id,
                repo,
                confidence: Confidence::Medium,
                cells: document_cells(doc),
            });
            nav.record(id, &op_name, &qname, node_kind::GRAPHQL_OPERATION, Some(module_id));
            anchors.push(Anchor { node: id, line });
        }
    }

    let templates = extract_gql_template_operations(source, &mut guard);
    let (requests, request_tally) = client_request_operations(source, &mut guard);
    guard.report("graphql_op");
    if graphql_debug()
        && let Some(marker) = request_tally.marker()
    {
        eprintln!("{marker}");
    }
    let templates = templates.into_iter().map(|(name, at, doc)| (name, at, doc, Confidence::Strong));
    let requests = requests.iter().map(|(name, at, doc)| (name.clone(), *at, Some(doc.as_str()), Confidence::Medium));
    for (name, at, doc, confidence) in templates.chain(requests) {
        if seen.insert(name.clone()) {
            let qname = format!("graphql_op:{name}");
            let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::GRAPHQL_OPERATION, &qname);
            nodes.push(Node {
                id,
                repo,
                confidence,
                cells: document_cells(doc),
            });
            nav.record(id, &name, &qname, node_kind::GRAPHQL_OPERATION, Some(module_id));
            anchors.push(Anchor { node: id, line: line_of(source, at) });
        }
    }

    GraphqlNodes { nodes, nav, anchors }
}

/// DT.11a: an operation's cells: its document as one CODE text cell, or none
/// when it was read from no document.
fn document_cells(doc: Option<&str>) -> Vec<Cell> {
    doc.map(|d| Cell { kind: cell_type::CODE, payload: CellPayload::Text(d.to_string()) })
        .into_iter()
        .collect()
}

/// CODE mode, for every code file of every language
/// (`engine::extract::apply_cross_cutting_extractors`, `lang` from
/// `detect_language`): the decorator family ([`decorator_scan`], LA.38) and
/// SDL only inside a GraphQL-marked literal ([`gql_literal_regions`], LA.27).
/// A comment or an unmarked string that holds `type Query {` mints nothing,
/// and neither does a decorator needle outside a TypeScript / Python / C# /
/// Rust / Scala / Ruby / Elixir file that imports a GraphQL server package. A `.graphql` / `.gql` body goes
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
/// marker. It runs only for a file of a [`decorator_family`] language that
/// imports a GraphQL server package ([`graphql_server_lib`]), and reads only
/// at decorator / class position:
/// - TypeScript (NestJS / TypeGraphQL): a line starting with `@Query(` /
///   `@Mutation(` / `@Subscription(` names that root type, and the method
///   under a field decorator ([`decorator_method_names`]) is a field;
/// - Python: `@strawberry.type` on `class Query:`, or graphene
///   `class Query(graphene.ObjectType):`, names that root type, and the
///   root class's field resolvers ([`py_field_resolvers`], CB.14) are fields
///   under their schema names;
/// - C# (HotChocolate, CL.6b): a root class ([`cs_root_classes`]) names its
///   root type, and its public members ([`cs_root_fields`]) are fields under
///   HotChocolate's names;
/// - Rust (async-graphql, DG.4): an `#[Object]` / `#[Subscription]` impl of
///   a root type names it, and its methods are fields under async-graphql's
///   names ([`async_graphql_roots_and_fields`]), all read through CJ.1's
///   literal / comment guard;
/// - Scala (Caliban, DG.4): a `RootResolver(..)` argument names its root
///   type, and the parameters of its in-file case class are fields
///   ([`caliban_roots_and_fields`]);
/// - Ruby (graphql-ruby, DH.8): a `QueryType` / `MutationType` /
///   `SubscriptionType` class (or one named exactly `Query`, ..) inheriting
///   a `*BaseObject` / `GraphQL::Schema::Object` names its root type, and
///   its body's `field :name` calls are fields under graphql-ruby's camelised
///   names ([`graphql_ruby_roots_and_fields`]);
/// - Elixir (Absinthe, DH.8): a `query do` / `mutation do` /
///   `subscription do` block names its root type, and the `field :name`
///   statements at its first level are fields under Absinthe's external
///   names ([`absinthe_roots_and_fields`]); both read through a code mask.
///
/// Every noun is a root type: `@Resolver(`, `@ResolveField(`,
/// `@strawberry.type`, `@strawberry.mutation`, `ObjectType` and an object
/// type extension name nothing.
fn decorator_scan(source: &str, lang: &str) -> (Vec<(String, u32)>, DecoratorCensus) {
    let family = decorator_family(lang);
    let root_sites = match family {
        Some(DecoratorFamily::CSharp) => cs_root_needle_sites(source),
        Some(DecoratorFamily::Rust) => rs_root_attr_sites(source),
        Some(DecoratorFamily::Scala) => scala_root_resolver_sites(source),
        Some(DecoratorFamily::Ruby) => ruby_root_class_sites(source),
        Some(DecoratorFamily::Elixir) => absinthe_root_sites(source),
        _ => Vec::new(),
    };
    let mut census = DecoratorCensus {
        hits: match family {
            Some(
                DecoratorFamily::CSharp
                | DecoratorFamily::Rust
                | DecoratorFamily::Scala
                | DecoratorFamily::Ruby
                | DecoratorFamily::Elixir,
            ) => root_sites.len(),
            _ => DECORATOR_NEEDLES.iter().map(|n| source.matches(n).count()).sum(),
        },
        ..DecoratorCensus::default()
    };
    let Some(family) = family else {
        census.rej_lang = census.hits;
        return (Vec::new(), census);
    };
    let Some(lib) = graphql_server_lib(source, family) else {
        census.rej_import = census.hits;
        return (Vec::new(), census);
    };
    census.lib = Some(lib);
    let lines: Vec<&str> = source.lines().collect();
    let (roots, fields) = match family {
        DecoratorFamily::Ts => {
            census.rej_position = misplaced_needles(&lines);
            (ts_root_decorators(&lines), decorator_method_names(source))
        }
        DecoratorFamily::Py => {
            census.rej_position = misplaced_needles(&lines);
            (py_root_classes(&lines, lib), py_field_resolvers(&lines, lib))
        }
        DecoratorFamily::CSharp => {
            let mask = cs_code_mask(source);
            census.rej_position = root_sites.iter().filter(|&&at| mask.get(at) == Some(&b' ')).count();
            hc_roots_and_fields(source, &mask)
        }
        DecoratorFamily::Rust => {
            let mut guard = LazyGuard::new(RS_GUARD_PATH, source);
            let mask = code_mask(source, |at| guard.admits(at));
            census.rej_position = root_sites.iter().filter(|&&at| mask.get(at) == Some(&b' ')).count();
            async_graphql_roots_and_fields(source, &mask)
        }
        DecoratorFamily::Scala => {
            let map = CodeMap::new(source);
            let mask = code_mask(source, |at| map.is_code(at));
            census.rej_position = root_sites.iter().filter(|&&at| mask.get(at) == Some(&b' ')).count();
            caliban_roots_and_fields(source, &mask, &root_sites)
        }
        DecoratorFamily::Ruby => {
            let mask = ruby_mask(source);
            census.rej_position = root_sites.iter().filter(|&&at| mask.get(at) == Some(&b' ')).count();
            graphql_ruby_roots_and_fields(source, &mask)
        }
        DecoratorFamily::Elixir => {
            let mask = elixir_code_mask(source);
            census.rej_position = root_sites.iter().filter(|&&at| mask.get(at) == Some(&b' ')).count();
            absinthe_roots_and_fields(source, &mask, &root_sites)
        }
    };
    census.roots = roots.iter().map(|&(root, _)| root).collect();
    let mut names: Vec<(String, u32)> = roots
        .into_iter()
        .map(|(root, line)| (root.to_string(), line))
        .collect();
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
        "csharp" => Some(DecoratorFamily::CSharp),
        "rust" => Some(DecoratorFamily::Rust),
        "scala" => Some(DecoratorFamily::Scala),
        "ruby" => Some(DecoratorFamily::Ruby),
        "elixir" => Some(DecoratorFamily::Elixir),
        _ => None,
    }
}

/// LA.38: the GraphQL server package the file imports. TypeScript: a quoted
/// `@nestjs/graphql` / `type-graphql` module string, so a multi-line import
/// counts and a REST controller importing `Query` from `@nestjs/common` does
/// not. Python: a line starting with `import` / `from` and the package name
/// as a whole token (`import strawberry as sb`, `from graphene import ...`).
/// C#: a `using` of the `HotChocolate` namespace ([`cs_imports_hotchocolate`]).
/// Rust: an `async_graphql` crate path or `use` at code position
/// ([`rs_imports_async_graphql`]). Scala: an `import caliban` line
/// ([`scala_imports_caliban`]). Ruby: `GraphQL::Schema` at code position or
/// a class inheriting a `*BaseObject` ([`ruby_imports_graphql`]). Elixir:
/// `use Absinthe.Schema` at code position ([`absinthe_schema`]).
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
        DecoratorFamily::CSharp => cs_imports_hotchocolate(source).then_some(GqlLib::HotChocolate),
        DecoratorFamily::Rust => rs_imports_async_graphql(source).then_some(GqlLib::AsyncGraphql),
        DecoratorFamily::Scala => scala_imports_caliban(source).then_some(GqlLib::Caliban),
        // A file that never spells the markers is never lexed.
        DecoratorFamily::Ruby => ((source.contains("GraphQL::Schema") || source.contains("BaseObject"))
            && ruby_imports_graphql(source, &ruby_mask(source)))
        .then_some(GqlLib::GraphqlRuby),
        DecoratorFamily::Elixir => (source.contains("Absinthe.Schema") && absinthe_schema(&elixir_code_mask(source)))
            .then_some(GqlLib::Absinthe),
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
            GqlLib::NestGraphql
            | GqlLib::TypeGraphql
            | GqlLib::HotChocolate
            | GqlLib::AsyncGraphql
            | GqlLib::Caliban
            | GqlLib::GraphqlRuby
            | GqlLib::Absinthe => {}
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
            GqlLib::NestGraphql
            | GqlLib::TypeGraphql
            | GqlLib::HotChocolate
            | GqlLib::AsyncGraphql
            | GqlLib::Caliban
            | GqlLib::GraphqlRuby
            | GqlLib::Absinthe => {}
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

// ---- CL.6b: HotChocolate code-first root types ----

/// CL.6b: HotChocolate attributes that make the class they sit on a root
/// type (the source generator's `[QueryType]` static classes).
const HC_ROOT_ATTRIBUTES: &[(&str, &str)] = &[
    ("QueryType", "Query"),
    ("MutationType", "Mutation"),
    ("SubscriptionType", "Subscription"),
];

/// CL.6b: the C# root markers the `[graphql-decorators]` census counts, each
/// a whole word at both ends.
const CS_ROOT_NEEDLES: &[&str] = &[
    "[QueryType",
    "[MutationType",
    "[SubscriptionType",
    "ExtendObjectType",
    "class Query",
    "class Mutation",
    "class Subscription",
];

/// CL.6b: the modifiers a C# class declaration may open with.
const CS_CLASS_MODIFIERS: &[&str] = &[
    "public", "internal", "private", "protected", "static", "partial", "sealed", "abstract", "file", "new",
    "unsafe",
];

/// CL.6b: words before a member's name that make it no GraphQL field: a
/// nested type, an event, an operator, a constant.
const CS_NOT_A_FIELD: &[&str] = &[
    "class", "struct", "interface", "enum", "record", "delegate", "event", "operator", "implicit", "explicit",
    "const",
];

/// CL.6b: words a `(` can follow in a member header that do not name a
/// method (a tuple return type follows a modifier; `this(` / `base(` chain a
/// constructor).
const CS_KEYWORDS: &[&str] = &[
    "public", "internal", "private", "protected", "static", "async", "override", "virtual", "sealed", "abstract",
    "new", "extern", "unsafe", "readonly", "partial", "required", "this", "base", "typeof", "nameof", "default",
    "where", "ref", "out", "in", "params",
];

/// CL.6b: `System.Object` methods, which HotChocolate never exposes.
const CS_OBJECT_METHODS: &[&str] = &["ToString", "GetHashCode", "Equals", "GetType"];

/// CL.6b: return types with no value, which make no field.
const CS_NO_VALUE_TYPES: &[&str] = &["void", "Task", "ValueTask"];

/// CL.6b: a `using` directive of the `HotChocolate` namespace or one under
/// it, in its `global`, `static` and aliased (`using HC = HotChocolate.Types;`)
/// forms; `HotChocolateX` is another namespace.
fn cs_imports_hotchocolate(source: &str) -> bool {
    let word = |t: &str, w: &str| -> Option<String> {
        t.strip_prefix(w)
            .filter(|rest| rest.starts_with(char::is_whitespace))
            .map(|rest| rest.trim_start().to_string())
    };
    source.lines().any(|line| {
        let t = line.trim_start();
        let t = word(t, "global").unwrap_or_else(|| t.to_string());
        let Some(rest) = word(&t, "using") else {
            return false;
        };
        let rest = word(&rest, "static").unwrap_or(rest);
        let rest = match rest.split_once('=') {
            Some((alias, value)) if is_ident(alias.trim()) => value.trim_start().to_string(),
            _ => rest,
        };
        rest.strip_prefix("HotChocolate")
            .is_some_and(|after| !after.bytes().next().is_some_and(is_ident_byte))
    })
}

/// CL.6b: the byte offsets of every [`CS_ROOT_NEEDLES`] occurrence.
fn cs_root_needle_sites(source: &str) -> Vec<usize> {
    let b = source.as_bytes();
    let mut sites: Vec<usize> = CS_ROOT_NEEDLES
        .iter()
        .flat_map(|needle| {
            source
                .match_indices(needle)
                .map(|(at, n)| (at, at + n.len()))
                .collect::<Vec<_>>()
        })
        .filter(|&(at, end)| !ident_byte_before(b, at) && !b.get(end).is_some_and(|&c| is_ident_byte(c)))
        .map(|(at, _)| at)
        .collect();
    sites.sort_unstable();
    sites
}

/// CL.6b: `source` with every C# comment, string / char literal
/// ([`cs_skip_opaque`]) and preprocessor line overwritten with blanks,
/// newlines kept, so an offset means the same in both and every brace,
/// bracket and keyword left in the mask is code.
fn cs_code_mask(source: &str) -> Vec<u8> {
    let b = source.as_bytes();
    let mut mask = b.to_vec();
    let mut line_start = true;
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        let opaque = if line_start && c == b'#' {
            Some(b.get(i..).and_then(|r| r.iter().position(|&x| x == b'\n')).map_or(b.len(), |p| i + p))
        } else {
            cs_skip_opaque(b, i)
        };
        if let Some(end) = opaque {
            for x in mask.iter_mut().take(end).skip(i) {
                if *x != b'\n' {
                    *x = b' ';
                }
            }
            line_start = false;
            i = end.max(i + 1);
            continue;
        }
        if c == b'\n' {
            line_start = true;
        } else if !c.is_ascii_whitespace() {
            line_start = false;
        }
        i += 1;
    }
    mask
}

/// CL.6b: how a C# declaration ends: a `{ .. }` body (the offsets of both
/// braces), a `;`, or an `=>` expression body.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CsEnd {
    Body(usize, usize),
    Semi,
    Arrow,
}

/// CL.6b: one C# declaration: its header (attributes, modifiers, type, name,
/// parameters) as a byte range of the mask, and how it ends.
struct CsDecl {
    head: (usize, usize),
    end: CsEnd,
}

/// CL.6b: the declarations between `from` and `to` of the mask, one brace
/// level: a header runs to the first `;`, `=>` or `{` outside its brackets;
/// a body is skipped to its `}`, an expression body to its `;`.
fn cs_decls(m: &[u8], from: usize, to: usize) -> Vec<CsDecl> {
    let mut decls = Vec::new();
    let (mut start, mut i, mut depth) = (from, from, 0usize);
    while i < to {
        let Some(&c) = m.get(i) else {
            break;
        };
        match c {
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth = depth.saturating_sub(1),
            b'{' if depth == 0 => {
                let close = cs_matching_close(m, i).map_or(to, |c| c.min(to));
                decls.push(CsDecl { head: (start, i), end: CsEnd::Body(i, close) });
                i = close + 1;
                start = i;
                continue;
            }
            b'}' if depth == 0 => start = i + 1,
            b';' if depth == 0 => {
                decls.push(CsDecl { head: (start, i), end: CsEnd::Semi });
                start = i + 1;
            }
            b'=' if depth == 0 && m.get(i + 1) == Some(&b'>') => {
                decls.push(CsDecl { head: (start, i), end: CsEnd::Arrow });
                i = cs_statement_end(m, i + 2, to) + 1;
                start = i;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    decls
}

/// CL.6b: the `;` ending the expression that starts at `from`, outside its
/// brackets (a switch expression or object initializer holds braces), or
/// `to` when there is none.
fn cs_statement_end(m: &[u8], from: usize, to: usize) -> usize {
    let mut depth = 0usize;
    for (i, &c) in m.iter().enumerate().take(to).skip(from) {
        match c {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => match depth.checked_sub(1) {
                Some(d) => depth = d,
                None => return i,
            },
            b';' if depth == 0 => return i,
            _ => {}
        }
    }
    to
}

/// CL.6b: one C# attribute: its name past any namespace and without the
/// `Attribute` suffix, and its argument text (`(..)` / `<..>`) read from the
/// source, string literals intact.
struct CsAttr<'a> {
    name: &'a str,
    args: &'a str,
}

/// CL.6b: the attribute lists a declaration header opens with, and the
/// offset past them.
fn cs_head_attrs<'a>(src: &'a str, m: &[u8], start: usize, end: usize) -> (Vec<CsAttr<'a>>, usize) {
    let mut attrs = Vec::new();
    let mut i = skip_ascii_ws(m, start);
    while i < end && m.get(i) == Some(&b'[') {
        let Some(close) = cs_matching_close(m, i).filter(|&c| c < end) else {
            break;
        };
        let (mut item, mut depth) = (i + 1, 0usize);
        for j in i + 1..=close {
            match m.get(j) {
                Some(b'(' | b'[' | b'{' | b'<') => depth += 1,
                Some(b')' | b']' | b'}' | b'>') if j < close => depth = depth.saturating_sub(1),
                _ => {}
            }
            if j == close || (depth == 0 && m.get(j) == Some(&b',')) {
                attrs.extend(cs_attr(src, m, item, j));
                item = j + 1;
            }
        }
        i = skip_ascii_ws(m, close + 1);
    }
    (attrs, i)
}

/// CL.6b: the attribute between `from` and `to` (one item of a list), past
/// an attribute target (`return:`).
fn cs_attr<'a>(src: &'a str, m: &[u8], from: usize, to: usize) -> Option<CsAttr<'a>> {
    let ident_end = |at: usize| cs_ident_end(m, at, to);
    let mut i = skip_ascii_ws(m, from);
    let target = skip_ascii_ws(m, ident_end(i));
    if ident_end(i) > i && m.get(target) == Some(&b':') && m.get(target + 1) != Some(&b':') {
        i = skip_ascii_ws(m, target + 1);
    }
    let dotted = i + m
        .get(i..to)?
        .iter()
        .take_while(|&&c| is_ident_byte(c) || c == b'.' || c == b':')
        .count();
    let last = src.get(i..dotted)?.rsplit(['.', ':']).next()?;
    let name = last.strip_suffix("Attribute").filter(|n| !n.is_empty()).unwrap_or(last);
    let args = src.get(dotted..to)?.trim();
    (!name.is_empty()).then_some(CsAttr { name, args })
}

/// CL.6b: the end of the identifier starting at `at`, no further than `to`.
fn cs_ident_end(m: &[u8], at: usize, to: usize) -> usize {
    at + m.get(at..to).map_or(0, |r| r.iter().take_while(|&&c| is_ident_byte(c)).count())
}

/// CL.6b: the arguments of `inner` (an argument list without its brackets)
/// split at its top-level commas.
fn cs_args(inner: &str) -> Vec<&str> {
    let mut args = Vec::new();
    let (mut depth, mut start, mut quoted) = (0i32, 0usize, false);
    for (i, c) in inner.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '(' | '[' | '{' | '<' if !quoted => depth += 1,
            ')' | ']' | '}' | '>' if !quoted => depth -= 1,
            ',' if !quoted && depth == 0 => {
                args.extend(inner.get(start..i).map(str::trim));
                start = i + 1;
            }
            _ => {}
        }
    }
    args.extend(inner.get(start..).map(str::trim));
    args
}

/// CL.6b: the root type a class's attributes make it: `[QueryType]` /
/// `[MutationType]` / `[SubscriptionType]`, or an `[ExtendObjectType]` of a
/// root type, its name spelled `"Query"`, `OperationTypeNames.Query`,
/// `typeof(Query)`, `nameof(Query)`, `Name = "Query"` or `<Query>`.
fn hc_attr_root(attrs: &[CsAttr<'_>]) -> Option<&'static str> {
    attrs.iter().find_map(|a| {
        if let Some(&(_, root)) = HC_ROOT_ATTRIBUTES.iter().find(|(name, _)| *name == a.name) {
            return Some(root);
        }
        if a.name != "ExtendObjectType" {
            return None;
        }
        let inner = match a.args.strip_prefix('<') {
            Some(generic) => generic.get(..generic.find('>')?)?,
            None => a.args.strip_prefix('(')?.strip_suffix(')')?,
        };
        let args = cs_args(inner);
        let named = args.iter().find_map(|arg| {
            let value = arg.strip_prefix("Name")?.trim_start().strip_prefix('=')?;
            (!value.starts_with('=')).then(|| value.trim())
        });
        let value = named.or_else(|| args.first().copied().filter(|arg| !arg.contains('=')))?;
        let value = ["typeof(", "nameof("]
            .iter()
            .find_map(|call| value.strip_prefix(call)?.strip_suffix(')'))
            .map_or(value, str::trim);
        let name = quoted_literal(value).unwrap_or_else(|| value.rsplit(['.', ':']).next().unwrap_or(value));
        ROOT_TYPES.iter().copied().find(|root| *root == name)
    })
}

/// CL.6b: a `[GraphQLName("..")]` among `attrs`, when its value is a
/// GraphQL name.
fn hc_graphql_name(attrs: &[CsAttr<'_>]) -> Option<String> {
    attrs.iter().filter(|a| a.name == "GraphQLName").find_map(|a| {
        let value = quoted_literal(a.args.strip_prefix('(')?.strip_suffix(')')?.trim())?;
        py_ident(value)
            .is_some_and(|(_, rest)| rest.is_empty())
            .then(|| value.to_string())
    })
}

/// CL.6b: a class declaration header from `from` (past its attributes):
/// modifiers ([`CS_CLASS_MODIFIERS`]), then `class <Name>`. The name, the
/// offset of the `class` keyword, and whether the class is `static`.
fn cs_class_head<'a>(src: &'a str, m: &[u8], from: usize, to: usize) -> Option<(&'a str, usize, bool)> {
    let ident_end = |at: usize| cs_ident_end(m, at, to);
    let mut i = skip_ascii_ws(m, from);
    let mut is_static = false;
    loop {
        let end = ident_end(i);
        let word = src.get(i..end)?;
        if word == "class" {
            let at = skip_ascii_ws(m, end);
            let name = src.get(at..ident_end(at)).filter(|n| is_ident(n))?;
            return Some((name, i, is_static));
        }
        if !CS_CLASS_MODIFIERS.contains(&word) {
            return None;
        }
        is_static |= word == "static";
        i = skip_ascii_ws(m, end);
    }
}

/// CL.6b: one HotChocolate root class.
struct CsRoot<'a> {
    root: &'static str,
    name: &'a str,
    /// The offset of its `class` keyword, where the root noun anchors.
    class_at: usize,
    is_static: bool,
    /// Its `{` and `}`.
    body: (usize, usize),
}

/// CL.6b: the root classes of a C# file, in source order, through namespace
/// blocks and enclosing classes: a class whose attributes make it a root
/// ([`hc_attr_root`]), else one named exactly `Query` / `Mutation` /
/// `Subscription`. A per-file scan: `AddQueryType<RootQuery>()` in another
/// file does not make `RootQuery` a root.
fn cs_root_classes<'a>(src: &'a str, m: &[u8]) -> Vec<CsRoot<'a>> {
    fn walk<'a>(src: &'a str, m: &[u8], span: (usize, usize), nesting: usize, out: &mut Vec<CsRoot<'a>>) {
        if nesting > 64 {
            return;
        }
        for decl in cs_decls(m, span.0, span.1) {
            let CsEnd::Body(open, close) = decl.end else {
                continue;
            };
            let (attrs, at) = cs_head_attrs(src, m, decl.head.0, decl.head.1);
            if let Some((name, class_at, is_static)) = cs_class_head(src, m, at, decl.head.1) {
                let root = hc_attr_root(&attrs).or_else(|| ROOT_TYPES.iter().copied().find(|r| *r == name));
                if let Some(root) = root {
                    out.push(CsRoot { root, name, class_at, is_static, body: (open, close) });
                }
            } else if !src.get(at..decl.head.1).is_some_and(|h| {
                h.strip_prefix("namespace").is_some_and(|rest| rest.starts_with(char::is_whitespace))
            }) {
                continue;
            }
            walk(src, m, (open + 1, close), nesting + 1, out);
        }
    }
    let mut out = Vec::new();
    walk(src, m, (0, m.len()), 0, &mut out);
    out
}

/// CL.6b: a header's top-level tokens: an identifier, a bracketed group
/// (`(..)` / `[..]` / `<..>`, by its opening byte), or a punctuation byte.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CsTok {
    Word(usize, usize),
    Group(u8),
    Punct(u8),
}

fn cs_head_tokens(m: &[u8], from: usize, to: usize) -> Vec<CsTok> {
    let mut toks = Vec::new();
    let mut i = from;
    while i < to {
        let Some(&c) = m.get(i) else {
            break;
        };
        if is_ident_byte(c) {
            let end = cs_ident_end(m, i, to);
            toks.push(CsTok::Word(i, end));
            i = end;
            continue;
        }
        if matches!(c, b'(' | b'[' | b'<') {
            let mut depth = 0usize;
            let mut j = i;
            while j < to {
                match m.get(j) {
                    Some(b'(' | b'[' | b'<') => depth += 1,
                    Some(b')' | b']' | b'>') => depth = depth.saturating_sub(1),
                    _ => {}
                }
                j += 1;
                if depth == 0 {
                    break;
                }
            }
            toks.push(CsTok::Group(c));
            i = j;
            continue;
        }
        if !c.is_ascii_whitespace() {
            toks.push(CsTok::Punct(c));
        }
        i += 1;
    }
    toks
}

/// CL.6b: the member a root-class declaration header (past its attributes)
/// declares, when HotChocolate makes it a field: its name, the name's offset
/// and whether it is a method. A `public` method (`Name(` or `Name<T>(` after
/// the return type) or property (`Name {` / `Name =>`); never a constructor or
/// destructor, a `static` member of a non-static class, a field, an indexer,
/// an initializer, a nested type, an event, an operator, a constant, a
/// `System.Object` method or a method returning `void` / `Task` / `ValueTask`.
fn cs_member<'a>(
    src: &'a str,
    m: &[u8],
    span: (usize, usize),
    end: CsEnd,
    class: &CsRoot<'_>,
) -> Option<(&'a str, usize, bool)> {
    let toks = cs_head_tokens(m, span.0, span.1);
    let word = |t: &CsTok| match *t {
        CsTok::Word(s, e) => src.get(s..e),
        _ => None,
    };
    if toks.contains(&CsTok::Punct(b'=')) {
        return None;
    }
    // A method: the first `(` group whose name (past a `<..>` group) is no
    // keyword. Else a property: a header ending in its name before `{` / `=>`.
    let method = toks.iter().enumerate().find_map(|(k, t)| {
        if *t != CsTok::Group(b'(') {
            return None;
        }
        let at = match toks.get(k.checked_sub(1)?)? {
            CsTok::Group(b'<') => k.checked_sub(2)?,
            _ => k - 1,
        };
        word(toks.get(at)?).filter(|w| !CS_KEYWORDS.contains(w)).map(|_| at)
    });
    let (at, is_method) = match method {
        Some(at) => (at, true),
        None if end != CsEnd::Semi => (toks.len().checked_sub(1)?, false),
        None => return None,
    };
    let CsTok::Word(name_start, name_end) = *toks.get(at)? else {
        return None;
    };
    let name = src.get(name_start..name_end).filter(|n| is_ident(n) && !CS_KEYWORDS.contains(n))?;
    let prefix: Vec<&str> = toks.get(..at)?.iter().filter_map(word).collect();
    let return_type = toks.get(..at)?.last().and_then(word);
    let field = prefix.contains(&"public")
        && !prefix.iter().any(|w| CS_NOT_A_FIELD.contains(w))
        && (class.is_static || !prefix.contains(&"static"))
        && name != class.name
        && !(is_method
            && (CS_OBJECT_METHODS.contains(&name) || return_type.is_some_and(|t| CS_NO_VALUE_TYPES.contains(&t))));
    field.then_some((name, name_start, is_method))
}

/// CL.6b: HotChocolate's name for a member: a method drops a `Get` prefix
/// (when an upper-case letter follows) and an `Async` suffix; then the
/// leading upper-case run goes lower case, keeping the run's last letter
/// when a letter follows it (`FormatFieldName`: `GetBooks` -> `books`,
/// `GetUserByIdAsync` -> `userById`, `URLPath` -> `urlPath`, `ID` -> `id`).
fn hc_field_name(member: &str, method: bool) -> Option<String> {
    let mut name = member;
    if method {
        if let Some(rest) = name.strip_prefix("Get").filter(|r| r.starts_with(|c: char| c.is_ascii_uppercase())) {
            name = rest;
        }
        if let Some(rest) = name.strip_suffix("Async").filter(|r| !r.is_empty()) {
            name = rest;
        }
    }
    if !is_ident(name) {
        return None;
    }
    let run = name.chars().take_while(|c| c.is_uppercase()).count();
    let lower = match name.chars().nth(run) {
        Some(next) if run > 1 && next.is_alphabetic() => run - 1,
        _ => run,
    };
    let mut out = String::with_capacity(name.len());
    for (i, c) in name.chars().enumerate() {
        if i < lower {
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    Some(out)
}

/// CL.6b: the fields of one root class, as (name, 0-indexed line of the
/// member's name) in source order: its first-level members HotChocolate
/// exposes ([`cs_member`]) under `[GraphQLName("..")]` when given, else
/// [`hc_field_name`]; a `[GraphQLIgnore]` member is none.
fn cs_root_fields(src: &str, m: &[u8], class: &CsRoot<'_>) -> Vec<(String, u32)> {
    let (open, close) = class.body;
    let mut fields = Vec::new();
    for decl in cs_decls(m, open + 1, close) {
        let (attrs, at) = cs_head_attrs(src, m, decl.head.0, decl.head.1);
        if attrs.iter().any(|a| a.name == "GraphQLIgnore") {
            continue;
        }
        let Some((member, name_at, method)) = cs_member(src, m, (at, decl.head.1), decl.end, class) else {
            continue;
        };
        if let Some(name) = hc_graphql_name(&attrs).or_else(|| hc_field_name(member, method)) {
            fields.push((name, line_of(src, name_at)));
        }
    }
    fields
}

/// Root nouns as (root type, 0-indexed anchor line), one per root type.
type RootNouns = Vec<(&'static str, u32)>;

/// CL.6b: a C# file's root nouns (each anchored at the `class` line of its
/// first root class) and their fields, given its [`cs_code_mask`].
fn hc_roots_and_fields(src: &str, m: &[u8]) -> (RootNouns, Vec<(String, u32)>) {
    let mut roots = RootNouns::new();
    let mut fields = Vec::new();
    for class in cs_root_classes(src, m) {
        if !roots.iter().any(|&(r, _)| r == class.root) {
            roots.push((class.root, line_of(src, class.class_at)));
        }
        fields.extend(cs_root_fields(src, m, &class));
    }
    fields.sort_by_key(|&(_, line)| line);
    (roots, fields)
}

// ---- DG.4: async-graphql (Rust) and Caliban (Scala) root types ----

/// DG.4: the stand-in path a Rust file's [`LazyGuard`] is built with.
/// [`extract_graphql_resolver_nodes`] takes the `detect_language` tag, not a
/// path, and `rust` is the tag of exactly the `.rs` extension, the one
/// `code_guard` keys its Rust lexer on (a bare `.rs` would not do: the
/// extension of a dot-file is none).
const RS_GUARD_PATH: &str = "schema.rs";

/// DG.4: async-graphql's root attributes as they open an attribute, bare or
/// crate-qualified; [`rs_root_attr_sites`] requires a `]`, `(` or blank
/// after one, so `#[ObjectType]` is none.
const RS_ROOT_ATTR_NEEDLES: &[&str] = &[
    "#[Object",
    "#[async_graphql::Object",
    "#[Subscription",
    "#[async_graphql::Subscription",
];

/// DG.4: the derives that merge root parts into one root type
/// (`#[derive(MergedObject)] struct Query(UserQuery, OrderQuery);`).
const RS_MERGED_DERIVES: &[&str] = &["MergedObject", "MergedSubscription"];

/// DG.4: the placeholder roots a schema takes for an absent mutation or
/// subscription type.
const RS_EMPTY_ROOTS: &[&str] = &["EmptyMutation", "EmptySubscription"];

/// DG.4: `#[graphql(..)]` flags that make an `#[Object]` method no field of
/// its type: `skip` hides it, an `entity` method is a federation `_entities`
/// lookup and a `flatten` method merges its result's fields instead.
const RS_NOT_A_FIELD: &[&str] = &["skip", "entity", "flatten"];

/// DG.4: what may precede `fn` in a method header.
const RS_FN_MODIFIERS: &[&str] = &["pub", "async", "const", "unsafe", "extern", "default"];

/// DG.4: Caliban's `RootResolver(queryResolver, mutationResolver,
/// subscriptionResolver)` parameter names, in root order.
const CALIBAN_RESOLVER_PARAMS: &[&str] = &["queryResolver", "mutationResolver", "subscriptionResolver"];

/// DG.4: Scala parameter modifiers a case-class field may carry before its
/// name.
const SCALA_PARAM_MODIFIERS: &[&str] = &["val", "var", "override", "private", "protected", "final", "implicit", "using"];

/// DG.4: `source` with every byte `is_code` refuses (its literals and
/// comments) and every `'` it admits (a Rust lifetime or loop label, a Scala
/// symbol literal, which [`cs_matching_close`] would read as a char quote)
/// overwritten with a blank, newlines kept, so an offset means the same in
/// both and every bracket, keyword and identifier left is code. A literal or
/// comment is blanked whole, so the mask stays valid UTF-8.
fn code_mask(source: &str, mut is_code: impl FnMut(usize) -> bool) -> Vec<u8> {
    source
        .bytes()
        .enumerate()
        .map(|(at, c)| match c {
            b'\n' => b'\n',
            b'\'' => b' ',
            _ if !is_code(at) => b' ',
            _ => c,
        })
        .collect()
}

/// DG.4: the offsets of every whole-word occurrence of `word` in a mask
/// (the gated files' structural scans; the per-file gates search the source
/// with `match_indices`).
fn word_sites(b: &[u8], word: &[u8]) -> Vec<usize> {
    if word.is_empty() {
        return Vec::new();
    }
    b.windows(word.len())
        .enumerate()
        .filter(|&(at, w)| {
            w == word && !ident_byte_before(b, at) && !b.get(at + word.len()).is_some_and(|&c| is_ident_byte(c))
        })
        .map(|(at, _)| at)
        .collect()
}

/// DG.4: the items of the bracket group opening at `open` and closing at
/// `close` of a mask, split at its commas outside nested `()[]{}` (and
/// `<>` when `angle`, where the `>` of `->` / `=>` is no bracket), as
/// trimmed `(start, end)` spans; an empty item is none.
fn mask_items(m: &[u8], open: usize, close: usize, angle: bool) -> Vec<(usize, usize)> {
    let mut items = Vec::new();
    let mut push = |from: usize, to: usize| {
        let start = skip_ascii_ws(m, from).min(to);
        let end = start + m.get(start..to).map_or(0, |r| r.iter().rposition(|c| !c.is_ascii_whitespace()).map_or(0, |p| p + 1));
        if end > start {
            items.push((start, end));
        }
    };
    let (mut depth, mut start) = (0i32, open + 1);
    for at in open + 1..close {
        match m.get(at) {
            Some(b'(' | b'[' | b'{') => depth += 1,
            Some(b')' | b']' | b'}') => depth -= 1,
            Some(b'<') if angle => depth += 1,
            Some(b'>') if angle && !matches!(at.checked_sub(1).and_then(|p| m.get(p)), Some(b'-' | b'=')) => depth -= 1,
            Some(b',') if depth == 0 => {
                push(start, at);
                start = at + 1;
            }
            _ => {}
        }
    }
    push(start, close);
    items
}

/// DG.4: the `>` closing the `<` at `open` of a mask, past nested `<..>`
/// and the `>` of `->` / `=>`; `None` at a `;`, `{` or `}` first (a
/// comparison, not a type argument list).
fn angle_close(m: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (at, &c) in m.iter().enumerate().skip(open) {
        match c {
            b'<' => depth += 1,
            b'>' if !matches!(at.checked_sub(1).and_then(|p| m.get(p)), Some(b'-' | b'=')) => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(at);
                }
            }
            b';' | b'{' | b'}' => return None,
            _ => {}
        }
    }
    None
}

/// DG.4: the identifiers of the path starting at `from` of a mask (past
/// blanks and, in Rust, a leading `::`), joined by `sep` (`::` in Rust, `.`
/// in Scala), no further than `to`: `crate::schema::QueryRoot::default` ->
/// `[crate, schema, QueryRoot, default]`.
fn path_segments<'a>(src: &'a str, m: &[u8], from: usize, to: usize, sep: &[u8]) -> Vec<&'a str> {
    let mut segs = Vec::new();
    let mut at = skip_ascii_ws(m, from);
    if m.get(at..).is_some_and(|r| r.starts_with(sep)) && sep == b"::" {
        at += sep.len();
    }
    while at < to {
        let end = cs_ident_end(m, at, to);
        let Some(seg) = src.get(at..end).filter(|s| !s.is_empty()) else {
            break;
        };
        segs.push(seg);
        if !m.get(end..to).is_some_and(|r| r.starts_with(sep)) {
            break;
        }
        at = end + sep.len();
    }
    segs
}

/// DG.4: the type a path names: its last segment that starts upper-case
/// (`QueryRoot` in `QueryRoot::default`, `Queries` in `api.Queries`).
fn type_segment<'a>(segs: &[&'a str]) -> Option<&'a str> {
    segs.iter().rev().copied().find(|s| s.starts_with(|c: char| c.is_ascii_uppercase()))
}

/// DG.4: the byte offsets of every async-graphql root attribute opener
/// ([`RS_ROOT_ATTR_NEEDLES`]).
fn rs_root_attr_sites(source: &str) -> Vec<usize> {
    let b = source.as_bytes();
    let mut sites: Vec<usize> = RS_ROOT_ATTR_NEEDLES
        .iter()
        .flat_map(|needle| {
            source
                .match_indices(needle)
                .map(|(at, n)| (at, at + n.len()))
                .collect::<Vec<_>>()
        })
        .filter(|&(_, end)| b.get(end).is_some_and(|&c| c == b']' || c == b'(' || c.is_ascii_whitespace()))
        .map(|(at, _)| at)
        .collect();
    sites.sort_unstable();
    sites
}

/// DG.4: a Rust file that uses async-graphql: the crate name
/// `async_graphql` as a whole word at code position (CJ.1's [`LazyGuard`],
/// so a sample in a string or a doc comment is none) that opens a path
/// (`async_graphql::Object`, `::async_graphql::Schema`) or is what a `use`
/// declaration (`use async_graphql;`, `pub use async_graphql as gql;`) or
/// `extern crate` names. `async_graphql_axum` is another crate; a file that
/// never spells the name is never lexed.
fn rs_imports_async_graphql(source: &str) -> bool {
    let b = source.as_bytes();
    let mut guard = LazyGuard::new(RS_GUARD_PATH, source);
    let name = "async_graphql";
    source.match_indices(name).any(|(at, _)| {
        if ident_byte_before(b, at) || b.get(at + name.len()).is_some_and(|&c| is_ident_byte(c)) {
            return false;
        }
        let path = b.get(skip_ascii_ws(b, at + name.len())..).is_some_and(|r| r.starts_with(b"::"));
        let line_start = source.get(..at).and_then(|s| s.rfind('\n')).map_or(0, |p| p + 1);
        let words: Vec<&str> = source.get(line_start..at).unwrap_or_default().split_whitespace().collect();
        let used = (words.last() == Some(&"use") && words.iter().all(|w| *w == "use" || w.starts_with("pub")))
            || words == ["extern", "crate"];
        (path || used) && guard.admits(at)
    })
}

/// DG.4: a Scala file that imports Caliban: a line starting with `import
/// caliban` past which no identifier byte follows (`import caliban.*`,
/// `import caliban.{graphQL, RootResolver}`); `import calibanx` is another
/// package.
fn scala_imports_caliban(source: &str) -> bool {
    source.lines().any(|line| {
        line.trim_start()
            .strip_prefix("import")
            .filter(|rest| rest.starts_with(char::is_whitespace))
            .and_then(|rest| rest.trim_start().strip_prefix("caliban"))
            .is_some_and(|after| !after.bytes().next().is_some_and(is_ident_byte))
    })
}

/// DG.4: async-graphql's field rename rules, the `rename_fields` values of
/// `#[Object(..)]` / `#[Subscription(..)]`; the default is `camelCase`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RenameRule {
    Lower,
    Upper,
    Pascal,
    Camel,
    Snake,
    ScreamingSnake,
}

impl RenameRule {
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "lowercase" => RenameRule::Lower,
            "UPPERCASE" => RenameRule::Upper,
            "PascalCase" => RenameRule::Pascal,
            "camelCase" => RenameRule::Camel,
            "snake_case" => RenameRule::Snake,
            "SCREAMING_SNAKE_CASE" => RenameRule::ScreamingSnake,
            _ => return None,
        })
    }

    /// The GraphQL name async-graphql derives from a Rust method name
    /// (async-graphql-derive's `RenameRule::rename`): `lowercase` and
    /// `UPPERCASE` change case only, the others are heck's conversions over
    /// [`heck_words`]. So `order_count` -> `orderCount`, `user_ID` ->
    /// `userId`, `field_2` -> `field2`, and a leading `_` is dropped
    /// (`_internal` -> `internal`).
    fn apply(self, name: &str) -> String {
        let words = heck_words(name);
        let joined = |sep: &str, word: fn(&str) -> String| words.iter().map(|w| word(w)).collect::<Vec<_>>().join(sep);
        match self {
            RenameRule::Lower => name.to_lowercase(),
            RenameRule::Upper => name.to_uppercase(),
            RenameRule::Pascal => joined("", capitalize),
            RenameRule::Camel => words
                .iter()
                .enumerate()
                .map(|(i, w)| if i == 0 { w.to_lowercase() } else { capitalize(w) })
                .collect(),
            RenameRule::Snake => joined("_", str::to_lowercase),
            RenameRule::ScreamingSnake => joined("_", str::to_uppercase),
        }
    }
}

/// DG.4: heck's word split (`transform`): every non-alphanumeric char is a
/// separator (so leading and doubled `_` make no word), and within a run a
/// word ends before an upper-case char that follows a lower-case one
/// (`getUser`) and before the last capital of a capital run a lower-case
/// char follows (`HTTPResponse` -> `HTTP`, `Response`); digits carry the
/// case mode through.
fn heck_words(s: &str) -> Vec<&str> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Mode {
        Boundary,
        Lower,
        Upper,
    }
    let mut words = Vec::new();
    for run in s.split(|c: char| !c.is_alphanumeric()) {
        let mut chars = run.char_indices().peekable();
        let (mut init, mut mode) = (0, Mode::Boundary);
        while let Some((i, c)) = chars.next() {
            let Some(&(next_i, next)) = chars.peek() else {
                words.extend(run.get(init..));
                break;
            };
            let next_mode = if c.is_lowercase() {
                Mode::Lower
            } else if c.is_uppercase() {
                Mode::Upper
            } else {
                mode
            };
            if next_mode == Mode::Lower && next.is_uppercase() {
                words.extend(run.get(init..next_i));
                (init, mode) = (next_i, Mode::Boundary);
            } else if mode == Mode::Upper && c.is_uppercase() && next.is_lowercase() {
                words.extend(run.get(init..i));
                (init, mode) = (i, Mode::Boundary);
            } else {
                mode = next_mode;
            }
        }
    }
    words
}

/// DG.4: heck's `capitalize`: the first char upper-case, the rest lower.
fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars.flat_map(char::to_lowercase)).collect(),
        None => String::new(),
    }
}

/// DG.4: one Rust outer attribute: its path past a leading `::`
/// (`Object`, `async_graphql::Object`, `derive`, `graphql`) and its argument
/// text from the source, string literals intact (`(name = "x")`, or empty).
struct RsAttr<'a> {
    path: &'a str,
    args: &'a str,
}

impl<'a> RsAttr<'a> {
    /// The items of the argument list, split at its top-level commas
    /// ([`cs_args`]); none for a bare attribute.
    fn items(&self) -> Vec<&'a str> {
        let args: &'a str = self.args;
        args.strip_prefix('(')
            .and_then(|a| a.strip_suffix(')'))
            .map(|inner| cs_args(inner).into_iter().filter(|item| !item.is_empty()).collect())
            .unwrap_or_default()
    }

    /// The quoted value of a `key = "value"` item.
    fn value(&self, key: &str) -> Option<&'a str> {
        self.items().into_iter().find_map(|item| {
            let rest = item.strip_prefix(key)?.trim_start().strip_prefix('=')?;
            quoted_literal(rest.trim())
        })
    }

    /// True when the list holds the flag `name`, bare or `= true`.
    fn flag(&self, name: &str) -> bool {
        self.items().into_iter().any(|item| {
            item == name
                || item
                    .strip_prefix(name)
                    .and_then(|rest| rest.trim_start().strip_prefix('='))
                    .is_some_and(|value| value.trim() == "true")
        })
    }
}

/// DG.4: the outer attributes a Rust item header opens with, and the
/// offset past them; an inner attribute (`#![..]`) is skipped, not kept.
fn rs_head_attrs<'a>(src: &'a str, m: &[u8], from: usize, to: usize) -> (Vec<RsAttr<'a>>, usize) {
    let mut attrs = Vec::new();
    let mut at = skip_ascii_ws(m, from);
    while at < to && m.get(at) == Some(&b'#') {
        let mut open = skip_ascii_ws(m, at + 1);
        let inner = m.get(open) == Some(&b'!');
        if inner {
            open = skip_ascii_ws(m, open + 1);
        }
        if m.get(open) != Some(&b'[') {
            break;
        }
        let Some(close) = cs_matching_close(m, open).filter(|&c| c < to) else {
            break;
        };
        if !inner {
            let start = skip_ascii_ws(m, open + 1);
            let path_end = start + m.get(start..close).map_or(0, |r| r.iter().take_while(|&&c| is_ident_byte(c) || c == b':').count());
            if let (Some(path), Some(args)) = (src.get(start..path_end), src.get(path_end..close)) {
                let path = path.strip_prefix("::").unwrap_or(path);
                attrs.push(RsAttr { path, args: args.trim() });
            }
        }
        at = skip_ascii_ws(m, close + 1);
    }
    (attrs, at)
}

/// DG.4: one `#[Object]` / `#[Subscription]` impl block.
struct RsObjectImpl<'a> {
    /// The self type's name.
    ty: &'a str,
    subscription: bool,
    rule: RenameRule,
    /// The offset of the `impl` keyword, where a root noun anchors.
    impl_at: usize,
    /// Its `{` and `}`.
    body: (usize, usize),
}

/// DG.4: whether `attrs` hold async-graphql's `#[Object]` (false) or
/// `#[Subscription]` (true), and that attribute's field rename rule.
fn rs_root_attr(attrs: &[RsAttr<'_>]) -> Option<(bool, RenameRule)> {
    attrs.iter().find_map(|a| {
        let subscription = match a.path {
            "Object" | "async_graphql::Object" => false,
            "Subscription" | "async_graphql::Subscription" => true,
            _ => return None,
        };
        let rule = a.value("rename_fields").and_then(RenameRule::parse).unwrap_or(RenameRule::Camel);
        Some((subscription, rule))
    })
}

/// DG.4: the self type of an inherent impl header from `from` (past its
/// attributes): `impl QueryRoot`, `impl<'a> QueryRoot<'a>`,
/// `impl crate::api::QueryRoot`, with the offset of its `impl` keyword. A
/// trait impl (`impl X for Y`) is none.
fn rs_impl_self_type<'a>(src: &'a str, m: &[u8], from: usize, to: usize) -> Option<(&'a str, usize)> {
    let toks = cs_head_tokens(m, from, to);
    let word = |t: Option<&CsTok>| match t {
        Some(&CsTok::Word(s, e)) => src.get(s..e),
        _ => None,
    };
    let k = toks.iter().position(|t| word(Some(t)) == Some("impl"))?;
    if toks.iter().take(k).any(|t| !matches!(word(Some(t)), Some("unsafe" | "default")))
        || toks.iter().any(|t| word(Some(t)) == Some("for"))
    {
        return None;
    }
    let CsTok::Word(impl_at, _) = *toks.get(k)? else {
        return None;
    };
    let mut i = k + 1;
    if toks.get(i) == Some(&CsTok::Group(b'<')) {
        i += 1;
    }
    let mut ty = word(toks.get(i))?;
    while toks.get(i + 1) == Some(&CsTok::Punct(b':')) && toks.get(i + 2) == Some(&CsTok::Punct(b':')) {
        let Some(next) = word(toks.get(i + 3)) else {
            break;
        };
        ty = next;
        i += 3;
    }
    is_ident(ty).then_some((ty, impl_at))
}

/// DG.4: a `#[derive(MergedObject)]` / `#[derive(MergedSubscription)]`
/// tuple struct header from `from` (past its attributes): its name and the
/// type of each member, in order (`struct Query(UserQuery, orders::OrderQuery)`
/// -> `Query`, `[UserQuery, OrderQuery]`).
fn rs_merged_struct<'a>(
    src: &'a str,
    m: &[u8],
    attrs: &[RsAttr<'_>],
    from: usize,
    to: usize,
) -> Option<(&'a str, Vec<&'a str>)> {
    let merged = attrs.iter().any(|a| {
        a.path == "derive"
            && a.items().iter().any(|item| item.rsplit("::").next().is_some_and(|d| RS_MERGED_DERIVES.contains(&d.trim())))
    });
    if !merged {
        return None;
    }
    let toks = cs_head_tokens(m, from, to);
    let k = toks.iter().position(|t| matches!(*t, CsTok::Word(s, e) if src.get(s..e) == Some("struct")))?;
    let CsTok::Word(name_start, name_end) = *toks.get(k + 1)? else {
        return None;
    };
    let name = src.get(name_start..name_end).filter(|n| is_ident(n))?;
    let mut open = skip_ascii_ws(m, name_end);
    if m.get(open) == Some(&b'<') {
        open = skip_ascii_ws(m, angle_close(m, open)? + 1);
    }
    if m.get(open) != Some(&b'(') {
        return None;
    }
    let close = cs_matching_close(m, open).filter(|&c| c <= to)?;
    let members = mask_items(m, open, close, true)
        .into_iter()
        .filter_map(|(s, e)| {
            let mut at = s;
            if src.get(s..cs_ident_end(m, s, e)) == Some("pub") {
                at = skip_ascii_ws(m, s + 3);
                if m.get(at) == Some(&b'(') {
                    at = cs_matching_close(m, at)? + 1;
                }
            }
            type_segment(&path_segments(src, m, at, e, b"::"))
        })
        .collect();
    Some((name, members))
}

/// DG.4: merged root structs, as (name, member types).
type MergedRoots<'a> = Vec<(&'a str, Vec<&'a str>)>;

/// DG.4: the impl blocks and merged root structs of a Rust file's mask, in
/// source order, through `mod` blocks and every other body (a test may
/// declare its schema inside a function).
fn rs_items<'a>(src: &'a str, m: &[u8]) -> (Vec<RsObjectImpl<'a>>, MergedRoots<'a>) {
    struct Found<'a> {
        impls: Vec<RsObjectImpl<'a>>,
        merged: MergedRoots<'a>,
    }
    fn walk<'a>(src: &'a str, m: &[u8], span: (usize, usize), nesting: usize, found: &mut Found<'a>) {
        if nesting > 64 {
            return;
        }
        for decl in cs_decls(m, span.0, span.1) {
            let (attrs, at) = rs_head_attrs(src, m, decl.head.0, decl.head.1);
            if let Some(merged) = rs_merged_struct(src, m, &attrs, at, decl.head.1) {
                found.merged.push(merged);
            }
            let CsEnd::Body(open, close) = decl.end else {
                continue;
            };
            if let Some((subscription, rule)) = rs_root_attr(&attrs)
                && let Some((ty, impl_at)) = rs_impl_self_type(src, m, at, decl.head.1)
            {
                found.impls.push(RsObjectImpl { ty, subscription, rule, impl_at, body: (open, close) });
            }
            walk(src, m, (open + 1, close), nesting + 1, found);
        }
    }
    let mut found = Found { impls: Vec::new(), merged: Vec::new() };
    walk(src, m, (0, m.len()), 0, &mut found);
    (found.impls, found.merged)
}

/// DG.4: the root each type a Rust file's schema construction names takes,
/// in source order: the three arguments of `Schema::new(Q, M, S)` /
/// `Schema::build(Q, M, S)` (values: `QueryRoot`, `QueryRoot::default()`,
/// `Query(A, B)`) or of the type `Schema<Q, M, S>` (a `type AppSchema =
/// Schema<..>` alias, a parameter type), each argument's type its leading
/// path's [`type_segment`]; [`RS_EMPTY_ROOTS`] name nothing. `None` when the
/// file has no such site, so its roots go by name.
fn rs_schema_roots<'a>(src: &'a str, m: &[u8]) -> Option<Vec<(&'a str, &'static str)>> {
    let mut named = Vec::new();
    let mut any = false;
    for at in word_sites(m, b"Schema") {
        let after = skip_ascii_ws(m, at + "Schema".len());
        let (open, close, angle) = if m.get(after..).is_some_and(|r| r.starts_with(b"::")) {
            let ctor = skip_ascii_ws(m, after + 2);
            let ctor_end = cs_ident_end(m, ctor, m.len());
            let open = skip_ascii_ws(m, ctor_end);
            if !matches!(m.get(ctor..ctor_end), Some(b"new" | b"build")) || m.get(open) != Some(&b'(') {
                continue;
            }
            let Some(close) = cs_matching_close(m, open) else {
                continue;
            };
            (open, close, false)
        } else if m.get(after) == Some(&b'<') {
            let Some(close) = angle_close(m, after) else {
                continue;
            };
            (after, close, true)
        } else {
            continue;
        };
        let items = mask_items(m, open, close, angle);
        if items.len() != ROOT_TYPES.len() {
            continue;
        }
        any = true;
        for (&(s, e), &root) in items.iter().zip(ROOT_TYPES) {
            if let Some(ty) = type_segment(&path_segments(src, m, s, e, b"::"))
                && !RS_EMPTY_ROOTS.contains(&ty)
            {
                named.push((ty, root));
            }
        }
    }
    any.then_some(named)
}

/// DG.4: the root a type name makes, for a file that names no schema:
/// `Query`, `QueryRoot` or any name ending in `Query` is the query root,
/// likewise `Mutation` and `Subscription`.
fn rs_root_by_name(name: &str) -> Option<&'static str> {
    ROOT_TYPES
        .iter()
        .copied()
        .find(|root| name.ends_with(root) || name.strip_prefix(root) == Some("Root"))
}

/// DG.4: the name and `fn` keyword offset of a method header from `from`
/// (past its attributes): only [`RS_FN_MODIFIERS`] (and a `pub(..)` group)
/// before `fn`; a raw identifier (`r#type`) reads as `type`.
fn rs_fn_name<'a>(src: &'a str, m: &[u8], from: usize, to: usize) -> Option<(&'a str, usize)> {
    let toks = cs_head_tokens(m, from, to);
    let word = |t: Option<&CsTok>| match t {
        Some(&CsTok::Word(s, e)) => src.get(s..e),
        _ => None,
    };
    let k = toks.iter().position(|t| word(Some(t)) == Some("fn"))?;
    let modifiers_only = toks.iter().take(k).all(|t| {
        *t == CsTok::Group(b'(') || word(Some(t)).is_some_and(|w| RS_FN_MODIFIERS.contains(&w))
    });
    let CsTok::Word(fn_at, _) = *toks.get(k)? else {
        return None;
    };
    let mut name = word(toks.get(k + 1))?;
    if name == "r" && toks.get(k + 2) == Some(&CsTok::Punct(b'#')) {
        name = word(toks.get(k + 3))?;
    }
    (modifiers_only && is_ident(name)).then_some((name, fn_at))
}

/// DG.4: the fields of one root impl, as (GraphQL name, 0-indexed line of
/// its `fn`) in source order: every method of its body (async-graphql makes
/// each one a field, whatever its visibility) but one whose `#[graphql(..)]`
/// holds a [`RS_NOT_A_FIELD`] flag, named by `#[graphql(name = "..")]` when
/// given, else by the impl's rename rule ([`RenameRule::apply`]).
fn rs_object_fields(src: &str, m: &[u8], imp: &RsObjectImpl<'_>) -> Vec<(String, u32)> {
    let (open, close) = imp.body;
    let mut fields = Vec::new();
    for decl in cs_decls(m, open + 1, close) {
        let (attrs, at) = rs_head_attrs(src, m, decl.head.0, decl.head.1);
        let graphql: Vec<&RsAttr<'_>> = attrs.iter().filter(|a| a.path == "graphql").collect();
        if graphql.iter().any(|a| RS_NOT_A_FIELD.iter().any(|flag| a.flag(flag))) {
            continue;
        }
        let Some((name, fn_at)) = rs_fn_name(src, m, at, decl.head.1) else {
            continue;
        };
        let field = graphql
            .iter()
            .find_map(|a| a.value("name"))
            .map_or_else(|| imp.rule.apply(name), str::to_string);
        if !field.is_empty() {
            fields.push((field, line_of(src, fn_at)));
        }
    }
    fields
}

/// DG.4: a Rust file's async-graphql root nouns (each anchored at the
/// `impl` line of its first root impl) and their fields ([`rs_object_fields`]),
/// given its [`code_mask`]. A root impl is an `#[Object]` / `#[Subscription]`
/// impl ([`rs_items`]) whose self type the file's schema construction names
/// ([`rs_schema_roots`]) or, when the file names no schema, whose name makes
/// a root ([`rs_root_by_name`]); a member of a merged root struct takes its
/// root, and a `#[Subscription]` impl is always the subscription root. An
/// object type (`#[Object] impl Order`, `#[derive(SimpleObject)]`) is none.
fn async_graphql_roots_and_fields(src: &str, m: &[u8]) -> (RootNouns, Vec<(String, u32)>) {
    let (impls, merged) = rs_items(src, m);
    let mut root_of: Vec<(&str, &'static str)> = match rs_schema_roots(src, m) {
        Some(named) => named,
        None => impls
            .iter()
            .map(|imp| imp.ty)
            .chain(merged.iter().map(|&(name, _)| name))
            .filter_map(|name| rs_root_by_name(name).map(|root| (name, root)))
            .collect(),
    };
    // Merged parts take their root, through nested merges.
    for _ in 0..=merged.len() {
        let mut grew = false;
        for (name, members) in &merged {
            let Some(root) = root_of.iter().find(|(n, _)| n == name).map(|&(_, r)| r) else {
                continue;
            };
            for member in members {
                if !root_of.iter().any(|(n, _)| n == member) {
                    root_of.push((member, root));
                    grew = true;
                }
            }
        }
        if !grew {
            break;
        }
    }
    let mut roots = RootNouns::new();
    let mut fields = Vec::new();
    for imp in &impls {
        let root = if imp.subscription {
            Some("Subscription")
        } else {
            root_of.iter().find(|(n, _)| *n == imp.ty).map(|&(_, r)| r)
        };
        let Some(root) = root else {
            continue;
        };
        if !roots.iter().any(|&(r, _)| r == root) {
            roots.push((root, line_of(src, imp.impl_at)));
        }
        fields.extend(rs_object_fields(src, m, imp));
    }
    fields.sort_by_key(|&(_, line)| line);
    (roots, fields)
}

/// DG.4: the byte offsets of every `RootResolver(` / `RootResolver[..](`
/// call, the name a whole word; an import of the name is no site.
fn scala_root_resolver_sites(source: &str) -> Vec<usize> {
    let b = source.as_bytes();
    let name = "RootResolver";
    source
        .match_indices(name)
        .map(|(at, _)| at)
        .filter(|&at| {
            !ident_byte_before(b, at) && matches!(b.get(skip_ascii_ws(b, at + name.len())), Some(b'(' | b'['))
        })
        .collect()
}

/// DG.4: one Scala case class: its name, the offset of its `case` keyword
/// and its first parameter list's brackets, when it has one.
struct ScalaCaseClass<'a> {
    name: &'a str,
    case_at: usize,
    params: Option<(usize, usize)>,
}

/// DG.4: every case class a Scala file's mask declares.
fn scala_case_classes<'a>(src: &'a str, m: &[u8]) -> Vec<ScalaCaseClass<'a>> {
    word_sites(m, b"case")
        .into_iter()
        .filter_map(|case_at| {
            let class = skip_ascii_ws(m, case_at + "case".len());
            let class_end = cs_ident_end(m, class, m.len());
            if m.get(class..class_end) != Some(b"class") {
                return None;
            }
            let at = skip_ascii_ws(m, class_end);
            let name_end = cs_ident_end(m, at, m.len());
            let name = src.get(at..name_end).filter(|n| is_ident(n))?;
            let mut open = skip_ascii_ws(m, name_end);
            if m.get(open) == Some(&b'[') {
                open = skip_ascii_ws(m, cs_matching_close(m, open)? + 1);
            }
            let params = (m.get(open) == Some(&b'('))
                .then(|| cs_matching_close(m, open).map(|close| (open, close)))
                .flatten();
            Some(ScalaCaseClass { name, case_at, params })
        })
        .collect()
}

/// DG.4: the type a Scala `val` / `var` named `v` holds: its annotated type
/// (`val q: Queries`) or its initialiser's constructor (`val q = Queries(..)`,
/// `val q = new Queries(..)`), each the path's [`type_segment`].
fn scala_val_type<'a>(src: &'a str, m: &[u8], v: &str) -> Option<&'a str> {
    let sites = word_sites(m, b"val").into_iter().chain(word_sites(m, b"var"));
    sites.into_iter().find_map(|at| {
        let name = skip_ascii_ws(m, at + 3);
        let name_end = cs_ident_end(m, name, m.len());
        if src.get(name..name_end) != Some(v) {
            return None;
        }
        let p = skip_ascii_ws(m, name_end);
        let mut rhs = match (m.get(p), m.get(p + 1)) {
            (Some(b':'), _) => skip_ascii_ws(m, p + 1),
            (Some(b'='), next) if !matches!(next, Some(b'=' | b'>')) => skip_ascii_ws(m, p + 1),
            _ => return None,
        };
        if m.get(rhs..cs_ident_end(m, rhs, m.len())) == Some(b"new") {
            rhs = skip_ascii_ws(m, rhs + 3);
        }
        type_segment(&path_segments(src, m, rhs, m.len(), b"."))
    })
}

/// DG.4: one `RootResolver` argument (`s..e`, the `k`-th): the root slot it
/// fills (its position, or the `queryResolver = ..` parameter it names) and
/// its value span, past `Some(..)` / `Option(..)`; `None` for an absent
/// root (`None`, `Unit`, `()`, `Option.empty[..]`).
fn caliban_arg(src: &str, m: &[u8], k: usize, s: usize, e: usize) -> Option<(usize, (usize, usize))> {
    let name_end = cs_ident_end(m, s, e);
    let eq = skip_ascii_ws(m, name_end);
    let (slot, mut start) = if name_end > s && m.get(eq) == Some(&b'=') && !matches!(m.get(eq + 1), Some(b'=' | b'>')) {
        let param = src.get(s..name_end)?;
        (CALIBAN_RESOLVER_PARAMS.iter().position(|p| *p == param)?, skip_ascii_ws(m, eq + 1))
    } else {
        (k, s)
    };
    let mut end = e;
    loop {
        let word_end = cs_ident_end(m, start, end);
        let next = skip_ascii_ws(m, word_end);
        match src.get(start..word_end)? {
            "Some" | "Option" if m.get(next) == Some(&b'(') => {
                let close = cs_matching_close(m, next).filter(|&c| c < end)?;
                start = skip_ascii_ws(m, next + 1);
                end = close;
            }
            "Option" if m.get(next) == Some(&b'.') => return None,
            "None" | "Unit" => return None,
            "" if m.get(start) == Some(&b'(') && m.get(skip_ascii_ws(m, start + 1)) == Some(&b')') => return None,
            _ => break,
        }
    }
    (start < end).then_some((slot, (start, end)))
}

/// DG.4: the case class a `RootResolver` argument value constructs: `T(..)`,
/// `new T(..)`, `T`, or a lower-case identifier bound in the file by `val v
/// = T(..)` / `val v: T` ([`scala_val_type`]); the path's last segment when it
/// is that identifier, else its [`type_segment`].
fn caliban_arg_type<'a>(src: &'a str, m: &[u8], (start, end): (usize, usize)) -> Option<&'a str> {
    let mut at = start;
    if m.get(at..cs_ident_end(m, at, end)) == Some(b"new") {
        at = skip_ascii_ws(m, at + 3);
    }
    let segs = path_segments(src, m, at, end, b".");
    let last = segs.last().copied()?;
    if last.starts_with(|c: char| c.is_ascii_lowercase() || c == '_')
        && let Some(ty) = scala_val_type(src, m, last)
    {
        return Some(ty);
    }
    type_segment(&segs)
}

/// DG.4: one case-class parameter (`s..e` of the mask) as a Caliban field:
/// its name (verbatim, or a `@GQLName("..")` annotation's value, read from
/// the source) and the 0-indexed line of the name; a `@GQLExcluded`
/// parameter is none.
fn caliban_param(src: &str, m: &[u8], s: usize, e: usize) -> Option<(String, u32)> {
    let mut at = skip_ascii_ws(m, s);
    let mut renamed = None;
    while at < e && m.get(at) == Some(&b'@') {
        let name_end = at + 1 + m.get(at + 1..e).map_or(0, |r| r.iter().take_while(|&&c| is_ident_byte(c) || c == b'.').count());
        let annotation = src.get(at + 1..name_end)?.rsplit('.').next()?;
        let mut next = skip_ascii_ws(m, name_end);
        let mut args = None;
        if m.get(next) == Some(&b'(') {
            let close = cs_matching_close(m, next).filter(|&c| c < e)?;
            args = src.get(next + 1..close);
            next = close + 1;
        }
        match annotation {
            "GQLExcluded" => return None,
            "GQLName" => renamed = args.and_then(|a| quoted_literal(a.trim())).map(str::to_string),
            _ => {}
        }
        at = skip_ascii_ws(m, next);
    }
    loop {
        let word_end = cs_ident_end(m, at, e);
        match src.get(at..word_end) {
            Some(w) if SCALA_PARAM_MODIFIERS.contains(&w) => at = skip_ascii_ws(m, word_end),
            _ => break,
        }
    }
    let (name, name_end) = if m.get(at) == Some(&b'`') {
        let close = at + 1 + m.get(at + 1..e)?.iter().position(|&c| c == b'`')?;
        (src.get(at + 1..close)?, close + 1)
    } else {
        let end = cs_ident_end(m, at, e);
        (src.get(at..end).filter(|n| is_ident(n))?, end)
    };
    let colon = skip_ascii_ws(m, name_end);
    (m.get(colon) == Some(&b':') && !name.is_empty())
        .then(|| (renamed.unwrap_or_else(|| name.to_string()), line_of(src, at)))
}

/// DG.4: the fields of one root case class, in parameter order
/// ([`caliban_param`]).
fn caliban_case_class_fields(src: &str, m: &[u8], class: &ScalaCaseClass<'_>) -> Vec<(String, u32)> {
    let Some((open, close)) = class.params else {
        return Vec::new();
    };
    mask_items(m, open, close, false)
        .into_iter()
        .filter_map(|(s, e)| caliban_param(src, m, s, e))
        .collect()
}

/// DG.4: a Scala file's Caliban root nouns and their fields, given its
/// [`code_mask`] and its [`scala_root_resolver_sites`]: each argument of a
/// `RootResolver(..)` call at code position names its root (position 0 / 1 /
/// 2, or `queryResolver = ..` / `mutationResolver = ..` /
/// `subscriptionResolver = ..`; [`caliban_arg`]), and when the case class it
/// constructs ([`caliban_arg_type`]) is declared in the file, that class's
/// parameters are the root's fields, under their Scala names. A root whose
/// type is declared in another file names its root only: the noun anchors at
/// the case class line when the class is in the file, else at the argument.
fn caliban_roots_and_fields(src: &str, m: &[u8], sites: &[usize]) -> (RootNouns, Vec<(String, u32)>) {
    let classes = scala_case_classes(src, m);
    let mut roots = RootNouns::new();
    let mut fields = Vec::new();
    let mut read: Vec<&str> = Vec::new();
    for &at in sites.iter().filter(|&&at| m.get(at) != Some(&b' ')) {
        let mut open = skip_ascii_ws(m, at + "RootResolver".len());
        if m.get(open) == Some(&b'[') {
            let Some(close) = cs_matching_close(m, open) else {
                continue;
            };
            open = skip_ascii_ws(m, close + 1);
        }
        if m.get(open) != Some(&b'(') {
            continue;
        }
        let Some(close) = cs_matching_close(m, open) else {
            continue;
        };
        for (k, (s, e)) in mask_items(m, open, close, false).into_iter().enumerate() {
            let Some((slot, value)) = caliban_arg(src, m, k, s, e) else {
                continue;
            };
            let Some(&root) = ROOT_TYPES.get(slot) else {
                continue;
            };
            let class = caliban_arg_type(src, m, value).and_then(|ty| classes.iter().find(|c| c.name == ty));
            if !roots.iter().any(|&(r, _)| r == root) {
                let anchor = class.map_or(value.0, |c| c.case_at);
                roots.push((root, line_of(src, anchor)));
            }
            if let Some(class) = class
                && !read.contains(&class.name)
            {
                read.push(class.name);
                fields.extend(caliban_case_class_fields(src, m, class));
            }
        }
    }
    fields.sort_by_key(|&(_, line)| line);
    (roots, fields)
}

// ---- DH.8: graphql-ruby and Absinthe (Elixir) code-first root types ----

/// DH.8: [`code_mask`] of a Ruby file, through [`CodeMap::script`]'s Ruby
/// lexer (`#` comments, `=begin` blocks, `'` / `"` strings, heredoc bodies).
fn ruby_mask(source: &str) -> Vec<u8> {
    let map = CodeMap::script(source, Script::Ruby);
    code_mask(source, |at| map.is_code(at))
}

/// DH.8: `(start, end)` of every line of `b` (a source or its mask), `end`
/// at the line's `\n` or the end of input.
fn line_spans(b: &[u8]) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = 0;
    for (at, &c) in b.iter().enumerate() {
        if c == b'\n' {
            spans.push((start, at));
            start = at + 1;
        }
    }
    if start < b.len() {
        spans.push((start, b.len()));
    }
    spans
}

/// DH.8: the offset of the first byte of `start..end` that is no blank or
/// tab (`end` when there is none).
fn first_non_blank(b: &[u8], start: usize, end: usize) -> usize {
    start + b.get(start..end).map_or(0, |r| r.iter().take_while(|&&c| c == b' ' || c == b'\t').count())
}

/// DH.8: the `::`-separated constant path starting at `from` of a Ruby mask
/// (past blanks and a leading `::`), no further than `to`, and the offset
/// past it: `Types::QueryType` -> `[Types, QueryType]`.
fn ruby_path<'a>(src: &'a str, m: &[u8], from: usize, to: usize) -> (Vec<&'a str>, usize) {
    let mut segs = Vec::new();
    let mut at = first_non_blank(m, from, to);
    if m.get(at..to).is_some_and(|r| r.starts_with(b"::")) {
        at += 2;
    }
    loop {
        let end = cs_ident_end(m, at, to);
        let Some(seg) = src.get(at..end).filter(|s| !s.is_empty()) else {
            break;
        };
        segs.push(seg);
        at = end;
        if !m.get(end..to).is_some_and(|r| r.starts_with(b"::")) {
            break;
        }
        at = end + 2;
    }
    (segs, at)
}

/// DH.8: the root type a graphql-ruby class name declares: exactly `Query` /
/// `Mutation` / `Subscription`, or a name ending in `QueryType` /
/// `MutationType` / `SubscriptionType` (the install generator's
/// `Types::QueryType`).
fn ruby_root(name: &str) -> Option<&'static str> {
    ROOT_TYPES.iter().copied().find(|&root| {
        name == root || name.strip_suffix("Type").is_some_and(|stem| stem.ends_with(root))
    })
}

/// DH.8: a graphql-ruby object superclass: one whose last segment ends in
/// `BaseObject` (`Types::BaseObject`, the generator's base class) or
/// `GraphQL::Schema::Object` itself.
fn ruby_object_superclass(segs: &[&str]) -> bool {
    segs.last().is_some_and(|s| s.ends_with("BaseObject")) || segs == ["GraphQL", "Schema", "Object"]
}

/// DH.8: one Ruby `class` declaration: the path it names, its superclass
/// path (empty for none), the offset of `class` and its 0-indexed line.
struct RubyClass<'a> {
    name: Vec<&'a str>,
    superclass: Vec<&'a str>,
    class_at: usize,
    line: usize,
}

/// DH.8: every `class <Path> [< <Superclass>]` declaration of a Ruby mask
/// that opens its line; `class << self` is none.
fn ruby_classes<'a>(src: &'a str, m: &[u8]) -> Vec<RubyClass<'a>> {
    let mut classes = Vec::new();
    for (line, (start, end)) in line_spans(m).into_iter().enumerate() {
        let at = first_non_blank(m, start, end);
        if m.get(at..at + 5) != Some(b"class") || m.get(at + 5).is_some_and(|&c| is_ident_byte(c)) {
            continue;
        }
        let after = first_non_blank(m, at + 5, end);
        if m.get(after..end).is_some_and(|r| r.starts_with(b"<<")) {
            continue;
        }
        let (name, name_end) = ruby_path(src, m, after, end);
        if name.is_empty() {
            continue;
        }
        let lt = first_non_blank(m, name_end, end);
        let superclass = if m.get(lt) == Some(&b'<') && m.get(lt + 1) != Some(&b'<') {
            ruby_path(src, m, lt + 1, end).0
        } else {
            Vec::new()
        };
        classes.push(RubyClass { name, superclass, class_at: at, line });
    }
    classes
}

/// DH.8: the census sites of a Ruby file: every whole-word `class` whose
/// declared name (read from the source, so a commented-out declaration
/// counts) is root-shaped ([`ruby_root`]).
fn ruby_root_class_sites(source: &str) -> Vec<usize> {
    let b = source.as_bytes();
    word_sites(b, b"class")
        .into_iter()
        .filter(|&at| {
            let end = b.get(at..).and_then(|r| r.iter().position(|&c| c == b'\n')).map_or(b.len(), |p| at + p);
            ruby_path(source, b, at + 5, end).0.last().is_some_and(|name| ruby_root(name).is_some())
        })
        .collect()
}

/// DH.8: a Ruby file that uses graphql-ruby: `GraphQL::Schema` (the schema
/// class, or `GraphQL::Schema::Object`) at code position, or a class whose
/// superclass ends in `BaseObject`.
fn ruby_imports_graphql(src: &str, m: &[u8]) -> bool {
    let needle = b"GraphQL::Schema";
    let schema = m.windows(needle.len()).enumerate().any(|(at, w)| {
        w == needle && !ident_byte_before(m, at) && !m.get(at + needle.len()).is_some_and(|&c| is_ident_byte(c))
    });
    schema || ruby_classes(src, m).iter().any(|c| c.superclass.last().is_some_and(|s| s.ends_with("BaseObject")))
}

/// DH.8: graphql-ruby's `Schema::Member::BuildType.camelize`, the default
/// for every field (`camelize: true`): a name with no `_` (or `_` itself)
/// stays as written; otherwise each `_`-separated part is capitalised
/// (first char upper, the rest lower, Ruby's `String#capitalize`), the parts
/// are joined, the first char is lowered, and the name's leading
/// underscores are put back. `user_orders` -> `userOrders`, `user_ID` ->
/// `userId`, `a__b` -> `aB`, `_private` -> `_private`.
fn ruby_camelize(s: &str) -> String {
    if s == "_" || !s.contains('_') {
        return s.to_string();
    }
    let mut joined = String::new();
    for part in s.split('_') {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            joined.extend(first.to_uppercase());
            joined.extend(chars.flat_map(char::to_lowercase));
        }
    }
    let mut chars = joined.chars();
    let Some(first) = chars.next() else {
        return s.to_string();
    };
    let lead = s.len() - s.trim_start_matches('_').len();
    let mut out = "_".repeat(lead);
    out.extend(first.to_lowercase());
    out.push_str(chars.as_str());
    out
}

/// DH.8: the offset past one Ruby statement starting at `at` on line `j` of
/// a mask: its line, plus each following line while the text so far ends
/// in `,` / `(` / `\` or leaves a bracket open (a multi-line `field` call),
/// at most [`PY_CALL_WINDOW`] lines.
fn ruby_statement_end(m: &[u8], lines: &[(usize, usize)], j: usize, at: usize) -> usize {
    let mut depth = 0i32;
    let mut end = at;
    for &(start, line_end) in lines.iter().skip(j).take(PY_CALL_WINDOW) {
        let from = start.max(at);
        for &c in m.get(from..line_end).unwrap_or_default() {
            match c {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth -= 1,
                _ => {}
            }
        }
        end = line_end;
        let tail = m.get(from..line_end).unwrap_or_default().trim_ascii_end();
        if depth <= 0 && !matches!(tail.last(), Some(b',' | b'(' | b'\\')) {
            break;
        }
    }
    end
}

/// DH.8: the offset of a `key:` (or `:key =>`) option's value in the
/// statement `from..to` of a Ruby mask.
fn ruby_option(m: &[u8], from: usize, to: usize, key: &[u8]) -> Option<usize> {
    let stmt = m.get(from..to)?;
    word_sites(stmt, key).into_iter().find_map(|at| {
        let after = at + key.len();
        let value = match (stmt.get(after), stmt.get(after + 1)) {
            (Some(b':'), next) if next != Some(&b':') => after + 1,
            _ => {
                let arrow = first_non_blank(stmt, after, stmt.len());
                stmt.get(arrow..arrow + 2).filter(|r| *r == b"=>").map(|_| arrow + 2)?
            }
        };
        Some(from + first_non_blank(stmt, value, stmt.len()))
    })
}

/// DH.8: one graphql-ruby `field` call whose `field` word starts at `at` on
/// line `j` of the mask: its Ruby name (a `:symbol` or a quoted string),
/// its schema name ([`ruby_camelize`]d unless the call holds `camelize:
/// false`) and the type method that resolves it (a `resolver_method:
/// :name` option, else the Ruby name), for the anchor.
fn ruby_field(src: &str, m: &[u8], lines: &[(usize, usize)], j: usize, at: usize) -> Option<(String, String)> {
    let line_end = lines.get(j)?.1;
    // Blanks are skipped in the source: the mask blanks a quoted name too.
    let sb = src.as_bytes();
    let mut p = first_non_blank(sb, at + 5, line_end);
    if m.get(p) == Some(&b'(') {
        p = first_non_blank(sb, p + 1, line_end);
    }
    let name = match (m.get(p), sb.get(p)) {
        (Some(b':'), _) => {
            let end = cs_ident_end(m, p + 1, line_end);
            if matches!(m.get(end), Some(b'?' | b'!' | b'=' | b':')) {
                return None;
            }
            src.get(p + 1..end)?
        }
        (Some(b' '), Some(b'"' | b'\'')) => quoted_literal(src.get(p..line_end)?)?,
        _ => return None,
    };
    if !is_ident(name) {
        return None;
    }
    let end = ruby_statement_end(m, lines, j, at);
    let camel = !ruby_option(m, at, end, b"camelize")
        .is_some_and(|v| m.get(v..v + 5) == Some(b"false") && !m.get(v + 5).is_some_and(|&c| is_ident_byte(c)));
    let method = ruby_option(m, at, end, b"resolver_method")
        .filter(|&v| m.get(v) == Some(&b':'))
        .and_then(|v| src.get(v + 1..cs_ident_end(m, v + 1, end)))
        .filter(|s| !s.is_empty())
        .unwrap_or(name);
    let schema = if camel { ruby_camelize(name) } else { name.to_string() };
    Some((schema, method.to_string()))
}

/// DH.8: the instance method a `def` line starting at `at` of a Ruby mask
/// defines; a class method (`def self.x`) is none.
fn ruby_def_name<'a>(src: &'a str, m: &[u8], at: usize, line_end: usize) -> Option<&'a str> {
    let p = first_non_blank(m, at + 3, line_end);
    let end = cs_ident_end(m, p, line_end);
    if m.get(end) == Some(&b'.') {
        return None;
    }
    src.get(p..end).filter(|s| !s.is_empty())
}

/// DH.8: the fields of the graphql-ruby root class declared on line
/// `class_line`, as (schema name, 0-indexed anchor line) in line order. The
/// body is the lines after the class line indented deeper than it (the
/// `end` at the class line's indentation closes it). A `def` / nested
/// `class` / `module` line hides its own body (the lines indented deeper,
/// and its `end`), so a `field` call inside a method or a nested class is
/// none. Each `field` line left is a field ([`ruby_field`]); it anchors at
/// the class's `def` of its resolving method when the class has one (so
/// HANDLED_BY lands on that method), else at the `field` line.
fn ruby_class_fields(src: &str, m: &[u8], lines: &[(usize, usize)], class_line: usize) -> Vec<(String, u32)> {
    let sb = src.as_bytes();
    let indent = |(start, end): (usize, usize)| first_non_blank(sb, start, end) - start;
    let Some(&class_span) = lines.get(class_line) else {
        return Vec::new();
    };
    let class_indent = indent(class_span);
    let mut hidden: Option<usize> = None;
    let mut defs: Vec<(&str, usize)> = Vec::new();
    let mut found: Vec<(String, String, usize)> = Vec::new();
    for (j, &(start, end)) in lines.iter().enumerate().skip(class_line + 1) {
        let at = first_non_blank(m, start, end);
        if m.get(at..end).is_none_or(|r| r.trim_ascii().is_empty()) {
            continue;
        }
        let ind = indent((start, end));
        let word = |w: &[u8]| m.get(at..at + w.len()) == Some(w) && !m.get(at + w.len()).is_some_and(|&c| is_ident_byte(c));
        if let Some(k) = hidden {
            if ind > k {
                continue;
            }
            hidden = None;
            if ind == k && word(b"end") {
                continue;
            }
        }
        if ind <= class_indent {
            break;
        }
        if word(b"def") {
            if let Some(name) = ruby_def_name(src, m, at, end) {
                defs.push((name, j));
            }
            hidden = Some(ind);
        } else if word(b"class") || word(b"module") {
            hidden = Some(ind);
        } else if word(b"field")
            && let Some((schema, method)) = ruby_field(src, m, lines, j, at)
        {
            found.push((schema, method, j));
        }
    }
    found
        .into_iter()
        .map(|(schema, method, j)| {
            let anchor = defs.iter().find(|&&(d, _)| d == method).map_or(j, |&(_, line)| line);
            (schema, line_u32(anchor))
        })
        .collect()
}

/// DH.8: a Ruby file's graphql-ruby root nouns and their fields, given its
/// [`ruby_mask`]: each class whose name is root-shaped ([`ruby_root`]) and
/// whose superclass is a graphql-ruby object ([`ruby_object_superclass`])
/// names its root, anchored at its class line, and its body's `field` calls
/// are the root's fields ([`ruby_class_fields`]). An object type (`class
/// OrderType < Types::BaseObject`) names nothing.
fn graphql_ruby_roots_and_fields(src: &str, m: &[u8]) -> (RootNouns, Vec<(String, u32)>) {
    let lines = line_spans(m);
    let mut roots = RootNouns::new();
    let mut fields = Vec::new();
    for class in ruby_classes(src, m) {
        let Some(root) = class.name.last().and_then(|n| ruby_root(n)) else {
            continue;
        };
        if !ruby_object_superclass(&class.superclass) {
            continue;
        }
        if !roots.iter().any(|&(r, _)| r == root) {
            roots.push((root, line_of(src, class.class_at)));
        }
        fields.extend(ruby_class_fields(src, m, &lines, class.line));
    }
    fields.sort_by_key(|&(_, line)| line);
    (roots, fields)
}

/// DH.8: the end of the Elixir string or charlist opening at `at` (`"`,
/// `'`, or a `"""` / `'''` heredoc), `\` escapes honoured; the end of input
/// when it never closes. Strings may span lines.
fn elixir_string_end(b: &[u8], at: usize) -> usize {
    let Some(&q) = b.get(at) else {
        return at;
    };
    let triple = [q, q, q];
    if b.get(at..at + 3) == Some(&triple[..]) {
        return find_close(b, at + 3, &triple, false).map_or(b.len(), |p| p + 3);
    }
    find_close(b, at + 1, &[q], false).map_or(b.len(), |p| p + 1)
}

/// DH.8: the end of the Elixir sigil opening at `at` (`~s(..)`, `~r/../i`,
/// `~S"""..."""`): its letters, its delimiter (a bracket closes with its
/// pair, any other with itself), the body and the modifier letters after
/// it. `None` when no sigil opens there.
fn elixir_sigil_end(b: &[u8], at: usize) -> Option<usize> {
    let letters = b.get(at + 1..)?.iter().take_while(|c| c.is_ascii_alphabetic()).count();
    if letters == 0 {
        return None;
    }
    let open = at + 1 + letters;
    let d = *b.get(open)?;
    let body_end = match d {
        b'"' | b'\'' => elixir_string_end(b, open),
        b'(' | b'[' | b'{' | b'<' | b'/' | b'|' => {
            let close = match d {
                b'(' => b')',
                b'[' => b']',
                b'{' => b'}',
                b'<' => b'>',
                other => other,
            };
            find_close(b, open + 1, &[close], false).map_or(b.len(), |p| p + 1)
        }
        _ => return None,
    };
    Some(body_end + b.get(body_end..).map_or(0, |r| r.iter().take_while(|c| c.is_ascii_alphabetic()).count()))
}

/// DH.8: [`code_mask`] of an Elixir file. One lexing pass blanks `#`
/// comments, `"` strings and `'` charlists (`"""` / `'''` heredocs
/// included), sigils ([`elixir_sigil_end`]) and `?c` char literals (`?#`,
/// `?"`; a `?` after an identifier byte ends a name such as `valid?`).
/// String interpolation is read as part of its string.
fn elixir_code_mask(source: &str) -> Vec<u8> {
    let b = source.as_bytes();
    let mut code = vec![true; b.len()];
    let mut i = 0;
    while i < b.len() {
        let end = match b[i] {
            b'#' => Some(b.get(i..).and_then(|r| r.iter().position(|&c| c == b'\n')).map_or(b.len(), |p| i + p)),
            b'"' | b'\'' => Some(elixir_string_end(b, i)),
            b'~' => elixir_sigil_end(b, i),
            b'?' if !ident_byte_before(b, i) => match b.get(i + 1) {
                Some(b'\\') => Some(i + 3),
                Some(_) => source.get(i + 1..).and_then(|r| r.chars().next()).map(|c| i + 1 + c.len_utf8()),
                None => None,
            },
            _ => None,
        };
        match end.map(|e| e.min(b.len())) {
            Some(end) if end > i => {
                code.iter_mut().take(end).skip(i).for_each(|c| *c = false);
                i = end;
            }
            _ => i += 1,
        }
    }
    code_mask(source, |at| code.get(at).copied().unwrap_or(false))
}

/// DH.8: an Elixir file that is an Absinthe schema: `use Absinthe.Schema`
/// at code position (`use Absinthe.Schema.Notation`, a type module's
/// import, is not).
fn absinthe_schema(m: &[u8]) -> bool {
    let module = b"Absinthe.Schema";
    word_sites(m, b"use").into_iter().any(|at| {
        let p = skip_ascii_ws(m, at + 3);
        m.get(p..p + module.len()) == Some(&module[..])
            && !m.get(p + module.len()).is_some_and(|&c| is_ident_byte(c) || c == b'.')
    })
}

/// DH.8: the offset past the `do` that opens the root block whose
/// `query` / `mutation` / `subscription` word ends at `after` on a line
/// ending at `line_end`: the next word is `do`, or the macro takes options
/// first (`query name: "RootQuery" do`, `query(name: "Q") do`) and the line
/// ends its options with a `do`. A keyword `do:` one-liner opens no block.
fn absinthe_block_do(b: &[u8], after: usize, line_end: usize) -> Option<usize> {
    let p = first_non_blank(b, after, line_end);
    let opener = |at: usize| {
        b.get(at..at + 2) == Some(b"do")
            && !ident_byte_before(b, at)
            && !b.get(at + 2).is_some_and(|&c| is_ident_byte(c) || c == b':')
    };
    if opener(p) {
        return Some(p + 2);
    }
    let options = b.get(p) == Some(&b'(') || {
        let end = cs_ident_end(b, p, line_end);
        end > p && b.get(end) == Some(&b':') && b.get(end + 1) != Some(&b':')
    };
    if !options {
        return None;
    }
    (p..line_end).rev().find(|&at| opener(at)).map(|at| at + 2)
}

/// DH.8: the census sites of an Elixir file: every whole-word `query` /
/// `mutation` / `subscription` (not an atom, a call on a module or an
/// attribute) that opens a block on its line ([`absinthe_block_do`]), read
/// from the source, so a commented-out block counts.
fn absinthe_root_sites(source: &str) -> Vec<usize> {
    let b = source.as_bytes();
    let mut sites: Vec<usize> = ROOT_TYPES
        .iter()
        .flat_map(|root| {
            let word = root.to_ascii_lowercase();
            word_sites(b, word.as_bytes())
                .into_iter()
                .filter(|&at| !matches!(at.checked_sub(1).and_then(|p| b.get(p)), Some(b':' | b'.' | b'@')))
                .filter(|&at| {
                    let line_end = b.get(at..).and_then(|r| r.iter().position(|&c| c == b'\n')).map_or(b.len(), |p| at + p);
                    absinthe_block_do(b, at + word.len(), line_end).is_some()
                })
                .collect::<Vec<_>>()
        })
        .collect();
    sites.sort_unstable();
    sites
}

/// DH.8: the fields of the Absinthe root block whose `do` ends at `from` of
/// an Elixir mask, as (schema name, 0-indexed line of `field`). The block
/// nests by token: `do` (not the keyword `do:`) and `fn` open, `end`
/// closes, an atom (`:do`) or a call on a module (`X.end`) is neither. A
/// `field :name` statement at the block's first level is a field, under
/// Absinthe's default `LanguageConventions` external name
/// ([`absinthe_external_name`]); a field's own `do .. end` (its `resolve`,
/// its `arg`s) is a deeper level, so nothing inside it is a root field.
/// `import_fields :order_queries` is not expanded: the imported object's
/// fields are declared in an `object` block, which no root scan reads.
fn absinthe_block_fields(src: &str, m: &[u8], from: usize) -> Vec<(String, u32)> {
    let mut fields = Vec::new();
    let mut depth = 1usize;
    let mut i = from;
    while i < m.len() {
        if !m.get(i).is_some_and(|&c| is_ident_byte(c)) || ident_byte_before(m, i) {
            i += 1;
            continue;
        }
        let end = cs_ident_end(m, i, m.len());
        let token = m.get(i..end).unwrap_or_default();
        let prev = i.checked_sub(1).and_then(|p| m.get(p)).copied();
        let next = m.get(end).copied();
        let plain = !matches!(prev, Some(b':' | b'.'));
        match token {
            b"do" | b"fn" if plain && next != Some(b':') => depth += 1,
            b"end" if plain && next != Some(b':') => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            b"field" if plain && depth == 1 => {
                let line_start = m.get(..i).and_then(|r| r.iter().rposition(|&c| c == b'\n')).map_or(0, |p| p + 1);
                let lead = m.get(line_start..i).unwrap_or_default().iter().rposition(|c| !c.is_ascii_whitespace());
                let statement = lead.is_none_or(|p| m.get(line_start + p) == Some(&b';'));
                let mut p = first_non_blank(m, end, m.len());
                if m.get(p) == Some(&b'(') {
                    p = first_non_blank(m, p + 1, m.len());
                }
                if statement && m.get(p) == Some(&b':') && m.get(p + 1) != Some(&b':') {
                    let name_end = cs_ident_end(m, p + 1, m.len());
                    if let Some(name) = src.get(p + 1..name_end).filter(|n| is_ident(n)) {
                        fields.push((absinthe_external_name(name), line_of(src, i)));
                    }
                }
            }
            _ => {}
        }
        i = end;
    }
    fields
}

/// DH.8: an Elixir file's Absinthe root nouns and their fields, given its
/// [`elixir_code_mask`] and its [`absinthe_root_sites`]: each site at code
/// position that opens its line's statement names its root, anchored at its
/// line, and the `field`s at its block's first level are the root's fields
/// ([`absinthe_block_fields`]). An `object :order do` block names nothing.
fn absinthe_roots_and_fields(src: &str, m: &[u8], sites: &[usize]) -> (RootNouns, Vec<(String, u32)>) {
    let mut roots = RootNouns::new();
    let mut fields = Vec::new();
    for &at in sites.iter().filter(|&&at| m.get(at) != Some(&b' ')) {
        let line_start = m.get(..at).and_then(|r| r.iter().rposition(|&c| c == b'\n')).map_or(0, |p| p + 1);
        if first_non_blank(m, line_start, at) != at {
            continue;
        }
        let line_end = m.get(at..).and_then(|r| r.iter().position(|&c| c == b'\n')).map_or(m.len(), |p| at + p);
        let word = cs_ident_end(m, at, line_end);
        let Some(root) = src.get(at..word).and_then(|w| ROOT_TYPES.iter().copied().find(|r| r.eq_ignore_ascii_case(w))) else {
            continue;
        };
        let Some(open) = absinthe_block_do(m, word, line_end) else {
            continue;
        };
        if !roots.iter().any(|&(r, _)| r == root) {
            roots.push((root, line_of(src, at)));
        }
        fields.extend(absinthe_block_fields(src, m, open));
    }
    fields.sort_by_key(|&(_, line)| line);
    (roots, fields)
}

/// DH.8: Absinthe's default adapter, `LanguageConventions.to_external_name`
/// for a field: a `__` prefix is kept and the rest converted; a name that
/// starts upper-case is `Absinthe.Utils.camelize` (Elixir's
/// `Macro.camelize`, [`macro_camelize`]); any other is `camelize(lower:
/// true)`: its leading underscores kept, the rest `Macro.camelize`d and its
/// first char lowered. `user_orders` -> `userOrders`, `field_2` -> `field2`,
/// `user_ID` -> `user_ID`, `_private` -> `_private`.
fn absinthe_external_name(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("__") {
        return format!("__{}", absinthe_external_name(rest));
    }
    if s.starts_with(|c: char| c.is_ascii_uppercase()) {
        return macro_camelize(s);
    }
    let lead = s.len() - s.trim_start_matches('_').len();
    let camel = macro_camelize(s);
    let mut chars = camel.chars();
    let mut out = "_".repeat(lead);
    if let Some(first) = chars.next() {
        out.extend(first.to_lowercase());
    }
    out.push_str(chars.as_str());
    out
}

/// DH.8: Elixir's `Macro.camelize` on an atom name: leading underscores
/// dropped, the first char upper-cased, then each `_` run collapsed and
/// read against the char after it: a lower-case letter is upper-cased, a
/// digit kept, the end of the name drops it, anything else keeps one `_`.
fn macro_camelize(s: &str) -> String {
    let mut chars = s.trim_start_matches('_').chars().peekable();
    let mut out = String::new();
    if let Some(first) = chars.next() {
        out.push(first.to_ascii_uppercase());
    }
    while let Some(c) = chars.next() {
        if c != '_' {
            out.push(c);
            continue;
        }
        while chars.peek() == Some(&'_') {
            chars.next();
        }
        match chars.peek().copied() {
            Some(h) if h.is_ascii_lowercase() => {
                chars.next();
                out.push(h.to_ascii_uppercase());
            }
            Some(h) if h.is_ascii_digit() => {
                chars.next();
                out.push(h);
            }
            Some(_) => out.push('_'),
            None => {}
        }
    }
    out
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

/// The operation a needle line names from the file's FIRST document tag (the
/// first tag form, in this order, that occurs), with that document (DT.11a:
/// the literal's body up to its closing quote, `None` when it never closes).
/// `None` when that first document names no operation.
fn extract_gql_operation_name(source: &str) -> Option<(String, Option<&str>)> {
    for tag in ["gql`", "gql(`", "gql(\"", "graphql(`", "graphql(\""] {
        if let Some(idx) = source.find(tag) {
            let start = idx + tag.len();
            let after = &source[start..];
            let quote = tag.as_bytes()[tag.len() - 1];
            return extract_operation_from_body(after).map(|name| (name, quoted_body(source, start, quote)));
        }
    }
    None
}

/// DT.11a: the body of the JS string / template literal whose text starts at
/// `start` and which `quote` closes: everything up to the first unescaped
/// `quote`, untrimmed. `None` when it never closes (the document is not
/// known to end anywhere).
fn quoted_body(source: &str, start: usize, quote: u8) -> Option<&str> {
    let b = source.as_bytes();
    let mut i = start;
    while let Some(&c) = b.get(i) {
        if c == b'\\' {
            i += 2;
            continue;
        }
        if c == quote {
            return source.get(start..i);
        }
        i += 1;
    }
    None
}

/// Every operation in a `` gql` `` template, with the byte offset of its
/// `` gql` `` tag and its document (DT.11a: the template body between the
/// backticks, untrimmed, so it starts on the tag's line; `None` for a
/// template that never closes). A named document is named by its operation
/// name; an anonymous or shorthand one (`` gql`{ orders { id } }` ``) by its
/// first root field ([`first_root_field`], CL.6a's rule for client requests)
/// when it closes. CJ.1b: a tag `guard` refuses (it starts in a Rust /
/// Python literal or comment) is no template.
fn extract_gql_template_operations<'s>(
    source: &'s str,
    guard: &mut LazyGuard<'_>,
) -> Vec<(String, usize, Option<&'s str>)> {
    let mut ops = Vec::new();
    let mut search_from = 0;
    while let Some(idx) = source[search_from..].find("gql`") {
        let tag = search_from + idx;
        let abs = tag + 4;
        search_from = abs;
        if !guard.admits(tag) {
            continue;
        }
        let doc = quoted_body(source, abs, b'`');
        let name = extract_operation_from_body(doc.unwrap_or(&source[abs..]))
            .or_else(|| doc.filter(|d| is_anonymous_operation(d)).and_then(first_root_field));
        if let Some(name) = name {
            ops.push((name, tag, doc));
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

// ---- DH.8: Dart package:graphql documents ----

/// DH.8: package:graphql and graphql_flutter (which re-exports its client),
/// as scope prefixes of a quoted import URI ([`quoted_module_at`]).
const DART_GRAPHQL_PACKAGES: &[&str] = &["package:graphql/", "package:graphql_flutter/"];

/// DH.8: the words that may precede a document constant's name on its
/// declaration line; one of `const` / `final` / `var` must be among them.
const DART_DECL_WORDS: &[&str] = &["static", "late", "const", "final", "var", "String"];

/// DH.8: where a Dart operation's name was read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DartDoc {
    /// `gql(<ident>)`, the identifier a same-file string constant.
    Constant,
    /// `gql(<literal>)`.
    Inline,
}

/// DH.8: a Dart file that imports package:graphql or graphql_flutter.
fn dart_imports_graphql(source: &str) -> bool {
    let b = source.as_bytes();
    b.iter()
        .enumerate()
        .filter(|&(_, &c)| c == b'\'' || c == b'"')
        .any(|(i, _)| DART_GRAPHQL_PACKAGES.iter().any(|p| quoted_module_at(b, i + 1, p)))
}

/// DH.8: the Dart string literal opening at `at` (an `r` raw prefix, then
/// `'''` / `"""` spanning lines or `'` / `"` on one line), as (body start,
/// body end, offset past the literal). `None` when no literal opens there or
/// it never closes.
fn dart_literal(b: &[u8], at: usize) -> Option<(usize, usize, usize)> {
    let raw = b.get(at) == Some(&b'r') && !ident_byte_before(b, at);
    let q_at = if raw { at + 1 } else { at };
    let q = *b.get(q_at).filter(|&&q| q == b'\'' || q == b'"')?;
    let triple = [q, q, q];
    if b.get(q_at..q_at + 3) == Some(&triple[..]) {
        let close = find_close(b, q_at + 3, &triple, false)?;
        return Some((q_at + 3, close, close + 3));
    }
    let close = find_close(b, q_at + 1, &[q], true)?;
    Some((q_at + 1, close, close + 1))
}

/// DH.8: the offset past the Dart literal or comment opening at `i`, or
/// `None` when neither does.
fn dart_skip(b: &[u8], i: usize) -> Option<usize> {
    match (b.get(i)?, b.get(i + 1)) {
        (b'/', Some(b'/')) => Some(b.get(i..)?.iter().position(|&c| c == b'\n').map_or(b.len(), |p| i + p)),
        (b'/', Some(b'*')) => Some(find_close(b, i + 2, b"*/", false).map_or(b.len(), |p| p + 2)),
        (b'\'' | b'"', _) | (b'r', Some(b'\'' | b'"')) => dart_literal(b, i).map(|(_, _, end)| end),
        _ => None,
    }
}

/// DH.8: the bracket closing the `(` at `open` of Dart source, over any mix
/// of `()[]{}`, past literals and comments ([`dart_skip`]).
fn dart_call_close(b: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = open;
    while let Some(&c) = b.get(i) {
        if let Some(end) = dart_skip(b, i) {
            i = end;
            continue;
        }
        match c {
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

/// DH.8: the operation the call opening at `open` (a kept needle's `(`)
/// sends, read from the first `gql(..)` among its arguments that names one
/// ([`extract_operation_from_body`]): an inline literal, or an identifier
/// bound to a string constant in the same file ([`dart_const_document`]).
/// `None` when no `gql(..)` in the call names an operation (an anonymous
/// `query { .. }`, a document built in another file). DT.11a: with the
/// document the name was read from (the literal's body).
fn dart_document_name(source: &str, open: usize) -> Option<(String, DartDoc, &str)> {
    let b = source.as_bytes();
    let close = dart_call_close(b, open)?;
    let mut i = open + 1;
    while i < close {
        if let Some(end) = dart_skip(b, i) {
            i = end;
            continue;
        }
        let tag = b.get(i..i + 3) == Some(b"gql")
            && !ident_byte_before(b, i)
            && i.checked_sub(1).and_then(|p| b.get(p)) != Some(&b'.')
            && !b.get(i + 3).is_some_and(|&c| is_ident_byte(c));
        let paren = skip_ascii_ws(b, i + 3);
        if tag && b.get(paren) == Some(&b'(') {
            let arg = skip_ascii_ws(b, paren + 1);
            let named = match dart_literal(b, arg) {
                Some((start, end, _)) => source.get(start..end).and_then(|doc| {
                    extract_operation_from_body(doc).map(|name| (name, DartDoc::Inline, doc))
                }),
                None => {
                    let end = cs_ident_end(b, arg, close);
                    source
                        .get(arg..end)
                        .filter(|id| is_ident(id) && b.get(skip_ascii_ws(b, end)) == Some(&b')'))
                        .and_then(|id| dart_const_document(source, id))
                        .map(|(name, doc)| (name, DartDoc::Constant, doc))
                }
            };
            if named.is_some() {
                return named;
            }
        }
        i += 1;
    }
    None
}

/// DH.8: the operation the same-file string constant `ident` declares:
/// `const listOrders = r"""query Orders {..}""";` (also `final` / `var`,
/// `static const String x = '...'`), read from the first declaration line
/// whose words before the name are all [`DART_DECL_WORDS`] (so a commented
/// line is none) and whose `=` binds a literal; with that literal's body
/// (DT.11a).
fn dart_const_document<'s>(source: &'s str, ident: &str) -> Option<(String, &'s str)> {
    let b = source.as_bytes();
    source.match_indices(ident).find_map(|(at, _)| {
        let end = at + ident.len();
        if ident_byte_before(b, at) || b.get(end).is_some_and(|&c| is_ident_byte(c)) {
            return None;
        }
        let line_start = source.get(..at)?.rfind('\n').map_or(0, |p| p + 1);
        let words: Vec<&str> = source.get(line_start..at)?.split_whitespace().collect();
        let declared = !words.is_empty()
            && words.iter().all(|w| DART_DECL_WORDS.contains(w))
            && words.iter().any(|w| matches!(*w, "const" | "final" | "var"));
        let eq = skip_ascii_ws(b, end);
        if !declared || b.get(eq) != Some(&b'=') || matches!(b.get(eq + 1), Some(b'=' | b'>')) {
            return None;
        }
        let (start, body_end, _) = dart_literal(b, skip_ascii_ws(b, eq + 1))?;
        let doc = source.get(start..body_end)?;
        extract_operation_from_body(doc).map(|name| (name, doc))
    })
}

// ============================================================================
// CL.6a — Go machinebox / C# GraphQL.Client request documents
// ============================================================================

/// machinebox/graphql, as a whole quoted Go import path. The import line's
/// alias (else `graphql`) is the receiver of its `NewRequest(` needle.
const MACHINEBOX_IMPORT: &str = "\"github.com/machinebox/graphql\"";

/// The namespace a C# file names (`using GraphQL.Client.Http;`) when it
/// builds requests for GraphQL.Client.
const GRAPHQL_CLIENT_NAMESPACE: &str = "GraphQL.Client";

/// GraphQL.Client's request types, read after `new` (optionally qualified).
const GRAPHQL_CLIENT_REQUESTS: &[&str] = &["GraphQLRequest", "GraphQLHttpRequest"];

/// What one file's client request needles came to, for the
/// `[graphql-ops] client-request` marker. Every counted needle lands in
/// exactly one of `named` / `root_field` / `unread`.
#[derive(Default, Debug)]
struct ClientRequestTally {
    machinebox: usize,
    graphql_client: usize,
    /// Minted under the document's operation name.
    named: usize,
    /// An anonymous document, minted under its first root field.
    root_field: usize,
    /// A non-literal document, or one with no readable root field.
    unread: usize,
}

impl ClientRequestTally {
    /// The marker line, or `None` for a file with no client request needle.
    fn marker(&self) -> Option<String> {
        (self.machinebox + self.graphql_client > 0).then(|| {
            format!(
                "[graphql-ops] client-request machinebox={} graphql-client={} named={} root_field={} unread={}",
                self.machinebox, self.graphql_client, self.named, self.root_field, self.unread
            )
        })
    }
}

/// CL.6a: the operations of the GraphQL client requests in `source`, as
/// (name, needle byte offset, document) in source order, plus the marker
/// tally. DT.11a: the document is the literal's text as the request sends it
/// (a Go raw string or an unescaped literal is the file's own bytes; escapes
/// are decoded):
/// - Go: `<recv>.NewRequest(<literal>)` in a file importing
///   machinebox/graphql ([`machinebox_receivers`]);
/// - C#: `new GraphQLRequest` / `new GraphQLHttpRequest` with a
///   `{ Query = <literal> }` initializer or a literal first constructor
///   argument, in a file naming `GraphQL.Client`.
///
/// No language tag reaches this extractor, so the import gates are the
/// language gate. A needle on a `//` / `/*` comment line, or one `guard`
/// refuses (a Rust / Python literal or comment, CJ.1b), is no site. A document is named by its operation name
/// ([`extract_operation_from_body`]), else by its first root field
/// ([`first_root_field`]), the field a resolver is keyed by. A non-literal
/// document (a constant, a concatenation, an interpolated string) mints
/// nothing.
fn client_request_operations(
    source: &str,
    guard: &mut LazyGuard<'_>,
) -> (Vec<(String, usize, String)>, ClientRequestTally) {
    let b = source.as_bytes();
    let mut tally = ClientRequestTally::default();
    let mut sites: Vec<(usize, Option<String>)> = Vec::new();
    for recv in machinebox_receivers(source) {
        let needle = if recv.is_empty() {
            "NewRequest(".to_string()
        } else {
            format!("{recv}.NewRequest(")
        };
        for (at, _) in source.match_indices(&needle) {
            let before = at.checked_sub(1).and_then(|p| b.get(p));
            if before.is_some_and(|&c| is_ident_byte(c) || c == b'.')
                || in_line_comment(b, at)
                || !guard.admits(at)
            {
                continue;
            }
            tally.machinebox += 1;
            let arg = skip_ascii_ws(b, at + needle.len());
            sites.push((at, go_literal(b, arg).and_then(|(doc, end)| ends_arg(b, end, b")").then_some(doc))));
        }
    }
    let names_client = source.match_indices(GRAPHQL_CLIENT_NAMESPACE).any(|(at, ns)| {
        !ident_byte_before(b, at) && !b.get(at + ns.len()).is_some_and(|&c| is_ident_byte(c))
    });
    if names_client {
        for &ty in GRAPHQL_CLIENT_REQUESTS {
            for (at, _) in source.match_indices(ty) {
                let after = at + ty.len();
                if ident_byte_before(b, at)
                    || b.get(after).is_some_and(|&c| is_ident_byte(c))
                    || !preceded_by_new(b, qualifier_start(b, at))
                    || in_line_comment(b, at)
                    || !guard.admits(at)
                {
                    continue;
                }
                tally.graphql_client += 1;
                sites.push((at, cs_request_document(b, after)));
            }
        }
    }
    sites.sort_by_key(|&(at, _)| at);
    let mut ops = Vec::new();
    for (at, doc) in sites {
        let Some(doc) = doc else {
            tally.unread += 1;
            continue;
        };
        if let Some(name) = extract_operation_from_body(&doc) {
            tally.named += 1;
            ops.push((name, at, doc));
        } else if let Some(field) = first_root_field(&doc) {
            tally.root_field += 1;
            ops.push((field, at, doc));
        } else {
            tally.unread += 1;
        }
    }
    (ops, tally)
}

/// The receivers machinebox/graphql is imported under: the import spec's
/// alias, else `graphql`; `""` for a dot import. A blank (`_`) import, or a
/// quoted path that is not an import spec, gives none.
fn machinebox_receivers(source: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (at, _) in source.match_indices(MACHINEBOX_IMPORT) {
        let head = source.get(..at).unwrap_or_default();
        let head = head.get(head.rfind('\n').map_or(0, |p| p + 1)..).unwrap_or_default().trim();
        let head = head
            .strip_prefix("import")
            .filter(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
            .map_or(head, str::trim);
        let recv = match head {
            "" => "graphql",
            "." => "",
            alias if alias != "_" && is_ident(alias) => alias,
            _ => continue,
        };
        if !out.iter().any(|r| r == recv) {
            out.push(recv.to_string());
        }
    }
    out
}

/// Where the (possibly `GraphQL.`-qualified) type name at `at` starts.
fn qualifier_start(b: &[u8], at: usize) -> usize {
    let mut start = at;
    while start >= 2 && b.get(start - 1) == Some(&b'.') && b.get(start - 2).is_some_and(|&c| is_ident_byte(c)) {
        start -= 1;
        while start >= 1 && b.get(start - 1).is_some_and(|&c| is_ident_byte(c)) {
            start -= 1;
        }
    }
    start
}

/// True when the token after a literal ending at `end` closes the argument
/// or member: `,` or one of `closers`. Anything else (`+`, a call) makes the
/// value a non-literal expression.
fn ends_arg(b: &[u8], end: usize, closers: &[u8]) -> bool {
    b.get(skip_ascii_ws(b, end)).is_some_and(|c| *c == b',' || closers.contains(c))
}

/// The text of a one-line backslash-escaped string whose body starts at
/// `from`, and the index just past its closing `"`. `\n` / `\t` / `\r`
/// decode to blanks, `\"` / `\\` / `\'` to the character; any other escape
/// is kept as written.
fn escaped_string(b: &[u8], from: usize) -> Option<(String, usize)> {
    let mut text = Vec::new();
    let mut i = from;
    loop {
        match *b.get(i)? {
            b'"' => return Some((String::from_utf8(text).ok()?, i + 1)),
            b'\n' => return None,
            b'\\' => {
                let esc = *b.get(i + 1)?;
                match esc {
                    b'n' => text.push(b'\n'),
                    b't' => text.push(b'\t'),
                    b'r' => text.push(b'\r'),
                    b'"' | b'\\' | b'\'' => text.push(esc),
                    _ => text.extend_from_slice(&[b'\\', esc]),
                }
                i += 2;
            }
            c => {
                text.push(c);
                i += 1;
            }
        }
    }
}

/// A Go string literal opening at `at` (a raw `` `..` `` string or an
/// interpreted `".."` one), and the index just past it.
fn go_literal(b: &[u8], at: usize) -> Option<(String, usize)> {
    match *b.get(at)? {
        b'`' => {
            let close = at + 1 + b.get(at + 1..)?.iter().position(|&c| c == b'`')?;
            let body = b.get(at + 1..close)?;
            Some((String::from_utf8(body.to_vec()).ok()?, close + 1))
        }
        b'"' => escaped_string(b, at + 1),
        _ => None,
    }
}

/// A C# string literal opening at `at` and the index just past it. The text
/// is `None` for an interpolated (`$`) string, whose holes are not literal.
/// Forms: `".."` (backslash escapes), `@".."` (`""` escape, multi-line) and a
/// `"""..."""` raw string (three or more quotes, closed by as many).
fn cs_literal(b: &[u8], at: usize) -> Option<(Option<String>, usize)> {
    let mut i = at;
    let (mut interpolated, mut verbatim) = (false, false);
    while let Some(&c) = b.get(i) {
        match c {
            b'$' => interpolated = true,
            b'@' => verbatim = true,
            _ => break,
        }
        i += 1;
    }
    if b.get(i) != Some(&b'"') {
        return None;
    }
    let quotes = b.get(i..)?.iter().take_while(|&&c| c == b'"').count();
    let (text, end) = if quotes >= 3 && !verbatim {
        let delim = vec![b'"'; quotes];
        let open = i + quotes;
        let close = open + b.get(open..)?.windows(quotes).position(|w| w == delim.as_slice())?;
        (String::from_utf8(b.get(open..close)?.to_vec()).ok()?, close + quotes)
    } else if verbatim {
        let mut text = Vec::new();
        let mut j = i + 1;
        loop {
            match (*b.get(j)?, b.get(j + 1)) {
                (b'"', Some(b'"')) => {
                    text.push(b'"');
                    j += 2;
                }
                (b'"', _) => break,
                (c, _) => {
                    text.push(c);
                    j += 1;
                }
            }
        }
        (String::from_utf8(text).ok()?, j + 1)
    } else {
        escaped_string(b, i + 1)?
    };
    Some(((!interpolated).then_some(text), end))
}

/// The index just past the C# string / char literal or comment that starts
/// at `i`, or `None` when none starts there. An unterminated one runs to the
/// end of the source.
fn cs_skip_opaque(b: &[u8], i: usize) -> Option<usize> {
    match (*b.get(i)?, b.get(i + 1)) {
        (b'/', Some(b'/')) => Some(b.get(i..)?.iter().position(|&x| x == b'\n').map_or(b.len(), |p| i + p)),
        (b'/', Some(b'*')) => Some(find_close(b, i + 2, b"*/", false).map_or(b.len(), |p| p + 2)),
        (b'\'', _) => Some(find_close(b, i + 1, b"'", true).map_or(b.len(), |p| p + 1)),
        (c @ (b'"' | b'@' | b'$'), _) => match cs_literal(b, i) {
            Some((_, end)) => Some(end),
            None => (c == b'"').then_some(b.len()),
        },
        _ => None,
    }
}

/// The index of the bracket closing the one opened at `open`, over any mix
/// of `()[]{}`, skipping C# string / char literals and comments.
fn cs_matching_close(b: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = open;
    while let Some(&c) = b.get(i) {
        if let Some(end) = cs_skip_opaque(b, i) {
            i = end;
            continue;
        }
        match c {
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

/// The document of a GraphQL.Client request whose type name ends at
/// `after`: the `Query = <literal>` member of a `{ .. }` object initializer
/// (assigned after the constructor ran, so it wins), else a literal first
/// constructor argument (`query:` may name it).
fn cs_request_document(b: &[u8], after: usize) -> Option<String> {
    let mut i = skip_ascii_ws(b, after);
    let mut ctor = None;
    if b.get(i) == Some(&b'(') {
        let mut arg = skip_ascii_ws(b, i + 1);
        if b.get(arg..).is_some_and(|r| r.starts_with(b"query"))
            && b.get(skip_ascii_ws(b, arg + 5)) == Some(&b':')
        {
            arg = skip_ascii_ws(b, skip_ascii_ws(b, arg + 5) + 1);
        }
        ctor = cs_literal(b, arg).and_then(|(doc, end)| doc.filter(|_| ends_arg(b, end, b")")));
        i = skip_ascii_ws(b, cs_matching_close(b, i)? + 1);
    }
    if b.get(i) == Some(&b'{')
        && let Some(member) = cs_initializer_query(b, i)
    {
        return member;
    }
    ctor
}

/// The `Query` member of the object initializer opened at `open`:
/// `Some(Some(doc))` for a literal value, `Some(None)` for any other
/// expression, `None` when the initializer sets no `Query`.
fn cs_initializer_query(b: &[u8], open: usize) -> Option<Option<String>> {
    let close = cs_matching_close(b, open)?;
    let mut depth = 0usize;
    let mut expect_member = true;
    let mut i = open + 1;
    while i < close {
        if let Some(end) = cs_skip_opaque(b, i) {
            // A literal is a value; a comment is trivia.
            expect_member &= b.get(i) == Some(&b'/');
            i = end;
            continue;
        }
        let c = *b.get(i)?;
        match c {
            _ if c.is_ascii_whitespace() => {}
            b'(' | b'[' | b'{' => {
                depth += 1;
                expect_member = false;
            }
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => expect_member = true,
            _ if is_ident_byte(c) && depth == 0 && expect_member => {
                let end = i + b.get(i..close)?.iter().take_while(|&&x| is_ident_byte(x)).count();
                let eq = skip_ascii_ws(b, end);
                if b.get(i..end) == Some(b"Query") && b.get(eq) == Some(&b'=') && b.get(eq + 1) != Some(&b'=') {
                    let value = skip_ascii_ws(b, eq + 1);
                    return Some(
                        cs_literal(b, value).and_then(|(doc, end)| doc.filter(|_| ends_arg(b, end, b"}"))),
                    );
                }
                i = end;
                expect_member = false;
                continue;
            }
            _ => expect_member = false,
        }
        i += 1;
    }
    None
}

/// GraphQL ignored tokens from `i`: blanks, commas and `#` comments.
fn gql_skip(b: &[u8], mut i: usize) -> usize {
    while let Some(&c) = b.get(i) {
        if c == b'#' {
            i += b.get(i..).and_then(|r| r.iter().position(|&x| x == b'\n')).unwrap_or(b.len() - i);
        } else if c.is_ascii_whitespace() || c == b',' {
            i += 1;
        } else {
            break;
        }
    }
    i
}

/// The index of the bracket closing the one opened at `open`, over any mix
/// of `()[]{}`, skipping GraphQL strings and `#` comments.
fn gql_close(b: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = open;
    while let Some(&c) = b.get(i) {
        match c {
            b'"' => {
                i = find_close(b, i + 1, b"\"", false)? + 1;
                continue;
            }
            b'#' => {
                i = gql_skip(b, i);
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

/// The GraphQL name (`[_A-Za-z][_0-9A-Za-z]*`) at `i`, and its end.
fn gql_name(s: &str, i: usize) -> Option<(&str, usize)> {
    let b = s.as_bytes();
    if !b.get(i).is_some_and(|&c| c.is_ascii_alphabetic() || c == b'_') {
        return None;
    }
    let end = i + b.get(i..)?.iter().take_while(|&&c| c.is_ascii_alphanumeric() || c == b'_').count();
    Some((s.get(i..end)?, end))
}

/// Past an optional `( .. )` argument / variable list and any `@directive`s
/// (each with optional arguments) from `i`, and the ignored tokens after.
/// `None` when a bracket never closes.
fn gql_skip_args_and_directives(s: &str, i: usize) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = i;
    loop {
        if b.get(i) == Some(&b'(') {
            i = gql_skip(b, gql_close(b, i)? + 1);
        }
        if b.get(i) != Some(&b'@') {
            return Some(i);
        }
        i = gql_skip(b, gql_name(s, i + 1)?.1);
    }
}

/// DT.11a: true when `doc`'s operation carries no name: a shorthand `{ .. }`
/// or a bare `query` / `mutation` / `subscription` keyword followed by no
/// name (`query ($n: Int) { .. }`). A named document whose name
/// [`extract_operation_from_body`] misses (`query\nOrders { .. }`) is never
/// named by its root field.
fn is_anonymous_operation(doc: &str) -> bool {
    let b = doc.as_bytes();
    match gql_name(doc, gql_skip(b, 0)) {
        Some(("query" | "mutation" | "subscription", end)) => gql_name(doc, gql_skip(b, end)).is_none(),
        Some(_) => false,
        None => true,
    }
}

/// The first root field `body` selects ([`operation_root_fields`]), the
/// field a resolver is keyed by: CL.6a's name for an anonymous client
/// request, and DT.11a's for an anonymous `` gql` `` template.
fn first_root_field(body: &str) -> Option<String> {
    operation_root_fields(body).into_iter().next()
}

/// DT.11a: every top-level field the document's operation selects, in
/// order, deduped: the fields the GraphQL resolver pairs the operation by
/// (`graph/src/resolvers/graphql.rs`). The walk passes blanks, commas and `#`
/// comments, an optional `query` / `mutation` / `subscription` keyword with
/// an optional name, variable list and directives, then reads the selection
/// set after the `{`:
/// - a field after an alias names the field (`recent: orders` -> `orders`),
///   and its arguments, directives and sub-selection are skipped;
/// - an introspection field (`__typename`, `__schema`) is passed over;
/// - an inline fragment (`... on Query { f }`, `... @include(if: $x) { f }`)
///   contributes its fields; a fragment spread (`...F`) contributes nothing.
///
/// A fragment definition, SDL (`type Query { .. }`) or any other shape gives
/// none. A document that stops parsing part way (an unclosed bracket, a
/// template hole `${..}` where a field should be) gives the fields read
/// before it.
pub fn operation_root_fields(doc: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let b = doc.as_bytes();
    let mut i = gql_skip(b, 0);
    if let Some((word, end)) = gql_name(doc, i)
        && matches!(word, "query" | "mutation" | "subscription")
    {
        i = gql_skip(b, end);
        if let Some((_, end)) = gql_name(doc, i) {
            i = gql_skip(b, end);
        }
        let Some(open) = gql_skip_args_and_directives(doc, i) else {
            return fields;
        };
        i = open;
    }
    if b.get(i) == Some(&b'{') {
        // `None` is a document that stops parsing: keep what was read.
        let _ = read_root_selection(doc, i, &mut fields);
    }
    fields
}

/// The root selection set opened at `open`, its fields appended to `fields`
/// ([`operation_root_fields`]'s rules). Inline fragments are read in place,
/// one brace level per open fragment, so no recursion is needed. `None` when
/// the set stops parsing before it closes.
fn read_root_selection(s: &str, open: usize, fields: &mut Vec<String>) -> Option<()> {
    let b = s.as_bytes();
    // The operation's own `{` plus one per open inline fragment.
    let mut depth = 1usize;
    let mut i = gql_skip(b, open + 1);
    loop {
        match *b.get(i)? {
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(());
                }
                i = gql_skip(b, i + 1);
            }
            b'.' => {
                if b.get(i..i + 3) != Some(b"...") {
                    return None;
                }
                let mut j = gql_skip(b, i + 3);
                // `... on Type`: an inline fragment with a type condition;
                // `...Name`: a spread; `... @dir {` / `... {`: an inline
                // fragment without one.
                let inline = match gql_name(s, j) {
                    Some(("on", end)) => {
                        j = gql_skip(b, gql_name(s, gql_skip(b, end))?.1);
                        true
                    }
                    Some((_, end)) => {
                        j = gql_skip(b, end);
                        false
                    }
                    None => true,
                };
                j = gql_skip_args_and_directives(s, j)?;
                if inline {
                    if b.get(j) != Some(&b'{') {
                        return None;
                    }
                    depth += 1;
                    j = gql_skip(b, j + 1);
                }
                i = j;
            }
            _ => {
                let (mut field, end) = gql_name(s, i)?;
                i = gql_skip(b, end);
                if b.get(i) == Some(&b':') {
                    let (name, end) = gql_name(s, gql_skip(b, i + 1))?;
                    field = name;
                    i = gql_skip(b, end);
                }
                if !field.starts_with("__") && !fields.iter().any(|f| f == field) {
                    fields.push(field.to_string());
                }
                i = gql_skip_args_and_directives(s, i)?;
                if b.get(i) == Some(&b'{') {
                    i = gql_skip(b, gql_close(b, i)? + 1);
                }
            }
        }
    }
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
        assert_eq!(hits, vec![("request".to_string(), 2, None)]);
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
        assert_eq!(hits, vec![("request".to_string(), 2, None)]);
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
        // DG.4: async-graphql's needles in a Rust table, the crate named only
        // in strings.
        let rs_table = "pub const NEEDLES: &[&str] = &[\"use async_graphql::Object;\", \"#[Object]\", \"#[Subscription(\", \"RootResolver(\"];\nimpl QueryRoot { fn orders(&self) {} }";
        assert_resolvers(rs_table, "rust", &[]);
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

    // ---- CL.6a: Go machinebox / C# GraphQL.Client requests ----

    /// The client tally of one file, read as `path`.
    fn request_tally(source: &str, path: &str) -> ClientRequestTally {
        client_request_operations(source, &mut LazyGuard::new(path, source)).1
    }

    /// The operation qnames of `source` read from `path`.
    fn op_qnames_at(source: &str, path: &str) -> Vec<String> {
        let out = extract_graphql_operation_nodes(source, path, module_id(), repo());
        let mut qnames: Vec<String> = out.nav.qname_by_id.values().cloned().collect();
        qnames.sort();
        qnames
    }

    /// matrix/go/graphql's client: an anonymous document is named by its
    /// first root field, anchored on the `NewRequest(` line.
    #[test]
    fn machinebox_request_names_its_root_field() {
        let src = "package main\n\nimport (\n\t\"context\"\n\n\t\"github.com/machinebox/graphql\"\n)\n\nfunc ListOrders(ctx context.Context) error {\n\tclient := graphql.NewClient(\"http://api/query\")\n\treq := graphql.NewRequest(`query { orders { id } }`)\n\tvar resp map[string]any\n\treturn client.Run(ctx, req, &resp)\n}\n";
        let out = extract_graphql_operation_nodes(src, "client/client.go", module_id(), repo());
        let qnames: Vec<&String> = out.nav.qname_by_id.values().collect();
        assert_eq!(qnames, vec!["graphql_op:orders"]);
        assert_eq!(out.nodes.first().map(|n| n.confidence), Some(Confidence::Medium));
        assert_eq!(anchor_line(&out, "graphql_op:orders"), Some(10));
        assert_eq!(
            request_tally(src, "client/client.go").marker().as_deref(),
            Some("[graphql-ops] client-request machinebox=1 graphql-client=0 named=0 root_field=1 unread=0")
        );
    }

    /// A named document keeps its operation name; an import alias is the
    /// needle receiver, and an interpreted string's `\n` escapes are blanks.
    #[test]
    fn machinebox_named_operation_keeps_its_name() {
        let named = "import \"github.com/machinebox/graphql\"\n\nvar req = graphql.NewRequest(`\n    query GetOrders($id: ID!) {\n        order(id: $id) { id }\n    }\n`)\n";
        assert_eq!(op_qnames_at(named, "c.go"), vec!["graphql_op:GetOrders"]);
        let alias = "import (\n\tgql \"github.com/machinebox/graphql\"\n)\n\nfunc f() {\n\ta := gql.NewRequest(\"query ListOrders {\\n  orders { id }\\n}\")\n\tb := gql.NewRequest(\n\t\t\"mutation {\\n\\tcancelOrder(id: 1) { id }\\n}\",\n\t)\n\t_ = graphql.NewRequest(`{ ignored }`)\n}\n";
        assert_eq!(
            op_qnames_at(alias, "c.go"),
            vec!["graphql_op:ListOrders", "graphql_op:cancelOrder"]
        );
        let tally = request_tally(alias, "c.go");
        assert_eq!((tally.machinebox, tally.named, tally.root_field, tally.unread), (2, 1, 1, 0));
        // A dot import has no receiver.
        let dot = "import . \"github.com/machinebox/graphql\"\nvar r = NewRequest(`{ shipments { id } }`)\nvar h, _ = http.NewRequest(\"GET\", u, nil)\n";
        assert_eq!(op_qnames_at(dot, "c.go"), vec!["graphql_op:shipments"]);
    }

    /// GraphQL.Client's request: a `Query =` initializer member (regular,
    /// verbatim and raw strings) or the first constructor argument.
    #[test]
    fn graphql_client_request_initializer_and_ctor() {
        let fixture = "using GraphQL;\nusing GraphQL.Client.Http;\n\npublic class BookClient\n{\n    public async Task Load(GraphQLHttpClient client)\n    {\n        var request = new GraphQLRequest { Query = \"query { books { title } }\" };\n        await client.SendQueryAsync<object>(request);\n    }\n}\n";
        let out = extract_graphql_operation_nodes(fixture, "client/BookClient.cs", module_id(), repo());
        let qnames: Vec<&String> = out.nav.qname_by_id.values().collect();
        assert_eq!(qnames, vec!["graphql_op:books"]);
        assert_eq!(anchor_line(&out, "graphql_op:books"), Some(7));
        assert_eq!(
            request_tally(fixture, "client/BookClient.cs").marker().as_deref(),
            Some("[graphql-ops] client-request machinebox=0 graphql-client=1 named=0 root_field=1 unread=0")
        );

        let forms = [
            // Verbatim, multi-line, after another member and a comment.
            "var r = new GraphQLRequest\n{\n    // Query = \"{ decoy }\"\n    OperationName = null,\n    Query = @\"\n        query {\n          books { title }\n        }\",\n    Variables = new { id = 1 },\n};\n",
            // A raw string constructor argument.
            "var r = new GraphQLHttpRequest(\"\"\"\n    { books { \"quoted\" title } }\n    \"\"\");\n",
            // `()` then an initializer, fully qualified.
            "var r = new GraphQL.GraphQLRequest() { Query = \"{ books { title } }\" };\n",
            // A named constructor argument.
            "var r = new GraphQLRequest(query: \"{\\n  books { title }\\n}\");\n",
            // The initializer's Query wins over the constructor argument.
            "var r = new GraphQLRequest(\"{ authors { name } }\") { Query = \"{ books { id } }\" };\n",
        ];
        for form in forms {
            let src = format!("using GraphQL.Client.Abstractions;\n{form}");
            assert_eq!(op_qnames_at(&src, "x.cs"), vec!["graphql_op:books"], "{form}");
        }
        let named = "using GraphQL.Client.Http;\nvar r = new GraphQLRequest { Query = @\"query BookShelf { books { title } }\" };\n";
        assert_eq!(op_qnames_at(named, "x.cs"), vec!["graphql_op:BookShelf"]);
    }

    #[test]
    fn first_root_field_skips_variables_aliases_and_directives() {
        for (doc, want) in [
            ("query { orders { id } }", Some("orders")),
            ("{ books { title } }", Some("books")),
            ("query ($id: ID!) { order(id: $id) { id } }", Some("order")),
            (
                "query Recent($n: [Int!] = [1, 2], $s: String = \"a)b\") @live @cached(ttl: 60) {\n  recent: orders(first: $n) { id }\n}",
                Some("orders"),
            ),
            ("# a comment\n\n  mutation { createOrder(input: {sku: \"x\"}) { id } }", Some("createOrder")),
            ("subscription @auth(role: \"x\") { orderShipped { id } }", Some("orderShipped")),
            ("{ __typename __schema { types { name } } shipments { id } }", Some("shipments")),
            ("{ __typename }", None),
            ("fragment F on Order { id }", None),
            ("{ ...F }", None),
            ("query {", None),
            ("{ 1abc }", None),
            ("queryX { orders }", None),
            ("", None),
        ] {
            assert_eq!(first_root_field(doc).as_deref(), want, "{doc:?}");
        }
    }

    /// The import gates are the language gate: no machinebox import, no
    /// `GraphQL.Client`, a blank import, a comment line or a Rust literal
    /// (CJ.1b's guard) mints nothing.
    #[test]
    fn a_request_without_the_import_mints_nothing() {
        let go_doc = "\treq := graphql.NewRequest(`query { orders { id } }`)\n";
        let cs_doc = "var r = new GraphQLRequest { Query = \"query { books { title } }\" };\n";
        let go_cases = [
            format!("import \"github.com/shurcooL/graphql\"\nfunc f() {{\n{go_doc}}}\n"),
            format!("import _ \"github.com/machinebox/graphql\"\nfunc f() {{\n{go_doc}}}\n"),
            format!("import \"github.com/machinebox/graphql\"\nfunc f() {{\n\t// {}\n}}\n", go_doc.trim()),
            "import \"github.com/machinebox/graphql\"\nfunc f() { r, _ := http.NewRequest(\"GET\", \"{ orders }\", nil) }\n".to_string(),
        ];
        let cs_cases = [
            format!("public class GraphQLRequest {{ public string Query; }}\n{cs_doc}"),
            format!("using GraphQL.ClientExtras;\n{cs_doc}"),
            "using GraphQL.Client.Http;\nvar r = new MyGraphQLRequest { Query = \"{ books }\" };\nvar t = typeof(GraphQLRequest);\n".to_string(),
        ];
        for src in go_cases.iter().chain(cs_cases.iter()) {
            assert!(op_qnames_at(src, "x.go").is_empty(), "{src}");
            let tally = request_tally(src, "x.go");
            assert_eq!(tally.machinebox + tally.graphql_client, 0, "{src}");
        }
        // The fixture's own text, held in a Rust literal and a comment.
        let rust = format!(
            "// import \"github.com/machinebox/graphql\" -> {}\nconst GO: &str = r#\"\nimport \"github.com/machinebox/graphql\"\n{go_doc}\"#;\nconst CS: &str = r#\"using GraphQL.Client.Http;\n{cs_doc}\"#;\n",
            go_doc.trim()
        );
        assert!(op_qnames_at(&rust, "scanner/src/samples.rs").is_empty());
        assert_eq!(op_qnames_at(&rust, "x.go"), vec!["graphql_op:books", "graphql_op:orders"]);
    }

    #[test]
    fn a_non_literal_document_mints_nothing() {
        let go = "import \"github.com/machinebox/graphql\"\nfunc f() {\n\ta := graphql.NewRequest(ordersQuery)\n\tb := graphql.NewRequest(\"query { \" + field + \" }\")\n\tc := graphql.NewRequest(fmt.Sprintf(`{ %s }`, f))\n\td := graphql.NewRequest(`query {`)\n}\n";
        assert!(op_qnames_at(go, "c.go").is_empty());
        let tally = request_tally(go, "c.go");
        assert_eq!((tally.machinebox, tally.unread), (4, 4));

        let cs = "using GraphQL.Client.Http;\nvar a = new GraphQLRequest { Query = BooksQuery };\nvar b = new GraphQLRequest(query);\nvar c = new GraphQLRequest { Query = $\"query {{ {field} }}\" };\nvar d = new GraphQLRequest { Query = \"{ books { title } }\" + suffix };\nvar e = new GraphQLRequest(\"{ books }\") { Query = Other };\nvar g = new GraphQLHttpRequest { OperationName = \"x\" };\n";
        assert!(op_qnames_at(cs, "x.cs").is_empty());
        let tally = request_tally(cs, "x.cs");
        assert_eq!(
            tally.marker().as_deref(),
            Some("[graphql-ops] client-request machinebox=0 graphql-client=6 named=0 root_field=0 unread=6")
        );
    }

    // ---- CL.6b: HotChocolate code-first root types ----

    /// bench/substrate-gap/matrix/csharp/graphql/server/Query.cs.
    const HC_QUERY: &str = "using HotChocolate;\n\npublic class Query\n{\n    public IEnumerable<Book> GetBooks() => new List<Book>();\n}\n\npublic record Book(string Title);\n";

    #[test]
    fn hotchocolate_query_class_fields() {
        assert_resolvers(HC_QUERY, "csharp", &["Query", "books"]);
        let out = extract_graphql_resolver_nodes(HC_QUERY, "csharp", module_id(), repo());
        assert_eq!(anchor_line(&out, "graphql_resolver:Query"), Some(2), "the class line");
        assert_eq!(anchor_line(&out, "graphql_resolver:books"), Some(4), "the method line");
        assert_eq!(
            decorator_scan(HC_QUERY, "csharp").1.marker("csharp").as_deref(),
            Some("[graphql-decorators] lang=csharp import=hotchocolate roots=Query fields=1 rejected lang=0 import=0 position=0")
        );
        // The source-generator shape: a `[QueryType]` static class whose
        // static methods and properties are the fields, a namespace block,
        // a multi-line signature, a block body holding braces in strings.
        let generated = "using HotChocolate.Types;\n\nnamespace Shop\n{\n    [QueryType]\n    public static partial class BookQueries\n    {\n        public static Book GetBookById(\n            int id,\n            [Service] IRepo repo) => repo.Find(id);\n\n        public static async Task<Author> GetAuthorAsync(int id)\n        {\n            var s = \"}\";\n            return new Author { Name = $\"{s}\" };\n        }\n\n        public static string Version { get; } = \"1\";\n\n        public static string Hello => \"world\";\n    }\n}\n";
        assert_resolvers(generated, "csharp", &["Query", "author", "bookById", "hello", "version"]);
        let out = extract_graphql_resolver_nodes(generated, "csharp", module_id(), repo());
        assert_eq!(anchor_line(&out, "graphql_resolver:Query"), Some(5));
        assert_eq!(anchor_line(&out, "graphql_resolver:bookById"), Some(7));
        assert_eq!(anchor_line(&out, "graphql_resolver:author"), Some(11));
        // Mutation / Subscription by name, an attribute on the class line, a
        // file-scoped namespace and `global using`.
        let roots = "global using HotChocolate;\nnamespace Shop;\npublic sealed class Mutation\n{\n    public Book AddBook(string title) => new(title);\n}\n[SubscriptionType] public class Events\n{\n    public Book OnBookAdded([EventMessage] Book book) => book;\n}\n";
        assert_resolvers(roots, "csharp", &["Mutation", "Subscription", "addBook", "onBookAdded"]);
    }

    #[test]
    fn extend_object_type_fields() {
        let source = "using HotChocolate;\nusing HotChocolate.Types;\n\n[ExtendObjectType(OperationTypeNames.Mutation)]\npublic class OrderMutations\n{\n    public Task<Order> CreateOrderAsync(OrderInput input) => _svc.Create(input);\n}\n";
        assert_resolvers(source, "csharp", &["Mutation", "createOrder"]);
        let out = extract_graphql_resolver_nodes(source, "csharp", module_id(), repo());
        assert_eq!(anchor_line(&out, "graphql_resolver:Mutation"), Some(4), "the class line, past the attribute");
        // Every spelling of the extended root, stacked attributes, and two
        // classes extending one root in a file: one noun, both fields.
        let spellings = "using HotChocolate.Types;\n[ExtendObjectType(\"Query\")]\npublic class A { public int GetA() => 1; }\n[Authorize]\n[ExtendObjectType(typeof(Query))]\npublic class B { public int GetB() => 2; }\n[ExtendObjectType<Subscription>]\npublic class C { public int GetC() => 3; }\n[ExtendObjectType(Name = \"Query\")]\npublic class D { public int GetD() => 4; }\n[HotChocolate.Types.ExtendObjectTypeAttribute(OperationTypeNames.Query)]\npublic class E { public int GetE() => 5; }\n";
        assert_resolvers(spellings, "csharp", &["Query", "Subscription", "a", "b", "c", "d", "e"]);
        // An object-type extension is no root, and neither is a class an
        // attribute block three lines up no longer reaches.
        let object_ext = "using HotChocolate.Types;\n[ExtendObjectType(typeof(Book))]\npublic class BookExtensions\n{\n    public Author GetAuthor([Parent] Book book) => null;\n}\n[QueryType]\npublic class Real { }\npublic class Next { public int GetNext() => 1; }\n";
        assert_resolvers(object_ext, "csharp", &["Query"]);
    }

    #[test]
    fn graphql_name_and_ignore() {
        let source = "using HotChocolate;\npublic class Query\n{\n    [GraphQLName(\"me\")]\n    public User GetCurrentUser() => null;\n    [Authorize]\n    [GraphQLNameAttribute(\"viewer\")]\n    [UsePaging]\n    public User GetWhoAmI() => null;\n    [GraphQLIgnore]\n    public string GetSecret() => \"\";\n    [GraphQLDescription(\"the [catalog]\")] public IQueryable<Book> GetBooks() => null;\n    [HotChocolate.GraphQLIgnore] public string Hidden { get; set; }\n}\n";
        assert_resolvers(source, "csharp", &["Query", "books", "me", "viewer"]);
        let out = extract_graphql_resolver_nodes(source, "csharp", module_id(), repo());
        assert_eq!(anchor_line(&out, "graphql_resolver:me"), Some(4), "the member line, not the attribute's");
        assert_eq!(anchor_line(&out, "graphql_resolver:viewer"), Some(8));
    }

    #[test]
    fn a_query_class_without_hotchocolate_mints_nothing() {
        let unimported = HC_QUERY.trim_start_matches("using HotChocolate;\n");
        assert_resolvers(unimported, "csharp", &[]);
        assert_eq!(
            decorator_scan(unimported, "csharp").1.marker("csharp").as_deref(),
            Some("[graphql-decorators] lang=csharp import=none roots= fields=0 rejected lang=0 import=1 position=0")
        );
        // A package that only starts with the name, an import in a comment.
        for using in ["using HotChocolateX;\n", "// using HotChocolate;\n", "using static Other.HotChocolate;\n"] {
            assert_resolvers(&format!("{using}{unimported}"), "csharp", &[]);
        }
        // A static import and an alias of the namespace are imports.
        for using in ["using static HotChocolate.Types.OperationTypeNames;\n", "using HC = HotChocolate.Types;\n"] {
            assert_resolvers(&format!("{using}{unimported}"), "csharp", &["Query", "books"]);
        }
        // The same C# held by a file of another language.
        assert_resolvers(HC_QUERY, "java", &[]);
        assert_resolvers(HC_QUERY, "typescript", &[]);
        // A root class in a comment or a string, and a longer class name.
        let decoys = "using HotChocolate;\n// public class Query { public int GetX() => 1; }\n/* [QueryType]\npublic class Mutation { public int GetY() => 1; } */\nvar s = @\"\npublic class Subscription { public int GetZ() => 1; }\";\npublic class QueryBuilder { public int GetW() => 1; }\n";
        assert_resolvers(decoys, "csharp", &[]);
        assert_eq!(
            decorator_scan(decoys, "csharp").1.marker("csharp").as_deref(),
            Some("[graphql-decorators] lang=csharp import=hotchocolate roots= fields=0 rejected lang=0 import=0 position=4")
        );
    }

    #[test]
    fn hc_field_name_rules() {
        for (member, method, field) in [
            ("GetBooks", true, "books"),
            ("GetUserByIdAsync", true, "userById"),
            ("Hello", true, "hello"),
            ("CreateOrderAsync", true, "createOrder"),
            ("OnBookAdded", true, "onBookAdded"),
            // `Get` before a lower-case letter is part of the word, and a
            // bare `Get` / `Async` keeps its text.
            ("Getaway", true, "getaway"),
            ("Get", true, "get"),
            ("Async", true, "async"),
            // FormatFieldName: the upper-case run keeps its last letter when
            // a letter follows it.
            ("GetURLPath", true, "urlPath"),
            ("ID", false, "id"),
            ("HTTP2Server", false, "http2Server"),
            ("IDs", false, "iDs"),
            ("already", true, "already"),
            // A property keeps its `Get` and `Async`.
            ("GetCount", false, "getCount"),
            ("LoadAsync", false, "loadAsync"),
        ] {
            assert_eq!(hc_field_name(member, method).as_deref(), Some(field), "{member}");
        }
        assert_eq!(hc_field_name("", true), None);
    }

    #[test]
    fn constructors_and_private_members_are_not_fields() {
        let source = "using HotChocolate;\n\npublic class Query\n{\n    #region Don't expose these {\n    private readonly IRepo _repo;\n    public const int Limit = 10;\n    #endregion\n    public Query(IRepo repo) { _repo = repo; }\n    public Query() : this(null) { }\n    ~Query() { }\n    private Book GetHidden() => null;\n    internal Book GetInternal() => null;\n    protected Book GetProtected() => null;\n    public static Book GetStatic() => null;\n    public void Log(string m) { }\n    public Task FlushAsync() => Task.CompletedTask;\n    public override string ToString() => \"Query\";\n    public string Name;\n    public Book this[int i] => null;\n    public event EventHandler Changed;\n    public class Nested { public int GetInner() => 1; }\n    public int Count { get; private set; }\n    public Book GetBook(int id)\n    {\n        // }\n        var verbatim = @\"}\"\"{\";\n        char c = '}';\n        if (id > 0) { return _repo.Find(id); }\n        return null;\n    }\n}\n\npublic class Other\n{\n    public int GetOther() => 1;\n}\n";
        assert_resolvers(source, "csharp", &["Query", "book", "count"]);
    }

    // ---- DG.4: async-graphql (Rust) and Caliban (Scala) roots ----

    /// bench/substrate-gap/matrix/rust/graphql/server/src/schema.rs.
    const AG_QUERY: &str = "use async_graphql::{Object, SimpleObject};\n\n#[derive(SimpleObject)]\npub struct Order {\n    id: String,\n}\n\npub struct QueryRoot;\n\n#[Object]\nimpl QueryRoot {\n    async fn orders(&self) -> Vec<Order> {\n        vec![]\n    }\n}\n";

    /// bench/substrate-gap/matrix/scala/graphql/server/Api.scala.
    const CALIBAN_API: &str = "package shop\n\nimport caliban.graphQL\nimport caliban.RootResolver\n\ncase class Order(id: String)\ncase class Queries(orders: List[Order])\n\nobject Api {\n  val api = graphQL(RootResolver(Queries(orders = Nil)))\n}\n";

    #[test]
    fn async_graphql_query_root_fields() {
        assert_resolvers(AG_QUERY, "rust", &["Query", "orders"]);
        let out = extract_graphql_resolver_nodes(AG_QUERY, "rust", module_id(), repo());
        assert_eq!(anchor_line(&out, "graphql_resolver:Query"), Some(10), "the impl line");
        assert_eq!(anchor_line(&out, "graphql_resolver:orders"), Some(11), "the fn line");
        assert_eq!(
            decorator_scan(AG_QUERY, "rust").1.marker("rust").as_deref(),
            Some("[graphql-decorators] lang=rust import=async-graphql roots=Query fields=1 rejected lang=0 import=0 position=0")
        );
        // Mutation / Subscription by name, a crate-qualified attribute, a
        // `use async_graphql;` import and generic impls.
        let roots = "use async_graphql;\n\n#[async_graphql::Object]\nimpl<'a> MutationRoot<'a> {\n    async fn add_order(&self, ctx: &async_graphql::Context<'_>) -> i32 { 1 }\n}\n#[Object(name = \"Q\")]\nimpl RootQuery {\n    pub async fn me(&self) -> i32 { 1 }\n}\n#[Object]\nimpl SubscriptionRoot {\n    async fn ticks(&self) -> i32 { 1 }\n}\n";
        assert_resolvers(roots, "rust", &["Mutation", "Query", "Subscription", "addOrder", "me", "ticks"]);
    }

    #[test]
    fn async_graphql_schema_names_the_roots() {
        // A schema in the file names the roots: `Q` and `M` take theirs by
        // position, and `LegacyQuery`, named by no schema, is no root.
        let named = "use async_graphql::{EmptySubscription, Object, Schema};\n\npub struct Q;\npub struct M;\n\n#[Object]\nimpl Q {\n    async fn orders(&self) -> i32 { 1 }\n}\n\n#[Object]\nimpl M {\n    async fn place_order(&self, id: i32) -> i32 { id }\n}\n\n#[Object]\nimpl LegacyQuery {\n    async fn legacy(&self) -> i32 { 0 }\n}\n\npub fn schema() -> Schema<Q, M, EmptySubscription> {\n    Schema::new(Q, M, EmptySubscription)\n}\n";
        assert_resolvers(named, "rust", &["Mutation", "Query", "orders", "placeOrder"]);
        // `Schema::build` with a struct-literal value and a qualified path.
        let built = "use async_graphql::{EmptyMutation, EmptySubscription, Object};\npub struct Root { db: Db }\n#[Object]\nimpl Root { async fn version(&self) -> &str { \"1\" } }\nfn main() {\n    let a = async_graphql::Schema::build(Root { db: Db::new() }, EmptyMutation, EmptySubscription).finish();\n    let b = Schema::build(crate::api::Root::default(), EmptyMutation, EmptySubscription);\n}\n";
        assert_resolvers(built, "rust", &["Query", "version"]);
        // A merged root: its parts take its root, whatever their names.
        let merged = "use async_graphql::*;\n#[derive(MergedObject, Default)]\npub struct Query(Users, shop::Orders);\n#[derive(Default)] pub struct Users;\n#[Object] impl Users { async fn users(&self) -> i32 { 1 } }\n#[Object] impl Orders { async fn orders(&self) -> i32 { 2 } }\n#[Object] impl Stray { async fn stray(&self) -> i32 { 3 } }\npub type AppSchema = Schema<Query, EmptyMutation, EmptySubscription>;\n";
        assert_resolvers(merged, "rust", &["Query", "orders", "users"]);
        let out = extract_graphql_resolver_nodes(merged, "rust", module_id(), repo());
        assert_eq!(anchor_line(&out, "graphql_resolver:Query"), Some(4), "the first root impl");
        // A call that is no async-graphql schema (another crate's
        // one-argument `Schema::new`) leaves the roots to their names.
        let arrow = "use async_graphql::Object;\nfn s() { let x = arrow::Schema::new(vec![]); }\n#[Object]\nimpl QueryRoot { async fn orders(&self) -> i32 { 1 } }\n";
        assert_resolvers(arrow, "rust", &["Query", "orders"]);
    }

    #[test]
    fn async_graphql_rename_skip_and_camel_case() {
        let source = "use async_graphql::{Context, Object, Subscription, ID};\nuse futures_util::Stream;\n\npub struct QueryRoot;\n\n#[Object]\nimpl QueryRoot {\n    /// All orders; a `}` in a doc comment.\n    async fn order_count(&self, ctx: &Context<'_>) -> usize {\n        let s = \"}\";\n        let c = '}';\n        s.len()\n    }\n    #[graphql(name = \"allOrders\")]\n    async fn orders(&self) -> Vec<i32> { vec![] }\n    #[graphql(skip)]\n    fn helper(&self) -> i32 { 1 }\n    #[graphql(entity)]\n    async fn find_order_by_id(&self, id: ID) -> i32 { 0 }\n    #[graphql(flatten)]\n    async fn common(&self) -> Common { Common }\n    #[graphql(skip = true)]\n    fn other(&self) -> i32 { 1 }\n    async fn r#type(&self) -> i32 { 0 }\n    async fn _internal(&self) -> i32 { 0 }\n    pub(crate) fn user_ID(&self) -> i32 { 0 }\n}\n\n#[async_graphql::Object(rename_fields = \"snake_case\")]\nimpl MutationRoot {\n    async fn placeOrder(&self) -> i32 { 0 }\n}\n\npub struct Events;\n\n#[Subscription]\nimpl Events {\n    async fn order_created<'a>(&'a self) -> impl Stream<Item = i32> + 'a { futures_util::stream::empty() }\n}\n";
        assert_resolvers(
            source,
            "rust",
            &["Mutation", "Query", "Subscription", "allOrders", "internal", "orderCount", "orderCreated", "place_order", "type", "userId"],
        );
        let out = extract_graphql_resolver_nodes(source, "rust", module_id(), repo());
        assert_eq!(anchor_line(&out, "graphql_resolver:orderCount"), Some(8), "the fn line, past its doc comment");
        assert_eq!(anchor_line(&out, "graphql_resolver:allOrders"), Some(14), "the fn line, past its attribute");
        assert_eq!(anchor_line(&out, "graphql_resolver:Subscription"), Some(36));
    }

    #[test]
    fn async_graphql_rename_rules() {
        for (name, field) in [
            ("order_count", "orderCount"),
            ("orders", "orders"),
            ("_internal", "internal"),
            ("user_ID", "userId"),
            ("field_2", "field2"),
            ("getHTTPResponse", "getHttpResponse"),
            ("a__b", "aB"),
            ("placeOrder", "placeOrder"),
        ] {
            assert_eq!(RenameRule::Camel.apply(name), field, "{name}");
        }
        assert_eq!(RenameRule::Pascal.apply("order_count"), "OrderCount");
        assert_eq!(RenameRule::Snake.apply("placeOrder"), "place_order");
        assert_eq!(RenameRule::ScreamingSnake.apply("placeOrder"), "PLACE_ORDER");
        assert_eq!(RenameRule::Lower.apply("placeOrder"), "placeorder");
        assert_eq!(RenameRule::Upper.apply("place_order"), "PLACE_ORDER");
        assert_eq!(RenameRule::parse("SCREAMING_SNAKE_CASE"), Some(RenameRule::ScreamingSnake));
        assert_eq!(RenameRule::parse("kebab-case"), None);
    }

    #[test]
    fn async_graphql_non_root_object_has_no_resolvers() {
        let source = "use async_graphql::{ComplexObject, Object, SimpleObject};\n#[derive(SimpleObject)]\n#[graphql(complex)]\npub struct Order { id: i32 }\n#[ComplexObject]\nimpl Order { async fn total(&self) -> i32 { 0 } }\n#[Object]\nimpl Customer { async fn orders(&self) -> Vec<Order> { vec![] } }\nimpl QueryRoot { async fn hidden(&self) -> i32 { 0 } }\n#[Object]\nimpl Resolver for QueryRoot { async fn x(&self) -> i32 { 0 } }\n";
        assert_resolvers(source, "rust", &[]);
        assert_eq!(
            decorator_scan(source, "rust").1.marker("rust").as_deref(),
            Some("[graphql-decorators] lang=rust import=async-graphql roots= fields=0 rejected lang=0 import=0 position=0")
        );
        // The same file of another language, and juniper's attribute.
        assert_resolvers(AG_QUERY, "go", &[]);
        let juniper = "use juniper::graphql_object;\npub struct Query;\n#[graphql_object]\nimpl Query { fn orders() -> i32 { 1 } }\n";
        assert_resolvers(juniper, "rust", &[]);
    }

    #[test]
    fn async_graphql_sample_in_a_rust_string_is_no_site() {
        // The whole sample in a string, a raw string or a doc comment: no
        // import at code position.
        for sample in [
            "const SAMPLE: &str = \"use async_graphql::Object;\\n#[Object]\\nimpl QueryRoot {\\n    async fn orders(&self) -> i32 { 1 }\\n}\\n\";\n",
            "const SAMPLE: &str = r#\"use async_graphql::Object;\n#[Object]\nimpl QueryRoot {\n    async fn orders(&self) -> i32 { 1 }\n}\n\"#;\n",
            "/// use async_graphql::Object;\n/// #[Object]\n/// impl QueryRoot { async fn orders(&self) -> i32 { 1 } }\npub fn f() {}\n",
        ] {
            assert_resolvers(sample, "rust", &[]);
            assert_eq!(
                decorator_scan(sample, "rust").1.marker("rust").as_deref(),
                Some("[graphql-decorators] lang=rust import=none roots= fields=0 rejected lang=0 import=1 position=0"),
                "{sample}"
            );
        }
        // A real import, the root impls in comments and a raw string.
        let decoys = "use async_graphql::Object;\n// #[Object]\n// impl QueryRoot { async fn orders(&self) -> i32 { 1 } }\n/// ```\n/// #[Object]\n/// impl MutationRoot { async fn add(&self) -> i32 { 1 } }\n/// ```\npub fn f() {}\npub const S: &str = r#\"#[async_graphql::Subscription]\nimpl Events { async fn ticks(&self) -> i32 { 1 } }\"#;\n";
        assert_resolvers(decoys, "rust", &[]);
        assert_eq!(
            decorator_scan(decoys, "rust").1.marker("rust").as_deref(),
            Some("[graphql-decorators] lang=rust import=async-graphql roots= fields=0 rejected lang=0 import=0 position=3")
        );
        // Another crate that only starts with the name; `extern crate` names it.
        let axum = "use async_graphql_axum::GraphQL;\n#[Object]\nimpl QueryRoot { async fn orders(&self) -> i32 { 1 } }\n";
        assert_resolvers(axum, "rust", &[]);
        let extern_crate = "extern crate async_graphql;\n#[Object]\nimpl QueryRoot { async fn orders(&self) -> i32 { 1 } }\n";
        assert_resolvers(extern_crate, "rust", &["Query", "orders"]);
    }

    #[test]
    fn caliban_root_resolver_case_class() {
        assert_resolvers(CALIBAN_API, "scala", &["Query", "orders"]);
        let out = extract_graphql_resolver_nodes(CALIBAN_API, "scala", module_id(), repo());
        assert_eq!(anchor_line(&out, "graphql_resolver:Query"), Some(6), "the case class line");
        assert_eq!(anchor_line(&out, "graphql_resolver:orders"), Some(6));
        assert_eq!(
            decorator_scan(CALIBAN_API, "scala").1.marker("scala").as_deref(),
            Some("[graphql-decorators] lang=scala import=caliban roots=Query fields=1 rejected lang=0 import=0 position=0")
        );
        // Queries and Mutations case classes through `val`s, a multi-line
        // parameter list, `@GQLName` / `@GQLExcluded`, a back-quoted name.
        let both = "import caliban._\nimport caliban.schema.Annotations.{GQLExcluded, GQLName}\n\ncase class Queries(\n  orders: List[Order],\n  @GQLName(\"customer\") user: Long => Option[User],\n  @GQLExcluded secret: String\n)\ncase class Mutations(placeOrder: OrderInput => Order, `cancel`: Long => Boolean)\n\nobject Api {\n  val queries = Queries(Nil, _ => None, \"\")\n  val mutations: Mutations = Mutations(_ => ???, _ => true)\n  val api = graphQL(RootResolver(queries, mutations))\n}\n";
        assert_resolvers(both, "scala", &["Mutation", "Query", "cancel", "customer", "orders", "placeOrder"]);
        let out = extract_graphql_resolver_nodes(both, "scala", module_id(), repo());
        assert_eq!(anchor_line(&out, "graphql_resolver:customer"), Some(5));
        // Types declared in another file name their roots only; `None`
        // names nothing; `Some(..)` / `Option(..)` hold the resolver.
        let elsewhere = "import caliban.{graphQL, RootResolver}\nobject Api {\n  val api = graphQL(\n    RootResolver(Some(Queries(Nil)), None, Option(Subscriptions(stream)))\n  )\n}\n";
        assert_resolvers(elsewhere, "scala", &["Query", "Subscription"]);
        let out = extract_graphql_resolver_nodes(elsewhere, "scala", module_id(), repo());
        assert_eq!(anchor_line(&out, "graphql_resolver:Query"), Some(3), "the argument line");
        // Named arguments pick their root.
        let named = "import caliban.RootResolver\ncase class M(add: Int => Int)\nval api = graphQL(RootResolver(mutationResolver = M(identity), queryResolver = Q()))\n";
        assert_resolvers(named, "scala", &["Mutation", "Query", "add"]);
    }

    #[test]
    fn caliban_needs_the_import() {
        let unimported = CALIBAN_API.replace("import caliban.graphQL\nimport caliban.RootResolver\n", "");
        assert_resolvers(&unimported, "scala", &[]);
        assert_eq!(
            decorator_scan(&unimported, "scala").1.marker("scala").as_deref(),
            Some("[graphql-decorators] lang=scala import=none roots= fields=0 rejected lang=0 import=1 position=0")
        );
        assert_resolvers(&format!("import calibanx.RootResolver\n{unimported}"), "scala", &[]);
        assert_resolvers(&format!("// import caliban.RootResolver\n{unimported}"), "scala", &[]);
        // The same file of another language.
        assert_resolvers(CALIBAN_API, "java", &[]);
        assert_resolvers(CALIBAN_API, "kotlin", &[]);
        // A RootResolver call in a comment or a string is no site.
        let decoys = "import caliban._\ncase class Queries(orders: List[Order])\n// graphQL(RootResolver(Queries(Nil)))\n/* RootResolver(Mutations(Nil)) */\nval doc = \"\"\"RootResolver(Queries(Nil))\"\"\"\n";
        assert_resolvers(decoys, "scala", &[]);
        assert_eq!(
            decorator_scan(decoys, "scala").1.marker("scala").as_deref(),
            Some("[graphql-decorators] lang=scala import=caliban roots= fields=0 rejected lang=0 import=0 position=3")
        );
    }

    // ---- DH.8: graphql-ruby, Absinthe, Dart package:graphql ----

    /// bench/substrate-gap/matrix/ruby/graphql/server/app/graphql/types/query_type.rb.
    const GRAPHQL_RUBY_QUERY_TYPE: &str = "module Types\n  class QueryType < Types::BaseObject\n    field :orders, [Types::OrderType], null: false\n\n    def orders\n      Order.all\n    end\n  end\nend\n";

    /// bench/substrate-gap/matrix/elixir/graphql/server/lib/schema.ex.
    const ABSINTHE_SCHEMA: &str = "defmodule ShopWeb.Schema do\n  use Absinthe.Schema\n\n  object :order do\n    field :id, :id\n  end\n\n  query do\n    field :orders, list_of(:order) do\n      resolve fn _, _, _ -> {:ok, []} end\n    end\n  end\nend\n";

    /// bench/substrate-gap/matrix/dart/graphql/client/lib/orders.dart.
    const DART_ORDERS: &str = "import 'package:graphql/client.dart';\n\nconst listOrders = r\"\"\"\n  query Orders {\n    orders {\n      id\n    }\n  }\n\"\"\";\n\nFuture<QueryResult> fetchOrders(GraphQLClient client) {\n  return client.query(QueryOptions(document: gql(listOrders)));\n}\n";

    #[test]
    fn graphql_ruby_query_type_fields() {
        assert_resolvers(GRAPHQL_RUBY_QUERY_TYPE, "ruby", &["Query", "orders"]);
        let out = extract_graphql_resolver_nodes(GRAPHQL_RUBY_QUERY_TYPE, "ruby", module_id(), repo());
        assert_eq!(anchor_line(&out, "graphql_resolver:Query"), Some(1), "the class line");
        assert_eq!(anchor_line(&out, "graphql_resolver:orders"), Some(4), "the type's `def orders`");
        assert_eq!(
            decorator_scan(GRAPHQL_RUBY_QUERY_TYPE, "ruby").1.marker("ruby").as_deref(),
            Some("[graphql-decorators] lang=ruby import=graphql-ruby roots=Query fields=1 rejected lang=0 import=0 position=0")
        );
        // The compact class path, an unqualified base class, the root class
        // named exactly `Subscription`, and `GraphQL::Schema::Object` itself.
        let compact = "class Types::MutationType < BaseObject\n  field :create_order, mutation: Mutations::CreateOrder\nend\nclass Subscription < GraphQL::Schema::Object\n  field(:order_placed, Types::OrderType)\nend\n";
        assert_resolvers(compact, "ruby", &["Mutation", "Subscription", "createOrder", "orderPlaced"]);
        // The same file of another language.
        assert_resolvers(GRAPHQL_RUBY_QUERY_TYPE, "python", &[]);
        assert_resolvers(GRAPHQL_RUBY_QUERY_TYPE, "elixir", &[]);
    }

    #[test]
    fn graphql_ruby_camelizes_and_honours_camelize_false() {
        let src = "class Types::QueryType < GraphQL::Schema::Object\n  field :user_orders, [Types::OrderType], null: false\n  field :raw_id, ID, null: false, camelize: false\n  field \"named_by_string\", String\n  field :order, Types::OrderType, null: true do\n    argument :order_id, ID, required: true\n  end\n  field :legacy, String,\n    camelize: false\n  field :by_method, String, resolver_method: :resolve_by_method\n\n  def resolve_by_method\n    field :not_a_field, String\n  end\n\n  def self.helper\n    field :nor_this, String\n  end\nend\n";
        assert_resolvers(src, "ruby", &["Query", "byMethod", "legacy", "namedByString", "order", "raw_id", "userOrders"]);
        let out = extract_graphql_resolver_nodes(src, "ruby", module_id(), repo());
        assert_eq!(anchor_line(&out, "graphql_resolver:byMethod"), Some(11), "its resolver_method's def");
        assert_eq!(anchor_line(&out, "graphql_resolver:userOrders"), Some(1), "no def: the field line");
        for (ruby, schema) in [
            ("orders", "orders"),
            ("user_orders", "userOrders"),
            ("user_ID", "userId"),
            ("HTTP_status", "httpStatus"),
            ("field_2", "field2"),
            ("a__b", "aB"),
            ("_private", "_private"),
            ("_", "_"),
        ] {
            assert_eq!(ruby_camelize(ruby), schema, "{ruby}");
        }
    }

    #[test]
    fn graphql_ruby_non_root_object_type_is_no_resolver() {
        let object = "module Types\n  class OrderType < Types::BaseObject\n    field :id, ID, null: false\n  end\nend\n";
        assert_resolvers(object, "ruby", &[]);
        assert_eq!(decorator_scan(object, "ruby").1.marker("ruby"), None, "no root-named class, no census line");
        // A root-named class that is no GraphQL object, in a file with no
        // graphql-ruby marker.
        let model = "class QueryType < ApplicationRecord\n  field :orders\nend\n";
        assert_resolvers(model, "ruby", &[]);
        assert_eq!(
            decorator_scan(model, "ruby").1.marker("ruby").as_deref(),
            Some("[graphql-decorators] lang=ruby import=none roots= fields=0 rejected lang=0 import=1 position=0")
        );
        // Declarations in a comment, a heredoc and a `=begin` block are no
        // roots; a nested class's body is hidden, and the fields after it
        // are still the root's.
        let decoys = "# class QueryType < Types::BaseObject\nDOC = <<~RUBY\n  class MutationType < Types::BaseObject\n    field :fake, String\n  end\nRUBY\n=begin\nclass SubscriptionType < Types::BaseObject\n=end\nclass QueryType < Types::BaseObject\n  class Inner < Types::BaseObject\n    field :inner, String\n  end\n  field :after_inner, String # field :in_comment\nend\n";
        assert_resolvers(decoys, "ruby", &["Query", "afterInner"]);
        assert_eq!(
            decorator_scan(decoys, "ruby").1.marker("ruby").as_deref(),
            Some("[graphql-decorators] lang=ruby import=graphql-ruby roots=Query fields=1 rejected lang=0 import=0 position=3")
        );
    }

    #[test]
    fn absinthe_query_block_fields() {
        assert_resolvers(ABSINTHE_SCHEMA, "elixir", &["Query", "orders"]);
        let out = extract_graphql_resolver_nodes(ABSINTHE_SCHEMA, "elixir", module_id(), repo());
        assert_eq!(anchor_line(&out, "graphql_resolver:Query"), Some(7), "the `query do` line");
        assert_eq!(anchor_line(&out, "graphql_resolver:orders"), Some(8));
        assert_eq!(
            decorator_scan(ABSINTHE_SCHEMA, "elixir").1.marker("elixir").as_deref(),
            Some("[graphql-decorators] lang=elixir import=absinthe roots=Query fields=1 rejected lang=0 import=0 position=0")
        );
        // A type module (`use Absinthe.Schema.Notation`) is no schema, and
        // its `object` blocks are no roots.
        let notation = "defmodule ShopWeb.Schema.OrderQueries do\n  use Absinthe.Schema.Notation\n\n  object :order_queries do\n    field :orders, list_of(:order)\n  end\nend\n";
        assert_resolvers(notation, "elixir", &[]);
        assert_resolvers(ABSINTHE_SCHEMA, "ruby", &[]);
    }

    #[test]
    fn absinthe_mutation_and_nested_do_blocks() {
        let src = "defmodule ShopWeb.Schema do\n  use Absinthe.Schema\n  import_types ShopWeb.Schema.Types\n\n  @desc \"\"\"\n  query do\n    field :in_doc, :string\n  end\n  \"\"\"\n  # query do field :commented, :string end\n\n  query name: \"RootQuery\" do\n    @desc \"Orders by user\"\n    field :user_orders, list_of(:order) do\n      arg :user_id, non_null(:id)\n      resolve fn _, %{user_id: id}, _ ->\n        case Shop.orders(id) do\n          {:ok, o} -> {:ok, o}\n          err -> err\n        end\n      end\n    end\n    field :ping, :string, resolve: fn _, _, _ -> {:ok, \"pong\"} end\n    field(:field_2, :string)\n    import_fields :order_queries\n  end\n\n  mutation do\n    field :place_order, :order do\n      arg :input, non_null(:order_input)\n      resolve &Resolvers.place_order/3\n    end\n  end\n\n  subscription do\n    field :order_placed, :order do\n      config fn _, _ -> {:ok, topic: \"*\"} end\n    end\n  end\n\n  object :order do\n    field :id, :id\n  end\nend\n";
        assert_resolvers(
            src,
            "elixir",
            &["Mutation", "Query", "Subscription", "field2", "orderPlaced", "ping", "placeOrder", "userOrders"],
        );
        assert_eq!(
            decorator_scan(src, "elixir").1.marker("elixir").as_deref(),
            Some("[graphql-decorators] lang=elixir import=absinthe roots=Query,Mutation,Subscription fields=5 rejected lang=0 import=0 position=2")
        );
        // An Elixir module that is no Absinthe schema.
        let plain = "defmodule Report do\n  def run(query) do\n    query |> Repo.all()\n  end\nend\n";
        assert_resolvers(plain, "elixir", &[]);
        assert_eq!(decorator_scan(plain, "elixir").1.marker("elixir"), None);
        for (elixir, schema) in [
            ("orders", "orders"),
            ("user_orders", "userOrders"),
            ("user_ID", "user_ID"),
            ("field_2", "field2"),
            ("a__b", "aB"),
            ("_private", "_private"),
            ("__typename_x", "__typenameX"),
            ("Orders_x", "OrdersX"),
        ] {
            assert_eq!(absinthe_external_name(elixir), schema, "{elixir}");
        }
    }

    #[test]
    fn elixir_mask_blanks_literals_comments_sigils_and_chars() {
        let src = "a = \"query do\" # end\nb = ~s(end) <> ~S\"\"\"\nend\n\"\"\" <> 'do'\nc = ?# <> ?\" <> valid?(x)\n";
        let m = elixir_code_mask(src);
        assert_eq!(m.len(), src.len());
        let code = String::from_utf8_lossy(&m).split_whitespace().collect::<Vec<_>>().join(" ");
        assert_eq!(code, "a = b = <> <> c = <> <> valid?(x)");
    }

    #[test]
    fn dart_gql_const_document_names_the_op() {
        assert_eq!(op_qnames(DART_ORDERS), ["graphql_op:Orders"]);
        let (hits, tally) = scan_operation_needles(DART_ORDERS, &mut LazyGuard::new("", DART_ORDERS));
        let named: Vec<(&str, u32)> = hits.iter().map(|(n, l, _)| (n.as_str(), *l)).collect();
        assert_eq!(named, [("Orders", 11)], "minted at the needle line");
        assert_eq!(
            tally.marker().as_deref(),
            Some("[graphql-ops] kept=1 rejected=0 hook=0 client=0 request=0 context=gql-tag")
        );
        assert_eq!(tally.dart_marker().as_deref(), Some("[graphql-ops] dart-document const=1 inline=0 unread=0"));
        // graphql_flutter, a multi-line call, a typed static constant
        // declared after its use.
        let flutter = "import \"package:graphql_flutter/graphql_flutter.dart\";\n\nclass OrdersApi {\n  Future<void> place(GraphQLClient client, String id) async {\n    final result = await client.mutate(\n      MutationOptions(\n        document: gql(placeOrderMutation),\n        variables: {'id': id},\n      ),\n    );\n  }\n\n  static const String placeOrderMutation = '''\n    mutation PlaceOrder($id: ID!) { placeOrder(id: $id) { id } }\n  ''';\n}\n";
        assert_eq!(op_qnames(flutter), ["graphql_op:PlaceOrder"]);
        // A constant declared only in a comment names nothing: the fallback.
        let commented = "import 'package:graphql/client.dart';\n// const listOrders = r'query Orders { orders { id } }';\nFuture<QueryResult> f(GraphQLClient client) => client.query(QueryOptions(document: gql(listOrders)));\n";
        assert_eq!(op_qnames(commented), ["graphql_op:client.query"]);
        let (_, tally) = scan_operation_needles(commented, &mut LazyGuard::new("", commented));
        assert_eq!(tally.dart_marker().as_deref(), Some("[graphql-ops] dart-document const=0 inline=0 unread=1"));
        // Without the package:graphql import the call keeps its fallback name.
        let unimported = DART_ORDERS.replace("import 'package:graphql/client.dart';\n", "");
        assert_eq!(op_qnames(&unimported), ["graphql_op:client.query"]);
        let (_, tally) = scan_operation_needles(&unimported, &mut LazyGuard::new("", &unimported));
        assert_eq!(tally.dart_marker(), None);
    }

    #[test]
    fn dart_inline_gql_literal() {
        let inline = "import 'package:graphql/client.dart';\n\nFuture<QueryResult> fetch(GraphQLClient client) {\n  return client.query(QueryOptions(document: gql(r'''\n    query Inline { orders { id } }\n  ''')));\n}\n";
        assert_eq!(op_qnames(inline), ["graphql_op:Inline"]);
        let (_, tally) = scan_operation_needles(inline, &mut LazyGuard::new("", inline));
        assert_eq!(tally.dart_marker().as_deref(), Some("[graphql-ops] dart-document const=0 inline=1 unread=0"));
        // Two calls, each named by its own document.
        let two = "import 'package:graphql/client.dart';\nconst a = \"query First { a }\";\nfinal f1 = client.query(QueryOptions(document: gql(a)));\nfinal f2 = client.query(QueryOptions(document: gql(\"query Second { b }\")));\n";
        assert_eq!(op_qnames(two), ["graphql_op:First", "graphql_op:Second"]);
        // An anonymous document names nothing.
        let anonymous = "import 'package:graphql/client.dart';\nfinal f = client.query(QueryOptions(document: gql(r'query { orders { id } }')));\n";
        assert_eq!(op_qnames(anonymous), ["graphql_op:client.query"]);
    }

    #[test]
    fn ts_fallback_naming_unchanged() {
        // The Dart shape in an Apollo TS file: the call's document is a
        // constant passed to `gql(..)`, which a TS file is not read for.
        let ts = "import { ApolloClient, gql } from '@apollo/client';\nconst listOrders = `query Orders { orders { id } }`;\nexport const load = () => client.query({ query: gql(listOrders) });\n";
        assert_eq!(op_qnames(ts), ["graphql_op:client.query"]);
        let (_, tally) = scan_operation_needles(ts, &mut LazyGuard::new("", ts));
        assert_eq!(tally.dart_marker(), None);
        // A template-tagged document still names the call.
        let tagged = "import { gql } from '@apollo/client';\nconst ORDERS = gql`query Orders { orders { id } }`;\nclient.query({ query: ORDERS });\n";
        assert_eq!(op_qnames(tagged), ["graphql_op:Orders"]);
    }
    /// DT.11a: the CODE text of the operation qname `qname` in `out`.
    fn op_code<'a>(out: &'a GraphqlNodes, qname: &str) -> Vec<&'a str> {
        let id = out
            .nav
            .qname_by_id
            .iter()
            .find(|(_, q)| q.as_str() == qname)
            .map(|(id, _)| *id)
            .unwrap_or_else(|| panic!("no {qname}: {:?}", out.nav.qname_by_id.values().collect::<Vec<_>>()));
        out.nodes
            .iter()
            .filter(|n| n.id == id)
            .flat_map(|n| &n.cells)
            .filter(|c| c.kind == cell_type::CODE)
            .map(|c| match &c.payload {
                CellPayload::Text(t) => t.as_str(),
                other => panic!("CODE is text: {other:?}"),
            })
            .collect()
    }

    /// DT.11a: the root fields in order and deduped, the field behind an
    /// alias, an inline fragment's fields with or without a type condition,
    /// no spread, no introspection field; a fragment definition, SDL and a
    /// non-document give none, a document that stops parsing gives what came
    /// before.
    #[test]
    fn operation_root_fields_reads_aliases_inline_fragments_and_skips_typename() {
        for (doc, want) in [
            ("query Dashboard { me { id } recent: orders(last: 5) { id } }", &["me", "orders"][..]),
            ("\n  query StockList($first: Int) {\n    products(first: $first) { id name }\n  }\n", &["products"][..]),
            ("query Products { __typename me { id } }", &["me"][..]),
            ("{ a: me { id } b: me { name } orders }", &["me", "orders"][..]),
            ("query Q @live { ... on Query { users { id } } ... @include(if: $x) { stats } ...Rest me }", &["users", "stats", "me"][..]),
            ("query { ...OnlySpreads ...onUser }", &[][..]),
            ("mutation M($id: ID!) { restock(id: $id, input: {qty: 2, note: \"}\"}) @auth { id } }", &["restock"][..]),
            ("# c\n{ orders(last: 1) { id } }", &["orders"][..]),
            ("fragment F on Query { users }", &[][..]),
            ("type Query { products: [Product] }", &[][..]),
            ("schema { query: Query }", &[][..]),
            ("{ orders { id } ${MORE} }", &["orders"][..]),
            ("{ orders(", &["orders"][..]),
            ("", &[][..]),
        ] {
            assert_eq!(operation_root_fields(doc), want, "{doc:?}");
        }
    }

    /// DT.11a: an anonymous `` gql` `` document is named by its first root
    /// field (CL.6a's client-request rule), a named one keeps its name, and
    /// a template that never closes names nothing it would not at HEAD.
    #[test]
    fn anonymous_template_is_named_by_first_root_field() {
        let src = "import { gql } from '@apollo/client/core';\nexport const RECENT = gql`\n  {\n    orders(last: 1) { id }\n  }\n`;\nexport const SHIPPED = gql`query ($n: Int) { __typename shipments(first: $n) { id } }`;\nexport const NAMED = gql`query Recent { orders { id } }`;\n";
        assert_eq!(op_qnames(src), ["graphql_op:Recent", "graphql_op:orders", "graphql_op:shipments"]);
        let open = "const X = gql`\n  { orders { id } }\n";
        assert!(op_qnames(open).is_empty(), "an unclosed anonymous template is no document");
        let named = "const X = gql`query\n  Named { orders { id } }`;\n";
        assert!(op_qnames(named).is_empty(), "a named document is never named by its root field");
        let open_named = "const X = gql`query Open { orders { id } }\n";
        assert_eq!(op_qnames(open_named), ["graphql_op:Open"], "HEAD's name rule");
        assert!(op_code(&extract_graphql_operation_nodes(open_named, "", module_id(), repo()), "graphql_op:Open").is_empty());
    }

    /// DT.11a: a template operation carries its document, the untrimmed
    /// body between the backticks, so it starts on the tag's line.
    #[test]
    fn template_op_carries_its_document() {
        let src = "export const STOCK_LIST = gql`\n  query StockList($first: Int) {\n    products(first: $first) { id sku }\n  }\n`;\nexport const RESTOCK = gql`mutation Restock { restock { id } }`;\n";
        let out = extract_graphql_operation_nodes(src, "", module_id(), repo());
        assert_eq!(
            op_code(&out, "graphql_op:StockList"),
            ["\n  query StockList($first: Int) {\n    products(first: $first) { id sku }\n  }\n"]
        );
        assert_eq!(op_code(&out, "graphql_op:Restock"), ["mutation Restock { restock { id } }"]);
        let anonymous = extract_graphql_operation_nodes("const R = gql`{ orders { id } }`;\n", "", module_id(), repo());
        assert_eq!(op_code(&anonymous, "graphql_op:orders"), ["{ orders { id } }"]);
    }

    /// DT.11a: a needle named from the file's first document carries that
    /// document; one named by its needle text carries none; a Dart call
    /// carries its own constant's or inline literal's document.
    #[test]
    fn needle_op_carries_its_document() {
        let tagged = "import { gql } from '@apollo/client';\nclient.query({ query: ORDERS });\nconst ORDERS = gql`query Orders { orders { id } }`;\n";
        let out = extract_graphql_operation_nodes(tagged, "", module_id(), repo());
        assert_eq!(out.anchors.first().map(|a| a.line), Some(1), "minted at the needle line");
        assert_eq!(op_code(&out, "graphql_op:Orders"), ["query Orders { orders { id } }"]);
        let call = "import { gql } from '@apollo/client';\nconst ORDERS = gql(\"query Orders { orders { id } }\");\nclient.query({ query: ORDERS });\n";
        let out = extract_graphql_operation_nodes(call, "", module_id(), repo());
        assert_eq!(op_code(&out, "graphql_op:Orders"), ["query Orders { orders { id } }"]);
        let bare = extract_graphql_operation_nodes("const { data } = useQuery(GET_USERS);", "", module_id(), repo());
        assert!(op_code(&bare, "graphql_op:useQuery").is_empty());

        let out = extract_graphql_operation_nodes(DART_ORDERS, "", module_id(), repo());
        assert_eq!(op_code(&out, "graphql_op:Orders"), ["\n  query Orders {\n    orders {\n      id\n    }\n  }\n"]);
        let inline = "import 'package:graphql/client.dart';\nfinal f = client.query(QueryOptions(document: gql(r'query Inline { orders { id } }')));\n";
        let out = extract_graphql_operation_nodes(inline, "", module_id(), repo());
        assert_eq!(op_code(&out, "graphql_op:Inline"), ["query Inline { orders { id } }"]);
    }

    /// DT.11a: a Go machinebox / C# GraphQL.Client request carries the
    /// document it sends, named or anonymous.
    #[test]
    fn client_request_op_carries_its_document() {
        let go = "package main\n\nimport \"github.com/machinebox/graphql\"\n\nfunc f() {\n\treq := graphql.NewRequest(`query { orders { id } }`)\n\t_ = req\n}\n";
        let out = extract_graphql_operation_nodes(go, "client.go", module_id(), repo());
        assert_eq!(op_code(&out, "graphql_op:orders"), ["query { orders { id } }"]);
        let cs = "using GraphQL.Client.Http;\nvar r = new GraphQLRequest { Query = @\"query BookShelf { books { title } }\" };\n";
        let out = extract_graphql_operation_nodes(cs, "x.cs", module_id(), repo());
        assert_eq!(op_code(&out, "graphql_op:BookShelf"), ["query BookShelf { books { title } }"]);
    }

    /// DT.11a: a fragment definition, in a template or a request, is no
    /// operation and names nothing by a root field.
    #[test]
    fn fragment_definition_mints_nothing() {
        assert!(op_qnames("export const PARTS = gql`\n  fragment UserParts on User { id name }\n`;\n").is_empty());
        assert!(op_qnames("const T = gql`type Query { users: [User] }`;\n").is_empty());
        let go = "package main\n\nimport \"github.com/machinebox/graphql\"\n\nfunc f() {\n\treq := graphql.NewRequest(`fragment F on Order { id }`)\n\t_ = req\n}\n";
        assert!(op_qnames(go).is_empty());
    }
}
