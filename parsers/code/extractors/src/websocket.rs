//! Cross-cutting WebSocket extraction: WS_HANDLER (`ws:<path>`) and WS_CLIENT
//! (`ws_client:<path>`) marker nodes, paired across repos by
//! `WebSocketStackResolver` in the graph crate.
//!
//! LA.18a (programme A5.9) rewrote the scan. Each needle is a [`WsRow`]: a
//! call-shaped needle, a framework gate (any-of substrings of the file, so a
//! broad needle such as Go's `.Upgrade(` only fires in a file that imports
//! gorilla), how the path is read ([`PathRead`]) and where the site is
//! anchored ([`AnchorAt`]). Every occurrence of every gated row is examined
//! (at most [`MAX_HITS_PER_NEEDLE`] per row per file), not only the first, so
//! a second `@app.websocket(...)` or `new WebSocket(...)` in a file is no
//! longer invisible. A needle that starts with an identifier byte must not be
//! preceded by one, so Dart's `Dio()` is not socket.io's `io(`.
//!
//! Names: a server path is kept verbatim (`/chat/{room}` stays templated); a
//! client argument goes through [`client_path`], which reads the static tail
//! of a concatenation or template literal and strips `ws(s)://` /
//! `http(s)://` + authority. Anything unreadable falls back to `ws`
//! (`default` for the path-less NestJS row), exactly as before the rewrite,
//! unless the row's [`Accept`] says an unreadable site mints nothing. A name
//! is never multi-line: whitespace and control characters reject it.
//!
//! Channel-keyed frameworks (LA.18c) join on a channel name or topic, not a
//! URL path, so the name IS that key and never falls back:
//! - ActionCable: the server's channel is the class inheriting
//!   `ApplicationCable::Channel` (`ws:ChatChannel`, [`PathRead::ClassBeforeLt`]);
//!   the JS consumer names it in `subscriptions.create({ channel: "..." })`,
//!   accepted only in the `...Channel` class-name shape
//!   ([`Accept::ChannelClass`]), so Stripe's `subscriptions.create(` never
//!   mints.
//! - Phoenix: the endpoint's `socket "/socket", ...` mount and the socket's
//!   `channel "room:*", ...` topic pattern ([`PathRead::LineLiteral`], gated on
//!   `Phoenix.Endpoint` / `Phoenix.Socket`); phoenix.js's `new Socket("/socket")`
//!   and `socket.channel("room:lobby")` on the client. `use Phoenix.Channel` is
//!   a module attribute, not a handler, and mints nothing. The resolver pairs
//!   a trailing-`*` topic with its concrete topics.
//!
//! Paths read off the file's routing structure (DG.5), not the call's own
//! arguments:
//! - axum: `ws.on_upgrade(..)` sits in the handler fn F; its path is the
//!   first in-file `.route("/p", get(F))` whose method router names F
//!   ([`PathRead::AxumRoute`]). No in-file route names F: the name is `ws`,
//!   and the graph crate's ws resolver lends it the paths of every ROUTE
//!   HANDLED_BY F, as for a gorilla upgrade.
//! - Akka / Pekko HTTP: `handleWebSocketMessages(..)` is routed by the
//!   `path(..)` / `pathPrefix(..)` directives whose blocks enclose it,
//!   composed outer to inner (`pathPrefix("api") { path("ws" / Segment) {..} }`
//!   -> `/api/ws/:segment`, [`PathRead::AkkaDirective`]); a matcher outside
//!   [`AKKA_MATCHERS`] makes the name `ws`.
//!
//! Both scan blocks over code bytes only ([`Blocks`], CodeMap's C-family
//! lexing, which covers Rust and Scala).
//!
//! Anchors: the name is read per site, so EVERY site is anchored and
//! `anchor::attach` gives each owning function its HANDLED_BY (server) or USES
//! (client) edge. Python decorators anchor at the decorated `def` line
//! ([`AnchorAt::NextDef`]), because a Python FUNCTION span starts at its
//! `def`, not at the decorator above it.
//!
//! Hosts (CB.21): every CLIENT site stacks one ENDPOINT_HIT on its node,
//! `{"via":"ws","host":"chat-svc:8080"}` when the URL's static head names a
//! literal authority ([`client_host`]), `{"via":"ws"}` otherwise. The graph
//! crate's ws resolver narrows a client's same-path handlers to the service
//! its hosts name; one hostless site blocks that, as on the HTTP side.
//!
//! fired_on marker, one line per file per framework that yielded a node, `n`
//! = distinct nodes that framework's sites read, path repo-relative:
//!   `[ws] handlers framework=fastapi n=2 in app.py`
//!   `[ws] handlers framework=axum n=1 in src/main.rs`
//!   `[ws] handlers framework=akka-http n=1 in ChatRoutes.scala`
//!   `[ws] clients framework=browser n=2 in live.ts`
//!   `[ws] handlers framework=phoenix n=1 in lib/app_web/channels/user_socket.ex`
//!   `[ws] clients framework=actioncable n=1 in chat.js`
//! and one per file whose client sites named a host (CB.21):
//!   `[ws] client hosts sites=1 in web/chat.ts`
//!
//! Self-scan precision (CJ.1b): in a Rust or Python file a needle whose first
//! byte sits in a string literal or comment is no site ([`LazyGuard`], CJ.1a's
//! guard), so a doc line or a test's sample source mints nothing. Only the
//! NEEDLE offset is tested, never the anchor line: a FastAPI handler's
//! `@app.websocket(` needle is code and anchors on the `def` below it. One
//! line per scan that refused an occurrence:
//!   `[code-guard] ws_client lang=rust dropped=2 path=src/x.rs`
//!
//! The call-argument reader here ([`call_region`], [`split_top`]) is a local
//! copy of the shape `queue_topic.rs` keeps private (that file belongs to
//! another packet's file set, the rule A2.8 followed for `escape_json`).
//! Follow-up: one shared call-argument reader for both.

use std::collections::HashMap;

use glia_code_domain::endpoint::{channel_hit_cell, client_url_split};
use glia_code_domain::{CodeNav, GRAPH_TYPE, node_kind};
use glia_core::{Confidence, Node, NodeId, NodeKindId, RepoId};

use crate::anchor::{Anchor, line_of};
use crate::cli::CodeMap;
use crate::code_guard::LazyGuard;
use crate::queue_topic::{LOOKAHEAD_LINES, MAX_HITS_PER_NEEDLE, MAX_REGION, clip};

pub struct WsNodes {
    pub nodes: Vec<Node>,
    pub nav: CodeNav,
    /// A5.8: every needle site that read each node (see `crate::anchor`).
    pub anchors: Vec<Anchor>,
}

/// How a row reads the socket path from its site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathRead {
    /// No path at the site: the name is `ws`.
    Generic,
    /// A path-less framework marker: the name is `default`.
    Default,
    /// The string literal that is positional argument `n` of the call.
    Arg(usize),
    /// `key: "/x"` / `key = "/x"` anywhere in the call region, else argument 0.
    KeyedOrArg(&'static [&'static str]),
    /// `MapHub<T>("/x")`: argument 0 of the call after the matching `>`.
    AfterAngle,
    /// [`client_path`] on positional argument `n`.
    Client(usize),
    /// The class declared on the needle's own line, before the needle:
    /// `class ChatChannel < ApplicationCable::Channel` -> `ChatChannel`.
    ClassBeforeLt,
    /// The first double-quoted literal on the needle's line, from the needle
    /// on (Elixir's paren-less `socket "/socket", ...` / `channel "room:*", ...`).
    LineLiteral,
    /// DG.5, axum: the path of the first in-file `.route("/p", ..)` whose
    /// method router names the `fn` enclosing the needle
    /// ([`enclosing_rust_fn`], [`axum_route_of`]).
    AxumRoute,
    /// DG.5, Akka / Pekko HTTP: the composed path of the `path(..)` /
    /// `pathPrefix(..)` directives whose blocks enclose the needle
    /// ([`akka_directive_path`]).
    AkkaDirective,
}

/// Which names a row mints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Accept {
    /// Any name the site reads; an unreadable site mints the fallback name
    /// (`ws`, or `default` for [`PathRead::Default`]).
    ReadOrFallback,
    /// Only a name the site reads: an unreadable site mints nothing.
    ReadOnly,
    /// Only a read name in ActionCable's channel-class shape
    /// ([`is_channel_class`]); anything else mints nothing.
    ChannelClass,
}

/// Which line a site anchors at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnchorAt {
    /// The needle's own line.
    Needle,
    /// The first `def ` / `async def ` line within [`LOOKAHEAD_LINES`] after
    /// the needle (a Python decorator), else the needle's line.
    NextDef,
}

/// One needle of the scan.
#[derive(Debug, Clone, Copy)]
struct WsRow {
    needle: &'static str,
    /// Any-of substrings of the file (case-sensitive import / type spellings);
    /// empty means the row is ungated.
    gate: &'static [&'static str],
    read: PathRead,
    anchor: AnchorAt,
    accept: Accept,
    /// Discriminator of the fired_on marker.
    framework: &'static str,
}

const fn row(
    needle: &'static str,
    gate: &'static [&'static str],
    read: PathRead,
    framework: &'static str,
) -> WsRow {
    keyed_row(needle, gate, read, Accept::ReadOrFallback, framework)
}

/// A row whose [`Accept`] is not the fallback one: a channel-keyed framework,
/// where a generic name would join nothing.
const fn keyed_row(
    needle: &'static str,
    gate: &'static [&'static str],
    read: PathRead,
    accept: Accept,
    framework: &'static str,
) -> WsRow {
    WsRow {
        needle,
        gate,
        read,
        anchor: AnchorAt::Needle,
        accept,
        framework,
    }
}

const NONE: &[&str] = &[];
const GORILLA: &[&str] = &["gorilla/websocket"];
const NHOOYR: &[&str] = &["nhooyr.io/websocket", "github.com/coder/websocket"];
const FASTAPI_STARLETTE: &[&str] = &["fastapi", "starlette"];
/// phoenix.js, by its import string (not `phoenix_html` / `phoenix_live_view`).
const PHOENIX_JS: &[&str] = &["\"phoenix\"", "'phoenix'"];
/// axum, by its crate path (`use axum::..`, `axum::extract::ws`).
const AXUM: &[&str] = &["axum"];
/// Akka HTTP and its Apache fork Pekko HTTP, by package.
const AKKA_HTTP: &[&str] = &["akka.http", "org.apache.pekko.http"];

