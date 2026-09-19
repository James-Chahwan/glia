//! Event-bus resolver — emitter → handler by event name.
//!
//! OWNERS (LB.8b). An event side inside a nested project carries LB.4a's
//! ` @<project path>` owner (engine `http_owner`). An in-process bus (Node
//! EventEmitter, NestJS `@OnEvent`, CQRS, Spring / MediatR, RxJS, DOM)
//! delivers only inside one process, and two nested projects are two build
//! artefacts, so an emitter pairs a handler only when:
//! - both carry the SAME owner in the same repo (`same-owner`);
//! - either side is unowned (`unowned-side`): a file under no nested project
//!   may be linked into any of them, so it keeps pairing with every owner
//!   (and a build without nested projects is unchanged);
//! - either side is transport-scoped (`transport`): the extractor marked it
//!   with an ORIGIN `"delivery":"transport"` (NestJS microservices
//!   `@EventPattern` and its `ClientProxy.emit`, AWS EventBridge), a network
//!   bus that crosses processes like a queue.
//!
//! Every other pair is dropped and counted (`cross-owner-dropped`). Owners are
//! compared together with the RepoId, so the same rel path in two repos is two
//! projects.

use std::collections::HashMap;

use repo_graph_code_domain::endpoint::split_owner;
use repo_graph_code_domain::{CodeNav, cell_type, edge_category, node_kind};
use repo_graph_core::{Cell, CellPayload, Edge, NodeId, RepoId};

use super::{CrossGraphResolver, RuleTally, ServiceTarget, weakest};
use crate::merged::MergedGraph;

// ============================================================================
// EventBusResolver — matches event emitter → handler by event name
// ============================================================================

pub struct EventBusResolver;

/// EVENT_* nodes come from two places: the cross-cutting extractor, whose
/// qnames carry an `event_emit:` / `event_handle:` prefix, and language parsers
/// (Solidity today) whose qnames are ordinary code qnames like
/// `Auction::Auction::BidPlaced`. Key on the prefix when it is there and on the
/// node's simple NAME when it is not, so a Solidity `BidPlaced` can reach a
/// Java `@EventListener(BidPlaced)`. `build_kind_index` is deliberately left
/// alone — GraphQL, WebSocket and CLI all share it and none of them has a
/// parser-side producer of the same kind.
///
/// LB.8b: the owner segment is split off first and returned beside the key.
/// A code qname never carries one, so a Solidity event is unowned.
fn event_key<'q>(
    nav: &CodeNav,
    id: NodeId,
    qname: &'q str,
    prefix: &str,
) -> Option<(String, Option<&'q str>)> {
    let (bare, owner) = split_owner(qname);
    let key = match bare.strip_prefix(prefix) {
        Some(rest) => rest.to_string(),
        None => nav.name_by_id.get(&id).cloned()?,
    };
    Some((key, owner))
}

/// LB.8b: does this side travel over a transport? The extractor records it as
/// an ORIGIN cell carrying `"delivery":"transport"`; a substring test (the
/// `http::extract_method_field` precedent), no serde_json in the graph crate.
fn is_transport(cells: &[Cell]) -> bool {
    cells.iter().any(|c| {
        c.kind == cell_type::ORIGIN
            && matches!(&c.payload, CellPayload::Json(j) if j.contains(r#""delivery":"transport""#))
    })
}

/// One event side as the owner rule reads it.
#[derive(Clone, Copy)]
struct Side<'g> {
    repo: RepoId,
    owner: Option<&'g str>,
    transport: bool,
}

/// One indexed handler.
struct HandlerEntry<'g> {
    target: ServiceTarget,
    side: Side<'g>,
}

/// Why an emitter / handler pair was kept, or that it was dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pairing {
    /// Both owned, by the same project of the same repo.
    SameOwner,
    /// Either side is under no nested project.
    UnownedSide,
    /// Two different owners, and either side is transport-scoped.
    Transport,
    /// Two different owners on in-process buses.
    Dropped,
}

fn pairing(e: Side<'_>, h: Side<'_>) -> Pairing {
    match (e.owner, h.owner) {
        (Some(a), Some(b)) if a == b && e.repo == h.repo => Pairing::SameOwner,
        (None, _) | (_, None) => Pairing::UnownedSide,
        _ if e.transport || h.transport => Pairing::Transport,
        _ => Pairing::Dropped,
    }
}

/// Per-build tallies behind the `[eventbus-owner]` line.
#[derive(Default)]
struct OwnerTally {
    same_owner: usize,
    unowned_side: usize,
    transport: usize,
    dropped: usize,
    /// Any indexed handler or matched emitter carried an owner.
    owned: bool,
}

