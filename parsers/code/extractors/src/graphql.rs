use std::sync::OnceLock;

use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, node_kind};
use repo_graph_core::{Confidence, Node, NodeId, RepoId};

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

/// Decorator and code-first resolver nouns, read from code files: the noun
/// (`Query`, `Resolver`, `strawberry.type`) is the resolver name. SDL root
/// types are not here (LA.27): a `type Query {` opener is SDL, so it counts
/// only in a `.graphql` / `.gql` file or inside a GraphQL-marked literal.
const DECORATOR_PATTERNS: &[&str] = &[
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

/// Decorators whose *following method* is the actual resolver field (e.g. the
/// `getUser` in NestJS `@Query() async getUser()`). The decorator nouns above
/// only recover "Query", never the field a client operation is keyed by, so
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
/// (`engine::extract::apply_cross_cutting_extractors`): the decorator nouns,
/// the method under a field decorator, and SDL only inside a GraphQL-marked
/// literal ([`gql_literal_regions`], LA.27). A comment or an unmarked string
/// that holds `type Query {` mints nothing. A `.graphql` / `.gql` body goes
/// through [`extract_graphql_sdl_file_nodes`] instead.
pub fn extract_graphql_resolver_nodes(
    source: &str,
    module_id: NodeId,
    repo: RepoId,
) -> GraphqlNodes {
    // (name, 0-indexed line the name was read from)
    let mut names: Vec<(String, u32)> = Vec::new();

    // Decorator nouns: "Query", "Resolver", "strawberry.type", ...
    for &pattern in DECORATOR_PATTERNS {
        if let Some(idx) = source.find(pattern) {
            let noun = pattern
                .trim_start_matches('@')
                .trim_end_matches('(')
                .trim_end_matches("):")
                .replace("graphene.", "");
            names.push((noun, line_of(source, idx)));
        }
    }

    // Field level, in source order: the method under a resolver decorator,
    // and the roots and fields of SDL held in marked literals. Fields carry
    // the operation name a client `gql query getUser` pairs against.
    let embedded = embedded_sdl(source);
    if graphql_debug()
        && let Some(marker) = embedded.marker()
    {
        eprintln!("{marker}");
    }
    let mut fields = decorator_method_names(source);
    fields.extend(embedded.hits.into_iter().map(|h| (h.name, h.line)));
    fields.sort_by_key(|&(_, line)| line);
    names.extend(fields);

    resolver_nodes(names, module_id, repo)
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
    let line_u32 = |i: usize| u32::try_from(i).unwrap_or(u32::MAX);
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
        let source = "@Query()\nasync users() { return []; }";
        let result = extract_graphql_resolver_nodes(source, module_id(), repo());
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
        let source = "  @Query(() => User)\n  async getUser(@Args('id') id: string) {\n    return this.userService.findOne(id);\n  }";
        let result = extract_graphql_resolver_nodes(source, module_id(), repo());
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
        // Existing decorator-noun extraction is retained.
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
        let src = "@Resolver('User')\nexport class R {\n  @Query(() => User)\n  async getUser(id: string) {\n    return 1;\n  }\n}";
        let out = extract_graphql_resolver_nodes(src, module_id(), repo());
        assert_eq!(anchor_line(&out, "graphql_resolver:getUser"), Some(3));
        assert_eq!(anchor_line(&out, "graphql_resolver:Query"), Some(2));
        assert_eq!(anchor_line(&out, "graphql_resolver:Resolver"), Some(0));

        let sdl = "const typeDefs = `\ntype Query {\n  getUser(id: ID!): User\n  listUsers: [User]\n}\n`;";
        let out = extract_graphql_resolver_nodes(sdl, module_id(), repo());
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
    fn resolver_qnames(source: &str) -> Vec<String> {
        let out = extract_graphql_resolver_nodes(source, module_id(), repo());
        let mut qnames: Vec<String> = out.nav.qname_by_id.values().cloned().collect();
        qnames.sort();
        qnames
    }

    fn assert_resolvers(source: &str, want: &[&str]) {
        let got = resolver_qnames(source);
        let want: Vec<String> = want.iter().map(|n| format!("graphql_resolver:{n}")).collect();
        assert_eq!(got, want, "{source}");
    }

    #[test]
    fn sdl_in_a_rust_comment_mints_nothing() {
        // engine/src/route.rs's A10.4 comment, verbatim: the glia self-build
        // minted graphql_resolver:Query from it, HANDLED_BY parse_repo_files.
        let source = "fn parse_repo_files() {\n        // A10.4: a `.graphql` / `.gql` schema reaches the SDL field scan that\n        // embedded `type Query {` blocks already get, so a schema-first\n        // service has resolvers for its clients' operations to pair with.\n        if lang == \"graphql\" {}\n}\n";
        assert_resolvers(source, &[]);
        assert_eq!(
            embedded_sdl(source).marker().as_deref(),
            Some("[graphql-sdl-code] literals=0 roots= fields=0 unmarked_roots=1")
        );
    }

    #[test]
    fn sdl_in_an_unmarked_string_mints_nothing() {
        assert_resolvers(r#"let sdl = "type Query {\n  getUser(id: ID!): User\n}\n";"#, &[]);
        // A multi-line raw string: the block scan read it line by line.
        let raw = "#[test]\nfn schema_routes() {\n    let sdl = r#\"\ntype Subscription {\n  orderShipped: Order\n}\n\"#;\n    assert!(!sdl.is_empty());\n}\n";
        assert_resolvers(raw, &[]);
        assert_eq!(
            embedded_sdl(raw).marker().as_deref(),
            Some("[graphql-sdl-code] literals=0 roots= fields=0 unmarked_roots=1")
        );
    }

    #[test]
    fn sdl_in_a_python_comment_mints_nothing() {
        let source = "# How the resolver scan works: a line like\n#   type Query {\n# opens a root type block.\ndef scan(text):\n    return text.splitlines()\n";
        assert_resolvers(source, &[]);
    }

    #[test]
    fn gql_tagged_sdl_mints_root_and_fields() {
        // The tag marks the literal whatever the variable is called.
        let source = "import { gql } from \"graphql-tag\";\n\nexport const schema = gql`\n  type Query {\n    listOrders: [Order]\n  }\n\n  type Order {\n    id: ID!\n  }\n`;\n";
        let out = extract_graphql_resolver_nodes(source, module_id(), repo());
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
        assert_resolvers(source, &["Mutation", "placeOrder"]);
        let out = extract_graphql_resolver_nodes(source, module_id(), repo());
        assert_eq!(anchor_line(&out, "graphql_resolver:placeOrder"), Some(4));
    }

    #[test]
    fn go_must_parse_schema_literal_mints_fields() {
        let source = "func Schema() *graphql.Schema {\n\treturn graphql.MustParseSchema(`\n\ttype Query {\n\t\tuser(id: ID!): User\n\t}\n`, &resolver{})\n}\n";
        assert_resolvers(source, &["Query", "user"]);
        // A schema string passed by name is not a marked literal.
        assert_resolvers("var s = `\ntype Query {\n  user: User\n}\n`\nvar _ = graphql.MustParseSchema(s, &r{})\n", &[]);
        // `rebuildSchema(` is not `buildSchema(`.
        assert_resolvers("rebuildSchema(`\ntype Query {\n  user: User\n}\n`)\n", &[]);
    }

    #[test]
    fn ruby_graphql_heredoc_mints_fields() {
        let source = "class Schema\n  DEFINITION = <<~GRAPHQL\n    type Query {\n      posts: [Post]\n    }\n  GRAPHQL\nend\nSchema2 = GraphQL::Schema.from_definition(<<-GQL)\n  extend type Mutation {\n    deletePost(id: ID!): Boolean\n  }\nGQL\n";
        assert_resolvers(source, &["Mutation", "Query", "deletePost", "posts"]);
        let out = extract_graphql_resolver_nodes(source, module_id(), repo());
        assert_eq!(anchor_line(&out, "graphql_resolver:posts"), Some(3));
        assert_eq!(anchor_line(&out, "graphql_resolver:deletePost"), Some(9));
        // No terminator line: no region, and no panic.
        assert_resolvers("q = <<~GRAPHQL\n  type Query {\n    posts: [Post]\n  }\n", &[]);
    }

    #[test]
    fn graphql_magic_comment_template_mints_fields() {
        let source = "const schema = /* GraphQL */ `\n  type Mutation {\n    addBook(title: String): Book\n  }\n`;\n";
        assert_resolvers(source, &["Mutation", "addBook"]);
    }

    #[test]
    fn hash_graphql_template_mints_fields() {
        // Apollo Server 4: `schema` is a name no binding rule knows.
        let source = "const schema = `#graphql\n  type Query {\n    books: [Book]\n  }\n`;\n";
        assert_resolvers(source, &["Query", "books"]);
        let py = "SCHEMA = \"\"\"\n# graphql\ntype Query {\n  books: [Book]\n}\n\"\"\"\n";
        assert_resolvers(py, &["Query", "books"]);
    }

    #[test]
    fn typedefs_bindings_mint_fields() {
        for source in [
            "export const typeDefs: string = `\ntype Query {\n  me: User\n}\n`;\n",
            "type_defs = \"\"\"\ntype Query {\n    me: User\n}\n\"\"\"\n",
            "type_defs: str = \"\"\"\ntype Query {\n    me: User\n}\n\"\"\"\n",
            "new ApolloServer({\n  typeDefs: `\n    type Query {\n      me: User\n    }\n  `,\n  resolvers,\n});\n",
        ] {
            assert_resolvers(source, &["Query", "me"]);
        }
        // The same template in a differently named variable is not read
        // (the coverage caveat row declares this), nor is a longer name.
        for source in [
            "const schema = `\ntype Query {\n  me: User\n}\n`;\n",
            "const typeDefsV2 = `\ntype Query {\n  me: User\n}\n`;\n",
            "if (typeDefs == `\ntype Query {\n  me: User\n}\n`) {}\n",
        ] {
            assert_resolvers(source, &[]);
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
        assert_resolvers(source, &[]);
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
        assert_resolvers(sdl, &[]);
    }
}
