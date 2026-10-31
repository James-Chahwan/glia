//! GH.6: the nginx / OpenResty reader, the environment side of the host graph.
//!
//! An OpenResty server runs Lua named in its nginx configuration: a
//! `location` block whose content phase is `content_by_lua_file lua/hello.lua;`
//! serves that location from the Lua file. This module reads such a conf with a
//! brace-nesting directive tokenizer (comments, quotes, `${var}` tokens, and a
//! `*_by_lua_block { .. }` body skipped as Lua) and says what it read; it binds
//! nothing. Per Lua phase directive with a literal file argument:
//!
//! - `content_by_lua_file <f>` in a `location [=|^~] <path>` mints the ROUTE
//!   `ANY <path>` ([`endpoint::route_qname`]) and records a
//!   [`NavFact::HostScriptRef`] HANDLED_BY on it (rule `content_by_lua_file`);
//! - every other phase (`access`, `rewrite`, `header_filter`, `body_filter`,
//!   `log`, `balancer`, `set`, `server_rewrite`, `ssl_*`) records a USES ref,
//!   from the location's ROUTE when it has one, else from the conf's MODULE
//!   (rule `<phase>_by_lua_file`);
//! - `init_by_lua_file` / `init_worker_by_lua_file` / `exit_worker_by_lua_file`
//!   record a USES ref from the conf's MODULE, which gets one ENTRYPOINT cell
//!   `{"decl":"<conf>:<1-based row>","source":"host:nginx"}`: the server runs
//!   them at start, so the conf is an entry and its scripts are reached
//!   through the USES the resolver binds.
//!
//! The graph crate's `HostResolver` (resolver:host) binds each ref's path to a
//! MODULE of the same repo by file path. Not read: a regex (`~`, `~*`) or named
//! (`@x`) location mints no ROUTE (its content ref is a USES from the MODULE,
//! counted `unresolved=`); a path holding an nginx variable (`$`) records no
//! ref (`dynamic=`); an inline `*_by_lua_block` / `*_by_lua` body is counted
//! `inline=` and read no further; `include` directives are not followed; a
//! plain `location` with no Lua content phase (`proxy_pass`, static files)
//! mints nothing.
//!
//! fired_on, once per conf that names a Lua phase:
//! `[nginx] locations=<l> lua_files=<f> inline=<i> dynamic=<d> unresolved=<u> path=<p>`.

use std::collections::BTreeMap;

use glia_code_domain::{
    CodeNav, GRAPH_TYPE, NavFact, cell_type, edge_category, endpoint, node_kind,
};
use glia_core::{Cell, CellPayload, Confidence, Node, NodeId, RepoId};

use crate::anchor;

/// The host name a [`NavFact::HostScriptRef`] from this reader carries.
pub const HOST: &str = "nginx";

/// Largest `.conf` the walk reads to sniff for a Lua phase directive.
pub const CONF_CAP: u64 = 1024 * 1024;

/// The ROUTE method of a location: nginx serves every verb there.
const ANY: &str = "ANY";

/// The ENTRYPOINT `source` of a conf that runs a script at server start.
const ENTRY_SOURCE: &str = "host:nginx";

/// Phases the server runs once per worker / master, not per request: their
/// scripts are entries, reached from the conf MODULE.
const ENTRY_PHASES: [&str; 3] = ["init", "init_worker", "exit_worker"];

/// A `.conf` the walk may sniff for nginx: a `*.conf` file name (any case)
/// that is not a HOCON conf the akka-quartz graft reads
/// ([`crate::cron::is_hocon_conf_path`]).
pub fn is_conf_path(path: &str) -> bool {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let lower = base.to_ascii_lowercase();
    lower.len() > ".conf".len()
        && lower.ends_with(".conf")
        && !crate::cron::is_hocon_conf_path(path)
}