const HANDLER_ROWS: &[WsRow] = &[
    // node `ws` / socket servers. A bare `WebSocketServer` (an import line)
    // no longer mints a handler: only the constructor does.
    row("ws.on(\"connection\"", NONE, PathRead::Generic, "ws"),
    row("ws.on('connection'", NONE, PathRead::Generic, "ws"),
    row(
        "new WebSocketServer(",
        NONE,
        PathRead::KeyedOrArg(&["path"]),
        "ws",
    ),
    row(
        "new WebSocket.Server(",
        NONE,
        PathRead::KeyedOrArg(&["path"]),
        "ws",
    ),
    row("ws.handleUpgrade", NONE, PathRead::Generic, "ws"),
    row("@WebSocketGateway", NONE, PathRead::Default, "nestjs"),
    // Go. `websocket.Upgrader` (a package var) and `gorilla/websocket` (an
    // import) are not handlers; the upgrade call inside the handler is.
    row(".Upgrade(", GORILLA, PathRead::Generic, "gorilla"),
    row("websocket.Accept(", NHOOYR, PathRead::Generic, "nhooyr"),
    // Rust: axum's `WebSocketUpgrade::on_upgrade`, inside the handler fn the
    // file's router mounts (DG.5).
    row(".on_upgrade(", AXUM, PathRead::AxumRoute, "axum"),
    // Python.
    WsRow {
        needle: ".websocket(",
        gate: FASTAPI_STARLETTE,
        read: PathRead::Arg(0),
        anchor: AnchorAt::NextDef,
        accept: Accept::ReadOrFallback,
        framework: "fastapi",
    },
    row(
        "WebSocketRoute(",
        &["starlette"],
        PathRead::Arg(0),
        "starlette",
    ),
    row(
        ".add_websocket_route(",
        FASTAPI_STARLETTE,
        PathRead::Arg(0),
        "starlette",
    ),
    // Java: JSR-356 and Spring.
    row(
        "@ServerEndpoint(",
        &["javax.websocket", "jakarta.websocket"],
        PathRead::KeyedOrArg(&["value"]),
        "jsr356",
    ),
    row(
        ".addHandler(",
        &["WebSocketHandlerRegistry"],
        PathRead::Arg(1),
        "spring",
    ),
    row(
        ".addEndpoint(",
        &["StompEndpointRegistry"],
        PathRead::Arg(0),
        "spring-stomp",
    ),
    // Scala: Akka / Pekko HTTP, under the path directives enclosing it (DG.5).
    row(
        "handleWebSocketMessages(",
        AKKA_HTTP,
        PathRead::AkkaDirective,
        "akka-http",
    ),
    row(
        "handleWebSocketMessagesForProtocol(",
        AKKA_HTTP,
        PathRead::AkkaDirective,
        "akka-http",
    ),
    // C#: SignalR hubs and raw ASP.NET Core sockets.
    row("MapHub<", &["SignalR"], PathRead::AfterAngle, "signalr"),
    row(
        "AcceptWebSocketAsync(",
        NONE,
        PathRead::Generic,
        "aspnetcore",
    ),
    // ActionCable (LA.18c): a channel is a class inheriting the app's
    // channel base (or ActionCable's own); its class name is the join key.
    keyed_row(
        "< ApplicationCable::Channel",
        NONE,
        PathRead::ClassBeforeLt,
        Accept::ReadOnly,
        "actioncable",
    ),
    keyed_row(
        "< ActionCable::Channel::Base",
        NONE,
        PathRead::ClassBeforeLt,
        Accept::ReadOnly,
        "actioncable",
    ),
    // Phoenix (LA.18c), paren-less Elixir calls: the endpoint's socket mount
    // and the socket's channel topic pattern. A single-quoted `channel '...'`
    // is an Elixir charlist, not a topic.
    keyed_row(
        "socket \"",
        &["Phoenix.Endpoint"],
        PathRead::LineLiteral,
        Accept::ReadOnly,
        "phoenix",
    ),
    keyed_row(
        "channel \"",
        &["Phoenix.Socket"],
        PathRead::LineLiteral,
        Accept::ReadOnly,
        "phoenix",
    ),
];

const CLIENT_ROWS: &[WsRow] = &[
    row("new WebSocket(", NONE, PathRead::Client(0), "browser"),
    row(
        "useWebSocket(",
        NONE,
        PathRead::Client(0),
        "react-use-websocket",
    ),
    row("io(", NONE, PathRead::Client(0), "socketio"),
    row("io.connect(", NONE, PathRead::Client(0), "socketio"),
    row("socket.io-client", NONE, PathRead::Generic, "socketio"),
    row("ws.connect(", NONE, PathRead::Client(0), "ws"),
    row("WebSocketSubject(", NONE, PathRead::Client(0), "rxjs"),
    row(
        "connectWebSocket(",
        NONE,
        PathRead::Client(0),
        "connectWebSocket",
    ),
    row(
        ".withUrl(",
        &["@microsoft/signalr"],
        PathRead::Client(0),
        "signalr",
    ),
    row(
        ".WithUrl(",
        &["Microsoft.AspNetCore.SignalR.Client"],
        PathRead::Client(0),
        "signalr-dotnet",
    ),
    row(
        "DefaultDialer.Dial(",
        GORILLA,
        PathRead::Client(0),
        "gorilla",
    ),
    row(
        "DefaultDialer.DialContext(",
        GORILLA,
        PathRead::Client(1),
        "gorilla",
    ),
    row("websocket.Dial(", NHOOYR, PathRead::Client(1), "nhooyr"),
    row(
        "websockets.connect(",
        NONE,
        PathRead::Client(0),
        "python-websockets",
    ),
    // ActionCable consumer (LA.18c): the channel class it subscribes to.
    keyed_row(
        "subscriptions.create(",
        NONE,
        PathRead::KeyedOrArg(&["channel"]),
        Accept::ChannelClass,
        "actioncable",
    ),
    // phoenix.js (LA.18c): the socket mount, then the topic it joins.
    row("new Socket(", PHOENIX_JS, PathRead::Client(0), "phoenix"),
    keyed_row(
        ".channel(",
        PHOENIX_JS,
        PathRead::Arg(0),
        Accept::ReadOnly,
        "phoenix",
    ),
];

/// Longest name accepted; anything longer is a payload, not a path.
const MAX_NAME_LEN: usize = 256;

/// Bound on the `<...>` walked by [`PathRead::AfterAngle`].
const MAX_ANGLE: usize = 256;

/// Stands in for a dynamic piece of a client URL. A control char, so it can
/// never survive into a name ([`valid_name`] rejects control chars).
const DYNAMIC: char = '\u{1}';

pub fn extract_ws_handler_nodes(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
) -> WsNodes {
    scan(
        source,
        path,
        module_id,
        repo,
        Side {
            rows: HANDLER_ROWS,
            kind: node_kind::WS_HANDLER,
            prefix: "ws:",
            label: "handlers",
            hits: false,
        },
    )
}

pub fn extract_ws_client_nodes(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
) -> WsNodes {
    scan(
        source,
        path,
        module_id,
        repo,
        Side {
            rows: CLIENT_ROWS,
            kind: node_kind::WS_CLIENT,
            prefix: "ws_client:",
            label: "clients",
            hits: true,
        },
    )
}

struct Side {
    rows: &'static [WsRow],
    kind: NodeKindId,
    prefix: &'static str,
    label: &'static str,
    /// CB.21: every site stacks one ENDPOINT_HIT on its node, `host` when the
    /// site's URL names a literal authority ([`site_host`]). Clients only.
    hits: bool,
}

/// Rows in table order, sites in byte order: node order and anchor order are
/// a function of the file's text alone. The name map is only ever looked up,
/// never iterated.
fn scan(source: &str, path: &str, module_id: NodeId, repo: RepoId, side: Side) -> WsNodes {
    let mut nodes = Vec::new();
    let mut nav = CodeNav::default();
    let mut anchors = Vec::new();
    let mut by_name: HashMap<String, usize> = HashMap::new();
    // (framework, distinct nodes its sites read), in first-seen order.
    let mut per_framework: Vec<(&'static str, Vec<NodeId>)> = Vec::new();
    // CB.21: client sites whose ENDPOINT_HIT names a host.
    let mut hosted = 0usize;
    // CJ.1b: a needle that starts in a Rust / Python literal or comment is no
    // site (a scanner's own needle table, a doc line, a test's sample source).
    let mut guard = LazyGuard::new(path, source);
    // DG.5: the file's code blocks, lexed on the first site that reads them.
    let mut blocks: Option<Blocks> = None;

    for r in side.rows {
        if !gate_holds(source, r.gate) {
            continue;
        }
        for offset in sites(source, r.needle, &mut guard) {
            let Some(name) = read_name(source, offset, r, &mut blocks)
                .or_else(|| fallback(r).map(str::to_string))
            else {
                continue;
            };
            let at = match by_name.get(&name) {
                Some(at) => *at,
                None => {
                    let qname = format!("{}{name}", side.prefix);
                    let id = NodeId::from_parts(GRAPH_TYPE, repo, side.kind, &qname);
                    nodes.push(Node {
                        id,
                        repo,
                        confidence: Confidence::Medium,
                        cells: vec![],
                    });
                    nav.record(id, &name, &qname, side.kind, Some(module_id));
                    by_name.insert(name, nodes.len() - 1);
                    nodes.len() - 1
                }
            };
            let Some(node) = nodes.get_mut(at) else {
                continue;
            };
            let id = node.id;
            if side.hits {
                let host = site_host(source, offset, r);
                if host.is_some() {
                    hosted += 1;
                }
                node.cells.push(channel_hit_cell("ws", host.as_deref()));
            }
            anchors.push(Anchor {
                node: id,
                line: anchor_line(source, offset, r.anchor),
            });
            let slot = match per_framework.iter().position(|(f, _)| *f == r.framework) {
                Some(i) => i,
                None => {
                    per_framework.push((r.framework, Vec::new()));
                    per_framework.len() - 1
                }
            };
            if let Some((_, ids)) = per_framework.get_mut(slot)
                && !ids.contains(&id)
            {
                ids.push(id);
            }
        }
    }

    for (framework, ids) in &per_framework {
        if !ids.is_empty() {
            eprintln!(
                "[ws] {} framework={framework} n={} in {path}",
                side.label,
                ids.len()
            );
        }
    }
    if hosted > 0 {
        eprintln!("[ws] client hosts sites={hosted} in {path}");
    }
    guard.report(if side.kind == node_kind::WS_HANDLER {
        "ws_handler"
    } else {
        "ws_client"
    });

    WsNodes {
        nodes,
        nav,
        anchors,
    }
}

fn gate_holds(source: &str, gate: &[&str]) -> bool {
    gate.is_empty() || gate.iter().any(|g| source.contains(g))
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Byte offsets of `needle` in `source`, word-bounded when the needle starts
/// with an identifier byte, at most [`MAX_HITS_PER_NEEDLE`]. CJ.1b: an
/// occurrence `guard` refuses (its first byte sits in a Rust / Python literal
/// or comment) is dropped BEFORE the cap, so literal occurrences never crowd
/// a real site out.
fn sites(source: &str, needle: &str, guard: &mut LazyGuard<'_>) -> Vec<usize> {
    if needle.is_empty() {
        return Vec::new();
    }
    let bounded = needle.as_bytes().first().is_some_and(|b| is_ident_byte(*b));
    let bytes = source.as_bytes();
    source
        .match_indices(needle)
        .map(|(i, _)| i)
        .filter(|&i| !bounded || i == 0 || bytes.get(i - 1).is_none_or(|b| !is_ident_byte(*b)))
        .filter(|&i| guard.admits(i))
        .take(MAX_HITS_PER_NEEDLE)
        .collect()
}

/// The name an unreadable site of `r` mints, or `None` when it mints nothing.
fn fallback(r: &WsRow) -> Option<&'static str> {
    if r.accept != Accept::ReadOrFallback {
        return None;
    }
    Some(match r.read {
        PathRead::Default => "default",
        _ => "ws",
    })
}

/// The name one site reads, or `None` when the row reads none there. `blocks`
/// is the scan's [`Blocks`], built here on the first site that needs it.
fn read_name(
    source: &str,
    offset: usize,
    r: &WsRow,
    blocks: &mut Option<Blocks>,
) -> Option<String> {
    let after = offset.checked_add(r.needle.len())?;
    let name = match r.read {
        PathRead::Generic | PathRead::Default => return None,
        PathRead::Arg(n) => server_literal(split_top(call_region(source, after)?, b',').get(n)?)?,
        PathRead::KeyedOrArg(keys) => {
            let region = call_region(source, after)?;
            keyed_literal(region, keys).or_else(|| {
                split_top(region, b',')
                    .first()
                    .and_then(|a| server_literal(a))
            })?
        }
        PathRead::AfterAngle => {
            let open = after_angle(source, after)?;
            server_literal(split_top(call_region(source, open)?, b',').first()?)?
        }
        PathRead::Client(n) => client_path(split_top(call_region(source, after)?, b',').get(n)?)?,
        PathRead::ClassBeforeLt => class_before(source, offset, after)?,
        PathRead::LineLiteral => line_literal(source, offset)?,
        PathRead::AxumRoute => {
            let blocks = blocks.get_or_insert_with(|| Blocks::new(source));
            axum_route_of(source, enclosing_rust_fn(source, offset, blocks)?, blocks)?
        }
        PathRead::AkkaDirective => akka_directive_path(
            source,
            offset,
            blocks.get_or_insert_with(|| Blocks::new(source)),
        )?,
    };
    let shaped = r.accept != Accept::ChannelClass || is_channel_class(&name);
    (valid_name(&name) && shaped).then_some(name)
}

/// For `class Name < Base` with the needle `< Base` at `offset` (`after` just
/// past it): `Name`, when the text before the needle on its line is exactly
/// `class Name` and the needle ends at a name boundary (`< ApplicationCable::
/// ChannelHelper` is not the channel base). A class whose own name is
/// `Channel` is the app's abstract base (`class Channel <
/// ActionCable::Channel::Base` inside `module ApplicationCable`), not a
/// channel, and reads nothing.
fn class_before(source: &str, offset: usize, after: usize) -> Option<String> {
    if source
        .as_bytes()
        .get(after)
        .is_some_and(|b| is_ident_byte(*b) || *b == b':')
    {
        return None;
    }
    let line_start = source.get(..offset)?.rfind('\n').map_or(0, |nl| nl + 1);
    let head = source.get(line_start..offset)?.trim();
    let rest = head.strip_prefix("class")?;
    if !rest.starts_with([' ', '\t']) {
        return None;
    }
    let name = rest.trim();
    let shaped = name.as_bytes().first().is_some_and(u8::is_ascii_uppercase)
        && name.bytes().all(|b| is_ident_byte(b) || b == b':');
    let base = name.rsplit("::").next() == Some("Channel");
    (shaped && !base).then(|| name.to_string())
}

/// The first `"..."` literal on the line of `offset`, from `offset` on. An
/// interpolated Elixir string (`#{...}`) is not a literal name.
fn line_literal(source: &str, offset: usize) -> Option<String> {
    let line = source.get(offset..)?.split('\n').next()?;
    let quote = line.find('"')?;
    let (lit, _) = leading_literal(line.get(quote..)?)?;
    (!lit.contains("#{")).then(|| lit.to_string())
}

/// ActionCable's channel-class shape, `^[A-Z][A-Za-z0-9_:]*Channel$`: the
/// consumer side's only precision guard, so a non-channel `subscriptions.create(`
/// (Stripe, a billing SDK) never mints. A channel class not ending in
/// `Channel` is a declared miss, never a false node.
fn is_channel_class(name: &str) -> bool {
    name.len() > "Channel".len()
        && name.ends_with("Channel")
        && name.as_bytes().first().is_some_and(u8::is_ascii_uppercase)
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b':')
}

