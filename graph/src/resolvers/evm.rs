//! EVM contract resolver (DT.10b): a client contract call or event
//! subscription -> the Solidity procedure / event of the contract it reaches.
//!
//! The vocabulary is `glia_code_domain::evm` (DT.10a): a callable contract
//! member is an RPC_PROCEDURE `evm:<Contract>.<member>` with a `via:evm` CODE
//! cell, HANDLED_BY its METHOD / STATE_VAR; a client call is an RPC_CALL
//! `evm_call:<scope>.<member>` and a subscription an EVENT_HANDLER
//! `evm_event:<scope>.<event>`, each with one `via:evm` ENDPOINT_HIT cell per
//! distinct `(lib, abi)` of its sites. Every cell is read through that module
//! (the graph crate has no serde_json).
//!
//! # The contract index
//!
//! A DECL is a Solidity contract or interface (a CLASS / INTERFACE node).
//! - Its FUNCTIONS are read from the procedures: the decl owning
//!   `evm:<C>.<m>` is the nav parent of each of the procedure's HANDLED_BY
//!   targets in its graph, never the procedure's own nav parent, which a
//!   second same-named contract in another file overwrites (both contracts
//!   share the one procedure node, the qname being per repo). A procedure
//!   with no HANDLED_BY (a getter whose STATE_VAR the noise gate dropped: the
//!   module CONTAINS it) falls back to its own nav parent. The decl's name
//!   must be `C`. Each file's facts are the evm CODE cell that follows that
//!   file's POSITION cell on the procedure (merge_parses appends a duplicate
//!   node's cells), else the first one.
//! - Its EVENTS are the EVENT_EMITTERs with a code qname (no `event_emit:`
//!   prefix) and a `.sol` POSITION file ([`is_solidity_event`]) whose nav
//!   parent is the decl, keyed by their nav name. This indexes an
//!   events-only contract too, and keeps any other language's code-qname
//!   event out.
//! - Its BASES are the transitive INHERITS_FROM / IMPLEMENTS targets among
//!   the graph's CLASS / INTERFACE nodes, breadth first in edge order, at
//!   most [`MAX_BASE_DEPTH`] hops, cycle-guarded, followed whether or not a
//!   base is itself indexed.
//! - Its VISIBLE ABI is its own members, then its bases' members. For one
//!   name the definitions are ranked own first, then the bases' bodied ones
//!   in base order, then their abstract ones. A member inherited from a base
//!   outside the build is not visible: containment is STRICT.
//! - A decl is CONCRETE when it is not an INTERFACE and every visible name's
//!   first-ranked definition has a body. Where a rule says "non-concrete
//!   decls are dropped", they are dropped only when a concrete one remains,
//!   so an interface (or an abstract base) never steals its implementation's
//!   pairing, and still pairs when it is all there is.
//!
//! # Identification, per (client, ABI variant)
//!
//! A client's VARIANTS are the distinct `abi` lists of its cells, plus "no
//! ABI" when a cell carries none; a node id held by several graphs is one
//! client with its cells unioned. The first rule that applies wins:
//! - `contract` (DERIVED): `Scope::Contract(C)` - every decl named `C`.
//! - `abi` (DERIVED): `Scope::Abi(N)` with an ABI of two or more entries -
//!   the one decl (concrete preferred) whose visible ABI holds every entry:
//!   a function entry needs that member with that arity (`/?` any arity), an
//!   event entry an event of that name (event arity is not recorded server
//!   side). Two or more covering: `abi_hint` (HEURISTIC) when exactly one of
//!   them has the name key (`evm::name_key`) of `N`, else ambiguous. None
//!   covering: unmatched, with no fall-back to the name.
//! - `name_hint` (HEURISTIC): `Scope::Abi(N)` with no ABI or a one-entry ABI,
//!   and `Scope::Recv(R)` - the one decl (concrete preferred) whose name key
//!   is that of `N` / `R` (three characters at least) and that holds every
//!   listed entry. A one-entry ABI is a match on a member name alone, so it
//!   is never DERIVED. Two or more: ambiguous; none: unmatched.
//!
//! No rule ever matches on the member name alone.
//!
//! # Agreement
//!
//! Every variant that identified something must name the same decl set, and
//! no variant with two or more ABI entries may be unmatched or ambiguous
//! while another identifies a decl; otherwise the client pairs nothing
//! (`conflict`). Two files that reuse one ABI binding name for two contracts
//! share one client node (the qname has no file), so pairing each variant on
//! its own would route each file's caller to the other's contract. The
//! agreed rule is the strongest any variant reached.
//!
//! # Pairing, per identified decl
//!
//! A call targets the member's procedure among its ranked definitions, the
//! first one that fits; the client misses with
//! - `not_declared`: the decl has no such visible member;
//! - `not_in_abi`: an ABI variant that lists entries does not list it;
//! - `arity_mismatch`: no definition fits. A variant whose ABI gives the
//!   member's arity fits a definition with that arity; one that gives none
//!   fits when every `args` value its sites passed is an arity `a` of the
//!   definition, or `a + 1` for a library that takes a trailing overrides
//!   argument ([`OVERRIDES_LIBS`]: ethers, hardhat, typechain; never viem,
//!   web3 or geth).
//!
//! An event subscription targets the EVENT_EMITTER the decl or a base
//! declares (`not_declared` / `not_in_abi` as for a call).
//!
//! Edges: RPC_CALLS client -> procedure, EVENT_FLOWS emitter -> client (the
//! EventBus direction, which leaves `evm_event:` handlers and Solidity events
//! to this resolver). Confidence: the weaker end for `contract` / `abi`, Weak
//! for `abi_hint` / `name_hint`. One edge per `(from, to)`, each with the
//! EVIDENCE `resolver:evm` + its rule (engine `why` tiers the two hint rules
//! heuristic). Every index iterated is a Vec or BTreeMap in graph order then
//! node order; the hash maps are lookup-only.
//!
//! # Marker
//!
//! Once per resolve with procedures or clients:
//! `[evm-link] calls=<c> events=<e> paired=<p> edges=<n> (contract=<a> abi=<b>
//! abi_hint=<h> name_hint=<k>) unmatched=<u> ambiguous=<m> conflict=<f>
//! not_declared=<d> arity_mismatch=<x> not_in_abi=<i> procedures=<P>
//! contracts=<K>`. c / e / p / u / m / f / d / x / i count distinct client
//! nodes (one client may pair one decl and miss another), the parenthesis
//! counts edges by rule, P the procedures and K the indexed decls. Grep
//! `^\[evm-link\] `. RuleTally adds `[evidence-rules] resolver=evm ..`.

use std::collections::{BTreeMap, HashMap, HashSet};

use glia_code_domain::endpoint::split_owner;
use glia_code_domain::evm::{self, AbiEntry, ClientFacts, ClientSide, EntryKind, ProcedureFacts, Scope};
use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId};

use super::{CrossGraphResolver, RuleTally, weakest};
use crate::merged::MergedGraph;
use crate::types::RepoGraph;

pub struct EvmContractResolver;

/// The pairing rules, strongest first: `contract` and `abi` are DERIVED,
/// `abi_hint` and `name_hint` HEURISTIC.
const RULES: [&str; 4] = ["contract", "abi", "abi_hint", "name_hint"];

/// The most INHERITS_FROM / IMPLEMENTS hops a decl's bases are followed.
const MAX_BASE_DEPTH: usize = 8;

/// The fewest characters a name key needs to identify a contract.
const NAME_MIN: usize = 3;

/// Client libraries whose contract call takes one trailing overrides
/// argument (`{ value, gasLimit }`) after the function's own.
const OVERRIDES_LIBS: &[&str] = &["ethers", "hardhat", "typechain"];

/// The qname prefix of an extractor-minted event emitter, which a Solidity
/// event never carries (EventBusResolver keys on the same literal).
const EXTRACTOR_EMIT_PREFIX: &str = "event_emit:";

/// The `file` of a POSITION cell (JSON `{"file":"…",…}`).
fn cell_file(cell: &Cell) -> Option<&str> {
    if cell.kind != cell_type::POSITION {
        return None;
    }
    let CellPayload::Json(j) = &cell.payload else {
        return None;
    };
    let marker = "\"file\":\"";
    let start = j.find(marker)? + marker.len();
    let end = j[start..].find('"')? + start;
    Some(&j[start..end])
}

