//! tRPC router/procedure and client-call extraction (A10.9).
//!
//! Server side: a router literal `<ident> = createTRPCRouter({ ... })` (also
//! `router({ ... })` and `<t>.router({ ... })`) declares procedures as object
//! keys whose value chain ends in `.query(` / `.mutation(` / `.subscription(`
//! at paren-depth 0 — so the multi-line `publicProcedure.input(...).query(...)`
//! chain is one entry, and an object key inside a resolver body (depth > 0) is
//! never mistaken for a procedure. Each procedure becomes an `RPC_PROCEDURE`
//! node, qname `rpc:<ns>.<proc>`.
//!
//! Namespace: the router ident with a trailing `Router` stripped and the first
//! char lowercased (`userRouter` -> `user`); `appRouter` / `rootRouter` /
//! `_app` / `router` are the root and carry no namespace. An inline nested
//! router (`user: router({ ... })`) prefixes its procedures with its key. A
//! mount `user: userRouter` re-keys `userRouter`'s procedures under
//! `<parent-ns>.user` **only when `userRouter` is defined in the same file**.
//!
//! LIMITATIONS (documented, not accidental):
//! - Cross-file mounts are not followed: the extractor sees one file, so a
//!   router mounted from another file keeps its own const-derived namespace.
//!   The T3 convention (`user: userRouter`) makes the two agree; a mount under a
//!   different key (`people: userRouter`) does not, and the pairing resolver
//!   (A10.10) sees `rpc:user.*` where the client calls `people.*`.
//! - tRPC v9's string-keyed `createRouter().query("name", {...})` is not read.
//!
//! LOCATION (LA.31): the extractor receives no path, so it does not write
//! POSITION itself. Like every other RPC-family marker it reports an
//! [`Anchor`] per site in [`TrpcNodes::anchors`], and the engine's A5.8 pass
//! ([`crate::anchor::attach`]) turns them into a one-line POSITION plus the
//! owner edge: a procedure is HANDLED_BY the function whose span holds its key
//! (a module-level router gets the module CONTAINS fallback), and the calling
//! function USES an `RPC_CALL`. A procedure anchors at its own key — a router
//! mounted in the same file anchors at its declaration, not at the mount — and
//! the key offset survives comment stripping and inline-router nesting
//! ([`Scan::text_mapped`]). Every call site is an anchor (the gRPC-client
//! density rule), so each calling function gets its own USES edge.
//!
//! Client side: a receiver root in [`CLIENT_ROOTS`] followed by 1-3 dotted
//! identifier segments and a hook in [`CLIENT_HOOKS`] is an `RPC_CALL` node,
//! qname `rpc_call:<path>`, Confidence::Medium (the root is a convention, not a
//! proof). Cache operations (`.invalidate(`, `.setData(`) are not hooks and so
//! never match. The same recogniser backs [`is_trpc_client_line`], which the
//! GraphQL operation scan uses to stop tRPC hooks minting phantom
//! `graphql_op:useQuery` nodes.

use std::collections::HashSet;

use glia_code_domain::{CodeNav, GRAPH_TYPE, node_kind};
use glia_core::{Confidence, Node, NodeId, RepoId};

use crate::anchor::{self, Anchor};

#[derive(Default)]
pub struct TrpcNodes {
    pub nodes: Vec<Node>,
    pub nav: CodeNav,
    /// Router literals that declared at least one procedure directly — the
    /// `routers=` count of the engine's `[trpc]` marker. Always 0 for calls.
    pub routers: usize,
    /// LA.31: one per procedure key / call site, for the engine's A5.8 anchor
    /// pass (POSITION + owner edge). Several anchors may name one node.
    pub anchors: Vec<Anchor>,
}

/// Cheap whole-file gate: without one of these the file declares no router.
const ROUTER_GATES: &[&str] = &["createTRPCRouter", "t.router(", "= router({", "initTRPC"];

/// A value chain ending (at depth 0) in one of these is a procedure.
const PROCEDURE_NEEDLES: &[&[u8]] = &[b".query(", b".mutation(", b".subscription("];

