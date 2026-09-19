//! Field-level contract diff (LE.10c): producer vs consumer fields per schema
//! copy, topic, AsyncAPI channel and HTTP route, with verdicts by each
//! format's own compatibility rules.
//!
//! `message_contracts` answers "do both sides name the same type"; this answers
//! "do both sides declare the same fields". It reads the SCHEMA_FIELDS cells
//! LE.10a (proto / Avro on MESSAGE_TYPE) and LE.10b (OpenAPI / AsyncAPI / Pact
//! on contract-op DOC_SECTIONs) wrote, over pairings the graph already holds:
//!
//! - `schema_copy`: every SHARES_SCHEMA edge between two MESSAGE_TYPE nodes
//!   (producer = edge.from). The pair has no direction, so a format whose rules
//!   do (Avro: reader vs writer) is judged both ways, and gets one row per
//!   direction only when the two verdicts differ;
//! - `topic`: a paired `message_contracts` row whose producer and consumer
//!   types resolve, by their last `.` segment and uniquely within their own
//!   repo, to a MESSAGE_TYPE. Two sides resolving to one node (one shared
//!   file) are identical by construction and skipped;
//! - `channel`: an AsyncAPI publish op against a subscribe op of the same
//!   channel (surrounding `/` trimmed) declared in a different file;
//! - `route`: an OpenAPI op (provider) against a Pact interaction (consumer)
//!   with the same method and `normalise_http_path` path, a provider `{}`
//!   segment matching any one Pact segment (most specific provider wins).
//!
//! The rules are FACT-level statements about declared schemas; the pairing is
//! DERIVED, hence every row's `tier`. Nothing is guessed: a side with no
//! SCHEMA_FIELDS cell (an enum, a body-less op) makes the row `unknown`, an
//! unresolvable `$ref` is an `unknown` change rather than a type mismatch, and
//! a field absent from a `truncated` cell is never reported as dropped.
//!
//! Read-only: no node, edge or cell is added or changed. Rows are sorted, so
//! two calls over one graph serialise byte-identically.
//!
//! Fired-on marker, printed once per call that returns rows:
//! `[contract-fields] pairs=<rows> schema_copy=.. topic=.. channel=.. route=..
//! identical=.. compatible=.. breaking=.. unknown=..`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{Cell, CellPayload, NodeId};
use glia_graph::{MergedGraph, normalise_http_path};
use serde_json::Value;

use crate::{Locator, MessageContractSide, message_contracts};

/// One side of a field diff: the node whose SCHEMA_FIELDS were read.
#[derive(serde::Serialize, Debug, Clone)]
#[non_exhaustive]
pub struct FieldSide {
    /// `RepoId.0`; `GenerateResult::repo_labels` maps it to a human label.
    pub repo_id: u64,
    /// The MESSAGE_TYPE / contract-op qname; for a topic side whose message
    /// type did not resolve, the queue node's own qname.
    pub qname: String,
    /// The cell's `format` (`proto`, `avro`, `openapi`, `pact`, `asyncapi`),
    /// else the ORIGIN `source`, else empty (an unresolved topic side).
    pub format: String,
    pub file: Option<String>,
    /// 1-based, from the one [`Locator`] this call builds.
    pub line: Option<i64>,
}

/// One declared difference between the two sides.
#[derive(serde::Serialize, Debug, Clone)]
#[non_exhaustive]
pub struct FieldChange {
    /// `fields` (proto / Avro), `request`, `response:<code>` or `payload[:<name>]`.
    pub section: String,
    /// The flattened field name (`a.b`, `lines[].sku`); the producer's name
    /// when both sides declare it.
    pub field: String,
    /// `type`, `number`, `name`, `label`, `reserved`, `producer_only`,
    /// `consumer_only` or `unknown`.
    pub change: &'static str,
    /// The producer's type (a proto field number for `number`, the name for
    /// `name`, `reserved` for the side that reserves it).
    pub producer: Option<String>,
    pub consumer: Option<String>,
    /// The format rule that judged it (`proto_wire_type`, `avro_promotion`, ...).
    pub rule: &'static str,
    pub breaking: bool,
}

/// One producer → consumer pairing and its verdict.
#[derive(serde::Serialize, Debug, Clone)]
#[non_exhaustive]
pub struct FieldDiffRow {
    /// `schema_copy` | `topic` | `channel` | `route`.
    pub pairing: &'static str,
    /// The message's qualified name, the topic, the channel, or `METHOD path`.
    pub key: String,
    pub producer: FieldSide,
    pub consumer: FieldSide,
    /// `breaking` if any change breaks, else `unknown` if something could not
    /// be compared, else `compatible` if anything differs, else `identical`.
    pub status: &'static str,
    /// Always `derived`: the rules are facts about the declared schemas, the
    /// pairing of two declarations is an inference.
    pub tier: &'static str,
    /// Why a row is `unknown` without a change saying so (`no_fields`,
    /// `producer_no_fields`, `format_mismatch`, `truncated`,
    /// `consumer_message_type_unresolved`, ...); `truncated` also rides on a
    /// verdict row whose cell a cap cut short.
    pub note: Option<&'static str>,
    pub changes: Vec<FieldChange>,
}