/// Type-named keys fold: `OrderPlacedEvent` and `OrderPlaced` are one event.
/// Applied ONLY to keys that look like a TYPE — a plain identifier starting
/// uppercase — so string topics (`user.created`) and the extractor's tag
/// fallbacks (`emit`, `on`, `@OnEvent`, `Subject.next`) keep matching
/// byte-exactly. There is deliberately NO separator folding: `user.created`
/// must not become `usercreated`, which is the all-to-all shape the queue side
/// just closed.
fn normalise_event_key(raw: &str) -> String {
    let is_type = raw.chars().next().is_some_and(char::is_uppercase)
        && raw.chars().all(|c| c.is_alphanumeric() || c == '_');
    if !is_type {
        return raw.to_string();
    }
    let low = raw.to_lowercase();
    let stripped = low
        .strip_suffix("events")
        .or_else(|| low.strip_suffix("event"));
    match stripped {
        // `Event` alone folds to nothing; keep a stem worth matching on.
        Some(stem) if stem.len() >= 3 => stem.to_string(),
        _ => low,
    }
}

impl CrossGraphResolver for EventBusResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        // Lookup-only map; each Vec keeps graph / node order, so the edges
        // come out in the same order on every run.
        let mut handler_index: HashMap<String, Vec<HandlerEntry<'_>>> = HashMap::new();
        let mut prefixed = 0usize;
        let mut by_name = 0usize;
        let mut tally = OwnerTally::default();
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::EVENT_HANDLER) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
                    continue;
                };
                let Some((key, owner)) = event_key(&g.nav, n.id, qname, "event_handle:") else {
                    continue;
                };
                if qname.starts_with("event_handle:") {
                    prefixed += 1;
                } else {
                    by_name += 1;
                }
                tally.owned |= owner.is_some();
                handler_index
                    .entry(normalise_event_key(&key))
                    .or_default()
                    .push(HandlerEntry {
                        target: ServiceTarget {
                            id: n.id,
                            confidence: n.confidence,
                        },
                        side: Side {
                            repo: g.repo,
                            owner,
                            transport: is_transport(&n.cells),
                        },
                    });
            }
        }

        let mut exact = 0usize;
        let mut folded = 0usize;
        // LC.3c: `exact` when the emitter's raw key is its normalised key,
        // `folded` when the type-name fold made it (the counters' branch).
        let mut rules = RuleTally::new("eventbus", &["exact", "folded"]);
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::EVENT_EMITTER) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
                    continue;
                };
                let Some((raw, owner)) = event_key(&g.nav, n.id, qname, "event_emit:") else {
                    continue;
                };
                let key = normalise_event_key(&raw);
                let Some(targets) = handler_index.get(&key) else {
                    continue;
                };
                tally.owned |= owner.is_some();
                let emitter = Side {
                    repo: g.repo,
                    owner,
                    transport: is_transport(&n.cells),
                };
                for h in targets {
                    match pairing(emitter, h.side) {
                        Pairing::SameOwner => tally.same_owner += 1,
                        Pairing::UnownedSide => tally.unowned_side += 1,
                        Pairing::Transport => tally.transport += 1,
                        Pairing::Dropped => {
                            tally.dropped += 1;
                            continue;
                        }
                    }
                    let rule = if key == raw {
                        exact += 1;
                        "exact"
                    } else {
                        folded += 1;
                        "folded"
                    };
                    let confidence = weakest(n.confidence, h.target.confidence);
                    merged.cross_edges.push(
                        Edge::new(n.id, h.target.id, edge_category::EVENT_FLOWS, confidence)
                            .with_cell(rules.cell(rule)),
                    );
                }
            }
        }

        rules.report();
        // One line per BUILD, and only when this resolver had anything to say —
        // the `[ws-resolve]` house style. `pairs` counts PUSHED edges.
        let pairs = exact + folded;
        if pairs > 0 || by_name > 0 {
            eprintln!(
                "[eventbus] {pairs} pairs (exact={exact} type-folded={folded}); \
                 handlers indexed: prefixed={prefixed} by-name={by_name}"
            );
        }
        // LB.8b fired_on marker, once per build that holds an owned event
        // side (a repo with nested projects); silent otherwise, so an
        // owner-free build prints exactly what it did before.
        if tally.owned {
            eprintln!(
                "[eventbus-owner] same-owner={} unowned-side={} transport={} cross-owner-dropped={}",
                tally.same_owner, tally.unowned_side, tally.transport, tally.dropped
            );
        }
    }
}