/// True when `source` holds an nginx Lua phase directive
/// (`<phase>_by_lua_file`, `<phase>_by_lua_block` or `<phase>_by_lua`) as a
/// directive name: the walk admits such a conf, and drops every other `.conf`.
pub fn sniff(source: &str) -> bool {
    if !source.contains("_by_lua") {
        return false;
    }
    fn any(dirs: &[Directive]) -> bool {
        dirs.iter()
            .any(|d| lua_phase(&d.name).is_some() || d.block.as_deref().is_some_and(any))
    }
    any(&tokenize(source))
}

/// What one conf held: the ROUTE nodes (with nav), the refs the resolver binds
/// (scope, fact) in source order, the conf MODULE's own cells (an ENTRYPOINT
/// when it runs a script at start) and the marker counts.
#[derive(Debug, Default)]
pub struct NginxOut {
    pub nodes: Vec<Node>,
    pub nav: CodeNav,
    pub facts: Vec<(NodeId, NavFact)>,
    pub module_cells: Vec<Cell>,
    pub stats: NginxStats,
}

/// The `[nginx]` marker's counts for one conf.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct NginxStats {
    /// `location` blocks read.
    pub locations: usize,
    /// `<phase>_by_lua_file` directives with an argument.
    pub lua_files: usize,
    /// `<phase>_by_lua_block` / `<phase>_by_lua` directives (inline Lua).
    pub inline: usize,
    /// Lua files whose path holds an nginx variable: no ref.
    pub dynamic: usize,
    /// Content handlers in a location no ROUTE names (regex or named).
    pub unresolved: usize,
}

impl NginxOut {
    /// The fired_on line, `None` for a conf that names no Lua phase.
    pub fn marker(&self, path: &str) -> Option<String> {
        let s = &self.stats;
        (s.lua_files + s.inline > 0).then(|| {
            format!(
                "[nginx] locations={} lua_files={} inline={} dynamic={} unresolved={} path={path}",
                s.locations, s.lua_files, s.inline, s.dynamic, s.unresolved
            )
        })
    }
}

/// Read one nginx conf at repo-relative `path`, whose synthetic MODULE is
/// `module_id` (module doc).
pub fn extract_nginx(source: &str, path: &str, module_id: NodeId, repo: RepoId) -> NginxOut {
    let mut cx = Cx {
        path,
        module_id,
        repo,
        out: NginxOut::default(),
        routes: BTreeMap::new(),
        entry_row: None,
    };
    cx.walk(&tokenize(source), None);
    let Cx {
        mut out,
        routes,
        entry_row,
        ..
    } = cx;
    for (qname, route) in routes {
        let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::ROUTE, &qname);
        let mut cells: Vec<Cell> = route
            .rows
            .iter()
            .map(|&r| anchor::position_cell(path, r))
            .collect();
        let line = route.rows.first().map_or(0, |r| u64::from(*r) + 1);
        cells.push(Cell {
            kind: cell_type::ROUTE_METHOD,
            payload: CellPayload::Json(
                serde_json::json!({
                    "method": ANY,
                    "handler": route.handler,
                    "file": path,
                    "line": line,
                    "col": 0,
                })
                .to_string(),
            ),
        });
        out.nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Strong,
            cells,
        });
        out.nav
            .record(id, &qname, &qname, node_kind::ROUTE, Some(module_id));
    }
    if let Some(row) = entry_row {
        out.module_cells.push(Cell {
            kind: cell_type::ENTRYPOINT,
            payload: CellPayload::Json(
                serde_json::json!({
                    "decl": format!("{path}:{}", row + 1),
                    "source": ENTRY_SOURCE,
                })
                .to_string(),
            ),
        });
    }
    out
}

/// One ROUTE a conf mints: its directive rows (0-based, sorted, deduped) and
/// the first content script that serves it.
#[derive(Debug, Default)]
struct RouteAcc {
    rows: Vec<u32>,
    handler: String,
}

/// The walk's state.
struct Cx<'a> {
    path: &'a str,
    module_id: NodeId,
    repo: RepoId,
    out: NginxOut,
    routes: BTreeMap<String, RouteAcc>,
    /// The first entry-phase directive's row.
    entry_row: Option<u32>,
}

