//! DA.17: the glia-rules cost gates (R1 section 9, spike 1), run over glia's own graph.
//!
//! - `cost_check20_glia`: the 20 check-shaped rules of [`CHECK20`] over glia x1, median
//!   of 5 evaluations under 200 ms;
//! - `cost_check20_x10_rss`: [`CHECK20`] over glia x10 (`testkit::scaled`) evaluates,
//!   with the evaluator's own peak under 320 MB ([`X10_RSS_GATE_KB`], 327,680 kB). R1's
//!   150 MB (153,600 kB) was restated by James's ruling of 2026-10-16 (option A): the
//!   first run measured 287,408 kB at defff629, so the gate is that plus headroom, and the
//!   evaluator's memory work moves to 0.5.4 (`dev-notes/next-leap-0.5.4.md` §F.3);
//! - `cost_blowup_x10`: the unseeded carry closure over glia x10 (R1's 7.25 M-row case;
//!   13,551,390 rows at 22c65ee) stops at the default budget as `RulesError::Budget`
//!   for `tcc/2`, the evaluator's peak under 512 MB.
//!
//! **Run.** Every test is `#[ignore]`d: timings mean nothing in a debug build, and
//! VmHWM is per process, so each gate runs alone, in release, over a fresh dump of the
//! tree under test:
//!
//! ```text
//! GLIA_NO_PERSIST=1 ./target/debug/glia analyze <git archive of HEAD> --format json > glia.json
//! GLIA_RULES_DUMP=glia.json cargo test -p glia-rules --release --test cost -- \
//!     --ignored --exact <name> --nocapture
//! ```
//!
//! Without `GLIA_RULES_DUMP` a test fails rather than passing on nothing. Each prints
//! one stdout line, `[rules-bench] <check20-glia|check20-x10|blowup-x10> nodes=<n>
//! edges=<e> ms=<t> rows=<r> rss_kb=<evaluator increment> hwm_kb=<whole-process
//! VmHWM>`, beside the evaluator's own `[rules] eval` / `[rules] budget` stderr lines.
//!
//! **The graph.** The rules crate cannot depend on the engine, so the graph is read
//! from the dump ([`read_dump`]: a std-only reader for its one shape) into a
//! [`MemSource`]: node rows deduplicated by id (first kept; one id can sit in several
//! language graphs), rendered by qname with the Str attribute `name`; kind ids from
//! the dump, category ids by first appearance. The dump has no evidence, so tiers come
//! from a fixed table ([`tier_of`]) that exercises all three levels.
//!
//! **Memory.** The gate measures the evaluator, not the graph the engine already holds
//! before any rule runs: with the dump, the x1 and the x10 source all alive, freed
//! heap goes back to the OS (glibc `malloc_trim`, as engine::memory does), VmHWM is
//! reset through `/proc/self/clear_refs`, and the increment is VmHWM minus the VmRSS
//! read right after. The whole-process VmHWM (the build's peak included, R1's figure)
//! is printed beside it.
#![cfg(target_os = "linux")]

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Instant;

use glia_activation::algo::GraphSource;
use glia_activation::profile::{DomainTables, EntryRule, Registries};
use glia_core::{Confidence, EdgeCategoryId, NodeId, NodeKindId};
use glia_rules::diag::RulesError;
use glia_rules::eval::{DEFAULT_MAX_ROWS, Evaluation, Options, evaluate};
use glia_rules::ir::{AttrDecl, BindReq, BuiltinDecl, BuiltinKind, ColType, Program};
use glia_rules::parser::parse_program;
use glia_rules::source::{Builtin, BuiltinRow, MemSource};
use glia_rules::testkit::{scaled, scaled_size};
use glia_rules::value::{Tier, Value, ValueRef};

// ============================================================================
// The dump reader
// ============================================================================

/// One node row of the dump.
struct DumpNode {
    id: u64,
    kind: u32,
    kind_name: String,
    name: String,
    qname: String,
}

/// One edge row of the dump.
struct DumpEdge {
    from: u64,
    to: u64,
    category: String,
}

/// A `glia analyze --format json` dump: `{"nodes": [..], "edges": [..]}`.
struct Dump {
    nodes: Vec<DumpNode>,
    edges: Vec<DumpEdge>,
}

/// A flat value of a dump row: what the shape holds, nothing more.
enum J {
    Str(String),
    UInt(u64),
    Bool,
    Null,
    Strs,
}

