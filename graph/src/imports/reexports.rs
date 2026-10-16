//! DT.1q: TS / JS re-export barrels, resolved as ECMA-262 ResolveExport
//! (16.2.1.6.3) over the export facts the TS parser records on each file
//! MODULE ([`NavFact::Exports`], [`NavFact::ReExportStar`],
//! [`NavFact::ReExportNamespace`], [`NavFact::ReExportName`],
//! [`NavFact::ExportLocal`]).
//!
//! A barrel is a MODULE with any re-export or from-less `export { .. }`
//! fact. [`Barrels::read`] gives every barrel one IMPORTS edge (rule
//! [`REEXPORT`]) per statement to each in-repo module it re-exports from,
//! and [`Barrels::materialize`] resolves every name a barrel exports:
//!
//! - a same-name re-exported declaration (`export * from './user.model'`
//!   passing `User`) joins the barrel's `module_symbols`, the table a named
//!   import and `ns.f()` read. Its `(name, id)` pair already sits in the
//!   declaring module's table, so the name-only fallbacks
//!   (`unique_global_function` / `unique_global_type`) answer as before;
//! - a renamed declaration (`export { AuditLog as Journal }`), a MODULE
//!   (`export * as fmt from`, a default re-export) and a `default` entry go
//!   to a build-time side table only the import arm reads, so a renamed name
//!   never answers a name guess for an unrelated `Journal`.
//!
//! The resolution order is the standard's: the module's own exports (every
//! name its `export` declarations export, node or not), then its from-less
//! and indirect entries, then its stars in statement order. A star never
//! supplies `default`; two stars resolving to distinct bindings are
//! ambiguous, and so is every barrel that stars the ambiguous one. An export
//! that exists but names nothing this graph binds (a type alias, a nodeless
//! const, `export { X } from 'some-lib'`) resolves [`Res::Opaque`]: it
//! shadows the stars exactly as a node would, and binds nothing. The path
//! set ends a cycle and [`MAX_DEPTH`] ends a chain.
//!
//! Build-local: rebuilt by every `resolve_imports_ts` call, never stored.
//! Barrels are read in `g.nodes` order and facts in record order; the
//! HashMaps are lookup-only, so the result does not depend on parse order.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use glia_code_domain::{ImportStmt, ImportTarget, NavFact, node_kind};
use glia_core::NodeId;

use super::{SameStem, import_edge, spec_ext, ts_target};
use crate::types::RepoGraph;

/// DT.1q: the IMPORTS evidence rule of a barrel -> each module it
/// re-exports from.
pub(super) const REEXPORT: &str = "reexport";

/// DT.1q: the deepest re-export chain one name is resolved through; a longer
/// one resolves to nothing (counted `capped`).
const MAX_DEPTH: usize = 32;

/// The ES export name of a default export, which no star re-exports.
const DEFAULT: &str = "default";

/// What a re-export or import specifier names.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Src {
    /// An in-repo MODULE.
    Module(NodeId),
    /// No in-repo module: the specifier as written (an npm package, or a
    /// path no file of the graph holds).
    External(String),
}

/// One indirect export entry of a module.
#[derive(Debug, Clone)]
enum Indirect {
    /// `export { <name> as <exported> } from '<src>'`.
    Name(Src, String),
    /// `export * as <exported> from '<src>'`.
    Namespace(Src),
}

/// What one of a module's own imports binds a local name to.
#[derive(Debug, Clone)]
enum Imported {
    /// `import { <name> as <local> } from '<src>'` (`default` for a default
    /// import).
    Name(Src, String),
    /// `import * as <local> from '<src>'`.
    Namespace(Src),
}

/// Where an [`Res::Opaque`] export lives: a module of this graph, or an
/// external specifier.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Origin {
    Module(NodeId),
    External(String),
}

