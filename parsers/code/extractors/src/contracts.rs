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

use std::collections::BTreeSet;

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
    /// LE.10b: what the SCHEMA_FIELDS cells on this file's ops hold. Zero for
    /// a file whose ops declare no body shape, and for `openapi_annot`.
    pub field_stats: FieldStats,
    /// LE.9a: the feature this file's ops are declared for — the `<f>` of a
    /// quokka `features/<f>/feature.yaml`, or the `<NNN-slug>` of a spec-kit
    /// `specs/<NNN-slug>/contracts/` file. `None` for every other contract.
    pub feature: Option<String>,
}

/// The contract formats `extract_yaml_contracts` / `extract_json_contract`
/// recognise. `Pact` only ever comes from JSON: Pact has no yaml form, and
/// `FeatureYaml` (LE.9a, a quokka feature's `backend_routes` list) only ever
/// comes from yaml.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContractSource {
    OpenApi,
    AsyncApi,
    Pact,
    FeatureYaml,
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
    /// LE.10b `[contract] fields` counters, summed from each file's
    /// [`FieldStats`]: ops given a SCHEMA_FIELDS cell, the fields listed in
    /// them, and the `$ref`s resolved inside / left pointing outside the
    /// document.
    pub ops_with_fields: usize,
    pub fields: usize,
    pub refs_resolved: usize,
    pub refs_external: usize,
    /// LE.9a: ops from quokka `features/<f>/feature.yaml` `backend_routes`
    /// lists, and the distinct features that declared at least one.
    pub feature_yaml: usize,
    pub feature_yaml_features: BTreeSet<String>,
    /// LE.9a: ops (of any format) from spec-kit `specs/<NNN-slug>/contracts/`
    /// files — already counted under their format too — and the distinct
    /// feature slugs that declared at least one.
    pub speckit: usize,
    pub speckit_features: BTreeSet<String>,
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
            Some(ContractSource::FeatureYaml) => self.feature_yaml += n,
            None => return,
        }
        if let Some(feature) = &out.feature {
            if out.source == Some(ContractSource::FeatureYaml) {
                self.feature_yaml_features.insert(feature.clone());
            } else {
                self.speckit += n;
                self.speckit_features.insert(feature.clone());
            }
        }
        self.files += 1;
        let f = out.field_stats;
        self.ops_with_fields += f.ops_with_fields;
        self.fields += f.fields;
        self.refs_resolved += f.refs_resolved;
        self.refs_external += f.refs_external;
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
    // LE.9a: a quokka feature list is gated on its path first, so every other
    // yaml pays one path split and goes on to the sniff.
    if let Some(feature) = feature_dir(path)
        && let Some(decls) = scan_feature_yaml(source)
    {
        out.source = Some(ContractSource::FeatureYaml);
        out.feature = Some(feature.to_string());
        emit_feature_yaml(&mut out, decls, feature, path, module_id, repo);
        return out;
    }
    let Some(kind) = sniff(source) else {
        return out;
    };
    let stem = scoped_stem(&mut out, path);
    // LE.10b: the field reader walks the whole document, built once per file
    // and only when the file declares an op to read fields for.
    let tree = |empty: bool| (!empty).then(|| yaml_document(source));
    match kind {
        Sniffed::OpenApi => {
            out.source = Some(ContractSource::OpenApi);
            let ops = scan_openapi(source);
            let doc = tree(ops.is_empty());
            emit_openapi(&mut out, ops, doc.as_ref(), &stem, path, module_id, repo);
        }
        Sniffed::AsyncApi => {
            out.source = Some(ContractSource::AsyncApi);
            let ops = scan_asyncapi(source);
            let doc = tree(ops.is_empty());
            emit_asyncapi(&mut out, ops, doc.as_ref(), &stem, path, module_id, repo);
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

// ----------------------------------------------------------------------
// LE.9a — feature-scoped declarations
// ----------------------------------------------------------------------
//
// Spec-driven repos declare their API per FEATURE. spec_status needs every
// declared op attributed to the feature that declared it, so:
//  - spec-kit puts each feature's contracts in `specs/<NNN-slug>/contracts/`,
//    and every one of those files is typically named `openapi.yaml`. Keyed by
//    the file stem alone, two features declaring the same op collided into one
//    NodeId; the stem segment is qualified by the feature instead.
//  - quokka lists a feature's routes in `features/<f>/feature.yaml` under
//    `backend_routes:`, items `- METHOD /path  # note`, optionally grouped
//    under `protected:` / `public:`. Each item is an HTTP op with the node
//    shape every other contract format emits, so the engine's contract-link
//    pass pairs it with its ROUTE unchanged.

/// The spec-kit feature a contract file is declared under: `001-orders` for
/// `specs/001-orders/contracts/openapi.yaml` (or any file deeper inside that
/// `contracts/`). `None` outside that layout.
fn speckit_feature(path: &str) -> Option<&str> {
    let segs: Vec<&str> = path.split(['/', '\\']).collect();
    // A window of four: `specs`, the slug, `contracts`, and at least the file.
    segs.windows(4)
        .find(|w| w[0] == "specs" && is_speckit_slug(w[1]) && w[2] == "contracts")
        .map(|w| w[1])
}

/// `^\d{3,}-[a-z0-9][a-z0-9-]*$`: spec-kit's numbered feature directory.
fn is_speckit_slug(seg: &str) -> bool {
    let digits = seg.bytes().take_while(u8::is_ascii_digit).count();
    let Some(rest) = seg.get(digits..).and_then(|r| r.strip_prefix('-')) else {
        return false;
    };
    let slug_byte = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    digits >= 3
        && rest.bytes().next().is_some_and(slug_byte)
        && rest.bytes().all(|b| slug_byte(b) || b == b'-')
}

/// The qname segment `path`'s ops sit under: the file stem, qualified as
/// `feature:<NNN-slug>:<stem>` inside a spec-kit feature, whose slug is also
/// recorded on `out` for the ORIGIN cells and the `[sdd]` counters. A file
/// outside that layout keeps its bare stem, byte for byte.
fn scoped_stem(out: &mut ContractNodes, path: &str) -> String {
    let stem = file_stem(path);
    match speckit_feature(path) {
        Some(slug) => {
            out.feature = Some(slug.to_string());
            format!("feature:{slug}:{stem}")
        }
        None => stem.to_string(),
    }
}

/// `,"feature":"<slug>"` for an op declared under a feature, or nothing (the
/// field is OMITTED, so an unscoped op's ORIGIN is unchanged).
fn feature_field(feature: Option<&str>) -> String {
    match feature {
        Some(f) => format!(r#","feature":{}"#, json_str(f)),
        None => String::new(),
    }
}

/// `activities` for `features/activities/feature.yaml` (or `.yml`): the
/// feature a quokka feature list declares, read off its path. `None` for any
/// other file, so a `feature.yaml` outside a `features/` directory is never
/// read as one.
fn feature_dir(path: &str) -> Option<&str> {
    let mut segs = path.rsplit(['/', '\\']);
    let file = segs.next()?;
    let feature = segs.next()?;
    let parent = segs.next()?;
    (matches!(file, "feature.yaml" | "feature.yml") && parent == "features" && !feature.is_empty())
        .then_some(feature)
}

/// One `backend_routes` item of a quokka feature list.
#[derive(Debug, PartialEq, Eq)]
struct RouteDecl {
    method: String,
    /// As written, minus any `?query` suffix. `:param` placeholders stay
    /// verbatim: the route matcher folds them.
    path: String,
    /// The group key the item sits under (`protected`, `public`, ...), if any.
    group: Option<String>,
    /// 0-indexed line of the item.
    line: u32,
}

/// `METHOD /path` → the declared op; `None` for an item that is not one (a
/// method outside the HTTP allow-list, a path not starting with `/`). A
/// trailing ` # note` and surrounding quotes are dropped first.
fn route_item(raw: &str) -> Option<(String, String)> {
    let item = unquote(strip_comment(raw));
    let mut parts = item.split_whitespace();
    let method = parts.next()?;
    let path = parts.next()?;
    if !METHODS.iter().any(|m| m.eq_ignore_ascii_case(method)) {
        return None;
    }
    let path = path.split('?').next().unwrap_or(path);
    path.starts_with('/').then(|| (method.to_ascii_uppercase(), path.to_string()))
}

/// Line scan of a feature list's top-level `backend_routes:` block. `None`
/// when the file declares no such key (it is then not a feature list at all);
/// `Some(empty)` for `backend_routes: []` or a block of non-route items.
///
/// Items are `- METHOD /path` lines anywhere in the block, attributed to the
/// top-level group key they sit under (`protected:` / `public:` / any other
/// name), or to none when they sit directly under `backend_routes:`. A flow
/// list (`[GET /a, POST /b]`) is read the same way, on the key's line. The
/// first declaration of a `(METHOD, path)` wins; the rest of the file
/// (`frontend_components:`, `data_model:`, ...) is never read.
fn scan_feature_yaml(source: &str) -> Option<Vec<RouteDecl>> {
    let lines: Vec<&str> = source.lines().collect();
    let start = lines.iter().position(|l| {
        !is_skippable(l) && indent_of(l) == 0 && key_of(l) == Some("backend_routes")
    })?;
    let mut decls: Vec<RouteDecl> = Vec::new();
    let push = |decls: &mut Vec<RouteDecl>, raw: &str, group: Option<&str>, line: usize| {
        let Some((method, path)) = route_item(raw) else {
            return;
        };
        if decls.len() >= MAX_OPS || decls.iter().any(|d| d.method == method && d.path == path) {
            return;
        }
        decls.push(RouteDecl { method, path, group: group.map(str::to_string), line: line as u32 });
    };
    let flow = |v: &str| -> Vec<String> {
        v.strip_prefix('[')
            .and_then(|v| v.strip_suffix(']'))
            .map(|inner| inner.split(',').map(|i| unquote(i.trim()).to_string()).collect())
            .unwrap_or_default()
    };
    for item in flow(value_of(lines[start])) {
        push(&mut decls, &item, None, start);
    }
    // The indent of the block's first line: keys there are the group keys.
    let mut child_indent: Option<usize> = None;
    let mut group: Option<(usize, &str)> = None;
    for (i, line) in lines.iter().enumerate().skip(start + 1) {
        if is_skippable(line) {
            continue;
        }
        let ind = indent_of(line);
        let t = line.trim();
        let item = list_rest(t);
        // A top-level key ends the block; a list item at indent 0 is the
        // compact form of `backend_routes:`'s own sequence.
        if ind == 0 && item.is_none() {
            break;
        }
        let child = *child_indent.get_or_insert(ind);
        // A sequence may sit at its key's own indent (`protected:` / `- ...`),
        // so only a shallower line, or another key at the group's indent,
        // leaves the group.
        if let Some((gi, _)) = group
            && (ind < gi || (ind == gi && item.is_none()))
        {
            group = None;
        }
        if let Some(rest) = item {
            push(&mut decls, rest, group.map(|(_, g)| g), i);
            continue;
        }
        if ind == child
            && let Some(key) = key_of(line)
        {
            group = Some((ind, key));
            for item in flow(value_of(line)) {
                push(&mut decls, &item, Some(key), i);
            }
        }
    }
    Some(decls)
}

/// One DOC_SECTION per `backend_routes` item: qname
/// `contract::feature:<f>::<METHOD>:<path>`, ORIGIN
/// `{provenance: contract, source: feature_yaml, feature, group?, method, path, raw_path}`.
fn emit_feature_yaml(
    out: &mut ContractNodes,
    decls: Vec<RouteDecl>,
    feature: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
) {
    for d in decls {
        let qname = format!("contract::feature:{feature}::{}:{}", d.method, d.path);
        let name = format!("{} {}", d.method, d.path);
        let group = d
            .group
            .as_deref()
            .map(|g| format!(r#","group":{}"#, json_str(g)))
            .unwrap_or_default();
        let origin = format!(
            r#"{{"provenance":"contract","source":"feature_yaml"{}{group},"method":{},"path":{},"raw_path":{}}}"#,
            feature_field(Some(feature)),
            json_str(&d.method),
            json_str(&d.path),
            json_str(&d.path),
        );
        push_op(out, &qname, &name, None, path, d.line, origin, module_id, repo);
    }
}

/// One DOC_SECTION per HTTP operation. The yaml and JSON paths both end here,
/// so an `openapi.json` op is byte-for-byte the node its yaml twin would be.
/// `doc` is the parsed document (LE.10b): an op declaring a request or
/// response body schema also gets a SCHEMA_FIELDS cell.
fn emit_openapi(
    out: &mut ContractNodes,
    ops: Vec<Op>,
    doc: Option<&YNode>,
    stem: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
) {
    let mut cx = doc.map(FieldCx::new);
    let feature = feature_field(out.feature.as_deref());
    for op in ops {
        let qname = format!("contract::{stem}::{}:{}", op.method, op.path);
        let name = format!("{} {}", op.method, op.path);
        let origin = format!(
            r#"{{"provenance":"contract","source":"openapi"{feature},"method":"{}","path":"{}","raw_path":"{}"{}}}"#,
            esc(&op.method),
            esc(&op.path),
            esc(&op.raw_path),
            operation_id_field(op.operation_id.as_deref())
        );
        let oid = op.operation_id.as_deref();
        push_op(out, &qname, &name, oid, path, op.line, origin, module_id, repo);
        if let Some(cx) = cx.as_mut() {
            cx.begin_op();
            let secs = openapi_sections(cx, &op);
            attach_fields(out, "openapi", &secs, cx.truncated);
        }
    }
    if let Some(cx) = cx {
        cx.fold_refs(out);
    }
}

/// One DOC_SECTION per AsyncAPI channel operation; shared like [`emit_openapi`],
/// and like it, a channel op whose message declares a payload schema also
/// gets a SCHEMA_FIELDS cell (LE.10b).
fn emit_asyncapi(
    out: &mut ContractNodes,
    ops: Vec<ChannelOp>,
    doc: Option<&YNode>,
    stem: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
) {
    let mut cx = doc.map(FieldCx::new);
    let feature = feature_field(out.feature.as_deref());
    for op in ops {
        let qname = format!("contract::{stem}::{}:{}", op.action, op.channel);
        let name = format!("{} {}", op.action, op.channel);
        let origin = format!(
            r#"{{"provenance":"contract","source":"asyncapi"{feature},"action":"{}","channel":"{}"{}}}"#,
            op.action,
            esc(&op.channel),
            operation_id_field(op.operation_id.as_deref())
        );
        let oid = op.operation_id.as_deref();
        push_op(out, &qname, &name, oid, path, op.line, origin, module_id, repo);
        if let Some(cx) = cx.as_mut() {
            cx.begin_op();
            let secs = asyncapi_sections(cx, &op);
            attach_fields(out, "asyncapi", &secs, cx.truncated);
        }
    }
    if let Some(cx) = cx {
        cx.fold_refs(out);
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
    /// Position in `interactions[]` of the interaction the op came from (the
    /// first one on this request), whose bodies LE.10b reads.
    index: usize,
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
    for (index, it) in interactions.iter().enumerate() {
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
            index,
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
    let stem = scoped_stem(&mut out, path);
    let feature = feature_field(out.feature.as_deref());
    out.source = Some(kind);
    // LE.10b: the field reader walks an order-keeping tree (the `Value` map
    // above may be sorted), parsed only when there is an op to read.
    let tree = |empty: bool| {
        if empty {
            return None;
        }
        serde_json::from_str::<YNode>(source.trim_start_matches('\u{feff}')).ok()
    };
    match kind {
        ContractSource::OpenApi => {
            let ops = json_openapi_ops(source, &doc, &lines);
            let t = tree(ops.is_empty());
            emit_openapi(&mut out, ops, t.as_ref(), &stem, path, module_id, repo);
        }
        ContractSource::AsyncApi => {
            let ops = json_asyncapi_ops(source, &doc, &lines);
            let t = tree(ops.is_empty());
            emit_asyncapi(&mut out, ops, t.as_ref(), &stem, path, module_id, repo);
        }
        // `sniff_json_contract` never answers it: a feature list is yaml-only.
        ContractSource::FeatureYaml => {}
        ContractSource::Pact => {
            let ops = json_pact_ops(source, &doc, &lines);
            let t = tree(ops.is_empty());
            let mut cx = t.as_ref().map(FieldCx::new);
            let party = |k: &str| {
                doc.get(k)
                    .and_then(|p| p.get("name"))
                    .and_then(Value::as_str)
                    .map(|n| format!(r#","{k}":{}"#, json_str(n)))
                    .unwrap_or_default()
            };
            let parties = format!("{}{}", party("consumer"), party("provider"));
            for op in ops {
                let qname = format!("contract::{stem}::{}:{}", op.method, op.path);
                let name = format!("{} {}", op.method, op.path);
                let description = op
                    .description
                    .as_deref()
                    .map(|d| format!(r#","description":{}"#, json_str(d)))
                    .unwrap_or_default();
                let origin = format!(
                    r#"{{"provenance":"contract","source":"pact"{feature},"method":{},"path":{},"raw_path":{}{description}{parties}}}"#,
                    json_str(&op.method),
                    json_str(&op.path),
                    json_str(&op.raw_path),
                );
                let desc = op.description.as_deref();
                push_op(&mut out, &qname, &name, desc, path, op.line, origin, module_id, repo);
                if let Some(cx) = cx.as_mut() {
                    cx.begin_op();
                    let root = cx.root;
                    let it = root.get("interactions").map_or(&[][..], YNode::items).get(op.index);
                    let secs = it.map(|it| pact_sections(cx, it)).unwrap_or_default();
                    attach_fields(&mut out, "pact", &secs, cx.truncated);
                }
            }
        }
    }
    out
}

// ----------------------------------------------------------------------
// LE.10b — declared body fields (SCHEMA_FIELDS on contract ops)
// ----------------------------------------------------------------------
//
// The route half of LE.10's field diff needs each contract op's DECLARED body
// shape. One intermediate tree, [`YNode`], carries every format: the yaml
// subset scanner builds it for a `.yaml` contract (zero-dependency, like the op
// scanners above), serde_json builds it for a `.json` one through an
// order-keeping `Deserialize`, and one walker flattens it into fields. A yaml
// spec and its JSON twin therefore give byte-identical cells.

/// One contract document, whichever format it came from. Map entries keep
/// document order (the store's byte-identical gate and the yaml / JSON parity
/// both depend on it); a duplicated key keeps every entry and lookups take the
/// first.
#[derive(Debug, Clone, PartialEq)]
enum YNode {
    Map(Vec<(String, YNode)>),
    List(Vec<YNode>),
    /// A leaf: its text with quotes stripped, and its JSON type (`string`,
    /// `integer`, `number`, `boolean` or `null`). A yaml plain scalar is typed
    /// by the YAML 1.2 core schema; a quoted one is a string.
    Scalar(String, &'static str),
}

impl YNode {
    fn null() -> Self {
        YNode::Scalar(String::new(), "null")
    }

    fn get(&self, key: &str) -> Option<&YNode> {
        self.entries().iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    fn entries(&self) -> &[(String, YNode)] {
        match self {
            YNode::Map(m) => m,
            _ => &[],
        }
    }

    fn items(&self) -> &[YNode] {
        match self {
            YNode::List(l) => l,
            _ => &[],
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            YNode::Scalar(s, _) => Some(s),
            _ => None,
        }
    }

    /// The text of scalar member `key`.
    fn str_at(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(YNode::as_str)
    }
}

/// JSON → [`YNode`], object members in document order whatever map type
/// serde_json was built with. serde_json's own recursion limit bounds depth.
impl<'de> serde::Deserialize<'de> for YNode {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(YNodeVisitor)
    }
}

struct YNodeVisitor;

impl<'de> serde::de::Visitor<'de> for YNodeVisitor {
    type Value = YNode;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<YNode, E> {
        Ok(YNode::Scalar(v.to_string(), "boolean"))
    }

    fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<YNode, E> {
        Ok(YNode::Scalar(v.to_string(), "integer"))
    }

    fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<YNode, E> {
        Ok(YNode::Scalar(v.to_string(), "integer"))
    }

    fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<YNode, E> {
        Ok(YNode::Scalar(v.to_string(), "number"))
    }

    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<YNode, E> {
        Ok(YNode::Scalar(v.to_string(), "string"))
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<YNode, E> {
        Ok(YNode::null())
    }

    fn visit_none<E: serde::de::Error>(self) -> Result<YNode, E> {
        Ok(YNode::null())
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<YNode, A::Error> {
        let mut items = Vec::new();
        while let Some(v) = seq.next_element::<YNode>()? {
            items.push(v);
        }
        Ok(YNode::List(items))
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<YNode, A::Error> {
        let mut entries = Vec::new();
        while let Some((k, v)) = map.next_entry::<String, YNode>()? {
            entries.push((k, v));
        }
        Ok(YNode::Map(entries))
    }
}

// ---- the yaml subset scanner -------------------------------------------

/// Deepest nesting the yaml subset scanner follows, block levels and flow
/// brackets alike. A deeper block is skipped whole: it yields nothing, and
/// recursion never grows past it.
const YAML_MAX_NEST: usize = 64;

/// Most continuation lines one multi-line flow collection (`required: [` ..
/// `]`) joins.
const YAML_MAX_FLOW_LINES: usize = 256;

/// One significant yaml line: its indent, and its text with the indent and
/// trailing whitespace removed.
#[derive(Clone, Copy)]
struct YLine<'a> {
    ind: usize,
    text: &'a str,
}

struct YamlScan<'a> {
    lines: Vec<YLine<'a>>,
    pos: usize,
}

/// The whole contract yaml as a [`YNode`], read by the zero-dependency subset
/// scanner: block mappings, `- ` block lists (a compact one at its key's own
/// indent included), `[a, b]` / `{k: v}` flow collections (which may span
/// lines), and plain, single- and double-quoted scalars.
///
/// What it skips, by design: aliases (`*a`) and `<<:` merge keys, block
/// scalars (`|` / `>`), multi-line quoted scalars, complex keys (`? `), the
/// continuation lines of a multi-line plain scalar, and anything nested past
/// [`YAML_MAX_NEST`]. A skipped construct drops its key, so it yields no
/// fields, never a panic. An anchor (`&a`) or tag (`!t`) on a value is
/// stripped and the value read as written. Every slice is taken at an ASCII
/// delimiter or a `trim` boundary, so no input can split a char.
fn yaml_document(source: &str) -> YNode {
    let lines = source
        .lines()
        .filter(|l| !is_skippable(l))
        .map(|l| YLine { ind: indent_of(l), text: l.trim() })
        .filter(|l| {
            // Document markers and directives carry no structure.
            !(l.ind == 0
                && (l.text == "---"
                    || l.text == "..."
                    || l.text.starts_with("--- ")
                    || l.text.starts_with('%')))
        })
        .collect();
    YamlScan { lines, pos: 0 }.block(-1, 0)
}

impl<'a> YamlScan<'a> {
    fn peek(&self) -> Option<YLine<'a>> {
        self.lines.get(self.pos).copied()
    }

    /// Consume every line indented deeper than `parent`.
    fn skip_deeper(&mut self, parent: isize) {
        while self.peek().is_some_and(|l| l.ind as isize > parent) {
            self.pos += 1;
        }
    }

    /// The block value under a key (or list dash) at indent `parent`: the
    /// lines after it that are indented deeper.
    fn block(&mut self, parent: isize, nest: usize) -> YNode {
        let Some(first) = self.peek().filter(|l| l.ind as isize > parent) else {
            return YNode::null();
        };
        if nest > YAML_MAX_NEST {
            self.skip_deeper(parent);
            return YNode::null();
        }
        if list_rest(first.text).is_some() {
            return self.list(first.ind, nest);
        }
        if is_map_entry(first.text) {
            return self.map(first.ind, nest);
        }
        // A scalar or a flow collection written on lines of its own.
        let mut text = String::new();
        while let Some(l) = self.peek().filter(|l| l.ind as isize > parent) {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(l.text);
            self.pos += 1;
        }
        inline_value(&text).unwrap_or_else(YNode::null)
    }

    fn map(&mut self, ind: usize, nest: usize) -> YNode {
        let mut entries = Vec::new();
        while let Some(l) = self.peek() {
            if l.ind < ind {
                break;
            }
            self.pos += 1;
            if l.ind > ind {
                continue; // a stray deeper line
            }
            let key_value = split_key(l.text).filter(|_| is_map_entry(l.text));
            let Some((key, raw)) = key_value else {
                // A list item or a bare scalar where a key belongs.
                self.skip_deeper(ind as isize);
                continue;
            };
            // A complex key's `? ` / `: ` halves and `<<:` merge keys.
            if key.is_empty() || key.starts_with('?') || key == "<<" {
                self.skip_deeper(ind as isize);
                continue;
            }
            if let Some(v) = self.value(raw, ind, nest, true) {
                entries.push((key.to_string(), v));
            }
        }
        YNode::Map(entries)
    }

    fn list(&mut self, ind: usize, nest: usize) -> YNode {
        let mut items = Vec::new();
        while let Some(l) = self.peek() {
            if l.ind < ind {
                break;
            }
            if l.ind > ind {
                self.pos += 1;
                continue;
            }
            let Some(rest) = list_rest(l.text) else { break };
            let rest = strip_props(rest);
            if rest.is_empty() || rest.starts_with('#') {
                self.pos += 1;
                items.push(self.block(ind as isize, nest + 1));
            } else if list_rest(rest).is_some() || is_map_entry(rest) {
                // `- key: v` / `- - x`: the item is a block whose first line
                // starts at the column after the dash. `rest` is a suffix of
                // the line, so the column is exact.
                let col = ind + (l.text.len() - rest.len());
                if let Some(slot) = self.lines.get_mut(self.pos) {
                    *slot = YLine { ind: col, text: rest };
                }
                items.push(self.block(ind as isize, nest + 1));
            } else {
                self.pos += 1;
                if let Some(v) = self.value(rest, ind, nest, false) {
                    items.push(v);
                }
            }
        }
        YNode::List(items)
    }

    /// The value written after a key (or dash) at indent `ind`. `compact`: a
    /// `- ` list at the key's own indent is its value (mapping context only).
    fn value(&mut self, raw: &str, ind: usize, nest: usize, compact: bool) -> Option<YNode> {
        let raw = strip_props(raw);
        if raw.is_empty() || raw.starts_with('#') {
            return Some(match self.peek() {
                Some(n) if compact && n.ind == ind && list_rest(n.text).is_some() => {
                    self.list(ind, nest + 1)
                }
                _ => self.block(ind as isize, nest + 1),
            });
        }
        if raw.starts_with(['[', '{']) && flow_depth(raw) > 0 {
            // A flow collection spanning lines: join its continuation lines.
            let mut text = raw.to_string();
            let mut joined = 0;
            while flow_depth(&text) > 0 && joined < YAML_MAX_FLOW_LINES {
                let next = self.peek().filter(|n| {
                    n.ind > ind || (n.ind == ind && n.text.starts_with([']', '}']))
                });
                let Some(n) = next else { break };
                text.push(' ');
                text.push_str(n.text);
                self.pos += 1;
                joined += 1;
            }
            self.skip_deeper(ind as isize);
            return inline_value(&text);
        }
        let v = inline_value(raw);
        // A block scalar's body, a multi-line scalar's continuation, or an
        // alias's stray children: never structure.
        self.skip_deeper(ind as isize);
        v
    }
}

/// `- x` → `x`, `-` → ``; `None` when the line is not a list item.
fn list_rest(t: &str) -> Option<&str> {
    if t == "-" {
        return Some("");
    }
    let r = t.strip_prefix('-')?;
    r.starts_with([' ', '\t']).then(|| r.trim_start())
}

/// A `key: value` / `key:` line (plain or quoted key), not a flow collection.
fn is_map_entry(t: &str) -> bool {
    !t.starts_with(['[', '{']) && split_key(t).is_some()
}

/// `raw` without a leading anchor (`&a`) or tag (`!t`, `!!str`). Always a
/// suffix of `raw`.
fn strip_props(raw: &str) -> &str {
    let mut t = raw.trim_start();
    for _ in 0..2 {
        if t.starts_with(['&', '!']) {
            t = match t.find(char::is_whitespace) {
                Some(i) => t[i..].trim_start(),
                None => "",
            };
        }
    }
    t
}

/// A plain scalar's text without a trailing ` # comment`.
fn strip_comment(raw: &str) -> &str {
    match raw.find(" #").or_else(|| raw.find("\t#")) {
        Some(i) => raw[..i].trim_end(),
        None => raw.trim_end(),
    }
}

/// A value written inline: a flow collection, a quoted or a plain scalar.
/// `None` for what the scanner skips: an alias, a block scalar indicator, a
/// quoted scalar that does not close on its line, a malformed flow collection.
fn inline_value(raw: &str) -> Option<YNode> {
    let raw = strip_props(raw);
    match raw.as_bytes().first() {
        None | Some(b'#') => Some(YNode::null()),
        Some(b'*' | b'|' | b'>') => None,
        Some(b'[' | b'{') => flow_value(raw, &mut 0, 0),
        Some(b'"' | b'\'') => flow_quoted(raw, &mut 0),
        _ => Some(plain_scalar(strip_comment(raw))),
    }
}

/// A plain scalar, typed by the YAML 1.2 core schema.
fn plain_scalar(t: &str) -> YNode {
    let t = t.trim();
    let ty = match t {
        "true" | "True" | "TRUE" | "false" | "False" | "FALSE" => "boolean",
        "" | "~" | "null" | "Null" | "NULL" => "null",
        ".inf" | "-.inf" | "+.inf" | ".nan" | ".NaN" => "number",
        _ if is_yaml_int(t) => "integer",
        _ if t.bytes().any(|c| c.is_ascii_digit())
            && t.bytes().all(|c| c.is_ascii_digit() || matches!(c, b'.' | b'e' | b'E' | b'+' | b'-'))
            && t.parse::<f64>().is_ok() =>
        {
            "number"
        }
        _ => "string",
    };
    YNode::Scalar(t.to_string(), ty)
}

fn is_yaml_int(t: &str) -> bool {
    let digits = t.strip_prefix(['+', '-']).unwrap_or(t);
    if let Some(hex) = digits.strip_prefix("0x") {
        return !hex.is_empty() && hex.bytes().all(|c| c.is_ascii_hexdigit());
    }
    if let Some(oct) = digits.strip_prefix("0o") {
        return !oct.is_empty() && oct.bytes().all(|c| matches!(c, b'0'..=b'7'));
    }
    !digits.is_empty() && digits.bytes().all(|c| c.is_ascii_digit())
}

/// Bracket depth left open at the end of `s`, quoted text ignored.
fn flow_depth(s: &str) -> i64 {
    let b = s.as_bytes();
    let mut depth = 0i64;
    let mut quote: Option<u8> = None;
    let mut j = 0;
    while let Some(&c) = b.get(j) {
        match quote {
            Some(b'"') if c == b'\\' => j += 1,
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => match c {
                b'"' | b'\'' => quote = Some(c),
                b'[' | b'{' => depth += 1,
                b']' | b'}' => depth -= 1,
                _ => {}
            },
        }
        j += 1;
    }
    depth
}

fn skip_ws(b: &[u8], i: &mut usize) {
    while b.get(*i).is_some_and(|c| c.is_ascii_whitespace()) {
        *i += 1;
    }
}

/// A flow value at `*i`: `[..]`, `{..}` or a scalar; `*i` ends past it.
fn flow_value(s: &str, i: &mut usize, nest: usize) -> Option<YNode> {
    let b = s.as_bytes();
    skip_ws(b, i);
    let open = *b.get(*i)?;
    if open != b'[' && open != b'{' {
        return flow_scalar(s, i, false);
    }
    if nest > YAML_MAX_NEST {
        return None;
    }
    *i += 1;
    let close = if open == b'[' { b']' } else { b'}' };
    let mut list = Vec::new();
    let mut map = Vec::new();
    loop {
        skip_ws(b, i);
        let c = *b.get(*i)?;
        if c == close {
            *i += 1;
            break;
        }
        if c == b',' {
            *i += 1;
            continue;
        }
        let start = *i;
        if open == b'[' {
            list.push(flow_value(s, i, nest + 1)?);
        } else {
            let key = flow_scalar(s, i, true)?;
            skip_ws(b, i);
            let v = if b.get(*i) == Some(&b':') {
                *i += 1;
                flow_value(s, i, nest + 1)?
            } else {
                YNode::null()
            };
            map.push((key.as_str().unwrap_or_default().to_string(), v));
        }
        if *i == start {
            return None; // no progress: a stray closer of the other kind
        }
    }
    Some(if open == b'[' { YNode::List(list) } else { YNode::Map(map) })
}

/// A flow scalar at `*i`, quoted or plain. A plain one ends at `,` `]` `}`,
/// and a plain KEY also at a `:` followed by a space or a closer.
fn flow_scalar(s: &str, i: &mut usize, key: bool) -> Option<YNode> {
    let b = s.as_bytes();
    if matches!(b.get(*i), Some(b'"' | b'\'')) {
        return flow_quoted(s, i);
    }
    let start = *i;
    while let Some(&c) = b.get(*i) {
        if matches!(c, b',' | b']' | b'}') {
            break;
        }
        if key
            && c == b':'
            && b.get(*i + 1).is_none_or(|n| n.is_ascii_whitespace() || matches!(n, b',' | b']' | b'}'))
        {
            break;
        }
        *i += 1;
    }
    Some(plain_scalar(s.get(start..*i)?))
}

/// A single- or double-quoted scalar opening at `*i`; `*i` ends past the
/// closing quote. `None` when it does not close. Double-quoted `\"`, `\\`,
/// `\/`, `\n`, `\t` are unescaped (any other escape is kept as written);
/// single-quoted `''` is one quote.
fn flow_quoted(s: &str, i: &mut usize) -> Option<YNode> {
    let b = s.as_bytes();
    let q = *b.get(*i)?;
    let mut out = String::new();
    let mut j = *i + 1;
    let mut seg = j;
    loop {
        let c = *b.get(j)?;
        if q == b'"' && c == b'\\' {
            let e = *b.get(j + 1)?;
            let un = match e {
                b'"' => '"',
                b'\\' => '\\',
                b'/' => '/',
                b'n' => '\n',
                b't' => '\t',
                _ => {
                    j += 1; // kept as written, the escaped char included
                    continue;
                }
            };
            out.push_str(s.get(seg..j)?);
            out.push(un);
            j += 2;
            seg = j;
            continue;
        }
        if c == q {
            if q == b'\'' && b.get(j + 1) == Some(&b'\'') {
                out.push_str(s.get(seg..=j)?);
                j += 2;
                seg = j;
                continue;
            }
            out.push_str(s.get(seg..j)?);
            *i = j + 1;
            return Some(YNode::Scalar(out, "string"));
        }
        j += 1;
    }
}

// ---- the field walker ---------------------------------------------------

/// Deepest property nesting flattened into `parent.child` names; an object
/// nested deeper is listed with its type and not descended.
const FIELD_MAX_DEPTH: usize = 4;

/// Most fields one section lists (LE.10a's cap); past it the cell says
/// `"truncated":true`.
const FIELD_MAX: usize = 500;

/// Most `$ref` hops one lookup follows (`#/channels/c/messages/m` →
/// `#/components/messages/M` → ...); a longer chain is a ref-only cycle.
const MAX_REF_HOPS: usize = 8;

/// Walk steps one op may spend. Bounds a pathological `allOf` / properties
/// fan-out; running out marks the cell truncated.
const FIELD_BUDGET: usize = 20_000;

/// One declared body field, flattened: `customer.address.city`, `lines[]`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Field {
    name: String,
    ty: String,
    /// In its object's `required` list. Never set from a Pact example, which
    /// states presence, not requiredness.
    required: bool,
}

/// Named field lists of one op (`request`, `response:201`, `payload`), in the
/// order the cell writes them.
type Sections = Vec<(String, Vec<Field>)>;

/// `[contract] fields` counters for one file, summed per build by
/// [`ContractCounts::record`].
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldStats {
    /// Ops given a SCHEMA_FIELDS cell.
    pub ops_with_fields: usize,
    /// Fields listed across those cells.
    pub fields: usize,
    /// `$ref`s resolved inside the document.
    pub refs_resolved: usize,
    /// `$ref`s pointing outside the document (another file, a URL) or at
    /// nothing in it: each is listed as a field typed by the ref itself.
    pub refs_external: usize,
}

/// Where an op section's schema sits.
enum Body<'a> {
    Schema(&'a YNode),
    /// A `$ref` that does not resolve inside the document.
    External(&'a str),
}

/// One file's field walk: the root refs resolve against, the refs being
/// expanded on the current descent (the cycle guard), and the counters.
struct FieldCx<'a> {
    root: &'a YNode,
    stack: Vec<&'a str>,
    refs_resolved: usize,
    refs_external: usize,
    /// The current op hit [`FIELD_MAX`] or ran out of [`FIELD_BUDGET`].
    truncated: bool,
    budget: usize,
}

impl<'a> FieldCx<'a> {
    fn new(root: &'a YNode) -> Self {
        FieldCx {
            root,
            stack: Vec::new(),
            refs_resolved: 0,
            refs_external: 0,
            truncated: false,
            budget: FIELD_BUDGET,
        }
    }

    fn begin_op(&mut self) {
        self.stack.clear();
        self.truncated = false;
        self.budget = FIELD_BUDGET;
    }

    /// Add this file's ref counters to its extraction.
    fn fold_refs(self, out: &mut ContractNodes) {
        out.field_stats.refs_resolved += self.refs_resolved;
        out.field_stats.refs_external += self.refs_external;
    }

    fn spend(&mut self) -> bool {
        if self.budget == 0 {
            self.truncated = true;
            return false;
        }
        self.budget -= 1;
        true
    }

    fn push(&mut self, out: &mut Vec<Field>, name: String, ty: String, required: bool) {
        if out.len() >= FIELD_MAX {
            self.truncated = true;
        } else {
            out.push(Field { name, ty, required });
        }
    }

    /// Follow `node`'s `$ref`s, recording each one followed in `via`. `Err`
    /// carries the first ref that does not resolve inside the document.
    fn deref(&mut self, mut node: &'a YNode, via: &mut Vec<&'a str>) -> Result<&'a YNode, &'a str> {
        for _ in 0..MAX_REF_HOPS {
            let Some(r) = node.str_at("$ref") else {
                return Ok(node);
            };
            match local_pointer(self.root, r) {
                Some(t) => {
                    self.refs_resolved += 1;
                    via.push(r);
                    node = t;
                }
                None => {
                    self.refs_external += 1;
                    return Err(r);
                }
            }
        }
        match node.str_at("$ref") {
            Some(r) => {
                self.refs_external += 1;
                Err(r)
            }
            None => Ok(node),
        }
    }

    /// A request / response / payload schema flattened. An object body lists
    /// its properties; an array body is `[]` (then `[].x`); anything else is
    /// one field named `$`.
    fn body(&mut self, schema: &'a YNode) -> Vec<Field> {
        let mut out = Vec::new();
        self.member(schema, String::new(), false, 0, &mut out);
        out
    }

    /// One named schema (a property, an array's items, or the body root when
    /// `name` is empty) and, when it is an object, its properties beneath it.
    fn member(&mut self, schema: &'a YNode, mut name: String, required: bool, depth: usize, out: &mut Vec<Field>) {
        if !self.spend() {
            return;
        }
        let mut via = Vec::new();
        let mut s = match self.deref(schema, &mut via) {
            Ok(s) => s,
            Err(r) => {
                let label = if name.is_empty() { "$ref".to_string() } else { name };
                self.push(out, label, r.to_string(), required);
                return;
            }
        };
        let mut hops = 0;
        while hops < FIELD_MAX_DEPTH && is_array(s) {
            hops += 1;
            name.push_str("[]");
            let Some(items) = s.get("items") else {
                self.push(out, name, "any".to_string(), required);
                return;
            };
            s = match self.deref(items, &mut via) {
                Ok(t) => t,
                Err(r) => {
                    self.push(out, name, r.to_string(), required);
                    return;
                }
            };
        }
        let object = is_object(s);
        if !(name.is_empty() && object) {
            let label = if name.is_empty() { "$".to_string() } else { name.clone() };
            self.push(out, label, type_name(s, 0), required);
        }
        if !object || depth >= FIELD_MAX_DEPTH || via.iter().any(|r| self.stack.contains(r)) {
            return; // a scalar, too deep, or a ref already being expanded (a cycle)
        }
        let mark = self.stack.len();
        self.stack.extend(via);
        self.properties(s, &name, depth + 1, out);
        self.stack.truncate(mark);
    }

    /// An object's properties (its `allOf` members' merged in, first
    /// declaration of a name wins), each marked from the merged `required`.
    fn properties(&mut self, obj: &'a YNode, prefix: &str, depth: usize, out: &mut Vec<Field>) {
        let mut props: Vec<(&'a str, &'a YNode)> = Vec::new();
        let mut required: std::collections::HashSet<&'a str> = std::collections::HashSet::new();
        let mut external: Vec<&'a str> = Vec::new();
        self.gather(obj, &mut props, &mut required, &mut external, 0);
        for r in external {
            self.push(out, join_field(prefix, "$ref"), r.to_string(), false);
        }
        for (name, schema) in props {
            if self.truncated {
                return;
            }
            let req = required.contains(name);
            self.member(schema, join_field(prefix, name), req, depth, out);
        }
    }

    fn gather(
        &mut self,
        obj: &'a YNode,
        props: &mut Vec<(&'a str, &'a YNode)>,
        required: &mut std::collections::HashSet<&'a str>,
        external: &mut Vec<&'a str>,
        hops: usize,
    ) {
        if !self.spend() {
            return;
        }
        let mut seen: std::collections::HashSet<&'a str> = props.iter().map(|(n, _)| *n).collect();
        for (k, v) in obj.get("properties").map_or(&[][..], YNode::entries) {
            if seen.insert(k.as_str()) {
                props.push((k.as_str(), v));
            }
        }
        for r in obj.get("required").map_or(&[][..], YNode::items) {
            if let Some(r) = r.as_str() {
                required.insert(r);
            }
        }
        if hops >= FIELD_MAX_DEPTH {
            return;
        }
        for m in obj.get("allOf").map_or(&[][..], YNode::items) {
            let mut via = Vec::new();
            match self.deref(m, &mut via) {
                Ok(t) if !via.iter().any(|r| self.stack.contains(r)) => {
                    let mark = self.stack.len();
                    self.stack.extend(via);
                    self.gather(t, props, required, external, hops + 1);
                    self.stack.truncate(mark);
                }
                Ok(_) => {}
                Err(r) => external.push(r),
            }
        }
    }

    /// A Pact example body flattened, each field typed by its JSON value.
    fn example(&mut self, body: &'a YNode) -> Vec<Field> {
        let mut out = Vec::new();
        self.example_member(vec![body], String::new(), 0, &mut out);
        out
    }

    /// `values` are the examples seen for one name (several when it sits in an
    /// array of objects): the first decides the type, and objects contribute
    /// the union of their keys in first-seen order.
    fn example_member(&mut self, mut values: Vec<&'a YNode>, mut name: String, depth: usize, out: &mut Vec<Field>) {
        if !self.spend() {
            return;
        }
        let mut hops = 0;
        while hops < FIELD_MAX_DEPTH && matches!(values.first(), Some(YNode::List(_))) {
            hops += 1;
            name.push_str("[]");
            values = values.iter().copied().flat_map(YNode::items).collect();
        }
        let Some(first) = values.first().copied() else {
            self.push(out, name, "any".to_string(), false);
            return;
        };
        let (ty, object) = match first {
            YNode::Map(_) => ("object", true),
            YNode::List(_) => ("array", false),
            YNode::Scalar(_, t) => (*t, false),
        };
        if !(name.is_empty() && object) {
            let label = if name.is_empty() { "$".to_string() } else { name.clone() };
            self.push(out, label, ty.to_string(), false);
        }
        if !object || depth >= FIELD_MAX_DEPTH {
            return;
        }
        let maps: Vec<&'a YNode> = values.into_iter().filter(|v| matches!(v, YNode::Map(_))).collect();
        let mut keys: Vec<&'a str> = Vec::new();
        let mut seen: std::collections::HashSet<&'a str> = std::collections::HashSet::new();
        for m in &maps {
            for (k, _) in m.entries() {
                if seen.insert(k.as_str()) {
                    keys.push(k.as_str());
                }
            }
        }
        for k in keys {
            if self.truncated {
                return;
            }
            let vs: Vec<&'a YNode> = maps.iter().filter_map(|m| m.get(k)).collect();
            self.example_member(vs, join_field(&name, k), depth + 1, out);
        }
    }

    /// One section from `body` into `secs`; nothing when it has no schema,
    /// lists no field, or its name is already taken.
    fn section(&mut self, secs: &mut Sections, name: String, body: Option<Body<'a>>) {
        let fields = match body {
            None => return,
            Some(Body::External(r)) => {
                vec![Field { name: "$ref".to_string(), ty: r.to_string(), required: false }]
            }
            Some(Body::Schema(s)) => self.body(s),
        };
        if !fields.is_empty() && !secs.iter().any(|(n, _)| *n == name) {
            secs.push((name, fields));
        }
    }

    /// The schema of a request body / response object: the object resolved,
    /// then its `content` media type (`application/json`, else the first
    /// `*json*`, else the first) → `schema`, else Swagger 2's direct `schema`.
    fn body_schema(&mut self, obj: &'a YNode) -> Option<Body<'a>> {
        let obj = match self.deref(obj, &mut Vec::new()) {
            Ok(o) => o,
            Err(r) => return Some(Body::External(r)),
        };
        let media = obj.get("content").and_then(|c| {
            let e = c.entries();
            e.iter()
                .find(|(k, _)| k == "application/json")
                .or_else(|| e.iter().find(|(k, _)| k.contains("json")))
                .or_else(|| e.first())
                .map(|(_, v)| v)
        });
        match media {
            Some(m) => m.get("schema"),
            None => obj.get("schema"),
        }
        .map(Body::Schema)
    }
}

/// `prefix.name`, or `name` at the body root.
fn join_field(prefix: &str, name: &str) -> String {
    if prefix.is_empty() { name.to_string() } else { format!("{prefix}.{name}") }
}

fn is_array(s: &YNode) -> bool {
    match s.get("type") {
        Some(YNode::Scalar(t, _)) => t == "array",
        Some(YNode::List(ts)) => ts.iter().any(|t| t.as_str() == Some("array")),
        _ => s.get("items").is_some(),
    }
}

fn is_object(s: &YNode) -> bool {
    match s.get("type") {
        Some(YNode::Scalar(t, _)) => t == "object",
        Some(YNode::List(ts)) => ts.iter().any(|t| t.as_str() == Some("object")),
        _ => {
            s.get("properties").is_some()
                || s.get("allOf").is_some()
                || s.get("additionalProperties").is_some()
        }
    }
}

/// A schema's declared type: `type` as written (a 3.1 type list joined with
/// `|`), `(format)` appended, `|null` for `nullable: true`; `oneOf<A|B>` /
/// `anyOf<A|B>` naming each member (a ref by its last segment) without
/// descending; `object` / `array` / `any` when no `type` is written.
fn type_name(s: &YNode, hops: usize) -> String {
    if s.get("type").is_none() && s.get("properties").is_none() {
        for key in ["oneOf", "anyOf"] {
            if let Some(YNode::List(ms)) = s.get(key) {
                let names: Vec<String> = ms
                    .iter()
                    .map(|m| match m.str_at("$ref") {
                        Some(r) => ref_name(r).to_string(),
                        None if hops < 2 => type_name(m, hops + 1),
                        None => "any".to_string(),
                    })
                    .collect();
                return format!("{key}<{}>", names.join("|"));
            }
        }
    }
    let mut t = match s.get("type") {
        Some(YNode::Scalar(t, _)) if !t.is_empty() => t.clone(),
        Some(YNode::List(ts)) => ts.iter().filter_map(YNode::as_str).collect::<Vec<_>>().join("|"),
        _ if is_object(s) => "object".to_string(),
        _ if s.get("items").is_some() => "array".to_string(),
        _ => "any".to_string(),
    };
    if let Some(f) = s.str_at("format").filter(|f| !f.is_empty()) {
        t = format!("{t}({f})");
    }
    if s.str_at("nullable") == Some("true") {
        t.push_str("|null");
    }
    t
}

/// `#/components/schemas/Order` → `Order`.
fn ref_name(r: &str) -> &str {
    r.rsplit('/').next().unwrap_or(r)
}

/// A same-document JSON pointer (`#/components/schemas/X`, RFC 6901 `~1` /
/// `~0` and URI `%xx` unescaped) resolved against `root`. Anything else — a
/// relative file, a URL, a pointer to nothing — is `None`.
fn local_pointer<'a>(root: &'a YNode, r: &str) -> Option<&'a YNode> {
    let p = r.trim().strip_prefix('#')?;
    if p.is_empty() {
        return Some(root);
    }
    let mut node = root;
    for seg in p.strip_prefix('/')?.split('/') {
        let seg = percent_decode(seg).replace("~1", "/").replace("~0", "~");
        node = match node {
            YNode::Map(_) => node.get(&seg)?,
            YNode::List(l) => l.get(seg.parse::<usize>().ok()?)?,
            YNode::Scalar(..) => return None,
        };
    }
    Some(node)
}

fn percent_decode(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let b = s.as_bytes();
    let hex = |k: usize| b.get(k).and_then(|h| (*h as char).to_digit(16));
    let mut out = Vec::with_capacity(b.len());
    let mut j = 0;
    while let Some(&c) = b.get(j) {
        match (c, hex(j + 1), hex(j + 2)) {
            (b'%', Some(h), Some(l)) => {
                out.push((h * 16 + l) as u8);
                j += 3;
            }
            _ => {
                out.push(c);
                j += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

// ---- per-format sections ------------------------------------------------

/// The body fields one OpenAPI op declares: `request` (OpenAPI 3
/// `requestBody`, or Swagger 2's `in: body` parameter) and `response:<code>`
/// for each response with a schema, in document order.
fn openapi_sections<'a>(cx: &mut FieldCx<'a>, op: &Op) -> Sections {
    let mut secs = Sections::new();
    let root = cx.root;
    let Some(item) = root.get("paths").and_then(|p| p.get(&op.raw_path)) else {
        return secs;
    };
    let Ok(item) = cx.deref(item, &mut Vec::new()) else {
        return secs;
    };
    let Some(body) = item
        .entries()
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(&op.method))
        .map(|(_, v)| v)
    else {
        return secs;
    };
    let request = match body.get("requestBody") {
        Some(rb) => cx.body_schema(rb),
        None if root.get("swagger").is_some() => {
            let params = body.get("parameters").map_or(&[][..], YNode::items);
            let shared = item.get("parameters").map_or(&[][..], YNode::items);
            let mut found = None;
            for p in params.iter().chain(shared) {
                if let Ok(p) = cx.deref(p, &mut Vec::new())
                    && p.str_at("in") == Some("body")
                {
                    found = p.get("schema").map(Body::Schema);
                    break;
                }
            }
            found
        }
        None => None,
    };
    cx.section(&mut secs, "request".to_string(), request);
    for (code, resp) in body.get("responses").map_or(&[][..], YNode::entries) {
        let schema = cx.body_schema(resp);
        cx.section(&mut secs, format!("response:{code}"), schema);
    }
    secs
}

/// The payload fields of one AsyncAPI channel op. v2:
/// `channels.<channel>.<publish|subscribe>.message` (a `oneOf` of messages
/// lists each); v3: the operation's `messages`, else its channel's. One message
/// is section `payload`; several are `payload:<name>` each.
fn asyncapi_sections<'a>(cx: &mut FieldCx<'a>, op: &ChannelOp) -> Sections {
    let mut secs = Sections::new();
    let root = cx.root;
    let v3 = root.str_at("asyncapi").is_some_and(|v| v.split('.').next() == Some("3"));
    let mut messages: Vec<(String, Result<&'a YNode, &'a str>)> = Vec::new();
    if v3 {
        let found = op
            .operation_id
            .as_deref()
            .and_then(|id| root.get("operations")?.get(id));
        let Some(Ok(opn)) = found.map(|o| cx.deref(o, &mut Vec::new())) else {
            return secs;
        };
        if let Some(YNode::List(ms)) = opn.get("messages") {
            for m in ms {
                let name = m.str_at("$ref").map_or("message", ref_name).to_string();
                messages.push((name, cx.deref(m, &mut Vec::new())));
            }
        } else if let Some(Ok(chan)) = opn.get("channel").map(|c| cx.deref(c, &mut Vec::new())) {
            for (k, m) in chan.get("messages").map_or(&[][..], YNode::entries) {
                messages.push((k.clone(), cx.deref(m, &mut Vec::new())));
            }
        }
    } else {
        let Some(chan) = root.get("channels").and_then(|c| c.get(&op.channel)) else {
            return secs;
        };
        let Ok(chan) = cx.deref(chan, &mut Vec::new()) else {
            return secs;
        };
        let Some(Ok(action)) = chan.get(op.action).map(|a| cx.deref(a, &mut Vec::new())) else {
            return secs;
        };
        let Some(message) = action.get("message") else {
            return secs;
        };
        match cx.deref(message, &mut Vec::new()) {
            Ok(m) if matches!(m.get("oneOf"), Some(YNode::List(_))) => {
                for (i, x) in m.get("oneOf").map_or(&[][..], YNode::items).iter().enumerate() {
                    let resolved = cx.deref(x, &mut Vec::new());
                    let name = x
                        .str_at("$ref")
                        .map(ref_name)
                        .or_else(|| resolved.ok().and_then(|r| r.str_at("name")))
                        .map_or_else(|| i.to_string(), str::to_string);
                    messages.push((name, resolved));
                }
            }
            other => messages.push((String::new(), other)),
        }
    }
    let single = messages.len() == 1;
    for (name, m) in messages {
        let section = if single { "payload".to_string() } else { format!("payload:{name}") };
        let body = match m {
            Ok(m) => m.get("payload").map(Body::Schema),
            Err(r) => Some(Body::External(r)),
        };
        cx.section(&mut secs, section, body);
    }
    secs
}

/// A Pact body: the JSON example itself, or a v4 `{content, contentType,
/// encoded}` wrapper's `content` when it is not encoded (base64 bodies list
/// nothing).
fn pact_body(v: &YNode) -> Option<&YNode> {
    let wrapper = v.get("content").is_some()
        && v.entries().iter().all(|(k, _)| {
            matches!(k.as_str(), "content" | "contentType" | "contentTypeHint" | "encoded")
        });
    if !wrapper {
        return Some(v);
    }
    let encoded = v
        .get("encoded")
        .is_some_and(|e| !matches!(e, YNode::Scalar(s, _) if s == "false" || s.is_empty()));
    if encoded { None } else { v.get("content") }
}

/// One Pact interaction's `request.body` → section `request`, and
/// `response.body` → section `response:<status>` (`response` when no status
/// is written).
fn pact_sections<'a>(cx: &mut FieldCx<'a>, it: &'a YNode) -> Sections {
    let mut secs = Sections::new();
    if let Some(b) = it.get("request").and_then(|r| r.get("body")).and_then(pact_body) {
        let fields = cx.example(b);
        if !fields.is_empty() {
            secs.push(("request".to_string(), fields));
        }
    }
    if let Some(resp) = it.get("response")
        && let Some(b) = resp.get("body").and_then(pact_body)
    {
        let fields = cx.example(b);
        let name = match resp.str_at("status").filter(|s| !s.is_empty()) {
            Some(s) => format!("response:{s}"),
            None => "response".to_string(),
        };
        if !fields.is_empty() {
            secs.push((name, fields));
        }
    }
    secs
}

/// The SCHEMA_FIELDS cell for one contract op: `{"format":<format>` then one
/// member per section (`"request":[..]`, `"response:201":[..]`,
/// `"payload":[..]`), each a list of `{"name","type"}` objects carrying
/// `"required":true` when declared, then `"truncated":true` when a cap cut
/// the walk short — LE.10a's object convention (`format` + named field-list
/// sections), so LE.10c compares one shape.
fn fields_cell(format: &str, secs: &Sections, truncated: bool) -> Cell {
    let mut j = format!(r#"{{"format":{}"#, json_str(format));
    for (name, fields) in secs {
        j.push(',');
        j.push_str(&json_str(name));
        j.push_str(":[");
        for (i, f) in fields.iter().enumerate() {
            if i > 0 {
                j.push(',');
            }
            j.push_str(&format!(r#"{{"name":{},"type":{}"#, json_str(&f.name), json_str(&f.ty)));
            if f.required {
                j.push_str(r#","required":true"#);
            }
            j.push('}');
        }
        j.push(']');
    }
    if truncated {
        j.push_str(r#","truncated":true"#);
    }
    j.push('}');
    Cell { kind: cell_type::SCHEMA_FIELDS, payload: CellPayload::Json(j) }
}

/// Add the op's SCHEMA_FIELDS cell to the node [`push_op`] just pushed, when
/// any section lists a field, and count it.
fn attach_fields(out: &mut ContractNodes, format: &str, secs: &Sections, truncated: bool) {
    if secs.is_empty() {
        return;
    }
    let Some(node) = out.nodes.last_mut() else {
        return;
    };
    node.cells.push(fields_cell(format, secs, truncated));
    out.field_stats.ops_with_fields += 1;
    out.field_stats.fields += secs.iter().map(|(_, f)| f.len()).sum::<usize>();
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

    // ------------------------------------------------------------------
    // LE.10b — SCHEMA_FIELDS on contract ops
    // ------------------------------------------------------------------

    /// The SCHEMA_FIELDS payload of the op named `qname`, if it has one.
    fn fields_of<'a>(out: &'a ContractNodes, qname: &str) -> Option<&'a str> {
        let node = out.nodes.iter().find(|n| out.nav.qname_by_id[&n.id] == qname)?;
        node.cells.iter().find(|c| c.kind == cell_type::SCHEMA_FIELDS).map(|c| match &c.payload {
            CellPayload::Json(j) => j.as_str(),
            other => panic!("SCHEMA_FIELDS must be Json, got {other:?}"),
        })
    }

    fn yaml(src: &str, path: &str) -> ContractNodes {
        extract_yaml_contracts(src, path, module_id(), repo())
    }

    fn json(src: &str, path: &str) -> ContractNodes {
        extract_json_contract(src, path, module_id(), repo())
    }

    const ORDERS_YAML: &str = r#"openapi: 3.0.3
paths:
  /orders:
    post:
      operationId: createOrder
      requestBody:
        required: true
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/OrderRequest'
      responses:
        '201':
          description: created
components:
  schemas:
    OrderRequest:
      type: object
      required: [sku, quantity]
      properties:
        sku:
          type: string
        quantity:
          type: integer
          format: int32
        coupon:
          type: string
          nullable: true
        address:
          $ref: '#/components/schemas/Address'
        lines:
          type: array
          items:
            type: object
            required:
              - sku
            properties:
              sku: { type: string }
    Address:
      type: object
      required:
      - city
      properties:
        city:
          type: string
"#;

    const ORDERS_CELL: &str = concat!(
        r#"{"format":"openapi","request":["#,
        r#"{"name":"sku","type":"string","required":true},"#,
        r#"{"name":"quantity","type":"integer(int32)","required":true},"#,
        r#"{"name":"coupon","type":"string|null"},"#,
        r#"{"name":"address","type":"object"},"#,
        r#"{"name":"address.city","type":"string","required":true},"#,
        r#"{"name":"lines[]","type":"object"},"#,
        r#"{"name":"lines[].sku","type":"string","required":true}]}"#
    );

    #[test]
    fn openapi_request_ref_and_required() {
        // requestBody through a local $ref, `required` as a flow list, a block
        // list and a compact list at its key's own indent; a nested $ref object
        // flattened as `parent.child`; an array of objects as `name[]`. The
        // 201 response declares no schema, so it is no section.
        let out = yaml(ORDERS_YAML, "openapi.yaml");
        assert_eq!(fields_of(&out, "contract::openapi::POST:/orders"), Some(ORDERS_CELL));
        // The cell comes after the existing three, which are unchanged.
        let kinds: Vec<_> = out.nodes[0].cells.iter().map(|c| c.kind).collect();
        assert_eq!(
            kinds,
            vec![cell_type::CODE, cell_type::POSITION, cell_type::ORIGIN, cell_type::SCHEMA_FIELDS]
        );
        assert_eq!(
            out.field_stats,
            FieldStats { ops_with_fields: 1, fields: 7, refs_resolved: 2, refs_external: 0 }
        );
    }

    #[test]
    fn openapi_inline_response_fields() {
        // Per status code, in document order: application/json preferred over
        // an earlier text/plain and a `+json`; allOf merged (its members'
        // `required` too); a 3.1 type list; oneOf named without descending; a
        // `+json` media type when no plain JSON one; an array root as `[]`.
        let src = r#"openapi: 3.1.0
paths:
  /orders/{id}:
    get:
      responses:
        '200':
          content:
            text/plain:
              schema:
                type: string
            application/problem+json:
              schema:
                type: object
                properties:
                  title: {type: string}
            application/json:
              schema:
                allOf:
                  - $ref: '#/components/schemas/Base'
                  - type: object
                    required: [total]
                    properties:
                      total:
                        type: [number, 'null']
                      payer:
                        oneOf:
                          - $ref: '#/components/schemas/Card'
                          - type: string
        '404':
          content:
            application/problem+json:
              schema:
                type: object
                properties:
                  title: {type: string}
        default:
          content:
            text/plain:
              schema:
                type: array
                items:
                  type: string
components:
  schemas:
    Base:
      type: object
      required: [id]
      properties:
        id:
          type: string
          format: uuid
    Card:
      type: object
"#;
        let out = yaml(src, "openapi.yaml");
        assert_eq!(
            fields_of(&out, "contract::openapi::GET:/orders/{id}"),
            Some(concat!(
                r#"{"format":"openapi","response:200":["#,
                r#"{"name":"id","type":"string(uuid)","required":true},"#,
                r#"{"name":"total","type":"number|null","required":true},"#,
                r#"{"name":"payer","type":"oneOf<Card|string>"}],"#,
                r#""response:404":[{"name":"title","type":"string"}],"#,
                r#""response:default":[{"name":"[]","type":"string"}]}"#
            ))
        );
    }

    #[test]
    fn openapi_json_twin_same_cell() {
        // The JSON twin of ORDERS_YAML with its keys in document order (not
        // sorted): the cell is byte-identical, whatever map order serde_json
        // was built with.
        let src = r##"{
  "openapi": "3.0.3",
  "paths": {
    "/orders": {
      "post": {
        "operationId": "createOrder",
        "requestBody": {
          "required": true,
          "content": {"application/json": {"schema": {"$ref": "#/components/schemas/OrderRequest"}}}
        },
        "responses": {"201": {"description": "created"}}
      }
    }
  },
  "components": {
    "schemas": {
      "OrderRequest": {
        "type": "object",
        "required": ["sku", "quantity"],
        "properties": {
          "sku": {"type": "string"},
          "quantity": {"type": "integer", "format": "int32"},
          "coupon": {"type": "string", "nullable": true},
          "address": {"$ref": "#/components/schemas/Address"},
          "lines": {
            "type": "array",
            "items": {"type": "object", "required": ["sku"], "properties": {"sku": {"type": "string"}}}
          }
        }
      },
      "Address": {"type": "object", "required": ["city"], "properties": {"city": {"type": "string"}}}
    }
  }
}"##;
        let j = json(src, "openapi.json");
        let y = yaml(ORDERS_YAML, "openapi.yaml");
        let q = "contract::openapi::POST:/orders";
        assert_eq!(fields_of(&j, q), Some(ORDERS_CELL));
        assert_eq!(fields_of(&j, q), fields_of(&y, q));
        assert_eq!(j.field_stats, y.field_stats);
    }

    #[test]
    fn swagger2_body_parameter_and_definitions() {
        let src = r#"swagger: "2.0"
paths:
  /pets:
    post:
      parameters:
        - in: query
          name: dryRun
          type: boolean
        - in: body
          name: pet
          schema:
            $ref: '#/definitions/Pet'
      responses:
        200:
          schema:
            type: array
            items:
              $ref: '#/definitions/Pet'
definitions:
  Pet:
    type: object
    required: [name]
    properties:
      name: {type: string}
"#;
        let out = yaml(src, "swagger.yaml");
        assert_eq!(
            fields_of(&out, "contract::swagger::POST:/pets"),
            Some(concat!(
                r#"{"format":"openapi","request":[{"name":"name","type":"string","required":true}],"#,
                r#""response:200":[{"name":"[]","type":"object"},{"name":"[].name","type":"string","required":true}]}"#
            ))
        );
    }

    const ASYNC_V2: &str = r#"asyncapi: 2.6.0
channels:
  orders.placed:
    publish:
      message:
        payload:
          type: object
          required: [orderId]
          properties:
            orderId: {type: string}
            totalCents: {type: integer}
    subscribe:
      message:
        $ref: '#/components/messages/OrderPlaced'
  orders.mixed:
    subscribe:
      message:
        oneOf:
          - $ref: '#/components/messages/OrderPlaced'
          - name: OrderCancelled
            payload:
              type: object
              properties:
                reason: {type: string}
components:
  messages:
    OrderPlaced:
      payload:
        $ref: '#/components/schemas/OrderPlacedPayload'
  schemas:
    OrderPlacedPayload:
      type: object
      properties:
        orderId: {type: string}
        currency: {type: string}
"#;

    #[test]
    fn asyncapi_v2_inline_and_message_ref() {
        let out = yaml(ASYNC_V2, "asyncapi.yaml");
        assert_eq!(
            fields_of(&out, "contract::asyncapi::publish:orders.placed"),
            Some(r#"{"format":"asyncapi","payload":[{"name":"orderId","type":"string","required":true},{"name":"totalCents","type":"integer"}]}"#)
        );
        // message → components/messages → its payload → components/schemas.
        let placed = r#"[{"name":"orderId","type":"string"},{"name":"currency","type":"string"}]"#;
        assert_eq!(
            fields_of(&out, "contract::asyncapi::subscribe:orders.placed"),
            Some(format!(r#"{{"format":"asyncapi","payload":{placed}}}"#).as_str())
        );
        // A oneOf of messages is one section per message.
        assert_eq!(
            fields_of(&out, "contract::asyncapi::subscribe:orders.mixed"),
            Some(
                format!(
                    r#"{{"format":"asyncapi","payload:OrderPlaced":{placed},"payload:OrderCancelled":[{{"name":"reason","type":"string"}}]}}"#
                )
                .as_str()
            )
        );
        assert_eq!(out.field_stats.ops_with_fields, 3);
        assert_eq!(out.field_stats.refs_external, 0);
    }

    #[test]
    fn asyncapi_v3_payload() {
        // `send` names its messages (a ref into the channel, which refs a
        // component); `receive` names none, so its channel's messages count.
        let src = r#"asyncapi: 3.0.0
channels:
  userSignedup:
    address: user/signedup
    messages:
      UserSignedUp:
        $ref: '#/components/messages/UserSignedUp'
operations:
  sendUserSignedup:
    action: send
    channel:
      $ref: '#/channels/userSignedup'
    messages:
      - $ref: '#/channels/userSignedup/messages/UserSignedUp'
  onUserSignedup:
    action: receive
    channel: { $ref: '#/channels/userSignedup' }
components:
  messages:
    UserSignedUp:
      payload:
        type: object
        properties:
          userId: {type: string}
          signedUpAt: {type: string, format: date-time}
"#;
        let cell = r#"{"format":"asyncapi","payload":[{"name":"userId","type":"string"},{"name":"signedUpAt","type":"string(date-time)"}]}"#;
        let out = yaml(src, "asyncapi.yaml");
        assert_eq!(fields_of(&out, "contract::asyncapi::publish:user/signedup"), Some(cell));
        assert_eq!(fields_of(&out, "contract::asyncapi::subscribe:user/signedup"), Some(cell));

        let v3_json = r##"{"asyncapi": "3.0.0",
 "channels": {"userSignedup": {"address": "user/signedup", "messages": {"UserSignedUp": {"$ref": "#/components/messages/UserSignedUp"}}}},
 "operations": {"onUserSignedup": {"action": "receive", "channel": {"$ref": "#/channels/userSignedup"}}},
 "components": {"messages": {"UserSignedUp": {"payload": {"type": "object", "properties": {
   "userId": {"type": "string"}, "signedUpAt": {"type": "string", "format": "date-time"}}}}}}}"##;
        let j = json(v3_json, "asyncapi.json");
        assert_eq!(fields_of(&j, "contract::asyncapi::subscribe:user/signedup"), Some(cell));
    }

    #[test]
    fn pact_body_keys_and_types() {
        let pact = r#"{
  "consumer": {"name": "web"},
  "provider": {"name": "orders"},
  "interactions": [
    {"description": "place", "request": {"method": "POST", "path": "/orders",
      "body": {"sku": "A", "quantity": 2, "price": 9.5, "giftWrap": true, "note": null, "tags": ["x"],
               "lines": [{"sku": "A"}, {"sku": "B", "qty": 1}], "empty": [], "address": {"city": "Oslo"}}},
     "response": {"status": 201, "body": {"id": "o-1"}}},
    {"description": "place again", "request": {"method": "POST", "path": "/orders", "body": {"other": 1}}},
    {"description": "list", "request": {"method": "GET", "path": "/orders"},
     "response": {"status": 200, "body": [{"id": "o-1"}]}},
    {"description": "v4", "type": "Synchronous/HTTP", "request": {"method": "PUT", "path": "/orders/1",
      "body": {"content": {"sku": "A"}, "contentType": "application/json", "encoded": false}},
     "response": {"status": 204}},
    {"description": "b64", "request": {"method": "PATCH", "path": "/orders/1",
      "body": {"content": "eyJ9", "contentType": "application/json", "encoded": "base64"}},
     "response": {"status": 204}}
  ]
}"#;
        let out = json(pact, "pacts/web-orders.json");
        // Types come from the example values; `required` is never claimed; the
        // second interaction on POST /orders is the same op, first one wins;
        // an array of objects contributes the union of its items' keys.
        assert_eq!(
            fields_of(&out, "contract::web-orders::POST:/orders"),
            Some(concat!(
                r#"{"format":"pact","request":["#,
                r#"{"name":"sku","type":"string"},{"name":"quantity","type":"integer"},"#,
                r#"{"name":"price","type":"number"},{"name":"giftWrap","type":"boolean"},"#,
                r#"{"name":"note","type":"null"},{"name":"tags[]","type":"string"},"#,
                r#"{"name":"lines[]","type":"object"},{"name":"lines[].sku","type":"string"},"#,
                r#"{"name":"lines[].qty","type":"integer"},{"name":"empty[]","type":"any"},"#,
                r#"{"name":"address","type":"object"},{"name":"address.city","type":"string"}],"#,
                r#""response:201":[{"name":"id","type":"string"}]}"#
            ))
        );
        assert_eq!(
            fields_of(&out, "contract::web-orders::GET:/orders"),
            Some(r#"{"format":"pact","response:200":[{"name":"[]","type":"object"},{"name":"[].id","type":"string"}]}"#)
        );
        // A v4 body wrapper is unwrapped; an encoded one lists nothing.
        assert_eq!(
            fields_of(&out, "contract::web-orders::PUT:/orders/1"),
            Some(r#"{"format":"pact","request":[{"name":"sku","type":"string"}]}"#)
        );
        assert_eq!(fields_of(&out, "contract::web-orders::PATCH:/orders/1"), None);
        assert_eq!(out.field_stats.ops_with_fields, 3);
        assert_eq!(out.field_stats.fields, 16);
    }

    #[test]
    fn ref_cycle_terminates() {
        let src = r#"openapi: 3.0.0
paths:
  /tree:
    post:
      requestBody:
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/Node'
      responses:
        '200':
          content:
            application/json:
              schema:
                $ref: '#/components/schemas/Loop'
components:
  schemas:
    Node:
      type: object
      properties:
        name: {type: string}
        parent:
          $ref: '#/components/schemas/Node'
        children:
          type: array
          items:
            $ref: '#/components/schemas/Node'
        meta:
          allOf:
            - $ref: '#/components/schemas/Node'
    Loop:
      $ref: '#/components/schemas/Loop2'
    Loop2:
      $ref: '#/components/schemas/Loop'
"#;
        let out = yaml(src, "openapi.yaml");
        // A ref already being expanded is listed with its type, never
        // descended; a ref-only cycle gives up after MAX_REF_HOPS and is
        // listed as the ref it could not resolve.
        assert_eq!(
            fields_of(&out, "contract::openapi::POST:/tree"),
            Some(concat!(
                r#"{"format":"openapi","request":["#,
                r#"{"name":"name","type":"string"},{"name":"parent","type":"object"},"#,
                r#"{"name":"children[]","type":"object"},{"name":"meta","type":"object"}],"#,
                r##""response:200":[{"name":"$ref","type":"#/components/schemas/Loop"}]}"##
            ))
        );
    }

    #[test]
    fn nesting_past_depth_four_is_listed_not_descended() {
        let src = r#"openapi: 3.0.0
paths:
  /deep:
    put:
      requestBody:
        content:
          application/json:
            schema:
              type: object
              properties:
                a:
                  type: object
                  properties:
                    b:
                      type: object
                      properties:
                        c:
                          type: object
                          properties:
                            d:
                              type: object
                              properties:
                                e: {type: string}
                external:
                  $ref: './common.yaml#/Money'
      responses: {}
"#;
        let out = yaml(src, "openapi.yaml");
        assert_eq!(
            fields_of(&out, "contract::openapi::PUT:/deep"),
            Some(concat!(
                r#"{"format":"openapi","request":["#,
                r#"{"name":"a","type":"object"},{"name":"a.b","type":"object"},"#,
                r#"{"name":"a.b.c","type":"object"},{"name":"a.b.c.d","type":"object"},"#,
                r#"{"name":"external","type":"./common.yaml#/Money"}]}"#
            ))
        );
        assert_eq!(out.field_stats.refs_external, 1);
    }

    #[test]
    fn unsupported_yaml_constructs_yield_no_fields() {
        // An alias, a merge key, block scalars (whose indented body looks like
        // structure), a multi-line quoted scalar and a complex key are skipped;
        // an anchor on a value is stripped and the value read.
        let src = r#"openapi: 3.0.0
paths:
  /a:
    post:
      description: |
        Not structure:
          properties:
            fake: {type: string}
      requestBody:
        content:
          application/json:
            schema: *shared
      responses:
        '200':
          content:
            application/json:
              schema:
                <<: *base
                type: object
                properties:
                  kept: {type: string}
                  folded: >
                    text
                  also: &anchor
                    type: integer
                  quoted: "a multi
                    line"
                  ? complex
                  : value
"#;
        let out = yaml(src, "openapi.yaml");
        assert_eq!(out.nodes.len(), 1, "the op itself is unaffected");
        assert_eq!(
            fields_of(&out, "contract::openapi::POST:/a"),
            Some(r#"{"format":"openapi","response:200":[{"name":"kept","type":"string"},{"name":"also","type":"integer"}]}"#)
        );
        // Only unsupported constructs: the op gets no cell at all.
        let only = "openapi: 3.0.0\npaths:\n  /b:\n    get:\n      responses:\n        '200':\n          content:\n            application/json:\n              schema: *shared\n";
        let out = yaml(only, "openapi.yaml");
        assert_eq!(out.nodes.len(), 1);
        assert_eq!(fields_of(&out, "contract::openapi::GET:/b"), None);
        assert_eq!(out.field_stats, FieldStats::default());
    }

    #[test]
    fn ops_without_a_body_schema_get_no_cell() {
        let openapi = "openapi: 3.0.0\npaths:\n  /a:\n    get:\n      responses:\n        '200':\n          description: ok\n";
        let asyncapi = "asyncapi: 2.6.0\nchannels:\n  orders:\n    publish:\n      message:\n        name: Order\n";
        let pact = r#"{"consumer":{"name":"w"},"provider":{"name":"a"},"interactions":[{"request":{"method":"GET","path":"/a"},"response":{"status":200}}]}"#;
        for out in [yaml(openapi, "o.yaml"), yaml(asyncapi, "e.yaml"), json(pact, "p.json")] {
            assert_eq!(out.nodes.len(), 1);
            assert!(out.nodes[0].cells.iter().all(|c| c.kind != cell_type::SCHEMA_FIELDS));
            assert_eq!(out.field_stats, FieldStats::default());
        }
    }

    #[test]
    fn yaml_scalars_and_flow_collections() {
        let doc = yaml_document(
            "a: 'it''s'\nb: \"q\\\"x\\\\y\"\nc: [1, 2.5, true, ~, 'x, y', {k: v}]\nd: plain # comment\ne: {f: [g, h], 'i j': \"k\"}\nl: [\n  m,\n  n\n]\n",
        );
        assert_eq!(doc.str_at("a"), Some("it's"));
        assert_eq!(doc.str_at("b"), Some("q\"x\\y"));
        let c: Vec<(&str, &str)> = doc
            .get("c")
            .map_or(&[][..], YNode::items)
            .iter()
            .filter_map(|v| match v {
                YNode::Scalar(s, t) => Some((s.as_str(), *t)),
                _ => None,
            })
            .collect();
        assert_eq!(
            c,
            [("1", "integer"), ("2.5", "number"), ("true", "boolean"), ("~", "null"), ("x, y", "string")]
        );
        assert_eq!(doc.get("c").map(|v| v.items().len()), Some(6));
        assert_eq!(doc.str_at("d"), Some("plain"));
        let e = doc.get("e").expect("flow map");
        assert_eq!(e.get("f").map(|f| f.items().len()), Some(2));
        assert_eq!(e.str_at("i j"), Some("k"));
        let l: Vec<&str> = doc.get("l").map_or(&[][..], YNode::items).iter().filter_map(YNode::as_str).collect();
        assert_eq!(l, ["m", "n"], "a flow list spanning lines is joined");
    }

    #[test]
    fn yaml_scanner_never_panics_on_arbitrary_input() {
        // Every char-prefix of real specs (so every construct is cut mid-way),
        // multibyte text in keys / values / quotes / escapes, unbalanced
        // brackets and deep nesting: never a panic, never a stack overflow.
        let deep_block: String = (0..3000).map(|i| format!("{}k{i}:\n", " ".repeat(i))).collect();
        let deep_list: String = (0..3000).map(|i| format!("{}- \n", " ".repeat(i))).collect();
        let deep_flow = format!("openapi: 3.0.0\nx: {}\n", "[".repeat(5000));
        let nasty = [
            "openapi: 3.0.0\npaths:\n  /é:\n    get:\n      responses:\n        'ü':\n          content:\n            application/json:\n              schema: {type: \"ß\\é\", properties: {ñ: {type: string}}}\n",
            "openapi: 3.0.0\npaths:\n  /a:\n    post:\n      requestBody: {content: {application/json: {schema: {$ref: '#/%zz/~9/é'}}}}\n      responses: {'200': [}\n",
            "openapi: 3.0.0\n- - - -\n  : :\n? ?\n\t\tx: \"\n'\n[{]}\n- 'é\n  \"é\\",
            "openapi: 3.0.0\npaths:\n  /a:\n    get:\n      responses:\n        200:\n          content:\n            a/json:\n              schema:\n                properties:\n                  x:\n                    $ref: '#/paths/~1a/get/responses/200/content/a~1json/schema'\n",
        ];
        let mut docs: Vec<String> = vec![ORDERS_YAML.to_string(), ASYNC_V2.to_string(), deep_block, deep_list, deep_flow];
        docs.extend(nasty.iter().map(|s| s.to_string()));
        for doc in &docs {
            let _ = yaml_document(doc);
            let _ = yaml(doc, "x.yaml");
            let _ = yaml(&format!("openapi: 3.0.0\npaths:\n  /a:\n    get:\n      responses:\n        '200':\n          content:\n            application/json:\n              schema:\n{}", doc), "y.yaml");
        }
        for doc in [ORDERS_YAML, ASYNC_V2, nasty[0], nasty[2]] {
            for (i, _) in doc.char_indices() {
                let _ = yaml(&doc[..i], "p.yaml");
            }
        }
        // A self-referencing inline pointer (the schema IS its own property)
        // terminates.
        let out = yaml(nasty[3], "openapi.yaml");
        assert!(fields_of(&out, "contract::openapi::GET:/a").is_some());
    }

    #[test]
    fn contract_counts_record_fields() {
        let mut c = ContractCounts::default();
        c.record(&yaml(ORDERS_YAML, "openapi.yaml"));
        c.record(&yaml(ASYNC_V2, "asyncapi.yaml"));
        c.record(&yaml("openapi: 3.0.0\npaths:\n  /a:\n    get:\n      summary: x\n", "o.yaml"));
        assert_eq!((c.files, c.openapi, c.asyncapi, c.pact), (3, 2, 3, 0));
        // 7 request fields + 2 + 2 + (2 + 1) payload fields; refs: the
        // openapi's 2, then message + payload for each of the two ops that
        // name OrderPlaced.
        assert_eq!(
            (c.ops_with_fields, c.fields, c.refs_resolved, c.refs_external),
            (4, 14, 6, 0)
        );
    }

    #[test]
    fn openapi_pact_fields_fixture_payloads() {
        let provider = include_str!(
            "../../../../bench/substrate-gap/fixtures/openapi-pact-fields/provider/openapi.yaml"
        );
        let pact = include_str!(
            "../../../../bench/substrate-gap/fixtures/openapi-pact-fields/web/pacts/web-orders.json"
        );
        let p = yaml(provider, "openapi.yaml");
        assert_eq!(
            fields_of(&p, "contract::openapi::POST:/orders"),
            Some(concat!(
                r#"{"format":"openapi","request":["#,
                r#"{"name":"sku","type":"string","required":true},"#,
                r#"{"name":"quantity","type":"integer","required":true},"#,
                r#"{"name":"coupon","type":"string"}],"#,
                r#""response:201":[{"name":"id","type":"string"}]}"#
            ))
        );
        let w = json(pact, "pacts/web-orders.json");
        assert_eq!(
            fields_of(&w, "contract::web-orders::POST:/orders"),
            Some(concat!(
                r#"{"format":"pact","request":["#,
                r#"{"name":"sku","type":"string"},{"name":"quantity","type":"integer"},"#,
                r#"{"name":"giftWrap","type":"boolean"}],"#,
                r#""response:201":[{"name":"id","type":"string"}]}"#
            ))
        );
    }

    #[test]
    fn asyncapi_payload_fields_fixture_payloads() {
        let orders = include_str!(
            "../../../../bench/substrate-gap/fixtures/asyncapi-payload-fields/orders/asyncapi.yaml"
        );
        let billing = include_str!(
            "../../../../bench/substrate-gap/fixtures/asyncapi-payload-fields/billing/asyncapi.yaml"
        );
        assert_eq!(
            fields_of(&yaml(orders, "asyncapi.yaml"), "contract::asyncapi::publish:orders.placed"),
            Some(r#"{"format":"asyncapi","payload":[{"name":"orderId","type":"string","required":true},{"name":"totalCents","type":"integer"}]}"#)
        );
        assert_eq!(
            fields_of(&yaml(billing, "asyncapi.yaml"), "contract::asyncapi::subscribe:orders.placed"),
            Some(r#"{"format":"asyncapi","payload":[{"name":"orderId","type":"string"},{"name":"currency","type":"string"}]}"#)
        );
    }

    // ------------------------------------------------------------------
    // LE.9a — feature-scoped declarations
    // ------------------------------------------------------------------

    /// `(qname, ORIGIN, POSITION)` of every op, in emission order.
    fn ops_of(out: &ContractNodes) -> Vec<(String, String, String)> {
        out.nodes
            .iter()
            .map(|n| {
                (
                    out.nav.qname_by_id[&n.id].clone(),
                    cell_text(n, cell_type::ORIGIN).to_string(),
                    cell_text(n, cell_type::POSITION).to_string(),
                )
            })
            .collect()
    }

    const QUOKKA_FEATURE: &str = "name: Activities
status: complete
backend_routes:
  protected:
    - POST /api/protected/activity                       # create activity
    - GET  /api/protected/activity/:id                  # get detail
  public:
    - get /api/public/activities
frontend_components:
  - web/src/app/features/activities/activities.component.ts
data_model:
  - GET /not/a/route/either
";

    #[test]
    fn feature_yaml_grouped_items() {
        let out = yaml(QUOKKA_FEATURE, "features/activities/feature.yaml");
        assert_eq!(out.source, Some(ContractSource::FeatureYaml));
        assert_eq!(out.feature.as_deref(), Some("activities"));
        let ops = ops_of(&out);
        let qnames: Vec<&str> = ops.iter().map(|(q, _, _)| q.as_str()).collect();
        assert_eq!(
            qnames,
            [
                "contract::feature:activities::POST:/api/protected/activity",
                "contract::feature:activities::GET:/api/protected/activity/:id",
                "contract::feature:activities::GET:/api/public/activities",
            ],
            "frontend_components and data_model are never read"
        );
        assert_eq!(
            ops[0].1,
            r#"{"provenance":"contract","source":"feature_yaml","feature":"activities","group":"protected","method":"POST","path":"/api/protected/activity","raw_path":"/api/protected/activity"}"#
        );
        assert!(ops[2].1.contains(r#""group":"public""#), "{}", ops[2].1);
        assert_eq!(
            ops[1].2,
            r#"{"file":"features/activities/feature.yaml","start_line":5,"end_line":5}"#,
            "POSITION is the item's 0-indexed line"
        );
        let names: Vec<&str> = out.nodes.iter().map(|n| out.nav.name_by_id[&n.id].as_str()).collect();
        assert_eq!(names[1], "GET /api/protected/activity/:id");
        for n in &out.nodes {
            assert_eq!(out.nav.kind_by_id[&n.id], node_kind::DOC_SECTION);
        }
    }

    #[test]
    fn feature_yaml_ungrouped_and_compact_items() {
        // Items directly under the key (no group), a sequence at its key's
        // own indent, and a flow list on the group key's line.
        let src = "backend_routes:\n- DELETE /a/:id\n- POST /b\nother: x\n";
        let out = yaml(src, "features/a/feature.yml");
        let ops = ops_of(&out);
        assert_eq!(ops.len(), 2);
        assert!(!ops[0].1.contains("\"group\""), "no group key: {}", ops[0].1);
        let src = "backend_routes:\n  protected:\n  - PUT /c\n  public: [GET /d, \"POST /e\"]\n";
        let ops = ops_of(&yaml(src, "features/b/feature.yaml"));
        let got: Vec<(&str, bool)> = ops
            .iter()
            .map(|(q, o, _)| (q.as_str(), o.contains(r#""group":"public""#)))
            .collect();
        assert_eq!(
            got,
            [
                ("contract::feature:b::PUT:/c", false),
                ("contract::feature:b::GET:/d", true),
                ("contract::feature:b::POST:/e", true),
            ]
        );
    }

    #[test]
    fn feature_yaml_empty_list() {
        let src = "name: Marketing\nbackend_routes: []\nfrontend_components:\n  - GET /looks/like/a/route\n";
        let out = yaml(src, "features/marketing/feature.yaml");
        assert!(out.nodes.is_empty());
        assert_eq!(out.source, Some(ContractSource::FeatureYaml));
        let mut c = ContractCounts::default();
        c.record(&out);
        assert_eq!((c.files, c.feature_yaml, c.feature_yaml_features.len()), (0, 0, 0));
    }

    #[test]
    fn feature_yaml_query_and_comment_stripped() {
        let src = "backend_routes:
  protected:
    - GET  /api/protected/activities/city?city=&page=   # paginated list
    - \"GET /api/quoted\"   # quoted item
    - GET /api/protected/activities/city   # a duplicate once the query is dropped
    - FETCH /api/not-a-method
    - GET api/no-leading-slash
    - method: GET
";
        let ops = ops_of(&yaml(src, "features/activities/feature.yaml"));
        let qnames: Vec<&str> = ops.iter().map(|(q, _, _)| q.as_str()).collect();
        assert_eq!(
            qnames,
            [
                "contract::feature:activities::GET:/api/protected/activities/city",
                "contract::feature:activities::GET:/api/quoted",
            ]
        );
        assert!(ops[0].1.contains(r#""path":"/api/protected/activities/city","raw_path":"/api/protected/activities/city""#));
    }

    #[test]
    fn non_features_dir_feature_yaml_ignored() {
        // The gate needs BOTH the `features/<f>/` path and the key.
        for path in ["feature.yaml", "config/activities/feature.yaml", "features/feature.yaml", "features/a/other.yaml"] {
            let out = yaml(QUOKKA_FEATURE, path);
            assert!(out.nodes.is_empty(), "{path}");
            assert_eq!(out.source, None, "{path}");
        }
        // A features/<f>/feature.yaml without the key is not a feature list;
        // it still takes the ordinary contract sniff.
        let no_key = "name: X\nroutes:\n  - GET /a\n";
        assert_eq!(yaml(no_key, "features/x/feature.yaml").source, None);
        assert_eq!(feature_dir(r"features\win\feature.yml"), Some("win"));
    }

    #[test]
    fn speckit_path_qualifies_qname() {
        let src = "openapi: 3.0.3\npaths:\n  /orders:\n    get:\n      operationId: listOrders\n";
        let a = yaml(src, "specs/001-orders/contracts/openapi.yaml");
        let b = yaml(src, "specs/002-admin/contracts/openapi.yaml");
        assert_eq!(ops_of(&a)[0].0, "contract::feature:001-orders:openapi::GET:/orders");
        assert_eq!(ops_of(&b)[0].0, "contract::feature:002-admin:openapi::GET:/orders");
        assert_ne!(a.nodes[0].id, b.nodes[0].id, "two features' ops never share a NodeId");
        assert_eq!(
            ops_of(&b)[0].1,
            r#"{"provenance":"contract","source":"openapi","feature":"002-admin","method":"GET","path":"/orders","raw_path":"/orders","operation_id":"listOrders"}"#
        );
        // AsyncAPI and the JSON formats take the same scope.
        let async_src = "asyncapi: 2.6.0\nchannels:\n  orders:\n    subscribe:\n      summary: x\n";
        let c = yaml(async_src, "docs/specs/0042-events/contracts/v1/asyncapi.yaml");
        assert_eq!(ops_of(&c)[0].0, "contract::feature:0042-events:asyncapi::subscribe:orders");
        assert!(ops_of(&c)[0].1.contains(r#""source":"asyncapi","feature":"0042-events","#));
        let pact = r#"{"consumer":{"name":"w"},"provider":{"name":"a"},"interactions":[
            {"request":{"method":"GET","path":"/orders"}}]}"#;
        let p = json(pact, "specs/003-web/contracts/web-api.json");
        assert_eq!(ops_of(&p)[0].0, "contract::feature:003-web:web-api::GET:/orders");
        assert!(ops_of(&p)[0].1.contains(r#""source":"pact","feature":"003-web","#));
        let oj = json(r#"{"openapi":"3.0.0","paths":{"/a":{"get":{}}}}"#, "specs/004-x/contracts/openapi.json");
        assert_eq!(ops_of(&oj)[0].0, "contract::feature:004-x:openapi::GET:/a");
        assert_eq!(speckit_feature(r"specs\004-x\contracts\openapi.json"), Some("004-x"));

        let mut counts = ContractCounts::default();
        for out in [&a, &b, &c, &p] {
            counts.record(out);
        }
        assert_eq!((counts.files, counts.openapi, counts.asyncapi, counts.pact), (4, 2, 1, 1));
        assert_eq!(counts.speckit, 4);
        assert_eq!(
            counts.speckit_features.iter().map(String::as_str).collect::<Vec<_>>(),
            ["001-orders", "002-admin", "003-web", "0042-events"]
        );
    }

    #[test]
    fn non_speckit_paths_unchanged() {
        let src = "openapi: 3.0.3\npaths:\n  /orders:\n    get:\n      operationId: listOrders\n";
        for path in [
            "openapi.yaml",
            "specs/openapi.yaml",
            "specs/contracts/openapi.yaml",
            "specs/01-short/contracts/openapi.yaml",
            "specs/001-Orders/contracts/openapi.yaml",
            "specs/001-/contracts/openapi.yaml",
            "specs/001-orders/openapi.yaml",
            "specs/001-orders/contracts",
            "specs/001-orders/api/contracts/openapi.yaml",
        ] {
            let out = yaml(src, path);
            let ops = ops_of(&out);
            assert_eq!(ops.len(), 1, "{path}");
            assert!(ops[0].0.starts_with("contract::"), "{path}");
            assert!(!ops[0].0.contains("feature:"), "{path}: {}", ops[0].0);
            assert!(!ops[0].1.contains("\"feature\""), "{path}: {}", ops[0].1);
            assert_eq!(out.feature, None, "{path}");
        }
        assert_eq!(
            ops_of(&yaml(src, "openapi.yaml"))[0].1,
            r#"{"provenance":"contract","source":"openapi","method":"GET","path":"/orders","raw_path":"/orders","operation_id":"listOrders"}"#,
            "an unscoped ORIGIN is byte-identical to before LE.9a"
        );
    }

    #[test]
    fn feature_yaml_counts() {
        let mut c = ContractCounts::default();
        c.record(&yaml(QUOKKA_FEATURE, "features/activities/feature.yaml"));
        c.record(&yaml("backend_routes:\n  protected:\n    - POST /api/protected/swipe\n", "features/discover/feature.yaml"));
        c.record(&yaml("backend_routes: []\n", "features/marketing/feature.yaml"));
        c.record(&yaml("openapi: 3.0.0\npaths:\n  /a:\n    get:\n      summary: x\n", "openapi.yaml"));
        assert_eq!((c.files, c.openapi, c.feature_yaml, c.speckit), (3, 1, 4, 0));
        assert_eq!(
            c.feature_yaml_features.iter().map(String::as_str).collect::<Vec<_>>(),
            ["activities", "discover"]
        );
    }

    #[test]
    fn sdd_quokka_feature_fixture_ops() {
        let src = include_str!(
            "../../../../bench/substrate-gap/fixtures/sdd-quokka-feature/features/activities/feature.yaml"
        );
        let ops = ops_of(&yaml(src, "features/activities/feature.yaml"));
        let qnames: Vec<&str> = ops.iter().map(|(q, _, _)| q.as_str()).collect();
        assert_eq!(
            qnames,
            [
                "contract::feature:activities::POST:/api/protected/activity",
                "contract::feature:activities::GET:/api/protected/activity/:id",
                "contract::feature:activities::POST:/api/protected/activity/:id/leave",
            ]
        );
    }
}