struct Reader<'a> {
    b: &'a [u8],
    i: usize,
}

impl Reader<'_> {
    fn err(&self, what: &str) -> String {
        format!("dump byte {}: {what}", self.i)
    }

    /// The next non-whitespace byte, not consumed.
    fn peek(&mut self) -> Option<u8> {
        while self.b.get(self.i).is_some_and(|c| c.is_ascii_whitespace()) {
            self.i += 1;
        }
        self.b.get(self.i).copied()
    }

    fn eat(&mut self, c: u8) -> Result<(), String> {
        if self.peek() != Some(c) {
            return Err(self.err(&format!("expected `{}`", c as char)));
        }
        self.i += 1;
        Ok(())
    }

    /// After a `,` / `[` / `{`: true and consumed when `close` is next.
    fn closes(&mut self, close: u8) -> bool {
        let done = self.peek() == Some(close);
        self.i += usize::from(done);
        done
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let digits = self.b.get(self.i..self.i + 4).ok_or_else(|| self.err("short \\u"))?;
        let text = std::str::from_utf8(digits).map_err(|_| self.err("bad \\u"))?;
        self.i += 4;
        u32::from_str_radix(text, 16).map_err(|_| self.err("bad \\u"))
    }

    fn string(&mut self) -> Result<String, String> {
        self.eat(b'"')?;
        let mut out: Vec<u8> = Vec::new();
        loop {
            let run = self.b[self.i..].iter().position(|c| *c == b'"' || *c == b'\\' || *c < 0x20);
            let end = self.i + run.ok_or_else(|| self.err("unterminated string"))?;
            out.extend_from_slice(&self.b[self.i..end]);
            self.i = end + 1;
            let esc = match self.b[end] {
                b'"' => break,
                b'\\' => self.b.get(self.i).copied().ok_or_else(|| self.err("bad escape"))?,
                _ => return Err(self.err("control byte in string")),
            };
            self.i += 1;
            let ch = match esc {
                b'"' | b'\\' | b'/' => esc as char,
                b'n' => '\n',
                b't' => '\t',
                b'r' => '\r',
                b'b' => '\u{8}',
                b'f' => '\u{c}',
                b'u' => {
                    let hi = self.hex4()?;
                    let code = if (0xD800..0xDC00).contains(&hi) {
                        if self.b.get(self.i..self.i + 2) != Some(b"\\u") {
                            return Err(self.err("lone high surrogate"));
                        }
                        self.i += 2;
                        let lo = self.hex4()?;
                        if !(0xDC00..0xE000).contains(&lo) {
                            return Err(self.err("bad low surrogate"));
                        }
                        0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                    } else {
                        hi
                    };
                    char::from_u32(code).ok_or_else(|| self.err("bad code point"))?
                }
                _ => return Err(self.err("unknown escape")),
            };
            out.extend_from_slice(ch.encode_utf8(&mut [0; 4]).as_bytes());
        }
        String::from_utf8(out).map_err(|_| self.err("string is not UTF-8"))
    }

    /// An unsigned integer, exactly (ids sit above 2^53, so never through f64).
    fn uint(&mut self) -> Result<u64, String> {
        let mut n: u64 = 0;
        let start = self.i;
        while let Some(d) = self.b.get(self.i).filter(|c| c.is_ascii_digit()) {
            n = n
                .checked_mul(10)
                .and_then(|n| n.checked_add(u64::from(d - b'0')))
                .ok_or_else(|| self.err("integer overflows u64"))?;
            self.i += 1;
        }
        if self.i == start || matches!(self.b.get(self.i), Some(b'.' | b'e' | b'E')) {
            return Err(self.err("expected an unsigned integer"));
        }
        Ok(n)
    }

    fn word(&mut self, w: &[u8], v: J) -> Result<J, String> {
        if self.b.get(self.i..self.i + w.len()) != Some(w) {
            return Err(self.err("bad literal"));
        }
        self.i += w.len();
        Ok(v)
    }

    fn value(&mut self) -> Result<J, String> {
        match self.peek() {
            Some(b'"') => self.string().map(J::Str),
            Some(b'0'..=b'9') => self.uint().map(J::UInt),
            Some(b't') => self.word(b"true", J::Bool),
            Some(b'f') => self.word(b"false", J::Bool),
            Some(b'n') => self.word(b"null", J::Null),
            Some(b'[') => {
                self.i += 1;
                if !self.closes(b']') {
                    loop {
                        self.string()?;
                        if self.closes(b']') {
                            break;
                        }
                        self.eat(b',')?;
                    }
                }
                Ok(J::Strs)
            }
            _ => Err(self.err("unexpected value")),
        }
    }

    /// `{ "key": value, .. }`, each pair handed to `field`.
    fn object(&mut self, field: &mut dyn FnMut(&str, J)) -> Result<(), String> {
        self.eat(b'{')?;
        if self.closes(b'}') {
            return Ok(());
        }
        loop {
            let key = self.string()?;
            self.eat(b':')?;
            let v = self.value()?;
            field(&key, v);
            if self.closes(b'}') {
                return Ok(());
            }
            self.eat(b',')?;
        }
    }

    /// `[ {..}, .. ]`, each object read by `row`.
    fn rows(&mut self, row: &mut dyn FnMut(&mut Self) -> Result<(), String>) -> Result<(), String> {
        self.eat(b'[')?;
        if self.closes(b']') {
            return Ok(());
        }
        loop {
            row(self)?;
            if self.closes(b']') {
                return Ok(());
            }
            self.eat(b',')?;
        }
    }
}