/// Every field-level contract row in `merged`. See the module docs.
pub fn contract_fields(merged: &MergedGraph) -> Vec<FieldDiffRow> {
    let holders = collect_holders(merged);
    let loc = Locator::new(merged);
    let mut sides: HashMap<u64, FieldSide> = HashMap::new();
    let mut side = |id: u64| -> FieldSide {
        sides
            .entry(id)
            .or_insert_with(|| {
                let at = loc.locate(NodeId(id));
                let h = holders.get(&id);
                FieldSide {
                    repo_id: h.map_or(0, |h| h.repo),
                    qname: at.qname,
                    format: h.map(Holder::format).unwrap_or_default(),
                    file: at.file,
                    line: at.line,
                }
            })
            .clone()
    };

    let mut rows: Vec<FieldDiffRow> = Vec::new();
    for (from, to) in schema_copies(merged, &holders) {
        let key = qualified_name(&holders[&from].qname).to_string();
        let (p, c) = (&holders[&from], &holders[&to]);
        let (ps, cs) = (side(from), side(to));
        match (p.schema(), c.schema()) {
            (Ok(a), Ok(b)) if a.format == "avro" && b.format == "avro" => {
                let (fwd, back) = (avro_rules(&a, &b), avro_rules(&b, &a));
                let split = signature(&fwd) != signature(&back);
                rows.push(row(
                    "schema_copy",
                    key.clone(),
                    ps.clone(),
                    cs.clone(),
                    Ok(fwd),
                ));
                if split {
                    rows.push(row("schema_copy", key, cs, ps, Ok(back)));
                }
            }
            (a, b) => rows.push(row("schema_copy", key, ps, cs, judge_message(a, b))),
        }
    }

    let mut seen: BTreeSet<(String, u64, String, u64, String)> = BTreeSet::new();
    let by_name = message_types_by_name(&holders);
    for r in message_contracts(merged) {
        let (Some(pq), Some(cq)) = (&r.producer, &r.consumer) else {
            continue;
        };
        let (pr, cr) = (resolve_type(&by_name, pq), resolve_type(&by_name, cq));
        if matches!((pr, cr), (Resolved::Missing, Resolved::Missing)) {
            // No schema on either side: `message_contracts` already says so.
            continue;
        }
        if let (Resolved::Node(a), Resolved::Node(b)) = (pr, cr)
            && a == b
        {
            continue;
        }
        let queue_side = |q: &MessageContractSide| FieldSide {
            repo_id: q.repo_id,
            qname: q.qname.clone(),
            format: String::new(),
            file: q.file.clone(),
            line: q.line,
        };
        let ps = if let Resolved::Node(id) = pr {
            side(id)
        } else {
            queue_side(pq)
        };
        let cs = if let Resolved::Node(id) = cr {
            side(id)
        } else {
            queue_side(cq)
        };
        if !seen.insert((
            r.topic.clone(),
            ps.repo_id,
            ps.qname.clone(),
            cs.repo_id,
            cs.qname.clone(),
        )) {
            continue;
        }
        let verdict = match (pr, cr) {
            (Resolved::Node(a), Resolved::Node(b)) => {
                judge_message(holders[&a].schema(), holders[&b].schema())
            }
            (Resolved::Ambiguous, _) => Err("producer_message_type_ambiguous"),
            (Resolved::Missing, _) => Err("producer_message_type_unresolved"),
            (_, Resolved::Ambiguous) => Err("consumer_message_type_ambiguous"),
            (_, Resolved::Missing) => Err("consumer_message_type_unresolved"),
        };
        rows.push(row("topic", r.topic.clone(), ps, cs, verdict));
    }

    for (key, p, c) in channel_pairs(&holders) {
        let (ps, cs) = (side(p), side(c));
        if ps.repo_id == cs.repo_id && ps.file == cs.file {
            continue;
        }
        let verdict = judge(
            &holders[&p],
            &holders[&c],
            "asyncapi",
            "asyncapi",
            |a, b| json_sections(a, b, &ASYNCAPI),
        );
        rows.push(row("channel", key, ps, cs, verdict));
    }

    for (key, p, c) in route_pairs(&holders) {
        let (ps, cs) = (side(p), side(c));
        let verdict = judge(&holders[&p], &holders[&c], "openapi", "pact", route_rules);
        rows.push(row("route", key, ps, cs, verdict));
    }

    rows.sort_by(|a, b| sort_key(a).cmp(&sort_key(b)));
    if !rows.is_empty() {
        let n = |f: &dyn Fn(&FieldDiffRow) -> bool| rows.iter().filter(|r| f(r)).count();
        eprintln!(
            "[contract-fields] pairs={} schema_copy={} topic={} channel={} route={} identical={} compatible={} breaking={} unknown={}",
            rows.len(),
            n(&|r| r.pairing == "schema_copy"),
            n(&|r| r.pairing == "topic"),
            n(&|r| r.pairing == "channel"),
            n(&|r| r.pairing == "route"),
            n(&|r| r.status == "identical"),
            n(&|r| r.status == "compatible"),
            n(&|r| r.status == "breaking"),
            n(&|r| r.status == "unknown"),
        );
    }
    rows
}

type SortKey<'a> = (
    &'a str,
    &'a str,
    Option<&'a str>,
    Option<&'a str>,
    &'a str,
    &'a str,
    u64,
    u64,
);

fn sort_key(r: &FieldDiffRow) -> SortKey<'_> {
    (
        r.pairing,
        r.key.as_str(),
        r.producer.file.as_deref(),
        r.consumer.file.as_deref(),
        r.producer.qname.as_str(),
        r.consumer.qname.as_str(),
        r.producer.repo_id,
        r.consumer.repo_id,
    )
}

fn row(
    pairing: &'static str,
    key: String,
    producer: FieldSide,
    consumer: FieldSide,
    diff: Result<Diff, &'static str>,
) -> FieldDiffRow {
    let (status, note, changes) = match diff {
        Err(note) => ("unknown", Some(note), Vec::new()),
        Ok(d) => {
            let status = if d.changes.iter().any(|c| c.breaking) {
                "breaking"
            } else if d.suppressed || d.changes.iter().any(|c| c.change == UNKNOWN) {
                "unknown"
            } else if d.changes.is_empty() {
                "identical"
            } else {
                "compatible"
            };
            (status, d.suppressed.then_some("truncated"), d.changes)
        }
    };
    FieldDiffRow {
        pairing,
        key,
        producer,
        consumer,
        status,
        tier: "derived",
        note,
        changes,
    }
}

// ---- the nodes and their cells -------------------------------------------

/// A MESSAGE_TYPE or contract-op DOC_SECTION, folded across every graph that
/// carries a copy of it.
struct Holder {
    repo: u64,
    message: bool,
    qname: String,
    /// Distinct SCHEMA_FIELDS payloads, first-seen order.
    payloads: Vec<String>,
    /// The parsed ORIGIN, when it says `provenance: contract`.
    origin: Option<Value>,
}

/// Why a side's fields cannot be compared.
#[derive(Clone, Copy)]
enum Gap {
    NoFields,
    Conflicting,
    Unparsable,
}

impl Holder {
    fn origin_str(&self, key: &str) -> Option<&str> {
        self.origin.as_ref()?.get(key)?.as_str()
    }