/// A `location` the walk is inside: its ROUTE qname when its path is literal,
/// its row, and whether its block holds a content handler with a literal file
/// (then its ROUTE is minted and its other phases hang off it).
struct Loc {
    route: Option<String>,
    row: u32,
    serves: bool,
}

impl Cx<'_> {
    fn walk(&mut self, dirs: &[Directive], loc: Option<&Loc>) {
        for d in dirs {
            if d.name == "location"
                && let Some(block) = &d.block
            {
                self.out.stats.locations += 1;
                let inner = Loc {
                    route: location_path(&d.args).map(|p| endpoint::route_qname(ANY, &p)),
                    row: d.line,
                    serves: has_content_file(block),
                };
                self.walk(block, Some(&inner));
                continue;
            }
            if let Some(phase) = lua_phase(&d.name) {
                self.phase(d, phase, loc);
            } else if let Some(block) = &d.block {
                // `http`, `server`, `upstream`, a location's `if`: the same
                // location scope.
                self.walk(block, loc);
            }
        }
    }

    fn phase(&mut self, d: &Directive, phase: Phase<'_>, loc: Option<&Loc>) {
        let Phase::File(name) = phase else {
            self.out.stats.inline += 1;
            return;
        };
        let Some(script) = script_arg(name, &d.args) else {
            return;
        };
        self.out.stats.lua_files += 1;
        if script.contains('$') {
            self.out.stats.dynamic += 1;
            return;
        }
        let rule = d.name.clone();
        if ENTRY_PHASES.contains(&name) {
            self.entry_row.get_or_insert(d.line);
            self.fact(self.module_id, script, edge_category::USES, rule, d.line);
            return;
        }
        // The location's ROUTE, when it is literal and served by a Lua file.
        let route = loc.and_then(|l| {
            l.route
                .as_ref()
                .filter(|_| l.serves)
                .map(|q| (q.clone(), l.row))
        });
        let Some((qname, row)) = route else {
            if name == "content" {
                // A regex / named location: no ROUTE to serve.
                self.out.stats.unresolved += 1;
            }
            self.fact(self.module_id, script, edge_category::USES, rule, d.line);
            return;
        };
        let id = NodeId::from_parts(GRAPH_TYPE, self.repo, node_kind::ROUTE, &qname);
        let acc = self.routes.entry(qname).or_default();
        if !acc.rows.contains(&row) {
            acc.rows.push(row);
            acc.rows.sort_unstable();
        }
        let category = if name == "content" {
            if acc.handler.is_empty() {
                acc.handler = script.to_string();
            }
            edge_category::HANDLED_BY
        } else {
            edge_category::USES
        };
        self.fact(id, script, category, rule, d.line);
    }

    fn fact(
        &mut self,
        scope: NodeId,
        script: &str,
        category: glia_core::EdgeCategoryId,
        rule: String,
        line: u32,
    ) {
        self.out.facts.push((
            scope,
            NavFact::HostScriptRef {
                host: HOST.to_string(),
                conf: self.path.to_string(),
                path: script.to_string(),
                category,
                rule,
                line,
            },
        ));
    }
}

/// The script argument of a `<name>_by_lua_file` directive: the first
/// argument, but the second for `set_by_lua_file $res <path> [args]`.
fn script_arg<'a>(name: &str, args: &'a [String]) -> Option<&'a str> {
    let i = usize::from(name == "set");
    args.get(i).map(String::as_str).filter(|a| !a.is_empty())
}

/// True when `block` (a location's body, its `if` blocks included, nested
/// locations not) holds `content_by_lua_file` with a literal file.
fn has_content_file(block: &[Directive]) -> bool {
    block.iter().any(|d| {
        if d.name == "location" {
            return false;
        }
        let serves = d.name == "content_by_lua_file"
            && script_arg("content", &d.args).is_some_and(|s| !s.contains('$'));
        serves || d.block.as_deref().is_some_and(has_content_file)
    })
}