fn read_dump(path: &str) -> Result<Dump, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let mut r = Reader { b: &bytes, i: 0 };
    let mut dump = Dump { nodes: Vec::new(), edges: Vec::new() };
    r.eat(b'{')?;
    loop {
        match r.string()?.as_str() {
            "nodes" => {
                r.eat(b':')?;
                r.rows(&mut |r| {
                    let mut n = DumpNode {
                        id: 0,
                        kind: 0,
                        kind_name: String::new(),
                        name: String::new(),
                        qname: String::new(),
                    };
                    let mut seen = 0;
                    r.object(&mut |k, v| match (k, v) {
                        ("id", J::UInt(x)) => (n.id, seen) = (x, seen + 1),
                        ("kind", J::UInt(x)) => (n.kind, seen) = (x as u32, seen + 1),
                        ("kind_name", J::Str(s)) => (n.kind_name, seen) = (s, seen + 1),
                        ("name", J::Str(s)) => (n.name, seen) = (s, seen + 1),
                        ("qname", J::Str(s)) => (n.qname, seen) = (s, seen + 1),
                        _ => {}
                    })?;
                    if seen != 5 {
                        return Err(r.err("node row lacks id / kind / kind_name / name / qname"));
                    }
                    dump.nodes.push(n);
                    Ok(())
                })?;
            }
            "edges" => {
                r.eat(b':')?;
                r.rows(&mut |r| {
                    let mut e = DumpEdge { from: 0, to: 0, category: String::new() };
                    let mut seen = 0;
                    r.object(&mut |k, v| match (k, v) {
                        ("from", J::UInt(x)) => (e.from, seen) = (x, seen + 1),
                        ("to", J::UInt(x)) => (e.to, seen) = (x, seen + 1),
                        ("category", J::Str(s)) => (e.category, seen) = (s, seen + 1),
                        _ => {}
                    })?;
                    if seen != 3 {
                        return Err(r.err("edge row lacks from / to / category"));
                    }
                    dump.edges.push(e);
                    Ok(())
                })?;
            }
            other => return Err(r.err(&format!("unknown top-level key `{other}`"))),
        }
        if r.closes(b'}') {
            return Ok(dump);
        }
        r.eat(b',')?;
    }
}

// ============================================================================
// The source
// ============================================================================

/// engine/src/check.rs `default_forbid_categories`: IMPORTS, then code-domain's
/// `CODE_TABLES.carry_edges` (code-domain/src/profile.rs) without TESTS and DOCUMENTS
/// (the R1 spike's `CARRY` list). A literal: the rules crate cannot read code-domain.
const CARRY: [&str; 25] = [
    "IMPORTS",
    "CALLS",
    "USES",
    "HTTP_CALLS",
    "GRPC_CALLS",
    "RPC_CALLS",
    "GRAPHQL_CALLS",
    "QUEUE_FLOWS",
    "WS_CONNECTS",
    "EVENT_FLOWS",
    "CLI_INVOKES",
    "NAVIGATES_TO",
    "HANDLED_BY",
    "INJECTS",
    "ACCESSES_DATA",
    "IMPLEMENTS",
    "INHERITS_FROM",
    "RETURNS_TYPE",
    "SHARES_SCHEMA",
    "SHARES_DATA_ENTITY",
    "INFRA_REFERENCES",
    "DEPENDS_ON",
    "SCHEDULES",
    "READS_CONFIG",
    "DEFINES_CONFIG",
];

