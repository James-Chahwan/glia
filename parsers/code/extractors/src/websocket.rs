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
//! client argument goes through [`path_of_text`], which reads the static tail
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
//! DH.9: Ratchet's `$app->route('/chat', new MyChat, ..)` is a PHP server
//! (gated on `Ratchet\App`, a path literal or nothing). Dart's
//! web_socket_channel (`WebSocketChannel` / `IOWebSocketChannel` /
//! `HtmlWebSocketChannel.connect(..)`) and dart:io's `WebSocket.connect(..)`
//! are clients whose URL may sit in a `Uri.parse(..)` / `Uri.tryParse(..)`
//! ([`PathRead::ClientUri`]); a Dart literal's `$name` / `${expr}` is a
//! dynamic piece of it ([`Interp::Dart`]).
//!
//! DT.3a: a client URL is read through the code that builds it
//! ([`client_url`], depth one): a bare identifier through the `const` /
//! `final` / Go `:=` binding in a scope enclosing the call ([`local_binding`],
//! an indentation walk; a `let` / `var` / reassignment or a parameter of that
//! name reads nothing), `this.<m>` through the file's one `readonly` field
//! ([`class_field_init`]), Go's `u.String()` through `u := url.URL{.. Path: ..}`
//! ([`go_url_struct_path`]), and a URL builder call with one literal
//! (`this.urls.buildWsUrl('api/chat/ws')` -> `/api/chat/ws`,
//! [`builder_literal`]). A node every one of whose sites read it through a
//! builder is Weak; the resolver's WS_CONNECTS takes the weakest side.
//!
//! DT.3b: a client URL whose base sits in the repo's configuration folds
//! through the const table, as the A11.2 HTTP endpoint fold does. The lookup
//! reads other files, so it cannot run here (the `constants.rs` CACHE RULE):
//! the engine re-runs the client scan post-cache through
//! [`extract_ws_client_nodes_with_consts`] with the repo table as a resolver,
//! and swaps a file's WS_CLIENT nodes ([`replace_ws_client_nodes`]) when a
//! site folded. The fold changes the one per-site reading ([`ClientUrl::path`]
//! / [`ClientUrl::host`], through [`client_text`]): a member chain in a
//! template's `${..}`, a Dart `$name` / `${..}`, a `+` piece or the whole
//! argument is replaced by the literal the resolver binds it to, so
//! `new WebSocket(environment.feedUrl)` reads `ws://feed-svc:8080/rt/feed`
//! (`/rt/feed`, host `feed-svc:8080`) and `` `${environment.wsBase}/prices` ``
//! with `wsBase = 'ws://feed-svc:8080/rt'` reads `/rt/prices`, never the tail
//! alone. A bare lower-case identifier or `this.<m>` that is the whole
//! argument is DT.3a's scoped read ([`client_url`]), not the file-wide
//! table's ([`ClientUrl::foldable`]); a folded text that no longer reads a
//! path where the plain one did is refused ([`ClientUrl::url_text`]). With no
//! resolver the scan is byte for byte the per-file one.
//!
//! Anchors: the name is read per site, so EVERY site is anchored and
//! `anchor::attach` gives each owning function its HANDLED_BY (server) or USES
//! (client) edge. Python decorators anchor at the decorated `def` line
//! ([`AnchorAt::NextDef`]), because a Python FUNCTION span starts at its
//! `def`, not at the decorator above it.
//!
//! Hosts (CB.21): every CLIENT site stacks one ENDPOINT_HIT on its node,
//! `{"via":"ws","host":"chat-svc:8080"}` when the URL's static head names a
//! literal authority ([`host_of_text`]), `{"via":"ws"}` otherwise. The graph
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
//!   `[ws] handlers framework=ratchet n=1 in server/server.php`
//!   `[ws] clients framework=web_socket_channel n=1 in lib/chat.dart`
//!   `[ws] clients framework=dart_io n=1 in bin/feed.dart`
//! and one per file whose client sites named a host (CB.21):
//!   `[ws] client hosts sites=1 in web/chat.ts`
//! and one per file with an identifier / member / builder URL argument
//! (DT.3a; `builder` and `url_struct` count the subset of reads whose text
//! came from a builder call / a Go `url.URL`, `unread` the arguments still
//! unreadable):
//!   `[ws] client urls read=1 (local=1 field=0 builder=0 url_struct=0) unread=0 in web/src/app/core/live/order-feed.service.ts`
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

use std::collections::{HashMap, HashSet};

use glia_code_domain::endpoint::{channel_hit_cell, client_url_split};
use glia_code_domain::{CodeNav, FileParse, GRAPH_TYPE, node_kind};
use glia_core::{Confidence, Node, NodeId, NodeKindId, RepoId};

use crate::anchor::{Anchor, line_of};
use crate::cli::CodeMap;
use crate::code_guard::LazyGuard;
use crate::marker_swap;
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
    /// [`path_of_text`] on positional argument `n`.
    Client(usize),
    /// DH.9, Dart: [`path_of_text`] on positional argument `n` once
    /// [`client_uri_arg`] stepped into a `Uri.parse(..)` / `Uri.tryParse(..)`
    /// wrapper, every quoted literal read with Dart interpolation
    /// ([`Interp::Dart`]).
    ClientUri(usize),
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
/// Ratchet's routing facade, by its class (`use Ratchet\App;` or a qualified
/// `new \Ratchet\App(..)`).
const RATCHET: &[&str] = &["Ratchet\\App"];
/// Dart's web_socket_channel package, by its import (`/web_socket_channel.dart`,
/// `/io.dart`, `/html.dart`).
const WEB_SOCKET_CHANNEL: &[&str] = &["package:web_socket_channel/"];

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
    // PHP: Ratchet's `$app->route('/chat', new MyChat, array('*'))` (DH.9).
    // ReadOnly: only a path literal mints. The handler object is an instance
    // (`new MyChat`), not a symbol a ref could bind, so no HANDLED_BY to it is
    // read. KNOWN IMPRECISION: a Slim / Laravel `->route('/p', ..)` sharing a
    // file with Ratchet reads as a socket route.
    keyed_row(
        "->route(",
        RATCHET,
        PathRead::Arg(0),
        Accept::ReadOnly,
        "ratchet",
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
    // Dart (DH.9): web_socket_channel's three channel classes and dart:io's
    // own socket. Needles are word-bounded ([`sites`]), so each channel class
    // is its own row; `connect` takes a `Uri` (or, for `IOWebSocketChannel`,
    // a `String`), dart:io's `WebSocket.connect` a `String`.
    row(
        "WebSocketChannel.connect(",
        WEB_SOCKET_CHANNEL,
        PathRead::ClientUri(0),
        "web_socket_channel",
    ),
    row(
        "IOWebSocketChannel.connect(",
        WEB_SOCKET_CHANNEL,
        PathRead::ClientUri(0),
        "web_socket_channel",
    ),
    row(
        "HtmlWebSocketChannel.connect(",
        WEB_SOCKET_CHANNEL,
        PathRead::ClientUri(0),
        "web_socket_channel",
    ),
    row(
        "WebSocket.connect(",
        &["dart:io"],
        PathRead::ClientUri(0),
        "dart_io",
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
        None,
        &mut WsFoldCounts::default(),
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
        CLIENT_SIDE,
        None,
        &mut WsFoldCounts::default(),
    )
}

/// The client half of the scan, shared by the per-file pass and the DT.3b
/// fold so the two cannot drift.
const CLIENT_SIDE: Side = Side {
    rows: CLIENT_ROWS,
    kind: node_kind::WS_CLIENT,
    prefix: "ws_client:",
    label: "clients",
    hits: true,
};

/// DT.3b: the resolver the engine's post-cache fold hands the client scan
/// (the LA.4 resolver over the repo const table): a source expression in,
/// the literal it is bound to out.
type Resolve<'r> = &'r dyn Fn(&str) -> Option<String>;

/// DT.3b: what the post-cache base fold read in one file.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct WsFoldCounts {
    /// Client sites whose URL text the resolver changed (a member chain
    /// replaced by its literal) and whose folded reading stood.
    pub folded: usize,
    /// Client sites that offered the resolver a member chain and kept their
    /// plain reading: nothing bound it (a parameter, a runtime value, an
    /// ambiguous or env-read key), or the literal it bound read no path where
    /// the plain text did. A site that folded is never counted here, even
    /// when another of its chains (a path parameter) stayed unread.
    pub unresolved: usize,
}

/// DT.3b: one file's WS_CLIENT nodes re-emitted through a resolver.
pub struct WsClientFold {
    pub nodes: WsNodes,
    pub counts: WsFoldCounts,
    /// The file the nodes were read from, which [`replace_ws_client_nodes`]
    /// hands [`crate::anchor::attach`] when it re-anchors them.
    pub path: String,
}

/// DT.3b: the file's WS_CLIENT nodes, emitted exactly as
/// [`extract_ws_client_nodes`] emits them except that every member chain in a
/// client URL that `resolve` binds to a literal holding no `${` is read as
/// that literal (module doc). So `new WebSocket(environment.feedUrl)` mints
/// `ws_client:/rt/feed` with an ENDPOINT_HIT naming `feed-svc:8080`, and
/// `` new WebSocket(`${environment.wsBase}/prices`) `` mints
/// `ws_client:/rt/prices`, not the tail-only `ws_client:/prices`.
///
/// Resolution needs the whole repo's const table, which is not a function of
/// this file, so the engine calls this AFTER the parse cache (never inside the
/// per-file extractors) and swaps the file's client nodes with
/// [`replace_ws_client_nodes`] only when `counts.folded > 0`. The fold prints
/// none of the per-file `[ws]` / `[code-guard]` lines: each file reports once
/// per parse; the engine's `[ws-base]` line reports the fold.
pub fn extract_ws_client_nodes_with_consts(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
    resolve: &dyn Fn(&str) -> Option<String>,
) -> WsClientFold {
    let mut counts = WsFoldCounts::default();
    let nodes = scan(
        source,
        path,
        module_id,
        repo,
        CLIENT_SIDE,
        Some(resolve),
        &mut counts,
    );
    WsClientFold {
        nodes,
        counts,
        path: path.to_string(),
    }
}

/// DT.3b: swap every WS_CLIENT node of one file's parse for `fold`'s, through
/// the swap the LA.4 queue and CB.3b event folds share
/// ([`marker_swap::swap`]): the old nodes, their owner edges (`function
/// -USES-> ws_client:..`, the anchor pass's), nav entries and every edge /
/// ref naming a gone id (a client whose path moved) go; the fold's nodes go
/// back in place, each site's ENDPOINT_HIT as the scan wrote it, re-anchored
/// (POSITION, then the owner edge or the module CONTAINS fallback, stamped
/// `extractor:anchor` rule `const_fold`), and the IMPORTS cell re-attached
/// when the old nodes carried it. Client nodes carry no edges of their own (a
/// WS marker reaches its module through `anchor::attach` only, the events
/// precedent) and no callbacks, so there is nothing to re-bind. Only the WS
/// client scan mints WS_CLIENT, so every one of them is the scan's.
pub fn replace_ws_client_nodes(
    fp: &mut FileParse,
    module_id: NodeId,
    lang: &str,
    fold: WsClientFold,
) {
    let old: HashSet<NodeId> = fp
        .nav
        .kind_by_id
        .iter()
        .filter(|(_, k)| **k == node_kind::WS_CLIENT)
        .map(|(id, _)| *id)
        .collect();
    let WsClientFold { nodes, path, .. } = fold;
    let fresh = marker_swap::MarkerNodes {
        nodes: nodes.nodes,
        edges: Vec::new(),
        navs: vec![nodes.nav],
        anchors: nodes.anchors,
    };
    marker_swap::swap(fp, module_id, lang, &old, fresh, &path, |_| {});
}

struct Side {
    rows: &'static [WsRow],
    kind: NodeKindId,
    prefix: &'static str,
    label: &'static str,
    /// CB.21: every site stacks one ENDPOINT_HIT on its node, `host` when the
    /// site's URL names a literal authority ([`ClientUrl::host`]). Clients only.
    hits: bool,
}

