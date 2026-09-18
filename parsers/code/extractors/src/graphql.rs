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

const RESOLVER_PATTERNS: &[&str] = &[
    "@Query(",
    "@Mutation(",
    "@Subscription(",
    "@Resolver(",
    "@ResolveField(",
    "type Query {",
    "type Mutation {",
    "type Subscription {",
    "@strawberry.type",
    "@strawberry.mutation",
    "ObjectType):",
    "graphene.ObjectType",
];

/// Decorators whose *following method* is the actual resolver field (e.g. the
/// `getUser` in NestJS `@Query() async getUser()`). The type-level patterns
/// above only recover the decorator noun ("Query"), never the field a client
/// operation is keyed by, so GRAPHQL_CALLS never pairs.
const RESOLVER_FIELD_DECORATORS: &[&str] =
    &["@Query(", "@Mutation(", "@Subscription(", "@ResolveField("];

/// GraphQL SDL type blocks whose fields are resolver operations.
const SDL_RESOLVER_TYPES: &[&str] = &["type Query {", "type Mutation {", "type Subscription {"];

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

pub fn extract_graphql_resolver_nodes(
    source: &str,
    module_id: NodeId,
    repo: RepoId,
) -> GraphqlNodes {
    let mut nodes = Vec::new();
    let mut nav = CodeNav::default();
    let mut anchors = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // (name, 0-indexed line the name was read from)
    let mut resolver_names: Vec<(String, u32)> = Vec::new();

    // Type-level / decorator-noun extraction (unchanged): "Query", "Mutation", …
    for &pattern in RESOLVER_PATTERNS {
        if let Some(idx) = source.find(pattern) {
            let resolver_name = pattern
                .trim_start_matches('@')
                .trim_end_matches('(')
                .trim_end_matches(" {")
                .trim_end_matches("):")
                .replace("type ", "")
                .replace("graphene.", "");
            resolver_names.push((resolver_name, line_of(source, idx)));
        }
    }

    // Field-level extraction: the method under a resolver decorator and the
    // fields inside an SDL `type Query {}` block. These carry the operation
    // name a client `gql query getUser` pairs against.
    resolver_names.extend(extract_resolver_field_names(source));

    for (resolver_name, line) in resolver_names {
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

/// Collect resolver *field* names: the method following a `@Query()` /
/// `@Mutation()` / `@Subscription()` / `@ResolveField()` decorator, and each
/// field declared inside an SDL `type Query {}` / `type Mutation {}` block.
/// Each name carries the 0-indexed line it was read from (the method
/// declaration line, not the decorator's).
fn extract_resolver_field_names(source: &str) -> Vec<(String, u32)> {
    let mut names = Vec::new();
    let lines: Vec<&str> = source.lines().collect();

    let mut in_sdl_type = false;
    let line_u32 = |i: usize| u32::try_from(i).unwrap_or(u32::MAX);
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();

        // SDL block field scan: `type Query {` … `}`.
        if in_sdl_type {
            if trimmed.starts_with('}') {
                in_sdl_type = false;
            } else if let Some(field) = sdl_field_name(trimmed) {
                names.push((field, line_u32(i)));
            }
            continue;
        }
        if SDL_RESOLVER_TYPES.iter().any(|t| trimmed.starts_with(t)) {
            in_sdl_type = true;
            continue;
        }

        // Decorator-method scan: the method name after `@Query()` etc.
        if RESOLVER_FIELD_DECORATORS
            .iter()
            .any(|d| trimmed.starts_with(d))
        {
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
    }

    names
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
        let source = "type Query {\n  users: [User]\n}";
        let result = extract_graphql_resolver_nodes(source, module_id(), repo());
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
        let result = extract_graphql_resolver_nodes(source, module_id(), repo());
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
}