/// The first qname segments [`CHECK20`] scopes by; each must hold nodes.
const SCOPES: [&str; 10] = [
    "cli",
    "store",
    "engine",
    "graph",
    "py",
    "activation",
    "code-domain",
    "core",
    "parsers",
    "projection-text",
];

/// The evidence table: DEFINES / CONTAINS / IMPORTS / USES FACT; CALLS FACT but every
/// 7th CALLS edge in ordinal order (`calls_before % 7 == 6`) HEURISTIC; the rest DERIVED.
fn tier_of(category: &str, calls_before: usize) -> Tier {
    match category {
        "DEFINES" | "CONTAINS" | "IMPORTS" | "USES" => Tier::Fact,
        "CALLS" if calls_before % 7 == 6 => Tier::Heuristic,
        "CALLS" => Tier::Fact,
        _ => Tier::Derived,
    }
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

/// `in_scope(Node, Str)`: the node's qname starts with `<scope>::`. A bound node is one
/// lookup; a free one enumerates the scope's members, memoised per scope.
struct InScope {
    /// Node id -> its x1 position (copy `c` of node `v` maps to `v`).
    position: HashMap<NodeId, u32>,
    /// The source's node order (what a free call enumerates).
    order: Vec<NodeId>,
    /// x1 qnames by position, shared by the x1 and the x10 builtin.
    qnames: Rc<Vec<String>>,
    memo: Rc<RefCell<HashMap<String, Rc<Vec<NodeId>>>>>,
}

fn in_scope(qname: &str, scope: &str) -> bool {
    qname.strip_prefix(scope).is_some_and(|rest| rest.starts_with("::"))
}

impl Builtin for InScope {
    fn decl(&self) -> BuiltinDecl {
        BuiltinDecl {
            name: "in_scope".into(),
            cols: vec![ColType::Node, ColType::Str],
            bind: vec![BindReq::May, BindReq::Must],
            tiered: false,
            kind: BuiltinKind::Domain,
        }
    }

    fn call(&self, args: &[Option<ValueRef<'_>>], out: &mut Vec<BuiltinRow>) -> Result<(), String> {
        let [node, Some(scope)] = args else {
            return Err("takes (node, bound scope)".to_string());
        };
        let ValueRef::Str(scope) = *scope else { return Ok(()) };
        let row = |id: NodeId| BuiltinRow {
            values: vec![Value::Node(id), Value::Str(scope.to_string())],
            tier: Tier::Fact,
        };
        match node {
            Some(ValueRef::Node(id)) => {
                let q = self.position.get(id).map(|p| self.qnames[*p as usize].as_str());
                if q.is_some_and(|q| in_scope(q, scope)) {
                    out.push(row(*id));
                }
            }
            Some(_) => {}
            None => {
                let members = Rc::clone(
                    self.memo.borrow_mut().entry(scope.to_string()).or_insert_with(|| {
                        let ids = self.order.iter().copied();
                        Rc::new(
                            ids.filter(|id| {
                                in_scope(&self.qnames[self.position[id] as usize], scope)
                            })
                            .collect(),
                        )
                    }),
                );
                out.extend(members.iter().map(|id| row(*id)));
            }
        }
        Ok(())
    }
}

/// A source with its in_scope builtin registered, and that builtin's memo.
struct Graph {
    src: MemSource,
    nodes: usize,
    edges: usize,
    memo: Rc<RefCell<HashMap<String, Rc<Vec<NodeId>>>>>,
}

fn register(mut src: MemSource, x1: usize, qnames: &Rc<Vec<String>>) -> Graph {
    let order = src.node_ids();
    let position = order.iter().enumerate().map(|(i, id)| (*id, (i % x1) as u32)).collect();
    let memo = Rc::new(RefCell::new(HashMap::new()));
    let edges = src.edges().count();
    let nodes = order.len();
    let b = InScope { position, order, qnames: Rc::clone(qnames), memo: Rc::clone(&memo) };
    src.builtin(Box::new(b));
    Graph { src, nodes, edges, memo }
}

/// The dump as a MemSource (module doc), its builtin registered, and the x1 qnames.
fn glia_x1(dump: &Dump) -> (Graph, Rc<Vec<String>>) {
    let mut kinds: Vec<(NodeKindId, &'static str)> = Vec::new();
    for n in &dump.nodes {
        if !kinds.iter().any(|(k, _)| k.0 == n.kind) {
            kinds.push((NodeKindId(n.kind), leak(&n.kind_name)));
        }
    }
    let mut cats: Vec<(EdgeCategoryId, &'static str)> = Vec::new();
    for e in &dump.edges {
        if !cats.iter().any(|(_, c)| *c == e.category) {
            cats.push((EdgeCategoryId(cats.len() as u32 + 1), leak(&e.category)));
        }
    }
    let kinds: &'static [(NodeKindId, &'static str)] = Box::leak(kinds.into_boxed_slice());
    let cats: &'static [(EdgeCategoryId, &'static str)] = Box::leak(cats.into_boxed_slice());
    let cat_id = |name: &str| cats.iter().find(|(_, c)| *c == name).map(|(id, _)| *id);
    let carry: Vec<EdgeCategoryId> = CARRY.iter().filter_map(|c| cat_id(c)).collect();
    let tables = DomainTables {
        graph_type: "glia-analyze-dump",
        registries: Registries { node_kinds: kinds, edge_categories: cats, cell_types: &[] },
        entry: EntryRule { kinds: &[], roles: &[], named: &[] },
        carry_edges: Box::leak(carry.into_boxed_slice()),
        effect_sinks: &[],
        activation_weights: &[],
        activation_presets: &[],
        community_weights: &[],
        identity_rules: &[],
    };
    let mut src = MemSource::new(tables);
    src.attr(AttrDecl { name: "name", ty: ColType::Str });
    let mut seen: HashSet<u64> = HashSet::new();
    let mut qnames = Vec::new();
    for n in dump.nodes.iter().filter(|n| seen.insert(n.id)) {
        src.node(NodeId(n.id), NodeKindId(n.kind), n.qname.clone());
        src.set_attr(NodeId(n.id), 0, Value::Str(n.name.clone()));
        qnames.push(n.qname.clone());
    }
    let mut calls = 0;
    for e in &dump.edges {
        let tier = tier_of(&e.category, calls);
        calls += usize::from(e.category == "CALLS");
        let conf = match tier {
            Tier::Fact => Confidence::Strong,
            Tier::Derived => Confidence::Medium,
            Tier::Heuristic => Confidence::Weak,
        };
        let cat = cat_id(&e.category).unwrap_or(EdgeCategoryId(0));
        src.edge(NodeId(e.from), NodeId(e.to), cat, conf, tier);
    }
    let qnames = Rc::new(qnames);
    assert!(qnames.len() >= 20_000, "{} distinct node ids: not a dump of glia", qnames.len());
    for s in SCOPES {
        assert!(qnames.iter().any(|q| in_scope(q, s)), "scope `{s}` holds no node");
    }
    let n = qnames.len();
    (register(src, n, &qnames), qnames)
}

