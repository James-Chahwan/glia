//! OpenAPI annotations on handlers become contract ops (LA.15a, programme
//! A10.11, the handler-attached half).
//!
//! springdoc / springfox (`@Operation`, `@ApiOperation`, `@ApiResponse`),
//! Swashbuckle (`[SwaggerOperation]`, `[SwaggerResponse]`), ASP.NET's own
//! `[ProducesResponseType]` and `@nestjs/swagger` (`@ApiOperation`,
//! `@ApiResponse`, `@ApiOkResponse`, ...) declare an API operation in code, on
//! the handler that implements it. Each such block becomes the A10.1 contract
//! op — DOC_SECTION + CODE / POSITION / ORIGIN{provenance:contract}, built by
//! [`push_op`] — so the engine's `link_contract_routes` pairs it with its ROUTE
//! unchanged and `glia docs-for` answers with it.
//!
//! Method + path are never guessed. Java / C# read them off the ROUTE the
//! annotated METHOD is `HANDLED_BY` (the language parser already composed that
//! path, and a METHOD span starts at its first annotation); NestJS composes
//! them with ts_routes' own decorator helpers, because its routes carry no
//! handler edge. A block with no key emits nothing and counts `unkeyed`.
//!
//! Parsers EXTRACT: this reads one file's text plus its own `FileParse`.

use repo_graph_code_domain::{FileParse, cell_type, edge_category, node_kind};
use repo_graph_core::{CellPayload, NodeId, RepoId};

use crate::anchor;
use crate::contracts::{ContractNodes, METHODS, esc, file_stem, operation_id_field, push_op};
use crate::ts_routes::{combine_nest_paths, extract_decorator_string};

const SPRINGDOC: &str = "springdoc";
const SPRINGFOX: &str = "springfox";
const SWASHBUCKLE: &str = "swashbuckle";
const APIEXPLORER: &str = "aspnet-apiexplorer";
const NESTJS: &str = "nestjs";

/// `@nestjs/swagger` status shorthands.
const NEST_SHORTHANDS: &[(&str, &str)] = &[
    ("ApiOkResponse", "200"),
    ("ApiCreatedResponse", "201"),
    ("ApiAcceptedResponse", "202"),
    ("ApiNoContentResponse", "204"),
    ("ApiBadRequestResponse", "400"),
    ("ApiUnauthorizedResponse", "401"),
    ("ApiForbiddenResponse", "403"),
    ("ApiNotFoundResponse", "404"),
    ("ApiConflictResponse", "409"),
    ("ApiUnprocessableEntityResponse", "422"),
    ("ApiInternalServerErrorResponse", "500"),
];

/// NestJS `HttpStatus.<NAME>` values a `status:` may name.
const HTTP_STATUS: &[(&str, &str)] = &[
    ("OK", "200"),
    ("CREATED", "201"),
    ("ACCEPTED", "202"),
    ("NO_CONTENT", "204"),
    ("BAD_REQUEST", "400"),
    ("UNAUTHORIZED", "401"),
    ("FORBIDDEN", "403"),
    ("NOT_FOUND", "404"),
    ("CONFLICT", "409"),
    ("UNPROCESSABLE_ENTITY", "422"),
    ("INTERNAL_SERVER_ERROR", "500"),
];

