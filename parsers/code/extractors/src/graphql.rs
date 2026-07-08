use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, node_kind};
use repo_graph_core::{Confidence, Node, NodeId, RepoId};

pub struct GraphqlNodes {
    pub nodes: Vec<Node>,
    pub nav: CodeNav,
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
    let mut seen = std::collections::HashSet::new();

    for line in source.lines() {
        let trimmed = line.trim();
        for &pattern in OPERATION_PATTERNS {
            if trimmed.contains(pattern) {
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
                }
                break;
            }
        }
    }

    for name in extract_gql_template_operations(source) {
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
        }
    }

    GraphqlNodes { nodes, nav }
}

pub fn extract_graphql_resolver_nodes(
    source: &str,
    module_id: NodeId,
    repo: RepoId,
) -> GraphqlNodes {
    let mut nodes = Vec::new();
    let mut nav = CodeNav::default();
    let mut seen = std::collections::HashSet::new();

    let mut resolver_names: Vec<String> = Vec::new();

    // Type-level / decorator-noun extraction (unchanged): "Query", "Mutation", …
    for &pattern in RESOLVER_PATTERNS {
        if source.contains(pattern) {
            let resolver_name = pattern
                .trim_start_matches('@')
                .trim_end_matches('(')
                .trim_end_matches(" {")
                .trim_end_matches("):")
                .replace("type ", "")
                .replace("graphene.", "");
            resolver_names.push(resolver_name);
        }
    }

    // Field-level extraction: the method under a resolver decorator and the
    // fields inside an SDL `type Query {}` block. These carry the operation
    // name a client `gql query getUser` pairs against.
    resolver_names.extend(extract_resolver_field_names(source));

    for resolver_name in resolver_names {
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
        }
    }

    GraphqlNodes { nodes, nav }
}

/// Collect resolver *field* names: the method following a `@Query()` /
/// `@Mutation()` / `@Subscription()` / `@ResolveField()` decorator, and each
/// field declared inside an SDL `type Query {}` / `type Mutation {}` block.
fn extract_resolver_field_names(source: &str) -> Vec<String> {
    let mut names = Vec::new();
    let lines: Vec<&str> = source.lines().collect();

    let mut in_sdl_type = false;
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();

        // SDL block field scan: `type Query {` … `}`.
        if in_sdl_type {
            if trimmed.starts_with('}') {
                in_sdl_type = false;
            } else if let Some(field) = sdl_field_name(trimmed) {
                names.push(field);
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
            for candidate in lines.iter().skip(i).take(4) {
                let t = candidate.trim();
                if t.is_empty() || t.starts_with('@') {
                    continue;
                }
                if let Some(name) = method_name_from_line(t) {
                    names.push(name);
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

fn extract_gql_template_operations(source: &str) -> Vec<String> {
    let mut ops = Vec::new();
    let mut search_from = 0;
    while let Some(idx) = source[search_from..].find("gql`") {
        let abs = search_from + idx + 4;
        if let Some(name) = extract_operation_from_body(&source[abs..]) {
            ops.push(name);
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
}