    fn format(&self) -> String {
        match self.schema() {
            Ok(s) => s.format,
            Err(_) => self.origin_str("source").unwrap_or_default().to_string(),
        }
    }

    /// The one SCHEMA_FIELDS payload, parsed. Two different payloads on one
    /// node (one repo declaring the message twice) are not a guess to pick from.
    fn schema(&self) -> Result<Schema, Gap> {
        let payload = match self.payloads.as_slice() {
            [] => return Err(Gap::NoFields),
            [one] => one,
            _ => return Err(Gap::Conflicting),
        };
        let v: Value = serde_json::from_str(payload).map_err(|_| Gap::Unparsable)?;
        let obj = v.as_object().ok_or(Gap::Unparsable)?;
        let format = obj
            .get("format")
            .and_then(Value::as_str)
            .ok_or(Gap::Unparsable)?;
        let mut sections: BTreeMap<String, Vec<Field>> = BTreeMap::new();
        for (k, val) in obj {
            if k == "reserved" {
                continue;
            }
            if let Some(items) = val.as_array() {
                sections.insert(k.clone(), items.iter().filter_map(Field::parse).collect());
            }
        }
        let reserved = obj
            .get("reserved")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        Ok(Schema {
            format: format.to_string(),
            sections,
            reserved,
            truncated: obj.get("truncated").and_then(Value::as_bool) == Some(true),
        })
    }
}

fn payload_str(c: &Cell) -> Option<&str> {
    match &c.payload {
        CellPayload::Json(s) | CellPayload::Text(s) => Some(s),
        _ => None,
    }
}

fn collect_holders(merged: &MergedGraph) -> BTreeMap<u64, Holder> {
    let mut out: BTreeMap<u64, Holder> = BTreeMap::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            let message = match g.nav.kind_by_id.get(&n.id).copied() {
                Some(k) if k == node_kind::MESSAGE_TYPE => true,
                Some(k) if k == node_kind::DOC_SECTION => false,
                _ => continue,
            };
            let origin = n
                .cells
                .iter()
                .filter(|c| c.kind == cell_type::ORIGIN)
                .find_map(payload_str)
                .filter(|j| j.contains("\"contract\""))
                .and_then(|j| serde_json::from_str::<Value>(j).ok())
                .filter(|v| v.get("provenance").and_then(Value::as_str) == Some("contract"));
            // A markdown DOC_SECTION is not a contract op.
            if !message && origin.is_none() {
                continue;
            }
            let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
                continue;
            };
            let h = out.entry(n.id.0).or_insert_with(|| Holder {
                repo: n.repo.0,
                message,
                qname: qname.clone(),
                payloads: Vec::new(),
                origin: None,
            });
            if h.origin.is_none() {
                h.origin = origin;
            }
            for p in n
                .cells
                .iter()
                .filter(|c| c.kind == cell_type::SCHEMA_FIELDS)
                .filter_map(payload_str)
            {
                if !h.payloads.iter().any(|x| x == p) {
                    h.payloads.push(p.to_string());
                }
            }
        }
    }
    out
}

/// `message:proto:shop.v1.OrderCreated` → `shop.v1.OrderCreated`.
fn qualified_name(qname: &str) -> &str {
    qname
        .strip_prefix("message:")
        .and_then(|rest| rest.split_once(':'))
        .map_or(qname, |(_, q)| q)
}

// ---- pairings --------------------------------------------------------------

/// SHARES_SCHEMA edges between two MESSAGE_TYPE holders, one per unordered
/// pair (a node folded from two graphs can repeat its edge).
fn schema_copies(merged: &MergedGraph, holders: &BTreeMap<u64, Holder>) -> Vec<(u64, u64)> {
    let is_message = |id: u64| holders.get(&id).is_some_and(|h| h.message);
    let edges: BTreeSet<(u64, u64)> = merged
        .all_edges()
        .filter(|e| e.category == edge_category::SHARES_SCHEMA)
        .map(|e| (e.from.0, e.to.0))
        .filter(|(f, t)| f != t && is_message(*f) && is_message(*t))
        .collect();
    let mut taken: HashSet<(u64, u64)> = HashSet::new();
    edges
        .into_iter()
        .filter(|&(f, t)| taken.insert((f.min(t), f.max(t))))
        .collect()
}

#[derive(Clone, Copy)]
enum Resolved {
    Node(u64),
    Missing,
    Ambiguous,
}

/// `(repo, last segment)` → the MESSAGE_TYPE holders so named, ascending id.
fn message_types_by_name(holders: &BTreeMap<u64, Holder>) -> HashMap<(u64, &str), Vec<u64>> {
    let mut out: HashMap<(u64, &str), Vec<u64>> = HashMap::new();
    for (id, h) in holders.iter().filter(|(_, h)| h.message) {
        let last = last_segment(qualified_name(&h.qname));
        out.entry((h.repo, last)).or_default().push(*id);
    }
    out
}

fn last_segment(s: &str) -> &str {
    s.rsplit(['.', ':']).next().unwrap_or(s)
}

/// A queue side's `message_type` as a MESSAGE_TYPE in its own repo: unique by
/// last segment, else ambiguous; a type that is not a bare dotted name
/// (`List<X>`, a primitive) resolves to nothing.
fn resolve_type(by_name: &HashMap<(u64, &str), Vec<u64>>, q: &MessageContractSide) -> Resolved {
    let Some(ty) = q.message_type.as_deref() else {
        return Resolved::Missing;
    };
    if ty.is_empty()
        || !ty
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | ':'))
    {
        return Resolved::Missing;
    }
    match by_name
        .get(&(q.repo_id, last_segment(ty)))
        .map(Vec::as_slice)
    {
        Some([one]) => Resolved::Node(*one),
        Some([_, _, ..]) => Resolved::Ambiguous,
        _ => Resolved::Missing,
    }
}