/// Non-empty, bounded, and a single token: no whitespace, no control chars.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_LEN
        && !name.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// 0-indexed anchor line of the site at `offset`.
fn anchor_line(source: &str, offset: usize, at: AnchorAt) -> u32 {
    let line = line_of(source, offset);
    if at == AnchorAt::Needle {
        return line;
    }
    let Some(next) = source
        .get(offset..)
        .and_then(|rest| rest.find('\n'))
        .and_then(|nl| source.get(offset + nl + 1..))
    else {
        return line;
    };
    next.lines()
        .take(LOOKAHEAD_LINES)
        .position(|l| {
            let t = l.trim_start();
            t.starts_with("def ") || t.starts_with("async def ")
        })
        .and_then(|k| u32::try_from(k + 1).ok())
        .map_or(line, |k| line.saturating_add(k))
}

/// The argument region of the call whose `(` ends just before `start`: the
/// text up to the matching `)`, respecting brackets, `'` / `"` / backtick
/// quoting and `\` escapes. Clipped at [`MAX_REGION`] bytes (on a char
/// boundary) when no close is found sooner. Every delimiter is ASCII, so each
/// slice boundary is a char boundary.
fn call_region(source: &str, start: usize) -> Option<&str> {
    let rest = source.get(start..)?;
    let rest = clip(rest, MAX_REGION);
    let b = rest.as_bytes();
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        if let Some(q) = quote {
            match c {
                b'\\' => i += 1,
                b'\n' if q != b'`' => quote = None,
                _ if c == q => quote = None,
                _ => {}
            }
        } else {
            match c {
                b'\'' | b'"' | b'`' => quote = Some(c),
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => {
                    if depth == 0 {
                        return rest.get(..i);
                    }
                    depth -= 1;
                }
                _ => {}
            }
        }
        i += 1;
    }
    Some(rest)
}

/// `s` split at every top-level `sep` (outside brackets and quotes).
fn split_top(s: &str, sep: u8) -> Vec<&str> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let mut start = 0usize;
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        if let Some(q) = quote {
            match c {
                b'\\' => i += 1,
                b'\n' if q != b'`' => quote = None,
                _ if c == q => quote = None,
                _ => {}
            }
        } else {
            match c {
                b'\'' | b'"' | b'`' => quote = Some(c),
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth -= 1,
                _ if c == sep && depth == 0 => {
                    if let Some(piece) = s.get(start..i) {
                        out.push(piece);
                    }
                    start = i + 1;
                }
                _ => {}
            }
        }
        i += 1;
    }
    if let Some(piece) = s.get(start..) {
        out.push(piece);
    }
    out
}

/// A leading `'...'` / `"..."` / backtick literal of `s`: its content and the
/// text after the closing quote. `None` when `s` does not start with a quote
/// or the literal is unterminated.
fn leading_literal(s: &str) -> Option<(&str, &str)> {
    let q = *s.as_bytes().first()?;
    if !matches!(q, b'\'' | b'"' | b'`') {
        return None;
    }
    let b = s.as_bytes();
    let mut i = 1usize;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 1,
            c if c == q => return Some((s.get(1..i)?, s.get(i + 1..)?)),
            _ => {}
        }
        i += 1;
    }
    None
}

/// `ident =` / `ident:` (a keyword or named argument) stripped off the front
/// of `s`; `s` unchanged when it has no such prefix.
fn strip_keyword(s: &str) -> &str {
    let b = s.as_bytes();
    let ident_end = b.iter().position(|c| !is_ident_byte(*c)).unwrap_or(b.len());
    if ident_end == 0
        || !b
            .first()
            .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_')
    {
        return s;
    }
    s.get(ident_end..).and_then(after_assign).unwrap_or(s)
}

/// The text after a leading `=` (not `==`) or `:` (not `::`), both sides
/// trimmed; `None` when `s` does not start with one.
fn after_assign(s: &str) -> Option<&str> {
    let s = s.trim_start();
    let assigns = (s.starts_with('=') && !s.starts_with("=="))
        || (s.starts_with(':') && !s.starts_with("::"));
    if assigns {
        s.get(1..).map(str::trim_start)
    } else {
        None
    }
}

/// A server argument that is exactly one string literal (optionally a
/// keyword / named argument). A template literal with a `${...}` span is not a
/// literal path.
fn server_literal(arg: &str) -> Option<String> {
    let (lit, rest) = leading_literal(strip_keyword(arg.trim()))?;
    if !rest.trim().is_empty() || lit.contains("${") {
        return None;
    }
    Some(lit.to_string())
}

/// The first `key: "/x"` / `key = "/x"` literal in `region` for any key.
fn keyed_literal(region: &str, keys: &[&str]) -> Option<String> {
    let b = region.as_bytes();
    for key in keys {
        for (i, _) in region.match_indices(key) {
            if i > 0 && b.get(i - 1).is_some_and(|c| is_ident_byte(*c)) {
                continue;
            }
            let Some(rest) = region.get(i + key.len()..) else {
                continue;
            };
            if rest.as_bytes().first().is_some_and(|c| is_ident_byte(*c)) {
                continue;
            }
            if let Some((lit, _)) = after_assign(rest).and_then(leading_literal)
                && !lit.contains("${")
            {
                return Some(lit.to_string());
            }
        }
    }
    None
}

/// For `MapHub<T>(`: `start` is just after the `<`. Returns the offset just
/// after the `(` that follows the matching `>`.
fn after_angle(source: &str, start: usize) -> Option<usize> {
    let rest = source.get(start..)?;
    let b = rest.as_bytes();
    let mut depth = 1i32;
    let mut i = 0usize;
    while i < b.len().min(MAX_ANGLE) {
        match b[i] {
            b'<' => depth += 1,
            b'>' => {
                depth -= 1;
                if depth == 0 {
                    let tail = rest.get(i + 1..)?;
                    let skipped = tail.len() - tail.trim_start().len();
                    return tail
                        .trim_start()
                        .starts_with('(')
                        .then_some(start + i + 1 + skipped + 1);
                }
            }
            b'(' | b')' | b';' | b'{' | b'\n' => return None,
            _ => {}
        }
        i += 1;
    }
    None
}

/// The request path a client URL argument names, or `None` when it cannot be
/// read. The argument is split into top-level `+` pieces ([`client_text`]): a
/// string literal is static text, a template literal is static text with each
/// `${...}` span a dynamic piece, anything else is dynamic. With a dynamic piece, the text
/// after the LAST one is the path when it starts with `/`; otherwise a static
/// head that is a full `scheme://host/path` URL is. The scheme + authority are
/// stripped and a query / fragment is dropped; only a result starting with
/// `/` is accepted.
///
/// `"ws://" + location.host + "/ws/chat"` -> `/ws/chat`,
/// `` `${base}/ws/admin` `` -> `/ws/admin`,
/// `"wss://api.example.com/echo"` -> `/echo`, `url` -> `None`.
fn client_path(arg: &str) -> Option<String> {
    let text = client_text(arg)?;
    let candidate = match (text.find(DYNAMIC), text.rfind(DYNAMIC)) {
        (Some(first), Some(last)) => {
            let tail = text.get(last + DYNAMIC.len_utf8()..).unwrap_or("");
            let head = text.get(..first).unwrap_or("");
            if tail.starts_with('/') {
                tail.to_string()
            } else if head.contains("://") {
                head.to_string()
            } else {
                return None;
            }
        }
        _ => text,
    };
    let path = normalise_ws_path(&candidate);
    let path = path.split(['?', '#']).next().unwrap_or("");
    path.starts_with('/').then(|| path.to_string())
}

/// A client URL argument as text: split into top-level `+` pieces, a string
/// literal is static text, a template literal is static text with each
/// `${...}` span a [`DYNAMIC`], anything else is one [`DYNAMIC`]. `None` for an
/// empty argument. Shared by [`client_path`] and [`client_host`], so the two
/// read one argument identically.
fn client_text(arg: &str) -> Option<String> {
    let arg = arg.trim();
    if arg.is_empty() {
        return None;
    }
    let mut text = String::new();
    for piece in split_top(arg, b'+') {
        let piece = piece.trim();
        match leading_literal(piece) {
            Some((lit, rest)) if rest.trim().is_empty() => {
                if piece.starts_with('`') {
                    push_template(&mut text, lit);
                } else {
                    text.push_str(lit);
                }
            }
            _ => text.push(DYNAMIC),
        }
    }
    Some(text)
}

/// The URL schemes a client argument's static head may start with for its
/// authority to be read ([`client_host`]), the ones [`normalise_ws_path`]
/// strips.
const WS_SCHEMES: [&str; 4] = ["ws://", "wss://", "http://", "https://"];

/// CB.21: the authority a client URL argument dials, `host[:port]` with any
/// userinfo dropped, or `None` when the source does not spell it.
///
/// Read off the argument's STATIC HEAD, the text before its first dynamic
/// piece (the whole argument when it is one literal), which must start with
/// one of [`WS_SCHEMES`]. With a dynamic piece after the head, the authority
/// must END inside it (a `/`, `?` or `#` follows it), so a port or host glued
/// to a variable is not a host. The authority itself goes through
/// `code_domain::endpoint::client_url_split`, the HTTP clients' rule, which
/// accepts only a literal `host[:port]`.
///
/// `"ws://chat-svc:8080/ws"` -> `chat-svc:8080`,
/// `"wss://api.example.com/ws?token=" + t` -> `api.example.com`,
/// `"ws://" + location.host + "/ws"` -> `None`, `` `${base}/ws` `` -> `None`,
/// `"ws://chat-svc:" + port + "/ws"` -> `None`.
fn client_host(arg: &str) -> Option<String> {
    let text = client_text(arg)?;
    let (head, dynamic) = match text.find(DYNAMIC) {
        Some(i) => (text.get(..i)?, true),
        None => (text.as_str(), false),
    };
    let rest = WS_SCHEMES.iter().find_map(|s| head.strip_prefix(s))?;
    if dynamic && !rest.contains(['/', '?', '#']) {
        return None;
    }
    client_url_split(head).0
}

