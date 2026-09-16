//! API-contract extraction from a repo's own spec files (A10.1).
//!
//! A service's `openapi.yaml` / `swagger.yaml` is its DECLARED interface. The
//! walker already admits every `.yml|.yaml`, but the three extractors that saw
//! them (cron / config / iac) have no notion of `paths:` — so the declared API
//! surface was invisible to the graph: nothing said `GET /users` is specified,
//! `docs-for` on a route returned nothing, and spec-vs-implementation drift had
//! no substrate.
//!
//! This is the NODE half: one node per OpenAPI operation. A10.2 adds the edge
//! that pairs an operation with the ROUTE that implements it.
//!
//! Zero-dependency on purpose: an indentation scanner, not a YAML crate. Every
//! `.yaml` in every repo hits the sniff, so the miss path must stay cheap, and
//! the engine must not grow a yaml dependency for a line-shaped read.
//!
//! Parsers EXTRACT — nothing here resolves anything; pairing operations with
//! routes is the graph crate's job.

use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, cell_type, node_kind};
use repo_graph_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};

/// Everything one contract file contributes to the graph. `edges` is empty for
/// OpenAPI: the module CONTAINS relation is carried by the nav parent, exactly
/// as queues / graphql / grpc do.
#[derive(Default)]
pub struct ContractNodes {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub nav: CodeNav,
}

/// Per-build counters behind the `[contract]` marker. All four fields exist now
/// so A10.3 (asyncapi) / A10.8 (pact) only ever increment — the marker string
/// never has to be rewritten.
#[derive(Default)]
pub struct ContractCounts {
    pub files: usize,
    pub openapi: usize,
    pub asyncapi: usize,
    pub pact: usize,
}

/// One declared operation: `<method> <path>` plus where it was declared.
#[derive(Debug, PartialEq, Eq)]
pub struct Op {
    pub method: String,
    /// Server base joined onto the path-item key.
    pub path: String,
    /// The path-item key exactly as written, un-prefixed. A10.2 retries on this
    /// when the prefixed form doesn't pair with a ROUTE.
    pub raw_path: String,
    pub operation_id: Option<String>,
    /// 0-indexed source line of the method key (`repo_graph_docs::position_json`
    /// convention, same as build_docs_graph).
    pub line: u32,
}

/// HTTP methods an OpenAPI path item may declare. The allow-list is what gates
/// emission: `parameters:`, `summary:`, `$ref:` and friends sit at the same
/// indent and are ignored by construction.
pub const METHODS: &[&str] = &[
    "get", "put", "post", "delete", "patch", "head", "options", "trace",
];

/// Never let a pathological file blow the node budget.
const MAX_OPS: usize = 2000;

/// How far into the file the `openapi:` / `swagger:` marker may sit.
const SNIFF_LINES: usize = 64;

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Count of leading whitespace characters. Tabs count as one — YAML forbids
/// them for indentation, so any file that uses them is malformed anyway and we
/// only need a consistent ordering, not a column number.
fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// Blank or a whole-line `#` comment: carries no structure, so it neither ends
/// a block nor sets an indent level.
fn is_skippable(line: &str) -> bool {
    let t = line.trim_start();
    t.is_empty() || t.starts_with('#')
}

/// The key of a `key: value` line, quotes stripped. `None` when the line has no
/// colon at all (a bare list element, a folded-scalar continuation).
fn key_of(line: &str) -> Option<&str> {
    let t = line.trim();
    let (k, _) = t.split_once(':')?;
    Some(unquote(k.trim()))
}

/// The value half of `key: value`, comment- and quote-stripped. Empty when the
/// key opens a block.
fn value_of(line: &str) -> &str {
    let t = line.trim();
    let Some((_, v)) = t.split_once(':') else {
        return "";
    };
    let v = match v.split_once(" #") {
        Some((head, _)) => head,
        None => v,
    };
    unquote(v.trim())
}