fn dump() -> Dump {
    let Ok(path) = std::env::var("GLIA_RULES_DUMP") else {
        panic!("set GLIA_RULES_DUMP to a glia analyze --format json dump");
    };
    read_dump(&path).unwrap_or_else(|e| panic!("{e}"))
}

// ============================================================================
// The programs and the measurements
// ============================================================================

/// 8 direct forbids, 6 transitive forbids, 3 cycle rules and 3 invariants.
const CHECK20: &str = r#"
f1(a, b) :- in_scope(a, "cli"), edge(a, b, c), carry_category(c), in_scope(b, "store").
f2(a, b) :- in_scope(a, "py"), edge(a, b, c), carry_category(c), in_scope(b, "graph").
f3(a, b) :- in_scope(a, "graph"), edge(a, b, c), carry_category(c), in_scope(b, "engine").
f4(a, b) :- in_scope(a, "store"), edge(a, b, c), carry_category(c), in_scope(b, "engine").
f5(a, b) :- in_scope(a, "activation"), edge(a, b, c), carry_category(c), in_scope(b, "code-domain").
f6(a, b) :- in_scope(a, "core"), edge(a, b, c), carry_category(c), in_scope(b, "graph").
f7(a, b) :- in_scope(a, "parsers"), edge(a, b, c), carry_category(c), in_scope(b, "engine").
f8(a, b) :- in_scope(a, "projection-text"), edge(a, b, c), carry_category(c), in_scope(b, "engine").
t1(b) :- in_scope(a, "cli"), reaches(a, b), in_scope(b, "store").
t2(b) :- in_scope(a, "py"), reaches(a, b), in_scope(b, "graph").
t3(b) :- in_scope(a, "graph"), reaches(a, b), in_scope(b, "engine").
t4(b) :- in_scope(a, "store"), reaches(a, b), in_scope(b, "graph").
t5(b) :- in_scope(a, "core"), reaches(a, b), in_scope(b, "activation").
t6(b) :- in_scope(a, "activation"), reaches(a, b), in_scope(b, "graph").
c1(r) :- scc(x, r, "IMPORTS"), in_scope(x, "engine").
c2(r) :- scc(x, r, "CALLS"), in_scope(x, "graph").
c3(r) :- scc(x, r), in_scope(x, "engine").
handled(r) :- edge(r, _, "HANDLED_BY").
unhandled_route(r) :- node(r, "ROUTE"), !handled(r).
tested(m) :- edge(_, m, "TESTS").
untested_module(m) :- node(m, "MODULE"), in_scope(m, "engine"), !tested(m).
live(y) :- node(x, "ROUTE"), reaches(x, y).
dead_function(f) :- node(f, "FUNCTION"), in_scope(f, "engine"), !live(f).
"#;

