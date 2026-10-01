use std::collections::HashSet;
use std::sync::OnceLock;

use glia_code_domain::{CodeNav, FileParse, GRAPH_TYPE, cell_type, node_kind};
use glia_core::{Cell, CellPayload, Confidence, Node, NodeId, NodeKindId, RepoId};

use crate::anchor::{Anchor, line_of};
use crate::code_guard::LazyGuard;
use crate::marker_swap;
use crate::queues::ConstFoldCounts;

pub struct EventNodes {
    pub nodes: Vec<Node>,
    pub nav: CodeNav,
    /// A5.8: where each node's needle fired (see `crate::anchor`). The
    /// type-keyed needles anchor every site; the string-keyed needles anchor
    /// the one site that minted the node.
    pub anchors: Vec<Anchor>,
}

/// (needle, name_rule, broker_ambiguous, gate).
///
/// `name_rule` ([`NameRule`], CB.3a) says where an occurrence reads its event
/// name. A site names a REAL event or mints nothing: there is no fallback to
/// the needle's verb, which named the API (`event_emit:emit`,
/// `event_emit:Subject.next`, `event_handle:@OnEvent`) and never paired.
///
/// `broker_ambiguous` (A2.9) marks the verbs a message-broker client shares with
/// an in-process bus: `publish(`, `.subscribe(`, `.on(`. In a file that imports
/// a broker client ([`broker_present`]) those needles are skipped outright —
/// the call is broker traffic, and `queues.rs` owns it as QUEUE_* +
/// QUEUE_FLOWS. They used to mint Weak EVENT_* nodes joined by EVENT_FLOWS, so
/// `trace` labelled a Redis/MQTT/NATS hop an in-process event, and NATS/Kafka
/// producers double-emitted. The in-process-only verbs (`.emit(`,
/// `.dispatch(`, `dispatchEvent(`, `@OnEvent(` ...) are never flagged, and a
/// file with no broker signal behaves exactly as before.
///
/// `gate` says how [`find_gated`] judges each occurrence ([`VerbGate`]):
/// `publish(` / `.subscribe(` need a bus or a pub/sub import (LA.29), the
/// DOM / jQuery / store / Node event verbs need an in-process bus as their
/// receiver (LA.39), and the rest count wherever they occur.
const EMITTER_PATTERNS: &[(&str, NameRule, bool, VerbGate)] = &[
    (".emit(", NameRule::Literal, false, VerbGate::Receiver),
    (".dispatch(", NameRule::Literal, false, VerbGate::Receiver),
    ("Subject.next(", NameRule::Literal, false, VerbGate::Open),
    ("EventBridge.putEvents", NameRule::DetailType, false, VerbGate::Open),
    ("eventBridge.putEvents", NameRule::DetailType, false, VerbGate::Open),
    ("publish(", NameRule::Literal, true, VerbGate::Bus),
    (".trigger(", NameRule::Literal, false, VerbGate::Receiver),
    ("dispatchEvent(", NameRule::Literal, false, VerbGate::Receiver),
];

const HANDLER_PATTERNS: &[(&str, NameRule, bool, VerbGate)] = &[
    (".on(", NameRule::Literal, true, VerbGate::Receiver),
    (".addEventListener(", NameRule::Literal, false, VerbGate::Receiver),
    (".subscribe(", NameRule::Literal, true, VerbGate::Bus),
    ("@EventPattern(", NameRule::Literal, false, VerbGate::Open),
    ("@OnEvent(", NameRule::Literal, false, VerbGate::Open),
    // Phoenix LiveView's callback clause, its event literal first (CB.3a).
    // The bare token `handle_event` — a Python / Home Assistant method, a
    // Ruby hook — is no needle.
    ("def handle_event(", NameRule::Literal, false, VerbGate::Open),
    (".addListener(", NameRule::Literal, false, VerbGate::Receiver),
];

/// Where an occurrence of a string-keyed needle reads its event name (CB.3a).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NameRule {
    /// The argument right after the needle: a quoted literal (LA.41), else a
    /// constant reference ([`constant_ref_after`]: `OrderEvents.Created`,
    /// `Events::ORDER_PLACED`, `ORDER_PLACED`), else nothing.
    Literal,
    /// AWS EventBridge `putEvents({ Entries: [{ DetailType: "OrderPlaced" }] })`:
    /// the entry's `DetailType` literal inside the call's argument span
    /// ([`detail_type_in`]), else nothing. The v3 `PutEventsCommand` shape is
    /// not a needle (LA.29 skips `.send(new` in `@aws-sdk/` files).
    DetailType,
}

/// How [`find_gated`] judges one occurrence of a string-keyed needle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VerbGate {
    /// Every occurrence is a site: the decorator needles, `Subject.next`,
    /// `def handle_event(`, `EventBridge.putEvents`.
    Open,
    /// LA.29: `publish(` and `.subscribe(`, the two broker-ambiguous verbs that
    /// name a pub/sub channel. A function named `publish`, an RxJS
    /// `obs.subscribe(...)` and a tokio `tx.subscribe()` all share them with a
    /// real bus; [`judge_verb`] keeps a call on a bus-shaped receiver or in a
    /// file importing an in-process pub/sub library — never a declaration,
    /// never a typed site.
    Bus,
    /// LA.39: `.on(` / `.addListener(` / `.addEventListener(` and their
    /// conjugates `.emit(` / `.dispatch(` / `.trigger(` / `dispatchEvent(`.
    /// DOM elements, jQuery, Leaflet, `process`, streams, sockets, Redux
    /// stores and Angular `@Output` emitters all speak them, so an occurrence
    /// counts only when [`receiver_admits`] finds an in-process bus as its
    /// receiver. No import-only admission: importing `events` does not make
    /// `req.on('data')` a bus.
    Receiver,
}

/// Why a gated occurrence was kept — the `via=` of the `[eventbus] verb-gate`
/// marker line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Via {
    /// The receiver's name is bus-shaped ([`BUS_RECEIVER_SUFFIXES`]).
    Receiver,
    /// LA.29: the file imports an in-process pub/sub library
    /// ([`PUBSUB_IMPORTS`]); `publish(` / `.subscribe(` only.
    Import,
    /// LA.39: the receiver is bound in this file to an emitter constructed
    /// from an emitter library, or to a `new EventTarget()`
    /// ([`emitter_bindings`]).
    Bound,
    /// LA.39: `this` / `self` in a file declaring an emitter subclass
    /// ([`emitter_subclass`]).
    Subclass,
    /// LA.39: `.emit(` in a file importing `@nestjs/microservices` — the
    /// `ClientProxy.emit` conjugate of the `@EventPattern` handler.
    Microservices,
}

impl Via {
    fn label(self) -> &'static str {
        match self {
            Via::Receiver => "receiver",
            Via::Import => "import",
            Via::Bound => "bound",
            Via::Subclass => "subclass",
            Via::Microservices => "microservices",
        }
    }
}

/// Buses keyed by MESSAGE TYPE rather than by a string topic. The captured
/// token is the type name, not a literal, so [`literal_after`]'s
/// quoted-literal rule can never see them: `publisher.publishEvent(new
/// OrderPlacedEvent(id))` matches no string needle at all (`publish(` wants
/// `(` straight after `publish`), and neither does `_mediator.Publish(new
/// OrderPlaced())`.
const TYPE_EMITTER_NEEDLES: &[&str] = &[
    "publishEvent(new ", // Spring ApplicationEventPublisher
    ".publish(new ",     // NestJS CQRS EventBus, MediatR (lowercase)
    ".Publish(new ",     // MediatR (C# casing)
    ".Send(new ",        // MediatR/Mediator request shapes
    AWS_V3_SEND,
];

/// The lowercase mediator send. It is also the AWS SDK v3 command shape
/// (`client.send(new PutItemCommand(..))`), a request to a service client
/// that queues.rs / data_entities.rs own as SQS / SNS / DynamoDB — so
/// [`scan_type_needles`] skips it in a file importing `@aws-sdk/` (LA.29). A
/// lowercase mediator in such a file loses its typed event (accepted, rare);
/// MediatR's `.Send(new ` is a different needle and unaffected.
const AWS_V3_SEND: &str = ".send(new ";

/// A receiver reads as an in-process bus when its name (lowercased, leading
/// `_` / `$` / `@` stripped) ends with one of these: `eventBus`, `this.bus`,
/// `PubSub`, `ActiveSupport::Notifications`, `this.events`, `_mediator`,
/// `eventEmitter`, `eventDispatcher` (LA.39). ONE list for both gates
/// ([`VerbGate::Bus`] and [`VerbGate::Receiver`]), so `dispatcher` admits an
/// `eventDispatcher` receiver for `publish(` as well as for `.dispatch(`.
/// `router.events.subscribe` passes it (accepted: the Ionic `Events` bus
/// shares the name).
const BUS_RECEIVER_SUFFIXES: &[&str] = &[
    "bus",
    "pubsub",
    "emitter",
    "events",
    "mediator",
    "publisher",
    "notifications",
    "dispatcher",
];

/// LA.39: the in-process emitter constructors [`emitter_bindings`] reads
/// (`new` optional, a `pkg.` qualifier allowed, `(` or `<` after the name):
/// Node `new EventEmitter()` / `new EventEmitter<E>()`, eventemitter2
/// `new EventEmitter2()`, DOM `new EventTarget()`, tiny-emitter
/// `new Emitter()`, `mitt()`, nanoevents `createNanoEvents()`, pyee
/// `EventEmitter()` / `AsyncIOEventEmitter()`. A binding counts only in a
/// file importing an emitter library ([`EMITTER_LIBS`]) — Angular's
/// `new EventEmitter<T>()` from `@angular/core` is a component output — except
/// `new EventTarget()`, which is the platform's own emitter.
const EMITTER_CONSTRUCTORS: &[&str] = &[
    "EventEmitter",
    "EventEmitter2",
    "EventTarget",
    "Emitter",
    "mitt",
    "createNanoEvents",
    "AsyncIOEventEmitter",
];

/// LA.39: the in-process emitter libraries, matched as a whole quoted module
/// string in import position ([`imports_emitter_lib`]); pyee is matched by its
/// Python import line.
const EMITTER_LIBS: &[&str] = &[
    "events",
    "node:events",
    "eventemitter2",
    "eventemitter3",
    "mitt",
    "nanoevents",
    "tiny-emitter",
    "@nestjs/event-emitter",
];

/// LA.39: the base classes that make `this` / `self` an in-process bus
/// receiver in a file importing an emitter library: JS
/// `extends EventEmitter` / `EventEmitter2` / `Emitter`, pyee
/// `class X(EventEmitter)` / `(AsyncIOEventEmitter)`. `extends EventTarget`
/// needs no import.
const EMITTER_BASES: &[&str] = &["EventEmitter", "EventEmitter2", "Emitter"];
const PYEE_BASES: &[&str] = &["EventEmitter", "AsyncIOEventEmitter"];

/// LA.39: the module whose `ClientProxy` emit is the emitter side of NestJS
/// microservices' `@EventPattern` handler.
const NEST_MICROSERVICES: &str = "@nestjs/microservices";

/// LB.8b: the string-keyed needles whose event travels over a TRANSPORT (a
/// broker, a network bus) rather than an in-process bus, with the `via` their
/// ORIGIN mark records. `@EventPattern(` is the NestJS microservices handler,
/// fed by a `ClientProxy.emit` in another service over Kafka / RMQ / Redis /
/// NATS / TCP; `putEvents` is AWS EventBridge. The `.emit(` needle is
/// transport too in a file naming [`NEST_MICROSERVICES`] (the ClientProxy
/// conjugate, LA.39's R4 test) — [`transport_via`]. Everything else is an
/// in-process bus, which the EventBusResolver pairs only inside one project.
const TRANSPORT_NEEDLES: &[(&str, &str)] = &[
    ("@EventPattern(", "nestjs-microservices"),
    ("EventBridge.putEvents", "aws-eventbridge"),
    ("eventBridge.putEvents", "aws-eventbridge"),
];

/// The `via` of a `.emit(` in a file naming [`NEST_MICROSERVICES`].
const MICROSERVICES_EMIT_VIA: &str = "nestjs-microservices";

/// The plural collection nouns among [`BUS_RECEIVER_SUFFIXES`]. They name a bus
/// only as a named receiver (`this.events`, `ActiveSupport::Notifications`);
/// a CALL that returns events or notifications is a data fetch —
/// `this.fetchNotifications().subscribe(...)` is an RxJS subscription (3
/// phantom HANDLED_BY in quokka-stack), while `vertx.eventBus()` /
/// `getPublisher()` still return a bus.
const COLLECTION_SUFFIXES: &[&str] = &["events", "notifications"];

/// In-process pub/sub libraries (matched in the lowercased file). A file that
/// names one admits any receiver (`const ps = new PubSub(); ps.publish(..)`)
/// and a bare call (Wisper's `publish('order_placed', self)`).
const PUBSUB_IMPORTS: &[&str] = &[
    "graphql-subscriptions",
    "pubsub-js",
    "@nestjs/cqrs",
    "wisper",
];

/// The word before a bare `publish(` that makes it a function declaration:
/// Python / Ruby / Scala `def`, Elixir `defp`, Rust `fn`, Kotlin `fun`, Swift
/// / Go `func`, JS / PHP `function`.
const DECL_KEYWORDS: &[&str] = &["def", "defp", "fn", "fun", "func", "function"];

/// Words that sit where a C-family return type would but make the `publish(`
/// that follows a call: `return publish(x);`, `await publish(x)`,
/// `if publish(x):`.
const CALL_WORDS: &[&str] = &[
    "return", "await", "yield", "new", "throw", "else", "do", "then", "case", "in", "not", "and",
    "or", "if", "elif", "while", "for", "print", "echo", "puts", "typeof", "unless", "until",
    "when", "go", "defer", "delete", "raise", "assert",
];

/// The only words a JS / TS class member may carry before its name.
const METHOD_MODIFIERS: &[&str] = &[
    "async",
    "static",
    "public",
    "private",
    "protected",
    "override",
    "readonly",
    "abstract",
    "get",
    "set",
    "export",
    "default",
];

/// Handler-side type needles. The `char` bounds the captured token — `>` for
/// the generic forms, `)` for the decorator forms. `None` is Spring's bare
/// `@EventListener`, whose event type is the annotated method's FIRST
/// PARAMETER rather than anything inside the annotation.
const TYPE_HANDLER_NEEDLES: &[(&str, Option<char>)] = &[
    ("INotificationHandler<", Some('>')), // MediatR: type inside <>
    ("IRequestHandler<", Some('>')),
    ("@EventsHandler(", Some(')')), // NestJS CQRS: type inside ()
    ("@EventListener", None),       // Spring: type is the handler's first param
    ("@TransactionalEventListener", None),
];

/// Call sites the QUEUE extractor claims even in a file that carries no broker
/// signal: `nc.publish(` is NATS and `channel.publish(` / `basic_publish(` are
/// RabbitMQ (queues.rs QUEUE_PRODUCER needles; `channel.basic_publish` has an
/// empty gate). Per call site, so it still matters where the file-level
/// [`broker_present`] gate does not fire. Everything else a broker publishes
/// through — MQTT, Redis pub/sub, SQS/SNS, Pub/Sub, Azure SB, Kafka — is
/// handled by that gate (A2.9), which is keyed on the queue rows' own library
/// signals rather than on a second hand-written list.
const QUEUE_OWNED_PUBLISH: &[&str] = &[
    "nc.publish(",
    "channel.publish(",
    "channel.basic_publish(",
    "basic_publish(",
];