fn unquote(s: &str) -> &str {
    let t = s.trim();
    for q in ['"', '\''] {
        if t.len() >= 2 && t.starts_with(q) && t.ends_with(q) {
            return &t[1..t.len() - 1];
        }
    }
    t
}

/// Fold a `servers[0].url` / Swagger-2 `basePath` into the path prefix every
/// operation inherits. Public because A10.8 folds the SAME url from the JSON
/// contract path and the two must agree.
///
/// An absolute url keeps only its path component; a TEMPLATED url (`{host}`)
/// folds to "" rather than a garbage prefix, because a wrong prefix is worse
/// than none — A10.2 pairs on `raw_path` as a fallback either way.
pub fn fold_server_base(raw: &str) -> String {
    let v = unquote(raw.trim());
    if v.is_empty() || v.contains('{') {
        return String::new();
    }
    let path = match v.find("://") {
        Some(i) => {
            let after = &v[i + 3..];
            match after.find('/') {
                Some(j) => &after[j..],
                None => "",
            }
        }
        None => v,
    };
    path.trim_end_matches('/').to_string()
}

/// `join("/api/v1", "/users")` → `/api/v1/users`, always leading-slashed and
/// never double-slashed.
fn join_path(base: &str, path: &str) -> String {
    let mut s = String::with_capacity(base.len() + path.len() + 1);
    s.push_str(base.trim_end_matches('/'));
    if !path.starts_with('/') {
        s.push('/');
    }
    s.push_str(path);
    while s.contains("//") {
        s = s.replace("//", "/");
    }
    if !s.starts_with('/') {
        s.insert(0, '/');
    }
    s
}

/// Cheap sniff: is this an OpenAPI/Swagger document at all? Scans at most the
/// first [`SNIFF_LINES`] lines for an indent-0 `openapi:` / `swagger:` key, and
/// allocates nothing.
fn is_openapi(source: &str) -> bool {
    for line in source.lines().take(SNIFF_LINES) {
        if is_skippable(line) || indent_of(line) != 0 {
            continue;
        }
        let t = line.trim_end();
        if t.starts_with("openapi:") || t.starts_with("swagger:") {
            return true;
        }
    }
    false
}

/// Indentation scan of the `paths:` block. Returns operations in declaration
/// order (never a map — the store's byte-identical gate depends on emission
/// order).
pub fn scan_openapi(source: &str) -> Vec<Op> {
    let lines: Vec<&str> = source.lines().collect();
    let base = scan_base(&lines);
    let mut ops: Vec<Op> = Vec::new();

    let Some(paths_idx) = lines
        .iter()
        .position(|l| !is_skippable(l) && indent_of(l) == 0 && l.trim() == "paths:")
    else {
        return ops;
    };

    // The indent of the first child of `paths:` defines the path-item level.
    let Some(item_indent) = lines[paths_idx + 1..]
        .iter()
        .find(|l| !is_skippable(l))
        .map(|l| indent_of(l))
        .filter(|i| *i > 0)
    else {
        return ops;
    };

    let mut cur_path: Option<String> = None;
    // (indent of the method key, index into `ops`)
    let mut cur_op: Option<(usize, usize)> = None;

    for (i, line) in lines.iter().enumerate().skip(paths_idx + 1) {
        if is_skippable(line) {
            continue;
        }
        let ind = indent_of(line);
        if ind == 0 {
            break; // a new top-level key ends the paths block
        }
        if let Some((op_indent, _)) = cur_op
            && ind <= op_indent
        {
            cur_op = None;
        }
        let Some(key) = key_of(line) else { continue };

        if ind == item_indent {
            cur_path = key.starts_with('/').then(|| key.to_string());
            continue;
        }
        if ind <= item_indent {
            continue;
        }

        // Inside an operation body: the only thing we still want is its id.
        if let Some((_, idx)) = cur_op {
            if key == "operationId" {
                let v = value_of(line);
                if !v.is_empty()
                    && let Some(op) = ops.get_mut(idx)
                {
                    op.operation_id = Some(v.to_string());
                }
            }
            continue;
        }

        // A direct child of the path item whose key is an HTTP method opens one.
        let Some(path) = cur_path.as_deref() else {
            continue;
        };
        let lower = key.to_ascii_lowercase();
        if !METHODS.contains(&lower.as_str()) {
            continue;
        }
        if ops.len() >= MAX_OPS {
            break;
        }
        ops.push(Op {
            method: lower.to_ascii_uppercase(),
            path: join_path(&base, path),
            raw_path: path.to_string(),
            operation_id: None,
            line: i as u32,
        });
        cur_op = Some((ind, ops.len() - 1));
    }

    ops
}

