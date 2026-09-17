use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, node_kind};
use repo_graph_core::{Confidence, Node, NodeId, RepoId};

use crate::anchor::{Anchor, line_of};

pub struct GraphqlNodes {
    pub nodes: Vec<Node>,
    pub nav: CodeNav,
    /// A5.8: the line that minted each node (see `crate::anchor`).
    pub anchors: Vec<Anchor>,
}

const OPERATION_PATTERNS: &[&str] = &[
    "useQuery(",
    "useMutation(",
    "useSubscription(",
    "useLazyQuery(",
    "client.query(",
    "client.mutate(",
    "client.subscribe(",
    "graphql-request",
    "request(",
];

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

    for (line_no, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        for &pattern in OPERATION_PATTERNS {
            if trimmed.contains(pattern) {
                // A10.9: `api.user.list.useQuery()` is a tRPC hook, not a
                // GraphQL operation. With no gql tag in the file the fallback
                // below would mint `graphql_op:useQuery`, which the GraphQL
                // resolver then pairs to any `Query` resolver by substring.
                if crate::trpc::is_trpc_client_line(trimmed) {
                    break;
                }
                let op_name = extract_gql_operation_name(source, trimmed)
                    .unwrap_or_else(|| pattern.trim_end_matches('(').to_string());
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
                    let line = u32::try_from(line_no).unwrap_or(u32::MAX);
                    anchors.push(Anchor { node: id, line });
                }
                break;
            }
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
}