/// The POSITION file of `node`.
fn node_file(node: &Node) -> Option<&str> {
    node.cells.iter().find_map(cell_file)
}

/// A Solidity-declared event: an EVENT_EMITTER (the caller checks the kind)
/// with a code qname and a `.sol` POSITION file. It belongs to this resolver
/// alone: EventBusResolver never pairs it by name.
pub(crate) fn is_solidity_event(qname: &str, node: &Node) -> bool {
    !split_owner(qname).0.starts_with(EXTRACTOR_EMIT_PREFIX)
        && node_file(node).is_some_and(|f| f.ends_with(".sol"))
}

/// One end of a pairing.
#[derive(Debug, Clone, Copy)]
struct Target {
    id: NodeId,
    confidence: Confidence,
}

/// One callable member of a decl, read off its procedure.
#[derive(Debug)]
struct Function {
    arities: Vec<u32>,
    abstract_: bool,
    target: Target,
}

/// One Solidity contract or interface (module doc).
#[derive(Debug)]
struct Decl {
    id: NodeId,
    name: String,
    interface: bool,
    functions: BTreeMap<String, Function>,
    events: BTreeMap<String, Target>,
    bases: Vec<NodeId>,
}

/// Every decl of the build, in graph order then decl-node order.
struct Contracts {
    decls: Vec<Decl>,
    /// `decl id -> index into decls`, lookup only.
    by_id: HashMap<u64, usize>,
    /// [`Contracts::is_concrete`] per decl.
    concrete: Vec<bool>,
    /// Distinct evm procedures.
    procedures: usize,
}

/// A CLASS or INTERFACE node of `g`.
fn is_decl_kind(g: &RepoGraph, id: NodeId) -> bool {
    matches!(g.nav.kind_by_id.get(&id), Some(&k) if k == node_kind::CLASS || k == node_kind::INTERFACE)
}

/// The evm facts of one contract's copy of a procedure: the evm CODE cell
/// after `file`'s POSITION cell, else the first one (module doc).
fn facts_for(cells: &[Cell], file: Option<&str>) -> Option<ProcedureFacts> {
    if let Some(file) = file {
        let mut here = false;
        for c in cells {
            if c.kind == cell_type::POSITION {
                here = cell_file(c) == Some(file);
            } else if here
                && let Some(facts) = evm::read_procedure_cell(std::slice::from_ref(c))
            {
                return Some(facts);
            }
        }
    }
    evm::read_procedure_cell(cells)
}

/// The decls owning procedure `proc` of contract `contract`, each with the
/// POSITION file of the handler it was found through (module doc).
fn proc_owners(
    g: &RepoGraph,
    proc: NodeId,
    contract: &str,
    handlers: &HashMap<u64, Vec<NodeId>>,
    at: &HashMap<u64, usize>,
) -> Vec<(NodeId, Option<String>)> {
    let named = |decl: NodeId| {
        is_decl_kind(g, decl) && g.nav.name_by_id.get(&decl).is_some_and(|n| n == contract)
    };
    let mut out: Vec<(NodeId, Option<String>)> = Vec::new();
    for h in handlers.get(&proc.0).into_iter().flatten() {
        let Some(&decl) = g.nav.parent_of.get(h) else { continue };
        if !named(decl) || out.iter().any(|(d, _)| *d == decl) {
            continue;
        }
        let file = at.get(&h.0).and_then(|&i| node_file(&g.nodes[i])).map(str::to_string);
        out.push((decl, file));
    }
    if out.is_empty()
        && let Some(&decl) = g.nav.parent_of.get(&proc)
        && named(decl)
    {
        out.push((decl, None));
    }
    out
}

