//! Public API stability lint (LD.9): every struct and enum the engine or graph
//! facade makes public can grow without breaking a caller.
//!
//! THE RULE. A type that `engine/src/lib.rs` or `graph/src/lib.rs` makes
//! public, and that is DEFINED in that crate, is one of:
//!
//! - (a) `#[non_exhaustive]`. A new field or variant is then not a break.
//!   Outside the crate the struct comes from its producing function, or from
//!   `Default` plus field assignment when it derives `Default` (struct-literal
//!   and `..Default::default()` syntax are both refused), and a `match` on the
//!   enum needs a wildcard arm;
//! - (b) a struct with at least one non-`pub` field: it is already
//!   unconstructible outside the crate;
//! - (c) named in [`ALLOWLIST`], with its reason.
//!
//! "Makes public" means named by a `pub use` in lib.rs (an explicit list, a
//! single path, or a `module::*` glob, followed through that module's own
//! `pub use`s), or declared at the top level of a `pub mod` slot lib.rs
//! declares (`repo_graph_engine::find::FoundNode`). A unit resolver struct
//! (`pub struct XResolver;` with an `impl CrossGraphResolver for XResolver`)
//! passes by rule: the engine constructs it in the resolver pass, which the
//! attribute would forbid. A re-export from another crate (stamp's constants,
//! the graph's `MergedGraph` alias) is that crate's to decide and is skipped,
//! printed once. activation's `ActivationConfig` / `DomainProfile` stay
//! exhaustive on purpose: a domain builds them with literals.
//!
//! The scan is text and deliberately simple: it assumes rustfmt layout (items
//! at column 0, fields at four spaces). It fails loudly rather than passing
//! vacuously when it meets a shape it cannot follow: a glob or `pub mod` whose
//! file it cannot find, a `pub use` name it cannot resolve, a stale allowlist
//! entry, or a scan that no longer finds the anchor types.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

/// Exported types that are neither `#[non_exhaustive]` nor private-fielded,
/// with the reason. An entry whose type starts passing (a) or (b), or that no
/// longer names an exported type, fails the lint: delete it then.
const ALLOWLIST: &[(&str, &str)] = &[
    (
        "MergedGraph",
        "data model: store builds `MergedGraph { .. }` when it loads a .gmap",
    ),
    (
        "RepoGraph",
        "data model: engine, graph and engram-export tests build `RepoGraph` literals; LC.2 owns the Node / Edge shape",
    ),
    (
        "SymbolTable",
        "data model: a `RepoGraph` field, built with it",
    ),
    // Stopgap: types in files outside LD.9's claim. Removal: mark them, move the
    // named test literals onto `Default` + field assignment, delete the entry.
    (
        "CacheDiff",
        "engine/tests/cache_diff.rs builds it by literal to compare",
    ),
    (
        "Identity",
        "graph/src/identity.rs (LB.6); graph/tests/identity_moves.rs builds it by literal",
    ),
    (
        "FileMove",
        "graph/src/identity.rs (LB.6); graph/tests + engine/tests identity_moves.rs build it by literal",
    ),
    (
        "MoveMap",
        "graph/src/identity.rs (LB.6); graph/tests/identity_moves.rs builds it by literal",
    ),
    (
        "NodeMove",
        "graph/src/identity.rs (LB.6), outside LD.9's files",
    ),
    (
        "MoveTier",
        "graph/src/identity.rs (LB.6), outside LD.9's files",
    ),
    (
        "Rebind",
        "graph/src/identity.rs (LB.6), outside LD.9's files",
    ),
];

/// Types the scan must find; missing any means the facade's shape moved under it.
const ANCHORS: &[&str] = &[
    "GenerateResult",
    "ServiceMap",
    "BlastHit",
    "RouteMatch",
    "MergedGraph",
];

/// A module file's top level: `pub use` statements (whitespace-normalised),
/// declared module names, `pub mod` names, `pub struct` / `pub enum` (name, 0-based line).
struct Top {
    uses: Vec<String>,
    mods: BTreeSet<String>,
    pub_mods: Vec<String>,
    types: Vec<(String, usize)>,
}

struct Scan {
    src: PathBuf,
    /// (file, 0-based line) -> type name.
    defs: BTreeMap<(PathBuf, usize), String>,
    foreign: BTreeSet<String>,
}

fn lines_of(path: &Path) -> Vec<String> {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    text.lines().map(str::to_string).collect()
}

fn ident(s: &str) -> String {
    s.chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect()
}

