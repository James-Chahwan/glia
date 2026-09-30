//! Contract breaks against a git rev (CC.8a): "did my change break my
//! clients?". LE.10c's `contract_fields` compares a producer against a
//! consumer at one point in time; this compares every contract against ITSELF
//! across the working tree's change, and lists the clients the change left
//! without a provider. CC.8c builds the rev pair as two multi-repo merges so
//! clients in other repos (`--with`) are reported too.
//!
//! [`contract_breaks_vs_rev`] builds the rev delta once
//! (`delta::graph_delta_vs_rev`, LE.1b); [`contract_breaks`] answers from an
//! already built [`RevDelta`].
//!
//! PAIRING. The holders are LE.10c's: every MESSAGE_TYPE (proto, Avro, JSON
//! Schema) and every OpenAPI / AsyncAPI contract op, read on both sides with
//! their SCHEMA_FIELDS. A Pact interaction is the CONSUMER's contract and a
//! feature spec is no API: both are skipped. The rev is built under the
//! working tree's identity, so an unchanged qname is the SAME NodeId: pairing
//! by id is tier `fact`. A before id the delta moved (LB.6) pairs through the
//! move: tier `derived`, change `moved`. A before-only holder is `removed`
//! (breaking: whoever used it loses it), an after-only one `added`
//! (compatible), both tier `fact`. A pair whose payloads are equal is
//! `identical`: counted, never listed.
//!
//! EVOLUTION RULES (old = before, new = after; every [`FieldChange`] reads
//! `producer` = before, `consumer` = after):
//! - proto: LE.10c's wire rules (symmetric), except that a field retired
//!   under `reserved` is `field_removed_reserved`, and a field number dropped
//!   WITHOUT `reserved` is `field_removed_unreserved` (compatible, and the
//!   row's note): old readers survive it, a later reuse of the number would
//!   not.
//! - Avro: schema resolution with writer = old, reader = new (`backward`, the
//!   default), writer = new, reader = old (`forward`), or both (`full`, where
//!   a change breaking either direction breaks).
//! - OpenAPI `request` (what clients send): a field the new side requires that
//!   the old side lacked or left optional is `new_required_request_field`
//!   (breaking); a type the new side no longer accepts every old value of is
//!   `request_type_changed` (breaking), else `request_type_widened`; a field
//!   only the old side declares is `request_field_removed`, one only the new
//!   side declares optionally `request_field_added` (both compatible).
//! - OpenAPI `response:<code>` (what clients read): a section the new side
//!   lacks is `response_removed`; a field it lacks `response_field_removed`; a
//!   field that stops being required `response_field_now_optional`; a type the
//!   old side does not accept every new value of `response_type_changed` (all
//!   breaking). A new field or section is `response_field_added` /
//!   `response_added`, a narrowed type `response_type_narrowed` (compatible).
//! - AsyncAPI `payload[:<name>]`: an old field the new side lacks is
//!   `payload_field_removed`, any type change `payload_type_changed`
//!   (breaking); a new field is `payload_field_added` (compatible).
//!
//! As in LE.10c, nothing is guessed: a field absent from a TRUNCATED side is
//! never reported removed or added (the row is `unknown`, note `truncated`),
//! an unresolvable `$ref` is an `unknown` change, a format that differs across
//! the pair is `unknown` (`format_mismatch`), and a message type with no
//! SCHEMA_FIELDS on one side (an enum, a fixed) is `unknown`. A contract op
//! with no SCHEMA_FIELDS declares no body field: the extractor read the op, so
//! it compares as an empty schema.
//!
//! ORPHANED CLIENTS. Every HTTP_CALLS / GRPC_CALLS / RPC_CALLS /
//! GRAPHQL_CALLS / WS_CONNECTS edge the delta removed whose client end still
//! exists in the after graph (directly or through a move) with NO edge of that
//! category left: `target_removed` (tier `fact`) when the before target is a
//! removed node, else `pairing_lost` (tier `derived`: the target survives and
//! the resolver no longer pairs them, a path / verb / host change). The client
//! is located in the after graph, the target named from the before graph.
//!
//! ORDER. Schema rows by (status: breaking < unknown < compatible, kind, key),
//! orphans by (category, client, target); `breaking` is the breaking schema
//! rows plus the orphans. An answer with no row carries an [`Absence`]
//! (reason `no_match`, FACT) saying why.
//!
//! Language-level public API (cargo-semver-checks parity) stays out: the
//! graph has no visibility or signature cell.
//!
//! Fired-on marker, one per answer:
//! `[contract-breaks] base=<rev> pairs=<P> breaking=<B> compatible=<C> unknown=<U> removed=<R> added=<A> orphaned_clients=<O>`
//! (`pairs` counts every paired contract, identical ones included; the
//! status counts are schema rows before `breaking_only` filters them).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use glia_code_domain::edge_category;
use glia_core::{EdgeCategoryId, NodeId};
use glia_graph::MergedGraph;

use crate::absence::{self, Absence};
use crate::answers::Locator;
use crate::contract_fields::{
    Diff, Field, FieldChange, FieldSide, Gap, Holder, Schema, TypeChange, UNKNOWN, ancestors,
    avro_rules, collect_holders, is_ref, json_type_change, proto_rules, qualified_name,
};
use crate::delta::{RevDelta, graph_delta_vs_rev};

/// The accepted [`ContractBreakArgs::avro_mode`] values, default first.
pub const AVRO_MODES: &[&str] = &["backward", "forward", "full"];