/// The transitive bases of `id` (module doc).
fn bases_of(id: NodeId, heritage: &HashMap<u64, Vec<NodeId>>) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut seen: HashSet<u64> = HashSet::from([id.0]);
    let mut frontier = vec![id];
    for _ in 0..MAX_BASE_DEPTH {
        let mut next = Vec::new();
        for f in &frontier {
            for &b in heritage.get(&f.0).into_iter().flatten() {
                if seen.insert(b.0) {
                    out.push(b);
                    next.push(b);
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    out
}

/// A decl's members while the index is gathered.
struct Parts {
    graph: usize,
    functions: BTreeMap<String, Function>,
    events: BTreeMap<String, Target>,
}

fn contract_index(graphs: &[RepoGraph]) -> Contracts {
    let mut parts: HashMap<u64, Parts> = HashMap::new();
    let mut procedures: HashSet<u64> = HashSet::new();
    for (gi, g) in graphs.iter().enumerate() {
        let mut at: HashMap<u64, usize> = HashMap::new();
        for (i, n) in g.nodes.iter().enumerate() {
            at.entry(n.id.0).or_insert(i);
        }
        let mut handlers: HashMap<u64, Vec<NodeId>> = HashMap::new();
        for e in &g.edges {
            if e.category == edge_category::HANDLED_BY
                && g.nav.kind_by_id.get(&e.from) == Some(&node_kind::RPC_PROCEDURE)
            {
                handlers.entry(e.from.0).or_default().push(e.to);
            }
        }
        for n in &g.nodes {
            let Some(&kind) = g.nav.kind_by_id.get(&n.id) else { continue };
            let Some(qname) = g.nav.qname_by_id.get(&n.id) else { continue };
            if kind == node_kind::RPC_PROCEDURE {
                let Some((contract, member)) = evm::split_procedure(qname) else { continue };
                procedures.insert(n.id.0);
                let target = Target { id: n.id, confidence: n.confidence };
                for (decl, file) in proc_owners(g, n.id, contract, &handlers, &at) {
                    let Some(facts) = facts_for(&n.cells, file.as_deref()) else { continue };
                    let Some(p) = decl_parts(&mut parts, decl, gi) else { continue };
                    p.functions.entry(member.to_string()).or_insert(Function {
                        arities: facts.arities,
                        abstract_: facts.abstract_,
                        target,
                    });
                }
            } else if kind == node_kind::EVENT_EMITTER && is_solidity_event(qname, n) {
                let Some(&decl) = g.nav.parent_of.get(&n.id) else { continue };
                let Some(name) = g.nav.name_by_id.get(&n.id) else { continue };
                if !is_decl_kind(g, decl) {
                    continue;
                }
                let Some(p) = decl_parts(&mut parts, decl, gi) else { continue };
                p.events
                    .entry(name.clone())
                    .or_insert(Target { id: n.id, confidence: n.confidence });
            }
        }
    }

    // Decls in graph order then decl-node order; a decl whose node no graph
    // lists (never built that way) follows, in id order.
    let mut decls: Vec<Decl> = Vec::new();
    let mut heritage: HashMap<usize, HashMap<u64, Vec<NodeId>>> = HashMap::new();
    let mut make = |gi: usize, id: NodeId, p: Parts, decls: &mut Vec<Decl>| {
        let g = &graphs[gi];
        let adjacency = heritage.entry(gi).or_insert_with(|| {
            let mut adj: HashMap<u64, Vec<NodeId>> = HashMap::new();
            for e in &g.edges {
                if (e.category == edge_category::INHERITS_FROM || e.category == edge_category::IMPLEMENTS)
                    && is_decl_kind(g, e.from)
                    && is_decl_kind(g, e.to)
                {
                    adj.entry(e.from.0).or_default().push(e.to);
                }
            }
            adj
        });
        decls.push(Decl {
            id,
            name: g.nav.name_by_id.get(&id).cloned().unwrap_or_default(),
            interface: g.nav.kind_by_id.get(&id) == Some(&node_kind::INTERFACE),
            functions: p.functions,
            events: p.events,
            bases: bases_of(id, adjacency),
        });
    };
    for (gi, g) in graphs.iter().enumerate() {
        for n in &g.nodes {
            if parts.get(&n.id.0).is_some_and(|p| p.graph == gi)
                && let Some(p) = parts.remove(&n.id.0)
            {
                make(gi, n.id, p, &mut decls);
            }
        }
    }
    let mut rest: Vec<(u64, Parts)> = parts.into_iter().collect();
    rest.sort_by_key(|(id, _)| *id);
    for (id, p) in rest {
        make(p.graph, NodeId(id), p, &mut decls);
    }

    let by_id = decls.iter().enumerate().map(|(i, d)| (d.id.0, i)).collect();
    let mut contracts = Contracts { decls, by_id, concrete: Vec::new(), procedures: procedures.len() };
    contracts.concrete = (0..contracts.decls.len()).map(|d| contracts.is_concrete(d)).collect();
    contracts
}

/// The gathering slot of `decl`, created in graph `gi`; `None` when an
/// earlier graph holds the decl (a decl id held by two graphs keeps the
/// first graph's members).
fn decl_parts(parts: &mut HashMap<u64, Parts>, decl: NodeId, gi: usize) -> Option<&mut Parts> {
    let p = parts.entry(decl.0).or_insert_with(|| Parts {
        graph: gi,
        functions: BTreeMap::new(),
        events: BTreeMap::new(),
    });
    (p.graph == gi).then_some(p)
}

impl Contracts {
    /// The indexed decls among `d`'s bases, in base order.
    fn base_decls(&self, d: usize) -> impl Iterator<Item = &Decl> {
        self.decls[d].bases.iter().filter_map(|b| self.by_id.get(&b.0).map(|&i| &self.decls[i]))
    }

    /// `d`'s visible definitions of function `name`, ranked (module doc).
    fn definitions(&self, d: usize, name: &str) -> Vec<&Function> {
        let mut out: Vec<&Function> = self.decls[d].functions.get(name).into_iter().collect();
        let inherited: Vec<&Function> = self.base_decls(d).filter_map(|b| b.functions.get(name)).collect();
        out.extend(inherited.iter().copied().filter(|f| !f.abstract_));
        out.extend(inherited.iter().copied().filter(|f| f.abstract_));
        out
    }

    /// The event `name` `d` or a base declares, own first.
    fn event(&self, d: usize, name: &str) -> Option<Target> {
        self.decls[d]
            .events
            .get(name)
            .copied()
            .or_else(|| self.base_decls(d).find_map(|b| b.events.get(name).copied()))
    }

    /// Not an interface, and every visible name's first-ranked definition
    /// has a body.
    fn is_concrete(&self, d: usize) -> bool {
        if self.decls[d].interface {
            return false;
        }
        let mut names: Vec<&str> = self.decls[d].functions.keys().map(String::as_str).collect();
        for b in self.base_decls(d) {
            names.extend(b.functions.keys().map(String::as_str));
        }
        names
            .into_iter()
            .all(|n| self.definitions(d, n).first().is_none_or(|f| !f.abstract_))
    }

    /// Does `d`'s visible ABI hold `entry`?
    fn holds(&self, d: usize, entry: &AbiEntry) -> bool {
        match entry.kind {
            EntryKind::Fn => self
                .definitions(d, &entry.name)
                .iter()
                .any(|f| entry.arity.is_none_or(|a| f.arities.contains(&a))),
            EntryKind::Event => self.event(d, &entry.name).is_some(),
        }
    }

    /// `decls` with the non-concrete ones dropped when a concrete remains.
    fn prefer_concrete(&self, mut decls: Vec<usize>) -> Vec<usize> {
        if decls.iter().any(|&d| self.concrete[d]) {
            decls.retain(|&d| self.concrete[d]);
        }
        decls
    }
}

// ---- clients -----------------------------------------------------------------

/// One client node, its cells unioned over every graph holding its id.
struct Client {
    id: NodeId,
    confidence: Confidence,
    side: ClientSide,
    scope: Scope,
    member: String,
    facts: Vec<ClientFacts>,
}

/// Every evm client: RPC_CALLs reading as calls, EVENT_HANDLERs reading as
/// subscriptions, in graph order then node order, one per id.
fn clients(graphs: &[RepoGraph]) -> Vec<Client> {
    let mut out: Vec<Client> = Vec::new();
    let mut at: HashMap<u64, usize> = HashMap::new();
    for g in graphs {
        for n in &g.nodes {
            let want = match g.nav.kind_by_id.get(&n.id) {
                Some(&k) if k == node_kind::RPC_CALL => ClientSide::Call,
                Some(&k) if k == node_kind::EVENT_HANDLER => ClientSide::Event,
                _ => continue,
            };
            let Some(qname) = g.nav.qname_by_id.get(&n.id) else { continue };
            let Some((side, scope, member)) = evm::split_client(split_owner(qname).0) else { continue };
            if side != want {
                continue;
            }
            let facts = evm::read_client_cells(&n.cells);
            match at.get(&n.id.0) {
                Some(&i) => out[i].facts.extend(facts),
                None => {
                    at.insert(n.id.0, out.len());
                    out.push(Client {
                        id: n.id,
                        confidence: n.confidence,
                        side,
                        scope,
                        member: member.to_string(),
                        facts,
                    });
                }
            }
        }
    }
    out
}

/// `(lib, args)` of one client cell.
type SiteArgs = (String, Vec<u32>);

/// A cell's `abi` keys, sorted; `None` when the sites' ABI is unknown.
type AbiKeys = Option<Vec<String>>;

/// One ABI a client's sites were built from.
struct Variant {
    /// The ABI's entries; `None` when the sites' ABI is unknown.
    entries: Option<Vec<AbiEntry>>,
    /// `(lib, args)` of each cell carrying this ABI.
    sites: Vec<SiteArgs>,
}

impl Variant {
    fn entries(&self) -> &[AbiEntry] {
        self.entries.as_deref().unwrap_or(&[])
    }

    /// The entries of `kind` named `name`.
    fn listed<'a>(&'a self, kind: EntryKind, name: &'a str) -> impl Iterator<Item = &'a AbiEntry> {
        self.entries().iter().filter(move |e| e.kind == kind && e.name == name)
    }

    /// The ABI lists entries and none of them is `kind` `name`.
    fn omits(&self, kind: EntryKind, name: &str) -> bool {
        !self.entries().is_empty() && self.listed(kind, name).next().is_none()
    }

    /// Does definition `f` of `member` fit this variant (module doc)?
    fn fits(&self, member: &str, f: &Function) -> bool {
        let known: Vec<u32> = self.listed(EntryKind::Fn, member).filter_map(|e| e.arity).collect();
        if !known.is_empty() {
            return known.iter().any(|a| f.arities.contains(a));
        }
        self.sites.iter().all(|(lib, args)| {
            let overrides = OVERRIDES_LIBS.contains(&lib.as_str());
            args.iter().all(|&n| {
                f.arities.contains(&n) || (overrides && n >= 1 && f.arities.contains(&(n - 1)))
            })
        })
    }
}

/// A client's variants, in `abi` order ("no ABI" first).
fn variants(facts: &[ClientFacts]) -> Vec<Variant> {
    let mut by_abi: BTreeMap<AbiKeys, Vec<SiteArgs>> = BTreeMap::new();
    for f in facts {
        let key = f.abi.as_ref().map(|abi| {
            let mut abi = abi.clone();
            abi.sort();
            abi.dedup();
            abi
        });
        by_abi.entry(key).or_default().push((f.lib.clone(), f.args.clone()));
    }
    if by_abi.is_empty() {
        by_abi.insert(None, Vec::new());
    }
    by_abi
        .into_iter()
        .map(|(abi, sites)| Variant {
            entries: abi.map(|keys| keys.iter().filter_map(|k| AbiEntry::from_key(k)).collect()),
            sites,
        })
        .collect()
}

// ---- identify, agree, pair ------------------------------------------------------

/// What one variant identified.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Identified {
    Decls { decls: Vec<usize>, rule: &'static str },
    Unmatched,
    Ambiguous,
}

/// Identify the decls one variant of a client reaches (module doc).
fn identify(scope: &Scope, variant: &Variant, c: &Contracts) -> Identified {
    let entries = variant.entries();
    let all = 0..c.decls.len();
    let one = |decls: Vec<usize>, rule: &'static str| match decls.len() {
        0 => Identified::Unmatched,
        1 => Identified::Decls { decls, rule },
        _ => Identified::Ambiguous,
    };
    match scope {
        Scope::Contract(name) => {
            let decls: Vec<usize> = all.filter(|&d| c.decls[d].name == *name).collect();
            if decls.is_empty() {
                Identified::Unmatched
            } else {
                Identified::Decls { decls, rule: "contract" }
            }
        }
        Scope::Abi(name) if entries.len() >= 2 => {
            let covering: Vec<usize> = all.filter(|&d| entries.iter().all(|e| c.holds(d, e))).collect();
            let covering = c.prefer_concrete(covering);
            if covering.len() < 2 {
                return one(covering, "abi");
            }
            let key = evm::name_key(name);
            if key.len() < NAME_MIN {
                return Identified::Ambiguous;
            }
            let named: Vec<usize> =
                covering.into_iter().filter(|&d| evm::name_key(&c.decls[d].name) == key).collect();
            if named.len() == 1 {
                Identified::Decls { decls: named, rule: "abi_hint" }
            } else {
                Identified::Ambiguous
            }
        }
        Scope::Abi(name) | Scope::Recv(name) => {
            let key = evm::name_key(name);
            if key.len() < NAME_MIN {
                return Identified::Unmatched;
            }
            let named: Vec<usize> = all
                .filter(|&d| evm::name_key(&c.decls[d].name) == key)
                .filter(|&d| entries.iter().all(|e| c.holds(d, e)))
                .collect();
            one(c.prefer_concrete(named), "name_hint")
        }
    }
}

/// A client's joint identification.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Agreed {
    Pair { decls: Vec<usize>, rule: &'static str },
    Unmatched,
    Ambiguous,
    Conflict,
}

/// Join the variants' results; `results` holds `(ABI of two or more
/// entries, what it identified)` per variant (module doc).
fn agree(results: &[(bool, Identified)]) -> Agreed {
    let mut found: Option<(&Vec<usize>, &'static str)> = None;
    for (_, r) in results {
        if let Identified::Decls { decls, rule } = r {
            match found {
                None => found = Some((decls, rule)),
                Some((seen, _)) if seen != decls => return Agreed::Conflict,
                Some((seen, best)) => {
                    let rank = |r: &str| RULES.iter().position(|x| *x == r).unwrap_or(RULES.len());
                    if rank(rule) < rank(best) {
                        found = Some((seen, rule));
                    }
                }
            }
        }
    }
    let Some((decls, rule)) = found else {
        return if results.iter().any(|(_, r)| *r == Identified::Ambiguous) {
            Agreed::Ambiguous
        } else {
            Agreed::Unmatched
        };
    };
    if results.iter().any(|(abi, r)| *abi && !matches!(r, Identified::Decls { .. })) {
        return Agreed::Conflict;
    }
    Agreed::Pair { decls: decls.clone(), rule }
}

/// Why an identified decl gave a client no edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Miss {
    NotDeclared,
    ArityMismatch,
    NotInAbi,
}

/// The target of `client` in decl `d` (module doc).
fn pair(c: &Contracts, client: &Client, d: usize, variants: &[Variant]) -> Result<Target, Miss> {
    let member = client.member.as_str();
    match client.side {
        ClientSide::Event => {
            let target = c.event(d, member).ok_or(Miss::NotDeclared)?;
            if variants.iter().any(|v| v.omits(EntryKind::Event, member)) {
                return Err(Miss::NotInAbi);
            }
            Ok(target)
        }
        ClientSide::Call => {
            let defs = c.definitions(d, member);
            if defs.is_empty() {
                return Err(Miss::NotDeclared);
            }
            if variants.iter().any(|v| v.omits(EntryKind::Fn, member)) {
                return Err(Miss::NotInAbi);
            }
            defs.into_iter()
                .find(|f| variants.iter().all(|v| v.fits(member, f)))
                .map(|f| f.target)
                .ok_or(Miss::ArityMismatch)
        }
    }
}

// ---- resolve -------------------------------------------------------------------

/// The `[evm-link]` counts (module doc).
#[derive(Debug, Default, PartialEq, Eq)]
struct Tally {
    calls: usize,
    events: usize,
    paired: usize,
    edges: usize,
    by_rule: [usize; 4],
    unmatched: usize,
    ambiguous: usize,
    conflict: usize,
    not_declared: usize,
    arity_mismatch: usize,
    not_in_abi: usize,
    procedures: usize,
    contracts: usize,
}

impl Tally {
    /// The marker line; `None` with no procedure and no client.
    fn line(&self) -> Option<String> {
        if self.procedures + self.calls + self.events == 0 {
            return None;
        }
        let [a, b, h, k] = self.by_rule;
        Some(format!(
            "[evm-link] calls={} events={} paired={} edges={} (contract={a} abi={b} abi_hint={h} name_hint={k}) \
             unmatched={} ambiguous={} conflict={} not_declared={} arity_mismatch={} not_in_abi={} \
             procedures={} contracts={}",
            self.calls,
            self.events,
            self.paired,
            self.edges,
            self.unmatched,
            self.ambiguous,
            self.conflict,
            self.not_declared,
            self.arity_mismatch,
            self.not_in_abi,
            self.procedures,
            self.contracts,
        ))
    }
}

/// One resolve's result, before it is printed and added.
struct Linked {
    edges: Vec<Edge>,
    tally: Tally,
    rules: RuleTally,
}

fn link(graphs: &[RepoGraph]) -> Linked {
    let contracts = contract_index(graphs);
    let mut tally = Tally {
        procedures: contracts.procedures,
        contracts: contracts.decls.len(),
        ..Tally::default()
    };
    let mut rules = RuleTally::new("evm", &RULES);
    let mut seen: HashSet<(u64, u64)> = HashSet::new();
    let mut edges = Vec::new();
    for client in clients(graphs) {
        match client.side {
            ClientSide::Call => tally.calls += 1,
            ClientSide::Event => tally.events += 1,
        }
        let variants = variants(&client.facts);
        let results: Vec<(bool, Identified)> = variants
            .iter()
            .map(|v| (v.entries().len() >= 2, identify(&client.scope, v, &contracts)))
            .collect();
        let (decls, rule) = match agree(&results) {
            Agreed::Pair { decls, rule } => (decls, rule),
            Agreed::Unmatched => {
                tally.unmatched += 1;
                continue;
            }
            Agreed::Ambiguous => {
                tally.ambiguous += 1;
                continue;
            }
            Agreed::Conflict => {
                tally.conflict += 1;
                continue;
            }
        };
        let (mut paired, mut misses) = (false, [false; 3]);
        for d in decls {
            let target = match pair(&contracts, &client, d, &variants) {
                Ok(t) => t,
                Err(m) => {
                    misses[m as usize] = true;
                    continue;
                }
            };
            paired = true;
            let (from, to, category) = match client.side {
                ClientSide::Call => (client.id, target.id, edge_category::RPC_CALLS),
                ClientSide::Event => (target.id, client.id, edge_category::EVENT_FLOWS),
            };
            if !seen.insert((from.0, to.0)) {
                continue;
            }
            let confidence = if matches!(rule, "abi_hint" | "name_hint") {
                Confidence::Weak
            } else {
                weakest(client.confidence, target.confidence)
            };
            edges.push(Edge::new(from, to, category, confidence).with_cell(rules.cell(rule)));
            if let Some(i) = RULES.iter().position(|r| *r == rule) {
                tally.by_rule[i] += 1;
            }
        }
        tally.paired += usize::from(paired);
        tally.not_declared += usize::from(misses[Miss::NotDeclared as usize]);
        tally.arity_mismatch += usize::from(misses[Miss::ArityMismatch as usize]);
        tally.not_in_abi += usize::from(misses[Miss::NotInAbi as usize]);
    }
    tally.edges = edges.len();
    Linked { edges, tally, rules }
}

impl CrossGraphResolver for EvmContractResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let Linked { edges, tally, rules } = link(&merged.graphs);
        rules.report();
        if let Some(line) = tally.line() {
            eprintln!("{line}");
        }
        merged.cross_edges.extend(edges);
    }
}