/// Push one event node, deduplicated on the event name so a file matched by
/// both the type-keyed and the string-keyed pass emits one node, not two.
/// Returns the node's id whether or not this call minted it (the id is a
/// function of kind + qname), so a later site of the same event can anchor.
#[allow(clippy::too_many_arguments)]
fn push_event_node(
    nodes: &mut Vec<Node>,
    nav: &mut CodeNav,
    seen: &mut std::collections::HashSet<String>,
    event_name: &str,
    kind: NodeKindId,
    prefix: &str,
    confidence: Confidence,
    module_id: NodeId,
    repo: RepoId,
) -> (NodeId, bool) {
    let qname = format!("{prefix}{event_name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, kind, &qname);
    if !seen.insert(event_name.to_string()) {
        return (id, false);
    }
    nodes.push(Node {
        id,
        repo,
        confidence,
        cells: vec![],
    });
    nav.record(id, event_name, &qname, kind, Some(module_id));
    (id, true)
}

/// One side of the bus: what [`emit_event_nodes`] mints, from which needles.
struct EventSide {
    /// `emitter` / `handler`: the A2.9 suppression and CB.3b fold markers.
    label: &'static str,
    /// CJ.1a: the scanner the `[code-guard]` marker names.
    guard_label: &'static str,
    kind: NodeKindId,
    prefix: &'static str,
    /// The type-keyed pass ([`scan_type_needles`] over the side's needles).
    typed: fn(&str) -> Vec<(String, usize)>,
    patterns: &'static [(&'static str, NameRule, bool, VerbGate)],
}

const EMITTER_SIDE: EventSide = EventSide {
    label: "emitter",
    guard_label: "event_emitter",
    kind: node_kind::EVENT_EMITTER,
    prefix: "event_emit:",
    typed: typed_emitters,
    patterns: EMITTER_PATTERNS,
};

const HANDLER_SIDE: EventSide = EventSide {
    label: "handler",
    guard_label: "event_handler",
    kind: node_kind::EVENT_HANDLER,
    prefix: "event_handle:",
    typed: typed_handlers,
    patterns: HANDLER_PATTERNS,
};

fn typed_emitters(source: &str) -> Vec<(String, usize)> {
    scan_type_needles(source, TYPE_EMITTER_NEEDLES.iter().map(|n| (*n, None)))
}

fn typed_handlers(source: &str) -> Vec<(String, usize)> {
    scan_type_needles(source, TYPE_HANDLER_NEEDLES.iter().copied())
}

/// CB.3b: turns a constant path (`OrderEvents.Created`) into the literal it
/// holds, or `None`. The engine closes it over the file's own const table and
/// the repo's; the per-file extractors pass none.
type ConstResolver<'a> = &'a dyn Fn(&str) -> Option<String>;

/// `path` is the file the source came from: in a Rust / Python file it
/// selects the CJ.1a literal / comment guard (`""` = no guard).
pub fn extract_event_emitter_nodes(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
) -> EventNodes {
    emit_event_nodes(
        source,
        path,
        module_id,
        repo,
        &EMITTER_SIDE,
        None,
        &mut ConstFoldCounts::default(),
    )
}

/// `path` as for [`extract_event_emitter_nodes`].
pub fn extract_event_handler_nodes(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
) -> EventNodes {
    emit_event_nodes(
        source,
        path,
        module_id,
        repo,
        &HANDLER_SIDE,
        None,
        &mut ConstFoldCounts::default(),
    )
}

/// CB.3b: both sides of one file's event nodes, re-emitted with a resolver.
pub struct EventFold {
    pub emitters: EventNodes,
    pub handlers: EventNodes,
    /// Constant-keyed sites whose constant resolved to a name (`folded`) or
    /// kept its path (`unresolved`: unbound, ambiguous, a lower-case binding
    /// in another file, or a value the LA.41 name rule rejects).
    pub counts: ConstFoldCounts,
    /// The file the nodes were read from, which [`replace_event_nodes`] hands
    /// [`crate::anchor::attach`] when it re-anchors them.
    pub path: String,
}

/// CB.3b: the file's event nodes, both sides, emitted exactly as
/// [`extract_event_emitter_nodes`] + [`extract_event_handler_nodes`] emit
/// them, except that a site keyed by a constant ([`SiteName::Constant`]) is
/// named by the value `resolve` turns its path into, when the LA.41 name rule
/// ([`is_event_name`]) accepts it. So `emit(OrderEvents.Created, ..)` mints
/// `event_emit:order.created` and pairs with `@OnEvent("order.created")`.
///
/// Resolution needs the whole repo's const table, which is not a function of
/// this file, so the engine calls this AFTER the parse cache (never inside the
/// per-file extractors) and swaps the file's event nodes with
/// [`replace_event_nodes`] only when `counts.folded > 0`.
pub fn extract_event_nodes_with_consts(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
    resolve: &dyn Fn(&str) -> Option<String>,
) -> EventFold {
    let mut counts = ConstFoldCounts::default();
    let emitters =
        emit_event_nodes(source, path, module_id, repo, &EMITTER_SIDE, Some(resolve), &mut counts);
    let handlers =
        emit_event_nodes(source, path, module_id, repo, &HANDLER_SIDE, Some(resolve), &mut counts);
    EventFold {
        emitters,
        handlers,
        counts,
        path: path.to_string(),
    }
}

/// CB.3b: is this node an event SITE this module mints (`event_emit:` /
/// `event_handle:`, before the engine's owner pass qualifies it)? A Solidity
/// `event` declaration is an EVENT_EMITTER under its code qname and is not.
pub fn is_event_site(kind: NodeKindId, qname: &str) -> bool {
    (kind == node_kind::EVENT_EMITTER && qname.starts_with("event_emit:"))
        || (kind == node_kind::EVENT_HANDLER && qname.starts_with("event_handle:"))
}

/// CB.3b: swap every event site of one file's parse ([`is_event_site`]) for
/// `fold`'s, emitters then handlers — the order the per-file pass adds them —
/// through the swap LA.4's queue fold shares ([`marker_swap::swap`]): the old
/// nodes, their module CONTAINS and owner edges, nav entries and every edge /
/// ref naming a gone id (a constant path that resolved) go; the fold's nodes
/// go back in place, carrying their transport marks as the per-file pass sets
/// them, re-anchored (POSITION, then the owner edge or the module CONTAINS
/// fallback, stamped `extractor:anchor` rule `const_fold`), and the IMPORTS
/// cell re-attached when the old nodes carried it. Event nodes carry no edges
/// of their own and no callbacks, so there is nothing to re-bind.
pub fn replace_event_nodes(fp: &mut FileParse, module_id: NodeId, lang: &str, fold: EventFold) {
    let old: HashSet<NodeId> = fp
        .nav
        .kind_by_id
        .iter()
        .filter(|(id, k)| {
            fp.nav
                .qname_by_id
                .get(id)
                .is_some_and(|q| is_event_site(**k, q))
        })
        .map(|(id, _)| *id)
        .collect();
    let EventFold {
        emitters,
        handlers,
        path,
        ..
    } = fold;
    let mut nodes = emitters.nodes;
    nodes.extend(handlers.nodes);
    let mut anchors = emitters.anchors;
    anchors.extend(handlers.anchors);
    let fresh = marker_swap::MarkerNodes {
        nodes,
        edges: Vec::new(),
        navs: vec![emitters.nav, handlers.nav],
        anchors,
    };
    marker_swap::swap(fp, module_id, lang, &old, fresh, &path, |_| {});
}

/// The shared emit loop for both sides. Type-keyed FIRST: it is stronger
/// evidence (a real type name, not a framework tag), so when both passes see
/// the same event the Medium confidence is the one that lands. Then each
/// string-keyed needle's first named site ([`find_gated`]), unless A2.9's
/// broker gate suppresses it, named by [`site_name`].
///
/// CB.3b: `resolve` is `None` on the per-file (cached) path, which is then
/// byte-identical to CB.3a; the engine's post-cache fold passes one, and
/// `counts` tallies the constant-keyed sites it folded or left on their path.
///
/// CJ.1a: in a Rust / Python `path` an occurrence of either pass whose first
/// byte sits in a string literal or comment is no site ([`LazyGuard`]); the
/// post-cache fold reads the same path, so it refuses the same occurrences.
fn emit_event_nodes(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
    side: &EventSide,
    resolve: Option<ConstResolver<'_>>,
    counts: &mut ConstFoldCounts,
) -> EventNodes {
    let mut nodes = Vec::new();
    let mut nav = CodeNav::default();
    let mut anchors = Vec::new();
    let mut seen = HashSet::new();
    let mut guard = LazyGuard::new(path, source);

    for (name, at) in (side.typed)(source) {
        if !guard.admits(at) {
            continue;
        }
        let (id, _) = push_event_node(
            &mut nodes,
            &mut nav,
            &mut seen,
            &name,
            side.kind,
            side.prefix,
            Confidence::Medium,
            module_id,
            repo,
        );
        anchors.push(Anchor { node: id, line: line_of(source, at) });
    }

    let mut ctx = VerbCtx::default();
    for &(pattern, rule, ambiguous, gate) in side.patterns {
        let Some((idx, key)) = find_gated(source, pattern, rule, gate, &mut ctx, &mut guard) else {
            continue;
        };
        if ambiguous && ctx.broker_present(source) {
            suppressed(side.label, pattern, key.raw());
            continue;
        }
        let event_name = site_name(key, side.label, pattern, resolve, counts);

        let (id, minted) = push_event_node(
            &mut nodes,
            &mut nav,
            &mut seen,
            &event_name,
            side.kind,
            side.prefix,
            Confidence::Weak,
            module_id,
            repo,
        );
        if minted {
            anchors.push(Anchor { node: id, line: line_of(source, idx) });
        }
        if let Some(via) = transport_via(pattern, source, &mut ctx) {
            mark_transport(&mut nodes, id, via);
        }
    }

    // The fold stays quiet: each file reports once per parse.
    if resolve.is_none() {
        guard.report(side.guard_label);
    }
    EventNodes { nodes, nav, anchors }
}

/// What a found string-keyed site read ([`find_gated`]): the two
/// [`SiteName`]s that name a site.
#[derive(Debug, PartialEq, Eq)]
enum SiteKey {
    Literal(String),
    /// CB.3a's constant path, which [`site_name`] may fold (CB.3b).
    Constant(String),
}

impl SiteKey {
    /// The name as the site wrote it: the literal, or the constant path.
    fn raw(&self) -> &str {
        match self {
            SiteKey::Literal(s) | SiteKey::Constant(s) => s,
        }
    }
}

/// CB.3b: the name a found site mints under. A quoted literal is never
/// re-resolved. A constant path is folded through `resolve` when one is
/// given: a value the LA.41 name rule ([`is_event_name`]) accepts names the
/// site and counts `folded`; no value, or a rejected one, leaves the site on
/// its path (CB.3a's fallback identity) and counts `unresolved`. With no
/// resolver (the per-file pass) the path is the name and nothing is counted.
///
/// fired_on marker, per folded or unresolved site:
///   `GLIA_EVENT_DEBUG=1 ... 2>&1 | grep '\[eventbus\] const-fold'`
fn site_name(
    key: SiteKey,
    side: &str,
    needle: &str,
    resolve: Option<ConstResolver<'_>>,
    counts: &mut ConstFoldCounts,
) -> String {
    let path = match key {
        SiteKey::Literal(name) => return name,
        SiteKey::Constant(path) => path,
    };
    let Some(resolve) = resolve else {
        return path;
    };
    let folded = resolve(&path).filter(|v| is_event_name(v));
    if event_debug() {
        eprintln!(
            "[eventbus] const-fold {side} needle='{needle}' path={path} -> {}",
            folded.as_deref().unwrap_or("(unresolved)")
        );
    }
    match folded {
        Some(value) => {
            counts.folded += 1;
            value
        }
        None => {
            counts.unresolved += 1;
            path
        }
    }
}

/// LB.8b: the delivery scope of a string-keyed needle's site, when it is a
/// transport ([`TRANSPORT_NEEDLES`], or `.emit(` in a file naming
/// [`NEST_MICROSERVICES`]); `None` for an in-process bus.
fn transport_via(pattern: &str, source: &str, ctx: &mut VerbCtx) -> Option<&'static str> {
    if let Some(&(_, via)) = TRANSPORT_NEEDLES.iter().find(|(needle, _)| *needle == pattern) {
        return Some(via);
    }
    (pattern == ".emit(" && ctx.ms_client(source)).then_some(MICROSERVICES_EMIT_VIA)
}

/// LB.8b: record that node `id`'s event is delivered over a transport, as an
/// ORIGIN cell `{"provenance":"synthetic","delivery":"transport","via":..}`.
/// `provenance` stays first and `synthetic`: the engine's
/// `tag_synthetic_provenance` skips a node that already carries ORIGIN (it
/// would otherwise write exactly `{"provenance":"synthetic"}` on every EVENT_*
/// node), and engram-export reads only `provenance`. A node that already has
/// an ORIGIN cell (a second transport needle naming the same event) is left
/// alone, so a node carries at most one. The EventBusResolver reads
/// `"delivery":"transport"` to pair this side across project owners.
fn mark_transport(nodes: &mut [Node], id: NodeId, via: &str) {
    let Some(node) = nodes.iter_mut().find(|n| n.id == id) else {
        return;
    };
    if node.cells.iter().any(|c| c.kind == cell_type::ORIGIN) {
        return;
    }
    node.cells.push(Cell {
        kind: cell_type::ORIGIN,
        payload: CellPayload::Json(format!(
            r#"{{"provenance":"synthetic","delivery":"transport","via":"{via}"}}"#
        )),
    });
}

/// What the occurrence of a string-keyed needle names ([`event_name_at`]).
#[derive(Debug, PartialEq, Eq)]
enum SiteName {
    /// A quoted literal that reads like a name (LA.41): the argument after the
    /// needle, or a putEvents entry's `DetailType`.
    Literal(String),
    /// CB.3a: a constant reference argument ([`constant_ref_after`]), kept as
    /// its path (`OrderEvents.Created`) — the name both sides of a
    /// constant-keyed bus share. This is the site's FALLBACK identity: CB.3b
    /// folds it to the constant's literal through the engine's repo const
    /// table ([`site_name`], post-cache), and this variant is the one place a
    /// constant site is named.
    Constant(String),
    /// LA.41: a quoted literal that is not name-shaped. Not a site.
    Malformed,
    /// CB.3a: no literal and no constant reference — a variable
    /// (`subject.next(items)`), a template, an object, a call, a putEvents
    /// without a `DetailType`. Not a site: the needle's verb names the API,
    /// not an event.
    Unnamed,
}

/// The event the occurrence of a string-keyed needle at `idx` names, by the
/// needle's [`NameRule`]. [`find_gated`] walks on past a
/// [`SiteName::Malformed`] or [`SiteName::Unnamed`] occurrence.
fn event_name_at(source: &str, pattern: &str, idx: usize, rule: NameRule) -> SiteName {
    let past = idx + pattern.len();
    let lit = match rule {
        NameRule::Literal => literal_after(source, past),
        NameRule::DetailType => detail_type_in(source, past),
    };
    match lit {
        LiteralAt::Name(name) => SiteName::Literal(name),
        LiteralAt::Malformed => SiteName::Malformed,
        LiteralAt::Absent if rule == NameRule::Literal => {
            constant_ref_after(source, past).map_or(SiteName::Unnamed, SiteName::Constant)
        }
        LiteralAt::Absent => SiteName::Unnamed,
    }
}

/// The most segments a constant reference may have (`A.B.C.D`).
const CONSTANT_MAX_SEGMENTS: usize = 4;