/// How [`contract_breaks`] judges and lists. Built outside the crate from
/// `Default` plus field assignment.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct ContractBreakArgs {
    /// Avro's direction ([`AVRO_MODES`]): `backward` (the default: the new
    /// schema reads what the old one wrote), `forward` (the old schema reads
    /// what the new one writes) or `full` (both). [`contract_breaks_vs_rev`]
    /// refuses any other value; [`contract_breaks`] judges one as `full`, the
    /// strictest.
    pub avro_mode: &'static str,
    /// List only `breaking` schema rows. [`ContractBreaks::breaking`] and the
    /// orphaned clients are the same either way.
    pub breaking_only: bool,
}

impl Default for ContractBreakArgs {
    fn default() -> Self {
        ContractBreakArgs {
            avro_mode: "backward",
            breaking_only: false,
        }
    }
}

/// One contract whose declaration changed between the rev and the working tree.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct SchemaChange {
    /// `message` (a MESSAGE_TYPE), `operation` (an OpenAPI op) or `channel`
    /// (an AsyncAPI op).
    pub kind: &'static str,
    /// The message's qualified name (`shop.Order`), `METHOD path` for an
    /// operation, `<action> <channel>` for a channel op.
    pub key: String,
    /// The SCHEMA_FIELDS format (`proto`, `avro`, `openapi`, `asyncapi`),
    /// else the ORIGIN source; the after side's when it has one.
    pub format: String,
    /// The holder in the rev's graph (`None` for an added contract).
    pub before: Option<FieldSide>,
    /// The holder in the working tree's graph (`None` for a removed contract).
    pub after: Option<FieldSide>,
    /// `breaking` | `compatible` | `unknown` (`identical` pairs are counted,
    /// never listed).
    pub status: &'static str,
    /// `modified` | `removed` | `added` | `moved`.
    pub change: &'static str,
    /// `fact` (paired by identity, or one-sided) or `derived` (paired through a move).
    pub tier: &'static str,
    /// Why a row is `unknown` without a change saying so (`format_mismatch`,
    /// `before_no_fields`, `truncated`, ...), or `field_removed_unreserved`.
    pub note: Option<&'static str>,
    /// Every declared difference, `producer` = before, `consumer` = after.
    pub changes: Vec<FieldChange>,
}

/// A client the change left without a provider.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct OrphanedClient {
    /// The client node in the working tree's graph.
    pub client_qname: String,
    pub file: Option<String>,
    /// 1-based.
    pub line: Option<i64>,
    /// The edge category it lost (`HTTP_CALLS`, `GRPC_CALLS`, ...).
    pub category: &'static str,
    /// The provider it was paired with at the rev, named from the rev's graph.
    pub target_qname: String,
    /// `target_removed` | `pairing_lost`.
    pub reason: &'static str,
    /// `fact` for `target_removed`, `derived` for `pairing_lost`.
    pub tier: &'static str,
}

/// The contract breaks of the working tree against `base` (module docs).
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct ContractBreaks {
    /// The rev as given.
    pub base: String,
    pub schemas: Vec<SchemaChange>,
    pub orphaned_clients: Vec<OrphanedClient>,
    /// Breaking schema rows plus orphaned clients.
    pub breaking: usize,
    /// `Some` exactly when `schemas` and `orphaned_clients` are both empty.
    pub absence: Option<Absence>,
}

/// The client -> provider edge categories an orphan is looked for in.
const CLIENT_CATEGORIES: [EdgeCategoryId; 5] = [
    edge_category::HTTP_CALLS,
    edge_category::GRPC_CALLS,
    edge_category::RPC_CALLS,
    edge_category::GRAPHQL_CALLS,
    edge_category::WS_CONNECTS,
];

const PRIMITIVE: &str = "contract_breaks";

/// The contract breaks of the working tree at `repo_path` against git rev
/// `base` (module docs). `Err` on an unknown [`ContractBreakArgs::avro_mode`]
/// (checked before anything is built) and on `delta::graph_delta_vs_rev`'s
/// errors: not a directory, not a git work tree, an unknown rev, a failed
/// build. Like the delta, it saves the working tree's parse-cache sidecar,
/// never a layout.
pub fn contract_breaks_vs_rev(
    repo_path: &str,
    base: &str,
    args: &ContractBreakArgs,
) -> Result<ContractBreaks, String> {
    if !AVRO_MODES.contains(&args.avro_mode) {
        return Err(format!(
            "unknown avro mode `{}`: expected one of {}",
            args.avro_mode,
            AVRO_MODES.join(", ")
        ));
    }
    let rev = graph_delta_vs_rev(repo_path, base)?;
    Ok(contract_breaks(&rev, args))
}

