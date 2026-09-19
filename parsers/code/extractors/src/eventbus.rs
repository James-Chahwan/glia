use std::sync::OnceLock;

use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, cell_type, node_kind};
use repo_graph_core::{Cell, CellPayload, Confidence, Node, NodeId, NodeKindId, RepoId};

use crate::anchor::{Anchor, line_of};

pub struct EventNodes {
    pub nodes: Vec<Node>,
    pub nav: CodeNav,
    /// A5.8: where each node's needle fired (see `crate::anchor`). The
    /// type-keyed needles anchor every site; the string-keyed needles anchor
    /// the one site that minted the node.
    pub anchors: Vec<Anchor>,
}

/// (needle, extract_name, broker_ambiguous, gate).
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
const EMITTER_PATTERNS: &[(&str, bool, bool, VerbGate)] = &[
    (".emit(", true, false, VerbGate::Receiver),
    (".dispatch(", true, false, VerbGate::Receiver),
    ("Subject.next(", true, false, VerbGate::Open),
    ("EventBridge.putEvents", false, false, VerbGate::Open),
    ("eventBridge.putEvents", false, false, VerbGate::Open),
    ("publish(", true, true, VerbGate::Bus),
    (".trigger(", true, false, VerbGate::Receiver),
    ("dispatchEvent(", true, false, VerbGate::Receiver),
];

const HANDLER_PATTERNS: &[(&str, bool, bool, VerbGate)] = &[
    (".on(", true, true, VerbGate::Receiver),
    (".addEventListener(", true, false, VerbGate::Receiver),
    (".subscribe(", true, true, VerbGate::Bus),
    ("@EventPattern(", true, false, VerbGate::Open),
    ("@OnEvent(", true, false, VerbGate::Open),
    ("handle_event", false, false, VerbGate::Open),
    (".addListener(", true, false, VerbGate::Receiver),
];

/// How [`find_gated`] judges one occurrence of a string-keyed needle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VerbGate {
    /// Every occurrence is a site: the decorator needles, `Subject.next`,
    /// `handle_event`, `EventBridge.putEvents`.
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

