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
//! A10.3: an `asyncapi.yaml` is the message-side twin — one node per channel
//! operation (`publish orders`), which the engine's contract post-pass pairs
//! with the QUEUE_PRODUCER / QUEUE_CONSUMER named for that channel.
//!
//! A10.8: the same contracts shipped as JSON — `swagger.json` / `openapi.json`
//! (what Swashbuckle, springdoc, FastAPI and NestJS emit), `asyncapi.json`, and
//! Pact files, whose interactions each carry a literal `request.method` +
//! `request.path`. The walker admits a `.json` only when
//! [`sniff_json_contract`] says it is one, so lock files, tsconfig and test
//! data are read once and dropped. Every format emits the node shape the yaml
//! path does, so the engine's pairing pass reads all of them unchanged.
//!
//! Zero-dependency on purpose: an indentation scanner, not a YAML crate. Every
//! `.yaml` in every repo hits the sniff, so the miss path must stay cheap, and
//! the engine must not grow a yaml dependency for a line-shaped read. JSON is
//! parsed with serde_json, which the crate already depends on, but only after
//! the sniff hits.
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
    /// Which contract format the file sniffed as; `None` for a non-contract
    /// yaml, and for the ops `openapi_annot` reads off handler annotations
    /// (LA.15a), which are counted by `[openapi-annot]`, never here. Drives
    /// the per-format `[contract]` counters.
    pub source: Option<ContractSource>,
}

/// The contract formats `extract_yaml_contracts` / `extract_json_contract`
/// recognise. `Pact` only ever comes from JSON: Pact has no yaml form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContractSource {
    OpenApi,
    AsyncApi,
    Pact,
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

impl ContractCounts {
    /// Fold one file's extraction into the build counters. A file that sniffed
    /// as a contract but declared nothing counts nowhere.
    pub fn record(&mut self, out: &ContractNodes) {
        if out.nodes.is_empty() {
            return;
        }
        let n = out.nodes.len();
        match out.source {
            Some(ContractSource::OpenApi) => self.openapi += n,
            Some(ContractSource::AsyncApi) => self.asyncapi += n,
            Some(ContractSource::Pact) => self.pact += n,
            None => return,
        }
        self.files += 1;
    }
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

pub(crate) fn esc(s: &str) -> String {
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

/// What a yaml's indent-0 format marker says it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sniffed {
    OpenApi,
    AsyncApi,
}

/// Cheap sniff: is this an API contract at all, and which kind? Scans at most
/// the first [`SNIFF_LINES`] lines for an indent-0 `openapi:` / `swagger:` /
/// `asyncapi:` key, and allocates nothing.
fn sniff(source: &str) -> Option<Sniffed> {
    for line in source.lines().take(SNIFF_LINES) {
        if is_skippable(line) || indent_of(line) != 0 {
            continue;
        }
        let t = line.trim_end();
        if t.starts_with("openapi:") || t.starts_with("swagger:") {
            return Some(Sniffed::OpenApi);
        }
        if t.starts_with("asyncapi:") {
            return Some(Sniffed::AsyncApi);
        }
    }
    None
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

/// One declared AsyncAPI channel operation: `<action> <channel>`.
#[derive(Debug, PartialEq, Eq)]
pub struct ChannelOp {
    /// `publish` or `subscribe`. AsyncAPI v3's `send` / `receive` are mapped
    /// onto these, so the pairing pass sees one vocabulary.
    pub action: &'static str,
    /// The channel string exactly as declared (v2: the `channels:` key; v3:
    /// the referenced channel's `address`, else its id).
    pub channel: String,
    pub operation_id: Option<String>,
    /// 0-indexed line of the operation key (`publish:` in v2, the operation id
    /// in v3).
    pub line: u32,
}

/// Split a block-mapping line into `(key, raw value)`, for keys that may carry
/// `:` or `/` themselves — an AsyncAPI channel name (`'urn:orders'`,
/// `user/{id}/signedup`). A quoted key ends at its closing quote; a plain key
/// ends at the first `": "` or a trailing `:`. A list element (`- x`) is never
/// a key.
fn split_key(line: &str) -> Option<(&str, &str)> {
    let t = line.trim();
    if t.starts_with('-') {
        return None;
    }
    for q in ['"', '\''] {
        if let Some(body) = t.strip_prefix(q) {
            let end = body.find(q)?;
            let rest = body[end + 1..].trim_start().strip_prefix(':')?;
            return Some((&body[..end], rest.trim()));
        }
    }
    match t.find(": ") {
        Some(i) => Some((t[..i].trim(), t[i + 2..].trim())),
        None => Some((t.strip_suffix(':')?.trim(), "")),
    }
}

/// The line index of an indent-0 `<name>:` block opener.
fn top_level_block(lines: &[&str], name: &str) -> Option<usize> {
    lines.iter().position(|l| {
        !is_skippable(l)
            && indent_of(l) == 0
            && split_key(l).is_some_and(|(k, v)| k == name && v.is_empty())
    })
}

/// Indent of the first non-skippable line after `start`, when it is a child
/// (indent > 0) rather than the next top-level key.
fn first_child_indent(lines: &[&str], start: usize) -> Option<usize> {
    lines[start + 1..]
        .iter()
        .find(|l| !is_skippable(l))
        .map(|l| indent_of(l))
        .filter(|i| *i > 0)
}

/// The `asyncapi:` version's major component (`"2.6.0"` → `"2"`).
fn asyncapi_major<'a>(lines: &[&'a str]) -> Option<&'a str> {
    lines.iter().take(SNIFF_LINES).find_map(|l| {
        if is_skippable(l) || indent_of(l) != 0 {
            return None;
        }
        let (k, _) = split_key(l)?;
        (k == "asyncapi").then(|| value_of(l).split('.').next().unwrap_or(""))
    })
}

/// Indentation scan of an AsyncAPI document's channel operations, in
/// declaration order. v3 (`asyncapi: 3.x`) reads the top-level `operations:`
/// block; every other version reads v2's `channels.<name>.publish|subscribe`.
/// A shape the scanner does not recognise yields nothing — a wrong channel is
/// worse than a missing one.
pub fn scan_asyncapi(source: &str) -> Vec<ChannelOp> {
    let lines: Vec<&str> = source.lines().collect();
    let mut ops = if asyncapi_major(&lines) == Some("3") {
        scan_asyncapi_v3(&lines)
    } else {
        scan_asyncapi_v2(&lines)
    };
    dedup_channel_ops(&mut ops);
    ops
}