/// The contract breaks of an already computed rev delta (module docs).
/// Nothing is built; `base` is `rev.answer.base`.
pub fn contract_breaks(rev: &RevDelta, args: &ContractBreakArgs) -> ContractBreaks {
    let (before, after) = (&rev.before.merged, &rev.after.merged);
    let (old, new) = (contract_holders(before), contract_holders(after));
    let (then, now) = (Locator::new(before), Locator::new(after));
    let moved: HashMap<u64, u64> = rev
        .delta
        .moved_nodes
        .iter()
        .map(|&(b, a)| (b.0, a.0))
        .collect();
    let old_ids: BTreeSet<u64> = old.keys().copied().collect();
    let new_ids: BTreeSet<u64> = new.keys().copied().collect();
    let pairing = pair(&old_ids, &new_ids, &moved);
    let mode = AvroMode::parse(args.avro_mode);

    let side = |loc: &Locator<'_>, id: u64, h: &Holder| {
        let at = loc.locate(NodeId(id));
        FieldSide {
            repo_id: h.repo,
            qname: at.qname,
            format: h.format(),
            file: at.file,
            line: at.line,
        }
    };
    let mut rows: Vec<SchemaChange> = Vec::new();
    for &(b, a, through_move) in &pairing.pairs {
        let (o, n) = (&old[&b], &new[&a]);
        let Some((status, note, changes)) = judge_pair(o, n, mode) else {
            continue;
        };
        rows.push(SchemaChange {
            kind: kind_of(n),
            key: key_of(n),
            format: format_of(Some(n), o),
            before: Some(side(&then, b, o)),
            after: Some(side(&now, a, n)),
            status,
            change: if through_move { "moved" } else { "modified" },
            tier: if through_move { "derived" } else { "fact" },
            note,
            changes,
        });
    }
    for &b in &pairing.removed {
        let o = &old[&b];
        rows.push(SchemaChange {
            kind: kind_of(o),
            key: key_of(o),
            format: format_of(None, o),
            before: Some(side(&then, b, o)),
            after: None,
            status: "breaking",
            change: "removed",
            tier: "fact",
            note: None,
            changes: Vec::new(),
        });
    }
    for &a in &pairing.added {
        let n = &new[&a];
        rows.push(SchemaChange {
            kind: kind_of(n),
            key: key_of(n),
            format: format_of(Some(n), n),
            before: None,
            after: Some(side(&now, a, n)),
            status: "compatible",
            change: "added",
            tier: "fact",
            note: None,
            changes: Vec::new(),
        });
    }
    rows.sort_by(|x, y| row_key(x).cmp(&row_key(y)));

    let removed_nodes: HashSet<u64> = rev.delta.removed_nodes.iter().map(|id| id.0).collect();
    let present: HashSet<u64> = after
        .graphs
        .iter()
        .flat_map(|g| g.nodes.iter().map(|n| n.id.0))
        .collect();
    let served: HashSet<(u64, u32)> = after
        .all_edges()
        .filter(|e| CLIENT_CATEGORIES.contains(&e.category))
        .map(|e| (e.from.0, e.category.0))
        .collect();
    let lost: Vec<RemovedCall> = rev
        .delta
        .removed_edges
        .iter()
        .filter(|k| CLIENT_CATEGORIES.contains(&k.category))
        .map(|k| RemovedCall {
            client: k.from.0,
            target: k.to.0,
            category: k.category.0,
        })
        .collect();
    let mut orphans: Vec<OrphanedClient> =
        orphaned(&lost, &moved, &present, &served, &removed_nodes)
            .into_iter()
            .map(|o| {
                let at = now.locate(NodeId(o.client));
                let target_removed = o.reason == "target_removed";
                OrphanedClient {
                    client_qname: at.qname,
                    file: at.file,
                    line: at.line,
                    category: edge_category::name(EdgeCategoryId(o.category)),
                    target_qname: then.locate(NodeId(o.target)).qname,
                    reason: o.reason,
                    tier: if target_removed { "fact" } else { "derived" },
                }
            })
            .collect();
    orphans.sort_by(|x, y| {
        (x.category, &x.client_qname, &x.target_qname).cmp(&(
            y.category,
            &y.client_qname,
            &y.target_qname,
        ))
    });
    orphans.dedup_by(|x, y| {
        (x.category, &x.client_qname, &x.target_qname)
            == (y.category, &y.client_qname, &y.target_qname)
    });

    let count = |f: &dyn Fn(&SchemaChange) -> bool| rows.iter().filter(|r| f(r)).count();
    let (n_breaking, n_compatible, n_unknown) = (
        count(&|r| r.status == "breaking"),
        count(&|r| r.status == "compatible"),
        count(&|r| r.status == "unknown"),
    );
    let (n_removed, n_added) = (
        count(&|r| r.change == "removed"),
        count(&|r| r.change == "added"),
    );
    let base = rev.answer.base.clone();
    eprintln!(
        "[contract-breaks] base={base} pairs={} breaking={n_breaking} compatible={n_compatible} unknown={n_unknown} removed={n_removed} added={n_added} orphaned_clients={}",
        pairing.pairs.len(),
        orphans.len(),
    );
    let breaking = n_breaking + orphans.len();
    let listed = rows.len();
    if args.breaking_only {
        rows.retain(|r| r.status == "breaking");
    }
    let absence = (rows.is_empty() && orphans.is_empty()).then(|| {
        let note = if old.is_empty() && new.is_empty() && lost.is_empty() {
            format!(
                "no contract (OpenAPI / AsyncAPI op or message type) in either graph and no client edge removed vs {base}"
            )
        } else if listed > 0 {
            format!(
                "{} contract {} vs {base}: {listed} compatible or unknown {} left out by breaking_only, and no client lost its provider",
                pairing.pairs.len(),
                absence::plural(pairing.pairs.len(), "pair", "pairs"),
                absence::plural(listed, "row", "rows"),
            )
        } else {
            format!(
                "{} contract {} vs {base}, none with a changed declaration; no contract added or removed, and no client lost its provider",
                pairing.pairs.len(),
                absence::plural(pairing.pairs.len(), "pair", "pairs"),
            )
        };
        let mechanisms: Vec<&'static str> = CLIENT_CATEGORIES.iter().map(|&c| edge_category::name(c)).collect();
        let mut a = absence::empty(after, PRIMITIVE, &format!("rev {base}"), "no_match", note, &mechanisms, None);
        a.unparsed_files = rev.after.parse_errors.len();
        a
    });
    ContractBreaks {
        base,
        schemas: rows,
        orphaned_clients: orphans,
        breaking,
        absence,
    }
}

// ---- holders and their pairing ---------------------------------------------

