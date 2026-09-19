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
//! Anchors: the name is read per site, so EVERY site is anchored and
//! `anchor::attach` gives each owning function its HANDLED_BY (server) or USES
//! (client) edge. Python decorators anchor at the decorated `def` line
//! ([`AnchorAt::NextDef`]), because a Python FUNCTION span starts at its
//! `def`, not at the decorator above it.
//!
//! fired_on marker, one line per file per framework that yielded a node, `n`
//! = distinct nodes that framework's sites read, path repo-relative:
//!   `[ws] handlers framework=fastapi n=2 in app.py`
//!   `[ws] clients framework=browser n=2 in live.ts`
//!   `[ws] handlers framework=phoenix n=1 in lib/app_web/channels/user_socket.ex`
//!   `[ws] clients framework=actioncable n=1 in chat.js`
//!
//! The call-argument reader here ([`call_region`], [`split_top`]) is a local
//! copy of the shape `queue_topic.rs` keeps private (that file belongs to
//! another packet's file set, the rule A2.8 followed for `escape_json`).
//! Follow-up: one shared call-argument reader for both.

use std::collections::HashMap;

use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, node_kind};
use repo_graph_core::{Confidence, Node, NodeId, NodeKindId, RepoId};

use crate::anchor::{Anchor, line_of};
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
        },
    )
}

struct Side {
    rows: &'static [WsRow],
    kind: NodeKindId,
    prefix: &'static str,
    label: &'static str,
}

/// Rows in table order, sites in byte order: node order and anchor order are
/// a function of the file's text alone. The name map is only ever looked up,
/// never iterated.
fn scan(source: &str, path: &str, module_id: NodeId, repo: RepoId, side: Side) -> WsNodes {
    let mut nodes = Vec::new();
    let mut nav = CodeNav::default();
    let mut anchors = Vec::new();
    let mut by_name: HashMap<String, NodeId> = HashMap::new();
    // (framework, distinct nodes its sites read), in first-seen order.
    let mut per_framework: Vec<(&'static str, Vec<NodeId>)> = Vec::new();

    for r in side.rows {
        if !gate_holds(source, r.gate) {
            continue;
        }
        for offset in sites(source, r.needle) {
            let Some(name) =
                read_name(source, offset, r).or_else(|| fallback(r).map(str::to_string))
            else {
                continue;
            };
            let id = match by_name.get(&name) {
                Some(id) => *id,
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
                    by_name.insert(name, id);
                    id
                }
            };
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
/// with an identifier byte, at most [`MAX_HITS_PER_NEEDLE`].
fn sites(source: &str, needle: &str) -> Vec<usize> {
    if needle.is_empty() {
        return Vec::new();
    }
    let bounded = needle.as_bytes().first().is_some_and(|b| is_ident_byte(*b));
    let bytes = source.as_bytes();
    source
        .match_indices(needle)
        .map(|(i, _)| i)
        .filter(|&i| !bounded || i == 0 || bytes.get(i - 1).is_none_or(|b| !is_ident_byte(*b)))
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

/// The name one site reads, or `None` when the row reads none there.
fn read_name(source: &str, offset: usize, r: &WsRow) -> Option<String> {
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
/// read. The argument is split into top-level `+` pieces: a string literal is
/// static text, a template literal is static text with each `${...}` span a
/// dynamic piece, anything else is dynamic. With a dynamic piece, the text
/// after the LAST one is the path when it starts with `/`; otherwise a static
/// head that is a full `scheme://host/path` URL is. The scheme + authority are
/// stripped and a query / fragment is dropped; only a result starting with
/// `/` is accepted.
///
/// `"ws://" + location.host + "/ws/chat"` -> `/ws/chat`,
/// `` `${base}/ws/admin` `` -> `/ws/admin`,
/// `"wss://api.example.com/echo"` -> `/echo`, `url` -> `None`.
fn client_path(arg: &str) -> Option<String> {
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
                PathRead::Arg(_) | PathRead::KeyedOrArg(_) | PathRead::Client(_) => {
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

    #[test]
    fn normalise_strips_every_scheme() {
        assert_eq!(normalise_ws_path("ws://h:1/a"), "/a");
        assert_eq!(normalise_ws_path("wss://h/a/b"), "/a/b");
        assert_eq!(normalise_ws_path("http://h/hubs/chat"), "/hubs/chat");
        assert_eq!(normalise_ws_path("https://h"), "");
        assert_eq!(normalise_ws_path("/rel"), "/rel");
    }
}
