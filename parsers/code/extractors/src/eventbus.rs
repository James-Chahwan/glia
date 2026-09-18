use std::sync::OnceLock;

use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, node_kind};
use repo_graph_core::{Confidence, Node, NodeId, NodeKindId, RepoId};

use crate::anchor::{Anchor, line_of};

pub struct EventNodes {
    pub nodes: Vec<Node>,
    pub nav: CodeNav,
    /// A5.8: where each node's needle fired (see `crate::anchor`). The
    /// type-keyed needles anchor every site; the string-keyed needles anchor
    /// the one site that minted the node.
    pub anchors: Vec<Anchor>,
}

/// (needle, extract_name, broker_ambiguous).
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
/// `publish(` and `.subscribe(` are additionally verb-gated (LA.29,
/// [`GATED_VERBS`]): they name a channel only on a bus, so an occurrence counts
/// only when it is a call on a bus-shaped receiver or in a file importing an
/// in-process pub/sub library — never a declaration, never a typed site.
const EMITTER_PATTERNS: &[(&str, bool, bool)] = &[
    (".emit(", true, false),
    (".dispatch(", true, false),
    ("Subject.next(", true, false),
    ("EventBridge.putEvents", false, false),
    ("eventBridge.putEvents", false, false),
    ("publish(", true, true),
    (".trigger(", true, false),
    ("dispatchEvent(", true, false),
];

const HANDLER_PATTERNS: &[(&str, bool, bool)] = &[
    (".on(", true, true),
    (".addEventListener(", true, false),
    (".subscribe(", true, true),
    ("@EventPattern(", true, false),
    ("@OnEvent(", true, false),
    ("handle_event", false, false),
    (".addListener(", true, false),
];

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

/// LA.29: the two broker-ambiguous verbs that name a pub/sub channel, and the
/// only needles [`find_gated`] judges per occurrence. A function named
/// `publish`, an RxJS `obs.subscribe(...)` and a tokio `tx.subscribe()` all
/// share them with a real bus; the conjugate kinds take the same gate.
const GATED_VERBS: &[&str] = &["publish(", ".subscribe("];

/// A receiver reads as an in-process bus when its name (lowercased, leading
/// `_` / `$` stripped) ends with one of these: `eventBus`, `this.bus`,
/// `PubSub`, `ActiveSupport::Notifications`, `this.events`, `_mediator`. The
/// only recall knob besides [`PUBSUB_IMPORTS`]. `router.events.subscribe`
/// passes it (accepted: the Ionic `Events` bus shares the name).
const BUS_RECEIVER_SUFFIXES: &[&str] = &[
    "bus",
    "pubsub",
    "emitter",
    "events",
    "mediator",
    "publisher",
    "notifications",
];

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
    for &(pattern, extract_name, ambiguous) in EMITTER_PATTERNS {
        let Some((idx, event_name)) = find_gated(source, pattern, extract_name, &mut ctx) else {
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
    for &(pattern, extract_name, ambiguous) in HANDLER_PATTERNS {
        let Some((idx, event_name)) = find_gated(source, pattern, extract_name, &mut ctx) else {
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
    }

    EventNodes { nodes, nav, anchors }
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

/// Per-extract-call file facts the verb gate (LA.29) and the broker gate
/// (A2.9) read. All lazy: the lowercase copy is built at most once per call,
/// and only when a gated verb or a broker-ambiguous needle actually matched —
/// a file with no `publish(` / `.subscribe(` / `.on(` never pays for it. The
/// copy is only ever substring-tested, never used to slice `source`.
#[derive(Default)]
struct VerbCtx {
    lower: Option<String>,
    bus_import: Option<bool>,
    broker: Option<bool>,
}

impl VerbCtx {
    fn lower(&mut self, source: &str) -> &str {
        self.lower.get_or_insert_with(|| source.to_ascii_lowercase())
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

/// Why [`judge_verb`] kept or rejected one occurrence of a gated verb.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// A call with bus evidence: this occurrence names the event.
    Keep,
    /// A typed publish (`new` is the first argument token): the type pass
    /// already minted the event under its type name.
    TypeSite,
    /// `def publish(self, m):`, `void publish(String p);`, `publish(m): void {`.
    Declaration,
    /// A call with neither a bus-shaped receiver nor a pub/sub import.
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
/// queue-owned, when — for the two gated verbs ([`GATED_VERBS`]) —
/// [`judge_verb`] keeps it, and when [`event_name_at`] reads a name there
/// (LA.41): a quoted literal that is not name-shaped skips that occurrence
/// and the walk goes on, so a needle table's `"@OnEvent(", "x"` never decides
/// the file's node and a later `@OnEvent('order.shipped')` does, anchored
/// there. A file whose first `publish(` is a declaration and a later one a
/// bus call anchors at the bus call (LA.29). Needles that extract no name
/// (`handle_event`, `EventBridge.putEvents`) keep their first ungated
/// occurrence. Broker suppression (A2.9) is the caller's, after this walk.
fn find_gated(
    source: &str,
    pattern: &str,
    extract_name: bool,
    ctx: &mut VerbCtx,
) -> Option<(usize, String)> {
    let gated = GATED_VERBS.contains(&pattern);
    let mut tally = GateTally::default();
    let mut bad_name = 0usize;
    let mut found = None;
    let mut from = 0usize;
    while let Some(rel) = source[from..].find(pattern) {
        let at = from + rel;
        from = at + pattern.len();
        if pattern == "publish(" && queue_owned_publish(source, at) {
            continue;
        }
        if gated {
            match judge_verb(source, pattern, at, ctx) {
                Verdict::Keep => {}
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
        }
        found = Some((at, name));
        break;
    }
    if event_debug() {
        // A gated occurrence the gate kept but whose literal was malformed is
        // counted by the bad-name line, not the verb-gate one.
        if gated && tally.kept + tally.decl + tally.no_bus + tally.type_site + bad_name > 0 {
            eprintln!(
                "[eventbus] verb-gate needle='{pattern}' kept={} rejected decl={} no_bus={} type_site={}",
                tally.kept, tally.decl, tally.no_bus, tally.type_site
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
    let bus = match dot_at {
        Some(dot) => {
            let (receiver, called) = receiver_segment(source, dot);
            is_bus_receiver(receiver, called) || ctx.bus_import(source)
        }
        None => ctx.bus_import(source),
    };
    if bus { Verdict::Keep } else { Verdict::NoBus }
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
        // A task-queue library is not a broker: BullMQ workers are EventEmitters.
        assert_eq!(
            handled("import { Worker } from 'bullmq';\nworker.on('completed', done);"),
            vec!["event_handle:completed"]
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
}