/// Every MESSAGE_TYPE and every OpenAPI / AsyncAPI op of `m` (module docs).
fn contract_holders(m: &MergedGraph) -> BTreeMap<u64, Holder> {
    collect_holders(m)
        .into_iter()
        .filter(|(_, h)| {
            h.message || matches!(h.origin_str("source"), Some("openapi" | "asyncapi"))
        })
        .collect()
}

fn kind_of(h: &Holder) -> &'static str {
    if h.message {
        "message"
    } else if h.origin_str("method").is_some() {
        "operation"
    } else {
        "channel"
    }
}

fn key_of(h: &Holder) -> String {
    if h.message {
        return qualified_name(&h.qname).to_string();
    }
    if let (Some(method), Some(path)) = (h.origin_str("method"), h.origin_str("path")) {
        return format!("{} {path}", method.to_ascii_uppercase());
    }
    match (h.origin_str("action"), h.origin_str("channel")) {
        (Some(action), Some(channel)) => format!("{action} {}", channel.trim_matches('/')),
        (None, Some(channel)) => channel.trim_matches('/').to_string(),
        _ => h.qname.clone(),
    }
}

/// `primary`'s format when it names one, else `fallback`'s.
fn format_of(primary: Option<&Holder>, fallback: &Holder) -> String {
    primary
        .map(Holder::format)
        .filter(|f| !f.is_empty())
        .unwrap_or_else(|| fallback.format())
}

type RowKey<'a> = (
    u8,
    &'static str,
    &'a str,
    &'static str,
    Option<&'a str>,
    Option<&'a str>,
);

fn row_key(r: &SchemaChange) -> RowKey<'_> {
    let rank = match r.status {
        "breaking" => 0,
        "unknown" => 1,
        _ => 2,
    };
    (
        rank,
        r.kind,
        r.key.as_str(),
        r.change,
        r.before.as_ref().map(|s| s.qname.as_str()),
        r.after.as_ref().map(|s| s.qname.as_str()),
    )
}

/// Before holders paired with after holders, and the one-sided rest.
#[derive(Debug, Default, PartialEq)]
struct Pairing {
    /// `(before id, after id, paired through a move)`, by before id.
    pairs: Vec<(u64, u64, bool)>,
    removed: Vec<u64>,
    added: Vec<u64>,
}

/// Moves first (a swap pairs each id with the other's), then the same id on
/// both sides; an after holder is taken once.
fn pair(old: &BTreeSet<u64>, new: &BTreeSet<u64>, moved: &HashMap<u64, u64>) -> Pairing {
    let mut taken: BTreeSet<u64> = BTreeSet::new();
    let mut through: BTreeMap<u64, u64> = BTreeMap::new();
    for &b in old {
        if let Some(&a) = moved.get(&b)
            && new.contains(&a)
            && taken.insert(a)
        {
            through.insert(b, a);
        }
    }
    let mut out = Pairing::default();
    for &b in old {
        if let Some(&a) = through.get(&b) {
            out.pairs.push((b, a, true));
        } else if new.contains(&b) && taken.insert(b) {
            out.pairs.push((b, b, false));
        } else {
            out.removed.push(b);
        }
    }
    out.added = new.iter().filter(|a| !taken.contains(a)).copied().collect();
    out
}

// ---- judging one pair --------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Debug)]
enum AvroMode {
    Backward,
    Forward,
    Full,
}

impl AvroMode {
    fn parse(s: &str) -> AvroMode {
        match s {
            "backward" => AvroMode::Backward,
            "forward" => AvroMode::Forward,
            _ => AvroMode::Full,
        }
    }
}

/// A listed verdict: status, note, changes.
type Verdict = (&'static str, Option<&'static str>, Vec<FieldChange>);

/// `None` when the pair is identical: equal payloads, or no declared
/// difference the rules see (a reordered declaration).
fn judge_pair(old: &Holder, new: &Holder, mode: AvroMode) -> Option<Verdict> {
    let (mut a, mut b) = (old.payloads.clone(), new.payloads.clone());
    a.sort();
    b.sort();
    if a == b {
        return None;
    }
    let op = !old.message && !new.message;
    let (o, n) = match (old.schema(), new.schema()) {
        (Ok(o), Ok(n)) => (o, n),
        // A contract op without SCHEMA_FIELDS declares no body field.
        (Ok(o), Err(Gap::NoFields)) if op => {
            let n = empty_like(&o);
            (o, n)
        }
        (Err(Gap::NoFields), Ok(n)) if op => (empty_like(&n), n),
        (o, n) => return Some(("unknown", Some(gap_note(o.err(), n.err())), Vec::new())),
    };
    if o.format != n.format {
        return Some(("unknown", Some("format_mismatch"), Vec::new()));
    }
    let d = match o.format.as_str() {
        "proto" => proto_evolution(&o, &n),
        "avro" => avro_evolution(&o, &n, mode),
        "openapi" => openapi_evolution(&o, &n),
        "asyncapi" => asyncapi_evolution(&o, &n),
        _ => return Some(("unknown", Some("unsupported_format"), Vec::new())),
    };
    verdict(d)
}

fn empty_like(s: &Schema) -> Schema {
    Schema {
        format: s.format.clone(),
        sections: BTreeMap::new(),
        reserved: Vec::new(),
        truncated: false,
    }
}

fn gap_note(before: Option<Gap>, after: Option<Gap>) -> &'static str {
    match (before, after) {
        (Some(Gap::NoFields), _) => "before_no_fields",
        (_, Some(Gap::NoFields)) => "after_no_fields",
        (Some(Gap::Conflicting), _) | (_, Some(Gap::Conflicting)) => "conflicting_fields",
        _ => "unparsable_fields",
    }
}