/// `servers: - url: <v>` (OpenAPI 3) or `basePath: <v>` (Swagger 2), whichever
/// appears first at indent 0.
fn scan_base(lines: &[&str]) -> String {
    for (i, line) in lines.iter().enumerate() {
        if is_skippable(line) || indent_of(line) != 0 {
            continue;
        }
        let t = line.trim();
        if t == "servers:" {
            for next in &lines[i + 1..] {
                if is_skippable(next) {
                    continue;
                }
                if indent_of(next) == 0 {
                    break;
                }
                let inner = next.trim();
                if let Some(rest) = inner.strip_prefix("- ")
                    && let Some((k, v)) = rest.split_once(':')
                    && k.trim() == "url"
                {
                    return fold_server_base(v);
                }
                // `- ` on its own line, `url:` beneath it.
                if let Some((k, v)) = inner.split_once(':')
                    && k.trim() == "url"
                {
                    return fold_server_base(v);
                }
            }
            return String::new();
        }
        if let Some(rest) = t.strip_prefix("basePath:") {
            return fold_server_base(rest);
        }
    }
    String::new()
}

/// Sniff a `.yaml`/`.yml` and, when it is an API contract, emit one node per
/// declared operation. A non-contract yaml — the overwhelming majority — takes
/// the allocation-free miss path and returns empty.
///
/// The node kind is the EXISTING `DOC_SECTION` (42), deliberately not a new
/// kind: `governing_docs` / `glia docs-for` then answer with contract ops for
/// free, engram-export already maps DOC_SECTION → `Content::Proposition`, and
/// pyo3 decodes it today. The `ORIGIN` cell (`provenance=contract`) is the
/// discriminator every downstream pass keys on.
pub fn extract_yaml_contracts(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
) -> ContractNodes {
    let mut out = ContractNodes::default();
    if !is_openapi(source) {
        return out;
    }
    let stem = std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("spec");

    for op in scan_openapi(source) {
        let qname = format!("contract::{stem}::{}:{}", op.method, op.path);
        let name = format!("{} {}", op.method, op.path);
        let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::DOC_SECTION, &qname);

        let code = match &op.operation_id {
            Some(oid) => format!("{name} — {oid}"),
            None => name.clone(),
        };
        let pos = format!(
            r#"{{"file":"{}","start_line":{},"end_line":{}}}"#,
            esc(path),
            op.line,
            op.line
        );
        let oid = match &op.operation_id {
            Some(oid) => format!(r#","operation_id":"{}""#, esc(oid)),
            None => String::new(),
        };
        let origin = format!(
            r#"{{"provenance":"contract","source":"openapi","method":"{}","path":"{}","raw_path":"{}"{}}}"#,
            esc(&op.method),
            esc(&op.path),
            esc(&op.raw_path),
            oid
        );
        out.nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Strong,
            cells: vec![
                Cell { kind: cell_type::CODE, payload: CellPayload::Text(code) },
                Cell { kind: cell_type::POSITION, payload: CellPayload::Json(pos) },
                Cell { kind: cell_type::ORIGIN, payload: CellPayload::Json(origin) },
            ],
        });
        out.nav
            .record(id, &name, &qname, node_kind::DOC_SECTION, Some(module_id));
    }
    out
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
    fn non_openapi_yaml_yields_nothing() {
        let source = r#"apiVersion: apps/v1
kind: Deployment
metadata:
  name: api
spec:
  template:
    spec:
      containers:
        - name: api
          image: api:1.0
"#;
        let out = extract_yaml_contracts(source, "k8s/deploy.yaml", module_id(), repo());
        assert!(
            out.nodes.is_empty(),
            "a k8s manifest is not an API contract — the sniff must miss"
        );
        assert!(scan_openapi(source).is_empty());
    }

    #[test]
    fn server_prefix_is_joined_onto_the_path() {
        let source = r#"openapi: 3.0.3
servers:
  - url: /api/v1
paths:
  /users:
    get:
      operationId: listUsers
      responses:
        '200':
          description: ok
"#;
        let out = extract_yaml_contracts(source, "spec.yaml", module_id(), repo());
        assert_eq!(out.nodes.len(), 1);
        let id = out.nodes[0].id;
        assert_eq!(out.nav.qname_by_id[&id], "contract::spec::GET:/api/v1/users");
        assert_eq!(out.nav.name_by_id[&id], "GET /api/v1/users");
        assert_eq!(out.nav.kind_by_id[&id], node_kind::DOC_SECTION);
        assert_eq!(out.nav.parent_of[&id], module_id());

        let origin = out.nodes[0]
            .cells
            .iter()
            .find(|c| c.kind == cell_type::ORIGIN)
            .expect("contract ops carry an ORIGIN discriminator");
        match &origin.payload {
            CellPayload::Json(j) => {
                assert!(j.contains(r#""provenance":"contract""#), "got {j}");
                assert!(j.contains(r#""source":"openapi""#), "got {j}");
                assert!(j.contains(r#""raw_path":"/users""#), "un-prefixed form kept: {j}");
                assert!(j.contains(r#""path":"/api/v1/users""#), "got {j}");
                assert!(j.contains(r#""operation_id":"listUsers""#), "got {j}");
            }
            other => panic!("ORIGIN must be Json, got {other:?}"),
        }
        let pos = out.nodes[0]
            .cells
            .iter()
            .find(|c| c.kind == cell_type::POSITION)
            .expect("located at its declaration line");
        match &pos.payload {
            CellPayload::Json(j) => {
                assert!(j.contains(r#""file":"spec.yaml""#), "got {j}");
                // `get:` is the 6th line => 0-indexed line 5.
                assert!(j.contains(r#""start_line":5"#), "got {j}");
            }
            other => panic!("POSITION must be Json, got {other:?}"),
        }
    }

    #[test]
    fn absolute_server_url_keeps_only_its_path() {
        let source = r#"openapi: 3.0.0
servers:
  - url: https://api.example.com/v1
paths:
  /orders:
    post:
      responses:
        '201':
          description: created
"#;
        let ops = scan_openapi(source);
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].method, "POST");
        assert_eq!(ops[0].path, "/v1/orders");
        assert_eq!(ops[0].raw_path, "/orders");
        assert_eq!(ops[0].operation_id, None);
    }

    #[test]
    fn templated_server_folds_to_no_prefix() {
        // A wrong prefix is worse than none: fold to the raw path.
        let source = r#"openapi: 3.0.0
servers:
  - url: https://{host}/v1
paths:
  /orders:
    get:
      responses:
        '200':
          description: ok
"#;
        let ops = scan_openapi(source);
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].path, "/orders");
    }

    #[test]
    fn path_item_level_parameters_emit_nothing() {
        // `parameters:` sits exactly where a method key sits; only the method
        // allow-list may open an operation. A nested `get:` inside a schema
        // must not either.
        let source = r#"swagger: "2.0"
basePath: /v2
paths:
  /pets/{id}:
    parameters:
      - name: id
        in: path
        required: true
    summary: one pet
    get:
      responses:
        '200':
          description: ok
          schema:
            properties:
              get:
                type: string
"#;
        let ops = scan_openapi(source);
        assert_eq!(
            ops.len(),
            1,
            "only the real `get:` operation, not `parameters:` / `summary:` / the nested `get` property"
        );
        assert_eq!(ops[0].method, "GET");
        assert_eq!(ops[0].path, "/v2/pets/{id}");
    }
}