fn top_items(lines: &[String]) -> Top {
    let mut top = Top {
        uses: vec![],
        mods: BTreeSet::new(),
        pub_mods: vec![],
        types: vec![],
    };
    let mut i = 0;
    while i < lines.len() {
        let l = &lines[i];
        if l.starts_with("pub use ") {
            let mut stmt = l.clone();
            while !stmt.contains(';') && i + 1 < lines.len() {
                i += 1;
                stmt.push(' ');
                stmt.push_str(&lines[i]);
            }
            top.uses
                .push(stmt.split_whitespace().collect::<Vec<_>>().join(" "));
        } else if let Some(r) = l.strip_prefix("pub mod ").filter(|_| l.ends_with(';')) {
            top.mods.insert(ident(r));
            top.pub_mods.push(ident(r));
        } else if let Some(r) = l.strip_prefix("mod ").filter(|_| l.ends_with(';')) {
            top.mods.insert(ident(r));
        } else if let Some(r) = l
            .strip_prefix("pub struct ")
            .or_else(|| l.strip_prefix("pub enum "))
        {
            top.types.push((ident(r), i));
        }
        i += 1;
    }
    top
}

/// `pub use a::{B, c::D as E, f::*};` -> (a::B, "B"), (a::c::D, "E"), (a::f::*, "*").
fn use_items(stmt: &str) -> Vec<(Vec<String>, String)> {
    let stmt = &stmt[..stmt.find(';').unwrap_or(stmt.len())];
    let body: String = stmt
        .trim_start_matches("pub use ")
        .split_whitespace()
        .map(|t| if t == "as" { " as " } else { t })
        .collect();
    let mut out = vec![];
    expand(&[], &body, &mut out);
    out
}

fn expand(prefix: &[String], s: &str, out: &mut Vec<(Vec<String>, String)>) {
    if let (Some(open), true) = (s.find('{'), s.ends_with('}')) {
        let mut pre = prefix.to_vec();
        pre.extend(
            s[..open]
                .split("::")
                .filter(|x| !x.is_empty())
                .map(str::to_string),
        );
        let inner = &s[open + 1..s.len() - 1];
        let (mut depth, mut start) = (0, 0);
        for (i, c) in inner.char_indices().chain([(inner.len(), ',')]) {
            match c {
                '{' => depth += 1,
                '}' => depth -= 1,
                ',' if depth == 0 => {
                    if !inner[start..i].is_empty() {
                        expand(&pre, &inner[start..i], out);
                    }
                    start = i + 1;
                }
                _ => {}
            }
        }
        return;
    }
    let (path, alias) = s
        .split_once(" as ")
        .map_or((s, None), |(p, a)| (p, Some(a)));
    let mut segs = prefix.to_vec();
    segs.extend(path.split("::").map(str::to_string));
    let last = segs.last().cloned().unwrap_or_default();
    if last != "self" {
        out.push((segs, alias.map_or(last, str::to_string)));
    }
}

impl Scan {
    fn file(&self, m: &[String]) -> Option<PathBuf> {
        if m.is_empty() {
            return Some(self.src.join("lib.rs"));
        }
        let rel: PathBuf = m.iter().collect();
        [
            self.src.join(&rel).with_extension("rs"),
            self.src.join(&rel).join("mod.rs"),
        ]
        .into_iter()
        .find(|p| p.is_file())
    }

    /// Record every type module `m` makes public: its definitions, `pub use`s and `pub mod`s.
    fn module(&mut self, m: &[String], depth: usize) {
        assert!(depth < 16, "module nesting too deep at {m:?}");
        let file = self.file(m).unwrap_or_else(|| {
            panic!(
                "glob / pub mod over {m:?}: no module file under {}",
                self.src.display()
            )
        });
        let top = top_items(&lines_of(&file));
        for (name, line) in top.types {
            self.defs.insert((file.clone(), line), name);
        }
        for stmt in &top.uses {
            for (segs, _) in use_items(stmt) {
                assert!(
                    self.follow(m, &top.mods, &segs, depth),
                    "{}: cannot follow `pub use {}`",
                    file.display(),
                    segs.join("::")
                );
            }
        }
        for sub in top.pub_mods {
            self.module(&[m, &[sub]].concat(), depth + 1);
        }
    }