/// CB.21: the host one client site dials. Only a row that reads a client URL
/// ([`PathRead::Client`]) can name one; every other client row (a channel
/// class, a topic, an import line) records none.
fn site_host(source: &str, offset: usize, r: &WsRow) -> Option<String> {
    let PathRead::Client(n) = r.read else {
        return None;
    };
    let after = offset.checked_add(r.needle.len())?;
    client_host(split_top(call_region(source, after)?, b',').get(n)?)
}

/// Template-literal content into `out`, each `${...}` span as [`DYNAMIC`].
fn push_template(out: &mut String, lit: &str) {
    let mut rest = lit;
    while let Some(open) = rest.find("${") {
        out.push_str(rest.get(..open).unwrap_or(""));
        out.push(DYNAMIC);
        let body = rest.get(open + 2..).unwrap_or("");
        let mut depth = 1i32;
        let mut end = body.len();
        for (i, c) in body.char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        rest = body.get(end..).unwrap_or("");
    }
    out.push_str(rest);
}

/// Strip `ws://` / `wss://` / `http://` / `https://` and the authority: the
/// path from the first `/` after the host, or the empty string when the URL
/// names no path. Anything else is returned unchanged.
fn normalise_ws_path(url: &str) -> String {
    for scheme in ["ws://", "wss://", "http://", "https://"] {
        if let Some(rest) = url.strip_prefix(scheme) {
            return rest
                .find('/')
                .and_then(|slash| rest.get(slash..))
                .unwrap_or("")
                .to_string();
        }
    }
    url.to_string()
}

// ---- DG.5: paths read off the file's routing structure -------------------

/// One file's code-byte `{ }` blocks: every `{` outside a literal or comment
/// ([`CodeMap::new`], the C family's lexing, which covers Rust's raw strings
/// and Scala's `"""` blocks and nesting comments) paired with its matching
/// `}`, or the end of input when it never closes.
struct Blocks {
    map: CodeMap,
    /// `(open, close)` of each block, sorted by `open`.
    pairs: Vec<(usize, usize)>,
}

impl Blocks {
    fn new(source: &str) -> Self {
        let map = CodeMap::new(source);
        let mut pairs: Vec<(usize, usize)> = Vec::new();
        let mut open: Vec<usize> = Vec::new();
        for (i, c) in source.bytes().enumerate() {
            if !matches!(c, b'{' | b'}') || !map.is_code(i) {
                continue;
            }
            if c == b'{' {
                open.push(pairs.len());
                pairs.push((i, source.len()));
            } else if let Some(pair) = open.pop().and_then(|k| pairs.get_mut(k)) {
                pair.1 = i;
            }
        }
        Blocks { map, pairs }
    }

    /// True when byte `pos` is code, not inside a literal or comment.
    fn is_code(&self, pos: usize) -> bool {
        self.map.is_code(pos)
    }

    /// True when the block whose `{` is byte `open` holds byte `at`.
    fn encloses(&self, open: usize, at: usize) -> bool {
        self.pairs
            .binary_search_by_key(&open, |p| p.0)
            .ok()
            .and_then(|k| self.pairs.get(k))
            .is_some_and(|&(o, close)| o < at && at < close)
    }
}

/// The name of the `fn` whose body holds byte `offset`: the nearest
/// `fn <ident>` before it (in code) whose body `{` — the first code `{` after
/// its name at bracket depth 0, where a `;` first means no body (a trait
/// method declaration) — opens a block that still spans `offset`. A fn nested
/// in another wins while the site is in its body; after its body ends, the
/// outer fn does.
fn enclosing_rust_fn<'s>(source: &'s str, offset: usize, blocks: &Blocks) -> Option<&'s str> {
    let b = source.as_bytes();
    for (i, _) in source.get(..offset)?.rmatch_indices("fn") {
        let keyword = (i == 0 || b.get(i - 1).is_some_and(|c| !is_ident_byte(*c)))
            && b.get(i + 2).is_some_and(u8::is_ascii_whitespace)
            && blocks.is_code(i);
        if !keyword {
            continue;
        }
        let Some(rest) = source.get(i + 2..) else {
            continue;
        };
        let name_at = i + 2 + (rest.len() - rest.trim_start().len());
        let name_len = b
            .get(name_at..)
            .map_or(0, |t| t.iter().take_while(|c| is_ident_byte(**c)).count());
        let Some(name) = source.get(name_at..name_at + name_len) else {
            continue;
        };
        if name.is_empty() || name.as_bytes().first().is_some_and(u8::is_ascii_digit) {
            continue;
        }
        if body_open(b, name_at + name_len, offset, blocks)
            .is_some_and(|o| blocks.encloses(o, offset))
        {
            return Some(name);
        }
    }
    None
}

/// The body `{` of the fn whose signature starts at `from`: the first code `{`
/// at `(` / `[` depth 0 before `limit` (a struct pattern parameter's `{` is
/// inside the parentheses, an array type's `;` inside its brackets); `None`
/// when a depth-0 `;` comes first.
fn body_open(b: &[u8], from: usize, limit: usize, blocks: &Blocks) -> Option<usize> {
    let mut depth = 0i32;
    for i in from..limit.min(b.len()) {
        let c = *b.get(i)?;
        if !matches!(c, b'(' | b')' | b'[' | b']' | b'{' | b';') || !blocks.is_code(i) {
            continue;
        }
        match c {
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth -= 1,
            b'{' if depth == 0 => return Some(i),
            b';' if depth == 0 => return None,
            _ => {}
        }
    }
    None
}

/// axum's method routers: `get(h)` ... `any(h)` take the handler as their one
/// argument, `on(filter, h)` as its second.
const AXUM_METHOD_ROUTERS: &[&str] = &[
    "get", "post", "put", "delete", "patch", "head", "options", "trace", "connect", "any", "on",
];

/// The path of the first `.route("/p", <method router>)` call in the file (in
/// source order, in code) whose method router names `handler`
/// ([`names_handler`]). A fn mounted on two routes keeps the first.
fn axum_route_of(source: &str, handler: &str, blocks: &Blocks) -> Option<String> {
    source
        .match_indices(".route(")
        .filter(|(i, _)| blocks.is_code(*i))
        .find_map(|(i, needle)| {
            let args = split_top(call_region(source, i + needle.len())?, b',');
            let path = server_literal(args.first()?)?;
            names_handler(args.get(1)?, handler).then_some(path)
        })
}

/// True when the method-router expression `expr` routes to `handler`: a call
/// chain `get(index).post(chat).layer(..)` whose calls are each `<path>(..)`,
/// one of them an [`AXUM_METHOD_ROUTERS`] entry (bare, or through a path such
/// as `routing::get` / `axum::routing::any`) whose handler argument is exactly
/// `handler`.
fn names_handler(expr: &str, handler: &str) -> bool {
    let mut rest = expr.trim();
    loop {
        let path_len = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == ':'))
            .unwrap_or(rest.len());
        let (Some(path), Some(tail)) = (rest.get(..path_len), rest.get(path_len..)) else {
            return false;
        };
        let Some(args_from) = tail.trim_start().strip_prefix('(') else {
            return false;
        };
        let Some(region) = call_region(args_from, 0) else {
            return false;
        };
        let method = path.rsplit("::").next().unwrap_or(path);
        if AXUM_METHOD_ROUTERS.contains(&method) {
            let args: Vec<&str> = split_top(region, b',')
                .into_iter()
                .map(str::trim)
                .filter(|a| !a.is_empty())
                .collect();
            let named = match (method, args.as_slice()) {
                ("on", [_, h]) => Some(*h),
                ("on", _) => None,
                (_, [h]) => Some(*h),
                _ => None,
            };
            if named == Some(handler) {
                return true;
            }
        }
        // Past the call's `)`: the next link of the chain, if any.
        let Some(after) = args_from.get(region.len() + 1..) else {
            return false;
        };
        match after.trim_start().strip_prefix('.') {
            Some(next) => rest = next.trim_start(),
            None => return false,
        }
    }
}

/// Akka HTTP's path matchers that stand for a value, read as the handler
/// parameter shapes the ws resolver knows (`:x` is one segment,
/// `is_handler_param`); a bare `*` stays literal there and pairs nothing.
/// The table DG.10 gives the Scala parser's Akka ROUTEs, so one Akka source
/// reads one path on both sides.
const AKKA_MATCHERS: &[(&str, &str)] = &[
    ("Segment", ":segment"),
    ("IntNumber", ":id"),
    ("LongNumber", ":id"),
    ("HexIntNumber", ":id"),
    ("HexLongNumber", ":id"),
    ("DoubleNumber", ":id"),
    ("JavaUUID", ":id"),
    ("Remaining", "*"),
    ("RemainingPath", "*"),
];

/// The path Akka / Pekko HTTP's directives route the needle at `offset` to:
/// every `path(..)` / `pathPrefix(..)` call before it (in code; an identifier
/// byte before `path` disqualifies it, so `subpath(` is no directive) that
/// encloses it ([`directive_encloses`]), composed outer to inner. `None` when
/// no directive encloses the site, or one that does reads unreadable — its
/// matcher ([`akka_matcher_path`]), or a `|` alternative.
fn akka_directive_path(source: &str, offset: usize, blocks: &Blocks) -> Option<String> {
    let head = source.get(..offset)?;
    let b = source.as_bytes();
    let mut sites: Vec<(usize, usize)> = ["path(", "pathPrefix("]
        .iter()
        .flat_map(|d| head.match_indices(d).map(|(i, n)| (i, i + n.len())))
        .collect();
    sites.sort_unstable();
    let mut path = String::new();
    for (at, args) in sites {
        if (at > 0 && b.get(at - 1).is_some_and(|c| is_ident_byte(*c))) || !blocks.is_code(at) {
            continue;
        }
        let Some(region) = call_region(source, args) else {
            continue;
        };
        let close = args + region.len();
        if b.get(close) != Some(&b')') {
            continue;
        }
        match directive_encloses(source, at, close, offset, blocks) {
            Encloses::No => {}
            Encloses::Unreadable => return None,
            Encloses::Yes => path.push_str(&akka_matcher_path(region)?),
        }
    }
    (!path.is_empty()).then_some(path)
}

/// Whether one directive call routes a site.
enum Encloses {
    /// Its block does not hold the site.
    No,
    /// It routes the site, alone or `&`-joined with other directives.
    Yes,
    /// It routes the site as one `|` alternative: no single path.
    Unreadable,
}

/// Whether the directive call `at..=close` (its `)` at `close`) routes byte
/// `offset`: the block it is applied to — a `{ .. }` or `( .. )` right after
/// its `)` — holds the site, or it sits in a parenthesised directive group
/// (not a call's argument list) applied to such a block:
/// `(path("ws") & get) { .. }` routes through it, `(path("a") | path("b"))
/// { .. }` through no single path.
fn directive_encloses(
    source: &str,
    at: usize,
    close: usize,
    offset: usize,
    blocks: &Blocks,
) -> Encloses {
    if applied_block_holds(source, close, offset, blocks) {
        return Encloses::Yes;
    }
    let Some(open) = group_open(source, at, blocks) else {
        return Encloses::No;
    };
    // A call's argument list (`handleRejections(h)`, `concat(..)`) is no
    // directive group.
    let called = source
        .get(..open)
        .and_then(|before| before.trim_end().bytes().last())
        .is_some_and(|c| is_ident_byte(c) || matches!(c, b']' | b')'));
    if called {
        return Encloses::No;
    }
    let Some(group) = call_region(source, open + 1) else {
        return Encloses::No;
    };
    let group_close = open + 1 + group.len();
    if source.as_bytes().get(group_close) != Some(&b')')
        || !applied_block_holds(source, group_close, offset, blocks)
    {
        return Encloses::No;
    }
    if split_top(group, b'|').len() > 1 {
        Encloses::Unreadable
    } else {
        Encloses::Yes
    }
}