/// ECMA-262 ResolveExport's answer for one (module, export name).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Res {
    /// The declaration (or MODULE: a namespace or default) the name binds.
    Found(NodeId),
    /// The name is exported, but binds nothing this graph has: a type alias,
    /// a nodeless const, an external module's export. Shadows the stars.
    Opaque(Origin, String),
    /// Two stars resolve the name to distinct bindings.
    Ambiguous,
    /// Not exported (or a cycle / the depth cap cut it).
    Null,
}

/// One module's export facts.
#[derive(Debug, Default)]
struct ModuleExports {
    /// Holds a re-export or from-less export fact.
    barrel: bool,
    /// [`NavFact::Exports`]: the names its own declarations export.
    own: BTreeSet<String>,
    /// [`NavFact::ExportLocal`]: exported name -> local name, first wins.
    local: BTreeMap<String, String>,
    /// [`NavFact::ReExportName`] / [`NavFact::ReExportNamespace`]: exported
    /// name -> entry, first wins.
    indirect: BTreeMap<String, Indirect>,
    /// [`NavFact::ReExportStar`] sources, in statement order.
    stars: Vec<Src>,
    /// The module's imports of the local names its [`NavFact::ExportLocal`]
    /// facts export: local name -> binding, the first statement wins.
    imports: BTreeMap<String, Imported>,
}

impl ModuleExports {
    /// The names of its from-less and indirect entries.
    fn entry_names(&self) -> impl Iterator<Item = &String> {
        self.local.keys().chain(self.indirect.keys())
    }

    /// The in-repo modules it stars, in statement order.
    fn star_modules(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.stars.iter().filter_map(|s| match s {
            Src::Module(id) => Some(*id),
            Src::External(_) => None,
        })
    }
}

/// DT.1q: what re-export resolution did in one graph build (the `[imports]
/// reexport` marker).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReexportStats {
    /// `export * from` statements.
    pub(crate) stars: usize,
    /// `export { a as b } from` specifiers.
    pub(crate) named: usize,
    /// `export * as ns from` statements.
    pub(crate) namespace: usize,
    /// From-less `export { a as b }` specifiers.
    pub(crate) local: usize,
    /// IMPORTS edges written (one per statement and in-repo source).
    pub(crate) edges: usize,
    /// Re-export statements whose source is no in-repo module.
    pub(crate) external: usize,
    /// Barrel names bound: same-name `module_symbols` rows, side-table rows
    /// and default entries.
    pub(crate) entries: usize,
    /// Of `entries`, renamed declarations (`export { A as B }`).
    pub(crate) aliased: usize,
    /// Named imports of a barrel bound through one of its entries.
    pub(crate) bound: usize,
    /// Named imports of a barrel that bind nothing.
    pub(crate) unbound: usize,
    /// Barrel names two stars resolve to distinct bindings.
    pub(crate) ambiguous: usize,
    /// Resolutions cut by the path set (and sources naming their own file).
    pub(crate) cycles: usize,
    /// Resolutions cut by [`MAX_DEPTH`].
    pub(crate) capped: usize,
}

/// DT.1q: one graph's re-export barrels, read off the TS parser's export
/// facts ([`Barrels::read`]) and resolved into its symbol tables
/// ([`Barrels::materialize`]).
#[derive(Debug, Default)]
pub(crate) struct Barrels {
    /// Every MODULE with an export fact (lookup only).
    modules: HashMap<NodeId, ModuleExports>,
    /// The barrels, in `g.nodes` order.
    order: Vec<NodeId>,
    /// Barrel -> the same-name re-exported declarations materialize added to
    /// its `module_symbols` (lookup only).
    added: HashMap<NodeId, BTreeMap<String, NodeId>>,
    /// Barrel -> its renamed and MODULE-valued entries, which stay out of
    /// `module_symbols` (lookup only).
    side: HashMap<NodeId, BTreeMap<String, NodeId>>,
    /// Barrel -> what its `default` entry binds (lookup only).
    default_entry: HashMap<NodeId, NodeId>,
    stats: ReexportStats,
}