    /// The module a `use` path written in `m` starts from, and the rest of the
    /// path; `None` for a path into another crate.
    fn base<'a>(
        m: &[String],
        mods: &BTreeSet<String>,
        segs: &'a [String],
    ) -> Option<(Vec<String>, &'a [String])> {
        let mut base = m.to_vec();
        let mut rest = segs;
        match rest.first().map(String::as_str) {
            Some("crate") => {
                base.clear();
                rest = &rest[1..];
            }
            Some("self") => rest = &rest[1..],
            Some("super") => {}
            Some(s) if mods.contains(s) => {}
            _ => return None,
        }
        while rest.first().map(String::as_str) == Some("super") {
            base.pop();
            rest = &rest[1..];
        }
        Some((base, rest))
    }

    /// Follow one `pub use` path written in module `m`. False when it names nothing.
    fn follow(
        &mut self,
        m: &[String],
        mods: &BTreeSet<String>,
        segs: &[String],
        depth: usize,
    ) -> bool {
        assert!(depth < 16, "pub use chain too deep at {m:?} -> {segs:?}");
        let Some((base, rest)) = Self::base(m, mods, segs) else {
            self.foreign.insert(segs.join("::"));
            return true;
        };
        let Some((item, path)) = rest.split_last() else {
            return false;
        };
        let target = [base.as_slice(), path].concat();
        if item == "*" {
            self.module(&target, depth + 1);
            return true;
        }
        self.item(&target, item, depth + 1)
    }

    /// Resolve `name` in module `m`: a type definition (recorded), a re-export
    /// (followed), or a non-type item (ignored). False when `m` has no such name.
    fn item(&mut self, m: &[String], name: &str, depth: usize) -> bool {
        let Some(file) = self.file(m) else {
            return false;
        };
        let lines = lines_of(&file);
        let top = top_items(&lines);
        if let Some((_, line)) = top.types.iter().find(|(n, _)| n == name) {
            self.defs.insert((file, *line), name.to_string());
            return true;
        }
        for stmt in &top.uses {
            for (segs, exported) in use_items(stmt) {
                if exported == name {
                    return self.follow(m, &top.mods, &segs, depth);
                }
                if exported == "*" {
                    if let Some((base, rest)) = Self::base(m, &top.mods, &segs) {
                        let glob = [base.as_slice(), &rest[..rest.len() - 1]].concat();
                        if self.item(&glob, name, depth + 1) {
                            return true;
                        }
                    }
                }
            }
        }
        let kinds = [
            "fn",
            "const fn",
            "async fn",
            "unsafe fn",
            "const",
            "static",
            "trait",
            "type",
            "mod",
        ];
        lines.iter().any(|l| {
            kinds.iter().any(|k| {
                l.strip_prefix(&format!("pub {k} "))
                    .is_some_and(|r| ident(r) == name)
            })
        })
    }
}

/// Attribute and comment lines directly above line `i`, multi-line attributes included.
fn attrs_above(lines: &[String], i: usize) -> Vec<&str> {
    let (mut out, mut j, mut open) = (vec![], i, false);
    while j > 0 {
        j -= 1;
        let t = lines[j].trim();
        if open {
            open = !t.starts_with("#[");
        } else if t == ")]" || t == "]" {
            open = true;
        } else if !(t.starts_with("#[") || t.starts_with("//")) {
            break;
        }
        out.push(t);
    }
    out
}

/// Does the struct at line `i` have a field without a bare `pub`?
fn has_private_field(lines: &[String], i: usize) -> bool {
    let head = lines[i].trim_end();
    if let Some(open) = head.find('(').filter(|o| !head[..*o].contains('{')) {
        let mut text = head[open + 1..].to_string();
        let mut k = i;
        while !text.contains(");") && k + 1 < lines.len() {
            k += 1;
            text.push_str(&lines[k]);
        }
        let inner = &text[..text.rfind(')').unwrap_or(text.len())];
        return split_top(inner)
            .iter()
            .any(|f| !f.trim().is_empty() && !f.trim().starts_with("pub "));
    }
    if head.ends_with(';') || head.ends_with("{}") {
        return false;
    }
    let Some(open) = (i..lines.len()).find(|k| lines[*k].trim_end().ends_with('{')) else {
        return false;
    };
    lines[open + 1..]
        .iter()
        .take_while(|l| l.as_str() != "}")
        .filter(|l| {
            l.starts_with("    ") && l[4..].starts_with(|c: char| c.is_alphabetic() || c == '_')
        })
        .any(|l| !l.trim_start().starts_with("pub "))
}

