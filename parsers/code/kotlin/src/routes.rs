//! Pure-text route scans, ported from the Java parser (`scan_ktor_routes`,
//! `scan_webflux_routes`, `scan_javalin_routes`, `emit_ktor_route`), which ran
//! them over `.kt` source until A14.2's routing flip. Same needles, same
//! filters, same ROUTE node (Confidence::Medium, a ROUTE_METHOD cell, no
//! handler), so a Kotlin repo's route NodeIds do not move with the flip. One
//! `METHOD path` key set is shared across the three scans, so a path two of
//! them match is one node. A14.5 replaces the Ktor scan with an AST scan that
//! also binds the enclosing function as the handler.

use repo_graph_core::{Cell, CellPayload, Confidence, Node, NodeId, RepoId};

use crate::{Acc, GRAPH_TYPE, cell_type, node_kind};

/// What must follow a matched `"/path"` for the call to count as a route.
#[derive(Clone, Copy)]
enum After {
    /// Ktor DSL: `get("/path") {` — the route always opens a block.
    Block,
    /// Javalin: `app.get("/path", handler)` — a handler is the second arg,
    /// which rules out a single-arg `cache.get("/key")`.
    Comma,
    /// WebFlux functional: `.GET("/path", h::list)` — the upper-case verb is
    /// discriminator enough.
    Anything,
}

struct Scan {
    needles: [(&'static str, &'static str); 7],
    /// Ktor's needles have no receiver, so they must start a word
    /// (`forget("/x")` is not a route).
    word_start: bool,
    after: After,
}

const KTOR: Scan = Scan {
    needles: [
        ("get(\"", "GET"),
        ("post(\"", "POST"),
        ("put(\"", "PUT"),
        ("patch(\"", "PATCH"),
        ("delete(\"", "DELETE"),
        ("head(\"", "HEAD"),
        ("options(\"", "OPTIONS"),
    ],
    word_start: true,
    after: After::Block,
};

const WEBFLUX: Scan = Scan {
    needles: [
        (".GET(\"", "GET"),
        (".POST(\"", "POST"),
        (".PUT(\"", "PUT"),
        (".PATCH(\"", "PATCH"),
        (".DELETE(\"", "DELETE"),
        (".HEAD(\"", "HEAD"),
        (".OPTIONS(\"", "OPTIONS"),
    ],
    word_start: false,
    after: After::Anything,
};

const JAVALIN: Scan = Scan {
    needles: [
        (".get(\"", "GET"),
        (".post(\"", "POST"),
        (".put(\"", "PUT"),
        (".patch(\"", "PATCH"),
        (".delete(\"", "DELETE"),
        (".head(\"", "HEAD"),
        (".options(\"", "OPTIONS"),
    ],
    word_start: false,
    after: After::Comma,
};

/// Run the Ktor, WebFlux and Javalin scans over `source`, emitting one ROUTE
/// per new `METHOD path`.
pub(crate) fn scan_routes(source: &str, repo: RepoId, acc: &mut Acc) {
    for scan in [&KTOR, &WEBFLUX, &JAVALIN] {
        for (method, path) in hits(source, scan) {
            if acc.routes_seen.insert(format!("{method} {path}")) {
                emit_route(method, path, repo, acc);
            }
        }
    }
}

/// Every `(method, path)` one scan matches, in needle then source order.
fn hits<'s>(source: &'s str, scan: &Scan) -> Vec<(&'static str, &'s str)> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    for (needle, method) in scan.needles {
        let mut search_from = 0;
        while let Some(rel) = source[search_from..].find(needle) {
            let pos = search_from + rel;
            let start = pos + needle.len();
            if scan.word_start
                && pos > 0
                && (bytes[pos - 1].is_ascii_alphanumeric()
                    || bytes[pos - 1] == b'_'
                    || bytes[pos - 1] == b'.')
            {
                search_from = start;
                continue;
            }
            // The closing quote, stepping over escapes. Every byte stepped to
            // is either ASCII or a continuation byte, never a `"`, so `j`
            // stops on a char boundary.
            let mut j = start;
            while j < bytes.len() && bytes[j] != b'"' {
                j += if bytes[j] == b'\\' && j + 1 < bytes.len() { 2 } else { 1 };
            }
            if j >= bytes.len() {
                break;
            }
            let path = &source[start..j];
            search_from = j + 1;
            if !path.starts_with('/') {
                continue;
            }
            let rest = &source[j + 1..];
            let accepted = match scan.after {
                After::Block => rest
                    .find(|c: char| !c.is_whitespace() && c != ')')
                    .is_some_and(|o| rest.as_bytes()[o] == b'{'),
                After::Comma => rest.trim_start().starts_with(','),
                After::Anything => true,
            };
            if accepted {
                out.push((method, path));
            }
        }
    }
    out
}

fn emit_route(method: &str, path: &str, repo: RepoId, acc: &mut Acc) {
    let route_name = format!("{method} {path}");
    let route_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::ROUTE, &route_name);
    acc.nodes.push(Node {
        id: route_id,
        repo,
        confidence: Confidence::Medium,
        cells: vec![Cell {
            kind: cell_type::ROUTE_METHOD,
            payload: CellPayload::Text(method.to_string()),
        }],
    });
    acc.nav
        .record(route_id, &route_name, &route_name, node_kind::ROUTE, None);
}