impl Barrels {
    /// Read every MODULE's export facts in `g.nodes` order and give each
    /// barrel its IMPORTS edges (rule [`REEXPORT`], the statement's row, one
    /// per statement and source module). A source resolves through
    /// `resolve_source` and [`ts_target`], an import's own order: the file,
    /// its same-stem sibling, then a directory's index (DT.1b). A source
    /// naming its own file is dropped (a cycle); one naming no in-repo module
    /// is kept as external, with no edge: a named or namespace re-export of
    /// it still shadows the stars, and an external star is skipped. A graph
    /// with no such fact (Dart, Solidity, Terraform, Swift, C/C++) costs one
    /// scan of its nodes.
    pub(crate) fn read<R: Fn(&str, &str) -> Option<String>>(
        g: &mut RepoGraph,
        imports: &[ImportStmt],
        resolve_source: &R,
        same: &mut SameStem,
    ) -> Self {
        let mut b = Barrels::default();
        let mut seen: HashSet<NodeId> = HashSet::new();
        let mut with_facts: Vec<(NodeId, Vec<NavFact>)> = Vec::new();
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::MODULE) || !seen.insert(n.id) {
                continue;
            }
            let facts: Vec<NavFact> = g
                .nav
                .nav_facts
                .get(&n.id)
                .into_iter()
                .flatten()
                .filter(|f| is_export_fact(f))
                .cloned()
                .collect();
            if !facts.is_empty() {
                with_facts.push((n.id, facts));
            }
        }
        if with_facts.is_empty() {
            return b;
        }
        // The importer qnames whose imports a from-less export may name.
        let wants_imports: HashSet<String> = with_facts
            .iter()
            .filter(|(_, facts)| facts.iter().any(|f| matches!(f, NavFact::ExportLocal { .. })))
            .filter_map(|(m, _)| g.nav.qname_by_id.get(m).cloned())
            .collect();
        let mut imports_of: HashMap<&str, Vec<&ImportStmt>> = HashMap::new();
        for stmt in imports {
            if wants_imports.contains(&stmt.from_module) {
                imports_of.entry(stmt.from_module.as_str()).or_default().push(stmt);
            }
        }
        let mut edges: Vec<(NodeId, NodeId, u32)> = Vec::new();
        for (m, facts) in with_facts {
            let Some(m_q) = g.nav.qname_by_id.get(&m).cloned() else { continue };
            let mut e = ModuleExports::default();
            let mut statement_edges: BTreeSet<(u32, u64)> = BTreeSet::new();
            let mut external: BTreeSet<u32> = BTreeSet::new();
            for fact in facts {
                let (source, line) = match &fact {
                    NavFact::Exports { names } => {
                        e.own.extend(names.iter().cloned());
                        continue;
                    }
                    NavFact::ExportLocal { local, exported, .. } => {
                        e.barrel = true;
                        b.stats.local += 1;
                        e.local.entry(exported.clone()).or_insert_with(|| local.clone());
                        continue;
                    }
                    NavFact::ReExportStar { source, line }
                    | NavFact::ReExportNamespace { source, line, .. }
                    | NavFact::ReExportName { source, line, .. } => (source.clone(), *line),
                    _ => continue,
                };
                e.barrel = true;
                match &fact {
                    NavFact::ReExportStar { .. } => b.stats.stars += 1,
                    NavFact::ReExportNamespace { .. } => b.stats.namespace += 1,
                    _ => b.stats.named += 1,
                }
                let src = match source_module(g, same, m, &m_q, &source, resolve_source) {
                    Src::Module(s) if s == m => {
                        b.stats.cycles += 1;
                        continue;
                    }
                    Src::Module(s) => {
                        statement_edges.insert((line, s.0));
                        Src::Module(s)
                    }
                    ext @ Src::External(_) => {
                        external.insert(line);
                        ext
                    }
                };
                match fact {
                    NavFact::ReExportStar { .. } => e.stars.push(src),
                    NavFact::ReExportNamespace { exported, .. } => {
                        e.indirect.entry(exported).or_insert(Indirect::Namespace(src));
                    }
                    NavFact::ReExportName { name, exported, .. } => {
                        e.indirect.entry(exported).or_insert(Indirect::Name(src, name));
                    }
                    _ => {}
                }
            }
            b.stats.external += external.len();
            edges.extend(statement_edges.into_iter().map(|(line, s)| (m, NodeId(s), line)));
            if !e.local.is_empty() {
                let wanted: BTreeSet<&String> = e.local.values().collect();
                let mut bound: BTreeMap<String, Imported> = BTreeMap::new();
                for stmt in imports_of.get(m_q.as_str()).into_iter().flatten() {
                    let (local, spec, name) = match &stmt.target {
                        ImportTarget::Symbol { module, name, alias, .. } => {
                            (alias.clone().unwrap_or_else(|| name.clone()), module, Some(name))
                        }
                        ImportTarget::Module { path, alias: Some(a) } => (a.clone(), path, None),
                        ImportTarget::Module { alias: None, .. } => continue,
                    };
                    if !wanted.contains(&local) || bound.contains_key(&local) {
                        continue;
                    }
                    let src = source_module(g, same, m, &m_q, spec, resolve_source);
                    let imported = match name {
                        Some(n) => Imported::Name(src, n.clone()),
                        None => Imported::Namespace(src),
                    };
                    bound.insert(local, imported);
                }
                e.imports = bound;
            }
            if e.barrel {
                b.order.push(m);
            }
            b.modules.insert(m, e);
        }
        for (from, to, line) in edges {
            import_edge(g, from, to, REEXPORT, line);
            b.stats.edges += 1;
        }
        b
    }

    /// Resolve every name each barrel exports ([`Resolver::resolve`], from
    /// the pre-pass tables) and bind it: a same-name declaration joins the
    /// barrel's `module_symbols` (never overwriting a name it holds), a
    /// renamed declaration or a MODULE goes to the side table, and a barrel
    /// with a `default` entry records what it binds. A name the barrel's
    /// own `module_symbols` already holds is skipped: its own declarations
    /// win, exported or not, as a direct import already reads them.
    pub(crate) fn materialize(&mut self, g: &mut RepoGraph) {
        if self.order.is_empty() {
            return;
        }
        let mut memo: HashMap<(NodeId, String), (Res, usize)> = HashMap::new();
        let mut added: Vec<(NodeId, String, NodeId)> = Vec::new();
        let mut side: Vec<(NodeId, String, NodeId)> = Vec::new();
        let mut defaults: Vec<(NodeId, NodeId)> = Vec::new();
        let gr: &RepoGraph = g;
        let mut r = Resolver {
            g: gr,
            modules: &self.modules,
            memo: &mut memo,
            cycles: 0,
            capped: 0,
        };
        for &m in &self.order {
            let own = gr.symbols.module_symbols.get(&m);
            for n in candidates(&self.modules, m) {
                if own.is_some_and(|s| s.contains_key(&n)) {
                    continue;
                }
                match r.resolve_from(m, &n) {
                    Res::Found(id) => {
                        let decl = !matches!(
                            gr.nav.kind_by_id.get(&id).copied(),
                            Some(k) if k == node_kind::MODULE || k == node_kind::PACKAGE
                        );
                        if decl && gr.nav.name_by_id.get(&id) == Some(&n) {
                            added.push((m, n, id));
                        } else {
                            self.stats.aliased += usize::from(decl);
                            side.push((m, n, id));
                        }
                    }
                    Res::Ambiguous => self.stats.ambiguous += 1,
                    Res::Opaque(..) | Res::Null => {}
                }
            }
            let has_default = self.modules.get(&m).is_some_and(|e| {
                e.local.contains_key(DEFAULT) || e.indirect.contains_key(DEFAULT)
            });
            if has_default && let Res::Found(id) = r.resolve_from(m, DEFAULT) {
                defaults.push((m, id));
            }
        }
        self.stats.cycles += r.cycles;
        self.stats.capped += r.capped;
        self.stats.entries = added.len() + side.len() + defaults.len();
        for (m, n, id) in added {
            g.symbols.module_symbols.entry(m).or_default().entry(n.clone()).or_insert(id);
            self.added.entry(m).or_default().insert(n, id);
        }
        for (m, n, id) in side {
            self.side.entry(m).or_default().insert(n, id);
        }
        self.default_entry.extend(defaults);
    }

    /// A barrel's renamed or MODULE-valued entry `n` (the side table): what a
    /// named import of `n` from barrel `m` binds when `module_symbols` has
    /// no `n`.
    pub(crate) fn module_entry(&self, m: NodeId, n: &str) -> Option<NodeId> {
        self.side.get(&m)?.get(n).copied()
    }

    /// What barrel `m`'s `default` entry binds (`export { X as default }`,
    /// `export { default } from './x'`); `None` keeps the default-import
    /// convention (the MODULE itself).
    pub(crate) fn default_of(&self, m: NodeId) -> Option<NodeId> {
        self.default_entry.get(&m).copied()
    }

    /// Count one named import of `name` from `target` that bound `hit`, when
    /// `target` is a barrel: bound through one of its entries, or unbound.
    /// A hit on the barrel's own declaration counts as neither.
    pub(crate) fn tally(&mut self, target: NodeId, name: &str, hit: Option<NodeId>) {
        if !self.modules.get(&target).is_some_and(|e| e.barrel) {
            return;
        }
        let Some(_) = hit else {
            self.stats.unbound += 1;
            return;
        };
        let entry = if name == DEFAULT {
            self.default_entry.contains_key(&target)
        } else {
            self.added.get(&target).is_some_and(|t| t.contains_key(name))
                || self.side.get(&target).is_some_and(|t| t.contains_key(name))
        };
        self.stats.bound += usize::from(entry);
    }

    /// The counters the `[imports] reexport` marker prints.
    pub(crate) fn stats(&self) -> ReexportStats {
        self.stats
    }

    /// DT.1q fired_on, once per graph build that read a re-export fact
    /// ([`ReexportStats::marker`]).
    pub(crate) fn marker(&self) -> Option<String> {
        self.stats.marker()
    }
}