/// True when the `{ .. }` / `( .. )` block that starts right after the `)` at
/// `close` (whitespace between) holds byte `offset`.
fn applied_block_holds(source: &str, close: usize, offset: usize, blocks: &Blocks) -> bool {
    let Some(tail) = source.get(close + 1..) else {
        return false;
    };
    let open = close + 1 + (tail.len() - tail.trim_start().len());
    match source.as_bytes().get(open) {
        Some(b'{') => blocks.encloses(open, offset),
        Some(b'(') => call_region(source, open + 1)
            .is_some_and(|args| open < offset && offset < open + 1 + args.len()),
        _ => false,
    }
}

/// The `(` of the innermost parenthesised group holding byte `at`, scanned
/// back over code bytes at most [`MAX_REGION`] bytes; `None` when a code `{`,
/// `}` or `;` at depth 0 (the start of the enclosing statement or block)
/// comes first.
fn group_open(source: &str, at: usize, blocks: &Blocks) -> Option<usize> {
    let b = source.as_bytes();
    let mut depth = 0i32;
    for i in (at.saturating_sub(MAX_REGION)..at).rev() {
        let c = *b.get(i)?;
        if !matches!(c, b'(' | b')' | b'{' | b'}' | b';') || !blocks.is_code(i) {
            continue;
        }
        match c {
            b')' => depth += 1,
            b'(' if depth == 0 => return Some(i),
            b'(' => depth -= 1,
            _ if depth == 0 => return None,
            _ => {}
        }
    }
    None
}