/// NestJS verb decorators; `@All(` is deliberately absent (no verb, no key).
const NEST_VERBS: &[(&str, &str)] = &[
    ("Get", "GET"),
    ("Post", "POST"),
    ("Put", "PUT"),
    ("Patch", "PATCH"),
    ("Delete", "DELETE"),
    ("Head", "HEAD"),
    ("Options", "OPTIONS"),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Syntax {
    Java,
    CSharp,
    Ts,
}

/// Which framework imports the file carries. An annotation name alone never
/// selects a framework: springfox and NestJS both spell `@ApiOperation`.
#[derive(Clone, Copy)]
struct Gates {
    springdoc: bool,
    springfox: bool,
    swashbuckle: bool,
    mvc: bool,
    nest: bool,
}

/// What one block (or several merged blocks) declares about an operation.
#[derive(Clone, Default)]
struct Decl {
    summary: Option<String>,
    operation_id: Option<String>,
    responses: Vec<String>,
    types: Vec<(String, String)>,
    hidden: bool,
}

impl Decl {
    fn response(&mut self, status: Option<String>, ty: Option<String>) {
        let Some(s) = status else { return };
        if !self.responses.contains(&s) {
            self.responses.push(s.clone());
        }
        if let Some(t) = ty.filter(|t| !t.is_empty())
            && !self.types.iter().any(|(k, _)| *k == s)
        {
            self.types.push((s, t));
        }
    }

    fn absorb(&mut self, other: &Decl) {
        self.summary = self.summary.take().or_else(|| other.summary.clone());
        self.operation_id = self
            .operation_id
            .take()
            .or_else(|| other.operation_id.clone());
        for s in &other.responses {
            let ty = other
                .types
                .iter()
                .find(|(k, _)| k == s)
                .map(|(_, t)| t.clone());
            self.response(Some(s.clone()), ty);
        }
    }
}

struct Pending {
    method: String,
    path: String,
    line: u32,
    source: &'static str,
    decl: Decl,
}

/// Per-framework counters behind the `[openapi-annot]` marker, in first-seen
/// order (a Vec, never a map: the marker order must not depend on hashing).
#[derive(Debug, PartialEq, Eq)]
struct FwStat {
    framework: &'static str,
    ops: usize,
    unkeyed: usize,
}

/// Emit one contract op per (method, path) an annotated handler declares.
/// Empty unless `lang` is java / csharp / a TypeScript flavour AND the file
/// imports a supported OpenAPI annotation package.
pub fn extract_annotated_ops(
    source: &str,
    path: &str,
    lang: &str,
    fp: &FileParse,
    module_id: NodeId,
    repo: RepoId,
) -> ContractNodes {
    let (out, stats) = scan(source, path, lang, fp, module_id, repo);
    // fired_on marker: `... 2>&1 | grep '^\[openapi-annot\] framework='`
    for s in &stats {
        eprintln!(
            "[openapi-annot] framework={} ops={} unkeyed={} in {path}",
            s.framework, s.ops, s.unkeyed
        );
    }
    out
}

fn scan(
    source: &str,
    path: &str,
    lang: &str,
    fp: &FileParse,
    module_id: NodeId,
    repo: RepoId,
) -> (ContractNodes, Vec<FwStat>) {
    let mut out = ContractNodes::default();
    let syn = match lang {
        "java" => Syntax::Java,
        "csharp" => Syntax::CSharp,
        "typescript" | "js" | "react" | "angular" => Syntax::Ts,
        _ => return (out, Vec::new()),
    };
    let gates = Gates {
        springdoc: syn == Syntax::Java && source.contains("io.swagger.v3.oas.annotations"),
        springfox: syn == Syntax::Java && source.contains("io.swagger.annotations"),
        swashbuckle: syn == Syntax::CSharp && source.contains("Swashbuckle.AspNetCore.Annotations"),
        mvc: syn == Syntax::CSharp && source.contains("Microsoft.AspNetCore.Mvc"),
        nest: syn == Syntax::Ts && source.contains("@nestjs/swagger"),
    };
    if !(gates.springdoc || gates.springfox || gates.swashbuckle || gates.mvc || gates.nest) {
        return (out, Vec::new());
    }

    let owners = (syn != Syntax::Ts).then(|| anchor::build_owner_index(&fp.nodes, &fp.nav));
    let mut stats: Vec<FwStat> = Vec::new();
    let mut pending: Vec<Pending> = Vec::new();
    let mut nest_prefix = String::new();
    for b in blocks(source, syn) {
        let text = source.get(b.start..b.end).unwrap_or("");
        let annots = annotations(text, syn, false);
        if let Some(c) = annots.iter().find(|a| a.name == "Controller") {
            nest_prefix = extract_decorator_string(c.whole).unwrap_or_default();
        }
        let Some((framework, decl)) = read_block(&annots, syn, gates) else {
            continue;
        };
        if decl.hidden {
            continue;
        }
        let keys = match &owners {
            Some(idx) => anchor::owner_of_line(idx, b.line)
                .map(|owner| handled_routes(fp, owner))
                .unwrap_or_default(),
            None => nest_keys(&annots, &nest_prefix),
        };
        let stat = match stats.iter().position(|s| s.framework == framework) {
            Some(i) => &mut stats[i],
            None => {
                stats.push(FwStat {
                    framework,
                    ops: 0,
                    unkeyed: 0,
                });
                let last = stats.len() - 1;
                &mut stats[last]
            }
        };
        if keys.is_empty() {
            stat.unkeyed += 1;
            continue;
        }
        for (method, route_path) in keys {
            match pending
                .iter_mut()
                .find(|p| p.method == method && p.path == route_path)
            {
                Some(p) => p.decl.absorb(&decl),
                None => {
                    stat.ops += 1;
                    pending.push(Pending {
                        method,
                        path: route_path,
                        line: b.line,
                        source: framework,
                        decl: decl.clone(),
                    });
                }
            }
        }
    }

    let stem = file_stem(path);
    for p in pending {
        let qname = format!("contract::{stem}::{}:{}", p.method, p.path);
        let name = format!("{} {}", p.method, p.path);
        let origin = origin_json(&p);
        let oid = p.decl.operation_id.as_deref();
        push_op(
            &mut out, &qname, &name, oid, path, p.line, origin, module_id, repo,
        );
    }
    (out, stats)
}

/// The ORIGIN payload, keys in the documented order; optional keys are
/// omitted when empty, never null.
fn origin_json(p: &Pending) -> String {
    let (m, path) = (esc(&p.method), esc(&p.path));
    let mut o = format!(
        r#"{{"provenance":"contract","source":"{}","method":"{m}","path":"{path}","raw_path":"{path}"{}"#,
        p.source,
        operation_id_field(p.decl.operation_id.as_deref())
    );
    if let Some(s) = &p.decl.summary {
        o.push_str(&format!(r#","summary":"{}""#, esc(s)));
    }
    if !p.decl.responses.is_empty() {
        let list: Vec<String> = p
            .decl
            .responses
            .iter()
            .map(|s| format!("\"{}\"", esc(s)))
            .collect();
        o.push_str(&format!(r#","responses":[{}]"#, list.join(",")));
    }
    if !p.decl.types.is_empty() {
        let map: Vec<String> = p
            .decl
            .types
            .iter()
            .map(|(s, t)| format!(r#""{}":"{}""#, esc(s), esc(t)))
            .collect();
        o.push_str(&format!(r#","response_types":{{{}}}"#, map.join(",")));
    }
    o.push('}');
    o
}

/// Java / C#: every (verb, path) of a ROUTE `HANDLED_BY` `owner`, in
/// `fp.edges` order. `<METHOD> <path>` qnames split on the space; a
/// `route:<path>` ROUTE takes its verbs from its ROUTE_METHOD cells. A
/// non-verb method (`ANY`) is not a key.
///
/// Precision guard: a verb route whose path is a strict prefix of an `ANY`
/// route on the SAME handler is not keyed. That pair is how the C# parser
/// reads `[HttpGet]` + `[Route("x")]` on one action (`GET <prefix>` and
/// `ANY <prefix>/x`), where ASP.NET serves `GET <prefix>/x`; keying on
/// `GET <prefix>` would document a sibling action's route. The block counts
/// `unkeyed` instead, and keys by itself once the parser composes the pair.
fn handled_routes(fp: &FileParse, owner: NodeId) -> Vec<(String, String)> {
    let mut all: Vec<(String, String)> = Vec::new();
    for e in &fp.edges {
        if e.category != edge_category::HANDLED_BY
            || e.to != owner
            || fp.nav.kind_by_id.get(&e.from) != Some(&node_kind::ROUTE)
        {
            continue;
        }
        let Some(q) = fp.nav.qname_by_id.get(&e.from) else {
            continue;
        };
        match q.strip_prefix("route:") {
            Some(p) => all.extend(
                route_methods(fp, e.from)
                    .into_iter()
                    .map(|m| (m, p.to_string())),
            ),
            None => all.extend(
                q.split_once(' ')
                    .map(|(m, p)| (m.to_ascii_uppercase(), p.to_string())),
            ),
        }
    }
    let split = |path: &str| {
        all.iter().any(|(m, p)| {
            m == "ANY"
                && p.len() > path.len()
                && p.starts_with(path)
                && (path.ends_with('/') || p.get(path.len()..).is_some_and(|r| r.starts_with('/')))
        })
    };
    let mut keys: Vec<(String, String)> = Vec::new();
    for key in &all {
        let verb = METHODS.contains(&key.0.to_ascii_lowercase().as_str());
        if verb && !key.1.is_empty() && !split(&key.1) && !keys.contains(key) {
            keys.push(key.clone());
        }
    }
    keys
}

/// The verbs on a ROUTE's ROUTE_METHOD cells: a bare verb, or JSON `{method}`.
fn route_methods(fp: &FileParse, route: NodeId) -> Vec<String> {
    let cells = fp
        .nodes
        .iter()
        .filter(|n| n.id == route)
        .flat_map(|n| n.cells.iter());
    cells
        .filter(|c| c.kind == cell_type::ROUTE_METHOD)
        .filter_map(|c| match &c.payload {
            CellPayload::Text(s) => Some(s.trim().to_ascii_uppercase()),
            CellPayload::Json(j) => serde_json::from_str::<serde_json::Value>(j)
                .ok()?
                .get("method")?
                .as_str()
                .map(str::to_ascii_uppercase),
            CellPayload::Bytes(_) => None,
        })
        .collect()
}

/// NestJS: the block's own verb decorators joined onto the nearest preceding
/// `@Controller(` prefix, by the helpers `nestjs_routes` composes with.
fn nest_keys(annots: &[Annot<'_>], prefix: &str) -> Vec<(String, String)> {
    let mut keys: Vec<(String, String)> = Vec::new();
    for a in annots {
        let Some((_, verb)) = NEST_VERBS.iter().find(|(d, _)| *d == a.name) else {
            continue;
        };
        let suffix = extract_decorator_string(a.whole).unwrap_or_default();
        let key = (verb.to_string(), combine_nest_paths(prefix, &suffix));
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    keys
}

/// The framework a block belongs to and what it declares, or `None` when it
/// carries no gated op annotation.
fn read_block(annots: &[Annot<'_>], syn: Syntax, g: Gates) -> Option<(&'static str, Decl)> {
    let has = |n: &str| annots.iter().any(|a| a.name == n);
    let mut d = Decl::default();
    match syn {
        Syntax::Java => {
            let fw = if g.springdoc && has("Operation") {
                SPRINGDOC
            } else if g.springfox && has("ApiOperation") {
                SPRINGFOX
            } else if has("ApiResponse") || has("ApiResponses") {
                let doc_shaped = annots.iter().any(|a| a.args.contains("responseCode"));
                if g.springdoc && (!g.springfox || doc_shaped) {
                    SPRINGDOC
                } else if g.springfox {
                    SPRINGFOX
                } else {
                    return None;
                }
            } else {
                return None;
            };
            for a in annots {
                java_annot(a, fw, &mut d);
            }
            Some((fw, d))
        }
        Syntax::CSharp => {
            let fw = if g.swashbuckle && (has("SwaggerOperation") || has("SwaggerResponse")) {
                SWASHBUCKLE
            } else if g.mvc && has("ProducesResponseType") {
                APIEXPLORER
            } else {
                return None;
            };
            for a in annots {
                csharp_attr(a, &mut d);
            }
            Some((fw, d))
        }
        Syntax::Ts => {
            let op = |a: &Annot<'_>| {
                matches!(a.name, "ApiOperation" | "ApiResponse")
                    || NEST_SHORTHANDS.iter().any(|(n, _)| *n == a.name)
            };
            if !(g.nest && annots.iter().any(op)) {
                return None;
            }
            for a in annots {
                nest_decorator(a, &mut d);
            }
            Some((NESTJS, d))
        }
    }
}

fn java_annot(a: &Annot<'_>, fw: &str, d: &mut Decl) {
    let t = tokens(a.args);
    let fox = fw == SPRINGFOX;
    match a.name {
        "Operation" if !fox => {
            d.summary = d.summary.take().or_else(|| named_str(&t, "summary", 0));
            d.operation_id = d
                .operation_id
                .take()
                .or_else(|| named_str(&t, "operationId", 0));
            d.hidden |= named_word(&t, "hidden", 0) == Some("true");
            // `@Operation(responses = { @ApiResponse(...) })`
            for n in annotations(a.args, Syntax::Java, true) {
                java_annot(&n, fw, d);
            }
        }
        "ApiOperation" if fox => {
            let positional = match t.first() {
                Some((0, Tok::Str(s))) if !s.is_empty() => Some(s.clone()),
                _ => None,
            };
            d.summary = d
                .summary
                .take()
                .or_else(|| named_str(&t, "value", 0).or(positional));
            d.operation_id = d
                .operation_id
                .take()
                .or_else(|| named_str(&t, "nickname", 0));
            d.hidden |= named_word(&t, "hidden", 0) == Some("true");
        }
        "ApiResponse" => {
            let (status_key, type_key) = if fox {
                ("code", "response")
            } else {
                ("responseCode", "implementation")
            };
            let status = named(&t, status_key, Some(0)).and_then(|i| status_of(&t[i].1));
            let ty = named(&t, type_key, if fox { Some(0) } else { None })
                .and_then(|i| word(&t[i].1))
                .map(|w| w.trim_end_matches(".class").to_string());
            d.response(status, ty);
        }
        "ApiResponses" => {
            for n in annotations(a.args, Syntax::Java, true) {
                java_annot(&n, fw, d);
            }
        }
        "Hidden" if !fox => d.hidden = true,
        _ => {}
    }
}

fn csharp_attr(a: &Annot<'_>, d: &mut Decl) {
    let t = tokens(a.args);
    match a.name {
        "SwaggerOperation" => {
            let positional = match t.first() {
                Some((0, Tok::Str(s))) if !s.is_empty() => Some(s.clone()),
                _ => None,
            };
            d.summary = d
                .summary
                .take()
                .or_else(|| named_str(&t, "Summary", 0).or(positional));
            d.operation_id = d
                .operation_id
                .take()
                .or_else(|| named_str(&t, "OperationId", 0));
        }
        "SwaggerResponse" | "ProducesResponseType" => {
            let mut status = t
                .iter()
                .filter(|(dp, _)| *dp == 0)
                .find_map(|(_, tk)| status_of(tk));
            let ty =
                typeof_arg(a.args).or((!a.generic.trim().is_empty()).then_some(a.generic.trim()));
            // `ProducesResponseType(typeof(T))` alone is ASP.NET's 200.
            let only_type = t
                .iter()
                .filter(|(dp, _)| *dp == 0)
                .all(|(_, tk)| matches!(tk, Tok::Word("typeof") | Tok::P(b'(' | b')')));
            if status.is_none() && only_type && ty.is_some() && a.name == "ProducesResponseType" {
                status = Some("200".to_string());
            }
            d.response(status, ty.map(str::to_string));
        }
        "ApiExplorerSettings" => d.hidden |= named_word(&t, "IgnoreApi", 0) == Some("true"),
        _ => {}
    }
}

fn nest_decorator(a: &Annot<'_>, d: &mut Decl) {
    let t = tokens(a.args);
    let ty = || {
        let i = named(&t, "type", Some(1))?;
        match (&t[i].1, t.get(i + 1).map(|x| &x.1)) {
            (Tok::Word(w), _) => Some(w.to_string()),
            (Tok::P(b'['), Some(Tok::Word(w))) => Some(format!("{w}[]")),
            _ => None,
        }
    };
    match a.name {
        "ApiOperation" => {
            d.summary = d.summary.take().or_else(|| named_str(&t, "summary", 1));
            d.operation_id = d
                .operation_id
                .take()
                .or_else(|| named_str(&t, "operationId", 1));
        }
        "ApiResponse" => {
            let status = named(&t, "status", Some(1)).and_then(|i| status_of(&t[i].1));
            d.response(status, ty());
        }
        "ApiExcludeEndpoint" => d.hidden = true,
        n => {
            if let Some((_, code)) = NEST_SHORTHANDS.iter().find(|(s, _)| *s == n) {
                d.response(Some(code.to_string()), ty());
            }
        }
    }
}

/// `404`, `"404"`, `"default"`, `"4XX"`, `StatusCodes.Status404NotFound`,
/// `HttpStatus.NOT_FOUND`: the status a token names, if any.
fn status_of(t: &Tok<'_>) -> Option<String> {
    let raw = match t {
        Tok::Str(s) => {
            let b = s.as_bytes();
            if s == "default"
                || (b.len() == 3 && (b'1'..=b'5').contains(&b[0]) && s.get(1..) == Some("XX"))
            {
                return Some(s.clone());
            }
            s.as_str()
        }
        Tok::Word(w) => w,
        Tok::P(_) => return None,
    };
    let last = raw.rsplit('.').next().unwrap_or(raw);
    let code = match last.strip_prefix("Status") {
        Some(rest) => rest.get(..3)?,
        None if raw.starts_with("HttpStatus.") => {
            return HTTP_STATUS
                .iter()
                .find(|(n, _)| *n == last)
                .map(|(_, c)| c.to_string());
        }
        None => last,
    };
    let n: u16 = code.parse().ok()?;
    ((100..=599).contains(&n) && code.len() == 3).then(|| code.to_string())
}

/// `T` of the first `typeof(T)` in an attribute's arguments.
fn typeof_arg(args: &str) -> Option<&str> {
    let at = args.find("typeof")?;
    let b = args.as_bytes();
    let open = skip_ws(b, at + "typeof".len());
    if b.get(open) != Some(&b'(') {
        return None;
    }
    let close = close_of(b, open)?;
    args.get(open + 1..close)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

// ----------------------------------------------------------------------
// Line-shaped lexing: blocks, annotations, argument tokens
// ----------------------------------------------------------------------

/// A maximal run of annotation lines: 0-indexed first line + byte range.
struct Block {
    line: u32,
    start: usize,
    end: usize,
}

/// Runs of lines whose trimmed text starts with `@` (Java, TS) or `[` (C#),
/// continued while a paren / bracket opened in the run is still open. Blank
/// and `//` lines inside a run neither extend nor end it.
fn blocks(source: &str, syn: Syntax) -> Vec<Block> {
    let lead = if syn == Syntax::CSharp { '[' } else { '@' };
    let mut out = Vec::new();
    let mut cur: Option<Block> = None;
    let mut depth = 0i32;
    let mut off = 0usize;
    for (n, raw) in source.split_inclusive('\n').enumerate() {
        let start = off;
        off += raw.len();
        let t = raw.trim();
        if depth > 0 || t.starts_with(lead) {
            let b = cur.get_or_insert(Block {
                line: n as u32,
                start,
                end: off,
            });
            b.end = off;
            depth = line_depth(raw.as_bytes(), depth);
        } else if cur.is_some() && (t.is_empty() || t.starts_with("//")) {
            continue;
        } else if let Some(b) = cur.take() {
            out.push(b);
        }
    }
    out.extend(cur);
    out
}

fn line_depth(b: &[u8], mut depth: i32) -> i32 {
    let mut i = 0;
    while i < b.len() {
        if let Some(j) = skip_lexeme(b, i) {
            i = j;
            continue;
        }
        match b[i] {
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth = (depth - 1).max(0),
            _ => {}
        }
        i += 1;
    }
    depth
}

/// One annotation / attribute / decorator: bare name (last dotted segment),
/// the text between its outer parens, a C# generic argument, and the whole
/// source slice from its sigil through its closing paren.
struct Annot<'a> {
    name: &'a str,
    args: &'a str,
    generic: &'a str,
    whole: &'a str,
}

fn is_ident(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'_' | b'$' | b'.')
}

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while b.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    i
}

/// End (exclusive) of the string literal or comment starting at `i`.
fn skip_lexeme(b: &[u8], i: usize) -> Option<usize> {
    match (b.get(i)?, b.get(i + 1)) {
        (q @ (b'"' | b'\'' | b'`'), _) => {
            let mut j = i + 1;
            while j < b.len() {
                match b[j] {
                    b'\\' => j += 2,
                    c if c == *q => return Some(j + 1),
                    _ => j += 1,
                }
            }
            Some(b.len())
        }
        (b'/', Some(b'/')) => Some(
            b[i..]
                .iter()
                .position(|&c| c == b'\n')
                .map_or(b.len(), |p| i + p),
        ),
        (b'/', Some(b'*')) => {
            let rest = b.get(i + 2..)?;
            Some(
                rest.windows(2)
                    .position(|w| w == b"*/")
                    .map_or(b.len(), |p| i + 2 + p + 2),
            )
        }
        _ => None,
    }
}

/// Index of the bracket closing the one opened at `open`.
fn close_of(b: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut i = open;
    while i < b.len() {
        if let Some(j) = skip_lexeme(b, i) {
            i = j;
            continue;
        }
        match b[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The annotations of a block, in source order. Java / TS: `@Name(...)` at
/// nesting depth 0 (any depth when `nested`, for `@ApiResponses({ @ApiResponse
/// })`); C#: a name right after `[` or `,` inside an attribute list. An
/// annotation's own arguments are skipped, so a nested one is never read twice.
fn annotations(text: &str, syn: Syntax, nested: bool) -> Vec<Annot<'_>> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let (mut depth, mut last, mut i) = (0i32, b'\n', 0usize);
    while i < b.len() {
        if let Some(j) = skip_lexeme(b, i) {
            i = j;
            last = b'"';
            continue;
        }
        let c = b[i];
        let head = match syn {
            Syntax::CSharp => depth == 1 && matches!(last, b'[' | b',') && c.is_ascii_alphabetic(),
            _ => c == b'@' && (depth == 0 || nested),
        };
        if !head {
            match c {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth = (depth - 1).max(0),
                _ => {}
            }
            if !c.is_ascii_whitespace() {
                last = c;
            }
            i += 1;
            continue;
        }
        let from = if c == b'@' { i + 1 } else { i };
        let mut k = from;
        while b.get(k).copied().is_some_and(is_ident) {
            k += 1;
        }
        let full = text.get(from..k).unwrap_or("");
        let name = full.rsplit('.').next().unwrap_or(full);
        let mut p = skip_ws(b, k);
        let mut generic = "";
        if b.get(p) == Some(&b'<') {
            let (mut q, mut angle) = (p, 0i32);
            while q < b.len() {
                match b[q] {
                    b'<' => angle += 1,
                    b'>' => angle -= 1,
                    _ => {}
                }
                if angle == 0 {
                    break;
                }
                q += 1;
            }
            if q < b.len() {
                generic = text.get(p + 1..q).unwrap_or("");
                p = skip_ws(b, q + 1);
            }
        }
        let (args, end) = match (b.get(p), close_of(b, p)) {
            (Some(b'('), Some(cl)) => (text.get(p + 1..cl).unwrap_or(""), cl + 1),
            (Some(b'('), None) => (text.get(p + 1..).unwrap_or(""), b.len()),
            _ => ("", k),
        };
        if !name.is_empty() {
            out.push(Annot {
                name,
                args,
                generic,
                whole: text.get(i..end).unwrap_or(""),
            });
        }
        i = end.max(i + 1);
        last = b')';
    }
    out
}

#[derive(Debug, PartialEq)]
enum Tok<'a> {
    Str(String),
    Word(&'a str),
    P(u8),
}

/// Argument tokens with their bracket depth (0 = the annotation's own list).
fn tokens(args: &str) -> Vec<(i32, Tok<'_>)> {
    let b = args.as_bytes();
    let mut out = Vec::new();
    let (mut depth, mut i) = (0i32, 0usize);
    while i < b.len() {
        let c = b[i];
        if let Some(end) = skip_lexeme(b, i) {
            if matches!(c, b'"' | b'\'' | b'`') {
                let inner = args.get(i + 1..end.saturating_sub(1)).unwrap_or("");
                out.push((depth, Tok::Str(unescape(inner))));
            }
            i = end;
            continue;
        }
        if is_ident(c) {
            let s = i;
            while b.get(i).copied().is_some_and(is_ident) {
                i += 1;
            }
            out.push((depth, Tok::Word(args.get(s..i).unwrap_or(""))));
            continue;
        }
        match c {
            b'(' | b'[' | b'{' => {
                out.push((depth, Tok::P(c)));
                depth += 1;
            }
            b')' | b']' | b'}' => {
                depth -= 1;
                out.push((depth, Tok::P(c)));
            }
            _ if !c.is_ascii_whitespace() => out.push((depth, Tok::P(c))),
            _ => {}
        }
        i += 1;
    }
    out
}

/// Literal text with `\x` escapes collapsed and control characters blanked, so
/// `esc` can put it in a JSON string.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        let c = match (c, c == '\\') {
            (_, true) => match chars.next() {
                Some('n' | 'r' | 't') => ' ',
                Some(e) => e,
                None => '\\',
            },
            (c, false) => c,
        };
        out.push(if c.is_control() { ' ' } else { c });
    }
    out
}

/// Index of the value token of `key = v` / `key: v` (at `depth`, or any depth).
fn named(t: &[(i32, Tok<'_>)], key: &str, depth: Option<i32>) -> Option<usize> {
    t.windows(3)
        .position(|w| {
            matches!(&w[0].1, Tok::Word(k) if *k == key)
                && depth.is_none_or(|d| w[0].0 == d)
                && matches!(w[1].1, Tok::P(b'=' | b':'))
        })
        .map(|i| i + 2)
}

fn named_str(t: &[(i32, Tok<'_>)], key: &str, depth: i32) -> Option<String> {
    match &t[named(t, key, Some(depth))?].1 {
        Tok::Str(s) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

fn named_word<'a>(t: &[(i32, Tok<'a>)], key: &str, depth: i32) -> Option<&'a str> {
    word(&t[named(t, key, Some(depth))?].1)
}

fn word<'a>(t: &Tok<'a>) -> Option<&'a str> {
    match t {
        Tok::Word(w) => Some(w),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use repo_graph_code_domain::GRAPH_TYPE;
    use repo_graph_core::{Cell, Confidence, Edge, Node};

    const REPO: RepoId = RepoId(1);

    fn module() -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, REPO, node_kind::MODULE, "m")
    }

    fn method(fp: &mut FileParse, name: &str, start: u32, end: u32) -> NodeId {
        let id = NodeId::from_parts(GRAPH_TYPE, REPO, node_kind::METHOD, name);
        let pos = format!(r#"{{"file":"f","start_line":{start},"end_line":{end}}}"#);
        fp.nodes.push(Node {
            id,
            repo: REPO,
            confidence: Confidence::Strong,
            cells: vec![Cell {
                kind: cell_type::POSITION,
                payload: CellPayload::Json(pos),
            }],
        });
        fp.nav.record(id, name, name, node_kind::METHOD, None);
        id
    }

    /// A `<METHOD> <path>` ROUTE handled by `handler`, the Java / C# shape.
    fn route(fp: &mut FileParse, qname: &str, handler: NodeId) {
        let id = NodeId::from_parts(GRAPH_TYPE, REPO, node_kind::ROUTE, qname);
        let verb = qname.split(' ').next().unwrap_or_default().to_string();
        fp.nodes.push(Node {
            id,
            repo: REPO,
            confidence: Confidence::Strong,
            cells: vec![Cell {
                kind: cell_type::ROUTE_METHOD,
                payload: CellPayload::Text(verb),
            }],
        });
        fp.nav.record(id, qname, qname, node_kind::ROUTE, None);
        fp.edges.push(Edge {
            from: id,
            to: handler,
            category: edge_category::HANDLED_BY,
            confidence: Confidence::Strong,
        });
    }

    fn run(
        src: &str,
        path: &str,
        lang: &str,
        fp: &FileParse,
    ) -> (Vec<(String, String)>, Vec<FwStat>) {
        let (out, stats) = scan(src, path, lang, fp, module(), REPO);
        let ops = out
            .nodes
            .iter()
            .map(|n| {
                let q = out.nav.qname_by_id.get(&n.id).cloned().unwrap_or_default();
                let origin = n
                    .cells
                    .iter()
                    .find_map(|c| match &c.payload {
                        CellPayload::Json(j) if c.kind == cell_type::ORIGIN => Some(j.clone()),
                        _ => None,
                    })
                    .unwrap_or_default();
                (q, origin)
            })
            .collect();
        (ops, stats)
    }

    const SPRINGDOC: &str = r#"package com.shop;

import io.swagger.v3.oas.annotations.Operation;
import io.swagger.v3.oas.annotations.responses.ApiResponse;
import org.springframework.web.bind.annotation.*;

@RestController
@RequestMapping("/api/users")
public class UserController {
    @Operation(summary = "Get a user", operationId = "getUser")
    @ApiResponse(responseCode = "200", description = "found",
        content = @Content(schema = @Schema(implementation = UserDto.class)))
    @ApiResponse(responseCode = "404", description = "missing")
    @GetMapping("/{id}")
    public User getUser(@PathVariable String id) {
        return new User(id);
    }

    @PostMapping
    public User createUser(@RequestBody User user) {
        return user;
    }
}
"#;

    fn spring_fp() -> FileParse {
        let mut fp = FileParse::default();
        let get = method(&mut fp, "getUser", 9, 16);
        let create = method(&mut fp, "createUser", 18, 21);
        let class = NodeId::from_parts(GRAPH_TYPE, REPO, node_kind::CLASS, "UserController");
        route(&mut fp, "ANY /api/users", class);
        route(&mut fp, "GET /api/users/{id}", get);
        route(&mut fp, "POST /api/users", create);
        fp
    }

    #[test]
    fn springdoc_block_keys_on_handled_by_route() {
        let (ops, stats) = run(SPRINGDOC, "src/UserController.java", "java", &spring_fp());
        assert_eq!(
            ops,
            vec![(
                "contract::UserController::GET:/api/users/{id}".to_string(),
                r#"{"provenance":"contract","source":"springdoc","method":"GET","path":"/api/users/{id}","raw_path":"/api/users/{id}","operation_id":"getUser","summary":"Get a user","responses":["200","404"],"response_types":{"200":"UserDto"}}"#
                    .to_string()
            )]
        );
        assert_eq!(
            stats,
            vec![FwStat {
                framework: "springdoc",
                ops: 1,
                unkeyed: 0
            }]
        );
        let (out, _) = scan(
            SPRINGDOC,
            "src/UserController.java",
            "java",
            &spring_fp(),
            module(),
            REPO,
        );
        let pos = out.nodes[0]
            .cells
            .iter()
            .find(|c| c.kind == cell_type::POSITION);
        assert_eq!(
            pos.map(|c| &c.payload),
            Some(&CellPayload::Json(
                r#"{"file":"src/UserController.java","start_line":9,"end_line":9}"#.to_string()
            ))
        );
    }

    #[test]
    fn springdoc_hidden_operation_is_skipped() {
        let src = SPRINGDOC.replace(
            r#"operationId = "getUser")"#,
            r#"operationId = "getUser", hidden = true)"#,
        );
        let (ops, stats) = run(&src, "UserController.java", "java", &spring_fp());
        assert!(ops.is_empty(), "{ops:?}");
        assert!(
            stats.is_empty(),
            "a hidden op is neither emitted nor unkeyed: {stats:?}"
        );
    }

    #[test]
    fn springfox_code_attr_is_a_response() {
        let src = r#"import io.swagger.annotations.ApiOperation;
import io.swagger.annotations.ApiResponse;
import io.swagger.annotations.ApiResponses;
class UserController {
    @ApiOperation(value = "Get a user", nickname = "getUser")
    @ApiResponses({ @ApiResponse(code = 200, message = "ok", response = UserDto.class),
                    @ApiResponse(code = 404, message = "missing") })
    @GetMapping("/{id}")
    public User getUser(String id) { return null; }
}
"#;
        let mut fp = FileParse::default();
        let get = method(&mut fp, "getUser", 4, 8);
        route(&mut fp, "GET /users/{id}", get);
        let (ops, _) = run(src, "UserController.java", "java", &fp);
        assert_eq!(
            ops,
            vec![(
                "contract::UserController::GET:/users/{id}".to_string(),
                r#"{"provenance":"contract","source":"springfox","method":"GET","path":"/users/{id}","raw_path":"/users/{id}","operation_id":"getUser","summary":"Get a user","responses":["200","404"],"response_types":{"200":"UserDto"}}"#
                    .to_string()
            )]
        );
    }

    const SWASH: &str = r#"using Microsoft.AspNetCore.Mvc;
using Swashbuckle.AspNetCore.Annotations;

[ApiController]
[Route("api/users")]
public class UsersController : ControllerBase
{
    [HttpGet("{id}")]
    [SwaggerOperation(Summary = "Get a user", OperationId = "GetUser")]
    [ProducesResponseType(typeof(UserDto), 200)]
    [ProducesResponseType(StatusCodes.Status404NotFound)]
    public IActionResult Get(string id) => Ok();

    [HttpDelete("{id}")]
    public IActionResult Delete(string id) => NoContent();
}
"#;

    fn swash_fp() -> FileParse {
        let mut fp = FileParse::default();
        let get = method(&mut fp, "Get", 7, 11);
        let del = method(&mut fp, "Delete", 13, 14);
        route(&mut fp, "GET /api/users/{id}", get);
        route(&mut fp, "DELETE /api/users/{id}", del);
        fp
    }

    #[test]
    fn swashbuckle_typeof_and_statuscodes_forms() {
        let (ops, stats) = run(
            SWASH,
            "Controllers/UsersController.cs",
            "csharp",
            &swash_fp(),
        );
        assert_eq!(
            ops,
            vec![(
                "contract::UsersController::GET:/api/users/{id}".to_string(),
                r#"{"provenance":"contract","source":"swashbuckle","method":"GET","path":"/api/users/{id}","raw_path":"/api/users/{id}","operation_id":"GetUser","summary":"Get a user","responses":["200","404"],"response_types":{"200":"UserDto"}}"#
                    .to_string()
            )]
        );
        assert_eq!(
            stats,
            vec![FwStat {
                framework: "swashbuckle",
                ops: 1,
                unkeyed: 0
            }]
        );
    }

    #[test]
    fn producesresponsetype_alone_is_aspnet_apiexplorer() {
        // Lines are blanked, not removed, so the method spans still hold; the
        // `//` line inside the attribute run must not end it.
        let src = SWASH
            .replace("using Swashbuckle.AspNetCore.Annotations;", "")
            .replace(
                "[SwaggerOperation(Summary = \"Get a user\", OperationId = \"GetUser\")]",
                "// documented by the api explorer alone",
            )
            .replace("[ProducesResponseType(typeof(UserDto), 200)]", "[ProducesResponseType<UserDto>(StatusCodes.Status200OK), ProducesResponseType(typeof(Error))]");
        let (ops, stats) = run(&src, "UsersController.cs", "csharp", &swash_fp());
        assert_eq!(
            ops,
            vec![(
                "contract::UsersController::GET:/api/users/{id}".to_string(),
                r#"{"provenance":"contract","source":"aspnet-apiexplorer","method":"GET","path":"/api/users/{id}","raw_path":"/api/users/{id}","responses":["200","404"],"response_types":{"200":"UserDto"}}"#
                    .to_string()
            )]
        );
        assert_eq!(
            stats,
            vec![FwStat {
                framework: "aspnet-apiexplorer",
                ops: 1,
                unkeyed: 0
            }]
        );
    }

    #[test]
    fn nestjs_composes_controller_and_verb_decorator() {
        let src = r#"import { Controller, Get, Post } from '@nestjs/common';
import { ApiOperation, ApiResponse, ApiNotFoundResponse, ApiTags } from '@nestjs/swagger';

@ApiTags('users')
@Controller('users')
export class UsersController {
  @Get(':id')
  @ApiOperation({
    summary: 'Get a user',
    operationId: 'getUser',
  })
  @ApiResponse({ status: HttpStatus.OK, type: [UserDto] })
  @ApiNotFoundResponse({ description: 'missing' })
  findOne(@Param('id') id: string) {
    return { id };
  }

  @Post()
  create(@Body() body: object) {
    return body;
  }
}
"#;
        let (ops, stats) = run(
            src,
            "src/users.controller.ts",
            "typescript",
            &FileParse::default(),
        );
        assert_eq!(
            ops,
            vec![(
                "contract::users.controller::GET:/users/:id".to_string(),
                r#"{"provenance":"contract","source":"nestjs","method":"GET","path":"/users/:id","raw_path":"/users/:id","operation_id":"getUser","summary":"Get a user","responses":["200","404"],"response_types":{"200":"UserDto[]"}}"#
                    .to_string()
            )]
        );
        assert_eq!(
            stats,
            vec![FwStat {
                framework: "nestjs",
                ops: 1,
                unkeyed: 0
            }]
        );
    }

    #[test]
    fn unannotated_handler_emits_nothing() {
        let src = SPRINGDOC
            .lines()
            .filter(|l| {
                !l.trim_start().starts_with("@Operation")
                    && !l.contains("ApiResponse(")
                    && !l.contains("content = ")
            })
            .collect::<Vec<_>>()
            .join("\n");
        let (ops, stats) = run(&src, "UserController.java", "java", &spring_fp());
        assert!(ops.is_empty(), "{ops:?}");
        assert!(stats.is_empty(), "{stats:?}");
    }

    #[test]
    fn block_without_route_counts_unkeyed() {
        // The same annotations, but the parser found no route for the method.
        let mut fp = FileParse::default();
        method(&mut fp, "getUser", 9, 16);
        let (ops, stats) = run(SPRINGDOC, "UserController.java", "java", &fp);
        assert!(ops.is_empty(), "no key, no guessed path: {ops:?}");
        assert_eq!(
            stats,
            vec![FwStat {
                framework: "springdoc",
                ops: 0,
                unkeyed: 1
            }]
        );
    }

    #[test]
    fn gate_absent_emits_nothing() {
        // `@Operation` from an unrelated package is not springdoc.
        let src = SPRINGDOC.replace("io.swagger.v3.oas.annotations", "com.acme.audit");
        let (ops, stats) = run(&src, "UserController.java", "java", &spring_fp());
        assert!(ops.is_empty(), "{ops:?}");
        assert!(stats.is_empty(), "{stats:?}");
        // Nor is a NestJS decorator in a file that never imports @nestjs/swagger.
        let ts = "@Controller('u')\nclass C {\n  @Get()\n  @ApiOperation({ summary: 'x' })\n  f() {}\n}\n";
        assert!(
            run(ts, "c.ts", "typescript", &FileParse::default())
                .0
                .is_empty()
        );
    }

    #[test]
    fn split_verb_and_route_attributes_are_not_keyed() {
        // `[HttpGet]` + `[Route("{id}")]` reaches the graph as `GET /api/users`
        // plus `ANY /api/users/{id}` on one action: keying on the bare verb
        // route would document the list action. Unkeyed, never mis-keyed.
        let src = SWASH.replace(r#"[HttpGet("{id}")]"#, r#"[HttpGet, Route("{id}")]"#);
        let mut fp = FileParse::default();
        let get = method(&mut fp, "Get", 7, 11);
        route(&mut fp, "GET /api/users", get);
        route(&mut fp, "ANY /api/users/{id}", get);
        let (ops, stats) = run(&src, "UsersController.cs", "csharp", &fp);
        assert!(ops.is_empty(), "{ops:?}");
        assert_eq!(
            stats,
            vec![FwStat {
                framework: "swashbuckle",
                ops: 0,
                unkeyed: 1
            }]
        );
        // A verb route beside an unrelated ANY route still keys.
        let mut fp = FileParse::default();
        let get = method(&mut fp, "Get", 7, 11);
        route(&mut fp, "GET /api/users/{id}", get);
        route(&mut fp, "ANY /api/users-legacy", get);
        let (ops, _) = run(&src, "UsersController.cs", "csharp", &fp);
        assert_eq!(ops.len(), 1, "{ops:?}");
    }

    #[test]
    fn two_blocks_on_one_route_merge() {
        // A Java method handling two verbs gives one op per verb; the same
        // (method, path) declared twice keeps the first block's line.
        let mut fp = FileParse::default();
        let a = method(&mut fp, "a", 9, 16);
        let b = method(&mut fp, "b", 18, 22);
        route(&mut fp, "GET /api/users/{id}", a);
        route(&mut fp, "GET /api/users/{id}", b);
        let src = SPRINGDOC.replace(
            "    @PostMapping\n",
            "    @ApiResponse(responseCode = \"500\")\n    @PostMapping\n",
        );
        let (out, stats) = scan(&src, "UserController.java", "java", &fp, module(), REPO);
        assert_eq!(out.nodes.len(), 1);
        assert_eq!(
            stats,
            vec![FwStat {
                framework: "springdoc",
                ops: 1,
                unkeyed: 0
            }]
        );
        let origin = out.nodes[0]
            .cells
            .iter()
            .find(|c| c.kind == cell_type::ORIGIN);
        let Some(CellPayload::Json(j)) = origin.map(|c| &c.payload) else {
            panic!("no ORIGIN")
        };
        assert!(j.contains(r#""responses":["200","404","500"]"#), "{j}");
    }
}