impl ReexportStats {
    /// DT.1q fired_on, once per graph build that read a re-export fact:
    ///   `[imports] reexport stars=<s> named=<n> namespace=<p> local=<l>
    ///   edges=<e> external=<x> entries=<t> aliased=<r> bound=<b>
    ///   unbound=<u> ambiguous=<a> cycles=<c> capped=<k>`
    pub(crate) fn marker(&self) -> Option<String> {
        let ReexportStats {
            stars,
            named,
            namespace,
            local,
            edges,
            external,
            entries,
            aliased,
            bound,
            unbound,
            ambiguous,
            cycles,
            capped,
        } = *self;
        (stars + named + namespace + local > 0).then(|| {
            format!(
                "[imports] reexport stars={stars} named={named} namespace={namespace} \
                 local={local} edges={edges} external={external} entries={entries} \
                 aliased={aliased} bound={bound} unbound={unbound} ambiguous={ambiguous} \
                 cycles={cycles} capped={capped}"
            )
        })
    }
}

/// The five DT.1q fact variants.
fn is_export_fact(f: &NavFact) -> bool {
    matches!(
        f,
        NavFact::Exports { .. }
            | NavFact::ReExportStar { .. }
            | NavFact::ReExportNamespace { .. }
            | NavFact::ReExportName { .. }
            | NavFact::ExportLocal { .. }
    )
}