/// CB.3a: the constant reference that is the whole argument at byte `at` (just
/// past a needle's `(`), verbatim. Whitespace and line breaks before it are
/// skipped; it is an identifier path over `[A-Za-z0-9_$]` segments joined by
/// `.` or `::` (at most [`CONSTANT_MAX_SEGMENTS`], none starting with a
/// digit), followed — after whitespace — by `,` or `)`. It counts when it has
/// two or more segments and the first starts with an ASCII uppercase letter
/// (`OrderEvents.Created`, `Events::ORDER_PLACED`), or is one segment of
/// `[A-Z0-9_]` with at least one letter (`ORDER_PLACED`). So `this.x`,
/// `payload.type`, `event`, `items`, a call (`name()`), a member of a call
/// (`Foo.bar()`), a template, a number and a PascalCase class name
/// (`OrderPlaced`) name nothing. Nor does a path whose last segment is
/// bus-shaped ([`is_bus_receiver`], the collection nouns `events` /
/// `notifications` not counted): `Phoenix.PubSub.subscribe(Shop.PubSub,
/// "orders")` names the PubSub SERVER first and its topic second, and keying
/// the site by the server would make every topic one event. The path is kept
/// as written (`.` and `::` alike), so both sides of a constant-keyed pair
/// produce one key, which the EventBusResolver's type-name fold leaves alone
/// unless it is one UPPER_SNAKE segment, which folds alike on both sides.
/// Walks ASCII bytes and slices only at ASCII positions.
fn constant_ref_after(source: &str, at: usize) -> Option<String> {
    let b = source.as_bytes();
    let mut i = at;
    while b.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    let start = i;
    let mut segments: Vec<(usize, usize)> = Vec::new();
    loop {
        let seg = i;
        if b.get(seg).is_some_and(u8::is_ascii_digit) {
            return None;
        }
        while b.get(i).copied().is_some_and(is_ident_byte) {
            i += 1;
        }
        if i == seg || segments.len() == CONSTANT_MAX_SEGMENTS {
            return None;
        }
        segments.push((seg, i));
        if b.get(i) == Some(&b'.') {
            i += 1;
        } else if b.get(i..i + 2) == Some(b"::".as_slice()) {
            i += 2;
        } else {
            break;
        }
    }
    let end = i;
    while b.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    if !matches!(b.get(i), Some(b',' | b')')) {
        return None;
    }
    let first = &b[segments[0].0..segments[0].1];
    let constant = if segments.len() >= 2 {
        first[0].is_ascii_uppercase()
    } else {
        first
            .iter()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == b'_')
            && first.iter().any(u8::is_ascii_uppercase)
    };
    let (last_start, last_end) = segments[segments.len() - 1];
    let names_a_bus = is_bus_receiver(&source[last_start..last_end], true);
    (constant && !names_a_bus).then(|| source[start..end].to_string())
}

/// The key an EventBridge entry names its event type by.
const DETAIL_TYPE: &[u8] = b"DetailType";

/// CB.3a: the `DetailType` literal of a `putEvents(..)` call whose needle ends
/// at byte `at`. The needle must be followed (after whitespace) by `(`; its
/// balanced argument span — brackets counted, quoted strings skipped — is
/// searched for the first `DetailType` key, bare (`DetailType: "X"`, a TS / JS
/// object; `DetailType="X"`, Python keyword arguments) or quoted
/// (`"DetailType": "X"`), and its value read with [`literal_after`]. A value
/// that is not a quoted literal, no `DetailType` in the span, or a needle with
/// no call is [`LiteralAt::Absent`]. Byte walk; every slice [`literal_after`]
/// takes starts just past an ASCII byte.
fn detail_type_in(source: &str, at: usize) -> LiteralAt {
    let b = source.as_bytes();
    let mut i = at;
    while b.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    if b.get(i) != Some(&b'(') {
        return LiteralAt::Absent;
    }
    let mut depth = 0usize;
    let mut quote: Option<u8> = None;
    while let Some(&c) = b.get(i) {
        if let Some(q) = quote {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        match c {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return LiteralAt::Absent;
                }
            }
            b'\'' | b'"' | b'`' => {
                let key_end = i + 1 + DETAIL_TYPE.len();
                if b[i + 1..].starts_with(DETAIL_TYPE)
                    && b.get(key_end) == Some(&c)
                    && let Some(lit) = detail_value(source, key_end + 1)
                {
                    return lit;
                }
                quote = Some(c);
            }
            _ if b[i..].starts_with(DETAIL_TYPE)
                && (i == 0 || !is_ident_byte(b[i - 1]))
                && !b
                    .get(i + DETAIL_TYPE.len())
                    .copied()
                    .is_some_and(is_ident_byte) =>
            {
                if let Some(lit) = detail_value(source, i + DETAIL_TYPE.len()) {
                    return lit;
                }
            }
            _ => {}
        }
        i += 1;
    }
    LiteralAt::Absent
}

/// The value after a `DetailType` key ending at byte `at`: `None` when no
/// `:` or `=` (not `==` / `=>`) follows, so the scan goes on; else what
/// [`literal_after`] reads there.
fn detail_value(source: &str, at: usize) -> Option<LiteralAt> {
    let b = source.as_bytes();
    let mut i = at;
    while matches!(b.get(i), Some(b' ' | b'\t')) {
        i += 1;
    }
    match b.get(i) {
        Some(b':') => {}
        Some(b'=') if !matches!(b.get(i + 1), Some(b'=' | b'>')) => {}
        _ => return None,
    }
    Some(literal_after(source, i + 1))
}

/// Per-extract-call file facts the verb gates (LA.29, LA.39) and the broker
/// gate (A2.9) read. All lazy: the lowercase copy is built at most once per
/// call, and only when a gated verb or a broker-ambiguous needle actually
/// matched — a file with no `publish(` / `.subscribe(` / `.on(` never pays for
/// it. The copy is only ever substring-tested, never used to slice `source`.
/// The LA.39 facts are computed only when a [`VerbGate::Receiver`] occurrence
/// is on a receiver that is not bus-shaped.
#[derive(Default)]
struct VerbCtx {
    lower: Option<String>,
    bus_import: Option<bool>,
    broker: Option<bool>,
    /// LA.39: the identifiers bound to a constructed emitter
    /// ([`emitter_bindings`]).
    bound: Option<Vec<EmitterBinding>>,
    /// LA.39: does the file import an emitter library
    /// ([`imports_emitter_lib`])?
    emitter_lib: Option<bool>,
    /// LA.39: does the file declare an emitter subclass whose evidence holds
    /// ([`emitter_subclass`])?
    subclass: Option<bool>,
    /// LA.39: does the file name the quoted module [`NEST_MICROSERVICES`]?
    ms_client: Option<bool>,
}

impl VerbCtx {
    fn lower(&mut self, source: &str) -> &str {
        self.lower.get_or_insert_with(|| source.to_ascii_lowercase())
    }

    fn emitter_lib(&mut self, source: &str) -> bool {
        *self
            .emitter_lib
            .get_or_insert_with(|| imports_emitter_lib(source))
    }

    /// Is `receiver` bound in this file to an in-process emitter: any
    /// constructor from an emitter-library file, or a `new EventTarget()`?
    fn bound(&mut self, source: &str, receiver: &str) -> bool {
        let bindings = self.bound.get_or_insert_with(|| emitter_bindings(source));
        let mut platform = false;
        let mut library = false;
        for b in bindings.iter().filter(|b| b.name == receiver) {
            if b.event_target {
                platform = true;
            } else {
                library = true;
            }
        }
        platform || (library && self.emitter_lib(source))
    }

    /// Does the file declare an emitter subclass (`this` / `self` is a bus)?
    fn subclass(&mut self, source: &str) -> bool {
        if let Some(v) = self.subclass {
            return v;
        }
        let v = match emitter_subclass(source) {
            SubclassEvidence::None => false,
            SubclassEvidence::EventTarget => true,
            SubclassEvidence::Library => self.emitter_lib(source),
        };
        self.subclass = Some(v);
        v
    }

    fn ms_client(&mut self, source: &str) -> bool {
        *self
            .ms_client
            .get_or_insert_with(|| names_quoted_module(source, NEST_MICROSERVICES))
    }

    /// Does the file name an in-process pub/sub library ([`PUBSUB_IMPORTS`])?
    fn bus_import(&mut self, source: &str) -> bool {
        if let Some(v) = self.bus_import {
            return v;
        }
        let v = bus_import(self.lower(source));
        self.bus_import = Some(v);
        v
    }

    /// A2.9: does this file import a message-broker client?
    fn broker_present(&mut self, source: &str) -> bool {
        if let Some(v) = self.broker {
            return v;
        }
        let v = broker_present(self.lower(source));
        self.broker = Some(v);
        v
    }
}

fn bus_import(lower_source: &str) -> bool {
    PUBSUB_IMPORTS.iter().any(|lib| lower_source.contains(lib))
}

/// True when `lower_source` carries any library signal that gates a BROKER row
/// in queues.rs (task-queue rows excluded). The list is DERIVED from those
/// rows, never re-typed here: a broker the queue extractor learns to gate is
/// suppressed on the event side in the same edit, and the two files cannot
/// disagree about which library makes a verb broker traffic.
///
/// Deliberately NOT the bare word `pubsub`: graphql-subscriptions' `PubSub`
/// is an in-process bus (`pubsub.publish('POST_ADDED', ...)`). Google Pub/Sub
/// is recognised by its package (`@google-cloud/pubsub`, `google.cloud`).
///
/// Accepted trade: a file that imports a broker client AND uses an in-process
/// emitter loses that emitter's `.on(` / `.subscribe(` / `publish(` nodes.
fn broker_present(lower_source: &str) -> bool {
    crate::queues::broker_signal_present(lower_source)
}

/// fired_on marker (A2.9). It lives here but speaks in the `[queues]`
/// namespace on purpose — that is where the suppressed traffic went, and the
/// queue-side `[queues] needle '...'` line is its other half:
///   `GLIA_QUEUE_DEBUG=1 ... 2>&1 | grep '\[queues\] broker-event suppressed'`
/// The extractor is called without a path, so the line names the event, not
/// the file; the paired `[queues] needle` line carries `file=`.
fn suppressed(side: &str, needle: &str, name: &str) {
    if crate::queues::debug_enabled() {
        eprintln!("[queues] broker-event suppressed {side}='{needle}' name={name}");
    }
}

/// fired_on marker for the verb gate (LA.29), the event-name shape rule
/// (LA.41) and the real-name rule (CB.3a), the queues.rs `debug_enabled`
/// pattern under its own switch:
///   `GLIA_EVENT_DEBUG=1 ... 2>&1 | grep '\[eventbus\] verb-gate'`
///   `GLIA_EVENT_DEBUG=1 ... 2>&1 | grep '\[eventbus\] aws-sdk command skipped'`
///   `GLIA_EVENT_DEBUG=1 ... 2>&1 | grep '\[eventbus\] bad-name'`
///   `GLIA_EVENT_DEBUG=1 ... 2>&1 | grep '\[eventbus\] unnamed'`
fn event_debug() -> bool {
    static DEBUG: OnceLock<bool> = OnceLock::new();
    *DEBUG.get_or_init(|| {
        std::env::var("GLIA_EVENT_DEBUG").is_ok_and(|v| !v.is_empty() && v != "0")
    })
}

/// Why a gate kept or rejected one occurrence of a gated verb.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// A call with bus evidence: this occurrence names the event.
    Keep(Via),
    /// A typed publish (`new` is the first argument token): the type pass
    /// already minted the event under its type name.
    TypeSite,
    /// `def publish(self, m):`, `void publish(String p);`, `publish(m): void {`.
    Declaration,
    /// A call with neither a bus-shaped receiver nor a pub/sub import
    /// ([`VerbGate::Bus`]); an occurrence [`receiver_admits`] finds no bus for
    /// ([`VerbGate::Receiver`]).
    NoBus,
}

/// Per-file tally behind the `[eventbus] verb-gate` line.
#[derive(Default)]
struct GateTally {
    kept: usize,
    decl: usize,
    no_bus: usize,
    type_site: usize,
}

/// The occurrence of `pattern` that names the event, with the event it names.
/// Occurrences are walked in order; one is the site when it is not
/// queue-owned, when its `gate` keeps it — [`judge_verb`] for
/// [`VerbGate::Bus`], [`receiver_admits`] for [`VerbGate::Receiver`] — and
/// when [`event_name_at`] reads a name there: a quoted literal that is not
/// name-shaped (LA.41) or an argument that names nothing (CB.3a: a variable,
/// a template, a putEvents without a `DetailType`) skips that occurrence and
/// the walk goes on, so a needle table's `"@OnEvent(", "x"` never decides the
/// file's node and a later `@OnEvent('order.shipped')` does, anchored there,
/// and a file's `emit(x)` then `emit("user.created", u)` mints `user.created`.
/// A file whose first `publish(` is a declaration and a later one a bus call
/// anchors at the bus call (LA.29); a file whose first `.addEventListener(` is
/// on a DOM element and a later one on a bus anchors at the bus call (LA.39).
/// Broker suppression (A2.9) is the caller's, after this walk. CJ.1a: an
/// occurrence `guard` refuses (it starts in a Rust / Python literal or
/// comment) is skipped before every check, so it is never tallied as a gate
/// rejection.
fn find_gated(
    source: &str,
    pattern: &str,
    rule: NameRule,
    gate: VerbGate,
    ctx: &mut VerbCtx,
    guard: &mut LazyGuard<'_>,
) -> Option<(usize, SiteKey)> {
    let gated = gate != VerbGate::Open;
    let mut tally = GateTally::default();
    let mut via: Option<Via> = None;
    let mut bad_name = 0usize;
    // CB.3a: occurrences that named nothing, and whether the site found was
    // named through a constant reference.
    let mut unnamed = 0usize;
    let mut constant = 0usize;
    let mut found = None;
    let mut from = 0usize;
    while let Some(rel) = source[from..].find(pattern) {
        let at = from + rel;
        from = at + pattern.len();
        if !guard.admits(at) {
            continue;
        }
        if pattern == "publish(" && queue_owned_publish(source, at) {
            continue;
        }
        let verdict = match gate {
            VerbGate::Open => None,
            VerbGate::Bus => Some(judge_verb(source, pattern, at, ctx)),
            VerbGate::Receiver => Some(
                receiver_admits(source, at, pattern, ctx).map_or(Verdict::NoBus, Verdict::Keep),
            ),
        };
        let mut kept_via = None;
        if let Some(verdict) = verdict {
            match verdict {
                Verdict::Keep(v) => kept_via = Some(v),
                Verdict::TypeSite => {
                    tally.type_site += 1;
                    continue;
                }
                Verdict::Declaration => {
                    tally.decl += 1;
                    continue;
                }
                Verdict::NoBus => {
                    tally.no_bus += 1;
                    continue;
                }
            }
        }
        let name = match event_name_at(source, pattern, at, rule) {
            SiteName::Literal(name) => SiteKey::Literal(name),
            SiteName::Constant(path) => {
                constant += 1;
                SiteKey::Constant(path)
            }
            SiteName::Malformed => {
                bad_name += 1;
                continue;
            }
            SiteName::Unnamed => {
                unnamed += 1;
                continue;
            }
        };
        if gated {
            tally.kept = 1;
            via = kept_via;
        }
        found = Some((at, name));
        break;
    }
    if event_debug() {
        // A gated occurrence the gate kept but whose literal was malformed, or
        // which named nothing, is counted by the bad-name / unnamed line, not
        // the verb-gate one.
        if gated
            && tally.kept + tally.decl + tally.no_bus + tally.type_site + bad_name + unnamed > 0
        {
            eprintln!(
                "[eventbus] verb-gate needle='{pattern}' kept={} rejected decl={} no_bus={} type_site={} via={}",
                tally.kept,
                tally.decl,
                tally.no_bus,
                tally.type_site,
                via.map_or("-", Via::label)
            );
        }
        if bad_name > 0 {
            eprintln!(
                "[eventbus] bad-name needle='{pattern}' skipped={bad_name} kept={}",
                usize::from(found.is_some())
            );
        }
        if unnamed + constant > 0 {
            eprintln!(
                "[eventbus] unnamed needle='{pattern}' skipped={unnamed} constant={constant}"
            );
        }
    }
    found
}