const CHECK20_OUTPUTS: [&str; 20] = [
    "f1",
    "f2",
    "f3",
    "f4",
    "f5",
    "f6",
    "f7",
    "f8",
    "t1",
    "t2",
    "t3",
    "t4",
    "t5",
    "t6",
    "c1",
    "c2",
    "c3",
    "unhandled_route",
    "untested_module",
    "dead_function",
];

/// The unseeded transitive closure over the carry set.
const BLOWUP: &str = r#"
tcc(x, y) :- edge(x, y, c), carry_category(c).
tcc(x, z) :- tcc(x, y), edge(y, z, c), carry_category(c).
"#;

fn program(name: &str, text: &str) -> Program {
    parse_program(name, text).unwrap_or_else(|e| panic!("{e}"))
}

fn options(outputs: &[&str]) -> Options {
    let mut o = Options::default();
    o.outputs = outputs.iter().map(|s| s.to_string()).collect();
    o
}

/// A `/proc/self/status` field in kB.
fn vm(field: &str) -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").expect("/proc/self/status");
    let line = status.lines().find_map(|l| l.strip_prefix(field)?.strip_prefix(':'));
    let kb = line.and_then(|l| l.split_whitespace().next()?.parse().ok());
    kb.unwrap_or_else(|| panic!("no {field} in /proc/self/status"))
}

#[cfg(target_env = "gnu")]
fn trim() {
    unsafe extern "C" {
        fn malloc_trim(pad: usize) -> std::os::raw::c_int;
    }
    // SAFETY: malloc_trim touches only the allocator's own state, under its locks.
    unsafe {
        malloc_trim(0);
    }
}

#[cfg(not(target_env = "gnu"))]
fn trim() {}

/// What an evaluator's peak is measured over.
struct Baseline {
    /// VmRSS right after the reset.
    rss: u64,
    /// VmHWM before it: the peak of reading the dump and building the sources.
    build_hwm: u64,
}

/// Read the build's VmHWM, return freed heap to the OS, reset VmHWM to VmRSS, and
/// read that VmRSS.
fn reset_hwm() -> Baseline {
    let build_hwm = vm("VmHWM");
    trim();
    std::fs::write("/proc/self/clear_refs", "5").expect("/proc/self/clear_refs");
    let rss = vm("VmRSS");
    let hwm = vm("VmHWM");
    assert!(hwm <= rss + 4_096, "clear_refs did not reset VmHWM ({hwm} kB, VmRSS {rss} kB)");
    Baseline { rss, build_hwm }
}

fn rows(ev: &Evaluation<'_, MemSource>, outputs: &[&str]) -> usize {
    outputs.iter().map(|p| ev.rows(p).unwrap_or_else(|e| panic!("{e}")).len()).sum()
}

/// Print the `[rules-bench]` line and return the evaluator's increment: VmHWM since
/// the reset minus the baseline VmRSS. `hwm_kb` is the whole process's peak, the
/// build's included (R1's figure).
fn bench(name: &str, g: &Graph, ms: f64, rows: usize, base: &Baseline) -> u64 {
    let peak = vm("VmHWM");
    let inc = peak.saturating_sub(base.rss);
    let hwm = peak.max(base.build_hwm);
    println!(
        "[rules-bench] {name} nodes={} edges={} ms={ms:.1} rows={rows} rss_kb={inc} hwm_kb={hwm}",
        g.nodes, g.edges
    );
    inc
}