/// `(channel, publish op, subscribe op)` for every AsyncAPI channel declared
/// with both actions. The caller drops a pair declared in one file.
fn channel_pairs(holders: &BTreeMap<u64, Holder>) -> Vec<(String, u64, u64)> {
    let mut pubs: BTreeMap<&str, Vec<u64>> = BTreeMap::new();
    let mut subs: BTreeMap<&str, Vec<u64>> = BTreeMap::new();
    for (id, h) in holders.iter().filter(|(_, h)| !h.message) {
        if h.origin_str("method").is_some() {
            continue;
        }
        let Some(channel) = h.origin_str("channel").map(|c| c.trim_matches('/')) else {
            continue;
        };
        if channel.is_empty() {
            continue;
        }
        match h.origin_str("action") {
            Some("publish") => pubs.entry(channel).or_default().push(*id),
            Some("subscribe") => subs.entry(channel).or_default().push(*id),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for (channel, ps) in &pubs {
        for c in subs.get(channel).map_or(&[][..], Vec::as_slice) {
            for p in ps {
                out.push((channel.to_string(), *p, *c));
            }
        }
    }
    out
}

/// One HTTP contract op read off its ORIGIN.
struct HttpOp {
    id: u64,
    method: String,
    /// The path as declared (server base joined, for OpenAPI).
    path: String,
    /// Normalised pairing keys: the path, and OpenAPI's as-written `raw_path`.
    keys: Vec<String>,
}

/// `(METHOD path, OpenAPI op, Pact op)`: each Pact interaction against the
/// most specific OpenAPI op(s) declaring its method and path. The provider's
/// server-joined `path` and its as-written `raw_path` both count.
fn route_pairs(holders: &BTreeMap<u64, Holder>) -> Vec<(String, u64, u64)> {
    let mut providers: Vec<HttpOp> = Vec::new();
    let mut pacts: Vec<HttpOp> = Vec::new();
    for (id, h) in holders.iter().filter(|(_, h)| !h.message) {
        let (Some(method), Some(path)) = (h.origin_str("method"), h.origin_str("path")) else {
            continue;
        };
        let mut op = HttpOp {
            id: *id,
            method: method.to_ascii_uppercase(),
            path: path.to_string(),
            keys: vec![normalise_http_path(path)],
        };
        match h.origin_str("source") {
            Some("openapi") => {
                if let Some(raw) = h.origin_str("raw_path").map(normalise_http_path)
                    && !op.keys.contains(&raw)
                {
                    op.keys.push(raw);
                }
                providers.push(op);
            }
            Some("pact") => pacts.push(op),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for pact in &pacts {
        let Some(path) = pact.keys.first() else {
            continue;
        };
        let fits: Vec<(usize, &HttpOp)> = providers
            .iter()
            .filter(|p| p.method == pact.method)
            .filter_map(|p| {
                p.keys
                    .iter()
                    .filter_map(|k| template_fit(k, path))
                    .min()
                    .map(|n| (n, p))
            })
            .collect();
        let Some(best) = fits.iter().map(|(n, _)| *n).min() else {
            continue;
        };
        for (_, p) in fits.iter().filter(|(n, _)| *n == best) {
            out.push((format!("{} {}", p.method, p.path), p.id, pact.id));
        }
    }
    out
}

/// How many `{}` placeholders of the provider's normalised path it took to
/// match the Pact's concrete one; `None` when they do not match.
fn template_fit(provider: &str, pact: &str) -> Option<usize> {
    let (a, b): (Vec<&str>, Vec<&str>) = (provider.split('/').collect(), pact.split('/').collect());
    if a.len() != b.len() {
        return None;
    }
    let mut holes = 0;
    for (x, y) in a.iter().zip(&b) {
        if *x == "{}" && *y != "{}" {
            holes += 1;
        } else if x != y {
            return None;
        }
    }
    Some(holes)
}

// ---- verdict plumbing ------------------------------------------------------

struct Field {
    name: String,
    ty: Option<String>,
    number: Option<u64>,
    repeated: bool,
    required: bool,
    default: bool,
}

impl Field {
    fn parse(v: &Value) -> Option<Field> {
        let s = |k: &str| v.get(k).and_then(Value::as_str);
        Some(Field {
            name: s("name")?.to_string(),
            ty: s("type").map(str::to_string),
            number: v.get("number").and_then(Value::as_u64),
            repeated: s("label") == Some("repeated"),
            required: v.get("required").and_then(Value::as_bool) == Some(true),
            default: v.get("default").and_then(Value::as_bool) == Some(true),
        })
    }
}

struct Schema {
    format: String,
    sections: BTreeMap<String, Vec<Field>>,
    reserved: Vec<String>,
    truncated: bool,
}

impl Schema {
    fn fields(&self, section: &str) -> &[Field] {
        self.sections.get(section).map_or(&[][..], Vec::as_slice)
    }
}

#[derive(Default)]
struct Diff {
    changes: Vec<FieldChange>,
    /// A comparison was skipped because the other side's cell is truncated.
    suppressed: bool,
}

const UNKNOWN: &str = "unknown";

impl Diff {
    fn push(
        &mut self,
        (section, field): (&str, &str),
        change: &'static str,
        (producer, consumer): (Option<&str>, Option<&str>),
        rule: &'static str,
        breaking: bool,
    ) {
        self.changes.push(FieldChange {
            section: section.to_string(),
            field: field.to_string(),
            change,
            producer: producer.map(str::to_string),
            consumer: consumer.map(str::to_string),
            rule,
            breaking,
        });
    }
}

/// The part of a verdict that says whether two directions agree.
fn signature(d: &Diff) -> Vec<(&str, &str, &'static str, bool)> {
    let mut s: Vec<_> = d
        .changes
        .iter()
        .map(|c| (c.section.as_str(), c.field.as_str(), c.rule, c.breaking))
        .collect();
    s.sort();
    s.push(("", "", if d.suppressed { "truncated" } else { "" }, false));
    s
}

fn gap_note(p: Option<Gap>, c: Option<Gap>) -> &'static str {
    match (p, c) {
        (Some(Gap::NoFields), Some(Gap::NoFields)) => "no_fields",
        (Some(Gap::NoFields), _) => "producer_no_fields",
        (_, Some(Gap::NoFields)) => "consumer_no_fields",
        (Some(Gap::Conflicting), _) | (_, Some(Gap::Conflicting)) => "conflicting_fields",
        _ => "unparsable_fields",
    }
}

/// Two message schemas, one direction (producer = writer): proto or Avro.
fn judge_message(p: Result<Schema, Gap>, c: Result<Schema, Gap>) -> Result<Diff, &'static str> {
    let (p, c) = match (p, c) {
        (Ok(p), Ok(c)) => (p, c),
        (p, c) => return Err(gap_note(p.err(), c.err())),
    };
    match (p.format.as_str(), c.format.as_str()) {
        ("proto", "proto") => Ok(proto_rules(&p, &c)),
        ("avro", "avro") => Ok(avro_rules(&p, &c)),
        (a, b) if a != b => Err("format_mismatch"),
        _ => Err("unsupported_format"),
    }
}

/// Two contract ops whose formats must be `pf` / `cf`.
fn judge(
    p: &Holder,
    c: &Holder,
    pf: &str,
    cf: &str,
    rules: impl Fn(&Schema, &Schema) -> Diff,
) -> Result<Diff, &'static str> {
    match (p.schema(), c.schema()) {
        (Ok(p), Ok(c)) if p.format == pf && c.format == cf => Ok(rules(&p, &c)),
        (Ok(_), Ok(_)) => Err("format_mismatch"),
        (p, c) => Err(gap_note(p.err(), c.err())),
    }
}

// ---- proto -----------------------------------------------------------------

/// Proto's wire rules. Fields are keyed by number on the wire, so a matched
/// number with a different type breaks and a different name does not; one name
/// under two numbers breaks; a number one side reserves and the other uses
/// breaks; a field only one side knows is skipped as unknown by the other.
/// Symmetric, so one direction speaks for the pair.
fn proto_rules(p: &Schema, c: &Schema) -> Diff {
    const S: &str = "fields";
    let mut d = Diff::default();
    let (pf, cf) = (p.fields(S), c.fields(S));
    let ((p_num, p_name), (c_num, c_name)) = (proto_index(pf), proto_index(cf));

    for f in pf {
        let ty = f.ty.as_deref();
        let at_number = f.number.and_then(|n| c_num.get(&n));
        if let Some(g) = at_number {
            proto_same_slot(&mut d, f, g);
        }
        let by_name = c_name.get(f.name.as_str());
        if let Some(g) = by_name {
            match (f.number, g.number) {
                (Some(a), Some(b)) if a != b => d.push(
                    (S, &f.name),
                    "number",
                    (Some(&a.to_string()), Some(&b.to_string())),
                    "proto_number_changed",
                    true,
                ),
                // A literal that did not parse: the name is all there is.
                (None, _) | (_, None) if at_number.is_none() => proto_same_slot(&mut d, f, g),
                _ => {}
            }
        }
        if at_number.is_some() || by_name.is_some() {
            continue;
        }
        if f.number.is_some_and(|n| reserves(&c.reserved, n)) {
            d.push(
                (S, &f.name),
                "reserved",
                (ty, Some("reserved")),
                "proto_reserved_reused",
                true,
            );
        } else if c.truncated {
            d.suppressed = true;
        } else {
            d.push(
                (S, &f.name),
                "producer_only",
                (ty, None),
                "proto_unknown_field",
                false,
            );
        }
    }
    for g in cf {
        let known = g.number.is_some_and(|n| p_num.contains_key(&n))
            || p_name.contains_key(g.name.as_str());
        if known {
            continue;
        }
        let ty = g.ty.as_deref();
        if g.number.is_some_and(|n| reserves(&p.reserved, n)) {
            d.push(
                (S, &g.name),
                "reserved",
                (Some("reserved"), ty),
                "proto_reserved_reused",
                true,
            );
        } else if p.truncated {
            d.suppressed = true;
        } else {
            d.push(
                (S, &g.name),
                "consumer_only",
                (None, ty),
                "proto_unknown_field",
                false,
            );
        }
    }
    d
}

/// A proto field list by number and by name, first declaration winning.
type ProtoIndex<'a> = (HashMap<u64, &'a Field>, HashMap<&'a str, &'a Field>);