/// Rows in table order, sites in byte order: node order and anchor order are
/// a function of the file's text alone. The name map is only ever looked up,
/// never iterated.
///
/// DT.3b: `resolve` is `None` on the per-file (cached) path, which is then
/// unchanged; the engine's post-cache fold passes one (clients only), every
/// client URL reads through it ([`ClientUrl::url_text`]), `counts` tallies the
/// sites it folded or left unread, and the per-file marker lines stay quiet.
fn scan(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
    side: Side,
    resolve: Option<Resolve<'_>>,
    counts: &mut WsFoldCounts,
) -> WsNodes {
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
    // DT.3a: client URLs read through a binding or a builder, for the marker.
    let mut reads = UrlReads::default();
    // DT.3a, per node (index-aligned with `nodes`): some site read it through
    // a URL builder / some site read it any other way.
    let mut builder_site: Vec<bool> = Vec::new();
    let mut plain_site: Vec<bool> = Vec::new();

    for r in side.rows {
        if !gate_holds(source, r.gate) {
            continue;
        }
        for offset in sites(source, r.needle, &mut guard) {
            let url = client_url(source, offset, r, resolve);
            let read = read_name(source, offset, r, url.as_ref(), &mut blocks);
            if let Some(u) = &url {
                reads.count(u, read.is_some());
                if resolve.is_some() {
                    match u.fold_read() {
                        FoldRead::Folded => counts.folded += 1,
                        FoldRead::Unread => counts.unresolved += 1,
                        FoldRead::Plain => {}
                    }
                }
            }
            let via_builder = read.is_some() && url.as_ref().is_some_and(|u| u.builder);
            let Some(name) = read.or_else(|| fallback(r).map(str::to_string)) else {
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
                    builder_site.push(false);
                    plain_site.push(false);
                    nav.record(id, &name, &qname, side.kind, Some(module_id));
                    by_name.insert(name, nodes.len() - 1);
                    nodes.len() - 1
                }
            };
            if let Some(seen) = if via_builder {
                builder_site.get_mut(at)
            } else {
                plain_site.get_mut(at)
            } {
                *seen = true;
            }
            let Some(node) = nodes.get_mut(at) else {
                continue;
            };
            let id = node.id;
            if side.hits {
                let host = url.as_ref().and_then(ClientUrl::host);
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

    // DT.3a: a node every one of whose sites read it through a URL builder is
    // Weak (CH.3a arm a's tier: the builder may transform the path).
    for (node, (builder, plain)) in nodes.iter_mut().zip(builder_site.iter().zip(&plain_site)) {
        if *builder && !*plain {
            node.confidence = Confidence::Weak;
        }
    }
    // DT.3b: the fold stays quiet: each file reports once per parse.
    if resolve.is_none() {
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
        if reads.read + reads.unread > 0 {
            eprintln!(
                "[ws] client urls read={} (local={} field={} builder={} url_struct={}) unread={} in {path}",
                reads.read, reads.local, reads.field, reads.builder, reads.url_struct, reads.unread
            );
        }
        guard.report(if side.kind == node_kind::WS_HANDLER {
            "ws_handler"
        } else {
            "ws_client"
        });
    }

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

/// The name one site reads, or `None` when the row reads none there. `url` is
/// the site's [`ClientUrl`] (client URL rows only, [`client_url`]); `blocks` is
/// the scan's [`Blocks`], built here on the first site that needs it.
fn read_name(
    source: &str,
    offset: usize,
    r: &WsRow,
    url: Option<&ClientUrl<'_>>,
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
        PathRead::Client(_) | PathRead::ClientUri(_) => url?.path()?,
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

/// The request path a client URL names, or `None` when it cannot be read, on
/// the URL already read as text ([`client_text`]: top-level `+` pieces, a
/// string literal static text, a template literal static text with each
/// `${...}` span a dynamic piece, anything else dynamic). With a dynamic
/// piece, the text after the LAST one is the path when it starts with `/`;
/// otherwise a static head that is a full `scheme://host/path` URL is. The
/// scheme + authority are stripped and a query / fragment is dropped; only a
/// result starting with `/` is accepted.
///
/// `"ws://" + location.host + "/ws/chat"` -> `/ws/chat`,
/// `` `${base}/ws/admin` `` -> `/ws/admin`,
/// `"wss://api.example.com/echo"` -> `/echo`, `url` -> `None`.
fn path_of_text(text: String) -> Option<String> {
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

/// How a client URL's quoted literals interpolate ([`client_text`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Interp {
    /// Every row but Dart's: only a backtick template's `${...}` spans are
    /// dynamic; a `'...'` / `"..."` literal is static text.
    Backtick,
    /// Dart (DH.9): `$name` and `${expr}` inside any quoted literal are
    /// dynamic ([`push_dart`]).
    Dart,
}

/// The URL argument a client row reads at the call whose `(` ends just before
/// `after`, and how its literals interpolate; `None` for a row that reads no
/// URL ([`PathRead::Client`] / [`PathRead::ClientUri`] only). The one reader
/// of that argument: [`client_url`] resolves it once per site, so the path
/// ([`read_name`]) and the host ([`ClientUrl::host`]) see the same text.
fn client_url_arg(source: &str, after: usize, read: PathRead) -> Option<(&str, Interp)> {
    let (n, interp) = match read {
        PathRead::Client(n) => (n, Interp::Backtick),
        PathRead::ClientUri(n) => (n, Interp::Dart),
        _ => return None,
    };
    let arg = *split_top(call_region(source, after)?, b',').get(n)?;
    Some(match interp {
        Interp::Dart => (client_uri_arg(arg), interp),
        Interp::Backtick => (arg, interp),
    })
}

/// DH.9: a Dart URL argument with its `Uri.parse(..)` / `Uri.tryParse(..)`
/// wrapper stepped into: the wrapper's first argument when `arg` is exactly
/// that call (a trailing null assertion `!` allowed), else `arg` itself
/// (`IOWebSocketChannel.connect('ws://..')`, dart:io's `String` URL). A
/// wrapper with more after it (`Uri.parse(u).replace(..)`) stays whole, so it
/// reads as one dynamic piece.
fn client_uri_arg(arg: &str) -> &str {
    let arg = arg.trim();
    ["Uri.parse(", "Uri.tryParse("]
        .iter()
        .find_map(|wrapper| {
            let inner = arg.strip_prefix(wrapper)?;
            let region = call_region(inner, 0)?;
            let tail = inner.get(region.len()..)?.strip_prefix(')')?.trim();
            if tail.is_empty() || tail == "!" {
                split_top(region, b',').first().copied()
            } else {
                None
            }
        })
        .unwrap_or(arg)
}

/// A client URL argument as text: split into top-level `+` pieces, a string
/// literal is static text, a template literal is static text with each
/// `${...}` span a [`DYNAMIC`], anything else is one [`DYNAMIC`]. Under
/// [`Interp::Dart`] a quoted literal's `$name` / `${expr}` are [`DYNAMIC`] too.
/// `None` for an empty argument. The one reader of a client URL's text:
/// [`ClientUrl::path`] ([`path_of_text`]) and [`ClientUrl::host`]
/// ([`host_of_text`]) read one argument identically.
///
/// DT.3b: with a resolver in `fold`, every span or piece that is a member
/// chain ([`Fold::push`]) is the literal it binds, when it binds one.
fn client_text(arg: &str, interp: Interp, fold: &mut Fold<'_>) -> Option<String> {
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
                    push_template(&mut text, lit, fold);
                } else if interp == Interp::Dart {
                    push_dart(&mut text, lit, fold);
                } else {
                    text.push_str(lit);
                }
            }
            _ => fold.push(&mut text, piece),
        }
    }
    Some(text)
}

/// DT.3b: the post-cache fold's member-chain reader, threaded through
/// [`client_text`] (`resolve` is `None` on the per-file pass, and every
/// dynamic piece is one [`DYNAMIC`], as before), and what it did to one URL.
struct Fold<'r> {
    resolve: Option<Resolve<'r>>,
    /// A member chain was replaced by its literal.
    folded: bool,
    /// A member chain was offered to the resolver and stayed [`DYNAMIC`].
    unread: bool,
}

impl<'r> Fold<'r> {
    fn new(resolve: Option<Resolve<'r>>) -> Self {
        Fold {
            resolve,
            folded: false,
            unread: false,
        }
    }

    /// A dynamic piece of a URL (a `${..}` / Dart span's expression, a `+`
    /// piece, the whole argument) into `out`: the literal the resolver binds
    /// it to when it is a member chain ([`is_member_chain`]: `environment.wsBase`,
    /// `this.base`, `WS_URL`) and that literal is one ([`is_folded_literal`]),
    /// else one [`DYNAMIC`]. A call, an operator or an optional chain is
    /// never offered.
    fn push(&mut self, out: &mut String, expr: &str) {
        let expr = expr.trim();
        let Some(resolve) = self.resolve.filter(|_| is_member_chain(expr)) else {
            out.push(DYNAMIC);
            return;
        };
        match resolve(expr).filter(|v| is_folded_literal(v)) {
            Some(value) => {
                out.push_str(&value);
                self.folded = true;
            }
            None => {
                out.push(DYNAMIC);
                self.unread = true;
            }
        }
    }
}

/// DT.3b: a resolved value [`Fold::push`] may splice into a URL: non-empty,
/// no `${` (a value that still interpolates is no literal; the const table
/// never binds one, the guard is the contract), no control char (a name never
/// holds one, [`valid_name`]) and no whitespace.
fn is_folded_literal(value: &str) -> bool {
    !value.is_empty()
        && !value.contains("${")
        && !value.chars().any(|c| c.is_control() || c.is_whitespace())
}

/// The URL schemes a client argument's static head may start with for its
/// authority to be read ([`host_of_text`]), the ones [`normalise_ws_path`]
/// strips.
const WS_SCHEMES: [&str; 4] = ["ws://", "wss://", "http://", "https://"];

/// CB.21: the authority a client URL dials, `host[:port]` with any userinfo
/// dropped, or `None` when the source does not spell it, on the URL already
/// read as text ([`client_text`]).
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
fn host_of_text(text: &str) -> Option<String> {
    let (head, dynamic) = match text.find(DYNAMIC) {
        Some(i) => (text.get(..i)?, true),
        None => (text, false),
    };
    let rest = WS_SCHEMES.iter().find_map(|s| head.strip_prefix(s))?;
    if dynamic && !rest.contains(['/', '?', '#']) {
        return None;
    }
    client_url_split(head).0
}

/// Template-literal content into `out`, each `${...}` span as [`DYNAMIC`]
/// (DT.3b: or, with a resolver, its expression's literal, [`Fold::push`]; a
/// span that never closes is always [`DYNAMIC`]).
fn push_template(out: &mut String, lit: &str, fold: &mut Fold<'_>) {
    let mut rest = lit;
    while let Some(open) = rest.find("${") {
        out.push_str(rest.get(..open).unwrap_or(""));
        let body = rest.get(open + 2..).unwrap_or("");
        push_span(out, body, fold);
        rest = body.get(span_end(body)..).unwrap_or("");
    }
    out.push_str(rest);
}

/// The `${..}` span whose text starts at `body` (just inside the `${`) into
/// `out`: its expression through [`Fold::push`] when it closes, else one
/// [`DYNAMIC`].
fn push_span(out: &mut String, body: &str, fold: &mut Fold<'_>) {
    match span_close(body).and_then(|close| body.get(..close)) {
        Some(expr) => fold.push(out, expr),
        None => out.push(DYNAMIC),
    }
}

/// For the text just inside a `${`: the offset just past its matching `}`, or
/// the end of `body` when it never closes.
fn span_end(body: &str) -> usize {
    span_close(body).map_or(body.len(), |close| close + 1)
}