#[cfg(test)]
mod tests {
    use super::super::eventbus::EventBusResolver;
    use super::super::tests::channel_graph;
    use super::*;
    use glia_code_domain::GRAPH_TYPE;
    use glia_code_domain::evidence::Evidence;
    use glia_core::{EdgeCategoryId, NodeKindId};

    use Confidence::{Medium, Strong, Weak};

    fn position(file: &str) -> Cell {
        Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(format!(r#"{{"file":"{file}","start_line":0,"end_line":1}}"#)),
        }
    }

    /// Record `(kind, qname)` in `g`, nav name `name`. A second record of one
    /// id appends its cells to the first node, as merge_parses does, and its
    /// nav record overwrites the first (the last parent wins).
    fn add(
        g: &mut RepoGraph,
        kind: NodeKindId,
        qname: &str,
        name: &str,
        parent: Option<NodeId>,
        confidence: Confidence,
        cells: Vec<Cell>,
    ) -> NodeId {
        let id = NodeId::from_parts(GRAPH_TYPE, g.repo, kind, qname);
        g.nav.record(id, name, qname, kind, parent);
        match g.nodes.iter_mut().find(|n| n.id == id) {
            Some(n) => n.cells.extend(cells),
            None => g.nodes.push(Node { id, repo: g.repo, confidence, cells }),
        }
        id
    }

    /// One repo's Solidity graph, shaped the way the parser (DT.10a) and
    /// merge_parses leave it.
    struct Sol {
        g: RepoGraph,
    }

    impl Sol {
        fn new(tag: &str) -> Self {
            Sol { g: channel_graph(tag, &[]) }
        }

        /// `contract <name>` (CLASS) or `interface <name>` (INTERFACE) in
        /// `file`, under the file's MODULE.
        fn decl(&mut self, file: &str, name: &str, kind: NodeKindId) -> NodeId {
            let module = file.trim_end_matches(".sol").replace('/', "::");
            let mid = NodeId::from_parts(GRAPH_TYPE, self.g.repo, node_kind::MODULE, &module);
            if !self.g.nav.qname_by_id.contains_key(&mid) {
                add(&mut self.g, node_kind::MODULE, &module, &module, None, Strong, vec![position(file)]);
            }
            add(&mut self.g, kind, &format!("{module}::{name}"), name, Some(mid), Strong, vec![position(file)])
        }

        fn contract(&mut self, file: &str, name: &str) -> NodeId {
            self.decl(file, name, node_kind::CLASS)
        }

        fn interface(&mut self, file: &str, name: &str) -> NodeId {
            self.decl(file, name, node_kind::INTERFACE)
        }

        fn qname(&self, id: NodeId) -> String {
            self.g.nav.qname_by_id.get(&id).cloned().unwrap_or_default()
        }

        fn name(&self, id: NodeId) -> String {
            self.g.nav.name_by_id.get(&id).cloned().unwrap_or_default()
        }

        /// A callable member of `decl` declared in `file`: its METHOD, and the
        /// procedure `evm:<C>.<member>` HANDLED_BY it.
        fn function(&mut self, file: &str, decl: NodeId, member: &str, arities: &[u32], abstract_: bool) -> NodeId {
            let (contract, dq) = (self.name(decl), self.qname(decl));
            let method =
                add(&mut self.g, node_kind::METHOD, &format!("{dq}::{member}"), member, Some(decl), Strong, vec![position(file)]);
            let facts = ProcedureFacts {
                arities: arities.to_vec(),
                mutability: evm::NONPAYABLE.into(),
                getter: false,
                abstract_,
            };
            let proc = add(
                &mut self.g,
                node_kind::RPC_PROCEDURE,
                &evm::procedure_qname(&contract, member),
                &format!("{contract}.{member}"),
                Some(decl),
                Strong,
                vec![position(file), evm::procedure_cell(&facts)],
            );
            self.g.edges.push(Edge::new(proc, method, edge_category::HANDLED_BY, Strong));
            proc
        }

        /// `function <member>` with a body, in the decl's own file.
        fn f(&mut self, decl: NodeId, member: &str, arities: &[u32]) -> NodeId {
            let file = self.file_of(decl);
            self.function(&file, decl, member, arities, false)
        }

        /// An interface member / a function without a body.
        fn abstract_f(&mut self, decl: NodeId, member: &str, arities: &[u32]) -> NodeId {
            let file = self.file_of(decl);
            self.function(&file, decl, member, arities, true)
        }

        fn file_of(&self, decl: NodeId) -> String {
            self.g
                .nodes
                .iter()
                .find(|n| n.id == decl)
                .and_then(node_file)
                .unwrap_or_default()
                .to_string()
        }

        /// `event <name>` of `decl`: an EVENT_EMITTER with a code qname.
        fn event(&mut self, decl: NodeId, name: &str) -> NodeId {
            let (file, dq) = (self.file_of(decl), self.qname(decl));
            add(&mut self.g, node_kind::EVENT_EMITTER, &format!("{dq}::{name}"), name, Some(decl), Strong, vec![position(&file)])
        }

        /// `from is to`.
        fn base(&mut self, from: NodeId, to: NodeId, category: EdgeCategoryId) {
            self.g.edges.push(Edge::new(from, to, category, Strong));
        }
    }

    /// One client site group: `(lib, args, abi keys)`.
    type Site<'a> = (&'a str, &'a [u32], Option<&'a [&'a str]>);

    fn cell(site: &Site<'_>) -> Cell {
        let (lib, args, abi) = *site;
        evm::client_cell(&ClientFacts {
            lib: lib.into(),
            args: args.to_vec(),
            abi: abi.map(|a| a.iter().map(|k| k.to_string()).collect()),
        })
    }

    /// A contract call `evm_call:<scope>.<member>` in `g`, Medium, with one
    /// cell per site group.
    fn call(g: &mut RepoGraph, scope: &Scope, member: &str, sites: &[Site<'_>]) -> NodeId {
        let qname = evm::call_qname(scope, member);
        let name = format!("{}.{member}", scope.name());
        add(g, node_kind::RPC_CALL, &qname, &name, None, Medium, sites.iter().map(cell).collect())
    }

    /// An event subscription `evm_event:<scope>.<event>` in `g`, Medium.
    fn subscribe(g: &mut RepoGraph, scope: &Scope, event: &str, sites: &[Site<'_>]) -> NodeId {
        let qname = evm::event_qname(scope, event);
        let name = format!("{}.{event}", scope.name());
        add(g, node_kind::EVENT_HANDLER, &qname, &name, None, Medium, sites.iter().map(cell).collect())
    }

    fn abi(n: &str) -> Scope {
        Scope::Abi(n.into())
    }

    fn named(n: &str) -> Scope {
        Scope::Contract(n.into())
    }

    fn recv(n: &str) -> Scope {
        Scope::Recv(n.into())
    }

    /// `(from qname, to qname, category, rule, confidence)` of every edge,
    /// sorted.
    type Row = (String, String, &'static str, String, Confidence);

    /// Link `graphs`; the rows and the tally.
    fn run(graphs: &[RepoGraph]) -> (Vec<Row>, Tally) {
        let linked = link(graphs);
        let q = |id: NodeId| graphs.iter().find_map(|g| g.nav.qname_by_id.get(&id).cloned()).unwrap_or_default();
        let mut rows: Vec<Row> = linked
            .edges
            .iter()
            .map(|e| {
                let ev = Evidence::read(&e.cells).expect("every evm edge carries EVIDENCE");
                assert_eq!(ev.emitter, "resolver:evm");
                (q(e.from), q(e.to), edge_category::name(e.category), ev.rule.unwrap_or_default(), e.confidence)
            })
            .collect();
        rows.sort_by(|a, b| (&a.0, &a.1, a.2).cmp(&(&b.0, &b.1, b.2)));
        (rows, linked.tally)
    }

    fn row(from: &str, to: &str, category: &'static str, rule: &str, c: Confidence) -> Row {
        (from.into(), to.into(), category, rule.into(), c)
    }

    /// The Escrow / Vault pair most tests build: Escrow {deposit/1,
    /// release/1}, Vault {release/1, withdraw/1}.
    fn escrow_and_vault() -> Sol {
        let mut s = Sol::new("evm-sol");
        let escrow = s.contract("contracts/Escrow.sol", "Escrow");
        s.f(escrow, "deposit", &[1]);
        s.f(escrow, "release", &[1]);
        let vault = s.contract("contracts/Vault.sol", "Vault");
        s.f(vault, "release", &[1]);
        s.f(vault, "withdraw", &[1]);
        s
    }

    const ESCROW_ABI: &[&str] = &["f:deposit/1", "f:release/1"];

    #[test]
    fn abi_set_picks_the_one_covering_contract() {
        let s = escrow_and_vault();
        let mut web = channel_graph("evm-web", &[]);
        call(&mut web, &abi("ESCROW_ABI"), "release", &[("ethers", &[1], Some(ESCROW_ABI))]);
        call(&mut web, &abi("ESCROW_ABI"), "deposit", &[("ethers", &[1], Some(ESCROW_ABI))]);
        let (rows, tally) = run(&[s.g, web]);
        assert_eq!(
            rows,
            [
                row("evm_call:abi:ESCROW_ABI.deposit", "evm:Escrow.deposit", "RPC_CALLS", "abi", Medium),
                row("evm_call:abi:ESCROW_ABI.release", "evm:Escrow.release", "RPC_CALLS", "abi", Medium),
            ],
            "Vault also declares release, but only Escrow covers the ABI"
        );
        assert_eq!(
            tally.line().as_deref(),
            Some(
                "[evm-link] calls=2 events=0 paired=2 edges=2 (contract=0 abi=2 abi_hint=0 name_hint=0) \
                 unmatched=0 ambiguous=0 conflict=0 not_declared=0 arity_mismatch=0 not_in_abi=0 \
                 procedures=4 contracts=2"
            )
        );
    }

    #[test]
    fn one_entry_abi_is_never_derived() {
        let s = escrow_and_vault();
        let mut web = channel_graph("evm-web", &[]);
        call(&mut web, &abi("ESCROW_ABI"), "release", &[("ethers", &[1], Some(&["f:release/1"]))]);
        // Vault and Escrow both hold `release`: a name alone identifies
        // nothing, and TOKEN_ABI names no contract.
        call(&mut web, &abi("TOKEN_ABI"), "release", &[("ethers", &[1], Some(&["f:release/1"]))]);
        let (rows, tally) = run(&[s.g, web]);
        assert_eq!(
            rows,
            [row("evm_call:abi:ESCROW_ABI.release", "evm:Escrow.release", "RPC_CALLS", "name_hint", Weak)]
        );
        assert_eq!((tally.unmatched, tally.by_rule), (1, [0, 0, 0, 1]));
    }

    /// Kina's ERC20_ABI against `contract MockERC20 is ERC20`: the members
    /// OpenZeppelin's ERC20 gives it are outside the build, so the mock does
    /// not cover the ABI, and the name never stands in for the ABI.
    #[test]
    fn strict_containment_ignores_out_of_repo_bases() {
        let mut s = Sol::new("evm-sol");
        let mock = s.contract("contracts/MockToken.sol", "MockToken");
        s.f(mock, "decimals", &[0]);
        s.f(mock, "mint", &[2]);
        let mut web = channel_graph("evm-web", &[]);
        let erc20: &[&str] = &["f:balanceOf/1", "f:decimals/0"];
        call(&mut web, &abi("ERC20_ABI"), "balanceOf", &[("ethers", &[1], Some(erc20))]);
        call(&mut web, &abi("ERC20_ABI"), "decimals", &[("ethers", &[0], Some(erc20))]);
        call(&mut web, &abi("MOCK_TOKEN_ABI"), "decimals", &[("ethers", &[0], Some(erc20))]);
        let (rows, tally) = run(&[s.g, web]);
        assert_eq!(rows, []);
        assert_eq!((tally.calls, tally.unmatched, tally.paired), (3, 3, 0));
    }

    #[test]
    fn interface_is_dropped_for_its_implementation() {
        let mut s = Sol::new("evm-sol");
        let iface = s.interface("contracts/IEscrow.sol", "IEscrow");
        s.abstract_f(iface, "deposit", &[1]);
        s.abstract_f(iface, "release", &[1]);
        let only_iface = s.g.clone();
        let escrow = s.contract("contracts/Escrow.sol", "Escrow");
        s.f(escrow, "deposit", &[1]);
        s.f(escrow, "release", &[1]);
        s.base(escrow, iface, edge_category::IMPLEMENTS);
        let mut web = channel_graph("evm-web", &[]);
        call(&mut web, &abi("SHOP_ABI"), "release", &[("ethers", &[1], Some(ESCROW_ABI))]);
        let (rows, _) = run(&[s.g, web.clone()]);
        assert_eq!(rows, [row("evm_call:abi:SHOP_ABI.release", "evm:Escrow.release", "RPC_CALLS", "abi", Medium)]);
        // With no implementation in the build the interface is what there is.
        let (rows, _) = run(&[only_iface, web]);
        assert_eq!(rows, [row("evm_call:abi:SHOP_ABI.release", "evm:IEscrow.release", "RPC_CALLS", "abi", Medium)]);
    }

    /// An abstract base covers the ABI as well as its concrete child; the
    /// child is the contract a client is deployed against.
    #[test]
    fn abstract_contract_is_dropped_for_its_concrete_child() {
        let mut s = Sol::new("evm-sol");
        let base = s.contract("contracts/Escrow.sol", "Escrow");
        s.f(base, "deposit", &[1]);
        s.abstract_f(base, "release", &[1]);
        let v2 = s.contract("contracts/EscrowV2.sol", "EscrowV2");
        s.f(v2, "release", &[1]);
        s.base(v2, base, edge_category::INHERITS_FROM);
        let mut web = channel_graph("evm-web", &[]);
        call(&mut web, &abi("ESCROW_ABI"), "release", &[("ethers", &[1], Some(ESCROW_ABI))]);
        call(&mut web, &abi("ESCROW_ABI"), "deposit", &[("ethers", &[1], Some(ESCROW_ABI))]);
        let (rows, _) = run(&[s.g, web]);
        assert_eq!(
            rows,
            [
                row("evm_call:abi:ESCROW_ABI.deposit", "evm:Escrow.deposit", "RPC_CALLS", "abi", Medium),
                row("evm_call:abi:ESCROW_ABI.release", "evm:EscrowV2.release", "RPC_CALLS", "abi", Medium),
            ],
            "the name key `escrow` would have picked the abstract Escrow"
        );
    }

    #[test]
    fn inherited_member_targets_the_base_procedure() {
        let mut s = Sol::new("evm-sol");
        let base = s.contract("contracts/Base.sol", "Base");
        s.f(base, "release", &[1]);
        let escrow = s.contract("contracts/Escrow.sol", "Escrow");
        s.f(escrow, "deposit", &[1]);
        s.base(escrow, base, edge_category::INHERITS_FROM);
        let mut web = channel_graph("evm-web", &[]);
        call(&mut web, &abi("ESCROW_ABI"), "release", &[("ethers", &[1], Some(ESCROW_ABI))]);
        call(&mut web, &abi("ESCROW_ABI"), "deposit", &[("ethers", &[1], Some(ESCROW_ABI))]);
        let (rows, _) = run(&[s.g, web]);
        assert_eq!(
            rows,
            [
                row("evm_call:abi:ESCROW_ABI.deposit", "evm:Escrow.deposit", "RPC_CALLS", "abi", Medium),
                row("evm_call:abi:ESCROW_ABI.release", "evm:Base.release", "RPC_CALLS", "abi", Medium),
            ]
        );
    }

    /// C -> B -> A where B holds only internal members (no procedure): B is
    /// not indexed, and A's members still reach C.
    #[test]
    fn base_without_procedures_still_passes_its_bases() {
        let mut s = Sol::new("evm-sol");
        let a = s.contract("contracts/A.sol", "A");
        s.f(a, "ping", &[0]);
        let b = s.contract("contracts/B.sol", "B");
        let c = s.contract("contracts/C.sol", "C");
        s.f(c, "go", &[0]);
        s.base(c, b, edge_category::INHERITS_FROM);
        s.base(b, a, edge_category::INHERITS_FROM);
        let mut web = channel_graph("evm-web", &[]);
        call(&mut web, &named("C"), "ping", &[("hardhat", &[0], None)]);
        let (rows, tally) = run(&[s.g, web]);
        assert_eq!(rows, [row("evm_call:C.ping", "evm:A.ping", "RPC_CALLS", "contract", Medium)]);
        assert_eq!(tally.contracts, 2, "A and C; B has no procedure");
    }

    /// Two files each declare `contract Escrow`: they share every procedure
    /// node (one qname per repo), whose nav parent is the last one recorded.
    /// Each decl keeps its own members and its own file's facts.
    #[test]
    fn same_named_contracts_in_two_files_keep_their_members() {
        let mut s = Sol::new("evm-sol");
        let a = s.contract("shop/Escrow.sol", "Escrow");
        s.f(a, "deposit", &[1]);
        s.f(a, "release", &[1]);
        let b = s.contract("legacy/Escrow.sol", "Escrow");
        s.f(b, "release", &[2]);
        s.f(b, "refund", &[1]);
        let release = NodeId::from_parts(GRAPH_TYPE, s.g.repo, node_kind::RPC_PROCEDURE, "evm:Escrow.release");
        assert_eq!(s.g.nav.parent_of.get(&release), Some(&b), "the shared node's parent is overwritten");

        let contracts = contract_index(std::slice::from_ref(&s.g));
        type Members<'a> = Vec<(&'a str, &'a [u32])>;
        let members: Vec<(NodeId, Members<'_>)> = contracts
            .decls
            .iter()
            .map(|d| (d.id, d.functions.iter().map(|(m, f)| (m.as_str(), f.arities.as_slice())).collect()))
            .collect();
        assert_eq!(
            members,
            [(a, vec![("deposit", &[1][..]), ("release", &[1][..])]), (b, vec![("refund", &[1][..]), ("release", &[2][..])])]
        );
        assert_eq!(contracts.procedures, 3);

        let mut web = channel_graph("evm-web", &[]);
        call(&mut web, &abi("SHOP_ABI"), "release", &[("ethers", &[1], Some(ESCROW_ABI))]);
        let (rows, _) = run(&[s.g, web]);
        assert_eq!(rows, [row("evm_call:abi:SHOP_ABI.release", "evm:Escrow.release", "RPC_CALLS", "abi", Medium)]);
    }

    /// Only a `.sol` code-qname event is indexed: a TypeScript class's
    /// code-qname EVENT_EMITTER is not a contract event. An events-only
    /// contract is indexed through its events.
    #[test]
    fn sol_gate_keeps_non_solidity_events_out() {
        let mut s = Sol::new("evm-sol");
        let auction = s.contract("contracts/Auction.sol", "Auction");
        let bid = s.event(auction, "BidPlaced");
        let mut ts = channel_graph("evm-ts", &[]);
        let shop = add(&mut ts, node_kind::CLASS, "src::shop::Shop", "Shop", None, Strong, vec![position("src/shop.ts")]);
        add(&mut ts, node_kind::EVENT_EMITTER, "src::shop::Shop::Placed", "Placed", Some(shop), Strong, vec![position("src/shop.ts")]);
        let mut web = channel_graph("evm-web", &[]);
        subscribe(&mut web, &named("Auction"), "BidPlaced", &[("ethers", &[], None)]);
        subscribe(&mut web, &named("Shop"), "Placed", &[("ethers", &[], None)]);
        let graphs = [s.g, ts, web];
        let contracts = contract_index(&graphs);
        assert_eq!(contracts.decls.iter().map(|d| d.id).collect::<Vec<_>>(), [auction]);
        assert_eq!(contracts.decls[0].events.get("BidPlaced").map(|t| t.id), Some(bid));
        let (rows, tally) = run(&graphs);
        assert_eq!(
            rows,
            [row(
                "contracts::Auction::Auction::BidPlaced",
                "evm_event:Auction.BidPlaced",
                "EVENT_FLOWS",
                "contract",
                Medium
            )]
        );
        assert_eq!((tally.events, tally.paired, tally.unmatched, tally.contracts), (2, 1, 1, 1));
    }

    /// Kina's hardhat `token.approve` on MockERC20: approve comes from
    /// OpenZeppelin, so the named contract does not declare it.
    #[test]
    fn contract_scope_counts_not_declared() {
        let mut s = Sol::new("evm-sol");
        let mock = s.contract("contracts/MockERC20.sol", "MockERC20");
        s.f(mock, "mint", &[2]);
        let mut web = channel_graph("evm-web", &[]);
        call(&mut web, &named("MockERC20"), "approve", &[("hardhat", &[2], None)]);
        call(&mut web, &named("MockERC20"), "mint", &[("hardhat", &[2], None)]);
        call(&mut web, &named("Missing"), "mint", &[("hardhat", &[2], None)]);
        let (rows, tally) = run(&[s.g, web]);
        assert_eq!(rows, [row("evm_call:MockERC20.mint", "evm:MockERC20.mint", "RPC_CALLS", "contract", Medium)]);
        assert_eq!((tally.paired, tally.not_declared, tally.unmatched), (1, 1, 1));
    }

    #[test]
    fn name_hint_is_heuristic_unique_and_arity_checked() {
        let escrow = || {
            let mut s = Sol::new("evm-sol");
            let e = s.contract("contracts/Escrow.sol", "Escrow");
            s.f(e, "release", &[1]);
            s
        };
        let client = |scope: &Scope, args: &[u32]| {
            let mut web = channel_graph("evm-web", &[]);
            call(&mut web, scope, "release", &[("geth", args, None)]);
            web
        };
        let (rows, _) = run(&[escrow().g, client(&recv("escrow"), &[1])]);
        assert_eq!(rows, [row("evm_call:recv:escrow.release", "evm:Escrow.release", "RPC_CALLS", "name_hint", Weak)]);

        let (rows, tally) = run(&[escrow().g, client(&recv("escrow"), &[3])]);
        assert_eq!((rows.len(), tally.arity_mismatch), (0, 1));

        let mut two = escrow();
        let other = two.contract("contracts/EscrowContract.sol", "EscrowContract");
        two.f(other, "release", &[1]);
        let (rows, tally) = run(&[two.g, client(&recv("escrow"), &[1])]);
        assert_eq!((rows.len(), tally.ambiguous), (0, 1), "escrow and escrowcontract share a name key");

        let (rows, tally) = run(&[escrow().g, client(&recv("c"), &[1])]);
        assert_eq!((rows.len(), tally.unmatched), (0, 1), "a name key under three characters");
    }

    #[test]
    fn abi_hint_breaks_a_covering_tie_heuristically() {
        let mut s = Sol::new("evm-sol");
        for name in ["Escrow", "Vault"] {
            let d = s.contract(&format!("contracts/{name}.sol"), name);
            s.f(d, "deposit", &[1]);
            s.f(d, "release", &[1]);
        }
        let mut web = channel_graph("evm-web", &[]);
        call(&mut web, &abi("ESCROW_ABI"), "release", &[("ethers", &[1], Some(ESCROW_ABI))]);
        call(&mut web, &abi("SHOP_ABI"), "release", &[("ethers", &[1], Some(ESCROW_ABI))]);
        let (rows, tally) = run(&[s.g, web]);
        assert_eq!(rows, [row("evm_call:abi:ESCROW_ABI.release", "evm:Escrow.release", "RPC_CALLS", "abi_hint", Weak)]);
        assert_eq!(tally.ambiguous, 1, "SHOP_ABI names neither");
    }

    #[test]
    fn ethers_overrides_allow_one_extra_argument() {
        let build = |lib: &str| {
            let mut s = Sol::new("evm-sol");
            let e = s.contract("contracts/Escrow.sol", "Escrow");
            s.f(e, "deposit", &[1]);
            let mut web = channel_graph("evm-web", &[]);
            call(&mut web, &named("Escrow"), "deposit", &[(lib, &[1, 2], None)]);
            run(&[s.g, web])
        };
        for lib in ["ethers", "hardhat", "typechain"] {
            let (rows, _) = build(lib);
            assert_eq!(rows.len(), 1, "{lib}: deposit(id, {{ value }})");
        }
        for lib in ["geth", "viem", "web3"] {
            let (rows, tally) = build(lib);
            assert_eq!((rows.len(), tally.arity_mismatch), (0, 1), "{lib}");
        }
        // An ABI that spells the arity decides it; the sites' count is not read.
        let mut s = Sol::new("evm-sol");
        let e = s.contract("contracts/Escrow.sol", "Escrow");
        s.f(e, "deposit", &[1]);
        s.f(e, "release", &[1]);
        let mut web = channel_graph("evm-web", &[]);
        call(&mut web, &named("Escrow"), "deposit", &[("geth", &[4], Some(&["f:deposit/1", "f:release/1"]))]);
        call(&mut web, &named("Escrow"), "release", &[("geth", &[1], Some(&["f:deposit/1", "f:release/2"]))]);
        let (rows, tally) = run(&[s.g, web]);
        assert_eq!(rows, [row("evm_call:Escrow.deposit", "evm:Escrow.deposit", "RPC_CALLS", "contract", Medium)]);
        assert_eq!(tally.arity_mismatch, 1);
    }

    #[test]
    fn events_flow_from_the_identified_contract_only() {
        let mut s = escrow_and_vault();
        let escrow = s.g.nav.qname_by_id.iter().find(|(_, q)| q.as_str() == "contracts::Escrow::Escrow").map(|(k, _)| *k).unwrap();
        let vault = s.g.nav.qname_by_id.iter().find(|(_, q)| q.as_str() == "contracts::Vault::Vault").map(|(k, _)| *k).unwrap();
        s.event(escrow, "Released");
        s.event(escrow, "Deposited");
        s.event(vault, "Released");
        let mut web = channel_graph("evm-web", &[]);
        let ab: &[&str] = &["e:Released/2", "f:deposit/1"];
        subscribe(&mut web, &abi("ESCROW_ABI"), "Released", &[("ethers", &[], Some(ab))]);
        subscribe(&mut web, &abi("ESCROW_ABI"), "Deposited", &[("ethers", &[], Some(ab))]);
        subscribe(&mut web, &abi("ESCROW_ABI"), "Frozen", &[("ethers", &[], Some(&["e:Frozen/1", "f:deposit/1"]))]);
        let (rows, tally) = run(&[s.g, web]);
        assert_eq!(
            rows,
            [row("contracts::Escrow::Escrow::Released", "evm_event:abi:ESCROW_ABI.Released", "EVENT_FLOWS", "abi", Medium)],
            "Vault's Released never flows: Vault does not cover the ABI"
        );
        assert_eq!((tally.events, tally.not_in_abi, tally.unmatched), (3, 1, 1));
    }

    /// One `evm_call:abi:ABI.release` node holds two files' sites, one built
    /// on Escrow's ABI and one on Vault's: no edge, `conflict`.
    #[test]
    fn colliding_variants_pair_nothing() {
        let s = escrow_and_vault();
        let mut web = channel_graph("evm-web", &[]);
        call(
            &mut web,
            &abi("ABI"),
            "release",
            &[
                ("ethers", &[1], Some(&["f:deposit/1", "f:release/1"])),
                ("ethers", &[1], Some(&["f:release/1", "f:withdraw/1"])),
            ],
        );
        let (rows, tally) = run(&[s.g.clone(), web]);
        assert_eq!(rows, []);
        assert_eq!((tally.calls, tally.conflict, tally.paired), (1, 1, 0));

        // A two-entry variant that identifies nothing also blocks the other.
        let mut web = channel_graph("evm-web", &[]);
        call(
            &mut web,
            &abi("ESCROW_ABI"),
            "release",
            &[
                ("ethers", &[1], Some(&["f:deposit/1", "f:release/1"])),
                ("ethers", &[1], Some(&["f:release/1", "f:unknown/0"])),
            ],
        );
        let (rows, tally) = run(&[s.g, web]);
        assert_eq!((rows.len(), tally.conflict), (0, 1));
    }

    /// Kina's two ESCROW_ABI variants (the frontend's and demo_trade.js's)
    /// both identify the contract: one edge, rule `abi`, and a copy of the
    /// node in a second graph of the repo adds its cells, not a client.
    #[test]
    fn agreeing_variants_pair_once() {
        let mut s = escrow_and_vault();
        let escrow = s.g.nav.qname_by_id.iter().find(|(_, q)| q.as_str() == "contracts::Escrow::Escrow").map(|(k, _)| *k).unwrap();
        s.f(escrow, "refund", &[0]);
        let mut web = channel_graph("evm-web", &[]);
        call(
            &mut web,
            &abi("ESCROW_ABI"),
            "release",
            &[
                ("ethers", &[1], Some(ESCROW_ABI)),
                ("ethers", &[2], Some(&["f:deposit/1", "f:refund/0", "f:release/1"])),
                ("geth", &[1], None),
            ],
        );
        let mut copy = channel_graph("evm-web", &[]);
        call(&mut copy, &abi("ESCROW_ABI"), "release", &[("viem", &[1], Some(ESCROW_ABI))]);
        let (rows, tally) = run(&[s.g, web, copy]);
        assert_eq!(rows, [row("evm_call:abi:ESCROW_ABI.release", "evm:Escrow.release", "RPC_CALLS", "abi", Medium)]);
        assert_eq!((tally.calls, tally.paired, tally.edges), (1, 1, 1));
    }

    /// Both resolvers in pipeline order over a Solidity `Released`, an
    /// in-process `event_handle:Released` and a chain subscription: the only
    /// EVENT_FLOWS is the evm one (James's ruling: a Solidity event never
    /// pairs by name).
    #[test]
    fn eventbus_never_pairs_an_evm_subscription() {
        let mut s = Sol::new("evm-sol");
        let escrow = s.contract("contracts/Escrow.sol", "Escrow");
        s.f(escrow, "release", &[1]);
        let released = s.event(escrow, "Released");
        let mut web = channel_graph(
            "evm-web",
            &[(node_kind::EVENT_HANDLER, "event_handle:Released")],
        );
        let sub = subscribe(&mut web, &named("Escrow"), "Released", &[("ethers", &[], None)]);
        let mut merged = MergedGraph::new(vec![s.g, web]);
        merged.run(&EventBusResolver);
        merged.run(&EvmContractResolver);
        let flows: Vec<(NodeId, NodeId, String)> = merged
            .cross_edges
            .iter()
            .filter(|e| e.category == edge_category::EVENT_FLOWS)
            .map(|e| (e.from, e.to, Evidence::read(&e.cells).map(|ev| ev.emitter).unwrap_or_default()))
            .collect();
        assert_eq!(flows, [(released, sub, "resolver:evm".to_string())]);
    }

    #[test]
    fn edges_are_identical_across_two_runs() {
        let mut s = escrow_and_vault();
        let escrow = s.g.nav.qname_by_id.iter().find(|(_, q)| q.as_str() == "contracts::Escrow::Escrow").map(|(k, _)| *k).unwrap();
        s.event(escrow, "Released");
        let mut web = channel_graph("evm-web", &[]);
        call(&mut web, &abi("ESCROW_ABI"), "release", &[("ethers", &[1], Some(ESCROW_ABI))]);
        call(&mut web, &abi("ESCROW_ABI"), "deposit", &[("ethers", &[1], Some(ESCROW_ABI))]);
        call(&mut web, &recv("vault"), "withdraw", &[("geth", &[1], None)]);
        call(&mut web, &named("Vault"), "release", &[("hardhat", &[1, 2], None)]);
        subscribe(&mut web, &named("Escrow"), "Released", &[("ethers", &[], None)]);
        let graphs = [s.g, web];
        let first = link(&graphs);
        assert_eq!(first.edges.len(), 5);
        for _ in 0..4 {
            let again = link(&graphs);
            assert_eq!(again.edges, first.edges);
            assert_eq!(again.tally, first.tally);
        }
    }

    #[test]
    fn marker_is_silent_without_procedures_or_clients() {
        assert_eq!(Tally::default().line(), None);
        let (rows, tally) = run(&[channel_graph("evm-empty", &[(node_kind::RPC_CALL, "rpc_call:user.list")])]);
        assert_eq!((rows.len(), tally.line()), (0, None), "a tRPC call is no evm client");
        let s = escrow_and_vault();
        let (_, tally) = run(&[s.g]);
        assert_eq!(
            tally.line().as_deref(),
            Some(
                "[evm-link] calls=0 events=0 paired=0 edges=0 (contract=0 abi=0 abi_hint=0 name_hint=0) \
                 unmatched=0 ambiguous=0 conflict=0 not_declared=0 arity_mismatch=0 not_in_abi=0 \
                 procedures=4 contracts=2"
            )
        );
    }

    /// A getter whose STATE_VAR the noise gate dropped has no HANDLED_BY
    /// (the module CONTAINS it): its decl is the procedure's own nav parent.
    #[test]
    fn contained_getter_falls_back_to_its_nav_parent() {
        let mut s = Sol::new("evm-sol");
        let e = s.contract("contracts/Escrow.sol", "Escrow");
        let facts = ProcedureFacts { arities: vec![0], mutability: "view".into(), getter: true, abstract_: false };
        add(
            &mut s.g,
            node_kind::RPC_PROCEDURE,
            "evm:Escrow.VERSION",
            "Escrow.VERSION",
            Some(e),
            Strong,
            vec![position("contracts/Escrow.sol"), evm::procedure_cell(&facts)],
        );
        let mut web = channel_graph("evm-web", &[]);
        call(&mut web, &named("Escrow"), "VERSION", &[("ethers", &[0], None)]);
        let (rows, _) = run(&[s.g, web]);
        assert_eq!(rows, [row("evm_call:Escrow.VERSION", "evm:Escrow.VERSION", "RPC_CALLS", "contract", Medium)]);
    }
}