/// The literal path of a `location` (`/x`, `= /x`, `=/x`, `^~ /x`); `None`
/// for a regex (`~`, `~*`) or named (`@x`) location, or one with no path.
fn location_path(args: &[String]) -> Option<String> {
    let first = args.first()?;
    let path = match first.as_str() {
        "=" | "^~" => args.get(1)?.as_str(),
        "~" | "~*" => return None,
        a => a
            .strip_prefix("^~")
            .or_else(|| a.strip_prefix('='))
            .unwrap_or(a),
    };
    (path.starts_with('/') && !path.starts_with("/~")).then(|| path.to_string())
}

/// A Lua phase directive's form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase<'a> {
    /// `<phase>_by_lua_file`: the phase name.
    File(&'a str),
    /// `<phase>_by_lua_block` / `<phase>_by_lua`: inline Lua.
    Inline,
}

/// The Lua phase a directive name names, if any.
fn lua_phase(name: &str) -> Option<Phase<'_>> {
    if let Some(p) = name.strip_suffix("_by_lua_file") {
        return (!p.is_empty()).then_some(Phase::File(p));
    }
    let p = name
        .strip_suffix("_by_lua_block")
        .or_else(|| name.strip_suffix("_by_lua"))?;
    (!p.is_empty()).then_some(Phase::Inline)
}

// ---------------------------------------------------------------------------
// The directive tokenizer
// ---------------------------------------------------------------------------

/// One nginx directive: its name, arguments (quotes removed), 0-based row of
/// its name, and its block when it opens one (`None` for a `;` directive and
/// for a `*_by_lua_block`, whose body is Lua, skipped).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Directive {
    name: String,
    args: Vec<String>,
    line: u32,
    block: Option<Vec<Directive>>,
}

/// Tokenize a conf into its directive tree. Never fails: an unterminated
/// block ends at the end of the file, a stray `}` closes nothing.
fn tokenize(source: &str) -> Vec<Directive> {
    let mut lx = Lexer {
        b: source.as_bytes(),
        i: 0,
        line: 0,
    };
    block(&mut lx, 0)
}

/// Nesting deeper than this is read as flat (a guard against a hostile file).
const MAX_DEPTH: usize = 64;

struct Lexer<'a> {
    b: &'a [u8],
    i: usize,
    line: u32,
}

/// One lexical token.
enum Tok {
    Word(String, u32),
    Semi,
    Open,
    Close,
    End,
}