/// The module specifier `spec` of a statement in MODULE `m` (qname `m_q`)
/// names: [`ts_target`] of `resolve_source`'s qname, as an import resolves
/// it, else [`Src::External`].
fn source_module<R: Fn(&str, &str) -> Option<String>>(
    g: &RepoGraph,
    same: &mut SameStem,
    m: NodeId,
    m_q: &str,
    spec: &str,
    resolve_source: &R,
) -> Src {
    resolve_source(m_q, spec)
        .and_then(|q| ts_target(g, same, &q, spec_ext(spec), m, "module"))
        .map_or_else(|| Src::External(spec.to_string()), |(id, _)| Src::Module(id))
}

/// Every name barrel `m` may export besides its own declarations: its entry
/// names, and the exported and entry names of every in-repo module its stars
/// reach (a visited set ends a star cycle). `default` excluded: no star
/// passes it, and a `default` entry is bound by the default-import arm.
fn candidates(modules: &HashMap<NodeId, ModuleExports>, m: NodeId) -> BTreeSet<String> {
    let mut out: BTreeSet<String> = BTreeSet::new();
    let Some(e) = modules.get(&m) else { return out };
    out.extend(e.entry_names().cloned());
    let mut visited: HashSet<NodeId> = HashSet::from([m]);
    let mut stack: Vec<NodeId> = e.star_modules().collect();
    while let Some(s) = stack.pop() {
        if !visited.insert(s) {
            continue;
        }
        let Some(se) = modules.get(&s) else { continue };
        out.extend(se.own.iter().cloned());
        out.extend(se.entry_names().cloned());
        stack.extend(se.star_modules());
    }
    out.remove(DEFAULT);
    out
}