const CLIENT_ROOTS: &[&str] = &["trpc", "api", "client"];

const CLIENT_HOOKS: &[&str] = &[
    "useQuery",
    "useMutation",
    "useSuspenseQuery",
    "useInfiniteQuery",
    "useSubscription",
    "query",
    "mutate",
];

/// A chain through one of these is a cache/utility accessor, not a call.
const CACHE_SEGMENTS: &[&str] = &["useUtils", "useContext", "useQueries", "useSuspenseQueries"];

/// Nesting guard for inline routers and in-file mount chains.
const MAX_DEPTH: usize = 8;

pub fn extract_trpc_procedure_nodes(source: &str, module_id: NodeId, repo: RepoId) -> TrpcNodes {
    let mut out = TrpcNodes::default();
    if !ROUTER_GATES.iter().any(|g| source.contains(g)) {
        return out;
    }
    let routers = scan_routers(source);
    out.routers = routers.iter().filter(|r| !r.procedures.is_empty()).count();

    let mut seen = HashSet::new();
    for idx in 0..routers.len() {
        for prefix in router_prefixes(&routers, idx, 0) {
            for (proc_path, key_offset) in &routers[idx].procedures {
                let name = join_path(&prefix, proc_path);
                let qname = format!("rpc:{name}");
                let id = if seen.insert(qname.clone()) {
                    push_node(
                        &mut out,
                        &name,
                        &qname,
                        node_kind::RPC_PROCEDURE,
                        Confidence::Strong,
                        module_id,
                        repo,
                    )
                } else {
                    NodeId::from_parts(GRAPH_TYPE, repo, node_kind::RPC_PROCEDURE, &qname)
                };
                out.anchors.push(Anchor {
                    node: id,
                    line: anchor::line_of(source, *key_offset),
                });
            }
        }
    }
    out
}

pub fn extract_trpc_call_nodes(source: &str, module_id: NodeId, repo: RepoId) -> TrpcNodes {
    let mut out = TrpcNodes::default();
    if !CLIENT_ROOTS.iter().any(|r| source.contains(r)) {
        return out;
    }
    let mut seen = HashSet::new();
    for (path, root_offset) in scan_client_calls(source) {
        let qname = format!("rpc_call:{path}");
        let id = if seen.insert(qname.clone()) {
            push_node(
                &mut out,
                &path,
                &qname,
                node_kind::RPC_CALL,
                Confidence::Medium,
                module_id,
                repo,
            )
        } else {
            NodeId::from_parts(GRAPH_TYPE, repo, node_kind::RPC_CALL, &qname)
        };
        // Every site, not just the first: each calling function gets its USES.
        out.anchors.push(Anchor {
            node: id,
            line: anchor::line_of(source, root_offset),
        });
    }
    out
}

/// True when `line` holds a tRPC client call (`api.user.list.useQuery(`). A
/// bare `useQuery(GET_USERS)` or Apollo `client.query(` has no procedure
/// segment and is NOT a tRPC line.
pub fn is_trpc_client_line(line: &str) -> bool {
    CLIENT_ROOTS.iter().any(|r| line.contains(r)) && !scan_client_calls(line).is_empty()
}

/// Push one node (no cells: POSITION comes from the anchor pass) and return
/// its id.
fn push_node(
    out: &mut TrpcNodes,
    name: &str,
    qname: &str,
    kind: glia_core::NodeKindId,
    confidence: Confidence,
    module_id: NodeId,
    repo: RepoId,
) -> NodeId {
    let id = NodeId::from_parts(GRAPH_TYPE, repo, kind, qname);
    out.nodes.push(Node {
        id,
        repo,
        confidence,
        cells: vec![],
    });
    out.nav.record(id, name, qname, kind, Some(module_id));
    id
}

// ---------------------------------------------------------------------------
// Lexical mask: which bytes are code, string, or comment.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Class {
    Code,
    Str,
    Comment,
}

struct Scan<'a> {
    src: &'a [u8],
    cls: Vec<Class>,
}