/// For the text just inside a `${`: the offset of its matching `}`, or `None`
/// when it never closes.
fn span_close(body: &str) -> Option<usize> {
    let mut depth = 1i32;
    for (i, c) in body.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
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

/// DH.9: Dart literal content into `out`, each `$name` / `${expr}`
/// interpolation as [`DYNAMIC`] (DT.3b: or, with a resolver, its literal,
/// [`Fold::push`]). A `\`-escaped char (`\$`) is kept as written and never
/// interpolates, as every other client literal keeps its escapes.
fn push_dart(out: &mut String, lit: &str, fold: &mut Fold<'_>) {
    let mut rest = lit;
    while let Some(at) = rest.find(['$', '\\']) {
        out.push_str(rest.get(..at).unwrap_or(""));
        let tail = rest.get(at..).unwrap_or("");
        let mut chars = tail.chars();
        let skip = match (chars.next(), chars.next()) {
            (Some('\\'), Some(c)) => {
                out.push('\\');
                out.push(c);
                1 + c.len_utf8()
            }
            (Some('$'), Some('{')) => {
                let body = tail.get(2..).unwrap_or("");
                push_span(out, body, fold);
                2 + span_end(body)
            }
            (Some('$'), Some(c)) if c.is_ascii_alphabetic() || c == '_' => {
                let len = tail
                    .bytes()
                    .skip(1)
                    .take_while(|b| is_ident_byte(*b))
                    .count();
                fold.push(out, tail.get(1..1 + len).unwrap_or(""));
                1 + len
            }
            (Some(c), _) => {
                out.push(c);
                c.len_utf8()
            }
            (None, _) => break,
        };
        rest = tail.get(skip..).unwrap_or("");
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

// ---- DT.3a: a client URL read through the code that builds it -------------

/// How far [`local_binding`] walks up from a client call, in lines.
const LOCAL_LOOKBACK_LINES: usize = 40;

/// Where a client site's URL text came from (DT.3a).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UrlVia {
    /// The call's own argument.
    Direct,
    /// The initializer of a `const` / `final` / `:=` binding in a scope
    /// enclosing the call ([`local_binding`]).
    Local,
    /// The initializer of the file's one `readonly` class field, for a
    /// `this.<m>` argument ([`class_field_init`]).
    Field,
}

/// One client site's URL ([`client_url`]): the text its path and host are
/// read from, a slice of the source, and how that text was reached. A
/// binding is followed once (depth one), never to a second binding.
#[derive(Clone, Copy)]
struct ClientUrl<'s> {
    text: &'s str,
    interp: Interp,
    via: UrlVia,
    /// DT.3b: the post-cache fold's resolver (`None` on the per-file pass),
    /// which [`Self::url_text`] reads `text` through.
    resolve: Option<Resolve<'s>>,
    /// `text` is a URL builder call's one literal ([`builder_literal`]): read
    /// root-relative ([`root_relative`]), and the node is Weak.
    builder: bool,
    /// `text` is a Go `url.URL{..}`'s `Path` literal ([`go_url_struct_path`]).
    url_struct: bool,
    /// The site's argument is an identifier, a member or a builder call, the
    /// shapes the `[ws] client urls` marker counts (read or unread).
    counted: bool,
}

impl ClientUrl<'_> {
    /// The request path the site names ([`path_of_text`]'s rule; a builder's
    /// literal root-relative, [`root_relative`]).
    fn path(&self) -> Option<String> {
        self.path_of(self.url_text()?.0)
    }

    /// CB.21: the literal authority the site dials ([`host_of_text`]). DT.3b:
    /// in the fold, the authority of the configured URL the site folded to;
    /// written in another file, which DF.8's external mark reads as configured
    /// when a config-shaped constant holds it.
    fn host(&self) -> Option<String> {
        host_of_text(&self.url_text()?.0)
    }

    /// DT.3b: what the fold did to this site's URL ([`WsFoldCounts`]).
    fn fold_read(&self) -> FoldRead {
        self.url_text().map_or(FoldRead::Plain, |(_, read)| read)
    }

    /// [`Self::path`] on a text already read.
    fn path_of(&self, text: String) -> Option<String> {
        if self.builder {
            path_of_text(root_relative(text))
        } else {
            path_of_text(text)
        }
    }

    /// The site's URL as text ([`client_text`]), and what the fold did to it.
    /// With no resolver, or a URL that is not [`Self::foldable`], the plain
    /// text. In the DT.3b fold, the text with every member chain the resolver
    /// binds replaced by its literal, unless that text reads no path where the
    /// plain one does (a value that is no URL base, `true/prices`): then the
    /// plain text stands and the site is unread.
    fn url_text(&self) -> Option<(String, FoldRead)> {
        let plain = client_text(self.text, self.interp, &mut Fold::new(None))?;
        let Some(resolve) = self.resolve.filter(|_| self.foldable()) else {
            return Some((plain, FoldRead::Plain));
        };
        let mut fold = Fold::new(Some(resolve));
        let folded = client_text(self.text, self.interp, &mut fold)?;
        if !fold.folded {
            let read = if fold.unread {
                FoldRead::Unread
            } else {
                FoldRead::Plain
            };
            return Some((plain, read));
        }
        let loses_path =
            self.path_of(folded.clone()).is_none() && self.path_of(plain.clone()).is_some();
        Some(if loses_path {
            (plain, FoldRead::Unread)
        } else {
            (folded, FoldRead::Folded)
        })
    }

    /// DT.3b: the whole argument is not a bare lower-case identifier or a
    /// `this.<m>` read straight off the call. Those are DT.3a's scoped reads
    /// ([`local_binding`], [`class_field_init`]): one they left unread is a
    /// `let`, a parameter, a mutable field or a binding out of reach, which
    /// the file-wide const table cannot tell apart. A SCREAMING_CASE name
    /// (an imported config constant) folds.
    fn foldable(&self) -> bool {
        let t = self.text.trim();
        let owned_by_dt3a = self.via == UrlVia::Direct
            && !self.builder
            && !self.url_struct
            && ((is_js_ident(t) && !is_screaming(t))
                || t.strip_prefix("this.").is_some_and(is_js_ident));
        !owned_by_dt3a
    }
}

/// DT.3b: what the base fold did to one client site's URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FoldRead {
    /// No member chain was offered to the resolver (or there is none).
    Plain,
    /// A member chain was replaced by its literal, and the reading stood.
    Folded,
    /// A member chain was offered and the plain reading stands.
    Unread,
}

/// `s` is SCREAMING_CASE: upper-case letters, digits and `_`, at least one
/// letter (the constants.rs cross-file trust rule's shape).
fn is_screaming(s: &str) -> bool {
    s.bytes()
        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        && s.bytes().any(|b| b.is_ascii_uppercase())
}

/// The `[ws] client urls` marker's counts for one file.
#[derive(Debug, Default)]
struct UrlReads {
    /// Sites whose URL was read through a binding or a builder.
    read: usize,
    local: usize,
    field: usize,
    /// Subsets of `read`: text from a builder call / a Go `url.URL`.
    builder: usize,
    url_struct: usize,
    /// Identifier / member / builder arguments whose URL stayed unreadable.
    unread: usize,
}

impl UrlReads {
    fn count(&mut self, url: &ClientUrl<'_>, named: bool) {
        if !url.counted {
            return;
        }
        let indirect = url.via != UrlVia::Direct || url.builder || url.url_struct;
        if !(named && indirect) {
            self.unread += 1;
            return;
        }
        self.read += 1;
        match url.via {
            UrlVia::Local => self.local += 1,
            UrlVia::Field => self.field += 1,
            UrlVia::Direct => {}
        }
        self.builder += usize::from(url.builder);
        self.url_struct += usize::from(url.url_struct);
    }
}

/// DT.3a: the [`ClientUrl`] of the client site at `offset`; `None` for a row
/// that reads no URL. The argument ([`client_url_arg`], Dart's `Uri.parse(..)`
/// already stepped into) is resolved once:
/// - a URL builder call with one literal reads that literal ([`builder_literal`]);
/// - a bare identifier reads the initializer of its binding in a scope
///   enclosing the call ([`local_binding`]);
/// - `this.<m>` reads the file's one `readonly <m> = ..` ([`class_field_init`]);
/// - Go's `<ident>.String()` reads the `Path` of the `url.URL{..}` the
///   identifier binds ([`go_url_struct_path`]).
///
/// An initializer is read like an argument: a builder call in it is a builder
/// read, and under [`Interp::Dart`] its `Uri.parse(..)` is stepped into again.
/// Anything else, or a binding that cannot be found, is the argument itself.
fn client_url<'s>(
    source: &'s str,
    offset: usize,
    r: &WsRow,
    resolve: Option<Resolve<'s>>,
) -> Option<ClientUrl<'s>> {
    let after = offset.checked_add(r.needle.len())?;
    let (arg, interp) = client_url_arg(source, after, r.read)?;
    let direct = ClientUrl {
        text: arg,
        interp,
        via: UrlVia::Direct,
        resolve,
        builder: false,
        url_struct: false,
        counted: false,
    };
    let shape = arg.trim();
    if let Some(lit) = builder_literal(shape) {
        return Some(ClientUrl {
            text: lit,
            builder: true,
            counted: true,
            ..direct
        });
    }
    let (init, via, go_string) =
        if let Some(m) = shape.strip_prefix("this.").filter(|m| is_js_ident(m)) {
            (class_field_init(source, m), UrlVia::Field, false)
        } else if is_js_ident(shape) {
            (local_binding(source, offset, shape), UrlVia::Local, false)
        } else if let Some(v) = shape.strip_suffix(".String()").filter(|v| is_js_ident(v)) {
            (local_binding(source, offset, v), UrlVia::Local, true)
        } else {
            return Some(ClientUrl {
                counted: is_member_chain(shape),
                ..direct
            });
        };
    let unread = ClientUrl {
        counted: true,
        ..direct
    };
    let Some(init) = init.map(str::trim) else {
        return Some(unread);
    };
    if go_string {
        return Some(match go_url_struct_path(init) {
            Some(path) => ClientUrl {
                text: path,
                via,
                url_struct: true,
                ..unread
            },
            None => unread,
        });
    }
    let init = match interp {
        Interp::Dart => client_uri_arg(init),
        Interp::Backtick => init,
    };
    Some(match builder_literal(init) {
        Some(lit) => ClientUrl {
            text: lit,
            via,
            builder: true,
            ..unread
        },
        None => ClientUrl {
            text: init,
            via,
            ..unread
        },
    })
}

/// A JS / TS / Dart / Go identifier byte (`$` included, so `$url` is not `url`).
fn is_js_ident_byte(b: u8) -> bool {
    is_ident_byte(b) || b == b'$'
}

/// `s` is exactly one identifier.
fn is_js_ident(s: &str) -> bool {
    s.as_bytes()
        .first()
        .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_' || *b == b'$')
        && s.bytes().all(is_js_ident_byte)
}

/// `s` is a dotted chain of identifiers (`this.urls.buildWsUrl`,
/// `environment.wsUrl`, `buildUrl`); a `#private` member is allowed.
fn is_member_chain(s: &str) -> bool {
    !s.is_empty()
        && s.split('.')
            .all(|seg| is_js_ident(seg.strip_prefix('#').unwrap_or(seg)))
}

/// DT.3a, CH.3a arm a / rule f: `<member chain>(<one string or template
/// literal>)` whose static text holds a `/` (a trailing `!` allowed): that
/// literal, quotes included. The builder may transform it, so a read through
/// it is Weak.
///
/// `this.urls.buildWsUrl('api/chat/ws')` -> `'api/chat/ws'`,
/// `buildUrl(`rooms/${id}`)` -> the template, `t('hello')` -> `None`.
fn builder_literal(expr: &str) -> Option<&str> {
    let expr = expr.trim();
    let expr = expr.strip_suffix(';').map_or(expr, str::trim_end);
    let open = expr.find('(')?;
    if !is_member_chain(expr.get(..open)?) {
        return None;
    }
    let region = call_region(expr, open + 1)?;
    let tail = expr
        .get(open + 1 + region.len()..)?
        .strip_prefix(')')?
        .trim();
    if !(tail.is_empty() || tail == "!") {
        return None;
    }
    let mut args = split_top(region, b',')
        .into_iter()
        .map(str::trim)
        .filter(|a| !a.is_empty());
    let (arg, None) = (args.next()?, args.next()) else {
        return None;
    };
    let (lit, rest) = leading_literal(arg)?;
    if !rest.trim().is_empty() {
        return None;
    }
    let mut text = String::new();
    if arg.starts_with('`') {
        push_template(&mut text, lit, &mut Fold::new(None));
    } else {
        text.push_str(lit);
    }
    text.contains('/').then_some(arg)
}