fn proto_index(fs: &[Field]) -> ProtoIndex<'_> {
    let mut by_num: HashMap<u64, &Field> = HashMap::new();
    let mut by_name: HashMap<&str, &Field> = HashMap::new();
    for f in fs {
        if let Some(n) = f.number {
            by_num.entry(n).or_insert(f);
        }
        by_name.entry(f.name.as_str()).or_insert(f);
    }
    (by_num, by_name)
}

/// One wire slot declared by both sides: type, cardinality, name.
fn proto_same_slot(d: &mut Diff, f: &Field, g: &Field) {
    const S: &str = "fields";
    let (a, b) = (f.ty.as_deref().unwrap_or(""), g.ty.as_deref().unwrap_or(""));
    if !proto_same_type(a, b) {
        d.push(
            (S, &f.name),
            "type",
            (Some(a), Some(b)),
            "proto_wire_type",
            true,
        );
    }
    if f.repeated != g.repeated {
        let label = |r: bool| if r { "repeated" } else { "singular" };
        d.push(
            (S, &f.name),
            "label",
            (Some(label(f.repeated)), Some(label(g.repeated))),
            "proto_cardinality_changed",
            true,
        );
    }
    if f.name != g.name {
        d.push(
            (S, &f.name),
            "name",
            (Some(&f.name), Some(&g.name)),
            "proto_renamed",
            false,
        );
    }
}

/// Proto resolves a type name relative to the package, so `Money` and
/// `shop.v1.Money` can name one message.
fn proto_same_type(a: &str, b: &str) -> bool {
    let suffix = |long: &str, short: &str| {
        long.len() > short.len()
            && long.ends_with(short)
            && long[..long.len() - short.len()].ends_with('.')
    };
    a == b || suffix(a, b) || suffix(b, a)
}

/// `reserved` entries are decimal numbers, `a to b` / `a to max` ranges, or
/// names; only numbers and ranges reserve a number.
fn reserves(reserved: &[String], n: u64) -> bool {
    reserved.iter().any(|r| match r.split_once(" to ") {
        Some((lo, hi)) => {
            let lo = lo.trim().parse::<u64>().ok();
            let hi = match hi.trim() {
                "max" => Some(u64::MAX),
                h => h.parse::<u64>().ok(),
            };
            matches!((lo, hi), (Some(lo), Some(hi)) if lo <= n && n <= hi)
        }
        None => r.trim().parse::<u64>().ok() == Some(n),
    })
}

// ---- avro ------------------------------------------------------------------