impl<'a> Scan<'a> {
    fn new(text: &'a str) -> Self {
        let src = text.as_bytes();
        let mut cls = vec![Class::Code; src.len()];
        let mut i = 0;
        while i < src.len() {
            let (class, end) = match src[i] {
                b'/' if src.get(i + 1) == Some(&b'/') => {
                    let end = src[i..]
                        .iter()
                        .position(|&b| b == b'\n')
                        .map_or(src.len(), |p| i + p);
                    (Class::Comment, end)
                }
                b'/' if src.get(i + 1) == Some(&b'*') => {
                    let end = src[i + 2..]
                        .windows(2)
                        .position(|w| w == b"*/")
                        .map_or(src.len(), |p| i + 2 + p + 2);
                    (Class::Comment, end)
                }
                q @ (b'\'' | b'"' | b'`') => {
                    let mut j = i + 1;
                    while j < src.len() {
                        match src[j] {
                            b'\\' => j += 2,
                            b if b == q => {
                                j += 1;
                                break;
                            }
                            b'\n' if q != b'`' => break,
                            _ => j += 1,
                        }
                    }
                    (Class::Str, j.min(src.len()))
                }
                _ => {
                    i += 1;
                    continue;
                }
            };
            cls[i..end].fill(class);
            i = end.max(i + 1);
        }
        Scan { src, cls }
    }

    fn code(&self, i: usize) -> bool {
        self.cls.get(i) == Some(&Class::Code)
    }