/// LE.10c's status rule over one diff; `None` when nothing differs.
fn verdict(d: Diff) -> Option<Verdict> {
    let status = if d.changes.iter().any(|c| c.breaking) {
        "breaking"
    } else if d.suppressed || d.changes.iter().any(|c| c.change == UNKNOWN) {
        "unknown"
    } else if d.changes.is_empty() {
        return None;
    } else {
        "compatible"
    };
    let note = if d.suppressed {
        Some("truncated")
    } else if d
        .changes
        .iter()
        .any(|c| c.rule == "field_removed_unreserved")
    {
        Some("field_removed_unreserved")
    } else {
        None
    };
    Some((status, note, d.changes))
}

// ---- proto -----------------------------------------------------------------

/// LE.10c's symmetric wire rules, read as an evolution: a field only the old
/// side declares was removed, compatible on the wire; under the new side's
/// `reserved` that is the documented retirement (`field_removed_reserved`),
/// without it the number is free for an incompatible reuse
/// (`field_removed_unreserved`).
fn proto_evolution(old: &Schema, new: &Schema) -> Diff {
    let mut d = proto_rules(old, new);
    let numbered: HashSet<&str> = old
        .fields("fields")
        .iter()
        .filter(|f| f.number.is_some())
        .map(|f| f.name.as_str())
        .collect();
    for c in &mut d.changes {
        match (c.change, c.rule) {
            ("producer_only", "proto_unknown_field") if numbered.contains(c.field.as_str()) => {
                c.rule = "field_removed_unreserved";
            }
            ("reserved", "proto_reserved_reused") if c.consumer.as_deref() == Some("reserved") => {
                c.rule = "field_removed_reserved";
                c.breaking = false;
            }
            _ => {}
        }
    }
    d
}

// ---- avro ------------------------------------------------------------------

/// Avro resolution in `mode`'s direction(s), every change oriented
/// before -> after.
fn avro_evolution(old: &Schema, new: &Schema, mode: AvroMode) -> Diff {
    let backward = || avro_rules(old, new);
    let forward = || {
        let mut d = avro_rules(new, old);
        d.changes.iter_mut().for_each(flip);
        d
    };
    match mode {
        AvroMode::Backward => backward(),
        AvroMode::Forward => forward(),
        AvroMode::Full => {
            let (mut d, f) = (backward(), forward());
            d.suppressed |= f.suppressed;
            for c in f.changes {
                let dup = d.changes.iter().any(|x| {
                    (
                        x.section.as_str(),
                        x.field.as_str(),
                        x.change,
                        x.rule,
                        x.breaking,
                        &x.producer,
                        &x.consumer,
                    ) == (
                        c.section.as_str(),
                        c.field.as_str(),
                        c.change,
                        c.rule,
                        c.breaking,
                        &c.producer,
                        &c.consumer,
                    )
                });
                if !dup {
                    d.changes.push(c);
                }
            }
            d
        }
    }
}

/// A change judged with the sides swapped, re-oriented before -> after.
fn flip(c: &mut FieldChange) {
    std::mem::swap(&mut c.producer, &mut c.consumer);
    c.change = match c.change {
        "producer_only" => "consumer_only",
        "consumer_only" => "producer_only",
        other => other,
    };
}

// ---- JSON-schema-shaped contracts (OpenAPI, AsyncAPI) -----------------------

/// Which way values cross the contract, for a type change.
#[derive(Clone, Copy, PartialEq)]
enum Flow {
    /// Clients send it (a request): the new type must accept every old value.
    Input,
    /// Clients read it (a response): the old type must accept every new value.
    Output,
    /// Either (an AsyncAPI payload): any type change breaks.
    Both,
}