// ============================================================================
// The gates
// ============================================================================

#[test]
#[ignore = "release cost gate: GLIA_RULES_DUMP=<dump> cargo test --release --test cost -- --ignored --exact"]
fn cost_check20_glia() {
    let dump = dump();
    let (g, _) = glia_x1(&dump);
    let (p, o) = (program("check20", CHECK20), options(&CHECK20_OUTPUTS));
    let base = reset_hwm();
    let mut times = Vec::new();
    let mut found = 0;
    for _ in 0..5 {
        g.memo.borrow_mut().clear();
        let t = Instant::now();
        let ev = evaluate(&p, &g.src, &o).unwrap_or_else(|e| panic!("{e}"));
        times.push(t.elapsed().as_secs_f64() * 1_000.0);
        found = rows(&ev, &CHECK20_OUTPUTS);
    }
    times.sort_by(f64::total_cmp);
    let median = times[2];
    bench("check20-glia", &g, median, found, &base);
    assert!(
        median < 200.0,
        "CHECK20 over glia x1: median {median:.1} ms of {times:?}, gate 200 ms"
    );
}

/// The x10 evaluator-peak gate, 320 MB. R1 section 9 spike 1 set 150 MB (153,600 kB);
/// the first DA.17 run (40959286, HEAD defff629) measured 287,408 kB, and James's ruling
/// of 2026-10-16 (option A) restated the gate to that number plus headroom. Shrinking the
/// evaluator's peak is 0.5.4 work (`dev-notes/next-leap-0.5.4.md` §F.3), not this test's.
const X10_RSS_GATE_KB: u64 = 327_680;

#[test]
#[ignore = "release cost gate: GLIA_RULES_DUMP=<dump> cargo test --release --test cost -- --ignored --exact"]
fn cost_check20_x10_rss() {
    let dump = dump();
    let (g1, qnames) = glia_x1(&dump);
    let g10 = register(scaled(&g1.src, 10), g1.nodes, &qnames);
    assert_eq!((g10.nodes, g10.edges), scaled_size(g1.nodes, g1.edges, 10));
    let (p, o) = (program("check20", CHECK20), options(&CHECK20_OUTPUTS));
    let base = reset_hwm();
    let t = Instant::now();
    let ev = evaluate(&p, &g10.src, &o).unwrap_or_else(|e| panic!("{e}"));
    let ms = t.elapsed().as_secs_f64() * 1_000.0;
    let inc = bench("check20-x10", &g10, ms, rows(&ev, &CHECK20_OUTPUTS), &base);
    drop(ev);
    assert!(
        inc < X10_RSS_GATE_KB,
        "CHECK20 over glia x10: evaluator peak {inc} kB, gate {X10_RSS_GATE_KB} kB \
         (320 MB, restated from R1's 153,600 kB by James's 2026-10-16 ruling)"
    );
    drop((g10, g1, dump));
}

#[test]
#[ignore = "release cost gate: GLIA_RULES_DUMP=<dump> cargo test --release --test cost -- --ignored --exact"]
fn cost_blowup_x10() {
    let dump = dump();
    let (g1, qnames) = glia_x1(&dump);
    let g10 = register(scaled(&g1.src, 10), g1.nodes, &qnames);
    let (p, o) = (program("blowup", BLOWUP), options(&["tcc"]));
    let base = reset_hwm();
    let t = Instant::now();
    let got = evaluate(&p, &g10.src, &o).map(|_| ());
    let ms = t.elapsed().as_secs_f64() * 1_000.0;
    let derived = match &got {
        Err(RulesError::Budget { rows, .. }) => *rows,
        _ => 0,
    };
    let inc = bench("blowup-x10", &g10, ms, derived, &base);
    match got {
        Err(RulesError::Budget { predicate, cap, .. }) => {
            assert_eq!((predicate.as_str(), cap), ("tcc/2", DEFAULT_MAX_ROWS));
            assert_eq!(cap, 5_000_000);
        }
        other => panic!("the unseeded carry closure at x10 must stop at the budget: {other:?}"),
    }
    assert!(inc < 524_288, "blowup over glia x10: evaluator peak {inc} kB, gate 524,288 kB");
    drop((g10, g1, dump));
}