fn split_top(s: &str) -> Vec<String> {
    let (mut out, mut cur, mut depth) = (vec![], String::new(), 0i32);
    for c in s.chars() {
        match c {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                out.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    out.push(cur);
    out
}

fn all_sources(dir: &Path, out: &mut String) {
    for e in fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()))
        .flatten()
    {
        let p = e.path();
        if p.is_dir() {
            all_sources(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push_str(&fs::read_to_string(&p).unwrap_or_default());
        }
    }
}

#[test]
fn facade_types_are_non_exhaustive() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let (mut offenders, mut seen, mut foreign, mut resolvers) =
        (vec![], BTreeSet::new(), BTreeSet::new(), 0);
    for krate in ["engine", "graph"] {
        let src = root.join(krate).join("src");
        let mut scan = Scan {
            src: src.clone(),
            defs: BTreeMap::new(),
            foreign: BTreeSet::new(),
        };
        let top = top_items(&lines_of(&src.join("lib.rs")));
        for stmt in &top.uses {
            for (segs, _) in use_items(stmt) {
                assert!(
                    scan.follow(&[], &top.mods, &segs, 0),
                    "{krate}/src/lib.rs: cannot follow `pub use {}`",
                    segs.join("::")
                );
            }
        }
        for m in top.pub_mods {
            scan.module(&[m], 1);
        }
        let mut code = String::new();
        all_sources(&src, &mut code);
        foreign.extend(scan.foreign);
        for ((file, line), name) in &scan.defs {
            let lines = lines_of(file);
            let head = lines[*line].trim_end();
            let is_struct = head.starts_with("pub struct ");
            let marked = attrs_above(&lines, *line).contains(&"#[non_exhaustive]");
            let private = is_struct && has_private_field(&lines, *line);
            let allowed = ALLOWLIST.iter().find(|(n, _)| n == name);
            let at = format!(
                "{}:{}",
                file.strip_prefix(&root).unwrap_or(file).display(),
                line + 1
            );
            seen.insert(name.clone());
            if is_struct
                && head.ends_with(';')
                && !head.contains('(')
                && code.contains(&format!("impl CrossGraphResolver for {name} "))
            {
                resolvers += 1;
            } else if let Some((_, why)) = allowed.filter(|_| marked || private) {
                offenders.push(format!(
                    "  {at}  {name} is ALLOWLISTED ({why}) but already passes: delete its entry"
                ));
            } else if !(marked || private || allowed.is_some()) {
                offenders.push(format!("  {at}  {}", head.trim_end_matches([' ', '{'])));
            }
        }
    }
    for (name, _) in ALLOWLIST {
        if !seen.contains(*name) {
            offenders.push(format!(
                "  ALLOWLIST names `{name}`, which the facade no longer exports: delete its entry"
            ));
        }
    }
    let lost: Vec<_> = ANCHORS.iter().filter(|a| !seen.contains(**a)).collect();
    assert!(
        lost.is_empty(),
        "the scan lost the anchor types {lost:?}: the facade's shape moved under the lint"
    );
    eprintln!(
        "[api_stability] scan types={} unit_resolvers={resolvers} allowlisted={} offenders={} foreign_skipped={foreign:?}",
        seen.len(),
        ALLOWLIST.len(),
        offenders.len()
    );
    assert!(
        offenders.is_empty(),
        "{} facade type(s) break a caller when they grow. Add #[non_exhaustive] (and derive Default if a caller \
         builds one), give it a private field, or add an ALLOWLIST entry with its reason:\n{}",
        offenders.len(),
        offenders.join("\n")
    );
}

#[test]
fn scanner_reads_the_shapes_it_gates() {
    let src: Vec<String> = [
        "#[derive(Debug)]",
        "#[non_exhaustive]",
        "pub struct Marked {",
        "    pub a: u8,",
        "}",
        "#[derive(",
        "    Debug,",
        ")]",
        "pub struct Hidden {",
        "    pub a: Vec<",
        "        u8,",
        "    >,",
        "    b: u8,",
        "}",
        "pub struct Crate {",
        "    #[serde(skip)]",
        "    /// doc",
        "    pub a: u8,",
        "    pub(crate) b: u8,",
        "}",
        "pub struct Open {",
        "    pub a: Vec<",
        "        u8,",
        "    >,",
        "}",
        "pub struct Tuple(pub u8, u8);",
        "pub struct Unit;",
        "pub use a::{B, c::{D as E, f::*}, self};",
        "pub use passes::classify;",
    ]
    .map(str::to_string)
    .to_vec();
    let top = top_items(&src);
    let names: Vec<_> = top.types.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        ["Marked", "Hidden", "Crate", "Open", "Tuple", "Unit"]
    );
    assert!(attrs_above(&src, 2).contains(&"#[non_exhaustive]"));
    assert_eq!(attrs_above(&src, 8), [")]", "Debug,", "#[derive("]);
    let private: Vec<_> = top
        .types
        .iter()
        .map(|(_, i)| has_private_field(&src, *i))
        .collect();
    assert_eq!(private, [false, true, true, false, true, false]);
    let items: Vec<_> = top
        .uses
        .iter()
        .flat_map(|u| use_items(u))
        .map(|(s, n)| format!("{}={n}", s.join("::")))
        .collect();
    assert_eq!(
        items,
        [
            "a::B=B",
            "a::c::D=E",
            "a::c::f::*=*",
            "passes::classify=classify"
        ]
    );
}