/// Judge one occurrence of a gated verb at byte `at` (the needle's start).
///
/// A DOTTED occurrence (`x.publish(`, `x?.publish(`, `X::publish(`, every
/// `.subscribe(`) is always a call; it needs a bus-shaped receiver or a
/// pub/sub import. A BARE `publish(` may be a declaration; if it is a call it
/// needs the import (Wisper's `publish('order_placed', self)` inside the
/// publisher). A typed site `publish(new X(..))` is rejected first: the type
/// pass owns it, and the string pass would add a duplicate `event_emit:publish`.
fn judge_verb(source: &str, pattern: &str, at: usize, ctx: &mut VerbCtx) -> Verdict {
    let open = at + pattern.len();
    if source[open..].trim_start().starts_with("new ") {
        return Verdict::TypeSite;
    }
    let b = source.as_bytes();
    let dot_at = if pattern.starts_with('.') {
        Some(at)
    } else {
        // `republish(` / `do_publish(`: the declaration test reads the whole
        // identifier the needle ends, not a prefix glued to it.
        let word = ident_start(b, at);
        match word.checked_sub(1).map(|i| b[i]) {
            Some(b'.') => Some(word - 1),
            Some(b':') if word >= 2 && b[word - 2] == b':' => Some(word - 2),
            _ => {
                if is_declaration(source, word, open) {
                    return Verdict::Declaration;
                }
                None
            }
        }
    };
    let bus_receiver = dot_at.is_some_and(|dot| {
        let (receiver, called) = receiver_segment(source, dot);
        is_bus_receiver(receiver, called)
    });
    if bus_receiver {
        Verdict::Keep(Via::Receiver)
    } else if ctx.bus_import(source) {
        Verdict::Keep(Via::Import)
    } else {
        Verdict::NoBus
    }
}

/// LA.39: is the occurrence of a [`VerbGate::Receiver`] needle at byte `at` a
/// call on an in-process bus? First match wins:
///  R1 [`Via::Receiver`] — the receiver's name is bus-shaped
///     ([`is_bus_receiver`]): `bus.on`, `this.eventBus.addListener`,
///     `@emitter.on` (pyee), `this.eventEmitter.emit` (NestJS EventEmitter2),
///     `eventDispatcher.dispatch`;
///  R2 [`Via::Bound`] — the receiver is bound in this file to a constructed
///     emitter ([`VerbCtx::bound`]): `const jobs = new EventEmitter()` beside
///     `import ... from 'node:events'`;
///  R3 [`Via::Subclass`] — the receiver is `this` / `self` and the file
///     declares an emitter subclass ([`VerbCtx::subclass`]);
///  R4 [`Via::Microservices`] — `.emit(` in a file importing
///     `@nestjs/microservices`.
///
/// The receiver of a dotted needle is [`receiver_segment`] at the needle's
/// `.`; `dispatchEvent(` has a receiver only when the byte before it is `.`,
/// so a bare `dispatchEvent(..)` — a global call, a method declaration — is
/// rejected. A call result (`$(form).on`, `this.getStore().dispatch`) is
/// admitted only by R1 or R4. A verb on an emitter CLASS
/// ([`is_emitter_class`]: `EventEmitter.emit(value)` in prose) is admitted
/// only by R2, when the capitalised name is itself a bound instance.
fn receiver_admits(source: &str, at: usize, pattern: &str, ctx: &mut VerbCtx) -> Option<Via> {
    let b = source.as_bytes();
    let dot_at = if pattern.starts_with('.') {
        at
    } else if at > 0 && b[at - 1] == b'.' {
        at - 1
    } else {
        return None;
    };
    let (receiver, called) = receiver_segment(source, dot_at);
    if receiver.is_empty() {
        // `'.emit('` in a needle table, `` `.on(` `` in prose, `arr[0].emit(`:
        // no identifier names a receiver, so nothing is evidence of a bus.
        return None;
    }
    let class = is_emitter_class(receiver);
    if !class && is_bus_receiver(receiver, called) {
        return Some(Via::Receiver);
    }
    if !called {
        if matches!(receiver, "this" | "self") {
            if ctx.subclass(source) {
                return Some(Via::Subclass);
            }
        } else if ctx.bound(source, receiver) {
            return Some(Via::Bound);
        }
    }
    if !class && pattern == ".emit(" && ctx.ms_client(source) {
        return Some(Via::Microservices);
    }
    None
}

/// Is `receiver` an emitter CLASS ([`EMITTER_CONSTRUCTORS`], the capitalised
/// ones) rather than an instance? A verb on the class itself —
/// `EventEmitter.emit(value)` in a doc comment, Node's static
/// `EventEmitter.on(emitter, name)` — is no bus call, though the name ends
/// with `emitter`. A capitalised INSTANCE bound in the file
/// (`const Emitter = new EventEmitter()`) is still admitted by R2.
fn is_emitter_class(receiver: &str) -> bool {
    receiver.starts_with(|c: char| c.is_ascii_uppercase()) && EMITTER_CONSTRUCTORS.contains(&receiver)
}

/// One identifier [`emitter_bindings`] found bound to an emitter constructor.
#[derive(Debug, Clone, PartialEq, Eq)]
struct EmitterBinding {
    name: String,
    /// `new EventTarget(..)`: the platform emitter, admitted without an
    /// emitter-library import.
    event_target: bool,
}

/// LA.39: every identifier this file assigns an emitter constructor
/// ([`EMITTER_CONSTRUCTORS`]) to, in source order. For each constructor name
/// (identifier-bounded on the left, `(` or `<` after it) the walk goes back
/// over a `pkg.` qualifier chain, whitespace and an optional `new`, then needs
/// a plain `=` (not `==`, `!=`, `<=`, `>=`, `=>`, `+=` ...), and reads the
/// assigned name with [`assigned_name`]: `const jobs = new EventEmitter()`,
/// `private hub: EventEmitter = new EventEmitter()`, `this.hub = mitt()`,
/// `ee = pyee.EventEmitter()`. Byte walks around ASCII anchors only.
fn emitter_bindings(source: &str) -> Vec<EmitterBinding> {
    let b = source.as_bytes();
    let mut out: Vec<EmitterBinding> = Vec::new();
    for &ctor in EMITTER_CONSTRUCTORS {
        let mut from = 0usize;
        while let Some(rel) = source[from..].find(ctor) {
            let site = from + rel;
            let after = site + ctor.len();
            from = after;
            if (site > 0 && is_ident_byte(b[site - 1]))
                || !matches!(b.get(after), Some(b'(' | b'<'))
            {
                continue;
            }
            let mut i = site;
            // `pyee.EventEmitter(` / `new events.EventEmitter(`.
            while i > 1 && b[i - 1] == b'.' && is_ident_byte(b[i - 2]) {
                i = ident_start(b, i - 1);
            }
            i = skip_ws_back(b, i);
            let had_new =
                i >= 3 && b[i - 3..i] == b"new"[..] && (i == 3 || !is_ident_byte(b[i - 4]));
            if had_new {
                i = skip_ws_back(b, i - 3);
            }
            if i == 0 || b[i - 1] != b'=' {
                continue;
            }
            let eq = i - 1;
            if eq > 0
                && matches!(
                    b[eq - 1],
                    b'=' | b'!' | b'<' | b'>' | b'+' | b'-' | b'*' | b'/' | b'%' | b'&' | b'|'
                        | b'^' | b'?'
                )
            {
                continue;
            }
            let Some(name) = assigned_name(source, eq) else {
                continue;
            };
            let binding = EmitterBinding {
                name: name.to_string(),
                event_target: had_new && ctor == "EventTarget",
            };
            if !out.contains(&binding) {
                out.push(binding);
            }
        }
    }
    out
}

/// Index of the first byte of the whitespace run ending at `i`.
fn skip_ws_back(b: &[u8], mut i: usize) -> usize {
    while i > 0 && b[i - 1].is_ascii_whitespace() {
        i -= 1;
    }
    i
}

/// The identifier assigned by the `=` at byte `eq`. When a `: Type`
/// annotation sits between the name and the `=` (a `:` at angle-bracket depth
/// 0 before any `, ( ) ; { } =`, quote or line break), the name is the
/// identifier before that `:` (a TS `!` / `?` marker skipped); otherwise it is
/// the identifier right before the `=`. A `this.` / `self.` prefix falls away
/// because the identifier stops at the `.`. `None` when no identifier is
/// there (`{ a, b } = ..`, `[x] = ..`).
fn assigned_name(source: &str, eq: usize) -> Option<&str> {
    let b = source.as_bytes();
    let mut depth = 0usize;
    let mut colon = None;
    let mut i = eq;
    while i > 0 {
        let c = b[i - 1];
        match c {
            b'>' => depth += 1,
            b'<' => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            }
            b':' if depth == 0 => {
                colon = Some(i - 1);
                break;
            }
            b',' | b'(' | b')' | b';' | b'{' | b'}' | b'=' | b'"' | b'\'' | b'`' | b'\n'
                if depth == 0 =>
            {
                break;
            }
            _ => {}
        }
        i -= 1;
    }
    let mut end = colon.unwrap_or(eq);
    while end > 0 && matches!(b[end - 1], b' ' | b'\t' | b'!' | b'?') {
        end -= 1;
    }
    let start = ident_start(b, end);
    (start < end).then(|| &source[start..end])
}

/// LA.39: does the file import an in-process emitter library? A quoted module
/// string exactly one of [`EMITTER_LIBS`] in import position — the bytes
/// before its opening quote, whitespace skipped, end with `from`, `require(`,
/// `import(` or a bare `import` keyword (identifier-bounded) — or a Python
/// line starting `from pyee` / `import pyee`. A bare quoted word is not
/// enough: `db.collection('events')` or `router.navigate(['events'])` never
/// make a file an emitter-library file.
fn imports_emitter_lib(source: &str) -> bool {
    EMITTER_LIBS.iter().any(|lib| {
        quoted_module_sites(source, lib).any(|quote| import_position(source.as_bytes(), quote))
    }) || source.lines().any(|line| {
        let t = line.trim_start();
        ["from pyee", "import pyee"].iter().any(|p| {
            t.strip_prefix(p)
                .is_some_and(|rest| !rest.bytes().next().is_some_and(is_ident_byte))
        })
    })
}

/// Byte offsets of the opening quote of every `'module'` / `"module"` in
/// `source`.
fn quoted_module_sites<'a>(source: &'a str, module: &'a str) -> impl Iterator<Item = usize> + 'a {
    [b'\'', b'"'].into_iter().flat_map(move |q| {
        let b = source.as_bytes();
        let mut from = 0usize;
        std::iter::from_fn(move || {
            while let Some(rel) = source[from..].find(module) {
                let at = from + rel;
                from = at + module.len();
                let end = at + module.len();
                if at > 0 && b[at - 1] == q && b.get(end) == Some(&q) {
                    return Some(at - 1);
                }
            }
            None
        })
    })
}

/// Is the quote at byte `quote` the module argument of an import?
fn import_position(b: &[u8], quote: usize) -> bool {
    let before = &b[..skip_ws_back(b, quote)];
    let word = |w: &[u8]| {
        before.ends_with(w)
            && (before.len() == w.len() || !is_ident_byte(before[before.len() - w.len() - 1]))
    };
    before.ends_with(b"require(") || before.ends_with(b"import(") || word(b"from") || word(b"import")
}

/// Does the file name `module` as a quoted string anywhere? For a scoped
/// package name (`@nestjs/microservices`) that is evidence enough.
fn names_quoted_module(source: &str, module: &str) -> bool {
    quoted_module_sites(source, module).next().is_some()
}

/// What [`emitter_subclass`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubclassEvidence {
    None,
    /// `extends EventTarget`: no import needed.
    EventTarget,
    /// `extends EventEmitter` / `EventEmitter2` / `Emitter`, or a pyee base
    /// class: holds only with [`imports_emitter_lib`].
    Library,
}

/// LA.39: does the file declare a class whose `this` / `self` is an emitter?
/// JS / TS `extends <Base>` (a `pkg.` qualifier and generic arguments
/// dropped) with a base in [`EMITTER_BASES`] or `EventTarget`; a Python
/// `class X(<bases>):` line with a base in [`PYEE_BASES`].
fn emitter_subclass(source: &str) -> SubclassEvidence {
    let b = source.as_bytes();
    let mut found = SubclassEvidence::None;
    let mut from = 0usize;
    while let Some(rel) = source[from..].find("extends") {
        let at = from + rel;
        from = at + "extends".len();
        if (at > 0 && is_ident_byte(b[at - 1])) || !b.get(from).is_some_and(u8::is_ascii_whitespace) {
            continue;
        }
        let rest = source[from..].trim_start();
        let end = rest
            .bytes()
            .position(|c| !(is_ident_byte(c) || c == b'.'))
            .unwrap_or(rest.len());
        let base = rest[..end].rsplit('.').next().unwrap_or("");
        if base == "EventTarget" {
            return SubclassEvidence::EventTarget;
        }
        if EMITTER_BASES.contains(&base) {
            found = SubclassEvidence::Library;
        }
    }
    let pyee_base = source.lines().any(|line| {
        let Some(rest) = line.trim_start().strip_prefix("class ") else {
            return false;
        };
        let Some(open) = rest.find('(') else {
            return false;
        };
        let Some(len) = rest[open + 1..].find(')') else {
            return false;
        };
        rest[open + 1..open + 1 + len]
            .split(',')
            .any(|base| PYEE_BASES.contains(&base.trim().rsplit('.').next().unwrap_or("")))
    });
    if pyee_base {
        found = SubclassEvidence::Library;
    }
    found
}

fn is_ident_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'$'
}

/// Start of the ASCII identifier that ends at `at` (`at` itself when the byte
/// before is not an identifier byte). Always a char boundary: it lands on an
/// ASCII byte or stays at `at`.
fn ident_start(b: &[u8], at: usize) -> usize {
    let mut i = at;
    while i > 0 && is_ident_byte(b[i - 1]) {
        i -= 1;
    }
    i
}