/// One node per (action, channel): a v3 doc may declare two `send` operations
/// on one channel, and the qname would collide. First wins. Shared by the yaml
/// and JSON AsyncAPI paths.
fn dedup_channel_ops(ops: &mut Vec<ChannelOp>) {
    let mut seen: Vec<(&'static str, String)> = Vec::new();
    ops.retain(|op| {
        let key = (op.action, op.channel.clone());
        if seen.contains(&key) {
            return false;
        }
        seen.push(key);
        true
    });
    ops.truncate(MAX_OPS);
}

/// v2: each child of `channels:` is a channel name; its direct `publish:` /
/// `subscribe:` children are the operations, and an `operationId:` directly
/// beneath one names it.
fn scan_asyncapi_v2(lines: &[&str]) -> Vec<ChannelOp> {
    let mut ops: Vec<ChannelOp> = Vec::new();
    let Some(start) = top_level_block(lines, "channels") else {
        return ops;
    };
    let Some(chan_indent) = first_child_indent(lines, start) else {
        return ops;
    };

    let mut cur_chan: Option<&str> = None;
    // Indent of the current channel item's direct children.
    let mut body_indent: Option<usize> = None;
    // (indent of the op key, index into `ops`, indent of the op's children)
    let mut cur_op: Option<(usize, usize, Option<usize>)> = None;

    for (i, line) in lines.iter().enumerate().skip(start + 1) {
        if is_skippable(line) {
            continue;
        }
        let ind = indent_of(line);
        if ind == 0 {
            break; // a new top-level key ends the channels block
        }
        if let Some((op_indent, _, _)) = cur_op
            && ind <= op_indent
        {
            cur_op = None;
        }
        if ind < chan_indent {
            continue;
        }
        if ind == chan_indent {
            cur_chan = split_key(line).map(|(k, _)| k);
            body_indent = None;
            continue;
        }

        // Inside an operation body: only its own `operationId` is wanted.
        if let Some((_, idx, ref mut op_body)) = cur_op {
            let child = *op_body.get_or_insert(ind);
            if ind == child
                && split_key(line).is_some_and(|(k, _)| k == "operationId")
                && let Some(op) = ops.get_mut(idx)
                && op.operation_id.is_none()
            {
                let v = value_of(line);
                if !v.is_empty() {
                    op.operation_id = Some(v.to_string());
                }
            }
            continue;
        }

        let Some(chan) = cur_chan.filter(|c| !c.is_empty()) else {
            continue;
        };
        if ind != *body_indent.get_or_insert(ind) {
            continue; // deeper than the channel item's own keys
        }
        let action = match split_key(line) {
            Some(("publish", "")) => "publish",
            Some(("subscribe", "")) => "subscribe",
            _ => continue,
        };
        if ops.len() >= MAX_OPS {
            break;
        }
        ops.push(ChannelOp {
            action,
            channel: chan.to_string(),
            operation_id: None,
            line: i as u32,
        });
        cur_op = Some((ind, ops.len() - 1, None));
    }
    ops
}

/// What a v3 `channels.<id>.address` says about the channel's real name.
enum Address<'a> {
    Declared(&'a str),
    /// `address: null` — the spec's "unknown / dynamic channel".
    Null,
}

/// v3 `channels:` → `(id, address)` for each channel item that declares one.
fn v3_addresses<'a>(lines: &[&'a str]) -> Vec<(&'a str, Address<'a>)> {
    let mut out = Vec::new();
    let Some(start) = top_level_block(lines, "channels") else {
        return out;
    };
    let Some(chan_indent) = first_child_indent(lines, start) else {
        return out;
    };
    let mut cur: Option<&str> = None;
    let mut body_indent: Option<usize> = None;
    for line in lines.iter().skip(start + 1) {
        if is_skippable(line) {
            continue;
        }
        let ind = indent_of(line);
        if ind == 0 {
            break;
        }
        if ind < chan_indent {
            continue;
        }
        if ind == chan_indent {
            cur = split_key(line).map(|(k, _)| k);
            body_indent = None;
            continue;
        }
        let Some(id) = cur else { continue };
        if ind != *body_indent.get_or_insert(ind) {
            continue;
        }
        if split_key(line).is_some_and(|(k, _)| k == "address") {
            let v = value_of(line);
            let addr = if v.is_empty() || v == "null" || v == "~" {
                Address::Null
            } else {
                Address::Declared(v)
            };
            out.push((id, addr));
        }
    }
    out
}

/// A same-document channel pointer `#/channels/<id>` → `<id>` (RFC 6901
/// unescaped). Anything deeper, or pointing into another file, is `None`.
fn v3_channel_ref(raw: &str) -> Option<String> {
    let rest = unquote(raw).strip_prefix("#/channels/")?;
    if rest.is_empty() || rest.contains('/') {
        return None;
    }
    Some(rest.replace("~1", "/").replace("~0", "~"))
}

/// The `$ref` inside a flow mapping `{ $ref: '#/channels/x' }`.
fn flow_ref(v: &str) -> Option<&str> {
    let inner = v.strip_prefix('{')?.strip_suffix('}')?;
    let (k, val) = inner.split_once(':')?;
    (unquote(k.trim()) == "$ref").then(|| val.trim())
}

/// v3: each child of `operations:` is an operation id carrying
/// `action: send|receive` and `channel: {$ref: '#/channels/<id>'}`. The
/// channel string is that channel's `address` when it declares one, nothing
/// when it declares `address: null` (dynamic), and the id otherwise.
fn scan_asyncapi_v3(lines: &[&str]) -> Vec<ChannelOp> {
    struct Pending<'a> {
        id: &'a str,
        line: u32,
        action: Option<&'static str>,
        channel_ref: Option<String>,
    }

    let mut ops: Vec<ChannelOp> = Vec::new();
    let Some(start) = top_level_block(lines, "operations") else {
        return ops;
    };
    let Some(op_indent) = first_child_indent(lines, start) else {
        return ops;
    };
    let addresses = v3_addresses(lines);
    let finish = |p: Pending<'_>, ops: &mut Vec<ChannelOp>| {
        let (Some(action), Some(id)) = (p.action, p.channel_ref) else {
            return;
        };
        let channel = match addresses.iter().find(|(k, _)| *k == id) {
            Some((_, Address::Declared(a))) => (*a).to_string(),
            Some((_, Address::Null)) => return,
            None => id,
        };
        if ops.len() < MAX_OPS {
            ops.push(ChannelOp {
                action,
                channel,
                operation_id: Some(p.id.to_string()),
                line: p.line,
            });
        }
    };

    let mut cur: Option<Pending<'_>> = None;
    let mut body_indent: Option<usize> = None;
    // Indent of a block-style `channel:` key whose `$ref` is on a later line.
    let mut channel_block: Option<usize> = None;

    for (i, line) in lines.iter().enumerate().skip(start + 1) {
        if is_skippable(line) {
            continue;
        }
        let ind = indent_of(line);
        if ind == 0 {
            break;
        }
        if channel_block.is_some_and(|c| ind <= c) {
            channel_block = None;
        }
        if ind < op_indent {
            continue;
        }
        if ind == op_indent {
            if let Some(p) = cur.take() {
                finish(p, &mut ops);
            }
            cur = split_key(line).map(|(k, _)| Pending {
                id: k,
                line: i as u32,
                action: None,
                channel_ref: None,
            });
            body_indent = None;
            channel_block = None;
            continue;
        }
        let Some(p) = cur.as_mut() else { continue };
        let Some((key, raw)) = split_key(line) else { continue };

        if let Some(c) = channel_block {
            // Only the `$ref` that is a direct child of `channel:`.
            if ind > c && key == "$ref" && p.channel_ref.is_none() {
                p.channel_ref = v3_channel_ref(raw);
            }
            continue;
        }
        if ind != *body_indent.get_or_insert(ind) {
            continue;
        }
        match key {
            "action" => {
                p.action = match value_of(line) {
                    "send" => Some("publish"),
                    "receive" => Some("subscribe"),
                    _ => None,
                };
            }
            "channel" if raw.is_empty() => channel_block = Some(ind),
            "channel" => p.channel_ref = flow_ref(raw).and_then(v3_channel_ref),
            _ => {}
        }
    }
    if let Some(p) = cur.take() {
        finish(p, &mut ops);
    }
    ops
}