/// DT.3a: a builder literal with no scheme and no leading `/` is
/// root-relative (`api/chat/ws` -> `/api/chat/ws`, LB.5's canonical form). A
/// text that starts with a dynamic piece is left as it is.
fn root_relative(text: String) -> String {
    if text.starts_with('/') || text.starts_with(DYNAMIC) || text.contains("://") {
        text
    } else {
        format!("/{text}")
    }
}

/// DT.3a: gorilla's canonical client URL, `[&]url.URL{.. Path: "/echo" ..}`:
/// the `Path` field's literal, quotes included. Any other shape, or a `Path`
/// that is not one literal, is `None`.
fn go_url_struct_path(init: &str) -> Option<&str> {
    let s = init.trim();
    let s = s.strip_prefix('&').map_or(s, str::trim_start);
    let body = s.strip_prefix("url.URL")?.trim_start().strip_prefix('{')?;
    let region = call_region(body, 0)?;
    if !body
        .get(region.len()..)?
        .strip_prefix('}')?
        .trim()
        .is_empty()
    {
        return None;
    }
    split_top(region, b',').into_iter().find_map(|field| {
        let value = field
            .trim()
            .strip_prefix("Path")?
            .trim_start()
            .strip_prefix(':')?
            .trim();
        let (_, rest) = leading_literal(value)?;
        rest.trim().is_empty().then_some(value)
    })
}

/// Leading-whitespace width of `line`, in bytes (a tab counts one, as gofmt
/// indents with tabs only).
fn indent(line: &str) -> usize {
    line.len() - line.trim_start_matches([' ', '\t']).len()
}

/// `ident` occurs in `text` as a whole word that is not a member access
/// (`this.url` / `cfg.url` are not `url`).
fn mentions(text: &str, ident: &str) -> bool {
    let b = text.as_bytes();
    text.match_indices(ident).any(|(i, _)| {
        let before = i.checked_sub(1).and_then(|j| b.get(j));
        let after = b.get(i + ident.len());
        before.is_none_or(|c| !is_js_ident_byte(*c) && *c != b'.')
            && after.is_none_or(|c| !is_js_ident_byte(*c))
    })
}

/// The leading identifier-ish word of `s` (`export`, `const`, `func`).
fn first_word(s: &str) -> &str {
    let end = s
        .bytes()
        .position(|b| !is_js_ident_byte(b))
        .unwrap_or(s.len());
    s.get(..end).unwrap_or("")
}

/// `s` with its first word and the whitespace after it removed.
fn after_word<'a>(s: &'a str, word: &str) -> Option<&'a str> {
    let rest = s.strip_prefix(word)?;
    rest.starts_with([' ', '\t']).then(|| rest.trim_start())
}

/// Statement-opening words that start a block but bind no parameter.
const CONTROL_WORDS: &[&str] = &[
    "if",
    "else",
    "while",
    "switch",
    "do",
    "try",
    "with",
    "return",
    "when",
    "using",
    "lock",
    "synchronized",
    "foreach",
    "select",
    "defer",
    "go",
];

/// A shallower line `t` (trimmed) the walk climbs through binds `ident`:
/// a function / method / arrow / constructor header (TS `function`, Go
/// `func`, a line ending `=>` / `=> {`, `<name>(<params>)<ret?> {`,
/// `constructor(`) or a `for` / `catch` header, whose text names `ident`.
/// `if` / `while` / `switch` / `} else {` never bind.
fn binds_param(t: &str, ident: &str) -> bool {
    let t = t.trim_start_matches(['}', ' ', '\t']);
    let first = first_word(t);
    let header = matches!(first, "for" | "catch" | "function" | "func" | "constructor")
        || mentions_word(t, "function")
        || mentions_word(t, "func")
        || t.contains("=>")
        || (t.ends_with('{') && t.contains('(') && !CONTROL_WORDS.contains(&first));
    header && mentions(t, ident)
}

/// `word` occurs in `t` as a whole word.
fn mentions_word(t: &str, word: &str) -> bool {
    let b = t.as_bytes();
    t.match_indices(word).any(|(i, _)| {
        i.checked_sub(1)
            .and_then(|j| b.get(j))
            .is_none_or(|c| !is_js_ident_byte(*c))
            && b.get(i + word.len()).is_none_or(|c| !is_js_ident_byte(*c))
    })
}

/// What one statement line at the walk's level says about `ident`.
enum LineBinding {
    /// An immutable binding of `ident`; the initializer starts at this byte
    /// offset of the trimmed line.
    Init(usize),
    /// A mutable or unreadable binding of `ident`, or an assignment to it:
    /// the walk ends with no read (CH.3a's rule).
    Mutable,
    /// Nothing about `ident`.
    Other,
}

/// Words that open a declaration whose binding cannot change.
const IMMUTABLE_DECL: &[&str] = &["const", "final"];
/// Words that open a declaration whose binding can (or that cannot be read).
const MUTABLE_DECL: &[&str] = &["let", "var", "import"];
/// Modifiers a declaration may carry before its keyword.
const DECL_MODIFIERS: &[&str] = &["export", "static", "late", "public", "private", "protected"];
/// Words that start a statement, never a `<Type> <name> =` declaration.
const NOT_A_TYPE: &[&str] = &[
    "return", "await", "throw", "new", "case", "yield", "typeof", "delete", "void", "else", "in",
    "of", "if", "for", "while", "readonly", "default", "type",
];