impl Lexer<'_> {
    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let c = self.peek()?;
        self.i += 1;
        if c == b'\n' {
            self.line += 1;
        }
        Some(c)
    }

    /// Skip whitespace and `#` comments (a `#` starts one only at a token start).
    fn skip_space(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_ascii_whitespace() {
                self.bump();
            } else if c == b'#' {
                while let Some(c) = self.peek() {
                    if c == b'\n' {
                        break;
                    }
                    self.bump();
                }
            } else {
                break;
            }
        }
    }

    fn next(&mut self) -> Tok {
        self.skip_space();
        let line = self.line;
        let Some(c) = self.peek() else {
            return Tok::End;
        };
        match c {
            b';' => {
                self.bump();
                Tok::Semi
            }
            b'{' => {
                self.bump();
                Tok::Open
            }
            b'}' => {
                self.bump();
                Tok::Close
            }
            b'"' | b'\'' => {
                self.bump();
                let mut out = Vec::new();
                while let Some(d) = self.bump() {
                    if d == c {
                        break;
                    }
                    if d == b'\\'
                        && let Some(e) = self.bump()
                    {
                        out.push(e);
                        continue;
                    }
                    out.push(d);
                }
                Tok::Word(String::from_utf8_lossy(&out).into_owned(), line)
            }
            _ => {
                let start = self.i;
                while let Some(d) = self.peek() {
                    if d.is_ascii_whitespace() || matches!(d, b';' | b'{' | b'}') {
                        // `${var}`: the brace belongs to the variable.
                        if d == b'{' && self.i > start && self.b[self.i - 1] == b'$' {
                            while let Some(e) = self.bump() {
                                if e == b'}' {
                                    break;
                                }
                            }
                            continue;
                        }
                        break;
                    }
                    self.bump();
                }
                Tok::Word(
                    String::from_utf8_lossy(&self.b[start..self.i]).into_owned(),
                    line,
                )
            }
        }
    }

    /// Skip a `*_by_lua_block` body after its `{`: Lua, braces balanced
    /// outside its strings (`'..'`, `".."`, `[[..]]`, `[=[..]=]`) and
    /// comments (`-- ..`, `--[[ .. ]]`).
    fn skip_lua_block(&mut self) {
        let mut depth = 1usize;
        while let Some(c) = self.bump() {
            match c {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return;
                    }
                }
                b'"' | b'\'' => {
                    while let Some(d) = self.bump() {
                        if d == b'\\' {
                            self.bump();
                        } else if d == c || d == b'\n' {
                            break;
                        }
                    }
                }
                b'[' => {
                    if let Some(level) = self.long_bracket_open() {
                        self.skip_long_bracket(level);
                    }
                }
                b'-' if self.peek() == Some(b'-') => {
                    self.bump();
                    if self.peek() == Some(b'[')
                        && let Some(level) = {
                            self.bump();
                            self.long_bracket_open()
                        }
                    {
                        self.skip_long_bracket(level);
                    } else {
                        while let Some(d) = self.peek() {
                            if d == b'\n' {
                                break;
                            }
                            self.bump();
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// After a `[`: the level of a Lua long bracket opening here (`[` = 0,
    /// `=[` = 1, ..), without consuming; `None` when none opens.
    fn long_bracket_open(&self) -> Option<usize> {
        let rest = &self.b[self.i..];
        let level = rest.iter().take_while(|&&c| c == b'=').count();
        (rest.get(level) == Some(&b'[')).then_some(level)
    }

    /// Consume a long bracket's `=*[` and body up to its `]=*]`.
    fn skip_long_bracket(&mut self, level: usize) {
        for _ in 0..=level {
            self.bump();
        }
        while let Some(c) = self.bump() {
            if c == b']' {
                let rest = &self.b[self.i..];
                if rest.iter().take_while(|&&c| c == b'=').count() == level
                    && rest.get(level) == Some(&b']')
                {
                    for _ in 0..=level {
                        self.bump();
                    }
                    return;
                }
            }
        }
    }
}

/// The directives up to the closing `}` (or the end of the file).
fn block(lx: &mut Lexer<'_>, depth: usize) -> Vec<Directive> {
    let mut out = Vec::new();
    let mut cur: Option<(String, u32, Vec<String>)> = None;
    loop {
        match lx.next() {
            Tok::End | Tok::Close => {
                if let Some((name, line, args)) = cur.take() {
                    out.push(Directive {
                        name,
                        args,
                        line,
                        block: None,
                    });
                }
                return out;
            }
            Tok::Word(w, line) => match &mut cur {
                Some((_, _, args)) => args.push(w),
                None => cur = Some((w, line, Vec::new())),
            },
            Tok::Semi => {
                if let Some((name, line, args)) = cur.take() {
                    out.push(Directive {
                        name,
                        args,
                        line,
                        block: None,
                    });
                }
            }
            Tok::Open => {
                let Some((name, line, args)) = cur.take() else {
                    // An unnamed block: read through it, keep nothing.
                    let _ = block(lx, depth + 1);
                    continue;
                };
                let body = if name.ends_with("_by_lua_block") {
                    lx.skip_lua_block();
                    None
                } else if depth >= MAX_DEPTH {
                    let _ = block(lx, depth + 1);
                    Some(Vec::new())
                } else {
                    Some(block(lx, depth + 1))
                };
                out.push(Directive {
                    name,
                    args,
                    line,
                    block: body,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPO: RepoId = RepoId(7);

    fn module() -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, REPO, node_kind::MODULE, "nginx.conf")
    }

    fn route_id(path: &str) -> NodeId {
        NodeId::from_parts(
            GRAPH_TYPE,
            REPO,
            node_kind::ROUTE,
            &endpoint::route_qname(ANY, path),
        )
    }

    /// `(scope, path, category, rule, line)` of every fact, in order.
    fn facts(out: &NginxOut) -> Vec<(NodeId, String, u32, String, u32)> {
        out.facts
            .iter()
            .map(|(scope, f)| match f {
                NavFact::HostScriptRef {
                    host,
                    conf,
                    path,
                    category,
                    rule,
                    line,
                } => {
                    assert_eq!((host.as_str(), conf.as_str()), (HOST, "nginx.conf"));
                    (*scope, path.clone(), category.0, rule.clone(), *line)
                }
                other => panic!("unexpected fact {other:?}"),
            })
            .collect()
    }

    fn read(src: &str) -> NginxOut {
        extract_nginx(src, "nginx.conf", module(), REPO)
    }

    /// The substrate-gap fixture's conf (patterns/lua/embedded_script).
    const FIXTURE: &str = "http {\n  server {\n    listen 8080;\n    location /api/hello {\n      content_by_lua_file lua/hello.lua;\n    }\n  }\n}\n";

    #[test]
    fn content_by_lua_file_mints_the_location_route() {
        let out = read(FIXTURE);
        assert_eq!(
            out.marker("nginx.conf").as_deref(),
            Some("[nginx] locations=1 lua_files=1 inline=0 dynamic=0 unresolved=0 path=nginx.conf")
        );
        assert_eq!(out.nodes.len(), 1);
        assert_eq!(out.nodes[0].id, route_id("/api/hello"));
        assert_eq!(
            out.nav
                .qname_by_id
                .get(&route_id("/api/hello"))
                .map(String::as_str),
            Some("ANY /api/hello")
        );
        assert_eq!(
            out.nav.parent_of.get(&route_id("/api/hello")),
            Some(&module())
        );
        let cells: Vec<u32> = out.nodes[0].cells.iter().map(|c| c.kind.0).collect();
        assert_eq!(cells, [cell_type::POSITION.0, cell_type::ROUTE_METHOD.0]);
        assert_eq!(
            facts(&out),
            [(
                route_id("/api/hello"),
                "lua/hello.lua".to_string(),
                edge_category::HANDLED_BY.0,
                "content_by_lua_file".to_string(),
                4
            )]
        );
        assert!(out.module_cells.is_empty());
    }

    #[test]
    fn phases_scopes_and_location_forms() {
        let src = r#"
# a comment with content_by_lua_file not/read.lua;
init_by_lua_file init.lua;
http {
  init_worker_by_lua_file "workers/boot.lua";
  upstream backend { balancer_by_lua_file lb.lua; }
  server {
    access_by_lua_file auth/server.lua;
    location = /exact { access_by_lua_file auth/gate.lua; content_by_lua_file app/exact.lua; }
    location ^~ /static/ { content_by_lua_file 'app/static.lua'; }
    location ~ ^/re/(.*)$ { content_by_lua_file app/re.lua; }
    location @named { content_by_lua_file app/named.lua; }
    location /proxy { access_by_lua_file auth/proxy.lua; proxy_pass http://up; }
    location /dyn { content_by_lua_file app/$host.lua; }
    location /inline {
      content_by_lua_block {
        local t = { a = "}" } -- a } in a comment
        ngx.say([[ } ]])
      }
    }
    location /set { set_by_lua_file $res app/set.lua a; content_by_lua_file app/set_page.lua; }
  }
}
"#;
        let out = read(src);
        assert_eq!(
            out.marker("nginx.conf").as_deref(),
            Some(
                "[nginx] locations=8 lua_files=13 inline=1 dynamic=1 unresolved=2 path=nginx.conf"
            )
        );
        let routes: Vec<&str> = out
            .nodes
            .iter()
            .filter_map(|n| out.nav.qname_by_id.get(&n.id).map(String::as_str))
            .collect();
        assert_eq!(routes, ["ANY /exact", "ANY /set", "ANY /static/"]);
        let m = module();
        let uses = edge_category::USES.0;
        let hb = edge_category::HANDLED_BY.0;
        let all = facts(&out);
        let got: Vec<(NodeId, &str, u32, &str)> = all
            .iter()
            .map(|(s, p, c, r, _)| (*s, p.as_str(), *c, r.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                (m, "init.lua", uses, "init_by_lua_file"),
                (m, "workers/boot.lua", uses, "init_worker_by_lua_file"),
                (m, "lb.lua", uses, "balancer_by_lua_file"),
                (m, "auth/server.lua", uses, "access_by_lua_file"),
                (
                    route_id("/exact"),
                    "auth/gate.lua",
                    uses,
                    "access_by_lua_file"
                ),
                (
                    route_id("/exact"),
                    "app/exact.lua",
                    hb,
                    "content_by_lua_file"
                ),
                (
                    route_id("/static/"),
                    "app/static.lua",
                    hb,
                    "content_by_lua_file"
                ),
                (m, "app/re.lua", uses, "content_by_lua_file"),
                (m, "app/named.lua", uses, "content_by_lua_file"),
                (m, "auth/proxy.lua", uses, "access_by_lua_file"),
                (route_id("/set"), "app/set.lua", uses, "set_by_lua_file"),
                (
                    route_id("/set"),
                    "app/set_page.lua",
                    hb,
                    "content_by_lua_file"
                ),
            ]
        );
        // The first entry phase's row, 1-based.
        assert_eq!(out.module_cells.len(), 1);
        assert_eq!(out.module_cells[0].kind, cell_type::ENTRYPOINT);
        assert_eq!(
            out.module_cells[0].payload,
            CellPayload::Json(r#"{"decl":"nginx.conf:3","source":"host:nginx"}"#.to_string())
        );
    }

    #[test]
    fn a_route_two_servers_name_is_one_node() {
        let src = "server { location /a { content_by_lua_file a.lua; } }\nserver { location /a { content_by_lua_file b.lua; } }\n";
        let out = read(src);
        assert_eq!(out.nodes.len(), 1);
        let rows: Vec<&Cell> = out.nodes[0]
            .cells
            .iter()
            .filter(|c| c.kind == cell_type::POSITION)
            .collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(facts(&out).len(), 2);
    }

    #[test]
    fn sniff_and_paths() {
        assert!(sniff(FIXTURE));
        assert!(sniff("location /x { content_by_lua_block { ngx.say(1) } }"));
        assert!(!sniff(
            "server { listen 80; location / { proxy_pass http://a; } }"
        ));
        // A word that is no directive name: an argument or a comment.
        assert!(!sniff(
            "# content_by_lua_file x.lua;\nlog_format main \"content_by_lua_file\";"
        ));
        assert!(!sniff(
            "akka.quartz.schedules { Nightly { expression = \"0 0 3 * * ?\" } }"
        ));
        for yes in [
            "nginx.conf",
            "nginx/site.conf",
            "conf.d/API.CONF",
            "notes.conf",
        ] {
            assert!(is_conf_path(yes), "{yes}");
        }
        for no in [
            "application.conf",
            "src/main/resources/prod.conf",
            ".conf",
            "nginx.conf.bak",
            "nginx.yaml",
        ] {
            assert!(!is_conf_path(no), "{no}");
        }
    }

    #[test]
    fn tokenizer_survives_hostile_input() {
        for src in [
            "}}}",
            "{{{",
            "location",
            "\"unterminated",
            "a ${b",
            "x_by_lua_block {",
            "--[[",
            "location /a { content_by_lua_file",
        ] {
            let _ = read(src);
            let _ = sniff(src);
        }
        let deep = "a {".repeat(200);
        assert!(!sniff(&deep));
        // A long-bracket string holding braces and a quote in a Lua block.
        let out = read(
            "location /x { content_by_lua_block { s = [==[ }\" ]==] } content_by_lua_file x.lua; }",
        );
        assert_eq!(out.stats.inline, 1);
        assert_eq!(out.stats.lua_files, 1);
        assert_eq!(out.nodes.len(), 1);
    }
}