/// Push one contract-operation node. Every contract format shares this shape:
/// DOC_SECTION + CODE / POSITION / ORIGIN cells, parented to the module.
#[allow(clippy::too_many_arguments)]
pub(crate) fn push_op(
    out: &mut ContractNodes,
    qname: &str,
    name: &str,
    operation_id: Option<&str>,
    path: &str,
    line: u32,
    origin: String,
    module_id: NodeId,
    repo: RepoId,
) {
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::DOC_SECTION, qname);
    let code = match operation_id {
        Some(oid) => format!("{name} — {oid}"),
        None => name.to_string(),
    };
    let pos = format!(
        r#"{{"file":"{}","start_line":{},"end_line":{}}}"#,
        esc(path),
        line,
        line
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
        .record(id, name, qname, node_kind::DOC_SECTION, Some(module_id));
}

/// `,"operation_id":"<id>"`, or nothing when the spec names none (the field is
/// OMITTED, never null).
pub(crate) fn operation_id_field(oid: Option<&str>) -> String {
    match oid {
        Some(oid) => format!(r#","operation_id":"{}""#, esc(oid)),
        None => String::new(),
    }
}

/// Sniff a `.yaml`/`.yml` and, when it is an API contract, emit one node per
/// declared operation. A non-contract yaml — the overwhelming majority — takes
/// the allocation-free miss path and returns empty.
///
/// The node kind is the EXISTING `DOC_SECTION` (42), deliberately not a new
/// kind: `governing_docs` / `glia docs-for` then answer with contract ops for
/// free, engram-export already maps DOC_SECTION → `Content::Proposition`, and
/// pyo3 decodes it today. The `ORIGIN` cell (`provenance=contract`) is the
/// discriminator every downstream pass keys on: an OpenAPI op carries
/// `method` + `path`, an AsyncAPI op carries `action` + `channel` and neither
/// of the HTTP keys — that absence is how the pairing pass tells them apart.
pub fn extract_yaml_contracts(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
) -> ContractNodes {
    let mut out = ContractNodes::default();
    let Some(kind) = sniff(source) else {
        return out;
    };
    let stem = file_stem(path);
    match kind {
        Sniffed::OpenApi => {
            out.source = Some(ContractSource::OpenApi);
            emit_openapi(&mut out, scan_openapi(source), stem, path, module_id, repo);
        }
        Sniffed::AsyncApi => {
            out.source = Some(ContractSource::AsyncApi);
            emit_asyncapi(&mut out, scan_asyncapi(source), stem, path, module_id, repo);
        }
    }
    out
}

/// The qname segment a contract file contributes: `openapi` for `openapi.yaml`.
pub(crate) fn file_stem(path: &str) -> &str {
    std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("spec")
}

/// One DOC_SECTION per HTTP operation. The yaml and JSON paths both end here,
/// so an `openapi.json` op is byte-for-byte the node its yaml twin would be.
fn emit_openapi(
    out: &mut ContractNodes,
    ops: Vec<Op>,
    stem: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
) {
    for op in ops {
        let qname = format!("contract::{stem}::{}:{}", op.method, op.path);
        let name = format!("{} {}", op.method, op.path);
        let origin = format!(
            r#"{{"provenance":"contract","source":"openapi","method":"{}","path":"{}","raw_path":"{}"{}}}"#,
            esc(&op.method),
            esc(&op.path),
            esc(&op.raw_path),
            operation_id_field(op.operation_id.as_deref())
        );
        let oid = op.operation_id.as_deref();
        push_op(out, &qname, &name, oid, path, op.line, origin, module_id, repo);
    }
}

/// One DOC_SECTION per AsyncAPI channel operation; shared like [`emit_openapi`].
fn emit_asyncapi(
    out: &mut ContractNodes,
    ops: Vec<ChannelOp>,
    stem: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
) {
    for op in ops {
        let qname = format!("contract::{stem}::{}:{}", op.action, op.channel);
        let name = format!("{} {}", op.action, op.channel);
        let origin = format!(
            r#"{{"provenance":"contract","source":"asyncapi","action":"{}","channel":"{}"{}}}"#,
            op.action,
            esc(&op.channel),
            operation_id_field(op.operation_id.as_deref())
        );
        let oid = op.operation_id.as_deref();
        push_op(out, &qname, &name, oid, path, op.line, origin, module_id, repo);
    }
}

// ----------------------------------------------------------------------
// A10.8 — contracts shipped as JSON
// ----------------------------------------------------------------------

/// How much of EACH end of a `.json` the sniff reads. Both ends, because key
/// order is up to the producer: pact-js v10+ / pact-python v3 / pact-go v2
/// all write through pact_ffi, whose keys come out sorted, so `"provider"`
/// lands after every interaction, past any head-only window.
const JSON_SNIFF_BYTES: usize = 8 * 1024;

/// Largest char boundary `<= i` (clamped to `s.len()`).
fn floor_boundary(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Offset in `hay` of the first `quoted` (a JSON-encoded string, quotes
/// included) in KEY position, i.e. followed by optional whitespace and `:`.
/// The same word as a value (`"note": "openapi"`) is not a key.
fn key_pos(hay: &str, quoted: &str) -> Option<usize> {
    hay.match_indices(quoted)
        .map(|(i, _)| i)
        .find(|&i| hay[i + quoted.len()..].trim_start().starts_with(':'))
}

/// Cheap content gate for a `.json`: is it an API contract, and which kind? No
/// JSON parse. The format marker (`"openapi"` / `"swagger"` / `"asyncapi"`,
/// or Pact's `"interactions"` + `"consumer"` + `"provider"`) must sit in the
/// first or last [`JSON_SNIFF_BYTES`], so a miss costs at most 16 KiB of
/// scanning. Only a file that carries an OpenAPI/AsyncAPI marker is searched
/// in full for its companion key (`"paths"` / `"channels"`), because a long
/// `info.description` can push it out of the head.
///
/// This is a gate and does not validate anything. A document that merely
/// contains these keys somewhere is admitted, and the structural parse in
/// [`extract_json_contract`] then emits nothing, so the cost is one wasted
/// parse and never a wrong node.
pub fn sniff_json_contract(text: &str) -> Option<ContractSource> {
    let body = text.trim_start_matches('\u{feff}');
    if !body.trim_start().starts_with('{') {
        return None; // an array, JSONL or JSONC comment header is never a contract
    }
    let head = &text[..floor_boundary(text, JSON_SNIFF_BYTES)];
    let tail = if text.len() > 2 * JSON_SNIFF_BYTES {
        &text[floor_boundary(text, text.len() - JSON_SNIFF_BYTES)..]
    } else {
        ""
    };
    let ends = |k: &str| key_pos(head, k).is_some() || key_pos(tail, k).is_some();
    if ends("\"openapi\"") || ends("\"swagger\"") {
        return key_pos(text, "\"paths\"").map(|_| ContractSource::OpenApi);
    }
    if ends("\"asyncapi\"") {
        return key_pos(text, "\"channels\"").map(|_| ContractSource::AsyncApi);
    }
    (ends("\"interactions\"") && ends("\"consumer\"") && ends("\"provider\""))
        .then_some(ContractSource::Pact)
}

/// 0-indexed line lookup over the byte offsets of every `\n`.
struct LineIndex(Vec<usize>);

impl LineIndex {
    fn new(s: &str) -> Self {
        LineIndex(s.match_indices('\n').map(|(i, _)| i).collect())
    }

    /// The line an offset sits on; an unlocated key (`None`) is line 0.
    fn line(&self, off: Option<usize>) -> u32 {
        off.map_or(0, |o| self.0.partition_point(|&n| n < o) as u32)
    }
}

/// Offset of the first `key` in KEY position within `source[from..to]`.
/// POSITION and emission order both come from it. serde_json's map may or may
/// not keep document order, depending on which features the build unifies, so
/// document order is recovered from the text.
fn json_key_at(source: &str, from: usize, to: usize, key: &str) -> Option<usize> {
    let quoted = serde_json::to_string(key).ok()?;
    let to = to.min(source.len());
    let hay = source.get(from.min(to)..to)?;
    key_pos(hay, &quoted).map(|i| from + i)
}

type JsonMap = serde_json::Map<String, serde_json::Value>;

/// The object members of `map` that `keep` accepts, in document order: each is
/// located after `from`, and a member that cannot be located sorts last (by
/// key, so the order never depends on the map). Returned with the byte range
/// its own body occupies (up to the next member), for locating its children.
fn located<'a, T>(
    source: &str,
    from: usize,
    map: &'a JsonMap,
    mut keep: impl FnMut(&'a str, &'a serde_json::Value) -> Option<T>,
) -> Vec<(Option<usize>, usize, &'a str, T)> {
    let mut v: Vec<(Option<usize>, usize, &str, T)> = map
        .iter()
        .filter_map(|(k, val)| {
            let t = keep(k, val)?;
            Some((json_key_at(source, from, source.len(), k), 0, k.as_str(), t))
        })
        .collect();
    v.sort_by(|a, b| {
        (a.0.unwrap_or(usize::MAX), a.2).cmp(&(b.0.unwrap_or(usize::MAX), b.2))
    });
    for i in 0..v.len() {
        v[i].1 = v.get(i + 1).and_then(|n| n.0).unwrap_or(source.len());
    }
    v
}

/// `paths` → each path item → each allow-listed method, in document order.
/// `servers[0].url` (OpenAPI 3) or `basePath` (Swagger 2) is folded exactly as
/// the yaml path folds it.
fn json_openapi_ops(source: &str, doc: &JsonMap, lines: &LineIndex) -> Vec<Op> {
    use serde_json::Value;
    let mut ops = Vec::new();
    if !doc.contains_key("openapi") && !doc.contains_key("swagger") {
        return ops;
    }
    let Some(paths) = doc.get("paths").and_then(Value::as_object) else {
        return ops;
    };
    let base = doc
        .get("servers")
        .and_then(Value::as_array)
        .and_then(|s| s.first())
        .and_then(|s| s.get("url"))
        .and_then(Value::as_str)
        .or_else(|| doc.get("basePath").and_then(Value::as_str))
        .map(fold_server_base)
        .unwrap_or_default();
    let start = json_key_at(source, 0, source.len(), "paths").unwrap_or(0);
    let items = located(source, start, paths, |k, v| {
        k.starts_with('/').then_some(())?;
        v.as_object()
    });
    for (off, end, key, item) in items {
        let from = off.unwrap_or(source.len());
        let methods = located(source, from, item, |m, body| {
            let lower = m.to_ascii_lowercase();
            let verb = METHODS.iter().copied().find(|v| *v == lower)?;
            Some((verb, body))
        });
        for (moff, _, _, (verb, body)) in methods {
            if ops.len() >= MAX_OPS {
                return ops;
            }
            // A method key located past its own path item's range belongs to
            // a later item; fall back to the path key's line.
            let moff = moff.filter(|m| *m < end).or(off);
            ops.push(Op {
                method: verb.to_ascii_uppercase(),
                path: join_path(&base, key),
                raw_path: key.to_string(),
                operation_id: body
                    .get("operationId")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
                line: lines.line(moff),
            });
        }
    }
    ops
}

/// AsyncAPI JSON: v2 `channels.<name>.publish|subscribe`, or v3 `operations`
/// (`send` / `receive` mapped onto the same two verbs), with the same rules as
/// the yaml scanners.
fn json_asyncapi_ops(source: &str, doc: &JsonMap, lines: &LineIndex) -> Vec<ChannelOp> {
    use serde_json::Value;
    let mut ops = Vec::new();
    let Some(version) = doc.get("asyncapi").and_then(Value::as_str) else {
        return ops;
    };
    let channels = doc.get("channels").and_then(Value::as_object);
    if version.split('.').next() == Some("3") {
        let Some(operations) = doc.get("operations").and_then(Value::as_object) else {
            return ops;
        };
        let start = json_key_at(source, 0, source.len(), "operations").unwrap_or(0);
        for (off, _, id, op) in located(source, start, operations, |_, v| v.as_object()) {
            let action = match op.get("action").and_then(Value::as_str) {
                Some("send") => "publish",
                Some("receive") => "subscribe",
                _ => continue,
            };
            let Some(chan_id) = op
                .get("channel")
                .and_then(|c| c.get("$ref"))
                .and_then(Value::as_str)
                .and_then(v3_channel_ref)
            else {
                continue;
            };
            let channel = match channels.and_then(|c| c.get(&chan_id)).and_then(|c| c.get("address")) {
                Some(Value::String(a)) if !a.is_empty() => a.clone(),
                Some(Value::String(_) | Value::Null) => continue, // dynamic channel
                _ => chan_id,
            };
            ops.push(ChannelOp {
                action,
                channel,
                operation_id: Some(id.to_string()),
                line: lines.line(off),
            });
        }
    } else if let Some(channels) = channels {
        let start = json_key_at(source, 0, source.len(), "channels").unwrap_or(0);
        let chans = located(source, start, channels, |k, v| {
            (!k.is_empty()).then_some(())?;
            v.as_object()
        });
        for (off, end, chan, item) in chans {
            let from = off.unwrap_or(source.len());
            let acts = located(source, from, item, |k, v| {
                let action = ["publish", "subscribe"].into_iter().find(|a| *a == k)?;
                Some((action, v.as_object()?))
            });
            for (aoff, _, _, (action, body)) in acts {
                let aoff = aoff.filter(|a| *a < end).or(off);
                ops.push(ChannelOp {
                    action,
                    channel: chan.to_string(),
                    operation_id: body
                        .get("operationId")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string),
                    line: lines.line(aoff),
                });
            }
        }
    }
    dedup_channel_ops(&mut ops);
    ops
}

/// One Pact interaction's HTTP request.
#[derive(Debug, PartialEq, Eq)]
struct PactOp {
    method: String,
    /// `request.path` with any query string dropped. Pact paths are concrete
    /// (`/users/42`), so they pair only with a ROUTE that is concrete too.
    path: String,
    /// `request.path` exactly as written.
    raw_path: String,
    description: Option<String>,
    /// 0-indexed line of the `request.path` literal.
    line: u32,
}

/// Pact `interactions[]` → each entry's `request.method` + `request.path`. An
/// entry missing either (a v4 message interaction), or with a method outside
/// the HTTP allow-list, emits nothing. Two interactions on one request (the
/// same call under two provider states) are one operation, and the first one wins.
fn json_pact_ops(source: &str, doc: &JsonMap, lines: &LineIndex) -> Vec<PactOp> {
    use serde_json::Value;
    let mut ops: Vec<PactOp> = Vec::new();
    let Some(interactions) = doc.get("interactions").and_then(Value::as_array) else {
        return ops;
    };
    // An array keeps document order, so one forward cursor locates every path.
    let mut cursor = json_key_at(source, 0, source.len(), "interactions").unwrap_or(0);
    for it in interactions {
        let req = it.get("request");
        let method = req.and_then(|r| r.get("method")).and_then(Value::as_str);
        let raw = req.and_then(|r| r.get("path")).and_then(Value::as_str);
        let (Some(method), Some(raw)) = (method, raw) else {
            continue;
        };
        let method = method.to_ascii_uppercase();
        let path = raw.split('?').next().unwrap_or(raw);
        if !path.starts_with('/') || !METHODS.iter().any(|m| m.eq_ignore_ascii_case(&method)) {
            continue;
        }
        let at = serde_json::to_string(raw)
            .ok()
            .and_then(|lit| source.get(cursor..)?.find(&lit).map(|i| (cursor + i, lit.len())));
        if let Some((i, len)) = at {
            cursor = i + len;
        }
        if ops.len() >= MAX_OPS {
            break;
        }
        if ops.iter().any(|o| o.method == method && o.path == path) {
            continue;
        }
        ops.push(PactOp {
            method,
            path: path.to_string(),
            raw_path: raw.to_string(),
            description: it.get("description").and_then(Value::as_str).map(str::to_string),
            line: lines.line(at.map(|(i, _)| i)),
        });
    }
    ops
}

/// A JSON string literal, fully escaped (control characters included). Pact
/// descriptions are free text, and `esc` escapes only `\` and `"`.
fn json_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| String::from("\"\""))
}