/// The receiver identifier before the separator at `dot_at` (`.` or the first
/// `:` of `::`). Whitespace (a chain broken across lines), `?.` and TS `!.`
/// are skipped, and a trailing call reads as the called name, so
/// `vertx.eventBus().publish` reads `eventBus` and
/// `this.http.get(u).subscribe` reads `get`; the flag says the receiver is
/// such a call result. Empty when no identifier precedes it. Reads ASCII bytes
/// only, so the slice is on char boundaries.
fn receiver_segment(source: &str, dot_at: usize) -> (&str, bool) {
    let b = source.as_bytes();
    let mut end = dot_at;
    while end > 0 && (b[end - 1].is_ascii_whitespace() || matches!(b[end - 1], b'?' | b'!')) {
        end -= 1;
    }
    let called = end > 0 && b[end - 1] == b')';
    if called {
        let mut depth = 0usize;
        let mut i = end;
        loop {
            if i == 0 {
                return ("", true);
            }
            i -= 1;
            match b[i] {
                b')' => depth += 1,
                b'(' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
        }
        end = i;
    }
    (&source[ident_start(b, end)..end], called)
}

/// `called`: the receiver is a call result (`x.eventBus().publish`), where the
/// plural [`COLLECTION_SUFFIXES`] do not count.
fn is_bus_receiver(receiver: &str, called: bool) -> bool {
    let name = receiver
        .trim_start_matches(['_', '$', '@'])
        .to_ascii_lowercase();
    !name.is_empty()
        && BUS_RECEIVER_SUFFIXES
            .iter()
            .filter(|s| !(called && COLLECTION_SUFFIXES.contains(s)))
            .any(|s| name.ends_with(s))
}

/// Is the bare `publish(` whose identifier starts at `word` (argument list
/// opening just before `open`) a function DECLARATION rather than a call?
///
/// PREFIX is the line up to the identifier, LAST its last word. One of:
///  (i)   LAST is a declaration keyword ([`DECL_KEYWORDS`]), or PREFIX is a Go
///        method receiver `func (n *Notifier)`;
///  (ii)  LAST reads as a C-family return type (`void`, `Task`,
///        `Future<void>`, `String[]`, `String?`) and is not a [`CALL_WORDS`]
///        entry, and after the argument list — past `throws A, B`, `async`,
///        `const noexcept` — comes `{`, `;` (interface / abstract) or `=>`
///        (C# expression body);
///  (iii) PREFIX is empty or only [`METHOD_MODIFIERS`] and `*`, and `{` or a
///        TS return annotation `:` follows the argument list directly.
///
/// Anything else is a call: `publish('x', y);`, `return publish(x)`,
/// `const r = publish(x)`, `if (publish(x))`, Python `if publish(x):`.
fn is_declaration(source: &str, word: usize, open: usize) -> bool {
    let line_start = source[..word].rfind('\n').map_or(0, |i| i + 1);
    let prefix = source[line_start..word].trim();
    let last = prefix.split_whitespace().last().unwrap_or("");
    if DECL_KEYWORDS.contains(&last)
        || ((prefix.starts_with("func ") || prefix.starts_with("func(")) && prefix.ends_with(')'))
    {
        return true;
    }
    let Some(close) = balanced_close(source.as_bytes(), open) else {
        return false;
    };
    let direct = source[close + 1..].trim_start();
    let past_words = direct.trim_start_matches(|c: char| {
        c.is_ascii_alphanumeric() || matches!(c, '_' | '$' | ',' | '.') || c.is_whitespace()
    });
    if is_type_word(last)
        && (past_words.starts_with('{')
            || past_words.starts_with(';')
            || past_words.starts_with("=>"))
    {
        return true;
    }
    prefix
        .split_whitespace()
        .all(|w| w == "*" || METHOD_MODIFIERS.contains(&w))
        && (direct.starts_with('{') || direct.starts_with(':'))
}

/// A word that can be a return type: starts like an identifier and ends with
/// an identifier byte or a type closer (`>`, `]`, `?`, `*`, `&`). The start
/// rule keeps operators out (`=>`, `->`, `|>`, `?`, `&&`).
fn is_type_word(w: &str) -> bool {
    let b = w.as_bytes();
    let (Some(&first), Some(&last)) = (b.first(), b.last()) else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == b'_' || first == b'$')
        && (is_ident_byte(last) || matches!(last, b'>' | b']' | b'?' | b'*' | b'&'))
        && !CALL_WORDS.contains(&w)
}