pub fn extract_event_emitter_nodes(source: &str, module_id: NodeId, repo: RepoId) -> EventNodes {
    let mut nodes = Vec::new();
    let mut nav = CodeNav::default();
    let mut anchors = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // Type-keyed FIRST: it is stronger evidence (a real type name, not a
    // framework tag), so when both passes see the same event the Medium
    // confidence is the one that lands.
    for (name, at) in scan_type_needles(source, TYPE_EMITTER_NEEDLES.iter().map(|n| (*n, None))) {
        let (id, _) = push_event_node(
            &mut nodes,
            &mut nav,
            &mut seen,
            &name,
            node_kind::EVENT_EMITTER,
            "event_emit:",
            Confidence::Medium,
            module_id,
            repo,
        );
        anchors.push(Anchor { node: id, line: line_of(source, at) });
    }

    let mut ctx = VerbCtx::default();
    for &(pattern, extract_name, ambiguous, gate) in EMITTER_PATTERNS {
        let Some((idx, event_name)) = find_gated(source, pattern, extract_name, gate, &mut ctx)
        else {
            continue;
        };
        if ambiguous && ctx.broker_present(source) {
            suppressed("emitter", pattern, &event_name);
            continue;
        }

        let (id, minted) = push_event_node(
            &mut nodes,
            &mut nav,
            &mut seen,
            &event_name,
            node_kind::EVENT_EMITTER,
            "event_emit:",
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

    EventNodes { nodes, nav, anchors }
}

pub fn extract_event_handler_nodes(source: &str, module_id: NodeId, repo: RepoId) -> EventNodes {
    let mut nodes = Vec::new();
    let mut nav = CodeNav::default();
    let mut anchors = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for (name, at) in scan_type_needles(source, TYPE_HANDLER_NEEDLES.iter().copied()) {
        let (id, _) = push_event_node(
            &mut nodes,
            &mut nav,
            &mut seen,
            &name,
            node_kind::EVENT_HANDLER,
            "event_handle:",
            Confidence::Medium,
            module_id,
            repo,
        );
        anchors.push(Anchor { node: id, line: line_of(source, at) });
    }

    let mut ctx = VerbCtx::default();
    for &(pattern, extract_name, ambiguous, gate) in HANDLER_PATTERNS {
        let Some((idx, event_name)) = find_gated(source, pattern, extract_name, gate, &mut ctx)
        else {
            continue;
        };
        if ambiguous && ctx.broker_present(source) {
            suppressed("handler", pattern, &event_name);
            continue;
        }

        let (id, minted) = push_event_node(
            &mut nodes,
            &mut nav,
            &mut seen,
            &event_name,
            node_kind::EVENT_HANDLER,
            "event_handle:",
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

    EventNodes { nodes, nav, anchors }
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

/// The event the occurrence of a string-keyed needle at `idx` names (LA.41):
/// the quoted literal after it when that literal is a name, else — when there
/// is no quoted literal at all — the needle word itself (`publish`,
/// `subscribe`). `None` when a quoted literal is there but malformed: that
/// occurrence is not an event site, and [`find_gated`] walks on.
fn event_name_at(source: &str, pattern: &str, idx: usize, extract_name: bool) -> Option<String> {
    if !extract_name {
        return Some(verb_name(pattern));
    }
    match literal_after(source, idx + pattern.len()) {
        LiteralAt::Name(name) => Some(name),
        LiteralAt::Absent => Some(verb_name(pattern)),
        LiteralAt::Malformed => None,
    }
}

/// The fallback event name: the needle's verb (`.subscribe(` -> `subscribe`).
fn verb_name(pattern: &str) -> String {
    pattern.trim_matches('.').trim_end_matches('(').to_string()
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

/// fired_on marker for the verb gate (LA.29) and the event-name shape rule
/// (LA.41), the queues.rs `debug_enabled` pattern under its own switch:
///   `GLIA_EVENT_DEBUG=1 ... 2>&1 | grep '\[eventbus\] verb-gate'`
///   `GLIA_EVENT_DEBUG=1 ... 2>&1 | grep '\[eventbus\] aws-sdk command skipped'`
///   `GLIA_EVENT_DEBUG=1 ... 2>&1 | grep '\[eventbus\] bad-name'`
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
/// when [`event_name_at`] reads a name there (LA.41): a quoted literal that
/// is not name-shaped skips that occurrence and the walk goes on, so a needle
/// table's `"@OnEvent(", "x"` never decides the file's node and a later
/// `@OnEvent('order.shipped')` does, anchored there. A file whose first
/// `publish(` is a declaration and a later one a bus call anchors at the bus
/// call (LA.29); a file whose first `.addEventListener(` is on a DOM element
/// and a later one on a bus anchors at the bus call (LA.39). Needles that
/// extract no name (`handle_event`, `EventBridge.putEvents`) keep their first
/// ungated occurrence. Broker suppression (A2.9) is the caller's, after this
/// walk.
fn find_gated(
    source: &str,
    pattern: &str,
    extract_name: bool,
    gate: VerbGate,
    ctx: &mut VerbCtx,
) -> Option<(usize, String)> {
    let gated = gate != VerbGate::Open;
    let mut tally = GateTally::default();
    let mut via: Option<Via> = None;
    let mut bad_name = 0usize;
    let mut found = None;
    let mut from = 0usize;
    while let Some(rel) = source[from..].find(pattern) {
        let at = from + rel;
        from = at + pattern.len();
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
        let Some(name) = event_name_at(source, pattern, at, extract_name) else {
            bad_name += 1;
            continue;
        };
        if gated {
            tally.kept = 1;
            via = kept_via;
        }
        found = Some((at, name));
        break;
    }
    if event_debug() {
        // A gated occurrence the gate kept but whose literal was malformed is
        // counted by the bad-name line, not the verb-gate one.
        if gated && tally.kept + tally.decl + tally.no_bus + tally.type_site + bad_name > 0 {
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
    /// No quoted literal: a variable, a backtick template, an object. The
    /// caller falls back to the needle's verb.
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
        let result = extract_event_emitter_nodes(source, module_id(), repo());
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
        let result = extract_event_handler_nodes(source, module_id(), repo());
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
        let result = extract_event_handler_nodes(source, module_id(), repo());
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
        let result = extract_event_emitter_nodes(source, module_id(), repo());
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
        let result = extract_event_handler_nodes(source, module_id(), repo());
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
        let result = extract_event_handler_nodes(source, module_id(), repo());
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
        let result = extract_event_handler_nodes(source, module_id(), repo());
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
        let result = extract_event_emitter_nodes(source, module_id(), repo());
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
        let result = extract_event_handler_nodes(source, module_id(), repo());
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
        let result = extract_event_emitter_nodes(source, module_id(), repo());
        assert!(
            result.nodes.is_empty(),
            "expected no event node, got {:?}",
            result.nav.qname_by_id
        );

        // An in-process bus that happens to use the same verb still fires.
        let bus = "bus.publish(\"user.created\", u);";
        let result = extract_event_emitter_nodes(bus, module_id(), repo());
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
        let mut v: Vec<String> = extract_event_emitter_nodes(source, module_id(), repo())
            .nav
            .qname_by_id
            .into_values()
            .collect();
        v.sort();
        v
    }

    fn handled(source: &str) -> Vec<String> {
        let mut v: Vec<String> = extract_event_handler_nodes(source, module_id(), repo())
            .nav
            .qname_by_id
            .into_values()
            .collect();
        v.sort();
        v
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
        let out = extract_event_emitter_nodes(src, module_id(), repo());
        assert_eq!(out.nodes.len(), 1);
        let id = out.nodes[0].id;
        assert_eq!(
            out.anchors,
            vec![Anchor { node: id, line: 1 }, Anchor { node: id, line: 3 }]
        );

        // String-keyed: the minting site only, at its own line.
        let src = "import x;\nexport function f() {\n  bus.on('user.created', h);\n}";
        let out = extract_event_handler_nodes(src, module_id(), repo());
        assert_eq!(out.anchors, vec![Anchor { node: out.nodes[0].id, line: 2 }]);

        // A suppressed broker call mints nothing, so it anchors nothing.
        let src = "import { connect } from 'mqtt';\nclient.subscribe('t');";
        let out = extract_event_handler_nodes(src, module_id(), repo());
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
        let out = extract_event_emitter_nodes(src, module_id(), repo());
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
        let out = extract_event_handler_nodes(src, module_id(), repo());
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
        let out = extract_event_handler_nodes(src, module_id(), repo());
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
            module_id(),
            repo(),
        );
        assert_eq!(
            origins(&out),
            vec![(s("event_handle:order_shipped"), transport("nestjs-microservices"))]
        );

        let client = "import { ClientProxy } from '@nestjs/microservices';\nthis.client.emit('order_shipped', o);";
        let out = extract_event_emitter_nodes(client, module_id(), repo());
        assert_eq!(
            origins(&out),
            vec![(s("event_emit:order_shipped"), transport("nestjs-microservices"))]
        );

        let bridge = "await eventBridge.putEvents({ Entries: [] }).promise();";
        let out = extract_event_emitter_nodes(bridge, module_id(), repo());
        assert_eq!(
            origins(&out),
            vec![(s("event_emit:eventBridge.putEvents"), transport("aws-eventbridge"))]
        );

        let local = "import { EventEmitter } from 'events';\nconst bus = new EventEmitter();\nbus.emit('x', 1);";
        let out = extract_event_emitter_nodes(local, module_id(), repo());
        assert_eq!(origins(&out), vec![(s("event_emit:x"), vec![])], "in-process: no ORIGIN");
        let out = extract_event_handler_nodes("bus.on('x', h);\n@OnEvent('y')\nh2() {}", module_id(), repo());
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
        let out = extract_event_handler_nodes(src, module_id(), repo());
        assert_eq!(origins(&out), vec![(s("event_handle:x"), transport("nestjs-microservices"))]);
        assert_eq!(out.nodes.len(), 1);
    }

    fn s(v: &str) -> String {
        v.to_string()
    }
}