/// The byte offset in `s` just after its first top-level assignment `=` (not
/// `==` / `=>` / `<=` / `>=` / `!=`), or `None`.
fn assign_at(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        if let Some(q) = quote {
            match c {
                b'\\' => i += 1,
                _ if c == q => quote = None,
                _ => {}
            }
        } else {
            match c {
                b'\'' | b'"' | b'`' => quote = Some(c),
                b'(' | b'[' | b'{' | b'<' => depth += 1,
                b')' | b']' | b'}' | b'>' => depth -= 1,
                b'=' if depth <= 0 => {
                    let prev = i.checked_sub(1).and_then(|j| b.get(j)).copied();
                    let next = b.get(i + 1).copied();
                    let compare = matches!(prev, Some(b'=' | b'!' | b'<' | b'>'))
                        || matches!(next, Some(b'=' | b'>'));
                    if !compare {
                        return Some(i + 1);
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

/// DT.3a: what the statement line `t` (trimmed) says about `ident`.
///
/// Immutable: TS / JS `const x =` / `const x: T =` (optionally `export`), Dart
/// `final [T ]x =` / `const [T ]x =` (`static` / `late` allowed), Go `x :=`
/// and `const x [T] =`. Mutable: `let` / `var` / `import` naming `ident`
/// (destructuring included), a `<Type> x =` declaration with no immutable
/// keyword, a multi-name `a, x :=`, and an assignment `x =` / `x +=` / `x++`.
fn binding_on(t: &str, ident: &str) -> LineBinding {
    let mut s = t;
    while let Some(rest) = DECL_MODIFIERS.iter().find_map(|m| after_word(s, m)) {
        s = rest;
    }
    let first = first_word(s);
    if IMMUTABLE_DECL.contains(&first) || MUTABLE_DECL.contains(&first) {
        let Some(rest) = after_word(s, first) else {
            return LineBinding::Other;
        };
        let at = assign_at(rest);
        let declared = at.and_then(|a| rest.get(..a - 1)).unwrap_or(rest);
        if !mentions(declared, ident) {
            return LineBinding::Other;
        }
        if MUTABLE_DECL.contains(&first) {
            return LineBinding::Mutable;
        }
        // `x`, `x: T`, `x T` (Go) or `T x` (Dart / C#), and nothing else.
        let declared = declared.trim();
        let simple = declared == ident
            || declared.strip_prefix(ident).is_some_and(|tail| {
                let ty = tail.trim_start();
                tail.starts_with([' ', '\t', ':'])
                    && (ty.starts_with(':')
                        || (ty.starts_with(|c: char| c.is_ascii_alphabetic()) && !ty.contains(',')))
            })
            || declared
                .rsplit_once([' ', '\t'])
                .is_some_and(|(ty, name)| name == ident && !ty.contains([',', '{', '[', '=']));
        return match at {
            Some(a) if simple => LineBinding::Init(t.len() - rest.len() + a),
            _ => LineBinding::Mutable,
        };
    }
    // Go `x :=` / `a, x :=`.
    if let Some(at) = s.find(":=") {
        let lhs = s.get(..at).unwrap_or("").trim();
        if lhs == ident {
            return LineBinding::Init(t.len() - s.len() + at + 2);
        }
        let names = lhs.split(',').map(str::trim);
        if lhs.contains(',')
            && names.clone().all(is_js_ident)
            && names.into_iter().any(|n| n == ident)
        {
            return LineBinding::Mutable;
        }
    }
    // An assignment to `ident`.
    if let Some(rest) = s.strip_prefix(ident)
        && !rest.starts_with(is_js_ident_byte_char)
    {
        let rest = rest.trim_start();
        let op = rest.trim_start_matches(['+', '-', '*', '/', '%', '|', '&', '?', '^']);
        let assigns = (op.starts_with('=') && !op.starts_with("==") && !op.starts_with("=>"))
            || rest.starts_with("++")
            || rest.starts_with("--");
        if assigns {
            return LineBinding::Mutable;
        }
    }
    // `<Type> x =` / `<Type> x;` with no immutable keyword (Dart, C#, Java).
    if !first.is_empty() && !NOT_A_TYPE.contains(&first) {
        let mut words = s.split_whitespace();
        if let (Some(ty), Some(name)) = (words.next(), words.next()) {
            let name = name.trim_end_matches(';');
            let next = words.next().unwrap_or("");
            let declares = name == ident
                && (s.trim_end().ends_with(';')
                    || (next.starts_with('=') && !next.starts_with("==")))
                && ty
                    .bytes()
                    .all(|b| is_js_ident_byte(b) || b"<>?.[]".contains(&b));
            if declares {
                return LineBinding::Mutable;
            }
        }
    }
    LineBinding::Other
}

fn is_js_ident_byte_char(c: char) -> bool {
    u8::try_from(c).is_ok_and(is_js_ident_byte)
}

/// DT.3a: the initializer text of the statement whose value starts at
/// `start`, up to its top-level `;`, a trailing `//` comment, or the end of
/// its continuation lines (lines deeper than `base`, the statement line's
/// indentation). Bounded by `limit` and [`MAX_REGION`]; `None` when the
/// statement does not end before either.
fn initializer(source: &str, start: usize, limit: usize, base: usize) -> Option<&str> {
    let region = clip(source.get(start..limit.min(source.len()))?, MAX_REGION);
    let b = region.as_bytes();
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
                        return region.get(..i);
                    }
                    depth -= 1;
                }
                b';' if depth == 0 => return region.get(..i),
                b'/' if b.get(i + 1) == Some(&b'/') => {
                    if depth == 0 {
                        return region.get(..i);
                    }
                    i = region.get(i..)?.find('\n').map_or(b.len(), |nl| i + nl) - 1;
                }
                b'\n' if depth == 0 => {
                    let continues = region
                        .get(i + 1..)?
                        .split('\n')
                        .find(|l| !l.trim().is_empty())
                        .is_some_and(|l| indent(l) > base);
                    if !continues {
                        return region.get(..i);
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

/// A line that is only a comment (`//`, `/* .. */`, a `*` doc line, `# `).
fn is_comment_line(t: &str) -> bool {
    t.starts_with("//")
        || t.starts_with("/*")
        || t.starts_with("*/")
        || t == "*"
        || t.starts_with("* ")
        || t.starts_with("# ")
}

/// DT.3a: the initializer of the immutable binding of `ident` in a scope
/// enclosing the call at `call`, or `None`.
///
/// The walk goes up from the call line, at most [`LOCAL_LOOKBACK_LINES`].
/// `level` starts at the call line's indentation; a deeper line is skipped (a
/// closed nested block or a continuation), unless it assigns `ident`. A
/// shallower line opens an enclosing block and lowers `level`: when it is a
/// function / method / arrow / constructor or `for` / `catch` header whose
/// text names `ident` ([`binds_param`]), or the `)` closing a multi-line
/// parameter list whose lines name it, `ident` is that parameter and the walk
/// ends with `None`; a shallower line that is a complete statement (ends `;`)
/// or a bare `}` means the layout was lost, and the walk ends too. At `level`,
/// a binding line ([`binding_on`]) ends the walk: an immutable one returns
/// its initializer ([`initializer`]), a mutable one `None`. A call line whose
/// text before the needle opens a scope (`{` / `=>`) naming `ident` reads
/// nothing.
fn local_binding<'s>(source: &'s str, call: usize, ident: &str) -> Option<&'s str> {
    let line_start = source.get(..call)?.rfind('\n').map_or(0, |nl| nl + 1);
    let before = source.get(line_start..call)?;
    if (before.contains('{') || before.contains("=>")) && mentions(before, ident) {
        return None;
    }
    let mut level = indent(before);
    let mut params_above = false;
    let mut pos = line_start;
    for _ in 0..LOCAL_LOOKBACK_LINES {
        if pos == 0 {
            break;
        }
        let end = pos - 1;
        let start = source.get(..end)?.rfind('\n').map_or(0, |nl| nl + 1);
        pos = start;
        let raw = source.get(start..end)?;
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let t = line.trim();
        if t.is_empty() || is_comment_line(t) {
            continue;
        }
        let ind = indent(line);
        if ind > level {
            if params_above && mentions(t, ident) {
                return None;
            }
            if matches!(binding_on(t, ident), LineBinding::Mutable) && t.starts_with(ident) {
                return None;
            }
            continue;
        }
        if ind < level {
            if t.ends_with(';') || t == "}" || binds_param(t, ident) {
                return None;
            }
            params_above = t.starts_with(')');
            level = ind;
            continue;
        }
        if params_above {
            params_above = false;
            if mentions(t, ident) {
                return None;
            }
            continue;
        }
        match binding_on(t, ident) {
            LineBinding::Init(at) => {
                let value = start + (line.len() - line.trim_start().len()) + at;
                return initializer(source, value, line_start, ind);
            }
            LineBinding::Mutable => return None,
            LineBinding::Other => {}
        }
    }
    None
}

/// DT.3a: for a `this.<m>` client argument, the initializer of the file's
/// one `readonly <m> = <expr>` (a TS / JS class field; `private` / `public` /
/// `protected` / `static` allowed). Any other declaration of `<m>` in the
/// file (a non-readonly field, a getter, a parameter property, a second
/// field) makes it `None`.
fn class_field_init<'s>(source: &'s str, member: &str) -> Option<&'s str> {
    let mut found: Option<(usize, usize)> = None;
    let mut decls = 0usize;
    let mut offset = 0usize;
    for raw in source.split_inclusive('\n') {
        let line_at = offset;
        offset += raw.len();
        let line = raw.trim_end_matches(['\n', '\r']);
        let t = line.trim_start();
        let mut s = t;
        let mut readonly = false;
        loop {
            if let Some(rest) = [
                "private",
                "public",
                "protected",
                "static",
                "override",
                "declare",
            ]
            .iter()
            .find_map(|m| after_word(s, m))
            {
                s = rest;
            } else if let Some(rest) = after_word(s, "readonly") {
                s = rest;
                readonly = true;
            } else {
                break;
            }
        }
        let accessor = ["get", "set"].iter().find_map(|w| after_word(s, w));
        let Some(rest) = accessor.unwrap_or(s).strip_prefix(member) else {
            continue;
        };
        let rest = rest.trim_start_matches(['?', '!']).trim_start();
        if rest.starts_with(is_js_ident_byte_char)
            || !(rest.starts_with([':', '=', ';', '(']) || rest.is_empty())
        {
            continue;
        }
        decls += 1;
        if readonly && accessor.is_none() {
            let at = assign_at(rest).map(|a| t.len() - rest.len() + a);
            found = at.map(|a| (line_at + (line.len() - t.len()) + a, indent(line)));
        }
    }
    let (value, base) = found.filter(|_| decls == 1)?;
    initializer(source, value, source.len(), base)
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
    /// The path one client URL argument names, read plain.
    fn client_path(arg: &str, interp: Interp) -> Option<String> {
        path_of_text(client_text(arg, interp, &mut Fold::new(None))?)
    }
    /// The host one client URL argument dials, read plain.
    fn client_host(arg: &str, interp: Interp) -> Option<String> {
        host_of_text(&client_text(arg, interp, &mut Fold::new(None))?)
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
                | PathRead::ClientUri(_)
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
        assert_eq!(
            client_path("\"ws://h/ß\"", Interp::Backtick),
            Some("/ß".to_string())
        );
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
        assert_eq!(
            client_host("\"https://api.example.com/hubs/chat\"", Interp::Backtick),
            Some("api.example.com".into())
        );
        assert_eq!(
            client_host("`ws://notify:9000/live`", Interp::Backtick),
            Some("notify:9000".into())
        );
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
            assert_eq!(client_host(arg, Interp::Backtick), None, "{arg}");
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

    // ---- DH.9: Ratchet servers, Dart clients -----------------------------

    /// bench/substrate-gap/matrix/php/ws/server/server.php.
    const RATCHET_SERVER: &str = "<?php\n\
        \n\
        use Ratchet\\Http\\HttpServer;\n\
        use Ratchet\\Server\\IoServer;\n\
        use Ratchet\\WebSocket\\WsServer;\n\
        use Ratchet\\App;\n\
        \n\
        $app = new App('localhost', 8080);\n\
        $app->route('/ws/chat', new Chat(), ['*']);\n\
        $app->run();\n";

    #[test]
    fn ratchet_route_mints_handler() {
        let out = handlers(RATCHET_SERVER);
        assert_eq!(anchored(&out), vec![("ws:/ws/chat".to_string(), 8)]);
        // A qualified `new \Ratchet\App(..)` gates the file as well, and every
        // route is read.
        let src = "<?php\n$app = new \\Ratchet\\App('localhost', 8080);\n\
                   $app->route('/chat', new MyChat, array('*'));\n\
                   $app->route(\"/echo\", new Echo, array('*'), 'localhost');\n";
        assert_eq!(qnames(&handlers(src)), vec!["ws:/chat", "ws:/echo"]);
    }

    #[test]
    fn ratchet_route_without_path_literal_mints_nothing() {
        let src = "<?php\nuse Ratchet\\App;\n$app = new App('localhost', 8080);\n\
                   $app->route($path, new Chat(), ['*']);\n\
                   $app->route(self::PATH . '/chat', new Chat());\n";
        let out = handlers(src);
        assert!(out.nodes.is_empty(), "{:?}", qnames(&out));
        assert!(out.anchors.is_empty());
        // `->route(` outside a Ratchet file (Slim, Laravel) is an HTTP router.
        let slim = "<?php\nuse Slim\\Factory\\AppFactory;\n\
                    $app->route('/users', 'UserController');\n\
                    $router->route('/ws/chat', $handler);\n";
        assert!(
            handlers(slim).nodes.is_empty(),
            "{:?}",
            qnames(&handlers(slim))
        );
        // A Ratchet route is never a client.
        assert!(clients(RATCHET_SERVER).nodes.is_empty());
    }

    /// bench/substrate-gap/matrix/dart/ws/client/lib/chat.dart.
    const DART_CHANNEL: &str = "import 'package:web_socket_channel/web_socket_channel.dart';\n\
        \n\
        WebSocketChannel connect() {\n\
        \x20 final channel = WebSocketChannel.connect(Uri.parse('ws://api/ws/chat'));\n\
        \x20 channel.stream.listen((m) => print(m));\n\
        \x20 return channel;\n\
        }\n";

    #[test]
    fn web_socket_channel_uri_parse_client() {
        let out = clients(DART_CHANNEL);
        assert_eq!(anchored(&out), vec![("ws_client:/ws/chat".to_string(), 3)]);
        assert_eq!(
            hits(&out),
            vec![(
                "ws_client:/ws/chat".to_string(),
                vec![r#"{"via":"ws","host":"api"}"#.to_string()]
            )]
        );
        // `Uri.tryParse(..)` steps in the same way; a variable inside the
        // wrapper is the generic client, with no host.
        let src = "import 'package:web_socket_channel/web_socket_channel.dart';\n\
                   final a = WebSocketChannel.connect(Uri.tryParse(\"wss://feed.example.com/live\")!);\n\
                   final b = WebSocketChannel.connect(Uri.parse(url));\n\
                   final c = WebSocketChannel.connect(Uri.parse('ws://h/x').replace(port: 9));\n";
        assert_eq!(
            hits(&clients(src)),
            vec![
                (
                    "ws_client:/live".to_string(),
                    vec![r#"{"via":"ws","host":"feed.example.com"}"#.to_string()]
                ),
                (
                    "ws_client:ws".to_string(),
                    vec![r#"{"via":"ws"}"#.to_string(), r#"{"via":"ws"}"#.to_string()]
                ),
            ]
        );
    }

    #[test]
    fn io_web_socket_channel_string_url() {
        let src = "import 'package:web_socket_channel/io.dart';\n\
                   import 'package:web_socket_channel/html.dart';\n\
                   final io = IOWebSocketChannel.connect('ws://chat-svc:8080/ws/io');\n\
                   final web = HtmlWebSocketChannel.connect(Uri.parse('wss://chat-svc/ws/web'));\n";
        let out = clients(src);
        assert_eq!(
            anchored(&out),
            vec![
                ("ws_client:/ws/io".to_string(), 2),
                ("ws_client:/ws/web".to_string(), 3)
            ]
        );
        assert_eq!(
            hits(&out),
            vec![
                (
                    "ws_client:/ws/io".to_string(),
                    vec![r#"{"via":"ws","host":"chat-svc:8080"}"#.to_string()]
                ),
                (
                    "ws_client:/ws/web".to_string(),
                    vec![r#"{"via":"ws","host":"chat-svc"}"#.to_string()]
                ),
            ]
        );
    }

    #[test]
    fn dart_io_websocket_connect() {
        let src = "import 'dart:io';\n\
                   Future<void> main() async {\n\
                   \x20 final socket = await WebSocket.connect('ws://localhost:4040/ws/raw');\n\
                   \x20 socket.add('hi');\n\
                   }\n";
        let out = clients(src);
        assert_eq!(anchored(&out), vec![("ws_client:/ws/raw".to_string(), 2)]);
        assert_eq!(
            hits(&out),
            vec![(
                "ws_client:/ws/raw".to_string(),
                vec![r#"{"via":"ws","host":"localhost:4040"}"#.to_string()]
            )]
        );
    }

    /// Dart interpolates `$name` and `${expr}` in any quoted literal: each is
    /// a dynamic piece, never path or host text.
    #[test]
    fn dart_interpolation_is_dynamic() {
        let src = "import 'package:web_socket_channel/web_socket_channel.dart';\n\
                   final a = WebSocketChannel.connect(Uri.parse('$wsBase/ws/chat'));\n\
                   final b = WebSocketChannel.connect(Uri.parse('ws://${env.host}/ws/feed'));\n\
                   final c = WebSocketChannel.connect(Uri.parse('ws://h/rooms/$room'));\n\
                   final d = WebSocketChannel.connect(Uri.parse('ws://h/cost\\$/x'));\n";
        assert_eq!(
            hits(&clients(src)),
            vec![
                (
                    "ws_client:/ws/chat".to_string(),
                    vec![r#"{"via":"ws"}"#.to_string()]
                ),
                (
                    "ws_client:/ws/feed".to_string(),
                    vec![r#"{"via":"ws"}"#.to_string()]
                ),
                (
                    "ws_client:/rooms/".to_string(),
                    vec![r#"{"via":"ws","host":"h"}"#.to_string()]
                ),
                (
                    "ws_client:/cost\\$/x".to_string(),
                    vec![r#"{"via":"ws","host":"h"}"#.to_string()]
                ),
            ]
        );
        // The JS rows keep reading `$` as text: no Dart rule leaks into them.
        let js = "const ws = new WebSocket('ws://h/$live');\n";
        assert_eq!(qnames(&clients(js)), vec!["ws_client:/$live"]);
    }

    #[test]
    fn ws_connect_without_the_package_is_nothing() {
        // No web_socket_channel / dart:io import: someone else's `connect`.
        let src = "import 'package:my_sockets/my_sockets.dart';\n\
                   final a = WebSocketChannel.connect(Uri.parse('ws://api/ws/chat'));\n\
                   final b = IOWebSocketChannel.connect('ws://api/ws/io');\n\
                   final c = await WebSocket.connect('ws://api/ws/raw');\n";
        let out = clients(src);
        assert!(out.nodes.is_empty(), "{:?}", qnames(&out));
        // A browser `new WebSocket(..)` beside a dart:io import is still only
        // the browser row, and the Dart row never reads the JS call.
        let js = "// dart:io\nconst ws = new WebSocket(\"ws://h/ws/js\");\n";
        assert_eq!(qnames(&clients(js)), vec!["ws_client:/ws/js"]);
        // Nor is a Dart client ever a handler.
        assert!(handlers(DART_CHANNEL).nodes.is_empty());
    }

    // ---- DT.3a: a client URL read through the code that builds it -------

    /// The `[ws] client urls` counts one file's client sites give, by the
    /// scan's own per-site steps.
    fn url_reads(src: &str) -> UrlReads {
        let mut reads = UrlReads::default();
        let mut guard = LazyGuard::new("t.src", src);
        let mut blocks = None;
        for r in CLIENT_ROWS.iter().filter(|r| gate_holds(src, r.gate)) {
            for offset in sites(src, r.needle, &mut guard) {
                let url = client_url(src, offset, r, None);
                let read = read_name(src, offset, r, url.as_ref(), &mut blocks);
                if let Some(u) = &url {
                    reads.count(u, read.is_some());
                }
            }
        }
        reads
    }

    /// `(read, local, field, builder, url_struct, unread)`.
    fn counts(r: &UrlReads) -> (usize, usize, usize, usize, usize, usize) {
        (r.read, r.local, r.field, r.builder, r.url_struct, r.unread)
    }

    /// examples-glia's `OrderFeedService::connect`.
    const ORDER_FEED: &str = "import { environment } from '../../../environments/environment';\n\
        \n\
        @Injectable({ providedIn: 'root' })\n\
        export class OrderFeedService {\n\
        \x20 readonly updates = new Subject<Order>();\n\
        \x20 private socket?: WebSocket;\n\
        \n\
        \x20 connect(): void {\n\
        \x20   const url = `${environment.liveUrl}/live/orders`;\n\
        \x20   this.socket = new WebSocket(url);\n\
        \x20   this.socket.onmessage = (event) => this.updates.next(JSON.parse(event.data));\n\
        \x20 }\n\
        }\n";

    #[test]
    fn const_local_template_reads_its_tail() {
        let out = clients(ORDER_FEED);
        assert_eq!(
            anchored(&out),
            vec![("ws_client:/live/orders".to_string(), 9)]
        );
        assert_eq!(out.nodes[0].confidence, Confidence::Medium);
        assert_eq!(hits(&out)[0].1, vec![r#"{"via":"ws"}"#.to_string()]);
        assert_eq!(counts(&url_reads(ORDER_FEED)), (1, 1, 0, 0, 0, 0));
        // `export const`, a type annotation and a continuation line read too.
        let src = "export function open() {\n\
                   \x20 const url: string =\n\
                   \x20   `${base}/ws/typed`;\n\
                   \x20 return new WebSocket(url);\n}\n";
        assert_eq!(qnames(&clients(src)), vec!["ws_client:/ws/typed"]);
    }

    /// quokka's `ChatWebSocketTransport::openSocket`.
    #[test]
    fn builder_local_reads_its_literal() {
        let src = "export class ChatWebSocketTransport {\n\
                   \x20 constructor(\n\
                   \x20   private readonly apiUrlBuilder: ApiUrlBuilderService\n\
                   \x20 ) {}\n\
                   \n\
                   \x20 private openSocket(isReconnect: boolean): void {\n\
                   \x20   if (this.manuallyDisconnected || this.authRequired) {\n\
                   \x20     this.setConnectionState('DISCONNECTED');\n\
                   \x20     return;\n\
                   \x20   }\n\
                   \n\
                   \x20   this.setConnectionState(isReconnect ? 'RECONNECTING' : 'CONNECTING');\n\
                   \n\
                   \x20   const url = this.apiUrlBuilder.buildWsUrl('api/protected/chat/ws');\n\
                   \x20   const nextSocket = new WebSocket(url);\n\
                   \x20   this.socket = nextSocket;\n\
                   \x20 }\n\
                   }\n";
        let out = clients(src);
        assert_eq!(
            anchored(&out),
            vec![("ws_client:/api/protected/chat/ws".to_string(), 14)]
        );
        assert_eq!(
            out.nodes[0].confidence,
            Confidence::Weak,
            "a builder read is Weak"
        );
        assert_eq!(counts(&url_reads(src)), (1, 1, 0, 1, 0, 0));
        // The call's own argument reads through a builder as well; a literal
        // with a leading `/` stays as written, a template's static tail reads.
        let src = "connect() {\n  return new WebSocket(this.urls.buildWsUrl('/ws/direct'));\n}\n\
                   rooms(id) {\n  return new WebSocket(buildUrl(`rooms/${id}/ws`));\n}\n";
        let out = clients(src);
        assert_eq!(qnames(&out), vec!["ws_client:/ws/direct", "ws_client:/ws"]);
        assert!(out.nodes.iter().all(|n| n.confidence == Confidence::Weak));
        assert_eq!(counts(&url_reads(src)), (2, 0, 0, 2, 0, 0));
        // A literal read once through a builder and once directly is Medium.
        let src = "a() {\n  const url = this.urls.buildWsUrl('/ws/mix');\n  return new WebSocket(url);\n}\n\
                   b() {\n  return new WebSocket('/ws/mix');\n}\n";
        assert_eq!(clients(src).nodes[0].confidence, Confidence::Medium);
        // A builder argument with no `/` is no URL; two arguments are no builder.
        for arg in ["this.i18n.t('hello')", "this.urls.build('api/ws', opts)"] {
            let src = format!("f() {{\n  const url = {arg};\n  return new WebSocket(url);\n}}\n");
            assert_eq!(qnames(&clients(&src)), vec!["ws_client:ws"], "{arg}");
        }
    }

    #[test]
    fn let_or_reassigned_local_is_unread() {
        let src = "admin() {\n\
                   \x20 let url = '/admin/live';\n\
                   \x20 url = url + '?x';\n\
                   \x20 return new WebSocket(url);\n}\n";
        assert_eq!(qnames(&clients(src)), vec!["ws_client:ws"]);
        assert_eq!(counts(&url_reads(src)), (0, 0, 0, 0, 0, 1));
        for decl in [
            "let url = '/a';",
            "var url = '/a';",
            "url = '/a';",
            "url += '/a';",
            "const { url } = cfg;",
            "String url = '/a';",
        ] {
            let src = format!("f() {{\n  {decl}\n  return new WebSocket(url);\n}}\n");
            assert_eq!(qnames(&clients(&src)), vec!["ws_client:ws"], "{decl}");
        }
        // A reassignment in a closed nested block also blocks the read.
        let src = "f() {\n  u := \"ws://h/a\"\n  if x {\n    u = other\n  }\n  websocket.DefaultDialer.Dial(u, nil)\n}\n";
        let go = format!("import \"github.com/gorilla/websocket\"\n{src}");
        assert_eq!(qnames(&clients(&go)), vec!["ws_client:ws"]);
    }

    #[test]
    fn const_in_another_function_is_not_read() {
        let src = "export class Feed {\n\
                   \x20 first(): void {\n\
                   \x20   const url = '/ws/first';\n\
                   \x20   this.log(url);\n\
                   \x20 }\n\
                   \n\
                   \x20 second(): WebSocket {\n\
                   \x20   return new WebSocket(url);\n\
                   \x20 }\n\
                   }\n";
        assert_eq!(qnames(&clients(src)), vec!["ws_client:ws"]);
        // A nested closure reads the const of the function enclosing it, and
        // a module const reads through a function that does not shadow it.
        let src = "const WS_URL = 'wss://feed.example.com/ws/module';\n\
                   export function outer() {\n\
                   \x20 const url = '/ws/outer';\n\
                   \x20 return () => {\n\
                   \x20   return new WebSocket(url);\n\
                   \x20 };\n\
                   }\n\
                   export function module() {\n\
                   \x20 return new WebSocket(WS_URL);\n\
                   }\n";
        assert_eq!(
            qnames(&clients(src)),
            vec!["ws_client:/ws/outer", "ws_client:/ws/module"]
        );
    }

    #[test]
    fn parameter_is_not_followed() {
        let module = "const url = '/ws/module';\n";
        for body in [
            "export function connect(url: string) {\n  return new WebSocket(url);\n}\n",
            "class C {\n  connect(url: string): WebSocket {\n    return new WebSocket(url);\n  }\n}\n",
            "class C {\n  connect(\n    url: string,\n  ): WebSocket {\n    return new WebSocket(url);\n  }\n}\n",
            "const open = (url: string) => {\n  return new WebSocket(url);\n};\n",
            "const open = (url) => new WebSocket(url);\n",
            "for (const url of urls) {\n  new WebSocket(url);\n}\n",
            "class C {\n  constructor(url: string) {\n    this.ws = new WebSocket(url);\n  }\n}\n",
        ] {
            let src = format!("{module}{body}");
            assert_eq!(qnames(&clients(&src)), vec!["ws_client:ws"], "{body}");
        }
        let go = "import \"github.com/gorilla/websocket\"\n\
                  const u = \"ws://h/module\"\n\
                  func dial(u string) {\n\
                  \tc, _, _ := websocket.DefaultDialer.Dial(u, nil)\n\
                  }\n";
        assert_eq!(qnames(&clients(go)), vec!["ws_client:ws"]);
    }

    #[test]
    fn else_branch_does_not_read_the_if_branch() {
        let src = "f(a) {\n\
                   \x20 if (a) {\n\
                   \x20   const url = '/a';\n\
                   \x20 } else {\n\
                   \x20   return new WebSocket(url);\n\
                   \x20 }\n}\n";
        assert_eq!(qnames(&clients(src)), vec!["ws_client:ws"]);
        let one_line = "if (a) { const url = '/a' } else { new WebSocket(url) }\n";
        assert_eq!(qnames(&clients(one_line)), vec!["ws_client:ws"]);
        // A const before the `if` is in an enclosing scope of both branches.
        let src = "f(a) {\n\
                   \x20 const url = '/ws/both';\n\
                   \x20 if (a) {\n\
                   \x20   log(a);\n\
                   \x20 } else {\n\
                   \x20   return new WebSocket(url);\n\
                   \x20 }\n}\n";
        assert_eq!(qnames(&clients(src)), vec!["ws_client:/ws/both"]);
    }

    #[test]
    fn readonly_field_reads() {
        let src = "export class Feed {\n\
                   \x20 private readonly wsUrl = 'ws://h/feed';\n\
                   \n\
                   \x20 open(): WebSocket {\n\
                   \x20   return new WebSocket(this.wsUrl);\n\
                   \x20 }\n\
                   }\n";
        let out = clients(src);
        assert_eq!(
            hits(&out),
            vec![(
                "ws_client:/feed".to_string(),
                vec![r#"{"via":"ws","host":"h"}"#.to_string()]
            )]
        );
        assert_eq!(counts(&url_reads(src)), (1, 0, 1, 0, 0, 0));
        // A non-readonly field, a readonly with no initializer, a getter and
        // two declarations read nothing.
        for fields in [
            "  private wsUrl = 'ws://h/feed';\n",
            "  readonly wsUrl: string;\n",
            "  get wsUrl() { return 'ws://h/feed'; }\n",
            "  readonly wsUrl = 'ws://h/feed';\n  readonly wsUrl = 'ws://h/other';\n",
        ] {
            let src = format!(
                "export class Feed {{\n{fields}  open() {{\n    return new WebSocket(this.wsUrl);\n  }}\n}}\n"
            );
            assert_eq!(qnames(&clients(&src)), vec!["ws_client:ws"], "{fields}");
            assert_eq!(counts(&url_reads(&src)), (0, 0, 0, 0, 0, 1), "{fields}");
        }
        // A member chain beyond `this.<m>` stays unread.
        let src = "f() { return new WebSocket(environment.wsUrl); }\n";
        assert_eq!(counts(&url_reads(src)), (0, 0, 0, 0, 0, 1));
    }

    #[test]
    fn dart_final_uri_reads() {
        let src = "import 'package:web_socket_channel/web_socket_channel.dart';\n\
                   \n\
                   WebSocketChannel live() {\n\
                   \x20 final uri = Uri.parse('ws://h/live');\n\
                   \x20 return WebSocketChannel.connect(uri);\n\
                   }\n\
                   \n\
                   WebSocketChannel feed() {\n\
                   \x20 const String url = 'ws://h/feed/$room';\n\
                   \x20 final ok = 1;\n\
                   \x20 return WebSocketChannel.connect(Uri.parse(url));\n\
                   }\n";
        let out = clients(src);
        assert_eq!(
            hits(&out),
            vec![
                (
                    "ws_client:/live".to_string(),
                    vec![r#"{"via":"ws","host":"h"}"#.to_string()]
                ),
                (
                    "ws_client:/feed/".to_string(),
                    vec![r#"{"via":"ws","host":"h"}"#.to_string()]
                ),
            ]
        );
        assert_eq!(counts(&url_reads(src)), (2, 2, 0, 0, 0, 0));
        // A Dart class's `final` field is in scope of its methods by bare name.
        let src = "import 'package:web_socket_channel/io.dart';\n\
                   class Feed {\n\
                   \x20 static final url = 'ws://h/ws/field';\n\
                   \x20 IOWebSocketChannel open() {\n\
                   \x20   return IOWebSocketChannel.connect(url);\n\
                   \x20 }\n\
                   }\n";
        assert_eq!(qnames(&clients(src)), vec!["ws_client:/ws/field"]);
    }

    #[test]
    fn go_url_struct_reads() {
        let src = "package main\n\
                   \n\
                   import (\n\
                   \t\"net/url\"\n\
                   \n\
                   \t\"github.com/gorilla/websocket\"\n\
                   )\n\
                   \n\
                   func main() {\n\
                   \tu := url.URL{Scheme: \"ws\", Host: *addr, Path: \"/echo\"}\n\
                   \tlog.Printf(\"connecting to %s\", u.String())\n\
                   \n\
                   \tc, _, err := websocket.DefaultDialer.Dial(u.String(), nil)\n\
                   \tdefer c.Close()\n\
                   }\n\
                   \n\
                   func feed() {\n\
                   \tf := &url.URL{\n\
                   \t\tScheme: \"wss\",\n\
                   \t\tHost:   host,\n\
                   \t\tPath:   \"/ws/feed\", // the feed socket\n\
                   \t}\n\
                   \tw := \"ws://chat-svc:8080/ws/raw\"\n\
                   \twebsocket.DefaultDialer.Dial(f.String(), nil)\n\
                   \twebsocket.DefaultDialer.Dial(w, nil)\n\
                   }\n";
        let out = clients(src);
        assert_eq!(
            anchored(&out),
            vec![
                ("ws_client:/echo".to_string(), 12),
                ("ws_client:/ws/feed".to_string(), 23),
                ("ws_client:/ws/raw".to_string(), 24),
            ]
        );
        assert_eq!(counts(&url_reads(src)), (3, 3, 0, 0, 2, 0));
        // `.String()` of anything but a url.URL reads nothing.
        let src = "import \"github.com/gorilla/websocket\"\n\
                   func f() {\n\tu := build()\n\twebsocket.DefaultDialer.Dial(u.String(), nil)\n}\n";
        assert_eq!(qnames(&clients(src)), vec!["ws_client:ws"]);
        assert_eq!(counts(&url_reads(src)), (0, 0, 0, 0, 0, 1));
    }

    #[test]
    fn host_read_through_local() {
        let src = "export function chat() {\n\
                   \x20 // the chat service\n\
                   \x20 const u = 'ws://chat-svc:8080/ws';\n\
                   \x20 return new WebSocket(u);\n}\n";
        assert_eq!(
            hits(&clients(src)),
            vec![(
                "ws_client:/ws".to_string(),
                vec![r#"{"via":"ws","host":"chat-svc:8080"}"#.to_string()]
            )]
        );
    }

    /// The walk is bounded and never panics on odd text.
    #[test]
    fn local_walk_is_bounded_and_total() {
        let mut src = String::from("const url = '/ws/far';\n");
        src.push_str(&"log(1);\n".repeat(LOCAL_LOOKBACK_LINES));
        src.push_str("new WebSocket(url);\n");
        assert_eq!(qnames(&clients(&src)), vec!["ws_client:ws"]);
        for src in [
            "new WebSocket(url",
            "const url = 'ß/ws\nnew WebSocket(url)",
            "const url = `${\nnew WebSocket(url)",
            "\u{feff}const url = '/é';\r\nnew WebSocket(url);\r\n",
            "  const url = '/ws/a';\nnew WebSocket(url)",
        ] {
            let _ = clients(src);
        }
        // CRLF line ends read like LF.
        let src = "f() {\r\n  const url = '/ws/crlf';\r\n  return new WebSocket(url);\r\n}\r\n";
        assert_eq!(qnames(&clients(src)), vec!["ws_client:/ws/crlf"]);
    }

    #[test]
    fn builder_and_struct_readers() {
        assert_eq!(
            builder_literal("this.urls.buildWsUrl('api/chat/ws')"),
            Some("'api/chat/ws'")
        );
        assert_eq!(builder_literal("this.#urls.ws(\"/x\")!;"), Some("\"/x\""));
        assert_eq!(builder_literal("buildUrl(`a/${b}`)"), Some("`a/${b}`"));
        assert_eq!(builder_literal("new URL('/x')"), None);
        assert_eq!(
            builder_literal("this.urls.ws('/x').replace('a', 'b')"),
            None
        );
        assert_eq!(builder_literal("this.urls.ws(`${x}`)"), None);
        assert_eq!(builder_literal("this.urls.ws(path)"), None);
        assert_eq!(root_relative("api/ws".into()), "/api/ws");
        assert_eq!(root_relative("/api/ws".into()), "/api/ws");
        assert_eq!(root_relative("ws://h/x".into()), "ws://h/x");
        assert_eq!(go_url_struct_path("url.URL{Path: \"/a\"}"), Some("\"/a\""));
        assert_eq!(
            go_url_struct_path("&url.URL{Scheme: \"ws\", RawPath: \"/r\"}"),
            None
        );
        assert_eq!(go_url_struct_path("url.URL{Path: p}"), None);
        assert_eq!(
            go_url_struct_path("url.URL{Path: \"/a\"}.ResolveReference(b)"),
            None
        );
    }

    // ---- DT.3b: the post-cache base fold ----------------------------------

    /// A resolver over fixed `(expr, literal)` pairs, as the engine's LA.4
    /// resolver hands one over (exact keys only here).
    fn table(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |expr: &str| {
            pairs
                .iter()
                .find(|(k, _)| *k == expr)
                .map(|(_, v)| v.to_string())
        }
    }

    /// The fixture's environment.ts.
    const ENV: &[(&str, &str)] = &[
        ("environment.feedUrl", "ws://feed-svc:8080/rt/feed"),
        ("environment.wsBase", "ws://feed-svc:8080/rt"),
        ("environment.liveUrl", "ws://localhost:8080"),
        ("environment.flag", "true"),
        ("WS_URL", "wss://stream.example.org/ws"),
        ("base", "ws://local-svc/rt"),
        ("url", "ws://wrong-svc/wrong"),
        ("Env.wsBase", "ws://dart-svc:9000/rt"),
    ];

    const PATH_TS: &str = "src/live.ts";

    fn folded(src: &str) -> WsClientFold {
        extract_ws_client_nodes_with_consts(src, PATH_TS, module_id(), repo(), &table(ENV))
    }

    fn fold_qnames(fold: &WsClientFold) -> Vec<String> {
        qnames(&fold.nodes)
    }

    /// The fixture's live.ts.
    const LIVE: &str = "import { environment } from './environment';\n\
        \n\
        export function openFeed(): WebSocket {\n\
        \x20 return new WebSocket(environment.feedUrl);\n\
        }\n\
        \n\
        export function openPrices(): WebSocket {\n\
        \x20 return new WebSocket(`${environment.wsBase}/prices`);\n\
        }\n";

    /// A whole-argument config member folds to its configured URL: the path
    /// is the URL's, the host its authority.
    #[test]
    fn with_consts_folds_whole_member_url() {
        let src = "const ws = new WebSocket(environment.feedUrl);\n";
        assert_eq!(qnames(&clients(src)), vec!["ws_client:ws"]);
        let fold = folded(src);
        assert_eq!(fold_qnames(&fold), vec!["ws_client:/rt/feed"]);
        assert_eq!(fold.counts, WsFoldCounts { folded: 1, unresolved: 0 });
        assert_eq!(
            hits(&fold.nodes)[0].1,
            vec![r#"{"via":"ws","host":"feed-svc:8080"}"#.to_string()]
        );
        assert_eq!(fold.path, PATH_TS);
        // A base with no path names no path either way, but records its host.
        let bare = folded("const ws = new WebSocket(environment.liveUrl);\n");
        assert_eq!(fold_qnames(&bare), vec!["ws_client:ws"]);
        assert_eq!(bare.counts, WsFoldCounts { folded: 1, unresolved: 0 });
        assert_eq!(
            hits(&bare.nodes)[0].1,
            vec![r#"{"via":"ws","host":"localhost:8080"}"#.to_string()]
        );
        // A SCREAMING_CASE import is a config constant; DT.3a's local read
        // of the same file's own const still wins.
        let imported = folded("import { WS_URL } from './config';\nnew WebSocket(WS_URL);\n");
        assert_eq!(fold_qnames(&imported), vec!["ws_client:/ws"]);
        assert_eq!(
            hits(&imported.nodes)[0].1,
            vec![r#"{"via":"ws","host":"stream.example.org"}"#.to_string()]
        );
        let own = folded("const WS_URL = 'ws://own-svc/own';\nnew WebSocket(WS_URL);\n");
        assert_eq!(fold_qnames(&own), vec!["ws_client:/own"]);
        assert_eq!(own.counts, WsFoldCounts::default());
    }

    /// A template's base keeps its own path in front of the tail, so the tail
    /// alone never names the client; a `+` piece and a DT.3a local read fold
    /// the same way.
    #[test]
    fn with_consts_folds_template_base_with_path() {
        let plain = clients(LIVE);
        assert_eq!(qnames(&plain), vec!["ws_client:ws", "ws_client:/prices"]);
        let fold = folded(LIVE);
        assert_eq!(fold_qnames(&fold), vec!["ws_client:/rt/feed", "ws_client:/rt/prices"]);
        assert!(!fold_qnames(&fold).contains(&"ws_client:/prices".to_string()));
        assert_eq!(fold.counts, WsFoldCounts { folded: 2, unresolved: 0 });
        assert_eq!(
            anchored(&fold.nodes),
            vec![
                ("ws_client:/rt/feed".to_string(), 3),
                ("ws_client:/rt/prices".to_string(), 7)
            ]
        );
        for (_, payloads) in hits(&fold.nodes) {
            assert_eq!(payloads, vec![r#"{"via":"ws","host":"feed-svc:8080"}"#.to_string()]);
        }
        let concat = folded("new WebSocket(environment.wsBase + '/prices');\n");
        assert_eq!(fold_qnames(&concat), vec!["ws_client:/rt/prices"]);
        // examples-glia: the const local's template keeps its id and gains
        // the configured host.
        let feed = folded(ORDER_FEED);
        assert_eq!(fold_qnames(&feed), vec!["ws_client:/live/orders"]);
        assert_eq!(feed.counts, WsFoldCounts { folded: 1, unresolved: 0 });
        assert_eq!(
            hits(&feed.nodes)[0].1,
            vec![r#"{"via":"ws","host":"localhost:8080"}"#.to_string()]
        );
        // A same-file template span folds through the resolver too.
        let span = folded("new WebSocket(`${base}/prices`);\n");
        assert_eq!(fold_qnames(&span), vec!["ws_client:/rt/prices"]);
    }

    /// A chain nothing binds keeps the plain reading and counts unresolved;
    /// so does a bound value that would lose the path, and a site with no
    /// chain counts nothing.
    #[test]
    fn with_consts_unresolved_keeps_the_tail() {
        let src = "new WebSocket(`${environment.missing}/prices`);\n";
        let fold = folded(src);
        assert_eq!(fold_qnames(&fold), vec!["ws_client:/prices"]);
        assert_eq!(fold.counts, WsFoldCounts { folded: 0, unresolved: 1 });
        assert_eq!(hits(&fold.nodes)[0].1, vec![r#"{"via":"ws"}"#.to_string()]);
        // `true/prices` reads no path where `/prices` did: refused.
        let flag = folded("new WebSocket(`${environment.flag}/prices`);\n");
        assert_eq!(fold_qnames(&flag), vec!["ws_client:/prices"]);
        assert_eq!(flag.counts, WsFoldCounts { folded: 0, unresolved: 1 });
        // A call, an env read and a literal offer the resolver nothing.
        let none = folded(
            "new WebSocket(`${cfg()}/a`);\n\
             new WebSocket(\"ws://h/b\");\n",
        );
        assert_eq!(fold_qnames(&none), vec!["ws_client:/a", "ws_client:/b"]);
        assert_eq!(none.counts, WsFoldCounts::default());
        // A bare lower-case identifier or `this.<m>` passed whole is DT.3a's
        // scoped read: a `let` or a parameter is never folded from the
        // file-wide table.
        let owned = folded(
            "function open(url: string) {\n\
             \x20 return new WebSocket(url);\n}\n\
             class A {\n\
             \x20 private base = 'x';\n\
             \x20 open() { return new WebSocket(this.base); }\n}\n",
        );
        assert_eq!(fold_qnames(&owned), vec!["ws_client:ws"]);
        assert_eq!(owned.counts, WsFoldCounts::default());
        // A folded site is counted once, as folded, even with a path
        // parameter left dynamic.
        let param = folded("new WebSocket(`${environment.wsBase}/rooms/${roomId}`);\n");
        assert_eq!(fold_qnames(&param), vec!["ws_client:/rt/rooms/"]);
        assert_eq!(param.counts, WsFoldCounts { folded: 1, unresolved: 0 });
        assert_eq!(
            hits(&param.nodes)[0].1,
            vec![r#"{"via":"ws","host":"feed-svc:8080"}"#.to_string()]
        );
    }

    /// Dart's `$name` / `${expr}` interpolations fold like a template span.
    #[test]
    fn with_consts_dart_dollar_base() {
        let src = "import 'package:web_socket_channel/web_socket_channel.dart';\n\
                   final a = WebSocketChannel.connect(Uri.parse('${Env.wsBase}/prices'));\n\
                   final b = WebSocketChannel.connect(Uri.parse('$base/feed'));\n";
        assert_eq!(qnames(&clients(src)), vec!["ws_client:/prices", "ws_client:/feed"]);
        let fold = extract_ws_client_nodes_with_consts(
            src,
            "lib/live.dart",
            module_id(),
            repo(),
            &table(ENV),
        );
        assert_eq!(fold_qnames(&fold), vec!["ws_client:/rt/prices", "ws_client:/rt/feed"]);
        assert_eq!(fold.counts, WsFoldCounts { folded: 2, unresolved: 0 });
        assert_eq!(
            hits(&fold.nodes),
            vec![
                (
                    "ws_client:/rt/prices".to_string(),
                    vec![r#"{"via":"ws","host":"dart-svc:9000"}"#.to_string()]
                ),
                (
                    "ws_client:/rt/feed".to_string(),
                    vec![r#"{"via":"ws","host":"local-svc"}"#.to_string()]
                ),
            ]
        );
        // An escaped `\$` never interpolates.
        let escaped = "import 'package:web_socket_channel/web_socket_channel.dart';\n\
                       final c = WebSocketChannel.connect(Uri.parse('ws://h/\\$base/x'));\n";
        let fold = extract_ws_client_nodes_with_consts(
            escaped,
            "lib/e.dart",
            module_id(),
            repo(),
            &table(ENV),
        );
        assert_eq!(fold.counts, WsFoldCounts::default());
    }

    /// The function span the swap test's parse holds: lines 2..=4 and 6..=8
    /// (0-indexed) of [`LIVE`].
    fn live_fn(name: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, &format!("live::{name}"))
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

    /// The per-file pass over [`LIVE`]: a MODULE, its two FUNCTIONs, the
    /// handler and client scans, the anchor pass, then the router's IMPORTS
    /// cell, as `apply_cross_cutting_extractors` and the router lay them out.
    fn live_parse(src: &str) -> FileParse {
        use glia_core::{Cell, CellPayload};
        let module = module_id();
        let node = |id, cells| Node {
            id,
            repo: repo(),
            confidence: Confidence::Strong,
            cells,
        };
        let span = |start: u32, end: u32| Cell {
            kind: glia_code_domain::cell_type::POSITION,
            payload: CellPayload::Json(format!(
                r#"{{"file":"{PATH_TS}","start_line":{start},"end_line":{end}}}"#
            )),
        };
        let mut fp = FileParse {
            nodes: vec![
                node(module, vec![]),
                node(live_fn("openFeed"), vec![span(2, 4)]),
                node(live_fn("openPrices"), vec![span(6, 8)]),
            ],
            ..Default::default()
        };
        fp.nav.record(module, "live", "live", node_kind::MODULE, None);
        for f in ["openFeed", "openPrices"] {
            fp.nav
                .record(live_fn(f), f, &format!("live::{f}"), node_kind::FUNCTION, Some(module));
        }
        let mut anchors = Vec::new();
        for out in [
            extract_ws_handler_nodes(src, PATH_TS, module, repo()),
            extract_ws_client_nodes(src, PATH_TS, module, repo()),
        ] {
            fp.nodes.extend(out.nodes);
            anchors.extend(out.anchors);
            merge(&mut fp.nav, out.nav);
        }
        crate::anchor::attach(&mut fp, PATH_TS, module, &mut anchors);
        fp.imports.push(glia_code_domain::ImportStmt {
            from_module: "live".into(),
            target: glia_code_domain::ImportTarget::Module {
                path: "./environment".into(),
                alias: None,
            },
            line: 0,
        });
        glia_code_domain::attach_imports_cell(&mut fp, "typescript");
        fp
    }

    /// The literal spelling of [`LIVE`]: what the fold must lay out.
    const LIVE_LITERAL: &str = "import { environment } from './environment';\n\
        \n\
        export function openFeed(): WebSocket {\n\
        \x20 return new WebSocket('ws://feed-svc:8080/rt/feed');\n\
        }\n\
        \n\
        export function openPrices(): WebSocket {\n\
        \x20 return new WebSocket('ws://feed-svc:8080/rt/prices');\n\
        }\n";

    #[test]
    fn replace_ws_client_nodes_re_anchors() {
        use glia_code_domain::{cell_type, edge_category, evidence};
        let client = |q: &str| NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::WS_CLIENT, q);
        let mut fp = live_parse(LIVE);
        let (generic, tail) = (client("ws_client:ws"), client("ws_client:/prices"));
        assert!(fp.nav.kind_by_id.contains_key(&generic));
        assert!(fp.edges.iter().any(|e| e.to == tail && e.category == edge_category::USES));

        let fold = folded(LIVE);
        assert_eq!(fold.counts.folded, 2);
        replace_ws_client_nodes(&mut fp, module_id(), "typescript", fold);
        let literal = live_parse(LIVE_LITERAL);

        // Nodes (cells in the per-file order: ENDPOINT_HIT, POSITION, IMPORTS),
        // nav and child order are the literal file's.
        assert_eq!(fp.nodes, literal.nodes);
        assert_eq!(fp.nav.qname_by_id, literal.nav.qname_by_id);
        assert_eq!(fp.nav.kind_by_id, literal.nav.kind_by_id);
        assert_eq!(fp.nav.parent_of, literal.nav.parent_of);
        assert_eq!(fp.nav.children_of, literal.nav.children_of);
        // USES moved to the re-keyed nodes; nothing names a gone id.
        let (feed, prices) = (client("ws_client:/rt/feed"), client("ws_client:/rt/prices"));
        let uses: Vec<(NodeId, NodeId)> = fp
            .edges
            .iter()
            .filter(|e| e.category == edge_category::USES)
            .map(|e| (e.from, e.to))
            .collect();
        assert_eq!(uses, vec![(live_fn("openFeed"), feed), (live_fn("openPrices"), prices)]);
        assert!(
            !fp.edges
                .iter()
                .any(|e| [generic, tail].iter().any(|g| e.from == *g || e.to == *g))
        );
        assert!(!fp.nav.qname_by_id.contains_key(&generic));
        // Re-anchored post-cache: `extractor:anchor` rule `const_fold`.
        assert!(fp.edges.iter().all(|e| evidence::Evidence::of(e).is_some_and(
            |ev| ev.emitter == "extractor:anchor" && ev.rule.as_deref() == Some("const_fold")
        )));
        assert_eq!(crate::anchor::census(&fp), crate::anchor::census(&literal));
        for n in fp.nodes.iter().filter(|n| n.id == feed || n.id == prices) {
            assert_eq!(n.cells.iter().filter(|c| c.kind == cell_type::IMPORTS).count(), 1);
            assert_eq!(n.cells.last().map(|c| c.kind), Some(cell_type::IMPORTS));
        }
    }

    /// The per-file scan is the fold with no resolver: a file whose URLs hold
    /// no chain the resolver binds yields the same nodes, nav and anchors
    /// through either, and the per-file entry point reads no table.
    #[test]
    fn without_consts_is_byte_identical() {
        let srcs = [
            LIVE,
            ORDER_FEED,
            "const a = new WebSocket(\"ws://\" + location.host + \"/ws\");\n\
             const b = new WebSocket(url);\n",
            "import 'package:web_socket_channel/web_socket_channel.dart';\n\
             final a = WebSocketChannel.connect(Uri.parse('$host/ws/a'));\n",
            "import \"github.com/gorilla/websocket\"\n\
             c, _, _ := websocket.DefaultDialer.Dial(\"ws://chat-svc/ws\", nil)\n",
        ];
        let nothing = |_: &str| -> Option<String> { None };
        for src in srcs {
            let plain = clients(src);
            let fold = extract_ws_client_nodes_with_consts(src, "t.src", module_id(), repo(), &nothing);
            assert_eq!(plain.nodes, fold.nodes.nodes, "{src}");
            assert_eq!(plain.nav.qname_by_id, fold.nodes.nav.qname_by_id, "{src}");
            assert_eq!(plain.nav.children_of, fold.nodes.nav.children_of, "{src}");
            assert_eq!(plain.anchors, fold.nodes.anchors, "{src}");
            assert_eq!(fold.counts.folded, 0, "{src}");
        }
        // The per-file pass over the fixture is still HEAD's reading.
        assert_eq!(qnames(&clients(LIVE)), vec!["ws_client:ws", "ws_client:/prices"]);
        assert_eq!(
            hits(&clients(LIVE)),
            vec![
                ("ws_client:ws".to_string(), vec![r#"{"via":"ws"}"#.to_string()]),
                ("ws_client:/prices".to_string(), vec![r#"{"via":"ws"}"#.to_string()]),
            ]
        );
    }
}