/// The rule names and verdicts of one section kind.
struct Evolution {
    flow: Flow,
    /// A field only the old side declares.
    removed: (&'static str, bool),
    /// A field only the new side declares, optional.
    added: (&'static str, bool),
    /// A field only the new side declares, required.
    added_required: (&'static str, bool),
    /// A field both declare that became required.
    now_required: (&'static str, bool),
    /// A field both declare that stopped being required.
    now_optional: (&'static str, bool),
    type_changed: &'static str,
    type_compatible: &'static str,
}

const REQUEST: Evolution = Evolution {
    flow: Flow::Input,
    removed: ("request_field_removed", false),
    added: ("request_field_added", false),
    added_required: ("new_required_request_field", true),
    now_required: ("new_required_request_field", true),
    now_optional: ("request_field_now_optional", false),
    type_changed: "request_type_changed",
    type_compatible: "request_type_widened",
};

const RESPONSE: Evolution = Evolution {
    flow: Flow::Output,
    removed: ("response_field_removed", true),
    added: ("response_field_added", false),
    added_required: ("response_field_added", false),
    now_required: ("response_field_now_required", false),
    now_optional: ("response_field_now_optional", true),
    type_changed: "response_type_changed",
    type_compatible: "response_type_narrowed",
};

const PAYLOAD: Evolution = Evolution {
    flow: Flow::Both,
    removed: ("payload_field_removed", true),
    added: ("payload_field_added", false),
    added_required: ("payload_field_added", false),
    now_required: ("payload_field_now_required", false),
    now_optional: ("payload_field_now_optional", true),
    type_changed: "payload_type_changed",
    type_compatible: "payload_type_changed",
};

/// OpenAPI: `request` by [`REQUEST`]; each `response:<code>` by [`RESPONSE`],
/// a whole response section on one side only judged as a section. Any other
/// section is compared as a response.
fn openapi_evolution(old: &Schema, new: &Schema) -> Diff {
    let mut d = Diff::default();
    let empty: Vec<Field> = Vec::new();
    for sec in section_names(old, new) {
        let (of, nf) = (old.sections.get(sec), new.sections.get(sec));
        if sec == "request" {
            evolve_section(
                &mut d,
                sec,
                (of.unwrap_or(&empty), old.truncated),
                (nf.unwrap_or(&empty), new.truncated),
                &REQUEST,
            );
            continue;
        }
        match (of, nf) {
            (Some(of), Some(nf)) => evolve_section(
                &mut d,
                sec,
                (of, old.truncated),
                (nf, new.truncated),
                &RESPONSE,
            ),
            (Some(_), None) if new.truncated => d.suppressed = true,
            (Some(_), None) => d.push(
                (sec, ""),
                "section",
                (Some("declared"), None),
                "response_removed",
                true,
            ),
            (None, Some(_)) if old.truncated => d.suppressed = true,
            (None, Some(_)) => d.push(
                (sec, ""),
                "section",
                (None, Some("declared")),
                "response_added",
                false,
            ),
            (None, None) => {}
        }
    }
    d
}

/// AsyncAPI: every `payload[:<name>]` section by [`PAYLOAD`], a section on
/// one side only as all of its fields.
fn asyncapi_evolution(old: &Schema, new: &Schema) -> Diff {
    let mut d = Diff::default();
    let empty: Vec<Field> = Vec::new();
    for sec in section_names(old, new) {
        let (of, nf) = (old.sections.get(sec), new.sections.get(sec));
        evolve_section(
            &mut d,
            sec,
            (of.map_or(&empty[..], Vec::as_slice), old.truncated),
            (nf.map_or(&empty[..], Vec::as_slice), new.truncated),
            &PAYLOAD,
        );
    }
    d
}

fn section_names<'a>(old: &'a Schema, new: &'a Schema) -> BTreeSet<&'a str> {
    old.sections
        .keys()
        .chain(new.sections.keys())
        .map(String::as_str)
        .collect()
}

/// One section, old fields `of` vs new fields `nf`, each with its side's
/// `truncated` flag, by `ev`. LE.10c's `json_section` guards, read as an
/// evolution: a whole-body `$ref` or a field under a `$ref` that differs is
/// `unknown`; a field absent from a truncated side is never reported; only
/// the topmost of a removed or added subtree is.
fn evolve_section(
    d: &mut Diff,
    sec: &str,
    (of, ot): (&[Field], bool),
    (nf, nt): (&[Field], bool),
    ev: &Evolution,
) {
    let o_by: HashMap<&str, &Field> = of.iter().map(|f| (f.name.as_str(), f)).collect();
    let n_by: HashMap<&str, &Field> = nf.iter().map(|f| (f.name.as_str(), f)).collect();
    let o_root = o_by.get("$ref").map(|f| f.ty.as_deref());
    let n_root = n_by.get("$ref").map(|f| f.ty.as_deref());
    if (o_root.is_some() || n_root.is_some()) && o_root != n_root {
        d.push(
            (sec, "$ref"),
            UNKNOWN,
            (o_root.flatten(), n_root.flatten()),
            "unresolved_ref",
            false,
        );
        return;
    }
    let refs: HashSet<&str> = of
        .iter()
        .chain(nf)
        .filter(|f| is_ref(f.ty.as_deref()))
        .map(|f| f.name.as_str())
        .collect();
    let ty_of = |by: &HashMap<&str, &Field>, name: &str| {
        by.get(name)
            .and_then(|f| f.ty.as_deref())
            .map(str::to_string)
    };
    let mut unknown: BTreeSet<String> = BTreeSet::new();
    let mut ref_unknown = |d: &mut Diff, name: &str| {
        if unknown.insert(name.to_string()) {
            let (a, b) = (ty_of(&o_by, name), ty_of(&n_by, name));
            d.push(
                (sec, name),
                UNKNOWN,
                (a.as_deref(), b.as_deref()),
                "unresolved_ref",
                false,
            );
        }
    };
    // Old and new names reported absent from the other side.
    let (mut gone, mut arrived): (HashSet<&str>, HashSet<&str>) = (HashSet::new(), HashSet::new());

    for f in of {
        let name = f.name.as_str();
        if let Some(r) = ancestors(name).find(|a| refs.contains(a)) {
            if ty_of(&o_by, r) != ty_of(&n_by, r) {
                ref_unknown(d, r);
            }
            continue;
        }
        let Some(g) = n_by.get(name) else {
            if ancestors(name).any(|a| gone.contains(a)) {
                continue;
            }
            if nt {
                d.suppressed = true;
                continue;
            }
            gone.insert(name);
            let (rule, breaking) = ev.removed;
            d.push(
                (sec, name),
                "producer_only",
                (f.ty.as_deref(), None),
                rule,
                breaking,
            );
            continue;
        };
        let (a, b) = (f.ty.as_deref(), g.ty.as_deref());
        if a != b {
            if is_ref(a) || is_ref(b) {
                ref_unknown(d, name);
            } else {
                type_change(d, sec, name, (a, b), ev);
            }
        }
        if f.required != g.required {
            let (label, (rule, breaking)) = if g.required {
                (("optional", "required"), ev.now_required)
            } else {
                (("required", "optional"), ev.now_optional)
            };
            d.push(
                (sec, name),
                "required",
                (Some(label.0), Some(label.1)),
                rule,
                breaking,
            );
        }
    }
    for g in nf {
        let name = g.name.as_str();
        if o_by.contains_key(name)
            || ancestors(name).any(|a| refs.contains(a) || arrived.contains(a))
        {
            continue;
        }
        if ot {
            d.suppressed = true;
            continue;
        }
        arrived.insert(name);
        let (rule, breaking) = if g.required {
            ev.added_required
        } else {
            ev.added
        };
        d.push(
            (sec, name),
            "consumer_only",
            (None, g.ty.as_deref()),
            rule,
            breaking,
        );
    }
}

/// A field both sides declare with different types, by `ev.flow`.
fn type_change(
    d: &mut Diff,
    sec: &str,
    name: &str,
    (a, b): (Option<&str>, Option<&str>),
    ev: &Evolution,
) {
    let (old, new) = (a.unwrap_or("any"), b.unwrap_or("any"));
    // `(from, to)` for json_type_change: every `from` value must be a `to` value.
    let judged = match ev.flow {
        Flow::Input => json_type_change(old, new),
        Flow::Output => json_type_change(new, old),
        Flow::Both if old == "any" || new == "any" => Some(TypeChange::Untyped),
        Flow::Both => Some(TypeChange::Changed),
    };
    match judged {
        Some(TypeChange::Changed) => d.push((sec, name), "type", (a, b), ev.type_changed, true),
        Some(TypeChange::Untyped) => d.push((sec, name), UNKNOWN, (a, b), "untyped", false),
        // Widened, or the accepting side takes any value.
        Some(TypeChange::Widened) | None => {
            d.push((sec, name), "type", (a, b), ev.type_compatible, false)
        }
    }
}

// ---- orphaned clients --------------------------------------------------------

/// One client -> provider edge the delta removed, as before-side ids.
struct RemovedCall {
    client: u64,
    target: u64,
    category: u32,
}

/// An orphan before it is located: the client's after id.
#[derive(Debug, PartialEq)]
struct Orphan {
    client: u64,
    target: u64,
    category: u32,
    reason: &'static str,
}

/// The removed calls whose client survives (`present`, directly or through
/// `moved`) with no edge of the call's category left (`served`, after ids).
fn orphaned(
    lost: &[RemovedCall],
    moved: &HashMap<u64, u64>,
    present: &HashSet<u64>,
    served: &HashSet<(u64, u32)>,
    removed_nodes: &HashSet<u64>,
) -> Vec<Orphan> {
    let mut out = Vec::new();
    for call in lost {
        let client = moved.get(&call.client).copied().unwrap_or(call.client);
        if !present.contains(&client) || served.contains(&(client, call.category)) {
            continue;
        }
        let reason = if removed_nodes.contains(&call.target) {
            "target_removed"
        } else {
            "pairing_lost"
        };
        out.push(Orphan {
            client,
            target: call.target,
            category: call.category,
            reason,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn holder(message: bool, payload: &str) -> Holder {
        Holder {
            repo: 1,
            message,
            qname: "q".to_string(),
            payloads: vec![payload.to_string()],
            origin: None,
        }
    }

    fn schema(payload: &str) -> Schema {
        holder(false, payload)
            .schema()
            .unwrap_or_else(|_| panic!("parses: {payload}"))
    }

    fn rules(d: &Diff) -> Vec<(&str, &str, &str, bool)> {
        d.changes
            .iter()
            .map(|c| (c.field.as_str(), c.change, c.rule, c.breaking))
            .collect()
    }

    #[test]
    fn pairing_follows_moves_then_identity() {
        let old: BTreeSet<u64> = [1, 2, 3, 4].into();
        let new: BTreeSet<u64> = [1, 2, 5, 6].into();
        // 1 and 2 swap ids; 3 moves to 5; 4 is gone; 6 is new.
        let moved: HashMap<u64, u64> = [(1, 2), (2, 1), (3, 5)].into();
        let p = pair(&old, &new, &moved);
        assert_eq!(p.pairs, vec![(1, 2, true), (2, 1, true), (3, 5, true)]);
        assert_eq!((p.removed, p.added), (vec![4], vec![6]));
        let p = pair(&old, &new, &HashMap::new());
        assert_eq!(p.pairs, vec![(1, 1, false), (2, 2, false)]);
        assert_eq!((p.removed, p.added), (vec![3, 4], vec![5, 6]));
    }

    #[test]
    fn orphans_by_reason() {
        let lost = [
            RemovedCall {
                client: 10,
                target: 20,
                category: 1,
            },
            RemovedCall {
                client: 11,
                target: 21,
                category: 1,
            },
            RemovedCall {
                client: 12,
                target: 22,
                category: 1,
            },
            RemovedCall {
                client: 13,
                target: 23,
                category: 1,
            },
            RemovedCall {
                client: 14,
                target: 24,
                category: 1,
            },
        ];
        // 10: target removed; 11 moved to 31, target survives; 12 is gone;
        // 13 still calls something; 14's target survives and nothing pairs it.
        let moved: HashMap<u64, u64> = [(11, 31)].into();
        let present: HashSet<u64> = [10, 31, 13, 14, 21, 23, 24].into();
        let served: HashSet<(u64, u32)> = [(13, 1)].into();
        let removed: HashSet<u64> = [20, 12, 22].into();
        let got = orphaned(&lost, &moved, &present, &served, &removed);
        assert_eq!(
            got,
            vec![
                Orphan {
                    client: 10,
                    target: 20,
                    category: 1,
                    reason: "target_removed"
                },
                Orphan {
                    client: 31,
                    target: 21,
                    category: 1,
                    reason: "pairing_lost"
                },
                Orphan {
                    client: 14,
                    target: 24,
                    category: 1,
                    reason: "pairing_lost"
                },
            ]
        );
    }

    #[test]
    fn truncated_side_never_reports_absence() {
        let old = schema(
            r#"{"format":"openapi","response:200":[{"name":"id","type":"string"},{"name":"total","type":"number"}]}"#,
        );
        let new = schema(
            r#"{"format":"openapi","response:200":[{"name":"id","type":"string"}],"truncated":true}"#,
        );
        let d = openapi_evolution(&old, &new);
        assert!(d.suppressed && d.changes.is_empty());
        assert_eq!(
            verdict(d).map(|v| (v.0, v.1)),
            Some(("unknown", Some("truncated")))
        );
        // A whole response section missing from a truncated side is not "removed" either.
        let new = schema(
            r#"{"format":"openapi","request":[{"name":"x","type":"string"}],"truncated":true}"#,
        );
        assert!(openapi_evolution(&old, &new).suppressed);
    }

    #[test]
    fn nested_removal_reports_the_topmost_field_and_refs_are_unknown() {
        let old = schema(
            r##"{"format":"openapi","response:200":[{"name":"customer","type":"object"},{"name":"customer.name","type":"string"},{"name":"lines","type":"#/components/schemas/Line"}]}"##,
        );
        let new = schema(
            r##"{"format":"openapi","response:200":[{"name":"lines","type":"#/components/schemas/Line2"}]}"##,
        );
        let d = openapi_evolution(&old, &new);
        assert_eq!(
            rules(&d),
            [
                ("customer", "producer_only", "response_field_removed", true),
                ("lines", "unknown", "unresolved_ref", false)
            ]
        );
    }

    #[test]
    fn type_changes_follow_the_flow() {
        let old = schema(
            r#"{"format":"openapi","request":[{"name":"n","type":"integer"}],"response:200":[{"name":"n","type":"integer"},{"name":"s","type":"string|null"}]}"#,
        );
        let new = schema(
            r#"{"format":"openapi","request":[{"name":"n","type":"number"}],"response:200":[{"name":"n","type":"number"},{"name":"s","type":"string"}]}"#,
        );
        let d = openapi_evolution(&old, &new);
        assert_eq!(
            rules(&d),
            [
                ("n", "type", "request_type_widened", false),
                ("n", "type", "response_type_changed", true),
                ("s", "type", "response_type_narrowed", false),
            ]
        );
        let old = schema(
            r#"{"format":"asyncapi","payload":[{"name":"n","type":"integer"},{"name":"gone","type":"string"}]}"#,
        );
        let new = schema(
            r#"{"format":"asyncapi","payload":[{"name":"n","type":"number"},{"name":"new","type":"string"}]}"#,
        );
        assert_eq!(
            rules(&asyncapi_evolution(&old, &new)),
            [
                ("n", "type", "payload_type_changed", true),
                ("gone", "producer_only", "payload_field_removed", true),
                ("new", "consumer_only", "payload_field_added", false),
            ]
        );
    }

    #[test]
    fn response_field_now_optional_breaks() {
        let old = schema(
            r#"{"format":"openapi","response:200":[{"name":"id","type":"string","required":true}]}"#,
        );
        let new = schema(r#"{"format":"openapi","response:200":[{"name":"id","type":"string"}]}"#);
        assert_eq!(
            rules(&openapi_evolution(&old, &new)),
            [("id", "required", "response_field_now_optional", true)]
        );
    }

    #[test]
    fn identical_and_gapped_pairs() {
        let a = holder(
            true,
            r#"{"format":"proto","fields":[{"name":"id","type":"string","number":1}]}"#,
        );
        assert!(judge_pair(&a, &a, AvroMode::Backward).is_none());
        let enum_like = Holder {
            payloads: Vec::new(),
            ..holder(true, "")
        };
        assert_eq!(
            judge_pair(&a, &enum_like, AvroMode::Backward).map(|v| (v.0, v.1)),
            Some(("unknown", Some("after_no_fields")))
        );
        let avro = holder(
            true,
            r#"{"format":"avro","fields":[{"name":"id","type":"string"}]}"#,
        );
        assert_eq!(
            judge_pair(&a, &avro, AvroMode::Backward).map(|v| (v.0, v.1)),
            Some(("unknown", Some("format_mismatch")))
        );
        // A reordered declaration differs in bytes, not in the contract.
        let two = holder(
            true,
            r#"{"format":"proto","fields":[{"name":"id","type":"string","number":1},{"name":"t","type":"int64","number":2}]}"#,
        );
        let swapped = holder(
            true,
            r#"{"format":"proto","fields":[{"name":"t","type":"int64","number":2},{"name":"id","type":"string","number":1}]}"#,
        );
        assert!(judge_pair(&two, &swapped, AvroMode::Backward).is_none());
        // An op that loses its only body lists that as a removal.
        let op = holder(
            false,
            r#"{"format":"openapi","response:200":[{"name":"id","type":"string"}]}"#,
        );
        let bare = Holder {
            payloads: Vec::new(),
            ..holder(false, "")
        };
        let v = judge_pair(&op, &bare, AvroMode::Backward).expect("a verdict");
        assert_eq!(v.0, "breaking");
        assert_eq!(v.2.first().map(|c| c.rule), Some("response_removed"));
    }

    #[test]
    fn avro_modes_parse_strictly() {
        assert_eq!(AvroMode::parse("backward"), AvroMode::Backward);
        assert_eq!(AvroMode::parse("forward"), AvroMode::Forward);
        assert_eq!(AvroMode::parse("full"), AvroMode::Full);
        assert_eq!(
            AvroMode::parse("sideways"),
            AvroMode::Full,
            "an unknown mode is judged strictest"
        );
        assert_eq!(ContractBreakArgs::default().avro_mode, AVRO_MODES[0]);
    }
}