/// Sniff a `.json` and, when it is an API contract, emit one node per declared
/// operation, with the same DOC_SECTION shape the yaml path emits. Pact ops
/// carry ORIGIN `{provenance: contract, source: pact, method, path, raw_path}`
/// plus `description` / `consumer` / `provider` when the file declares them,
/// so the engine's pairing pass reads them as HTTP ops with no change. A file
/// that sniffs but does not parse, or declares nothing, yields nothing.
pub fn extract_json_contract(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
) -> ContractNodes {
    use serde_json::Value;
    let mut out = ContractNodes::default();
    let Some(kind) = sniff_json_contract(source) else {
        return out;
    };
    let Ok(Value::Object(doc)) = serde_json::from_str::<Value>(source.trim_start_matches('\u{feff}'))
    else {
        return out;
    };
    let lines = LineIndex::new(source);
    let stem = file_stem(path);
    out.source = Some(kind);
    match kind {
        ContractSource::OpenApi => {
            emit_openapi(&mut out, json_openapi_ops(source, &doc, &lines), stem, path, module_id, repo);
        }
        ContractSource::AsyncApi => {
            emit_asyncapi(&mut out, json_asyncapi_ops(source, &doc, &lines), stem, path, module_id, repo);
        }
        ContractSource::Pact => {
            let party = |k: &str| {
                doc.get(k)
                    .and_then(|p| p.get("name"))
                    .and_then(Value::as_str)
                    .map(|n| format!(r#","{k}":{}"#, json_str(n)))
                    .unwrap_or_default()
            };
            let parties = format!("{}{}", party("consumer"), party("provider"));
            for op in json_pact_ops(source, &doc, &lines) {
                let qname = format!("contract::{stem}::{}:{}", op.method, op.path);
                let name = format!("{} {}", op.method, op.path);
                let description = op
                    .description
                    .as_deref()
                    .map(|d| format!(r#","description":{}"#, json_str(d)))
                    .unwrap_or_default();
                let origin = format!(
                    r#"{{"provenance":"contract","source":"pact","method":{},"path":{},"raw_path":{}{description}{parties}}}"#,
                    json_str(&op.method),
                    json_str(&op.path),
                    json_str(&op.raw_path),
                );
                let desc = op.description.as_deref();
                push_op(&mut out, &qname, &name, desc, path, op.line, origin, module_id, repo);
            }
        }
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
    // ------------------------------------------------------------------
    // A10.3 — AsyncAPI channel operations
    // ------------------------------------------------------------------

    fn origin_json(node: &Node) -> &str {
        match &node
            .cells
            .iter()
            .find(|c| c.kind == cell_type::ORIGIN)
            .expect("contract ops carry an ORIGIN discriminator")
            .payload
        {
            CellPayload::Json(j) => j.as_str(),
            other => panic!("ORIGIN must be Json, got {other:?}"),
        }
    }

    #[test]
    fn asyncapi_v2_channels_yield_one_op_per_publish_subscribe() {
        // Two channels; `orders` declares both directions, `user/signedup`
        // only subscribes. Channel-level `description:` / `parameters:` and the
        // nested `message:` bodies must open nothing.
        let source = r#"asyncapi: 2.6.0
info:
  title: Orders
  version: 1.0.0
channels:
  orders:
    description: order events
    publish:
      operationId: publishOrder
      message:
        $ref: '#/components/messages/Order'
    subscribe:
      operationId: onOrder
      message:
        payload:
          type: object
          properties:
            publish:
              type: string
  'user/signedup':
    parameters:
      userId:
        description: who
    subscribe:
      message:
        name: UserSignedUp
components:
  messages:
    Order:
      payload:
        type: object
"#;
        let out = extract_yaml_contracts(source, "specs/asyncapi.yaml", module_id(), repo());
        assert_eq!(out.source, Some(ContractSource::AsyncApi));
        let qnames: Vec<&str> = out
            .nodes
            .iter()
            .map(|n| out.nav.qname_by_id[&n.id].as_str())
            .collect();
        assert_eq!(
            qnames,
            vec![
                "contract::asyncapi::publish:orders",
                "contract::asyncapi::subscribe:orders",
                "contract::asyncapi::subscribe:user/signedup",
            ]
        );
        let first = &out.nodes[0];
        assert_eq!(out.nav.name_by_id[&first.id], "publish orders");
        assert_eq!(out.nav.kind_by_id[&first.id], node_kind::DOC_SECTION);
        assert_eq!(out.nav.parent_of[&first.id], module_id());

        let j = origin_json(first);
        assert_eq!(
            j,
            r#"{"provenance":"contract","source":"asyncapi","action":"publish","channel":"orders","operation_id":"publishOrder"}"#
        );
        // No HTTP keys: that absence is the pairing pass's discriminator.
        assert!(!j.contains("\"method\"") && !j.contains("\"path\""), "got {j}");
        // No operationId declared → the field is omitted, not null.
        assert!(!origin_json(&out.nodes[2]).contains("operation_id"));

        let ops = scan_asyncapi(source);
        assert_eq!(ops[0].line, 7, "`publish:` is the 8th line => 0-indexed 7");
        assert_eq!(ops[1].operation_id.as_deref(), Some("onOrder"));
        assert_eq!(ops[2].operation_id, None);
    }

    #[test]
    fn openapi_doc_never_enters_the_asyncapi_arm() {
        // The OpenAPI doc has a `channels:`-looking nothing and a `publish`
        // path; it must still come out as HTTP ops only.
        let source = r#"openapi: 3.0.3
paths:
  /publish:
    post:
      operationId: publish
      responses:
        '200':
          description: ok
"#;
        let out = extract_yaml_contracts(source, "openapi.yaml", module_id(), repo());
        assert_eq!(out.source, Some(ContractSource::OpenApi));
        assert_eq!(out.nodes.len(), 1);
        let j = origin_json(&out.nodes[0]);
        assert!(j.contains(r#""source":"openapi""#), "got {j}");
        assert!(!j.contains("\"action\"") && !j.contains("\"channel\""), "got {j}");
        assert!(scan_asyncapi(source).is_empty());

        // And an AsyncAPI doc yields no HTTP ops.
        let async_src = "asyncapi: 2.0.0\nchannels:\n  orders:\n    publish:\n      message: {}\n";
        assert!(scan_openapi(async_src).is_empty());
        let out = extract_yaml_contracts(async_src, "events.yml", module_id(), repo());
        assert_eq!(out.source, Some(ContractSource::AsyncApi));
        assert_eq!(out.nav.qname_by_id[&out.nodes[0].id], "contract::events::publish:orders");
    }

    #[test]
    fn asyncapi_v3_operations_map_send_receive_onto_publish_subscribe() {
        let source = r#"asyncapi: 3.0.0
info:
  title: Accounts
  version: 1.0.0
channels:
  userSignedup:
    address: 'user/signedup'
    messages:
      UserSignedUp:
        $ref: '#/components/messages/UserSignedUp'
  orders:
    messages: {}
  dynamic:
    address: null
operations:
  sendUserSignedup:
    action: send
    channel:
      $ref: '#/channels/userSignedup'
    messages:
      - $ref: '#/channels/orders/messages/Nope'
  onOrders:
    action: receive
    channel: { $ref: '#/channels/orders' }
  sendDynamic:
    action: send
    channel:
      $ref: '#/channels/dynamic'
  sendElsewhere:
    action: send
    channel:
      $ref: './other.yaml#/channels/orders'
  duplicateSend:
    action: send
    channel:
      $ref: '#/channels/userSignedup'
  weird:
    action: publish
    channel:
      $ref: '#/channels/orders'
"#;
        let ops = scan_asyncapi(source);
        let got: Vec<(&str, &str, Option<&str>)> = ops
            .iter()
            .map(|o| (o.action, o.channel.as_str(), o.operation_id.as_deref()))
            .collect();
        assert_eq!(
            got,
            vec![
                // send → publish; the channel is its declared ADDRESS, not its id.
                ("publish", "user/signedup", Some("sendUserSignedup")),
                // receive → subscribe; flow-style ref; no address → the id.
                ("subscribe", "orders", Some("onOrders")),
                // `address: null` (dynamic), a cross-file ref, a duplicate
                // (action, channel), and a v2 verb under v3 all emit nothing.
            ]
        );
        assert_eq!(ops[0].line, 15, "the operation id key's 0-indexed line");

        let out = extract_yaml_contracts(source, "asyncapi.yaml", module_id(), repo());
        assert_eq!(
            out.nav.qname_by_id[&out.nodes[0].id],
            "contract::asyncapi::publish:user/signedup"
        );
        assert_eq!(out.nav.name_by_id[&out.nodes[0].id], "publish user/signedup");
    }

    #[test]
    fn asyncapi_v3_json_pointer_is_unescaped() {
        let source = r##"asyncapi: 3.1.0
operations:
  send:
    action: send
    channel:
      $ref: "#/channels/user~1signedup"
"##;
        let ops = scan_asyncapi(source);
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].channel, "user/signedup");
    }

    #[test]
    fn contract_counts_record_by_source() {
        let mut c = ContractCounts::default();
        let openapi = "openapi: 3.0.0\npaths:\n  /a:\n    get:\n      summary: x\n    put:\n      summary: y\n";
        let asyncapi = "asyncapi: 2.6.0\nchannels:\n  orders:\n    subscribe:\n      summary: x\n";
        let empty_async = "asyncapi: 2.6.0\ninfo:\n  title: nothing declared\n";
        let k8s = "apiVersion: v1\nkind: Service\n";
        for src in [openapi, asyncapi, empty_async, k8s] {
            c.record(&extract_yaml_contracts(src, "x.yaml", module_id(), repo()));
        }
        assert_eq!((c.files, c.openapi, c.asyncapi, c.pact), (2, 2, 1, 0));
        assert_eq!(extract_yaml_contracts(k8s, "x.yaml", module_id(), repo()).source, None);
    }

    // ------------------------------------------------------------------
    // A10.8 — contracts shipped as JSON
    // ------------------------------------------------------------------

    fn cell_text(node: &Node, kind: repo_graph_core::CellTypeId) -> &str {
        match &node.cells.iter().find(|c| c.kind == kind).expect("cell present").payload {
            CellPayload::Json(s) | CellPayload::Text(s) => s.as_str(),
            other => panic!("unexpected payload {other:?}"),
        }
    }

    #[test]
    fn json_sniff_rejects_non_contract_json() {
        let package_lock = r#"{
  "name": "shop",
  "lockfileVersion": 3,
  "requires": true,
  "packages": {
    "": { "name": "shop", "dependencies": { "swagger-ui": "^5.0.0" } },
    "node_modules/swagger-ui": { "version": "5.0.0", "resolved": "https://x/openapi" }
  }
}"#;
        // tsconfig carries a real `"paths"` KEY; it has no format marker.
        let tsconfig = r#"{
  "compilerOptions": {
    "baseUrl": ".",
    "paths": { "@app/*": ["src/app/*"] }
  }
}"#;
        // A substrate-gap key.json: the words appear only as VALUES.
        let key_json = r#"{
  "framework": "contract-pact",
  "language": "json+python",
  "dirs": ["."],
  "expect_nodes": [{"kind": "DOC_SECTION", "name": "GET /users", "note": "openapi"}],
  "expect_edges": [{"from": "consumer", "to": "provider", "category": "DOCUMENTS", "note": "interactions"}],
  "note": "\"paths\": is prose here, and so is \"swagger\""
}"#;
        let array = r#"[{"openapi": "3.0.0", "paths": {}}]"#;
        for (what, src) in [
            ("package-lock", package_lock),
            ("tsconfig", tsconfig),
            ("key.json", key_json),
            ("top-level array", array),
            ("empty", ""),
        ] {
            assert_eq!(sniff_json_contract(src), None, "{what} must not sniff");
            let out = extract_json_contract(src, "x.json", module_id(), repo());
            assert!(out.nodes.is_empty() && out.source.is_none(), "{what}");
        }
    }

    #[test]
    fn json_sniff_accepts_each_contract_kind() {
        let openapi = "\u{feff}{\"openapi\": \"3.1.0\", \"paths\": {}}";
        let swagger = r#"{"swagger":"2.0","paths":{}}"#;
        let asyncapi = r#"{"asyncapi": "2.6.0", "channels": {}}"#;
        let pact = r#"{"consumer":{"name":"web"},"provider":{"name":"api"},"interactions":[]}"#;
        assert_eq!(sniff_json_contract(openapi), Some(ContractSource::OpenApi));
        assert_eq!(sniff_json_contract(swagger), Some(ContractSource::OpenApi));
        assert_eq!(sniff_json_contract(asyncapi), Some(ContractSource::AsyncApi));
        assert_eq!(sniff_json_contract(pact), Some(ContractSource::Pact));

        // A pact_ffi pact is key-sorted, so `"provider"` comes after every
        // interaction and falls outside the first 8 KiB.
        let one = r#"{"description":"a request for users","request":{"method":"GET","path":"/users"},"response":{"status":200}}"#;
        let many = vec![one; 200].join(",");
        let sorted_pact = format!(
            r#"{{"consumer":{{"name":"web"}},"interactions":[{many}],"metadata":{{"pactSpecification":{{"version":"3.0.0"}}}},"provider":{{"name":"api"}}}}"#
        );
        assert!(sorted_pact.find("\"provider\"").unwrap() > JSON_SNIFF_BYTES);
        assert_eq!(sniff_json_contract(&sorted_pact), Some(ContractSource::Pact));
        let out = extract_json_contract(&sorted_pact, "pacts/web-api.json", module_id(), repo());
        assert_eq!(out.nodes.len(), 1, "200 interactions on one request are one op");

        // The same text, but the marker sits in neither window: not sniffed.
        let buried = format!(r#"{{"a":"{}","openapi":"3.0.0","b":"{}","paths":{{}}}}"#, "x".repeat(9000), "y".repeat(9000));
        assert_eq!(sniff_json_contract(&buried), None);
        // A long `info.description` pushes `"paths"` out of the head; the
        // marker is in the head, so the companion key is searched in full.
        let long_info = format!(
            r#"{{"openapi":"3.0.0","info":{{"description":"{}"}},"paths":{{"/a":{{"get":{{}}}}}},"components":{{"x":"{}"}}}}"#,
            "d".repeat(9000),
            "c".repeat(9000)
        );
        assert_eq!(sniff_json_contract(&long_info), Some(ContractSource::OpenApi));
    }

    #[test]
    fn openapi_json_emits_the_yaml_nodes_in_document_order() {
        // `/b` before `/a`, and `post` before `get`: emission follows the
        // document, whatever order serde_json's map iterates in.
        let json = r#"{
  "openapi": "3.0.3",
  "servers": [{"url": "https://api.example.com/v1"}],
  "paths": {
    "/b": {
      "parameters": [],
      "post": {"operationId": "makeB"},
      "get": {"operationId": "getB"}
    },
    "/a": {
      "get": {"responses": {"200": {"description": "ok"}}}
    },
    "x-not-a-path": {"get": {}}
  }
}"#;
        let yaml = r#"openapi: 3.0.3
servers:
  - url: https://api.example.com/v1
paths:
  /b:
    parameters: []
    post:
      operationId: makeB
    get:
      operationId: getB
  /a:
    get:
      responses:
        '200':
          description: ok
"#;
        let j = extract_json_contract(json, "api/openapi.json", module_id(), repo());
        let y = extract_yaml_contracts(yaml, "api/openapi.yaml", module_id(), repo());
        assert_eq!(j.source, Some(ContractSource::OpenApi));
        let shape = |out: &ContractNodes| -> Vec<(String, String, String, String)> {
            out.nodes
                .iter()
                .map(|n| {
                    (
                        out.nav.qname_by_id[&n.id].clone(),
                        out.nav.name_by_id[&n.id].clone(),
                        cell_text(n, cell_type::CODE).to_string(),
                        origin_json(n).to_string(),
                    )
                })
                .collect()
        };
        assert_eq!(shape(&j), shape(&y), "the JSON path emits the yaml path's nodes");
        let qnames: Vec<String> = shape(&j).into_iter().map(|t| t.0).collect();
        assert_eq!(
            qnames,
            [
                "contract::openapi::POST:/v1/b",
                "contract::openapi::GET:/v1/b",
                "contract::openapi::GET:/v1/a",
            ]
        );
        assert_eq!(j.nodes[0].id, y.nodes[0].id, "same stem, same qname, same id");
        assert_eq!(j.nav.parent_of[&j.nodes[0].id], module_id());
        // POSITION is the method key's line (`"post"` is the 7th line).
        assert!(cell_text(&j.nodes[0], cell_type::POSITION).contains(r#""file":"api/openapi.json","start_line":6"#));
        assert!(cell_text(&j.nodes[2], cell_type::POSITION).contains(r#""start_line":10"#));
    }

    #[test]
    fn swagger2_json_folds_base_path() {
        let json = r#"{"swagger":"2.0","basePath":"/v2","paths":{"/pets/{id}":{"get":{"operationId":"getPet"}}}}"#;
        let out = extract_json_contract(json, "swagger.json", module_id(), repo());
        assert_eq!(out.nodes.len(), 1);
        assert_eq!(out.nav.qname_by_id[&out.nodes[0].id], "contract::swagger::GET:/v2/pets/{id}");
        assert_eq!(
            origin_json(&out.nodes[0]),
            r#"{"provenance":"contract","source":"openapi","method":"GET","path":"/v2/pets/{id}","raw_path":"/pets/{id}","operation_id":"getPet"}"#
        );
        // Minified: everything is on line 0.
        assert!(cell_text(&out.nodes[0], cell_type::POSITION).contains(r#""start_line":0"#));
    }

    #[test]
    fn asyncapi_json_v2_and_v3_match_the_yaml_shape() {
        let v2 = r#"{
  "asyncapi": "2.6.0",
  "channels": {
    "orders": {
      "description": "order events",
      "subscribe": {"operationId": "onOrder"},
      "publish": {"operationId": "publishOrder", "message": {"payload": {"properties": {"publish": {}}}}}
    },
    "user/signedup": {"subscribe": {"message": {"name": "UserSignedUp"}}}
  }
}"#;
        let out = extract_json_contract(v2, "asyncapi.json", module_id(), repo());
        assert_eq!(out.source, Some(ContractSource::AsyncApi));
        let qnames: Vec<&str> = out.nodes.iter().map(|n| out.nav.qname_by_id[&n.id].as_str()).collect();
        assert_eq!(
            qnames,
            [
                "contract::asyncapi::subscribe:orders",
                "contract::asyncapi::publish:orders",
                "contract::asyncapi::subscribe:user/signedup",
            ]
        );
        assert_eq!(
            origin_json(&out.nodes[1]),
            r#"{"provenance":"contract","source":"asyncapi","action":"publish","channel":"orders","operation_id":"publishOrder"}"#
        );
        assert!(!origin_json(&out.nodes[2]).contains("operation_id"));
        assert!(cell_text(&out.nodes[1], cell_type::POSITION).contains(r#""start_line":6"#));

        let v3 = r##"{
  "asyncapi": "3.0.0",
  "channels": {
    "userSignedup": {"address": "user/signedup"},
    "orders": {"messages": {}},
    "dynamic": {"address": null}
  },
  "operations": {
    "sendUserSignedup": {"action": "send", "channel": {"$ref": "#/channels/userSignedup"}},
    "onOrders": {"action": "receive", "channel": {"$ref": "#/channels/orders"}},
    "sendDynamic": {"action": "send", "channel": {"$ref": "#/channels/dynamic"}},
    "sendElsewhere": {"action": "send", "channel": {"$ref": "./other.json#/channels/orders"}},
    "duplicateSend": {"action": "send", "channel": {"$ref": "#/channels/userSignedup"}},
    "weird": {"action": "publish", "channel": {"$ref": "#/channels/orders"}}
  }
}"##;
        let out = extract_json_contract(v3, "events.json", module_id(), repo());
        let got: Vec<(&str, &str)> = out
            .nodes
            .iter()
            .map(|n| (out.nav.qname_by_id[&n.id].as_str(), out.nav.name_by_id[&n.id].as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("contract::events::publish:user/signedup", "publish user/signedup"),
                ("contract::events::subscribe:orders", "subscribe orders"),
            ]
        );
        assert!(origin_json(&out.nodes[0]).ends_with(r#""operation_id":"sendUserSignedup"}"#));
    }

    #[test]
    fn pact_interactions_become_http_contract_ops() {
        let pact = r#"{
  "consumer": {"name": "web"},
  "provider": {"name": "api"},
  "interactions": [
    {
      "description": "a request for users",
      "providerState": "users exist",
      "request": {"method": "get", "path": "/users", "query": "page=1"},
      "response": {"status": 200}
    },
    {
      "description": "a request for users when there are none",
      "request": {"method": "GET", "path": "/users"},
      "response": {"status": 200, "body": []}
    },
    {
      "description": "one user\nby id",
      "request": {"method": "GET", "path": "/users/42?fields=name"},
      "response": {"status": 200}
    },
    {"description": "no path", "request": {"method": "DELETE"}},
    {"description": "not http", "request": {"method": "CONNECT", "path": "/x"}},
    {"description": "user created event", "type": "Asynchronous/Messages", "contents": {"id": 1}}
  ],
  "metadata": {"pactSpecification": {"version": "3.0.0"}}
}"#;
        let out = extract_json_contract(pact, "pacts/web-api.json", module_id(), repo());
        assert_eq!(out.source, Some(ContractSource::Pact));
        let got: Vec<(&str, &str)> = out
            .nodes
            .iter()
            .map(|n| (out.nav.qname_by_id[&n.id].as_str(), out.nav.name_by_id[&n.id].as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("contract::web-api::GET:/users", "GET /users"),
                ("contract::web-api::GET:/users/42", "GET /users/42"),
            ],
            "duplicate request is one op; a message / path-less / non-HTTP interaction is none"
        );
        let first = &out.nodes[0];
        assert_eq!(out.nav.kind_by_id[&first.id], node_kind::DOC_SECTION);
        assert_eq!(out.nav.parent_of[&first.id], module_id());
        assert_eq!(
            origin_json(first),
            r#"{"provenance":"contract","source":"pact","method":"GET","path":"/users","raw_path":"/users","description":"a request for users","consumer":"web","provider":"api"}"#
        );
        // The description goes in the CODE cell, first interaction wins.
        assert_eq!(cell_text(first, cell_type::CODE), "GET /users — a request for users");
        // `"path": "/users"` is the 8th line => 0-indexed 7.
        assert!(cell_text(first, cell_type::POSITION).contains(r#""start_line":7"#));

        // The query string is dropped from `path` but kept in `raw_path`, and a
        // newline in free text stays valid JSON.
        let second: serde_json::Value = serde_json::from_str(origin_json(&out.nodes[1])).unwrap();
        assert_eq!(second["path"], "/users/42");
        assert_eq!(second["raw_path"], "/users/42?fields=name");
        assert_eq!(second["description"], "one user\nby id");
        assert!(cell_text(&out.nodes[1], cell_type::POSITION).contains(r#""start_line":17"#));
    }

    #[test]
    fn malformed_contract_json_yields_nothing() {
        // Sniffs as OpenAPI, but does not parse: empty, never a panic.
        let broken = r#"{"openapi": "3.0.0", "paths": {"/a": {"get": "#;
        assert_eq!(sniff_json_contract(broken), Some(ContractSource::OpenApi));
        assert!(extract_json_contract(broken, "openapi.json", module_id(), repo()).nodes.is_empty());
        // Parses, but the marker is nested rather than top-level.
        let nested = r#"{"docs": {"openapi": "3.0.0"}, "paths": {"/a": {"get": {}}}}"#;
        assert!(extract_json_contract(nested, "x.json", module_id(), repo()).nodes.is_empty());
    }

    #[test]
    fn contract_counts_record_pact() {
        let mut c = ContractCounts::default();
        let pact = r#"{"consumer":{"name":"w"},"provider":{"name":"a"},"interactions":[
            {"request":{"method":"GET","path":"/a"}},{"request":{"method":"POST","path":"/a"}}]}"#;
        let openapi = r#"{"openapi":"3.0.0","paths":{"/a":{"get":{}}}}"#;
        let empty_pact = r#"{"consumer":{},"provider":{},"interactions":[]}"#;
        for (src, p) in [(pact, "p.json"), (openapi, "o.json"), (empty_pact, "e.json")] {
            c.record(&extract_json_contract(src, p, module_id(), repo()));
        }
        assert_eq!((c.files, c.openapi, c.asyncapi, c.pact), (2, 1, 0, 2));
    }
}