    /// Index of the bracket closing the one at `open`, counting code bytes only.
    fn close_of(&self, open: usize) -> Option<usize> {
        let mut depth = 0usize;
        for i in open..self.src.len() {
            if !self.code(i) {
                continue;
            }
            match self.src[i] {
                b'{' | b'(' | b'[' => depth += 1,
                b'}' | b')' | b']' => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// Byte ranges of `[s, e)` split at depth-0 commas.
    fn split_top(&self, s: usize, e: usize) -> Vec<(usize, usize)> {
        let mut parts = Vec::new();
        let (mut depth, mut start) = (0usize, s);
        for i in s..e {
            if !self.code(i) {
                continue;
            }
            match self.src[i] {
                b'{' | b'(' | b'[' => depth += 1,
                b'}' | b')' | b']' => depth = depth.saturating_sub(1),
                b',' if depth == 0 => {
                    parts.push((start, i));
                    start = i + 1;
                }
                _ => {}
            }
        }
        parts.push((start, e));
        parts
    }

    /// `[s, e)` with comment bytes dropped, trimmed — plus, for every byte of
    /// the returned text, its offset in the ORIGINAL source: `map[i]` is the
    /// source offset of byte `i` of the text this `Scan` was built over. The
    /// offsets are recorded per kept byte, never recomputed from the trimmed
    /// string, so they survive comment stripping and nesting (a nested body is
    /// a slice of this text, scanned with the matching slice of the result).
    ///
    /// Comment ranges start and end on ASCII delimiters, so the kept bytes are
    /// valid UTF-8 and the result maps 1:1; should an invalid sequence ever
    /// reach here, its U+FFFD replacement maps to the sequence's first byte.
    fn text_mapped(&self, s: usize, e: usize, map: &[usize]) -> (String, Vec<usize>) {
        let kept: Vec<usize> = (s..e)
            .filter(|&i| self.cls.get(i).is_some_and(|c| *c != Class::Comment))
            .collect();
        let bytes: Vec<u8> = kept.iter().map(|&i| self.src[i]).collect();
        // `map` is built 1:1 with the scanned text; the fallback is unreachable
        // and only keeps a malformed call from panicking.
        let origin = |i: usize| map.get(i).copied().unwrap_or(i);
        let mut text = String::with_capacity(bytes.len());
        let mut offsets = Vec::with_capacity(bytes.len());
        let mut pos = 0usize;
        for chunk in bytes.utf8_chunks() {
            let valid = chunk.valid();
            text.push_str(valid);
            let run = kept.get(pos..pos + valid.len()).unwrap_or_default();
            offsets.extend(run.iter().map(|&i| origin(i)));
            pos += valid.len();
            if !chunk.invalid().is_empty() {
                text.push(char::REPLACEMENT_CHARACTER);
                let at = kept.get(pos).map_or(0, |&i| origin(i));
                offsets.extend(std::iter::repeat_n(
                    at,
                    char::REPLACEMENT_CHARACTER.len_utf8(),
                ));
                pos += chunk.invalid().len();
            }
        }
        let lead = text.len() - text.trim_start().len();
        let keep = text.trim().len();
        let trimmed = text[lead..lead + keep].to_string();
        offsets.truncate(lead + keep);
        offsets.drain(..lead);
        (trimmed, offsets)
    }

    /// Whether `needle` occurs as code at bracket depth 0.
    fn has_top(&self, needle: &[u8]) -> bool {
        let mut depth = 0usize;
        for i in 0..self.src.len() {
            if !self.code(i) {
                continue;
            }
            if depth == 0 && self.src[i..].starts_with(needle) {
                return true;
            }
            match self.src[i] {
                b'{' | b'(' | b'[' => depth += 1,
                b'}' | b')' | b']' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        false
    }
}

// ---------------------------------------------------------------------------
// Server side: routers and procedures.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct RouterDecl {
    ident: String,
    ns: String,
    /// Procedure paths relative to this router (`list`, `admin.ban`), each
    /// with the absolute source byte offset of its key.
    procedures: Vec<(String, usize)>,
    /// `(relative key path, mounted router ident)`.
    mounts: Vec<(String, String)>,
}

fn scan_routers(source: &str) -> Vec<RouterDecl> {
    let scan = Scan::new(source);
    let mut routers = Vec::new();
    let mut resume_at = 0usize;
    let mut line_start = 0usize;
    for line in source.split_inclusive('\n') {
        let ls = line_start;
        line_start += line.len();
        let trimmed = line.trim_start();
        let off = ls + (line.len() - trimmed.len());
        if ls < resume_at || !scan.code(off) {
            continue;
        }
        let Some((ident, rhs)) = parse_declaration(trimmed) else {
            continue;
        };
        let Some(rel_open) = router_call_open(rhs) else {
            continue;
        };
        let open = off + (trimmed.len() - rhs.len()) + rel_open;
        let Some(close) = scan.close_of(open) else {
            continue;
        };
        let mut decl = RouterDecl {
            ident: ident.to_string(),
            ns: router_namespace(ident),
            ..RouterDecl::default()
        };
        let map: Vec<usize> = (open + 1..close).collect();
        parse_router_body(&source[open + 1..close], &map, "", &mut decl, 0);
        routers.push(decl);
        resume_at = close;
    }
    routers
}

/// `export const userRouter = <rhs>` -> `("userRouter", "<rhs>")`.
fn parse_declaration(line: &str) -> Option<(&str, &str)> {
    let mut t = line.strip_prefix("export ").unwrap_or(line).trim_start();
    for kw in ["const ", "let ", "var "] {
        if let Some(rest) = t.strip_prefix(kw) {
            t = rest.trim_start();
            break;
        }
    }
    let ident_len = ident_prefix_len(t);
    if ident_len == 0 {
        return None;
    }
    let (ident, rest) = t.split_at(ident_len);
    let rest = rest.trim_start();
    // Optional `: Type` annotation before the `=`.
    let rest = if rest.starts_with(':') {
        &rest[rest.find('=')?..]
    } else {
        rest
    };
    let rhs = rest.strip_prefix('=')?;
    if rhs.starts_with('=') || rhs.starts_with('>') {
        return None;
    }
    Some((ident, rhs.trim_start()))
}

/// If `rhs` starts with `createTRPCRouter(`, `router(` or `<ident>.router(`
/// followed by `{`, the byte index of that `{`.
fn router_call_open(rhs: &str) -> Option<usize> {
    let head_len = ident_prefix_len(rhs);
    let after_head = &rhs[head_len..];
    let call_rest = match &rhs[..head_len] {
        "createTRPCRouter" | "router" => after_head.strip_prefix('(')?,
        "" => return None,
        _ => after_head.strip_prefix(".router(")?,
    };
    let brace = call_rest.len() - call_rest.trim_start().len();
    call_rest[brace..]
        .starts_with('{')
        .then(|| rhs.len() - call_rest.len() + brace)
}

/// `map[i]` is the absolute source offset of `body` byte `i` (see
/// [`Scan::text_mapped`]).
fn parse_router_body(body: &str, map: &[usize], prefix: &str, decl: &mut RouterDecl, depth: usize) {
    let scan = Scan::new(body);
    for (s, e) in scan.split_top(0, body.len()) {
        let (entry, emap) = scan.text_mapped(s, e, map);
        let Some((key, value)) = split_key(&entry) else {
            continue;
        };
        let path = join_path(prefix, &key);
        let vscan = Scan::new(value);
        if let Some(open) = router_call_open(value) {
            if depth < MAX_DEPTH {
                if let (Some(close), Some(vstart)) =
                    (vscan.close_of(open), offset_in(&entry, value))
                {
                    let sub = emap
                        .get(vstart + open + 1..vstart + close)
                        .unwrap_or_default();
                    parse_router_body(&value[open + 1..close], sub, &path, decl, depth + 1);
                }
            }
        } else if PROCEDURE_NEEDLES.iter().any(|n| vscan.has_top(n)) {
            // The entry is trimmed, so its first byte is the key's (or the
            // opening quote of a quoted key, on the same line).
            if let Some(&key_offset) = emap.first() {
                decl.procedures.push((path, key_offset));
            }
        } else if ident_prefix_len(value) == value.len() && !value.is_empty() {
            decl.mounts.push((path, value.to_string()));
        }
    }
}

/// Byte offset of `inner` within `outer` when `inner` is a subslice of it.
fn offset_in(outer: &str, inner: &str) -> Option<usize> {
    let base = outer.as_ptr() as usize;
    let at = inner.as_ptr() as usize;
    (at >= base && at + inner.len() <= base + outer.len()).then(|| at - base)
}

/// `list: publicProcedure...` -> `("list", "publicProcedure...")`; a shorthand
/// `userRouter` -> `("userRouter", "userRouter")`. Spreads, computed keys and
/// method shorthands yield `None`.
fn split_key(entry: &str) -> Option<(String, &str)> {
    let t = entry.trim();
    if let q @ (b'"' | b'\'') = *t.as_bytes().first()? {
        let close = t[1..].find(q as char)? + 1;
        let value = t[close + 1..].trim_start().strip_prefix(':')?.trim();
        return Some((t[1..close].to_string(), value));
    }
    let n = ident_prefix_len(t);
    if n == 0 {
        return None;
    }
    let rest = t[n..].trim_start();
    if rest.is_empty() {
        return Some((t.to_string(), t)); // shorthand `{ userRouter }`
    }
    Some((t[..n].to_string(), rest.strip_prefix(':')?.trim()))
}

fn router_namespace(ident: &str) -> String {
    let base = ident.strip_suffix("Router").unwrap_or(ident);
    if matches!(base, "" | "app" | "root" | "_app" | "router") {
        return String::new();
    }
    let mut chars = base.chars();
    match chars.next() {
        Some(c) => c.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Every effective namespace of router `idx`: its own, or — when routers in
/// the same file mount it — `<parent prefix>.<mount key>` for each mount.
fn router_prefixes(routers: &[RouterDecl], idx: usize, depth: usize) -> Vec<String> {
    let own = &routers[idx];
    let mut parents: Vec<(usize, &str)> = Vec::new();
    for (i, r) in routers.iter().enumerate().filter(|(i, _)| *i != idx) {
        for (key, target) in &r.mounts {
            if *target == own.ident {
                parents.push((i, key.as_str()));
            }
        }
    }
    if parents.is_empty() || depth >= MAX_DEPTH {
        return vec![own.ns.clone()];
    }
    let mut out = Vec::new();
    for (parent, key) in parents {
        for pre in router_prefixes(routers, parent, depth + 1) {
            let full = join_path(&pre, key);
            if !out.contains(&full) {
                out.push(full);
            }
        }
    }
    out
}

fn join_path(prefix: &str, key: &str) -> String {
    if prefix.is_empty() {
        key.to_string()
    } else {
        format!("{prefix}.{key}")
    }
}

// ---------------------------------------------------------------------------
// Client side: `<root>.<seg>(.<seg>){0,2}.<hook>(`.
// ---------------------------------------------------------------------------

/// `(procedure path, byte offset of the root identifier)` per call site.
fn scan_client_calls(src: &str) -> Vec<(String, usize)> {
    let scan = Scan::new(src);
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let boundary = i == 0 || !(is_ident_byte(b[i - 1]) || b[i - 1] == b'.');
        if !boundary || !scan.code(i) || !is_ident_start(b[i]) {
            i += 1;
            continue;
        }
        let root_end = i + ident_prefix_len(&src[i..]);
        let mut j = root_end;
        if CLIENT_ROOTS.contains(&&src[i..root_end]) {
            let mut segs: Vec<&str> = Vec::new();
            while segs.len() <= 4 {
                let dot = skip_ws(b, j);
                if b.get(dot) != Some(&b'.') {
                    break;
                }
                let start = skip_ws(b, dot + 1);
                let len = ident_prefix_len(&src[start..]);
                if len == 0 {
                    break;
                }
                segs.push(&src[start..start + len]);
                j = start + len;
            }
            if b.get(skip_ws(b, j)) == Some(&b'(') && (2..=4).contains(&segs.len()) {
                if let Some((hook, path)) = segs.split_last() {
                    if CLIENT_HOOKS.contains(hook)
                        && !path.iter().any(|s| CACHE_SEGMENTS.contains(s))
                    {
                        out.push((path.join("."), i));
                    }
                }
            }
        }
        i = j.max(i + 1);
    }
    out
}

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while b.get(i).is_some_and(|c| c.is_ascii_whitespace()) {
        i += 1;
    }
    i
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c == b'$'
}

fn is_ident_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'$'
}

/// Length of the ASCII identifier at the start of `s` (0 if none).
fn ident_prefix_len(s: &str) -> usize {
    let b = s.as_bytes();
    if !b.first().is_some_and(|&c| is_ident_start(c)) {
        return 0;
    }
    b.iter().position(|&c| !is_ident_byte(c)).unwrap_or(b.len())
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
    fn qnames(out: &TrpcNodes) -> Vec<String> {
        out.nodes
            .iter()
            .filter_map(|n| out.nav.qname_by_id.get(&n.id).cloned())
            .collect()
    }

    const T3_ROUTER: &str = r#"import { z } from "zod";
import { createTRPCRouter, publicProcedure } from "~/server/api/trpc";

export const userRouter = createTRPCRouter({
  list: publicProcedure.query(() => db.user.findMany()),
  // byId looks one user up
  byId: publicProcedure
    .input(z.object({ id: z.string() }))
    .query(({ input }) => {
      return db.user.findUnique({ where: { id: input.id } });
    }),
});
"#;

    #[test]
    fn t3_router_yields_namespaced_procedures() {
        let out = extract_trpc_procedure_nodes(T3_ROUTER, module_id(), repo());
        assert_eq!(qnames(&out), vec!["rpc:user.list", "rpc:user.byId"]);
        assert_eq!(out.routers, 1);
        let names: Vec<&String> = out.nav.name_by_id.values().collect();
        assert!(names.iter().any(|n| *n == "user.list"));
        assert!(out.nodes.iter().all(|n| n.confidence == Confidence::Strong));
    }

    #[test]
    fn same_file_mount_under_matching_key_rekeys_nothing() {
        let src = format!(
            "{T3_ROUTER}\nexport const appRouter = createTRPCRouter({{ user: userRouter }});\n"
        );
        let out = extract_trpc_procedure_nodes(&src, module_id(), repo());
        assert_eq!(qnames(&out), vec!["rpc:user.list", "rpc:user.byId"]);
        assert_eq!(
            out.routers, 1,
            "appRouter only mounts; it declares no procedure"
        );
    }

    #[test]
    fn same_file_mount_under_other_key_and_inline_router_rekey() {
        let src = r#"const t = initTRPC.create();
const userRouter = t.router({ list: t.procedure.query(() => []) });
export const appRouter = t.router({
  people: userRouter,
  admin: t.router({ ban: t.procedure.mutation(() => null) }),
  health: t.procedure.query(() => "ok"),
});
"#;
        let out = extract_trpc_procedure_nodes(src, module_id(), repo());
        assert_eq!(
            qnames(&out),
            vec!["rpc:people.list", "rpc:admin.ban", "rpc:health"]
        );
    }

    #[test]
    fn file_without_router_gate_yields_nothing() {
        let src = "const x = { list: foo.query(() => 1) };";
        assert!(
            extract_trpc_procedure_nodes(src, module_id(), repo())
                .nodes
                .is_empty()
        );
    }

    #[test]
    fn client_hook_yields_rpc_call() {
        let src =
            "const { data } = api.user.list.useQuery();\nconst m = api.user.list.useQuery({});";
        let out = extract_trpc_call_nodes(src, module_id(), repo());
        assert_eq!(qnames(&out), vec!["rpc_call:user.list"]);
        assert!(out.nodes.iter().all(|n| n.confidence == Confidence::Medium));
        let vanilla =
            extract_trpc_call_nodes("await trpc.userById.query('1');", module_id(), repo());
        assert_eq!(qnames(&vanilla), vec!["rpc_call:userById"]);
    }

    #[test]
    fn cache_ops_are_not_calls() {
        for src in [
            "utils.user.list.invalidate();",
            "api.user.list.invalidate();",
            "api.user.list.setData(undefined, []);",
            "const utils = api.useUtils();",
            "// api.user.list.useQuery();",
            "const s = \"api.user.list.useQuery()\";",
            "this.api.user.list.useQuery();",
        ] {
            let out = extract_trpc_call_nodes(src, module_id(), repo());
            assert!(out.nodes.is_empty(), "{src} -> {:?}", qnames(&out));
        }
    }

    #[test]
    fn graphql_lines_are_not_trpc_lines() {
        assert!(!is_trpc_client_line(
            "const { data } = useQuery(GET_USERS);"
        ));
        assert!(!is_trpc_client_line(
            "const { data } = useQuery(GET_USER, { variables });"
        ));
        assert!(!is_trpc_client_line(
            "const res = await client.query({ query: GET_USERS });"
        ));
        assert!(!is_trpc_client_line(
            "client.mutate({ mutation: ADD_USER });"
        ));
        assert!(is_trpc_client_line(
            "const { data } = trpc.user.list.useQuery();"
        ));
        assert!(is_trpc_client_line(
            "const m = api.post.create.useMutation();"
        ));
    }

    /// `(qname, 0-indexed line)` per anchor, in emission order.
    fn anchor_rows(out: &TrpcNodes) -> Vec<(String, u32)> {
        out.anchors
            .iter()
            .map(|a| {
                let q = out
                    .nav
                    .qname_by_id
                    .get(&a.node)
                    .cloned()
                    .unwrap_or_default();
                (q, a.line)
            })
            .collect()
    }

    fn rows(pairs: &[(&str, u32)]) -> Vec<(String, u32)> {
        pairs.iter().map(|(q, l)| (q.to_string(), *l)).collect()
    }

    /// bench/substrate-gap/fixtures/xcut-trpc/server/router.ts, verbatim.
    const FIXTURE_ROUTER: &str = r#"import { z } from "zod";
import { createTRPCRouter, publicProcedure } from "./trpc";
import { db } from "./db";

export const userRouter = createTRPCRouter({
  list: publicProcedure.query(() => db.user.findMany()),
  byId: publicProcedure
    .input(z.object({ id: z.string() }))
    .query(({ input }) => {
      return db.user.findUnique({ where: { id: input.id } });
    }),
});

export const appRouter = createTRPCRouter({
  user: userRouter,
});

export type AppRouter = typeof appRouter;
"#;

    #[test]
    fn procedure_anchors_point_at_their_keys() {
        // Router at row 4: `list` on row 5, the multi-line `byId` chain's key on
        // row 6. The appRouter mount re-keys nothing and anchors nothing.
        let out = extract_trpc_procedure_nodes(FIXTURE_ROUTER, module_id(), repo());
        assert_eq!(
            anchor_rows(&out),
            rows(&[("rpc:user.list", 5), ("rpc:user.byId", 6)])
        );
        // A line comment between two entries does not shift the next key.
        let out = extract_trpc_procedure_nodes(T3_ROUTER, module_id(), repo());
        assert_eq!(
            anchor_rows(&out),
            rows(&[("rpc:user.list", 4), ("rpc:user.byId", 6)])
        );
        assert!(
            out.nodes.iter().all(|n| n.cells.is_empty()),
            "POSITION is the anchor pass's job"
        );
    }

    #[test]
    fn nested_inline_router_anchor_uses_source_rows() {
        let src = r#"const t = initTRPC.create();
export const appRouter = t.router({
  health: t.procedure.query(() => "ok"),
  admin: t.router({
    // c
    ban: t.procedure.mutation(() => null),
    /* two
       lines */ kick: t.procedure.mutation(() => null),
  }),
});
"#;
        let out = extract_trpc_procedure_nodes(src, module_id(), repo());
        assert_eq!(
            anchor_rows(&out),
            rows(&[
                ("rpc:health", 2),
                ("rpc:admin.ban", 5),
                ("rpc:admin.kick", 7)
            ])
        );
    }

    #[test]
    fn block_comment_before_a_key_keeps_the_key_row() {
        // Multi-byte text inside comments and strings must not shift offsets.
        let src = r#"export const userRouter = createTRPCRouter({
  /**
   * Lists users — café ✓.
   */
  list: publicProcedure.query(() => "naïve"),
  /* inline ✓ */ byId: publicProcedure.query(() => null),
  "quoted": publicProcedure.query(() => null),
});
"#;
        let out = extract_trpc_procedure_nodes(src, module_id(), repo());
        assert_eq!(
            anchor_rows(&out),
            rows(&[
                ("rpc:user.list", 4),
                ("rpc:user.byId", 5),
                ("rpc:user.quoted", 6)
            ])
        );
    }

    #[test]
    fn mounted_router_procedures_anchor_at_their_declaration() {
        let src = r#"const t = initTRPC.create();
const userRouter = t.router({
  list: t.procedure.query(() => []),
});
export const appRouter = t.router({
  people: userRouter,
});
"#;
        let out = extract_trpc_procedure_nodes(src, module_id(), repo());
        assert_eq!(
            anchor_rows(&out),
            rows(&[("rpc:people.list", 2)]),
            "the procedure's own key, not the `people:` mount on row 5"
        );
    }

    #[test]
    fn every_call_site_is_an_anchor() {
        let src = r#"import { api } from "~/utils/api";
export function Users() {
  const { data } = api.user.list.useQuery();
  return data;
}
export function Count() {
  const q =
    api.user.list.useQuery({ take: 1 });
  return q.data?.length;
}
"#;
        let out = extract_trpc_call_nodes(src, module_id(), repo());
        assert_eq!(
            qnames(&out),
            vec!["rpc_call:user.list"],
            "one node per path"
        );
        assert_eq!(
            anchor_rows(&out),
            rows(&[("rpc_call:user.list", 2), ("rpc_call:user.list", 7)]),
            "one anchor per site, at the root identifier's row"
        );
        let none = extract_trpc_call_nodes("// api.user.list.useQuery();", module_id(), repo());
        assert!(none.anchors.is_empty() && none.nodes.is_empty());
    }

    #[test]
    fn namespace_rules() {
        assert_eq!(router_namespace("userRouter"), "user");
        assert_eq!(router_namespace("PostRouter"), "post");
        assert_eq!(router_namespace("appRouter"), "");
        assert_eq!(router_namespace("rootRouter"), "");
        assert_eq!(router_namespace("_app"), "");
        assert_eq!(router_namespace("router"), "");
    }
}