/// Byte index of the `)` closing the argument list whose `(` ends just before
/// `open`. String contents are not special-cased: a `)` inside a literal
/// closes early and the text after it then reads as a call, never as a
/// declaration.
fn balanced_close(b: &[u8], open: usize) -> Option<usize> {
    let mut depth = 1usize;
    for (i, &c) in b.iter().enumerate().skip(open) {
        match c {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn queue_owned_publish(source: &str, at: usize) -> bool {
    let end = at + "publish(".len();
    QUEUE_OWNED_PUBLISH
        .iter()
        .any(|owned| source[..end].ends_with(owned))
}

/// Every occurrence of every needle, in needle order, with the byte offset of
/// the needle. The string-keyed path calls `find` ONCE per needle, so a file
/// publishing three event types contributed one node; the type-keyed pass
/// walks the whole file. [`AWS_V3_SEND`] is skipped in a file importing
/// `@aws-sdk/`: there it is a service-client command, not an event.
fn scan_type_needles<'a>(
    source: &str,
    needles: impl Iterator<Item = (&'a str, Option<char>)>,
) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    let aws_sdk = source.contains("@aws-sdk/");
    for (needle, close) in needles {
        let skip = aws_sdk && needle == AWS_V3_SEND;
        let mut from = 0usize;
        while let Some(rel) = source[from..].find(needle) {
            let site = from + rel;
            let at = site + needle.len();
            let token = match close {
                Some(c) => extract_type_token(&source[at..], Some(c)),
                // A bracketed needle bounds its own token; a bare annotation
                // does not, and Spring's does not name the type at all.
                None if needle.starts_with('@') => extract_listener_param_type(&source[at..]),
                None => extract_type_token(&source[at..], None),
            };
            if let Some(token) = token {
                if !skip {
                    out.push((token, site));
                } else if event_debug() {
                    eprintln!("[eventbus] aws-sdk command skipped type={token}");
                }
            }
            from = at;
        }
    }
    out
}

/// The type token following a type-keyed needle. `close` bounds a bracketed
/// form (`<Type>` / `(Type)`); `None` reads the identifier run and stops at the
/// first delimiter. Namespaces and generic arguments fall away, so
/// `Shop.Events.OrderPlaced`, `shop::events::OrderPlaced` and
/// `OrderPlaced<Guid>` all read `OrderPlaced`.
fn extract_type_token(after: &str, close: Option<char>) -> Option<String> {
    let trimmed = after.trim_start();
    let end = match close {
        Some(c) => trimmed.find(|ch: char| ch == c || ch == ',')?,
        None => trimmed
            .find(|ch: char| !(ch.is_alphanumeric() || ch == '_' || ch == '.' || ch == ':'))
            .unwrap_or(trimmed.len()),
    };
    let mut raw = trimmed[..end].trim();
    raw = raw.strip_suffix(".class").unwrap_or(raw); // Java `OrderPlaced.class`
    raw = raw.rsplit("::").next().unwrap_or(raw);
    raw = raw.rsplit('.').next().unwrap_or(raw);
    raw = raw.split('<').next().unwrap_or(raw);
    let token = raw.trim();
    // Uppercase-initial plain identifier ONLY. This is a type needle, so a
    // lowercase capture means we read an argument name or a keyword
    // (`@EventListener(condition = "...")`), not an event type. The same guard
    // is what keeps the resolver's fold off string topics.
    if token.is_empty()
        || token.len() > 128
        || !token.starts_with(|c: char| c.is_uppercase())
        || !token.chars().all(|c| c.is_alphanumeric() || c == '_')
    {
        return None;
    }
    Some(token.to_string())
}

/// Spring's `@EventListener` names no type: the event is the annotated
/// method's FIRST PARAMETER. `@EventListener(OrderPlaced.class)` names it
/// directly; the bare form needs the next signature, bounded to a few lines so
/// a dangling annotation captures nothing.
fn extract_listener_param_type(after: &str) -> Option<String> {
    let rest = if after.starts_with('(') {
        if let Some(token) = extract_type_token(&after[1..], Some(')')) {
            return Some(token);
        }
        match after.find(')') {
            Some(i) => &after[i + 1..],
            None => after,
        }
    } else {
        after
    };
    let window: String = rest.lines().take(4).collect::<Vec<_>>().join("\n");
    let open = window.find('(')?;
    let first = window[open + 1..].split([',', ')']).next()?;
    extract_type_token(first.split_whitespace().next()?, None)
}

/// What follows a string-keyed needle (LA.41).
#[derive(Debug, PartialEq, Eq)]
enum LiteralAt {
    /// A quoted literal that closes on its own line and reads like an event
    /// name ([`is_event_name`]).
    Name(String),
    /// A quoted literal that spans a line break, never closes, or is not
    /// name-shaped (`", "` between two strings of a needle table). The
    /// occurrence is not an event site.
    Malformed,
    /// No quoted literal: a variable, a constant, a backtick template, an
    /// object. The caller tries a constant reference ([`constant_ref_after`]),
    /// else the occurrence names nothing (CB.3a).
    Absent,
}

/// The quoted literal after a needle; `at` is the byte offset just past the
/// matched needle. Whitespace and line breaks BEFORE the literal are skipped
/// (`@OnEvent(\n  'order.shipped'\n)`); the literal itself must close on its
/// line. The quote scan walks bytes and slices only at ASCII quote bytes, so
/// every slice is on a char boundary.
fn literal_after(source: &str, at: usize) -> LiteralAt {
    let Some(after) = source.get(at..) else {
        return LiteralAt::Absent;
    };
    let trimmed = after.trim_start();
    let quote = match trimmed.as_bytes().first() {
        Some(&q @ (b'\'' | b'"')) => q,
        _ => return LiteralAt::Absent,
    };
    let body = &trimmed[1..];
    match body
        .bytes()
        .position(|b| b == quote || b == b'\n' || b == b'\r')
    {
        Some(end) if body.as_bytes()[end] == quote => {
            let lit = &body[..end];
            if is_event_name(lit) {
                LiteralAt::Name(lit.to_string())
            } else {
                LiteralAt::Malformed
            }
        }
        // A line break before the closing quote, or no closing quote at all.
        _ => LiteralAt::Malformed,
    }
}

/// Does a closed literal read like an event name? 1..=128 bytes; the first
/// char alphanumeric (any script) or one of `_ $ @ #`; every other char
/// alphanumeric, one of `_ . : / - @ $ * # +`, or a single space between two
/// non-spaces; at least one alphanumeric. So `user.created`, `user:login`,
/// `sensors/temp`, `update:modelValue`, `order.*`, `MY TOPIC` pass, and the
/// text between two strings of a table (`, `), anything with a control char,
/// parens, braces or doubled / edge spaces, `--help` and `*` do not. This is
/// the one place the accepted alphabet lives.
fn is_event_name(lit: &str) -> bool {
    if lit.is_empty() || lit.len() > 128 {
        return false;
    }
    let mut alnum = false;
    let mut prev: Option<char> = None;
    let mut chars = lit.chars().peekable();
    while let Some(c) = chars.next() {
        let ok = if c.is_alphanumeric() {
            alnum = true;
            true
        } else if prev.is_none() {
            matches!(c, '_' | '$' | '@' | '#')
        } else if c == ' ' {
            prev != Some(' ') && chars.peek().is_some_and(|&n| n != ' ')
        } else {
            matches!(c, '_' | '.' | ':' | '/' | '-' | '@' | '$' | '*' | '#' | '+')
        };
        if !ok {
            return false;
        }
        prev = Some(c);
    }
    alnum
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
    fn detects_emit() {
        let source = "emitter.emit('user.created', data);";
        let result = extract_event_emitter_nodes(source, "", module_id(), repo());
        assert!(!result.nodes.is_empty());
        assert!(
            result
                .nav
                .qname_by_id
                .values()
                .any(|q| q == "event_emit:user.created")
        );
    }

    #[test]
    fn detects_handler() {
        let source = "emitter.on('user.created', handler);";
        let result = extract_event_handler_nodes(source, "", module_id(), repo());
        assert!(!result.nodes.is_empty());
        assert!(
            result
                .nav
                .qname_by_id
                .values()
                .any(|q| q == "event_handle:user.created")
        );
    }

    #[test]
    fn detects_nest_event_pattern() {
        let source = "@EventPattern('order.placed')\nasync handleOrder(data) {}";
        let result = extract_event_handler_nodes(source, "", module_id(), repo());
        assert!(
            result
                .nav
                .qname_by_id
                .values()
                .any(|q| q == "event_handle:order.placed")
        );
    }

    #[test]
    fn detects_spring_publish_event_type() {
        let source = "publisher.publishEvent(new OrderPlacedEvent(id));";
        let result = extract_event_emitter_nodes(source, "", module_id(), repo());
        assert!(
            result
                .nav
                .qname_by_id
                .values()
                .any(|q| q == "event_emit:OrderPlacedEvent"),
            "{:?}",
            result.nav.qname_by_id
        );
        assert!(
            result
                .nodes
                .iter()
                .all(|n| n.confidence == Confidence::Medium)
        );
    }

    #[test]
    fn detects_spring_event_listener_param_type() {
        let source = "@EventListener\npublic void onOrderPlaced(OrderPlacedEvent event) {}";
        let result = extract_event_handler_nodes(source, "", module_id(), repo());
        assert!(
            result
                .nav
                .qname_by_id
                .values()
                .any(|q| q == "event_handle:OrderPlacedEvent"),
            "{:?}",
            result.nav.qname_by_id
        );
    }

    #[test]
    fn detects_mediatr_notification_handler() {
        let source = "public class Emailer : INotificationHandler<OrderPlaced>\n{\n}";
        let result = extract_event_handler_nodes(source, "", module_id(), repo());
        assert!(
            result
                .nav
                .qname_by_id
                .values()
                .any(|q| q == "event_handle:OrderPlaced"),
            "{:?}",
            result.nav.qname_by_id
        );
    }

    #[test]
    fn detects_nest_events_handler_decorator() {
        let source = "@EventsHandler(OrderPlacedEvent)\nexport class H {}";
        let result = extract_event_handler_nodes(source, "", module_id(), repo());
        assert!(
            result
                .nav
                .qname_by_id
                .values()
                .any(|q| q == "event_handle:OrderPlacedEvent"),
            "{:?}",
            result.nav.qname_by_id
        );
    }

    #[test]
    fn type_scan_is_multi_occurrence_and_strips_namespaces() {
        // The string-keyed path calls `find` once per needle; this one walks
        // the whole file, and `Shop.Events.X` reduces to `X`.
        let source = "_mediator.Publish(new Shop.Events.OrderPlaced());\n                      _mediator.Publish(new Shop.Events.OrderShipped());";
        let result = extract_event_emitter_nodes(source, "", module_id(), repo());
        let mut names: Vec<_> = result.nav.qname_by_id.values().cloned().collect();
        names.sort();
        assert_eq!(
            names,
            vec!["event_emit:OrderPlaced", "event_emit:OrderShipped"]
        );
    }

    #[test]
    fn type_needle_rejects_lowercase_and_keyword_captures() {
        // `condition` is an annotation argument, not an event type.
        let source = "@EventListener(condition = \"#e.ok\")\npublic void on(int n) {}";
        let result = extract_event_handler_nodes(source, "", module_id(), repo());
        assert!(
            !result
                .nav
                .qname_by_id
                .values()
                .any(|q| q.starts_with("event_handle:c")),
            "{:?}",
            result.nav.qname_by_id
        );
    }

    #[test]
    fn queue_owned_publish_does_not_double_emit() {
        // `nc.publish("orders", p)` is a NATS QUEUE_PRODUCER; the generic
        // `publish(` needle must not mint a phantom EVENT_EMITTER beside it.
        let source = "func pub(nc conn, p []byte) { nc.publish(\"orders\", p) }";
        let result = extract_event_emitter_nodes(source, "", module_id(), repo());
        assert!(
            result.nodes.is_empty(),
            "expected no event node, got {:?}",
            result.nav.qname_by_id
        );

        // An in-process bus that happens to use the same verb still fires.
        let bus = "bus.publish(\"user.created\", u);";
        let result = extract_event_emitter_nodes(bus, "", module_id(), repo());
        assert!(
            result
                .nav
                .qname_by_id
                .values()
                .any(|q| q == "event_emit:user.created")
        );
    }

    // ---- A2.9: broker pub/sub leaves the event bus -------------------------

    fn emitted(source: &str) -> Vec<String> {
        let mut v: Vec<String> = extract_event_emitter_nodes(source, "", module_id(), repo())
            .nav
            .qname_by_id
            .into_values()
            .collect();
        v.sort();
        v
    }

    fn handled(source: &str) -> Vec<String> {
        let mut v: Vec<String> = extract_event_handler_nodes(source, "", module_id(), repo())
            .nav
            .qname_by_id
            .into_values()
            .collect();
        v.sort();
        v
    }

    /// CJ.1a: in a Rust file a site whose needle starts in a string literal
    /// or a doc comment mints nothing, on both passes and in the post-cache
    /// fold; the same text read as TypeScript (no guard) still mints.
    #[test]
    fn literal_and_comment_sites_mint_nothing_in_rust() {
        let src = "/// Spring's `@EventListener(OrderPlaced)` handler.\nfn f() {\n    let bus = \"emitter.emit('user.created', u);\";\n}\n";
        let qn = |out: EventNodes| {
            let mut v: Vec<String> = out.nav.qname_by_id.into_values().collect();
            v.sort();
            v
        };
        assert_eq!(
            qn(extract_event_emitter_nodes(src, "x.ts", module_id(), repo())),
            vec![s("event_emit:user.created")]
        );
        assert_eq!(
            qn(extract_event_handler_nodes(src, "x.ts", module_id(), repo())),
            vec![s("event_handle:OrderPlaced")]
        );
        assert_eq!(qn(extract_event_emitter_nodes(src, "x.rs", module_id(), repo())), Vec::<String>::new());
        assert_eq!(qn(extract_event_handler_nodes(src, "x.rs", module_id(), repo())), Vec::<String>::new());
        let fold = extract_event_nodes_with_consts(src, "x.rs", module_id(), repo(), &|_| None);
        assert!(fold.emitters.nodes.is_empty() && fold.handlers.nodes.is_empty());
        // A real call after the literal still mints, anchored at the call.
        let real = "let bus = \"emitter.emit('user.created', u);\";\nemitter.emit(\"order.placed\", o);\n";
        assert_eq!(
            qn(extract_event_emitter_nodes(real, "x.rs", module_id(), repo())),
            vec![s("event_emit:order.placed")]
        );
    }

    #[test]
    fn redis_publish_is_not_an_event_emitter() {
        let publisher = "import redis\nr = redis.Redis()\nr.publish('notifications', payload)\n";
        assert!(emitted(publisher).is_empty(), "{:?}", emitted(publisher));
        let subscriber = "import redis\np = redis.Redis().pubsub()\np.subscribe('notifications')\n";
        assert!(handled(subscriber).is_empty(), "{:?}", handled(subscriber));
    }

    #[test]
    fn plain_emitter_still_emits_without_broker_signal() {
        // Regression guard: the same verbs with no broker import are an
        // in-process bus, exactly as before A2.9.
        assert_eq!(
            emitted("bus.publish('user.created', u);"),
            vec!["event_emit:user.created"]
        );
        assert_eq!(
            handled("bus.subscribe('user.created', h);"),
            vec!["event_handle:user.created"]
        );
        assert_eq!(
            handled("emitter.on('user.created', h);"),
            vec!["event_handle:user.created"]
        );
    }

    #[test]
    fn broker_suppression_covers_every_measured_shape() {
        // mqtt.js, both sides, and its `client.on('connect')` lifecycle hook.
        let mqtt_sub = "import mqtt from 'mqtt';\nclient.on('connect', () => { client.subscribe('sensors/temp'); });";
        assert!(handled(mqtt_sub).is_empty(), "{:?}", handled(mqtt_sub));
        assert!(emitted("import mqtt from 'mqtt';\nclient.publish('sensors/temp', v);").is_empty());
        // kafkajs object argument: used to mint the method-named `event_handle:subscribe`.
        let kafkajs =
            "import { Kafka } from 'kafkajs';\nawait consumer.subscribe({ topic: 'orders' });";
        assert!(handled(kafkajs).is_empty(), "{:?}", handled(kafkajs));
        // Spring: a method DECLARATION `publish(` in a broker file.
        let spring = "import org.springframework.kafka.core.KafkaTemplate;\npublic void publish(String p) { kafkaTemplate.send(\"orders\", p); }";
        assert!(emitted(spring).is_empty(), "{:?}", emitted(spring));
        // AWS SDK v2 SQS (the java/sqs_sns cell) and SNS via boto3.
        let sqs = "import software.amazon.awssdk.services.sqs.SqsClient;\npublic void publish(String p) {}";
        assert!(emitted(sqs).is_empty(), "{:?}", emitted(sqs));
        assert!(emitted("import boto3\nsns.publish(TopicArn=arn, Message=m)").is_empty());
        // Google Pub/Sub by package, both sides.
        let gcp = "from google.cloud import pubsub_v1\npublisher.publish(topic_path, data)\nsubscriber.subscribe(path, callback=cb)";
        assert!(emitted(gcp).is_empty() && handled(gcp).is_empty());
    }

    #[test]
    fn in_process_verbs_and_buses_are_never_suppressed() {
        // `.emit(` is not broker-ambiguous: kept even in a broker file.
        assert_eq!(
            emitted("import redis\nemitter.emit('cache.flushed', k);"),
            vec!["event_emit:cache.flushed"]
        );
        // graphql-subscriptions' `PubSub` is an in-process bus; the bare word
        // `pubsub` is deliberately not a broker signal.
        assert_eq!(
            emitted(
                "import { PubSub } from 'graphql-subscriptions';\npubsub.publish('POST_ADDED', p);"
            ),
            vec!["event_emit:POST_ADDED"]
        );
        // A task-queue library is not a broker signal: a bus in a BullMQ file
        // keeps its handler.
        assert_eq!(
            handled("import { Worker } from 'bullmq';\nbus.on('completed', done);"),
            vec!["event_handle:completed"]
        );
        // LA.39: a queue worker's own lifecycle event is not a bus event (like
        // mqtt's `client.on('connect')`, which A2.9 already drops).
        assert_eq!(
            handled("import { Worker } from 'bullmq';\nworker.on('completed', done);"),
            Vec::<String>::new()
        );
        // Type-keyed buses (NestJS CQRS) stay, broker import or not.
        assert_eq!(
            emitted(
                "import { Kafka } from 'kafkajs';\nthis.eventBus.publish(new OrderPlaced(id));"
            ),
            vec!["event_emit:OrderPlaced"]
        );
    }

    #[test]
    fn anchors_type_sites_all_and_string_sites_once() {
        // Two type-keyed publishes of one event: one node, two anchors.
        let src = "class A {\n  void a() { publisher.publishEvent(new OrderPlaced(1)); }\n  void b() {\n    publisher.publishEvent(new OrderPlaced(2));\n  }\n}";
        let out = extract_event_emitter_nodes(src, "", module_id(), repo());
        assert_eq!(out.nodes.len(), 1);
        let id = out.nodes[0].id;
        assert_eq!(
            out.anchors,
            vec![Anchor { node: id, line: 1 }, Anchor { node: id, line: 3 }]
        );

        // String-keyed: the minting site only, at its own line.
        let src = "import x;\nexport function f() {\n  bus.on('user.created', h);\n}";
        let out = extract_event_handler_nodes(src, "", module_id(), repo());
        assert_eq!(out.anchors, vec![Anchor { node: out.nodes[0].id, line: 2 }]);

        // A suppressed broker call mints nothing, so it anchors nothing.
        let src = "import { connect } from 'mqtt';\nclient.subscribe('t');";
        let out = extract_event_handler_nodes(src, "", module_id(), repo());
        assert!(out.nodes.is_empty() && out.anchors.is_empty());
    }

    // ---- LA.29: the event-bus verbs need a bus ----------------------------

    #[test]
    fn a_function_named_publish_is_not_an_emitter() {
        for src in [
            // Python: a method, a module function, a call on a non-bus.
            "class Notifier:\n    def publish(self, m):\n        print(m)\n\ndef publish(report):\n    return report.render()\n\ndef run(notifier, report):\n    notifier.publish(x)\n",
            "function publish(msg) {}",
            "class N {\n  publish(msg: string): void {\n    console.log(msg);\n  }\n}",
            "func (n *Notifier) publish(m string) {\n\tfmt.Println(m)\n}",
            "impl Notifier {\n    fn publish(&self) {}\n}",
        ] {
            assert_eq!(emitted(src), Vec::<String>::new(), "{src}");
        }
    }

    #[test]
    fn first_qualifying_publish_wins() {
        // HEAD took the first `publish(` (the def) and minted event_emit:publish.
        let src = "def publish(self, m): pass\nbus.publish('user.created', u)";
        assert_eq!(emitted(src), vec!["event_emit:user.created"]);
        let out = extract_event_emitter_nodes(src, "", module_id(), repo());
        assert_eq!(out.anchors, vec![Anchor { node: out.nodes[0].id, line: 1 }]);
    }

    #[test]
    fn bus_shaped_receivers_still_emit() {
        assert_eq!(emitted("this.eventBus.publish('x');"), vec!["event_emit:x"]);
        assert_eq!(
            emitted("vertx.eventBus().publish(\"addr\", m);"),
            vec!["event_emit:addr"]
        );
        assert_eq!(
            emitted("ActiveSupport::Notifications.publish('render', p)"),
            vec!["event_emit:render"]
        );
        assert_eq!(
            emitted("this.events.publish('user:login', u);"),
            vec!["event_emit:user:login"]
        );
    }

    #[test]
    fn import_admits_other_receivers() {
        assert_eq!(
            emitted(
                "import { PubSub } from 'graphql-subscriptions';\nconst ps = new PubSub();\nps.publish('POST_ADDED', p);"
            ),
            vec!["event_emit:POST_ADDED"]
        );
        let wisper = "class PlaceOrder\n  include Wisper::Publisher\n  def call\n    publish('order_placed', self)\n  end\nend\n";
        assert_eq!(emitted(wisper), vec!["event_emit:order_placed"]);
    }

    #[test]
    fn typed_publish_mints_no_fallback() {
        // No broker import: HEAD also minted event_emit:publish from the
        // string pass's fallback on the same call.
        assert_eq!(
            emitted("this.eventBus.publish(new OrderPlaced(id));"),
            vec!["event_emit:OrderPlaced"]
        );
    }

    #[test]
    fn rxjs_subscribe_is_not_a_handler() {
        for src in [
            "this.route.params.subscribe(p => this.load(p));",
            "obs$.subscribe(x);",
            "this.http.get(u).subscribe(r => {});",
            "let mut rx = tx.subscribe();",
            // A call returning notifications is a fetch, not a bus (quokka-stack).
            "this.notificationService.fetchNotifications().subscribe({ error: () => void 0 });",
        ] {
            assert_eq!(handled(src), Vec::<String>::new(), "{src}");
        }
    }

    #[test]
    fn bus_subscribe_still_handles() {
        assert_eq!(
            handled("PubSub.subscribe('MY TOPIC', fn);"),
            vec!["event_handle:MY TOPIC"]
        );
        assert_eq!(
            handled("pubsub.subscribe('POST_ADDED', h);"),
            vec!["event_handle:POST_ADDED"]
        );
    }

    #[test]
    fn aws_sdk_commands_are_not_events() {
        let aws = "import { DynamoDBClient, PutItemCommand } from '@aws-sdk/client-dynamodb';\nawait ddb.send(new PutItemCommand({ TableName: 'orders' }));";
        assert_eq!(emitted(aws), Vec::<String>::new());
        // Split so glia's own build does not read this file's test data as a
        // MediatR send (the type needle wants the type right after `new `).
        let mediatr = concat!("_mediator.Send(new ", "CreateOrder(id));");
        assert_eq!(emitted(mediatr), vec!["event_emit:CreateOrder"]);
    }

    #[test]
    fn calls_in_conditions_are_not_declarations() {
        assert_eq!(
            emitted("import PubSub from 'pubsub-js';\nif (publish('x', data)) {}"),
            vec!["event_emit:x"]
        );
        let wisper = "include Wisper::Publisher\ndef call\n  return publish('order_placed', self)\nend\n";
        assert_eq!(emitted(wisper), vec!["event_emit:order_placed"]);
        // Declarations stay declarations with the import present.
        let java = "import PubSub from 'pubsub-js';\ninterface Notifier {\n  void publish(String p);\n}";
        assert_eq!(emitted(java), Vec::<String>::new());
        let dart = "import PubSub from 'pubsub-js';\nFuture<void> publish(String m) async {\n  print(m);\n}";
        assert_eq!(emitted(dart), Vec::<String>::new());
    }

    // ---- LA.41: event names are name-shaped -------------------------------
    // Needles in this test data are split with `concat!` so glia's own build
    // does not read them as event sites in this file.

    #[test]
    fn malformed_literal_occurrence_is_skipped() {
        // A Python needle table. HEAD minted event_handle:`, `,
        // event_handle:`,\n    ` and (the literal never closes on its line)
        // the verb fallback event_emit:Subject.next.
        let table = concat!(
            "EVENT_NEEDLES = [\n    \"@On",
            "Event(\", \"@Event",
            "Pattern(\",\n    \"Subject",
            ".next(\",\n]"
        );
        assert_eq!(handled(table), Vec::<String>::new());
        assert_eq!(emitted(table), Vec::<String>::new());
    }

    #[test]
    fn later_valid_occurrence_wins() {
        // HEAD took the first occurrence and minted event_handle:`, ` on line 0.
        let src = concat!(
            "const table = [\"@On",
            "Event(\", \"x\"];\n@On",
            "Event('order.shipped')\nonShipped() {}"
        );
        assert_eq!(handled(src), vec!["event_handle:order.shipped"]);
        let out = extract_event_handler_nodes(src, "", module_id(), repo());
        assert_eq!(out.anchors, vec![Anchor { node: out.nodes[0].id, line: 1 }]);
    }

    #[test]
    fn literal_must_close_on_its_line() {
        // HEAD: event_handle:`order.\nshipped`.
        assert_eq!(
            handled(concat!("@On", "Event('order.\nshipped')")),
            Vec::<String>::new()
        );
        assert_eq!(
            handled(concat!("@On", "Event('order.\r\nshipped')")),
            Vec::<String>::new()
        );
        // Line breaks BEFORE the literal are fine.
        assert_eq!(
            handled(concat!("@On", "Event(\n  'order.shipped'\n)")),
            vec!["event_handle:order.shipped"]
        );
        // The three outcomes, at the byte just past a needle's `(`.
        assert_eq!(
            literal_after("('order.shipped')", 1),
            LiteralAt::Name("order.shipped".into())
        );
        assert_eq!(
            literal_after("(\"pedido.criado\", x)", 1),
            LiteralAt::Name("pedido.criado".into())
        );
        assert_eq!(literal_after("('order.shipped", 1), LiteralAt::Malformed);
        assert_eq!(literal_after("('', x)", 1), LiteralAt::Malformed);
        assert_eq!(literal_after("(`order.shipped`)", 1), LiteralAt::Absent);
        assert_eq!(literal_after("(name, x)", 1), LiteralAt::Absent);
        assert_eq!(literal_after("x", 5), LiteralAt::Absent);
        // Multi-byte text before and inside the literal slices cleanly.
        assert_eq!(
            literal_after("(  'événement.créé')", 1),
            LiteralAt::Name("événement.créé".into())
        );
    }

    #[test]
    fn event_name_shape() {
        for ok in [
            "user.created",
            "order.placed",
            "user:login",
            "POST_ADDED",
            "sensors/temp",
            "cache.flushed",
            "update:modelValue",
            "order.*",
            "MY TOPIC",
            "some event",
            "_internal",
            "$destroy",
            "@app/ready",
            "#channel",
            "a+b",
            "注文.確定",
        ] {
            assert!(is_event_name(ok), "{ok:?} should be a name");
        }
        let too_long = "e".repeat(129);
        for bad in [
            "",
            ", ",
            ", true, false),\n    (",
            ",\n    ",
            ") {",
            " x",
            "x ",
            "x  y",
            "--help",
            "*",
            "{0}.done",
            "${id}",
            "a\tb",
            "user created!",
            too_long.as_str(),
        ] {
            assert!(!is_event_name(bad), "{bad:?} should not be a name");
        }
        assert!(is_event_name(&"e".repeat(128)));
    }

    // ---- LA.39: DOM / store / Node event verbs need an in-process bus -----

    #[test]
    fn dom_and_library_listeners_are_not_bus_handlers() {
        // HEAD minted each: zoomend, click, scroll, change, SIGINT, data,
        // message, addListener.
        for src in [
            "this.map.on('zoomend', f);",
            "el.addEventListener('click', h);",
            "window.addEventListener('scroll', h);",
            "$(form).on('change', v);",
            "process.on('SIGINT', h);",
            "req.on('data', h);",
            "socket.on('message', h);",
            "_controller.addListener(cb);",
        ] {
            assert_eq!(handled(src), Vec::<String>::new(), "{src}");
        }
    }

    #[test]
    fn dom_and_store_emits_are_not_bus_emits() {
        // HEAD: dispatch, submit, dispatchEvent, trigger, chat, emit.
        for src in [
            "this.store.dispatch({ type: 'X' });",
            "$(form).trigger('submit');",
            "window.dispatchEvent(new Event('resize'));",
            "debouncer.trigger();",
            "socket.emit('chat', m);",
            "import { Component, EventEmitter, Output } from '@angular/core';\nexport class A {\n  @Output() picked = new EventEmitter<string>();\n  pick(u: string) { this.picked.emit(u); }\n}",
            // A verb on the emitter CLASS names no bus instance (HEAD: emit).
            "// Angular @Output: EventEmitter.emit(value) pushes a component output.",
        ] {
            assert_eq!(emitted(src), Vec::<String>::new(), "{src}");
        }
    }

    #[test]
    fn bus_receivers_keep_every_verb() {
        assert_eq!(handled("bus.on('a', h);"), vec!["event_handle:a"]);
        assert_eq!(
            handled("this.eventBus.addListener('a', h);"),
            vec!["event_handle:a"]
        );
        assert_eq!(emitted("emitter.emit('a');"), vec!["event_emit:a"]);
        assert_eq!(emitted("this.events.trigger('a');"), vec!["event_emit:a"]);
        assert_eq!(
            emitted("eventDispatcher.dispatch('a', e);"),
            vec!["event_emit:a"]
        );
        assert_eq!(
            handled("bus.addEventListener('a', h);"),
            vec!["event_handle:a"]
        );
        assert_eq!(
            handled("@emitter.on(\"user_created\")"),
            vec!["event_handle:user_created"]
        );
    }

    #[test]
    fn bound_emitters_count() {
        let node = "import { EventEmitter } from 'node:events';\nconst jobs = new EventEmitter();\njobs.on('drained', r);\njobs.emit('drained');";
        assert_eq!(handled(node), vec!["event_handle:drained"]);
        assert_eq!(emitted(node), vec!["event_emit:drained"]);
        assert_eq!(
            handled("import mitt from 'mitt';\nconst m = mitt();\nm.on('x', f);"),
            vec!["event_handle:x"]
        );
        // The platform emitter needs no import.
        assert_eq!(
            handled("const target = new EventTarget();\ntarget.addEventListener('ready', f);"),
            vec!["event_handle:ready"]
        );
        // A typed class field, reached through `this.`.
        assert_eq!(
            emitted(
                "import { EventEmitter } from 'events';\nclass S { private hub: EventEmitter = new EventEmitter(); go() { this.hub.emit('go'); } }"
            ),
            vec!["event_emit:go"]
        );
        assert_eq!(
            emitted("from pyee import EventEmitter\nee = EventEmitter()\nee.emit('started')"),
            vec!["event_emit:started"]
        );
        // A capitalised instance is bound, not the class.
        assert_eq!(
            emitted(
                "import { EventEmitter } from 'events';\nexport const Emitter = new EventEmitter();\nEmitter.emit('ready');"
            ),
            vec!["event_emit:ready"]
        );
        // HEAD: event_handle:drained. No emitter-library import.
        assert_eq!(
            handled("const jobs = new EventEmitter();\njobs.on('drained', r);"),
            Vec::<String>::new()
        );
        // A quoted `events` that is not an import does not make one.
        assert_eq!(
            handled(
                "const jobs = new EventEmitter();\njobs.on('drained', r);\nconst c = db.collection('events');"
            ),
            Vec::<String>::new()
        );
    }

    #[test]
    fn emitter_subclass_this() {
        assert_eq!(
            emitted(
                "import { EventEmitter } from 'events';\nexport class Uploader extends EventEmitter {\n  finish() { this.emit('uploaded'); }\n}"
            ),
            vec!["event_emit:uploaded"]
        );
        // HEAD: event_emit:x.
        assert_eq!(
            emitted("class A { go() { this.emit('x'); } }"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn nest_microservice_client_emit() {
        assert_eq!(
            emitted(
                "import { ClientProxy } from '@nestjs/microservices';\nthis.client.emit('user_created', u);"
            ),
            vec!["event_emit:user_created"]
        );
        // HEAD: event_emit:user_created.
        assert_eq!(
            emitted("this.client.emit('user_created', u);"),
            Vec::<String>::new()
        );
        // A verb on the emitter class is no client call either, and a needle
        // with no receiver identifier (a string table) is no call at all.
        for src in [
            "import { ClientProxy } from '@nestjs/microservices';\n// EventEmitter.emit('x') is not this",
            "import { ClientProxy } from '@nestjs/microservices';\nconst verbs = ['.emit(', 'x'];",
        ] {
            assert_eq!(emitted(src), Vec::<String>::new(), "{src}");
        }
    }

    #[test]
    fn first_bus_site_wins() {
        // HEAD took the first `.addEventListener(` and minted event_handle:click.
        let src = "el.addEventListener('click', h);\nbus.addEventListener('user.created', h2);";
        assert_eq!(handled(src), vec!["event_handle:user.created"]);
        let out = extract_event_handler_nodes(src, "", module_id(), repo());
        assert_eq!(out.anchors, vec![Anchor { node: out.nodes[0].id, line: 1 }]);
    }

    #[test]
    fn bare_dispatch_event_has_no_receiver() {
        // HEAD: event_emit:dispatchEvent for both.
        for src in [
            "dispatchEvent(new CustomEvent('x'));",
            "dispatchEvent(event: Event): boolean { return true; }",
        ] {
            assert_eq!(emitted(src), Vec::<String>::new(), "{src}");
        }
    }

    /// The ORIGIN payloads on each node of an extract, by qname, sorted.
    fn origins(out: &EventNodes) -> Vec<(String, Vec<String>)> {
        let mut v: Vec<(String, Vec<String>)> = out
            .nodes
            .iter()
            .map(|n| {
                let q = out.nav.qname_by_id.get(&n.id).cloned().unwrap_or_default();
                let cells = n
                    .cells
                    .iter()
                    .filter(|c| c.kind == cell_type::ORIGIN)
                    .map(|c| match &c.payload {
                        CellPayload::Json(j) => j.clone(),
                        other => format!("{other:?}"),
                    })
                    .collect();
                (q, cells)
            })
            .collect();
        v.sort();
        v
    }

    fn transport(via: &str) -> Vec<String> {
        vec![format!(r#"{{"provenance":"synthetic","delivery":"transport","via":"{via}"}}"#)]
    }

    /// LB.8b: a transport needle marks the node it mints (or re-sees) with the
    /// transport ORIGIN; an in-process bus gets no ORIGIN from the extractor
    /// (the engine's synthetic tag adds the plain one later).
    #[test]
    fn transport_needles_mark_their_nodes() {
        let out = extract_event_handler_nodes(
            "@EventPattern('order_shipped')\nasync onShipped(data) {}",
            "",
            module_id(),
            repo(),
        );
        assert_eq!(
            origins(&out),
            vec![(s("event_handle:order_shipped"), transport("nestjs-microservices"))]
        );

        let client = "import { ClientProxy } from '@nestjs/microservices';\nthis.client.emit('order_shipped', o);";
        let out = extract_event_emitter_nodes(client, "", module_id(), repo());
        assert_eq!(
            origins(&out),
            vec![(s("event_emit:order_shipped"), transport("nestjs-microservices"))]
        );

        // CB.3a: a putEvents names its entry's DetailType; with none it names
        // nothing (HEAD: event_emit:eventBridge.putEvents).
        let bridge = "await eventBridge.putEvents({ Entries: [] }).promise();";
        let out = extract_event_emitter_nodes(bridge, "", module_id(), repo());
        assert_eq!(origins(&out), vec![]);
        let bridge = "await eventBridge.putEvents({ Entries: [{ DetailType: \"OrderPlaced\" }] }).promise();";
        let out = extract_event_emitter_nodes(bridge, "", module_id(), repo());
        assert_eq!(
            origins(&out),
            vec![(s("event_emit:OrderPlaced"), transport("aws-eventbridge"))]
        );

        let local = "import { EventEmitter } from 'events';\nconst bus = new EventEmitter();\nbus.emit('x', 1);";
        let out = extract_event_emitter_nodes(local, "", module_id(), repo());
        assert_eq!(origins(&out), vec![(s("event_emit:x"), vec![])], "in-process: no ORIGIN");
        let out = extract_event_handler_nodes("bus.on('x', h);\n@OnEvent('y')\nh2() {}", "", module_id(), repo());
        assert_eq!(
            origins(&out),
            vec![(s("event_handle:x"), vec![]), (s("event_handle:y"), vec![])],
            "@OnEvent is the in-process EventEmitter2 handler"
        );
    }

    /// The string pass re-sees a name an earlier needle minted: the ONE node
    /// is marked, once.
    #[test]
    fn a_transport_needle_marks_a_node_an_in_process_needle_minted() {
        let src = "bus.on('x', h);\n@EventPattern('x')\nonX(data) {}";
        let out = extract_event_handler_nodes(src, "", module_id(), repo());
        assert_eq!(origins(&out), vec![(s("event_handle:x"), transport("nestjs-microservices"))]);
        assert_eq!(out.nodes.len(), 1);
    }

    fn s(v: &str) -> String {
        v.to_string()
    }

    // ---- CB.3a: a site reads a real name or mints nothing ----------------
    // Needles in this test data are split with `concat!` (LA.41's rule) so
    // glia's own build does not read them as event sites in this file.

    #[test]
    fn constant_reference_names_both_sides() {
        // HEAD: event_emit:emit and event_handle:@OnEvent, which never pair.
        assert_eq!(
            emitted(concat!("this.eventEmitter.em", "it(OrderEvents.Created, {id})")),
            vec!["event_emit:OrderEvents.Created"]
        );
        assert_eq!(
            handled(concat!("@On", "Event(OrderEvents.Created)\naudit(p) {}")),
            vec!["event_handle:OrderEvents.Created"]
        );
        // Across lines, `::` paths, and up to four segments, kept verbatim.
        assert_eq!(
            handled(concat!("@On", "Event(\n  OrderEvents.Created\n)")),
            vec!["event_handle:OrderEvents.Created"]
        );
        assert_eq!(
            emitted(concat!("bus.pub", "lish(Events::ORDER_PLACED, order)")),
            vec!["event_emit:Events::ORDER_PLACED"]
        );
        assert_eq!(
            emitted(concat!("bus.em", "it(Shop.Orders.Events.Created, o)")),
            vec!["event_emit:Shop.Orders.Events.Created"]
        );
        // The path at the byte just past a needle's `(`.
        assert_eq!(
            constant_ref_after("(OrderEvents.Created)", 1),
            Some(s("OrderEvents.Created"))
        );
        assert_eq!(
            constant_ref_after("(  Events::Placed , x)", 1),
            Some(s("Events::Placed"))
        );
    }

    #[test]
    fn upper_snake_constant() {
        assert_eq!(
            emitted(concat!("bus.em", "it(ORDER_PLACED, x)")),
            vec!["event_emit:ORDER_PLACED"]
        );
        assert_eq!(
            handled(concat!("bus.o", "n(V2_READY, h)")),
            vec!["event_handle:V2_READY"]
        );
        assert_eq!(constant_ref_after("(ORDER_PLACED)", 1), Some(s("ORDER_PLACED")));
        // A collection noun is not a bus name here: `ORDER_EVENTS` is a topic.
        assert_eq!(constant_ref_after("(ORDER_EVENTS, x)", 1), Some(s("ORDER_EVENTS")));
        // No letter: not a constant.
        assert_eq!(constant_ref_after("(1_000)", 1), None);
    }

    #[test]
    fn a_value_argument_mints_nothing() {
        // HEAD: event_emit:Subject.next, event_emit:emit, event_handle:@OnEvent.
        assert_eq!(
            emitted(concat!("this.itemsSubj", "ect.next(items);")),
            Vec::<String>::new()
        );
        assert_eq!(emitted(concat!("emitter.em", "it(evt);")), Vec::<String>::new());
        assert_eq!(
            handled(concat!("@On", "Event(name)\nh() {}")),
            Vec::<String>::new()
        );
        // The topic is Phoenix.PubSub's SECOND argument (HEAD:
        // event_handle:subscribe, the CF.11b elixir/eventbus probe).
        assert_eq!(
            handled(concat!("Phoenix.PubSub.subsc", "ribe(Shop.PubSub, \"order_placed\")")),
            Vec::<String>::new()
        );
        for arg in [
            "this.x, 1",
            "payload.type, p",
            "Foo.bar(), 1",
            "name(), 1",
            "OrderPlaced, 1",
            "`order.${id}`, 1",
            "1",
            "1.5, x",
            "A.B.C.D.E, 1",
            "OrderEvents.Created as string, x",
            "OrderEvents?.Created, x",
            "Events[0], x",
            // The bus itself, not an event: Phoenix.PubSub's server argument.
            "Shop.PubSub, \"orders\"",
            "App::EventBus, x",
            "...args",
            "{ type: 'x' }",
            "",
        ] {
            assert_eq!(constant_ref_after(&format!("({arg})"), 1), None, "{arg}");
        }
    }

    #[test]
    fn eventbridge_detail_type() {
        let bridge = concat!(
            "await eventBridge.put",
            "Events({ Entries: [{ Source: \"shop\", DetailType: \"OrderPlaced\" }] }).promise();"
        );
        assert_eq!(emitted(bridge), vec!["event_emit:OrderPlaced"]);
        // The capitalised receiver, a quoted key, a `)` inside an earlier
        // string, and the key on a later line.
        for src in [
            concat!("EventBridge.put", "Events({ Entries: [{ DetailType: 'OrderPlaced' }] });"),
            concat!("eventBridge.put", "Events({ \"Entries\": [{ \"DetailType\": \"OrderPlaced\" }] });"),
            concat!("eventBridge.put", "Events({ Entries: [{ Source: \"a)b\", DetailType: \"OrderPlaced\" }] });"),
            concat!("eventBridge.put", "Events({\n  Entries: [{\n    DetailType: \"OrderPlaced\",\n  }],\n});"),
        ] {
            assert_eq!(emitted(src), vec!["event_emit:OrderPlaced"], "{src}");
        }
        // boto3's put_events is not a needle; no DetailType, a variable
        // DetailType, a DetailType outside the call and a needle that is no
        // call all name nothing (HEAD: event_emit:eventBridge.putEvents).
        for src in [
            "events.put_events(Entries=[{'Source': 'shop', 'DetailType': 'OrderPlaced'}])",
            concat!("await eventBridge.put", "Events({ Entries: [] }).promise();"),
            concat!("await eventBridge.put", "Events(params).promise();"),
            concat!("eventBridge.put", "Events({ Entries: [{ DetailType: kind }] });"),
            concat!("eventBridge.put", "Events(p);\nconst e = { DetailType: \"Other\" };"),
            concat!("// announce with eventBridge.put", "Events, DetailType: \"Other\""),
            concat!("eventBridge.put", "Events({ Entries: [{ MyDetailType: \"Other\" }] });"),
        ] {
            assert_eq!(emitted(src), Vec::<String>::new(), "{src}");
        }
        // Python keyword arguments read too.
        assert_eq!(
            detail_type_in("(Entries=[dict(Source='s', DetailType='OrderPlaced')])", 0),
            LiteralAt::Name(s("OrderPlaced"))
        );
        assert_eq!(detail_type_in("({ DetailType: 'a\nb' })", 0), LiteralAt::Malformed);
    }

    #[test]
    fn liveview_handle_event() {
        let live = concat!(
            "defmodule ShopWeb.CounterLive do\n  use Phoenix.LiveView\n\n  def handle",
            "_event(\"inc\", _params, socket) do\n    {:noreply, socket}\n  end\nend\n"
        );
        assert_eq!(handled(live), vec!["event_handle:inc"]);
        // HEAD: event_handle:handle_event for each.
        for src in [
            concat!("class Listener:\n    def handle", "_event(self, event):\n        pass\n"),
            concat!("self.handle", "_event(event)"),
            concat!("def handle", "_event(event)\n  log(event)\nend"),
        ] {
            assert_eq!(handled(src), Vec::<String>::new(), "{src}");
        }
    }

    #[test]
    fn first_named_occurrence_wins() {
        // HEAD: event_emit:emit from the first occurrence.
        let src = concat!("bus.em", "it(x);\nbus.em", "it(\"user.created\", u);");
        assert_eq!(emitted(src), vec!["event_emit:user.created"]);
        let out = extract_event_emitter_nodes(src, "", module_id(), repo());
        assert_eq!(out.anchors, vec![Anchor { node: out.nodes[0].id, line: 1 }]);
        // A constant reference is a name like a literal.
        let src = concat!("bus.em", "it(x);\nbus.em", "it(OrderEvents.Created, u);");
        assert_eq!(emitted(src), vec!["event_emit:OrderEvents.Created"]);
    }

    #[test]
    fn malformed_literal_still_skips() {
        // LA.41 unchanged: a malformed literal is not a site, and it never
        // falls back to a constant or the verb.
        for src in [
            concat!("bus.em", "it(', ', x);"),
            concat!("bus.em", "it('order.\nshipped');"),
            concat!("bus.em", "it('order.shipped"),
        ] {
            assert_eq!(emitted(src), Vec::<String>::new(), "{src}");
        }
        let src = concat!("bus.em", "it(', ');\nbus.em", "it(OrderEvents.Created, x);");
        assert_eq!(emitted(src), vec!["event_emit:OrderEvents.Created"]);
        assert_eq!(event_name_at("(', ')", "", 1, NameRule::Literal), SiteName::Malformed);
        assert_eq!(event_name_at("(x)", "", 1, NameRule::Literal), SiteName::Unnamed);
        assert_eq!(
            event_name_at("(X.Y)", "", 1, NameRule::Literal),
            SiteName::Constant(s("X.Y"))
        );
        assert_eq!(
            event_name_at("('a.b')", "", 1, NameRule::Literal),
            SiteName::Literal(s("a.b"))
        );
    }

    // ---- CB.3b: a constant-keyed site folds to its resolved literal -------
    // Needles split with `concat!`, as above.

    /// A resolver over a fixed table, the engine's shape.
    fn table(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |expr: &str| {
            pairs
                .iter()
                .find(|(k, _)| *k == expr)
                .map(|(_, v)| v.to_string())
        }
    }

    fn folded(src: &str, pairs: &'static [(&'static str, &'static str)]) -> EventFold {
        extract_event_nodes_with_consts(src, PATH, module_id(), repo(), &table(pairs))
    }

    fn qnames(out: &EventNodes) -> Vec<String> {
        let mut v: Vec<String> = out.nav.qname_by_id.values().cloned().collect();
        v.sort();
        v
    }

    #[test]
    fn constant_sites_fold_on_both_sides() {
        let src = concat!(
            "this.eventEmitter.em", "it(OrderEvents.Created, { id });\n",
            "@On", "Event(OrderEvents.Paid)\nonPaid(p) {}\n",
            "@On", "Event(ORDER_PLACED)\nonPlaced(p) {}\n"
        );
        let fold = folded(
            src,
            &[
                ("OrderEvents.Created", "order.created"),
                ("OrderEvents.Paid", "order.paid"),
                ("ORDER_PLACED", "order.placed"),
            ],
        );
        assert_eq!(qnames(&fold.emitters), vec!["event_emit:order.created"]);
        // `@OnEvent(` is one needle: its first named site wins (CB.3a's rule).
        assert_eq!(qnames(&fold.handlers), vec!["event_handle:order.paid"]);
        assert_eq!(fold.counts, ConstFoldCounts { folded: 2, unresolved: 0 });
        assert_eq!(fold.path, PATH);
        // `Events::ORDER_PLACED` reaches the resolver as written.
        let fold = folded(
            concat!("bus.pub", "lish(Events::ORDER_PLACED, order)"),
            &[("Events::ORDER_PLACED", "order.placed")],
        );
        assert_eq!(qnames(&fold.emitters), vec!["event_emit:order.placed"]);
    }

    #[test]
    fn resolver_none_is_cb3a_output() {
        // A resolver that names nothing re-emits exactly the per-file pass's
        // nodes (the constant path is the fallback), counting each constant
        // site unresolved; the per-file pass itself counts nothing.
        for (src, constants) in [
            (concat!("this.eventEmitter.em", "it(OrderEvents.Created, {id})"), 1),
            (concat!("@On", "Event(OrderEvents.Created)\naudit(p) {}"), 1),
            (concat!("bus.em", "it('user.created', u);\nbus.o", "n(V2_READY, h)"), 1),
            (
                concat!(
                    "import { ClientProxy } from '@nestjs/microservices';\nthis.client.em",
                    "it(Topics.ORDER, o);\n@Event",
                    "Pattern(Topics.ORDER)\nh(d) {}"
                ),
                2,
            ),
            (concat!("publisher.publishEv", "ent(new OrderPlacedEvent(id));"), 0),
        ] {
            let fold = extract_event_nodes_with_consts(src, PATH, module_id(), repo(), &|_| None);
            let emitters = extract_event_emitter_nodes(src, "", module_id(), repo());
            let handlers = extract_event_handler_nodes(src, "", module_id(), repo());
            assert_eq!(fold.emitters.nodes, emitters.nodes, "{src}");
            assert_eq!(fold.emitters.anchors, emitters.anchors, "{src}");
            assert_eq!(fold.emitters.nav.qname_by_id, emitters.nav.qname_by_id, "{src}");
            assert_eq!(fold.handlers.nodes, handlers.nodes, "{src}");
            assert_eq!(fold.handlers.anchors, handlers.anchors, "{src}");
            assert_eq!(fold.handlers.nav.qname_by_id, handlers.nav.qname_by_id, "{src}");
            assert_eq!(
                fold.counts,
                ConstFoldCounts { folded: 0, unresolved: constants },
                "{src}"
            );
        }
    }

    #[test]
    fn a_quoted_literal_is_never_re_resolved() {
        let src = concat!(
            "bus.em", "it('order.created', x);\n",
            "@On", "Event(\"order.created\")\nh(p) {}"
        );
        let fold = folded(src, &[("order.created", "other.event")]);
        assert_eq!(qnames(&fold.emitters), vec!["event_emit:order.created"]);
        assert_eq!(qnames(&fold.handlers), vec!["event_handle:order.created"]);
        assert_eq!(fold.counts, ConstFoldCounts::default());
        // A variable argument is no site, so the resolver never sees it.
        let fold = folded(concat!("bus.em", "it(evt, x);"), &[("evt", "order.created")]);
        assert!(fold.emitters.nodes.is_empty());
        assert_eq!(fold.counts, ConstFoldCounts::default());
    }

    #[test]
    fn a_rejected_value_keeps_the_path() {
        // LA.41's name rule refuses the value (a comma, a doubled space, an
        // empty string): the site keeps its constant path and counts
        // unresolved. A single inner space (`MY TOPIC`) is a name.
        for value in ["a, b", "a  b", ""] {
            let resolve = move |_: &str| Some(value.to_string());
            let fold = extract_event_nodes_with_consts(
                concat!("bus.em", "it(OrderEvents.Created, x);"),
                PATH,
                module_id(),
                repo(),
                &resolve,
            );
            assert_eq!(qnames(&fold.emitters), vec!["event_emit:OrderEvents.Created"], "{value:?}");
            assert_eq!(fold.counts, ConstFoldCounts { folded: 0, unresolved: 1 }, "{value:?}");
        }
        let fold = folded(
            concat!("bus.em", "it(OrderEvents.Created, x);"),
            &[("OrderEvents.Created", "MY TOPIC")],
        );
        assert_eq!(qnames(&fold.emitters), vec!["event_emit:MY TOPIC"]);
    }

    #[test]
    fn a_suppressed_site_is_not_counted() {
        // A2.9: `publish(` in a broker file is queue traffic; the fold never
        // reaches it, so it counts neither way.
        let src = concat!(
            "import { connect } from 'nats';\nbus.pub",
            "lish(Topics.ORDER, o);"
        );
        let fold = folded(src, &[("Topics.ORDER", "order")]);
        assert!(fold.emitters.nodes.is_empty());
        assert_eq!(fold.counts, ConstFoldCounts::default());
    }

    #[test]
    fn a_transport_site_keeps_its_mark_when_folded() {
        let src = concat!(
            "import { ClientProxy } from '@nestjs/microservices';\nthis.client.em",
            "it(Topics.ORDER, o);"
        );
        let fold = folded(src, &[("Topics.ORDER", "order_shipped")]);
        assert_eq!(
            origins(&fold.emitters),
            vec![(s("event_emit:order_shipped"), transport("nestjs-microservices"))]
        );
    }

    // ---- CB.3b: replace_event_nodes over the shared marker swap ----------

    const PATH: &str = "src/orders.ts";

    fn function_id() -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "test::create")
    }

    /// The engine's `merge_nav`, for the hand-assembled parse.
    fn merge(dst: &mut CodeNav, src: CodeNav) {
        dst.name_by_id.extend(src.name_by_id);
        dst.qname_by_id.extend(src.qname_by_id);
        dst.kind_by_id.extend(src.kind_by_id);
        dst.parent_of.extend(src.parent_of);
        for (k, v) in src.children_of {
            dst.children_of.entry(k).or_default().extend(v);
        }
    }

    /// The per-file pass over `src`: a MODULE, a FUNCTION spanning line 1
    /// (0-indexed), the two event extractors, the anchor pass and then the
    /// router's IMPORTS cell, as `apply_cross_cutting_extractors` and the
    /// router lay them out.
    fn event_parse(src: &str) -> FileParse {
        let module = module_id();
        let func = function_id();
        let node = |id, cells| Node {
            id,
            repo: repo(),
            confidence: Confidence::Strong,
            cells,
        };
        let span = Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(format!(
                r#"{{"file":"{PATH}","start_line":1,"end_line":1}}"#
            )),
        };
        let mut fp = FileParse {
            nodes: vec![node(module, vec![]), node(func, vec![span])],
            ..Default::default()
        };
        fp.nav.record(module, "test", "test", node_kind::MODULE, None);
        fp.nav
            .record(func, "create", "test::create", node_kind::FUNCTION, Some(module));
        let mut anchors = Vec::new();
        for out in [
            extract_event_emitter_nodes(src, "", module, repo()),
            extract_event_handler_nodes(src, "", module, repo()),
        ] {
            fp.nodes.extend(out.nodes);
            anchors.extend(out.anchors);
            merge(&mut fp.nav, out.nav);
        }
        crate::anchor::attach(&mut fp, PATH, module, &mut anchors);
        fp.imports.push(glia_code_domain::ImportStmt {
            from_module: "test".into(),
            target: glia_code_domain::ImportTarget::Module {
                path: "@nestjs/event-emitter".into(),
                alias: None,
            },
            line: 0,
        });
        glia_code_domain::attach_imports_cell(&mut fp, "typescript");
        fp
    }

    /// The constant spelling and the literal spelling of one file: the
    /// emitter inside the function (line 1), the handler at module scope.
    const CONST_SRC: &str = concat!(
        "import { OnEvent } from '@nestjs/event-emitter';\n",
        "  this.eventEmitter.em", "it(OrderEvents.Created, { id });\n",
        "@On", "Event(OrderEvents.Paid)\n"
    );
    const LITERAL_SRC: &str = concat!(
        "import { OnEvent } from '@nestjs/event-emitter';\n",
        "  this.eventEmitter.em", "it('order.created', { id });\n",
        "@On", "Event('order.paid')\n"
    );
    const BINDINGS: &[(&str, &str)] = &[
        ("OrderEvents.Created", "order.created"),
        ("OrderEvents.Paid", "order.paid"),
    ];

    type Triple = (NodeId, NodeId, glia_core::EdgeCategoryId);

    fn triples(fp: &FileParse) -> Vec<Triple> {
        fp.edges.iter().map(|e| (e.from, e.to, e.category)).collect()
    }

    #[test]
    fn replace_event_nodes_lays_out_as_the_literal_file() {
        use glia_code_domain::{edge_category, evidence};
        let mut fp = event_parse(CONST_SRC);
        let old_emit = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::EVENT_EMITTER,
            "event_emit:OrderEvents.Created",
        );
        assert!(fp.nav.kind_by_id.contains_key(&old_emit));
        let fold = folded(CONST_SRC, BINDINGS);
        assert_eq!(fold.counts, ConstFoldCounts { folded: 2, unresolved: 0 });
        replace_event_nodes(&mut fp, module_id(), "typescript", fold);
        let literal = event_parse(LITERAL_SRC);

        // Nodes (cells in the per-file order: POSITION, then IMPORTS), nav
        // and child order are the literal file's.
        assert_eq!(fp.nodes, literal.nodes);
        assert_eq!(fp.nav.qname_by_id, literal.nav.qname_by_id);
        assert_eq!(fp.nav.name_by_id, literal.nav.name_by_id);
        assert_eq!(fp.nav.kind_by_id, literal.nav.kind_by_id);
        assert_eq!(fp.nav.parent_of, literal.nav.parent_of);
        assert_eq!(fp.nav.children_of, literal.nav.children_of);
        // The emitter is USED by the function; the module-scope handler takes
        // the module CONTAINS fallback; nothing names a gone id.
        let emit = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::EVENT_EMITTER, "event_emit:order.created");
        let handle = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::EVENT_HANDLER, "event_handle:order.paid");
        assert_eq!(triples(&fp), triples(&literal));
        assert_eq!(
            triples(&fp),
            vec![
                (function_id(), emit, edge_category::USES),
                (module_id(), handle, edge_category::CONTAINS),
            ]
        );
        assert!(!fp.edges.iter().any(|e| e.from == old_emit || e.to == old_emit));
        // Re-anchored post-cache: `extractor:anchor` rule `const_fold`.
        assert!(fp.edges.iter().all(|e| evidence::Evidence::of(e).is_some_and(
            |ev| ev.emitter == "extractor:anchor" && ev.rule.as_deref() == Some("const_fold")
        )));
        assert_eq!(crate::anchor::census(&fp), crate::anchor::census(&literal));
        // One IMPORTS cell per folded node, last.
        for n in fp.nodes.iter().filter(|n| n.id == emit || n.id == handle) {
            assert_eq!(n.cells.iter().filter(|c| c.kind == cell_type::IMPORTS).count(), 1);
            assert_eq!(n.cells.last().map(|c| c.kind), Some(cell_type::IMPORTS));
        }
    }

    #[test]
    fn replace_event_nodes_leaves_other_nodes_and_code_qname_events_alone() {
        // A Solidity-style EVENT_EMITTER under a code qname is no event site:
        // the swap never removes it.
        let mut fp = event_parse(CONST_SRC);
        let declared = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::EVENT_EMITTER,
            "Token::Transfer",
        );
        fp.nodes.push(Node {
            id: declared,
            repo: repo(),
            confidence: Confidence::Strong,
            cells: vec![],
        });
        fp.nav.record(declared, "Transfer", "Token::Transfer", node_kind::EVENT_EMITTER, Some(module_id()));
        assert!(!is_event_site(node_kind::EVENT_EMITTER, "Token::Transfer"));
        assert!(is_event_site(node_kind::EVENT_EMITTER, "event_emit:x"));
        assert!(!is_event_site(node_kind::EVENT_HANDLER, "event_emit:x"));
        replace_event_nodes(&mut fp, module_id(), "typescript", folded(CONST_SRC, BINDINGS));
        assert!(fp.nodes.iter().any(|n| n.id == declared));
        assert_eq!(fp.nav.qname_by_id.get(&declared).map(String::as_str), Some("Token::Transfer"));
        assert_eq!(fp.nodes.first().map(|n| n.id), Some(module_id()));
        assert_eq!(fp.nodes.last().map(|n| n.id), Some(declared));
    }
}