/// One Akka path directive's argument as a path: segments split at top-level
/// `/`, a `"..."` literal verbatim (an inner `/` kept), a value matcher by
/// [`AKKA_MATCHERS`]; `path("ws" / "chat")` -> `/ws/chat`, `path("rooms" /
/// IntNumber)` -> `/rooms/:id`. `None` for any other token (a `val` matcher,
/// `separateOnSlashes(..)`, `Slash`, an interpolated string), an empty
/// segment or a second argument.
fn akka_matcher_path(args: &str) -> Option<String> {
    if split_top(args, b',').len() != 1 {
        return None;
    }
    let mut path = String::new();
    for seg in split_top(args, b'/') {
        let seg = seg.trim();
        let piece = match leading_literal(seg) {
            Some((lit, rest))
                if seg.starts_with('"') && rest.trim().is_empty() && !lit.is_empty() =>
            {
                lit
            }
            _ => AKKA_MATCHERS
                .iter()
                .find(|(token, _)| *token == seg)
                .map(|(_, param)| *param)?,
        };
        path.push('/');
        path.push_str(piece);
    }
    Some(path)
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
    fn handlers(src: &str) -> WsNodes {
        extract_ws_handler_nodes(src, "t.src", module_id(), repo())
    }
    fn clients(src: &str) -> WsNodes {
        extract_ws_client_nodes(src, "t.src", module_id(), repo())
    }
    /// Qnames in node order.
    fn qnames(out: &WsNodes) -> Vec<String> {
        out.nodes
            .iter()
            .map(|n| out.nav.qname_by_id[&n.id].clone())
            .collect()
    }
    /// `(qname, 0-indexed line)` for every anchor, in anchor order.
    fn anchored(out: &WsNodes) -> Vec<(String, u32)> {
        out.anchors
            .iter()
            .map(|a| (out.nav.qname_by_id[&a.node].clone(), a.line))
            .collect()
    }

    const FASTAPI: &str = "from fastapi import FastAPI, WebSocket\n\
        \n\
        app = FastAPI()\n\
        \n\
        \n\
        @app.websocket(\"/ws/chat\")\n\
        async def chat(websocket: WebSocket):\n\
        \x20   await websocket.accept()\n\
        \n\
        \n\
        @app.websocket(\"/ws/admin\")\n\
        async def admin(websocket: WebSocket):\n\
        \x20   await websocket.close()\n";

    const GORILLA_HUB: &str = "package server\n\
        \n\
        import (\n\
        \t\"net/http\"\n\
        \n\
        \t\"github.com/gorilla/websocket\"\n\
        )\n\
        \n\
        var upgrader = websocket.Upgrader{}\n\
        \n\
        func ServeWs(w http.ResponseWriter, r *http.Request) {\n\
        \tconn, _ := upgrader.Upgrade(w, r, nil)\n\
        \tdefer conn.Close()\n\
        }\n";

    #[test]
    fn detects_ws_handler() {
        let out = handlers("ws.on('connection', (socket) => { socket.send('hi'); });");
        assert_eq!(qnames(&out), vec!["ws:ws"]);
    }

    #[test]
    fn detects_ws_gateway() {
        let out = handlers("@WebSocketGateway()\nexport class ChatGateway {}");
        assert_eq!(qnames(&out), vec!["ws:default"]);
    }

    #[test]
    fn detects_ws_client() {
        let out = clients("const ws = new WebSocket('ws://localhost:8080/chat');");
        assert_eq!(qnames(&out), vec!["ws_client:/chat"]);
    }

    #[test]
    fn anchors_each_node_at_its_needle_line() {
        let client =
            "// chat\nexport function f() {\n  const ws = new WebSocket('ws://h/chat');\n}";
        let out = clients(client);
        assert_eq!(out.nodes.len(), 1);
        assert_eq!(
            out.anchors,
            vec![Anchor {
                node: out.nodes[0].id,
                line: 2
            }]
        );

        // No path at the upgrade call: the node falls back to `ws`, and the
        // call inside the handler function locates it.
        let server = "package api\n\nimport \"github.com/gorilla/websocket\"\n\n\
                      func ServeWs(w http.ResponseWriter, r *http.Request) {\n\
                      \tconn, _ := upgrader.Upgrade(w, r, nil)\n}\n";
        let out = handlers(server);
        assert_eq!(qnames(&out), vec!["ws:ws"]);
        assert_eq!(
            out.anchors,
            vec![Anchor {
                node: out.nodes[0].id,
                line: 5
            }]
        );
    }

    #[test]
    fn every_occurrence_is_read() {
        let out = handlers(FASTAPI);
        assert_eq!(qnames(&out), vec!["ws:/ws/chat", "ws:/ws/admin"]);
        assert_eq!(out.anchors.len(), 2);
    }

    #[test]
    fn fastapi_anchor_moves_to_def_line() {
        let out = handlers(FASTAPI);
        assert_eq!(
            anchored(&out),
            vec![
                ("ws:/ws/chat".to_string(), 6),
                ("ws:/ws/admin".to_string(), 11)
            ],
            "each decorator anchors at the `async def` below it"
        );
        // A stacked decorator between the two still finds the def.
        let src =
            "import fastapi\n@router.websocket(\"/live\")\n@traced\ndef live(ws):\n    pass\n";
        assert_eq!(anchored(&handlers(src)), vec![("ws:/live".to_string(), 3)]);
        // No def within reach: the decorator's own line.
        let src = "import fastapi\nx = app.websocket(\"/raw\")\n";
        assert_eq!(anchored(&handlers(src)), vec![("ws:/raw".to_string(), 1)]);
    }

    #[test]
    fn gorilla_upgrade_call_is_the_handler_site() {
        let out = handlers(GORILLA_HUB);
        assert_eq!(qnames(&out), vec!["ws:ws"]);
        assert_eq!(
            anchored(&out),
            vec![("ws:ws".to_string(), 11)],
            "the `upgrader.Upgrade(` line inside ServeWs, not the Upgrader var"
        );
        // `.Upgrade(` without the gorilla import is not a WebSocket handler.
        let src = "package h2\n\nfunc f() { conn.Upgrade(w, r) }\n";
        assert!(handlers(src).nodes.is_empty());
    }

    #[test]
    fn gorilla_import_alone_mints_nothing() {
        let src = "package server\n\nimport (\n\t\"github.com/gorilla/websocket\"\n)\n\n\
                   var upgrader = websocket.Upgrader{\n\tReadBufferSize: 1024,\n}\n";
        let out = handlers(src);
        assert!(out.nodes.is_empty(), "{:?}", qnames(&out));
        assert!(out.anchors.is_empty());
    }

    /// Run-2 verifier: the dropped `gorilla/websocket` needle read the text
    /// after the import's closing quote as a "literal" and minted
    /// `ws:\n)\n\nvar upgrader = websocket.Upgr...`. No name is ever multi-line.
    #[test]
    fn gorilla_hub_never_mints_a_multiline_qname() {
        for out in [handlers(GORILLA_HUB), clients(GORILLA_HUB)] {
            for q in qnames(&out) {
                assert!(
                    !q.chars().any(|c| c.is_whitespace() || c.is_control()),
                    "multi-line / spaced qname {q:?}"
                );
            }
        }
        assert_eq!(qnames(&handlers(GORILLA_HUB)), vec!["ws:ws"]);
        // The same shape through socket.io-client's import line.
        let src = "import { io } from \"socket.io-client\";\nconst s = io(base);\n";
        assert_eq!(qnames(&clients(src)), vec!["ws_client:ws"]);
    }

    #[test]
    fn nhooyr_accept_is_a_handler() {
        let src = "package chat\n\nimport \"nhooyr.io/websocket\"\n\n\
                   func Echo(w http.ResponseWriter, r *http.Request) {\n\
                   \tc, err := websocket.Accept(w, r, nil)\n}\n";
        let out = handlers(src);
        assert_eq!(anchored(&out), vec![("ws:ws".to_string(), 5)]);
        let coder = src.replace("nhooyr.io/websocket", "github.com/coder/websocket");
        assert_eq!(qnames(&handlers(&coder)), vec!["ws:ws"]);
        // Ungated: some other `websocket.Accept(` package is not nhooyr.
        let other = src.replace("nhooyr.io/websocket", "example.com/websocket");
        assert!(handlers(&other).nodes.is_empty());
    }

    #[test]
    fn server_endpoint_value_forms() {
        for (src, want) in [
            (
                "import jakarta.websocket.server.ServerEndpoint;\n@ServerEndpoint(\"/a\")\nclass A {}",
                "ws:/a",
            ),
            (
                "import javax.websocket.server.ServerEndpoint;\n@ServerEndpoint(value = \"/b\", encoders = {E.class})\nclass B {}",
                "ws:/b",
            ),
            (
                "import jakarta.websocket.server.ServerEndpoint;\n@ServerEndpoint(encoders = {E.class}, value = \"/c\")\nclass C {}",
                "ws:/c",
            ),
        ] {
            assert_eq!(qnames(&handlers(src)), vec![want], "{src}");
        }
        // No JSR-356 import: the annotation name alone is not enough.
        assert!(
            handlers("@ServerEndpoint(\"/d\")\nclass D {}")
                .nodes
                .is_empty()
        );
    }

    #[test]
    fn spring_add_handler_reads_arg_one() {
        let src = "import org.springframework.web.socket.config.annotation.WebSocketHandlerRegistry;\n\
                   class C {\n  void reg(WebSocketHandlerRegistry registry) {\n\
                   \x20   registry.addHandler(new EchoHandler(), \"/echo\").setAllowedOrigins(\"*\");\n\
                   \x20   registry.addHandler(chatHandler(), \"/chat\", \"/chat2\");\n  }\n}\n";
        let out = handlers(src);
        assert_eq!(qnames(&out), vec!["ws:/echo", "ws:/chat"]);
        assert_eq!(
            anchored(&out),
            vec![("ws:/echo".to_string(), 3), ("ws:/chat".to_string(), 4)]
        );
        // STOMP endpoints read argument 0.
        let stomp = "import org.springframework.web.socket.config.annotation.StompEndpointRegistry;\n\
                     registry.addEndpoint(\"/stomp\").withSockJS();\n";
        assert_eq!(qnames(&handlers(stomp)), vec!["ws:/stomp"]);
    }

    #[test]
    fn signalr_maphub_after_angle() {
        let src = "using Microsoft.AspNetCore.SignalR;\nvar app = builder.Build();\n\
                   app.MapHub<ChatHub>(\"/hubs/chat\");\n\
                   app.MapHub<Ns.Hub<Other>>( \"/hubs/nested\" );\n";
        let out = handlers(src);
        assert_eq!(qnames(&out), vec!["ws:/hubs/chat", "ws:/hubs/nested"]);
        // Unreadable path: the generic fallback, still anchored.
        let src = "builder.Services.AddSignalR();\napp.MapHub<ChatHub>(HubPath);\n";
        assert_eq!(anchored(&handlers(src)), vec![("ws:ws".to_string(), 1)]);
    }

    #[test]
    fn client_concat_static_tail() {
        let out = clients("const ws = new WebSocket(\"ws://\" + location.host + \"/ws/chat\");");
        assert_eq!(qnames(&out), vec!["ws_client:/ws/chat"]);
        // A query string appended to a full URL: the static head names the path.
        let out = clients("new WebSocket(\"wss://api.example.com/ws?token=\" + token)");
        assert_eq!(qnames(&out), vec!["ws_client:/ws"]);
        // A port glued to a dynamic host is not a path.
        let out = clients("new WebSocket(\"ws://\" + host + \":8080\")");
        assert_eq!(qnames(&out), vec!["ws_client:ws"]);
    }

    #[test]
    fn client_template_dynamic_head() {
        let out = clients("return new WebSocket(`${base}/ws/admin`);");
        assert_eq!(qnames(&out), vec!["ws_client:/ws/admin"]);
        let out = clients("new WebSocket(`${proto}://${location.host}/live`)");
        assert_eq!(qnames(&out), vec!["ws_client:/live"]);
        let out = clients("new WebSocket(`wss://api.example.com/notifications`)");
        assert_eq!(qnames(&out), vec!["ws_client:/notifications"]);
    }

    #[test]
    fn client_variable_is_generic() {
        let src =
            "export function connectUnknown(url: string) {\n  return new WebSocket(url);\n}\n";
        let out = clients(src);
        assert_eq!(qnames(&out), vec!["ws_client:ws"]);
        assert_eq!(out.anchors.len(), 1);
    }

    #[test]
    fn every_client_site_is_read_and_anchored() {
        let src = "export function a() {\n  return new WebSocket(\"wss://h/echo\");\n}\n\
                   export function b() {\n  return new WebSocket(`wss://h/notifications`);\n}\n\
                   export function c() {\n  return new WebSocket(\"wss://h/echo\");\n}\n";
        let out = clients(src);
        assert_eq!(
            qnames(&out),
            vec!["ws_client:/echo", "ws_client:/notifications"]
        );
        assert_eq!(
            anchored(&out),
            vec![
                ("ws_client:/echo".to_string(), 1),
                ("ws_client:/notifications".to_string(), 4),
                ("ws_client:/echo".to_string(), 7),
            ]
        );
    }

    #[test]
    fn signalr_and_go_and_python_clients() {
        let ts = "import * as signalR from \"@microsoft/signalr\";\n\
                  const c = new signalR.HubConnectionBuilder().withUrl(\"https://h/hubs/chat\").build();\n";
        assert_eq!(qnames(&clients(ts)), vec!["ws_client:/hubs/chat"]);
        // `.withUrl(` without the SignalR import is someone else's builder.
        assert!(clients("b.withUrl(\"/hubs/chat\")").nodes.is_empty());
        let cs = "using Microsoft.AspNetCore.SignalR.Client;\n\
                  var c = new HubConnectionBuilder().WithUrl(\"https://localhost:5001/chathub\").Build();\n";
        assert_eq!(qnames(&clients(cs)), vec!["ws_client:/chathub"]);
        let go = "import \"github.com/gorilla/websocket\"\n\
                  c, _, _ := websocket.DefaultDialer.Dial(\"ws://localhost:8080/ws/echo\", nil)\n\
                  d, _, _ := websocket.DefaultDialer.DialContext(ctx, \"ws://h/ws/ctx\", nil)\n";
        assert_eq!(
            qnames(&clients(go)),
            vec!["ws_client:/ws/echo", "ws_client:/ws/ctx"]
        );
        let nh = "import \"nhooyr.io/websocket\"\nc, _, err := websocket.Dial(ctx, \"ws://h/sub\", nil)\n";
        assert_eq!(qnames(&clients(nh)), vec!["ws_client:/sub"]);
        let py =
            "import websockets\nasync with websockets.connect(\"ws://localhost:8765/py\") as ws:\n";
        assert_eq!(qnames(&clients(py)), vec!["ws_client:/py"]);
    }

    #[test]
    fn dart_dio_is_not_socketio() {
        let src = "import 'package:dio/dio.dart';\nfinal dio = Dio();\nfinal r = ratio(2);\n";
        let out = clients(src);
        assert!(out.nodes.is_empty(), "{:?}", qnames(&out));
        // A real socket.io call still reads.
        let out = clients("const socket = io(\"https://api.example.com/realtime\");");
        assert_eq!(qnames(&out), vec!["ws_client:/realtime"]);
    }

    #[test]
    fn websocketserver_import_is_not_a_handler() {
        let src = "import { WebSocketServer } from 'ws';\nexport const x = 1;\n";
        assert!(handlers(src).nodes.is_empty());
        let src = "import { WebSocketServer } from 'ws';\n\
                   const wss = new WebSocketServer({ port: 8080, path: '/live' });\n";
        assert_eq!(qnames(&handlers(src)), vec!["ws:/live"]);
        let src = "const wss = new WebSocket.Server({ host: \"localhost\", port: 8080 });\n";
        assert_eq!(
            qnames(&handlers(src)),
            vec!["ws:ws"],
            "an object literal with no `path` key is not a path"
        );
    }

    #[test]
    fn python_route_forms() {
        let src = "from starlette.routing import WebSocketRoute\n\
                   routes = [WebSocketRoute(\"/ws/feed\", endpoint=Feed)]\n\
                   app.add_websocket_route(\"/ws/extra\", extra)\n";
        assert_eq!(qnames(&handlers(src)), vec!["ws:/ws/feed", "ws:/ws/extra"]);
        // Keyword path argument.
        let src = "from fastapi import APIRouter\n@router.websocket(path=\"/kw\")\nasync def kw(ws): ...\n";
        assert_eq!(qnames(&handlers(src)), vec!["ws:/kw"]);
        // No FastAPI / Starlette import: `.websocket(` is not a route.
        assert!(
            handlers("@app.websocket(\"/x\")\ndef x(): ...\n")
                .nodes
                .is_empty()
        );
    }

    #[test]
    fn server_paths_are_verbatim() {
        let src = "import fastapi\n@app.websocket(\"/chat/{room}\")\nasync def room(ws): ...\n";
        assert_eq!(qnames(&handlers(src)), vec!["ws:/chat/{room}"]);
    }

    #[test]
    fn scan_is_capped_per_row() {
        let src = "new WebSocket(\"/a\");\n".repeat(MAX_HITS_PER_NEEDLE + 10);
        assert_eq!(clients(&src).anchors.len(), MAX_HITS_PER_NEEDLE);
    }

    #[test]
    fn rows_that_read_arguments_are_call_shaped() {
        for r in HANDLER_ROWS.iter().chain(CLIENT_ROWS) {
            match r.read {
                PathRead::Arg(_)
                | PathRead::KeyedOrArg(_)
                | PathRead::Client(_)
                | PathRead::AxumRoute
                | PathRead::AkkaDirective => {
                    assert!(r.needle.ends_with('('), "{}", r.needle)
                }
                PathRead::AfterAngle => assert!(r.needle.ends_with('<'), "{}", r.needle),
                PathRead::ClassBeforeLt => assert!(r.needle.starts_with("< "), "{}", r.needle),
                PathRead::LineLiteral => assert!(r.needle.ends_with('"'), "{}", r.needle),
                PathRead::Generic | PathRead::Default => {}
            }
            // A row that never falls back must read something.
            if r.accept != Accept::ReadOrFallback {
                assert!(
                    !matches!(r.read, PathRead::Generic | PathRead::Default),
                    "{}",
                    r.needle
                );
            }
        }
    }

    // ---- LA.18c: channel-keyed frameworks --------------------------------

    const CHAT_CHANNEL_RB: &str = "class ChatChannel < ApplicationCable::Channel\n\
        \x20 def subscribed\n\
        \x20   stream_from \"chat_#{params[:room]}\"\n\
        \x20 end\n\
        \n\
        \x20 def speak(data)\n\
        \x20   ActionCable.server.broadcast(\"chat\", data)\n\
        \x20 end\n\
        end\n";

    #[test]
    fn actioncable_class_is_the_channel() {
        let out = handlers(CHAT_CHANNEL_RB);
        assert_eq!(anchored(&out), vec![("ws:ChatChannel".to_string(), 0)]);
        // The broadcast inside a method is not a handler (it minted
        // `ws:default` HANDLED_BY `speak` before LA.18c).
        assert!(!qnames(&out).contains(&"ws:default".to_string()));
        // ActionCable's own base, and a namespaced compact class.
        let src = "class Admin::AuditChannel < ActionCable::Channel::Base\nend\n";
        assert_eq!(qnames(&handlers(src)), vec!["ws:Admin::AuditChannel"]);
        // A class named after something that merely starts with the base.
        let src = "class Helper < ApplicationCable::ChannelHelper\nend\n";
        assert!(handlers(src).nodes.is_empty());
    }

    #[test]
    fn application_cable_base_is_not_a_channel() {
        // Rails' generated app/channels/application_cable/channel.rb.
        let src = "module ApplicationCable\n  class Channel < ActionCable::Channel::Base\n  end\nend\n";
        let out = handlers(src);
        assert!(out.nodes.is_empty(), "{:?}", qnames(&out));
        let src = "class ApplicationCable::Channel < ActionCable::Channel::Base\nend\n";
        assert!(handlers(src).nodes.is_empty());
        // The needle without a `class Name` in front of it reads nothing.
        let src = "# subclasses say `< ApplicationCable::Channel`\n";
        assert!(handlers(src).nodes.is_empty());
    }

    #[test]
    fn actioncable_client_channel_key() {
        let src = "import { createConsumer } from \"@rails/actioncable\"\n\
                   const consumer = createConsumer()\n\
                   export function joinChat(room) {\n\
                   \x20 return consumer.subscriptions.create({ channel: \"ChatChannel\", room: room }, {\n\
                   \x20   received(data) { console.log(data) }\n\
                   \x20 })\n\
                   }\n";
        let out = clients(src);
        assert_eq!(anchored(&out), vec![("ws_client:ChatChannel".to_string(), 3)]);
        // The positional string form.
        let src = "consumer.subscriptions.create(\"NotificationsChannel\", { received() {} })\n";
        assert_eq!(
            qnames(&clients(src)),
            vec!["ws_client:NotificationsChannel"]
        );
        // A channel class outside the `...Channel` shape is a declared miss.
        let src = "consumer.subscriptions.create({ channel: \"Notifications\" })\n";
        assert!(clients(src).nodes.is_empty());
    }

    #[test]
    fn stripe_subscriptions_create_is_not_a_client() {
        for src in [
            "const sub = await stripe.subscriptions.create({ customer: \"cus_1\", items: [{ price: p }] });\n",
            "await stripe.subscriptions.create(params);\n",
            "await stripe.subscriptions.create(\"sub_1\");\n",
        ] {
            let out = clients(src);
            assert!(out.nodes.is_empty(), "{src}: {:?}", qnames(&out));
            assert!(out.anchors.is_empty());
        }
    }

    const ENDPOINT_EX: &str = "defmodule AppWeb.Endpoint do\n\
        \x20 use Phoenix.Endpoint, otp_app: :app\n\
        \n\
        \x20 socket \"/socket\", AppWeb.UserSocket, websocket: true, longpoll: false\n\
        \x20 socket \"/live\", Phoenix.LiveView.Socket, websocket: [connect_info: [session: @s]]\n\
        end\n";

    const USER_SOCKET_EX: &str = "defmodule AppWeb.UserSocket do\n\
        \x20 use Phoenix.Socket\n\
        \n\
        \x20 channel \"room:*\", AppWeb.RoomChannel\n\
        \x20 channel \"user:*\", AppWeb.UserChannel\n\
        \n\
        \x20 def connect(_params, socket, _connect_info), do: {:ok, socket}\n\
        end\n";

    #[test]
    fn phoenix_endpoint_socket_mount() {
        let out = handlers(ENDPOINT_EX);
        assert_eq!(
            anchored(&out),
            vec![("ws:/socket".to_string(), 3), ("ws:/live".to_string(), 4)]
        );
        // `socket "` outside a Phoenix endpoint is not a mount.
        let src = "defmodule Net do\n  def send(socket \"x\"), do: :ok\nend\n";
        assert!(handlers(src).nodes.is_empty());
    }

    #[test]
    fn phoenix_socket_channel_topic() {
        let out = handlers(USER_SOCKET_EX);
        assert_eq!(
            anchored(&out),
            vec![("ws:room:*".to_string(), 3), ("ws:user:*".to_string(), 4)]
        );
        // An interpolated topic is not a literal, and never falls back.
        let src = "use Phoenix.Socket\nchannel \"room:#{@prefix}\", R\n";
        assert!(handlers(src).nodes.is_empty());
        // A single-quoted charlist is not a topic.
        let src = "use Phoenix.Socket\nchannel 'room:*', R\n";
        assert!(handlers(src).nodes.is_empty());
        // `channel "` without the Phoenix.Socket import.
        assert!(handlers("channel \"room:*\", R\n").nodes.is_empty());
    }

    #[test]
    fn phoenix_use_line_mints_nothing() {
        let src = "defmodule AppWeb.RoomChannel do\n\
                   \x20 use Phoenix.Channel\n\
                   \x20 def join(\"room:lobby\", _message, socket) do\n\
                   \x20   {:ok, socket}\n\
                   \x20 end\n\
                   end\n";
        let out = handlers(src);
        assert!(out.nodes.is_empty(), "{:?}", qnames(&out));
        assert!(clients(src).nodes.is_empty());
    }

    #[test]
    fn phoenix_js_socket_and_channel() {
        let src = "import { Socket } from \"phoenix\"\n\
                   \n\
                   let socket = new Socket(\"/socket\", { params: {} })\n\
                   socket.connect()\n\
                   \n\
                   let channel = socket.channel(\"room:lobby\", {})\n\
                   channel.join()\n";
        let out = clients(src);
        assert_eq!(
            anchored(&out),
            vec![
                ("ws_client:/socket".to_string(), 2),
                ("ws_client:room:lobby".to_string(), 5)
            ]
        );
        // A topic held in a variable is unreadable and mints nothing.
        let src = "import { Socket } from 'phoenix'\nconst c = socket.channel(topic, {})\n";
        assert!(clients(src).nodes.is_empty());
        // Without the phoenix import, `.channel(` is someone else's API
        // (amqp's `connection.channel()`).
        let src = "import amqp from \"amqplib\"\nconst ch = conn.channel(\"orders\")\n";
        assert!(clients(src).nodes.is_empty());
        // `phoenix_html` is not phoenix.js.
        let src = "import \"phoenix_html\"\nlet s = new Socket(\"/socket\")\n";
        assert!(clients(src).nodes.is_empty());
    }

    #[test]
    fn multibyte_and_unterminated_input_never_panics() {
        let src = "new WebSocket(\"ws://h/é\" + `${\"ü\"}/ß`";
        let _ = clients(src);
        let long = format!("new WebSocket(\"{}", "é".repeat(MAX_REGION));
        let _ = clients(&long);
        let _ = handlers("using SignalR;\napp.MapHub<");
        let _ = handlers("import fastapi\n@app.websocket(");
        assert_eq!(client_path("\"ws://h/ß\""), Some("/ß".to_string()));
    }

    /// Every row, gate satisfied, with 4-byte chars before the needle, inside
    /// its argument and after it: no read may slice inside a char.
    #[test]
    fn every_row_survives_multibyte_neighbours() {
        let gates: String = HANDLER_ROWS
            .iter()
            .chain(CLIENT_ROWS)
            .flat_map(|r| r.gate.iter())
            .map(|g| format!("// {g}\n"))
            .collect();
        for r in HANDLER_ROWS.iter().chain(CLIENT_ROWS) {
            for pad in 0..4 {
                let wide = "\u{1F600}".repeat(pad);
                for tail in [
                    format!("\"{wide}/p{wide}\")"),
                    format!("`${{{wide}}}/q{wide}` + {wide})"),
                    format!("{wide}<{wide}>(\"/x\", \"{wide}\")"),
                    wide.clone(),
                ] {
                    let src = format!("{gates}{wide}{}{tail}\ndef f(): {wide}\n", r.needle);
                    let _ = handlers(&src);
                    let _ = clients(&src);
                }
            }
        }
    }

    /// The ENDPOINT_HIT payloads on each node, in node order.
    fn hits(out: &WsNodes) -> Vec<(String, Vec<String>)> {
        out.nodes
            .iter()
            .map(|n| {
                let payloads = n
                    .cells
                    .iter()
                    .filter(|c| c.kind == glia_code_domain::cell_type::ENDPOINT_HIT)
                    .map(|c| match &c.payload {
                        glia_core::CellPayload::Json(j) => j.clone(),
                        other => format!("{other:?}"),
                    })
                    .collect();
                (out.nav.qname_by_id[&n.id].clone(), payloads)
            })
            .collect()
    }

    /// CB.21: a literal ws / http URL names its authority, userinfo dropped and
    /// the port kept, one cell per site; a second site on the same path stacks
    /// its own cell.
    #[test]
    fn ws_client_host_from_a_literal_url() {
        let src = "export function connectChat(): WebSocket {\n\
                   \x20 const ws = new WebSocket(\"ws://chat-svc:8080/ws\");\n\
                   \x20 return ws;\n}\n\
                   export function again() {\n\
                   \x20 return new WebSocket(\"wss://u:p@chat-svc/ws?token=\" + token);\n}\n";
        let out = clients(src);
        assert_eq!(
            hits(&out),
            vec![(
                "ws_client:/ws".to_string(),
                vec![
                    r#"{"via":"ws","host":"chat-svc:8080"}"#.to_string(),
                    r#"{"via":"ws","host":"chat-svc"}"#.to_string(),
                ]
            )]
        );
        assert_eq!(client_host("\"https://api.example.com/hubs/chat\""), Some("api.example.com".into()));
        assert_eq!(client_host("`ws://notify:9000/live`"), Some("notify:9000".into()));
        let go = "import \"github.com/gorilla/websocket\"\n\
                  c, _, _ := websocket.DefaultDialer.DialContext(ctx, \"ws://chat-svc/ws\", nil)\n";
        assert_eq!(
            hits(&clients(go)),
            vec![("ws_client:/ws".to_string(), vec![r#"{"via":"ws","host":"chat-svc"}"#.to_string()])]
        );
    }

    /// CB.21: a dynamic authority, a relative path, a variable URL and a
    /// non-URL row all record a hostless cell, never a guessed host.
    #[test]
    fn ws_client_host_dynamic_is_absent() {
        for arg in [
            "\"ws://\" + location.host + \"/ws\"",
            "`${proto}://${location.host}/live`",
            "`ws://${host}/ws`",
            "\"ws://chat-svc:\" + port + \"/ws\"",
            "\"ws://\" + host + \":8080\"",
            "\"/ws\"",
            "url",
            "\"ws://${…}/ws\"",
            "\"\"",
        ] {
            assert_eq!(client_host(arg), None, "{arg}");
        }
        let src = "const a = new WebSocket(\"ws://\" + location.host + \"/ws\");\n\
                   const b = new WebSocket(url);\n\
                   App.cable.subscriptions.create({ channel: \"ChatChannel\" });\n";
        assert_eq!(
            hits(&clients(src)),
            vec![
                ("ws_client:/ws".to_string(), vec![r#"{"via":"ws"}"#.to_string()]),
                ("ws_client:ws".to_string(), vec![r#"{"via":"ws"}"#.to_string()]),
                ("ws_client:ChatChannel".to_string(), vec![r#"{"via":"ws"}"#.to_string()]),
            ]
        );
        // A handler never carries one.
        assert!(handlers(FASTAPI).nodes.iter().all(|n| n.cells.is_empty()));
    }

    /// CJ.1b: a needle inside a Rust string literal or `///` comment mints
    /// nothing in a `.rs` file; the same text read as TypeScript (no guard)
    /// mints as before, and a Python needle in code stays.
    #[test]
    fn literal_and_comment_sites_mint_nothing_in_rust() {
        let rust = "/// The NestJS `@WebSocketGateway` handler, as the scanner reads it.\n\
                    fn sample() {\n\
                    \x20   let out = clients(\"const ws = new WebSocket('ws://localhost:8080/chat');\");\n\
                    \x20   let _ = out;\n\
                    }\n";
        let rs_handlers = extract_ws_handler_nodes(rust, "src/x.rs", module_id(), repo());
        let rs_clients = extract_ws_client_nodes(rust, "src/x.rs", module_id(), repo());
        assert!(qnames(&rs_handlers).is_empty(), "{:?}", qnames(&rs_handlers));
        assert!(qnames(&rs_clients).is_empty(), "{:?}", qnames(&rs_clients));
        assert!(rs_clients.anchors.is_empty() && rs_handlers.anchors.is_empty());

        let ts_handlers = extract_ws_handler_nodes(rust, "src/x.ts", module_id(), repo());
        let ts_clients = extract_ws_client_nodes(rust, "src/x.ts", module_id(), repo());
        assert_eq!(qnames(&ts_handlers), vec!["ws:default"]);
        assert_eq!(qnames(&ts_clients), vec!["ws_client:/chat"]);

        let py = "import websockets\n\
                  \n\
                  # websockets.connect(\"ws://h:8765/commented\")\n\
                  DOC = \"websockets.connect('ws://h:8765/quoted')\"\n\
                  async def follow():\n\
                  \x20   async with websockets.connect(\"ws://h:8765/live\") as ws:\n\
                  \x20       return await ws.recv()\n";
        let out = extract_ws_client_nodes(py, "client/a.py", module_id(), repo());
        assert_eq!(qnames(&out), vec!["ws_client:/live"]);
        assert_eq!(anchored(&out), vec![("ws_client:/live".to_string(), 5)]);
    }

    /// CJ.1b: refused literal occurrences are dropped before the per-needle
    /// cap, so a file whose literals repeat a needle keeps its real site.
    #[test]
    fn literal_sites_do_not_fill_the_cap() {
        let mut src = "const SAMPLE: &str = \"new WebSocket('/fake')\";\n"
            .repeat(MAX_HITS_PER_NEEDLE + 5);
        src.push_str("fn connect(url: &str) { let _ = new WebSocket(url); }\n");
        let out = extract_ws_client_nodes(&src, "src/table.rs", module_id(), repo());
        assert_eq!(qnames(&out), vec!["ws_client:ws"]);
        assert_eq!(out.anchors.len(), 1);
    }

    // ---- DG.5: axum and Akka / Pekko HTTP ---------------------------------

    /// bench/substrate-gap/matrix/rust/ws/server/src/main.rs.
    const AXUM_MAIN_RS: &str = "use axum::{extract::ws::{WebSocket, WebSocketUpgrade}, response::Response, routing::get, Router};\n\
        \n\
        async fn chat(ws: WebSocketUpgrade) -> Response {\n\
        \x20   ws.on_upgrade(handle)\n\
        }\n\
        \n\
        async fn handle(_socket: WebSocket) {}\n\
        \n\
        pub fn app() -> Router {\n\
        \x20   Router::new().route(\"/ws/chat\", get(chat))\n\
        }\n";

    /// An axum file whose handler `chat` is mounted by `router`.
    fn axum_with(router: &str) -> String {
        format!(
            "use axum::extract::ws::WebSocketUpgrade;\n\
             async fn chat(ws: WebSocketUpgrade) -> impl IntoResponse {{\n\
             \x20   ws.on_upgrade(|socket| async move {{ handle(socket).await }})\n\
             }}\n\
             pub fn app() -> Router {{\n\
             \x20   {router}\n\
             }}\n"
        )
    }

    #[test]
    fn axum_on_upgrade_reads_its_route() {
        let out = handlers(AXUM_MAIN_RS);
        assert_eq!(qnames(&out), vec!["ws:/ws/chat"]);
        assert_eq!(
            anchored(&out),
            vec![("ws:/ws/chat".to_string(), 3)],
            "the on_upgrade line inside `chat`, so anchor::attach gives chat its HANDLED_BY"
        );
        for (router, want) in [
            ("Router::new().route(\"/live\", any(chat))", "ws:/live"),
            (
                "Router::new().route(\"/live\", routing::get(chat))",
                "ws:/live",
            ),
            (
                "Router::new().route(\"/live\", axum::routing::any(chat))",
                "ws:/live",
            ),
            (
                "Router::new()\n        .route(\"/\", get(index))\n        .route(\"/live\", get(index).post(chat))",
                "ws:/live",
            ),
            (
                "Router::new().route(\"/live\", get(chat).layer(cors))",
                "ws:/live",
            ),
            (
                "Router::new().route(\"/live\", on(MethodFilter::GET, chat))",
                "ws:/live",
            ),
            (
                "Router::new().route(\"/ws/{room}\", get(chat)).route(\"/second\", get(chat))",
                "ws:/ws/{room}",
            ),
            // A route naming another fn, a non-router call and a substring of
            // the name never mount `chat`.
            ("Router::new().route(\"/other\", get(other))", "ws:ws"),
            ("Router::new().route(\"/svc\", get_service(chat))", "ws:ws"),
            ("Router::new().route(\"/x\", get(chatty))", "ws:ws"),
        ] {
            assert_eq!(
                qnames(&handlers(&axum_with(router))),
                vec![want],
                "{router}"
            );
        }
    }

    #[test]
    fn axum_two_handlers_and_a_nested_fn() {
        let src = "use axum::extract::ws::WebSocketUpgrade;\n\
                   async fn chat(ws: WebSocketUpgrade) -> Response {\n\
                   \x20   fn helper(p: [u8; 4], Point { x, y }: Point) -> u8 { x }\n\
                   \x20   ws.on_upgrade(handle)\n\
                   }\n\
                   async fn feed(ws: WebSocketUpgrade) -> Response {\n\
                   \x20   ws.on_upgrade(stream)\n\
                   }\n\
                   trait Hub { fn chat(&self); }\n\
                   pub fn app() -> Router {\n\
                   \x20   Router::new().route(\"/ws/chat\", get(chat)).route(\"/ws/feed\", get(feed))\n\
                   }\n";
        assert_eq!(
            anchored(&handlers(src)),
            vec![
                ("ws:/ws/chat".to_string(), 3),
                ("ws:/ws/feed".to_string(), 6)
            ],
            "the nested `helper` ended before the site, so `chat` owns it"
        );
    }

    #[test]
    fn axum_handler_routed_elsewhere_is_generic() {
        let src = "use axum::extract::ws::WebSocketUpgrade;\n\
                   pub async fn chat(ws: WebSocketUpgrade) -> Response {\n\
                   \x20   ws.on_upgrade(handle)\n\
                   }\n";
        assert_eq!(
            anchored(&handlers(src)),
            vec![("ws:ws".to_string(), 2)],
            "the router lives in another file: the resolver lends `ws` the ROUTEs HANDLED_BY chat"
        );
        // No enclosing fn at all reads nothing either.
        assert_eq!(
            qnames(&handlers("use axum;\nstatic X: u8 = ws.on_upgrade(h);\n")),
            vec!["ws:ws"]
        );
    }

    #[test]
    fn axum_on_upgrade_without_axum_import_is_no_site() {
        // warp's `Ws::on_upgrade` is not axum's.
        let src = "use warp::Filter;\n\
                   fn routes() -> impl Filter {\n\
                   \x20   warp::path(\"echo\").and(warp::ws()).map(|ws: warp::ws::Ws| ws.on_upgrade(echo))\n\
                   }\n";
        let out = handlers(src);
        assert!(out.nodes.is_empty(), "{:?}", qnames(&out));
        assert!(out.anchors.is_empty());
    }

    #[test]
    fn axum_needle_in_a_rust_string_is_no_site() {
        let rs = |src: &str| extract_ws_handler_nodes(src, "src/main.rs", module_id(), repo());
        let src = "use axum::Router;\n\
                   /// Calls `ws.on_upgrade(handle)` in a doc line.\n\
                   const SAMPLE: &str = \"async fn chat(ws: WebSocketUpgrade) { ws.on_upgrade(h) }\";\n";
        let out = rs(src);
        assert!(out.nodes.is_empty(), "{:?}", qnames(&out));
        // In code, a `.route(` and a `fn` in a literal or comment mount nothing.
        let src = "use axum::Router;\n\
                   // Router::new().route(\"/commented\", get(chat))\n\
                   const S: &str = \"Router::new().route(\\\"/quoted\\\", get(chat))\";\n\
                   async fn chat(ws: WebSocketUpgrade) -> Response {\n\
                   \x20   let _ = \"fn decoy() {\";\n\
                   \x20   ws.on_upgrade(handle)\n\
                   }\n\
                   pub fn app() -> Router { Router::new().route(\"/real\", get(chat)) }\n";
        assert_eq!(anchored(&rs(src)), vec![("ws:/real".to_string(), 5)]);
    }

    /// bench/substrate-gap/matrix/scala/ws/server/ChatRoutes.scala.
    const AKKA_CHAT_ROUTES: &str = "package shop\n\
        \n\
        import akka.http.scaladsl.model.ws.Message\n\
        import akka.http.scaladsl.server.Directives._\n\
        import akka.stream.scaladsl.Flow\n\
        \n\
        object ChatRoutes {\n\
        \x20 val echo = Flow[Message]\n\
        \x20 val route = path(\"ws\" / \"chat\") {\n\
        \x20   handleWebSocketMessages(echo)\n\
        \x20 }\n\
        }\n";

    /// An Akka HTTP file whose route is `body`.
    fn akka_with(body: &str) -> String {
        format!(
            "import akka.http.scaladsl.server.Directives._\n\
             object Routes {{\n\
             \x20 val route = {body}\n\
             }}\n"
        )
    }

    #[test]
    fn akka_handle_websocket_reads_the_composed_path() {
        let out = handlers(AKKA_CHAT_ROUTES);
        assert_eq!(anchored(&out), vec![("ws:/ws/chat".to_string(), 9)]);
        for (body, want) in [
            (
                "pathPrefix(\"api\") {\n    path(\"ws\" / Segment) { room =>\n      handleWebSocketMessages(chat(room))\n    }\n  }",
                "ws:/api/ws/:segment",
            ),
            (
                "pathPrefix(\"api\" / \"v1\") { pathPrefix(\"rooms\") { path(IntNumber) { id => handleWebSocketMessages(f(id)) } } }",
                "ws:/api/v1/rooms/:id",
            ),
            (
                "pathPrefix(\"files\") { path(Remaining) { r => handleWebSocketMessages(f) } }",
                "ws:/files/*",
            ),
            (
                "path(\"ws/chat\") { handleWebSocketMessages(f) }",
                "ws:/ws/chat",
            ),
            (
                "path(\"ws\") { handleWebSocketMessagesForProtocol(f, \"chat\") }",
                "ws:/ws",
            ),
            // A sibling route's block does not hold the site.
            (
                "concat(\n    path(\"health\") { complete(\"ok\") },\n    path(\"ws\") { handleWebSocketMessages(f) }\n  )",
                "ws:/ws",
            ),
            (
                "(path(\"ws\") & get) { handleWebSocketMessages(f) }",
                "ws:/ws",
            ),
            (
                "pathPrefix(\"api\") { (get & path(\"live\")) { handleWebSocketMessages(f) } }",
                "ws:/api/live",
            ),
            ("path(\"ws\")(handleWebSocketMessages(f))", "ws:/ws"),
            // Unreadable: a val matcher, `separateOnSlashes`, an alternative,
            // or a directive group the site is not in.
            ("path(wsMatcher) { handleWebSocketMessages(f) }", "ws:ws"),
            (
                "pathPrefix(\"api\") { path(wsMatcher) { handleWebSocketMessages(f) } }",
                "ws:ws",
            ),
            (
                "path(separateOnSlashes(\"ws/chat\")) { handleWebSocketMessages(f) }",
                "ws:ws",
            ),
            (
                "(path(\"a\") | path(\"b\")) { handleWebSocketMessages(f) }",
                "ws:ws",
            ),
            ("handleWebSocketMessages(f)", "ws:ws"),
        ] {
            assert_eq!(qnames(&handlers(&akka_with(body))), vec![want], "{body}");
        }
        // Pekko's package gates the same rows.
        let pekko = AKKA_CHAT_ROUTES.replace("import akka.http", "import org.apache.pekko.http");
        assert_eq!(qnames(&handlers(&pekko)), vec!["ws:/ws/chat"]);
    }

    /// The matcher table DG.10 gives the Scala parser's Akka ROUTEs, pinned on
    /// the same inputs.
    #[test]
    fn akka_matcher_paths() {
        for (args, want) in [
            ("\"ws\" / \"chat\"", Some("/ws/chat")),
            ("\"users\" / IntNumber", Some("/users/:id")),
            (
                "\"users\" / Segment / \"posts\" / LongNumber",
                Some("/users/:segment/posts/:id"),
            ),
            ("JavaUUID", Some("/:id")),
            ("Remaining", Some("/*")),
            ("\"a/b\"", Some("/a/b")),
            ("wsMatcher", None),
            ("separateOnSlashes(\"a/b\")", None),
            ("\"a\" / Slash", None),
            ("\"\"", None),
            ("s\"x$y\"", None),
            ("\"a\", \"b\"", None),
        ] {
            assert_eq!(akka_matcher_path(args).as_deref(), want, "{args}");
        }
    }

    #[test]
    fn akka_needs_the_http_import() {
        let src = AKKA_CHAT_ROUTES
            .replace("import akka.http.scaladsl.model.ws.Message\n", "")
            .replace("import akka.http.scaladsl.server.Directives._\n", "");
        let out = handlers(&src);
        assert!(out.nodes.is_empty(), "{:?}", qnames(&out));
    }

    #[test]
    fn subpath_is_not_a_path_directive() {
        for body in [
            "subpath(\"x\") { handleWebSocketMessages(f) }",
            "filepath(\"x\") { handleWebSocketMessages(f) }",
            "{ val s = \"path(\\\"x\\\") {\"; handleWebSocketMessages(f) }",
        ] {
            assert_eq!(qnames(&handlers(&akka_with(body))), vec!["ws:ws"], "{body}");
        }
        // An enclosing `subpath(` does not hide the real directive inside it.
        let body = "subpath(\"x\") { path(\"ws\") { handleWebSocketMessages(f) } }";
        assert_eq!(qnames(&handlers(&akka_with(body))), vec!["ws:/ws"]);
    }

    #[test]
    fn normalise_strips_every_scheme() {
        assert_eq!(normalise_ws_path("ws://h:1/a"), "/a");
        assert_eq!(normalise_ws_path("wss://h/a/b"), "/a/b");
        assert_eq!(normalise_ws_path("http://h/hubs/chat"), "/hubs/chat");
        assert_eq!(normalise_ws_path("https://h"), "");
        assert_eq!(normalise_ws_path("/rel"), "/rel");
    }
}