/// One ResolveExport walk's context: the pre-pass tables, the memo of
/// results computed without a cut, and the cut counters.
struct Resolver<'a> {
    g: &'a RepoGraph,
    modules: &'a HashMap<NodeId, ModuleExports>,
    /// (module, name) -> (result, height): results computed with no cycle or
    /// depth cut, and how many levels below the module they reached. A hit
    /// is reused only where those levels still fit under [`MAX_DEPTH`], so a
    /// memoised answer is the one the walk would compute, whatever the order
    /// barrels are resolved in.
    memo: &'a mut HashMap<(NodeId, String), (Res, usize)>,
    cycles: usize,
    capped: usize,
}

impl Resolver<'_> {
    /// [`Self::resolve`] from an empty path.
    fn resolve_from(&mut self, m: NodeId, n: &str) -> Res {
        self.resolve(m, n, &mut HashSet::new(), 0).0
    }

    /// ECMA-262 ResolveExport(`m`, `n`), `path` the (module, name) pairs
    /// being resolved above this one (a pair met again is a cycle: Null) and
    /// `depth` their number (past [`MAX_DEPTH`]: Null). Returns the result
    /// and, when no cut was taken below, the height it reached.
    fn resolve(
        &mut self,
        m: NodeId,
        n: &str,
        path: &mut HashSet<(NodeId, String)>,
        depth: usize,
    ) -> (Res, Option<usize>) {
        let key = (m, n.to_string());
        if path.contains(&key) {
            self.cycles += 1;
            return (Res::Null, None);
        }
        if depth > MAX_DEPTH {
            self.capped += 1;
            return (Res::Null, None);
        }
        if let Some((res, height)) = self.memo.get(&key)
            && depth + height <= MAX_DEPTH
        {
            return (res.clone(), Some(*height));
        }
        path.insert(key.clone());
        let (res, clean) = self.resolve_in(m, n, path, depth);
        path.remove(&key);
        if let Some(height) = clean {
            self.memo.insert(key, (res.clone(), height));
        }
        (res, clean)
    }

    /// One resolution step of [`Self::resolve`]: local exports, then the
    /// from-less and indirect entries, then the stars.
    fn resolve_in(
        &mut self,
        m: NodeId,
        n: &str,
        path: &mut HashSet<(NodeId, String)>,
        depth: usize,
    ) -> (Res, Option<usize>) {
        let (g, modules) = (self.g, self.modules);
        let Some(e) = modules.get(&m) else {
            // A module with no export fact exports nothing.
            return (Res::Null, Some(0));
        };
        let symbol =
            |name: &str| g.symbols.module_symbols.get(&m).and_then(|s| s.get(name)).copied();
        // Local exports: the module's own `export` declarations.
        if e.own.contains(n) {
            let res = if n == DEFAULT {
                Res::Found(m)
            } else {
                symbol(n).map_or_else(|| Res::Opaque(Origin::Module(m), n.to_string()), Res::Found)
            };
            return (res, Some(0));
        }
        // From-less `export { local as n }`: an own declaration, else what the
        // module's import of `local` binds.
        if let Some(local) = e.local.get(n) {
            if let Some(id) = symbol(local) {
                return (Res::Found(id), Some(0));
            }
            return match e.imports.get(local) {
                Some(Imported::Name(Src::Module(s), name)) => {
                    let (res, clean) = self.resolve(*s, name, path, depth + 1);
                    (default_fallback(res, name, *s), clean.map(|h| h + 1))
                }
                Some(Imported::Name(Src::External(spec), name)) => {
                    (Res::Opaque(Origin::External(spec.clone()), name.clone()), Some(0))
                }
                Some(Imported::Namespace(Src::Module(s))) => (Res::Found(*s), Some(0)),
                Some(Imported::Namespace(Src::External(spec))) => {
                    (Res::Opaque(Origin::External(spec.clone()), "*".to_string()), Some(0))
                }
                None => (Res::Opaque(Origin::Module(m), n.to_string()), Some(0)),
            };
        }
        // Indirect: `export { name as n } from`, `export * as n from`.
        match e.indirect.get(n) {
            Some(Indirect::Name(Src::Module(s), name)) => {
                let (res, clean) = self.resolve(*s, name, path, depth + 1);
                return (default_fallback(res, name, *s), clean.map(|h| h + 1));
            }
            Some(Indirect::Name(Src::External(spec), name)) => {
                return (Res::Opaque(Origin::External(spec.clone()), name.clone()), Some(0));
            }
            Some(Indirect::Namespace(Src::Module(s))) => return (Res::Found(*s), Some(0)),
            Some(Indirect::Namespace(Src::External(spec))) => {
                return (Res::Opaque(Origin::External(spec.clone()), "*".to_string()), Some(0));
            }
            None => {}
        }
        if n == DEFAULT {
            return (Res::Null, Some(0));
        }
        // Stars, in statement order; an external star is skipped.
        let mut found: Option<Res> = None;
        let mut height = 0;
        let mut clean = true;
        for s in e.star_modules() {
            let (res, c) = self.resolve(s, n, path, depth + 1);
            match c {
                Some(h) => height = height.max(h + 1),
                None => clean = false,
            }
            match res {
                Res::Ambiguous => return (Res::Ambiguous, clean.then_some(height)),
                Res::Null => {}
                res => match &found {
                    None => found = Some(res),
                    Some(f) if *f == res => {}
                    Some(_) => return (Res::Ambiguous, clean.then_some(height)),
                },
            }
        }
        (found.unwrap_or(Res::Null), clean.then_some(height))
    }
}

/// A `default` that resolves to nothing binds its source MODULE, as a
/// direct `import X from './x'` does (glia's default-import convention):
/// `import X from './x'; export { X }` and `export { default as X } from
/// './x'` with no `export default` in `x` (a Vue `<script setup>`, a file
/// the parser reads no default of).
fn default_fallback(res: Res, name: &str, source: NodeId) -> Res {
    if res == Res::Null && name == DEFAULT {
        Res::Found(source)
    } else {
        res
    }
}