/// Avro schema resolution, `w` = writer (producer), `r` = reader (consumer).
/// Fields match by name. A reader field the writer lacks needs a default; a
/// writer field the reader lacks is ignored; a type change resolves only by
/// the spec's promotions and union / named-type rules.
fn avro_rules(w: &Schema, r: &Schema) -> Diff {
    const S: &str = "fields";
    let mut d = Diff::default();
    let (wf, rf) = (w.fields(S), r.fields(S));
    let r_name: HashMap<&str, &Field> = rf.iter().map(|f| (f.name.as_str(), f)).collect();
    let w_name: HashSet<&str> = wf.iter().map(|f| f.name.as_str()).collect();
    for f in wf {
        let Some(g) = r_name.get(f.name.as_str()) else {
            if r.truncated {
                d.suppressed = true;
            } else {
                d.push(
                    (S, &f.name),
                    "producer_only",
                    (f.ty.as_deref(), None),
                    "avro_writer_field_ignored",
                    false,
                );
            }
            continue;
        };
        let (a, b) = (f.ty.as_deref(), g.ty.as_deref());
        let (Some(wt), Some(rt)) = (a, b) else {
            // A position that held no schema: nothing to resolve.
            d.push((S, &f.name), UNKNOWN, (a, b), "avro_no_schema", false);
            continue;
        };
        if wt == rt {
            continue;
        }
        let (rule, breaking) = match avro_resolve(&AvroTy::parse(wt), &AvroTy::parse(rt)) {
            Res::Same | Res::Resolves => ("avro_type_resolves", false),
            Res::Promotion => ("avro_promotion", false),
            Res::Fails => ("avro_type_changed", true),
        };
        d.push((S, &f.name), "type", (a, b), rule, breaking);
    }
    for g in rf.iter().filter(|g| !w_name.contains(g.name.as_str())) {
        let ty = g.ty.as_deref();
        if w.truncated {
            d.suppressed = true;
        } else if g.default {
            d.push(
                (S, &g.name),
                "consumer_only",
                (None, ty),
                "avro_reader_field_default",
                false,
            );
        } else {
            d.push(
                (S, &g.name),
                "consumer_only",
                (None, ty),
                "avro_reader_field_no_default",
                true,
            );
        }
    }
    d
}

/// A rendered Avro type (LE.10a): `a|b` unions, `array<T>`, `map<T>`, a name
/// with an optional `(logicalType)`.
#[derive(PartialEq)]
enum AvroTy {
    Union(Vec<AvroTy>),
    Array(Box<AvroTy>),
    Map(Box<AvroTy>),
    Named(String, Option<String>),
}

/// How a writer type reaches a reader type, weakest first.
#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
enum Res {
    Same,
    /// By union branch selection, an unqualified name match or a logical-type
    /// annotation: the reader reads every value the writer writes.
    Resolves,
    Promotion,
    Fails,
}

impl AvroTy {
    fn parse(s: &str) -> AvroTy {
        let parts = split_top(s, '|');
        if parts.len() > 1 {
            return AvroTy::Union(parts.into_iter().map(AvroTy::parse).collect());
        }
        let inner = |p: &str| s.strip_prefix(p).and_then(|r| r.strip_suffix('>'));
        if let Some(t) = inner("array<") {
            return AvroTy::Array(Box::new(AvroTy::parse(t)));
        }
        if let Some(t) = inner("map<") {
            return AvroTy::Map(Box::new(AvroTy::parse(t)));
        }
        match s.strip_suffix(')').and_then(|r| r.split_once('(')) {
            Some((base, logical)) => AvroTy::Named(base.to_string(), Some(logical.to_string())),
            None => AvroTy::Named(s.to_string(), None),
        }
    }
}

/// `s` split on `sep` outside `<>` / `()`.
fn split_top(s: &str, sep: char) -> Vec<&str> {
    let (mut out, mut depth, mut start) = (Vec::new(), 0usize, 0usize);
    for (i, ch) in s.char_indices() {
        match ch {
            '<' | '(' => depth += 1,
            '>' | ')' => depth = depth.saturating_sub(1),
            c if c == sep && depth == 0 => {
                out.push(&s[start..i]);
                start = i + ch.len_utf8();
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

const AVRO_PRIMITIVES: &[&str] = &[
    "null", "boolean", "int", "long", "float", "double", "bytes", "string",
];

fn avro_promotes(w: &str, r: &str) -> bool {
    matches!(
        (w, r),
        ("int", "long" | "float" | "double")
            | ("long", "float" | "double")
            | ("float", "double")
            | ("string", "bytes")
            | ("bytes", "string")
    )
}

fn avro_resolve(w: &AvroTy, r: &AvroTy) -> Res {
    if w == r {
        return Res::Same;
    }
    match (w, r) {
        // Every branch the writer may write must resolve.
        (AvroTy::Union(ws), _) => ws
            .iter()
            .map(|b| avro_resolve(b, r))
            .max()
            .unwrap_or(Res::Fails)
            .max(Res::Resolves),
        // The reader picks the first branch the writer's type resolves to.
        (_, AvroTy::Union(rs)) => match rs.iter().map(|b| avro_resolve(w, b)).min() {
            Some(Res::Fails) | None => Res::Fails,
            Some(best) => best.max(Res::Resolves),
        },
        (AvroTy::Array(a), AvroTy::Array(b)) | (AvroTy::Map(a), AvroTy::Map(b)) => {
            avro_resolve(a, b)
        }
        (AvroTy::Named(a, la), AvroTy::Named(b, lb)) => {
            if a == b {
                if la == lb { Res::Same } else { Res::Resolves }
            } else if avro_promotes(a, b) {
                Res::Promotion
            } else if !AVRO_PRIMITIVES.contains(&a.as_str())
                && !AVRO_PRIMITIVES.contains(&b.as_str())
                && last_segment(a) == last_segment(b)
            {
                // Named types resolve on their unqualified name.
                Res::Resolves
            } else {
                Res::Fails
            }
        }
        _ => Res::Fails,
    }
}

// ---- JSON-schema-shaped contracts (AsyncAPI, OpenAPI vs Pact) --------------

/// What a section comparison reports.
struct JsonRules {
    /// A consumer field the producer does not declare: rule and whether it breaks.
    consumer_only: (&'static str, bool),
    /// A producer field the consumer lacks, when reported at all.
    producer_only: Option<(&'static str, bool)>,
    /// A producer-REQUIRED field the consumer lacks.
    required_missing: Option<&'static str>,
    /// Compare the types of fields both declare.
    types: bool,
}

const ASYNCAPI: JsonRules = JsonRules {
    consumer_only: ("consumer_field_not_produced", true),
    producer_only: Some(("producer_extra_field", false)),
    required_missing: None,
    types: true,
};

const ROUTE_REQUEST: JsonRules = JsonRules {
    consumer_only: ("undeclared_request_field", false),
    producer_only: None,
    required_missing: Some("missing_required_field"),
    types: false,
};

const ROUTE_RESPONSE: JsonRules = JsonRules {
    consumer_only: ("consumer_expects_undeclared_field", true),
    producer_only: None,
    required_missing: None,
    types: false,
};

/// Every consumer section against the producer's same-named one. A section
/// the producer does not declare cannot be judged.
fn json_sections(p: &Schema, c: &Schema, rules: &JsonRules) -> Diff {
    let mut d = Diff::default();
    for (sec, cf) in &c.sections {
        match p.sections.get(sec) {
            Some(pf) => json_section(&mut d, sec, (pf, p.truncated), (cf, c.truncated), rules),
            None => d.push(
                (sec, ""),
                UNKNOWN,
                (None, None),
                "section_undeclared",
                false,
            ),
        }
    }
    d
}

/// OpenAPI provider (`p`) vs Pact consumer (`c`): the Pact's request body
/// against the provider's request schema, and each Pact response body against
/// the provider's schema for that status (else its `NXX` class, else
/// `default`).
fn route_rules(p: &Schema, c: &Schema) -> Diff {
    let mut d = Diff::default();
    let empty: Vec<Field> = Vec::new();
    match (p.sections.get("request"), c.sections.get("request")) {
        // A Pact cell without a request section sends no body.
        (Some(pf), cf) => json_section(
            &mut d,
            "request",
            (pf, p.truncated),
            (cf.unwrap_or(&empty), c.truncated),
            &ROUTE_REQUEST,
        ),
        (None, Some(_)) => d.push(
            ("request", ""),
            UNKNOWN,
            (None, None),
            "section_undeclared",
            false,
        ),
        (None, None) => {}
    }
    for (sec, cf) in c.sections.iter().filter(|(s, _)| s.starts_with("response")) {
        let status = sec.strip_prefix("response:").unwrap_or("");
        let class = status
            .chars()
            .next()
            .filter(|_| status.len() == 3)
            .map(|ch| [format!("response:{ch}XX"), format!("response:{ch}xx")]);
        let found = p.sections.get(sec.as_str()).or_else(|| {
            class
                .iter()
                .flatten()
                .find_map(|k| p.sections.get(k))
                .or_else(|| p.sections.get("response:default"))
        });
        match found {
            Some(pf) => json_section(
                &mut d,
                sec,
                (pf, p.truncated),
                (cf, c.truncated),
                &ROUTE_RESPONSE,
            ),
            None => d.push(
                (sec, ""),
                UNKNOWN,
                (None, None),
                "section_undeclared",
                false,
            ),
        }
    }
    d
}

/// A type that is an external or unresolvable `$ref` string (LE.10b).
fn is_ref(ty: Option<&str>) -> bool {
    ty.is_some_and(|t| {
        t.contains('#')
            || t.contains('/')
            || [".yaml", ".yml", ".json"].iter().any(|e| t.ends_with(e))
    })
}

/// The flattened names enclosing `name`: `lines[].sku` → `lines`, `lines[]`.
fn ancestors(name: &str) -> impl Iterator<Item = &str> {
    name.char_indices()
        .filter(|(i, ch)| *i > 0 && matches!(ch, '.' | '['))
        .map(move |(i, _)| &name[..i])
}

/// One section, producer `pf` vs consumer `cf`, each with its `truncated` flag.
fn json_section(
    d: &mut Diff,
    sec: &str,
    (pf, pt): (&[Field], bool),
    (cf, ct): (&[Field], bool),
    rules: &JsonRules,
) {
    let p_by: HashMap<&str, &Field> = pf.iter().map(|f| (f.name.as_str(), f)).collect();
    let c_by: HashMap<&str, &Field> = cf.iter().map(|f| (f.name.as_str(), f)).collect();
    // A whole body that is one unresolvable reference on either side.
    let p_root = p_by.get("$ref").map(|f| f.ty.as_deref());
    let c_root = c_by.get("$ref").map(|f| f.ty.as_deref());
    if (p_root.is_some() || c_root.is_some()) && p_root != c_root {
        d.push(
            (sec, "$ref"),
            UNKNOWN,
            (p_root.flatten(), c_root.flatten()),
            "unresolved_ref",
            false,
        );
        return;
    }
    let p_refs: HashSet<&str> = pf
        .iter()
        .filter(|f| is_ref(f.ty.as_deref()))
        .map(|f| f.name.as_str())
        .collect();
    let c_refs: HashSet<&str> = cf
        .iter()
        .filter(|f| is_ref(f.ty.as_deref()))
        .map(|f| f.name.as_str())
        .collect();
    let mut unknown: BTreeSet<String> = BTreeSet::new();
    // Names reported absent: their children are not reported again.
    let mut absent: HashSet<&str> = HashSet::new();
    let mut ref_unknown = |d: &mut Diff, name: &str| {
        if unknown.insert(name.to_string()) {
            let (a, b) = (
                p_by.get(name).and_then(|f| f.ty.as_deref()),
                c_by.get(name).and_then(|f| f.ty.as_deref()),
            );
            d.push((sec, name), UNKNOWN, (a, b), "unresolved_ref", false);
        }
    };

    for g in cf {
        let name = g.name.as_str();
        if let Some(r) = ancestors(name).find(|a| p_refs.contains(a) || c_refs.contains(a)) {
            if p_by.get(r).and_then(|f| f.ty.as_deref())
                != c_by.get(r).and_then(|f| f.ty.as_deref())
            {
                ref_unknown(d, r);
            }
            continue;
        }
        if let Some(f) = p_by.get(name) {
            let (a, b) = (f.ty.as_deref(), g.ty.as_deref());
            if a == b {
                continue;
            }
            if is_ref(a) || is_ref(b) {
                ref_unknown(d, name);
            } else if rules.types {
                match json_type_change(a.unwrap_or("any"), b.unwrap_or("any")) {
                    Some(TypeChange::Widened) => {
                        d.push((sec, name), "type", (a, b), "type_widened", false)
                    }
                    Some(TypeChange::Changed) => {
                        d.push((sec, name), "type", (a, b), "type_changed", true)
                    }
                    Some(TypeChange::Untyped) => {
                        d.push((sec, name), UNKNOWN, (a, b), "untyped", false)
                    }
                    None => {}
                }
            }
            continue;
        }
        if ancestors(name).any(|a| absent.contains(a)) {
            continue;
        }
        if pt {
            d.suppressed = true;
            continue;
        }
        absent.insert(name);
        let (rule, breaking) = rules.consumer_only;
        d.push(
            (sec, name),
            "consumer_only",
            (None, g.ty.as_deref()),
            rule,
            breaking,
        );
    }

    for f in pf {
        let name = f.name.as_str();
        if c_by.contains_key(name)
            || ancestors(name).any(|a| p_refs.contains(a) || c_refs.contains(a))
        {
            continue;
        }
        if ancestors(name).any(|a| absent.contains(a)) {
            continue;
        }
        let (rule, breaking) = match (rules.required_missing, rules.producer_only) {
            (Some(rule), _) if f.required => {
                // Required within its parent: only missing when the parent is sent.
                let parent = name.rfind('.').map(|i| &name[..i]);
                if parent.is_some_and(|p| !c_by.contains_key(p)) {
                    continue;
                }
                (rule, true)
            }
            (_, Some(r)) => r,
            _ => continue,
        };
        if ct {
            d.suppressed = true;
            continue;
        }
        absent.insert(name);
        d.push(
            (sec, name),
            "producer_only",
            (f.ty.as_deref(), None),
            rule,
            breaking,
        );
    }
}

enum TypeChange {
    /// Every value the producer's type allows, the consumer's type accepts.
    Widened,
    Changed,
    /// The producer declares no type: nothing to compare.
    Untyped,
}

/// JSON-schema types as LE.10b renders them: `a|b` alternatives, each
/// `base(format)`. `integer` is a `number`; a consumer with no format accepts
/// any format. `None` when there is nothing to report.
fn json_type_change(p: &str, c: &str) -> Option<TypeChange> {
    if p == c || c == "any" {
        return None;
    }
    if p == "any" {
        return Some(TypeChange::Untyped);
    }
    let member = |m: &str| -> (String, Option<String>) {
        match m.strip_suffix(')').and_then(|r| r.split_once('(')) {
            Some((base, format)) => (base.to_string(), Some(format.to_string())),
            None => (m.to_string(), None),
        }
    };
    let cs: Vec<(String, Option<String>)> = split_top(c, '|').into_iter().map(member).collect();
    let accepted = split_top(p, '|').into_iter().map(member).all(|(pb, pfmt)| {
        cs.iter().any(|(cb, cfmt)| {
            (pb == *cb || (pb == "integer" && cb == "number")) && (cfmt.is_none() || *cfmt == pfmt)
        })
    });
    Some(if accepted {
        TypeChange::Widened
    } else {
        TypeChange::Changed
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avro_resolution_follows_the_spec() {
        let r = |w: &str, rd: &str| avro_resolve(&AvroTy::parse(w), &AvroTy::parse(rd));
        assert!(r("int", "long") == Res::Promotion);
        assert!(r("long", "int") == Res::Fails);
        assert!(r("string", "null|string") == Res::Resolves);
        assert!(
            r("null|string", "string") == Res::Fails,
            "a written null has no reader branch"
        );
        assert!(r("null|string", "string|null") == Res::Resolves);
        assert!(r("array<int>", "array<double>") == Res::Promotion);
        assert!(r("map<string>", "map<int>") == Res::Fails);
        assert!(r("long(timestamp-millis)", "long") == Res::Resolves);
        assert!(r("com.a.Address", "com.b.Address") == Res::Resolves);
        assert!(r("com.a.Address", "com.a.Street") == Res::Fails);
    }

    #[test]
    fn reserved_numbers_and_ranges() {
        let res: Vec<String> = ["2", "9 to 11", "40 to max", "foo"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(reserves(&res, 2) && reserves(&res, 10) && reserves(&res, 1_000));
        assert!(!reserves(&res, 3) && !reserves(&res, 12));
    }

    #[test]
    fn proto_types_resolve_relative_to_the_package() {
        assert!(proto_same_type("Money", "shop.v1.Money"));
        assert!(!proto_same_type("Money", "shop.v1.XMoney"));
        assert!(!proto_same_type("int32", "int64"));
    }

    #[test]
    fn route_templates_prefer_the_most_specific() {
        assert_eq!(template_fit("/orders/{}", "/orders/42"), Some(1));
        assert_eq!(template_fit("/orders/latest", "/orders/latest"), Some(0));
        assert_eq!(template_fit("/orders/{}", "/orders/42/lines"), None);
        assert_eq!(template_fit("/users/{}", "/orders/42"), None);
    }

    #[test]
    fn json_types_widen_or_change() {
        assert!(matches!(
            json_type_change("integer", "number"),
            Some(TypeChange::Widened)
        ));
        assert!(matches!(
            json_type_change("string", "string|null"),
            Some(TypeChange::Widened)
        ));
        assert!(matches!(
            json_type_change("string|null", "string"),
            Some(TypeChange::Changed)
        ));
        assert!(matches!(
            json_type_change("integer(int64)", "integer"),
            Some(TypeChange::Widened)
        ));
        assert!(matches!(
            json_type_change("integer", "integer(int32)"),
            Some(TypeChange::Changed)
        ));
        assert!(json_type_change("string", "any").is_none());
    }

    #[test]
    fn ancestors_walk_dots_and_brackets() {
        let a: Vec<&str> = ancestors("lines[].sku").collect();
        assert_eq!(a, vec!["lines", "lines[]"]);
        assert_eq!(ancestors("[]").count(), 0);
    }
}
