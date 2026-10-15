//! Import resolution — one pass per import-path dialect (dotted / Go / slash /
//! TypeScript), all of them writing IMPORTS edges and module import bindings.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use glia_code_domain::{
    ImportStmt, ImportTarget, bare_module_qname, edge_category, evidence, node_kind,
    same_stem_order,
};
use glia_core::{NodeId, NodeKindId};

use crate::build::{DirImport, GoPackages};
use crate::calls::{graph_evidence, push_edge, unique_global_function, unique_global_module};
use crate::types::RepoGraph;

// ============================================================================
// Same-stem siblings (LB.13)
// ============================================================================

/// LB.13: the stems that name two or more file-named MODULEs of ONE graph
/// (`src/util.ts` + `src/util.js` are MODULEs `src::util.ts` / `src::util.js`,
/// both of bare form `src::util`), for which `build_symbol_table` registers
/// no bare alias, so an import can bind the sibling its importer's language
/// loads ([`same_stem_order`]).
///
/// Build-local: rebuilt by every builder call from the graph's own MODULEs and
/// never stored in the `SymbolTable`, so an incremental build and a clean one
/// pick alike. Order comes from `g.nodes` and the BTreeMap; the HashMap is
/// lookup-only.
#[derive(Debug, Default)]
pub(crate) struct SameStem {
    /// Bare form -> (file extension, MODULE id) of each sibling, in `g.nodes`
    /// order. Only stems with two or more siblings.
    by_bare: BTreeMap<String, Vec<(String, NodeId)>>,
    /// Every MODULE's own file extension, from its POSITION. Read the first
    /// time a pick needs an importer's, so a graph whose imports all bind
    /// exactly (C/C++ includes name the file) never reads one.
    module_ext: Option<HashMap<NodeId, String>>,
    stats: SameStemStats,
}

/// What [`SameStem`] did in one graph build (the `[imports] same-stem picks`
/// marker).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SameStemStats {
    /// Bound to the sibling the specifier's own extension names (`./util.js`).
    pub(crate) explicit: usize,
    /// Bound to the first sibling of the importer's resolution order.
    pub(crate) importer: usize,
    /// An import of an ambiguous stem left unbound: the importer's language
    /// has no file-import order (`.cljc`; Java / Kotlin imports that no
    /// class binds either), its MODULE has no POSITION, no sibling is in its
    /// order, or two stems end in the import's path.
    pub(crate) unresolved: usize,
}

/// What a same-stem lookup ([`SameStem::pick`]) found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SamePick {
    /// Not an ambiguous stem of this graph: resolution goes on as before.
    NotAStem,
    /// The sibling the specifier or the importer's language names.
    Bound(NodeId),
    /// An ambiguous stem no rule picks a sibling of. The import must not
    /// fall back to a same-named MODULE elsewhere (the pair's own qnames no
    /// longer end in the stem, so that module would look unique); the caller
    /// counts it with [`SameStem::unresolved`] if nothing else binds it.
    Refused,
}

/// The same-stem siblings of `g`, after `build_symbol_table`. Returns at once
/// with an empty table when no stem has two file-named MODULEs.
pub(crate) fn same_stem_table(g: &RepoGraph) -> SameStem {
    let mut by_bare: BTreeMap<String, Vec<(String, NodeId)>> = BTreeMap::new();
    for n in &g.nodes {
        if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::MODULE) {
            continue;
        }
        let (Some(qname), Some(name)) = (g.nav.qname_by_id.get(&n.id), g.nav.name_by_id.get(&n.id))
        else {
            continue;
        };
        let Some(bare) = bare_module_qname(qname, name) else { continue };
        let last = qname.rsplit("::").next().unwrap_or(qname);
        let Some((_, ext)) = last.rsplit_once('.') else { continue };
        let siblings = by_bare.entry(bare).or_default();
        // A MODULE recorded twice (two node records of one id) is one sibling.
        if !siblings.iter().any(|(_, id)| *id == n.id) {
            siblings.push((ext.to_string(), n.id));
        }
    }
    by_bare.retain(|_, siblings| siblings.len() > 1);
    SameStem { by_bare, ..SameStem::default() }
}

/// The extension of a file path's last component (`src/app.ts` -> `ts`).
fn file_ext(path: &str) -> Option<&str> {
    let file = path.rsplit(['/', '\\']).next().unwrap_or(path);
    file.rsplit_once('.').map(|(_, ext)| ext).filter(|e| !e.is_empty())
}

/// The extension an import specifier names itself (`./util.js` -> `js`,
/// `./util` -> None), quotes stripped as the TS source resolver strips them.
fn spec_ext(specifier: &str) -> Option<&str> {
    let spec = specifier.trim().trim_matches(|c| c == '"' || c == '\'');
    file_ext(spec)
}

impl SameStem {
    /// `bare` is the stem-form qname an import resolved to (`src::util`).
    /// The sibling whose extension the specifier names wins
    /// (`explicit_ext`, only when a sibling has it), else the first of the
    /// importer's own resolution order ([`same_stem_order`] of its MODULE's
    /// file extension).
    pub(crate) fn pick(
        &mut self,
        g: &RepoGraph,
        bare: &str,
        explicit_ext: Option<&str>,
        importer: NodeId,
    ) -> SamePick {
        let Some(siblings) = self.by_bare.get(bare) else {
            return SamePick::NotAStem;
        };
        if let Some(ext) = explicit_ext
            && let Some((_, id)) = siblings.iter().find(|(e, _)| e == ext)
        {
            self.stats.explicit += 1;
            return SamePick::Bound(*id);
        }
        let exts = self.module_ext.get_or_insert_with(|| module_exts(g));
        let order = exts.get(&importer).map_or(&[][..], |e| same_stem_order(e));
        let hit = order
            .iter()
            .find_map(|want| siblings.iter().find(|(e, _)| e == want).map(|(_, id)| *id));
        match hit {
            Some(id) => {
                self.stats.importer += 1;
                SamePick::Bound(id)
            }
            None => SamePick::Refused,
        }
    }

    /// [`Self::pick`] of `path_qname`, else of the one ambiguous stem ending
    /// in `::<path_qname>`: a dotted require that leaves out the directory
    /// (a Clojure `(:require [app.core])` is `app::core` while the file is
    /// `clj/app/core.clj`). Two such stems: [`SamePick::Refused`].
    pub(crate) fn pick_path(
        &mut self,
        g: &RepoGraph,
        path_qname: &str,
        importer: NodeId,
    ) -> SamePick {
        if self.by_bare.is_empty() {
            return SamePick::NotAStem;
        }
        let exact = self.pick(g, path_qname, None, importer);
        if exact != SamePick::NotAStem || path_qname.is_empty() {
            return exact;
        }
        let tail = format!("::{path_qname}");
        let mut stems = self.by_bare.keys().filter(|b| b.ends_with(&tail));
        let Some(stem) = stems.next().cloned() else {
            return SamePick::NotAStem;
        };
        if stems.next().is_some() {
            return SamePick::Refused;
        }
        self.pick(g, &stem, None, importer)
    }

    /// Count one import of an ambiguous stem that stayed unbound.
    pub(crate) fn unresolved(&mut self) {
        self.stats.unresolved += 1;
    }

    /// LB.13 fired_on, once per graph build that tried a same-stem pick:
    ///   `[imports] same-stem picks: explicit={e} importer={i} unresolved={u} (stems={s} exts={x,y})`
    /// `exts` (the siblings' extensions, sorted) tells the language graph.
    pub(crate) fn marker(&self) -> Option<String> {
        let SameStemStats { explicit, importer, unresolved } = self.stats;
        (explicit + importer + unresolved > 0).then(|| {
            let exts: BTreeSet<&str> = self
                .by_bare
                .values()
                .flatten()
                .map(|(e, _)| e.as_str())
                .collect();
            format!(
                "[imports] same-stem picks: explicit={explicit} importer={importer} \
                 unresolved={unresolved} (stems={} exts={})",
                self.by_bare.len(),
                exts.into_iter().collect::<Vec<_>>().join(",")
            )
        })
    }

    pub(crate) fn report(&self) {
        if let Some(line) = self.marker() {
            eprintln!("{line}");
        }
    }

    /// The file extension of MODULE `importer` (`php`), from the same
    /// lazily-read table [`Self::pick`] uses, so a graph reads every MODULE's
    /// POSITION at most once whichever step asks first.
    fn importer_ext(&mut self, g: &RepoGraph, importer: NodeId) -> Option<String> {
        self.module_ext.get_or_insert_with(|| module_exts(g)).get(&importer).cloned()
    }
}

// ============================================================================
// Namespace members (DF.2)
// ============================================================================

/// The node kinds a namespace member can be: what a symbol import names
/// (`use App\Billing\Invoicer`, `use function App\Util\fmt`, C#
/// `using static Shop.Core.Helpers`).
const MEMBER_KINDS: &[NodeKindId] = &[
    node_kind::CLASS,
    node_kind::INTERFACE,
    node_kind::ENUM,
    node_kind::STRUCT,
    node_kind::FUNCTION,
];

/// DF.2: every namespace PACKAGE of one graph with its member types and
/// functions, and the root namespaces the repo declares.
///
/// A PHP / C# namespace is a PACKAGE: each file opening `namespace
/// App\Billing;` has a CONTAINS edge to PACKAGE `App::Billing` and keeps its
/// top-level types itself (`module_symbols` of the file MODULE), while a
/// braced or block namespace's members are the PACKAGE's own nav children
/// (`module_symbols` of the PACKAGE). PSR-4 types are directory-scoped
/// (`src::Billing::Invoicer`, LB.7b), so no MODULE qname is the namespace a
/// `use` names and the import's module lookups miss; this index binds it by
/// the namespace instead of the repo-unique short name.
///
/// A file opening several statement-form namespaces (`namespace A; ...
/// namespace B; ...`, which PSR-4 forbids) lists its types under each: the
/// parser records no finer owner.
///
/// Build-local: rebuilt by every [`resolve_imports_python`] call that needs
/// it, never stored. A pure function of the graph: BTree containers, and a
/// member set is deduped by id (one type registered under its MODULE and
/// its PACKAGE is one member), so `g.edges` / HashMap order never shows.
#[derive(Debug, Default)]
pub(crate) struct NamespaceIndex {
    /// PACKAGE qname -> member name -> the distinct member ids. Every
    /// namespace PACKAGE of the graph has an entry, members or not, so a
    /// declared namespace without the name is told from an undeclared one.
    members: BTreeMap<String, BTreeMap<String, Vec<NodeId>>>,
    /// The first `::` segment of every PACKAGE qname (`App`): the root
    /// namespaces the repo owns.
    roots: BTreeSet<String>,
}

/// What [`NamespaceIndex::member`] found for `ns` + `name`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NsHit {
    /// The one member of the declared namespace named so.
    Unique(NodeId),
    /// Two distinct members of the namespace share the name (a class two
    /// files declare): nothing binds.
    Ambiguous,
    /// The namespace is declared and has no member of that name, nor a
    /// sub-namespace: the short-name guess could only pick another
    /// namespace's type.
    Missing,
    /// `ns::name` is itself a declared namespace: a namespace import (C#
    /// `using Shop.Core;`, PHP `use App\Billing;`), resolved as before.
    Namespace,
    /// No PACKAGE has the qname `ns`.
    NotDeclared,
}

impl NamespaceIndex {
    pub(crate) fn build(g: &RepoGraph) -> Self {
        let mut idx = NamespaceIndex::default();
        let is_package = |id: &NodeId| g.nav.kind_by_id.get(id) == Some(&node_kind::PACKAGE);
        // Every PACKAGE is a namespace with its own nav children as members.
        let mut scopes: Vec<(NodeId, NodeId)> = g
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::PACKAGE)
            .map(|(id, _)| (*id, *id))
            .collect();
        // A file opening the namespace contributes its own top-level members.
        scopes.extend(
            g.edges
                .iter()
                .filter(|e| {
                    e.category == edge_category::CONTAINS
                        && is_package(&e.to)
                        && g.nav.kind_by_id.get(&e.from) == Some(&node_kind::MODULE)
                })
                .map(|e| (e.to, e.from)),
        );
        for (package, scope) in scopes {
            let Some(qname) = g.nav.qname_by_id.get(&package) else { continue };
            let root = qname.split("::").next().unwrap_or(qname);
            idx.roots.insert(root.to_string());
            let members = idx.members.entry(qname.clone()).or_default();
            let Some(symbols) = g.symbols.module_symbols.get(&scope) else { continue };
            for (name, id) in symbols {
                let member_kind = g.nav.kind_by_id.get(id).is_some_and(|k| MEMBER_KINDS.contains(k));
                if !member_kind {
                    continue;
                }
                let ids = members.entry(name.clone()).or_default();
                if !ids.contains(id) {
                    ids.push(*id);
                }
            }
        }
        idx
    }

    /// The member `name` of namespace `ns` (a `::` qname).
    pub(crate) fn member(&self, ns: &str, name: &str) -> NsHit {
        let nested = if ns.is_empty() { name.to_string() } else { format!("{ns}::{name}") };
        let Some(members) = self.members.get(ns) else {
            return if self.members.contains_key(&nested) {
                NsHit::Namespace
            } else {
                NsHit::NotDeclared
            };
        };
        match members.get(name).map(Vec::as_slice) {
            Some([id]) => NsHit::Unique(*id),
            Some([_, _, ..]) => NsHit::Ambiguous,
            _ if self.members.contains_key(&nested) => NsHit::Namespace,
            _ => NsHit::Missing,
        }
    }

    /// Whether `ns` lies under a root namespace some file of the repo
    /// declares (`App::Legal` when `App::Billing` is declared).
    pub(crate) fn owns_root(&self, ns: &str) -> bool {
        let root = ns.split("::").next().unwrap_or(ns);
        !root.is_empty() && self.roots.contains(root)
    }
}

/// What the namespace step did in one [`resolve_imports_python`] call (the
/// `[imports] namespace symbols:` marker).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct NamespaceStats {
    /// Bound to the member of the declared namespace (rule `namespace`).
    pub(crate) bound: usize,
    /// The declared namespace has two members of the name: left unbound.
    pub(crate) ambiguous: usize,
    /// The declared namespace has no member of the name: left unbound.
    pub(crate) missing: usize,
    /// A `.php` import into an undeclared namespace under a root namespace
    /// the repo declares: left unbound, never the short-name guess.
    pub(crate) refused_owned_root: usize,
    /// The importers' file extensions, for the marker's discriminator.
    pub(crate) exts: BTreeSet<String>,
}

/// What the namespace step decided for one symbol import.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NsStep {
    /// Bind this member, rule `namespace`.
    Bind(NodeId),
    /// Bind nothing: no short-name guess either.
    Refuse,
    /// Not the namespace step's: the short-name fallback runs as before.
    Fallthrough,
}

/// The namespace step of a symbol import, built lazily: a graph whose
/// symbol imports all bind by module never builds the index.
#[derive(Debug, Default)]
struct Namespaces {
    index: Option<NamespaceIndex>,
    stats: NamespaceStats,
}

impl Namespaces {
    /// `ns` is the import's resolved module qname, `importer` its file MODULE.
    fn step(
        &mut self,
        g: &RepoGraph,
        same: &mut SameStem,
        ns: &str,
        name: &str,
        importer: NodeId,
    ) -> NsStep {
        let idx = self.index.get_or_insert_with(|| NamespaceIndex::build(g));
        let hit = idx.member(ns, name);
        let step = match hit {
            NsHit::Unique(id) => {
                self.stats.bound += 1;
                NsStep::Bind(id)
            }
            NsHit::Ambiguous => {
                self.stats.ambiguous += 1;
                NsStep::Refuse
            }
            NsHit::Missing => {
                self.stats.missing += 1;
                NsStep::Refuse
            }
            NsHit::Namespace => NsStep::Fallthrough,
            NsHit::NotDeclared => {
                if idx.owns_root(ns) && same.importer_ext(g, importer).as_deref() == Some("php") {
                    self.stats.refused_owned_root += 1;
                    NsStep::Refuse
                } else {
                    NsStep::Fallthrough
                }
            }
        };
        if step != NsStep::Fallthrough {
            let ext = same.importer_ext(g, importer).unwrap_or_else(|| "?".to_string());
            self.stats.exts.insert(ext);
        }
        step
    }

    /// DF.2 fired_on, once per [`resolve_imports_python`] call in which the
    /// namespace step bound or refused anything:
    ///   `[imports] namespace symbols: bound={b} ambiguous={a} missing={m} refused_owned_root={r} (exts={x,y})`
    /// `exts` (the importers' file extensions, sorted) tells PHP from C#.
    fn marker(&self) -> Option<String> {
        let NamespaceStats { bound, ambiguous, missing, refused_owned_root, exts } = &self.stats;
        (bound + ambiguous + missing + refused_owned_root > 0).then(|| {
            format!(
                "[imports] namespace symbols: bound={bound} ambiguous={ambiguous} \
                 missing={missing} refused_owned_root={refused_owned_root} (exts={})",
                exts.iter().map(String::as_str).collect::<Vec<_>>().join(",")
            )
        })
    }
}

/// Every MODULE's file extension, from its POSITION ([`evidence::locate`]).
fn module_exts(g: &RepoGraph) -> HashMap<NodeId, String> {
    let mut out = HashMap::new();
    for n in &g.nodes {
        if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::MODULE) || out.contains_key(&n.id) {
            continue;
        }
        if let Some((file, _)) = evidence::locate(&n.cells)
            && let Some(ext) = file_ext(&file)
        {
            out.insert(n.id, ext.to_string());
        }
    }
    out
}

// ============================================================================
// Import resolution
// ============================================================================

/// Dotted imports (Python, and every `build_dotted` language). LB.13: a
/// module path that misses exactly is tried as an ambiguous same-stem pair,
/// also one whose directory the path leaves out (a Clojure
/// `(:require [app.core])`, [`SameStem::pick_path`]), before the tail
/// fallback; a pair no rule picks from keeps the tail's class guess but not
/// its module guess, which would bind a lone same-named module elsewhere once
/// the pair's own qnames stop ending in the stem.
///
/// DF.2: a symbol import is tried, in order, as a submodule, a symbol of its
/// module, a symbol of a same-stem sibling, a member of its namespace
/// PACKAGE ([`NamespaceIndex`], rule `namespace`), and last the repo-unique
/// short name (`tail_unique`). A declared namespace without a unique member
/// of the name binds nothing, and so does a `.php` import into an undeclared
/// namespace under a root namespace the repo declares (`use App\Legal\X`
/// beside `namespace App\Billing`); a vendor root (`Illuminate\..`) and
/// every language without namespace PACKAGEs keep the short-name fallback.
pub(crate) fn resolve_imports_python(
    g: &mut RepoGraph,
    imports: &[ImportStmt],
    same: &mut SameStem,
) {
    let mut ns = Namespaces::default();
    for stmt in imports {
        let Some(from_mod_id) = g
            .symbols
            .module_by_qname
            .get(&stmt.from_module)
            .copied()
        else {
            continue;
        };

        match &stmt.target {
            ImportTarget::Module { path, alias } => {
                // `import foo.bar` — convert `.` → `::` and look up by qname.
                let target_qname = path.replace('.', "::");
                let (found, same_pick) = match g.symbols.module_by_qname.get(&target_qname) {
                    Some(id) => (Some((*id, "module")), SamePick::NotAStem),
                    None => match same.pick_path(g, &target_qname, from_mod_id) {
                        SamePick::Bound(id) => (Some((id, "same_stem")), SamePick::NotAStem),
                        other => (None, other),
                    },
                };
                if let Some((target_id, rule)) = found {
                    import_edge(g, from_mod_id, target_id, rule, stmt.line);
                    let bound_name = alias.clone().unwrap_or_else(|| {
                        path.split('.').next().unwrap_or(path).to_string()
                    });
                    g.symbols
                        .module_import_bindings
                        .entry(from_mod_id)
                        .or_default()
                        .insert(bound_name, target_id);
                } else {
                    // Tail fallback (Pattern B): `import com.example.util.Helper`
                    // (class) or `require app.util` (module) where the file layout
                    // doesn't mirror the package path, so the full qname misses.
                    // Bind the final segment — as a unique global class/interface
                    // (module_symbols, incl. PACKAGE members) or a unique module by
                    // short name. Miss-only + ambiguity-safe (returns None on tie).
                    // LB.13: a path naming an ambiguous same-stem pair no rule
                    // picks from keeps the class guess (`import shop.Foo` beside
                    // Foo.java + Foo.kt binds CLASS Foo) but never the module
                    // one: a lone `core` elsewhere is not the pair's `core`.
                    let tail = path
                        .rsplit(|c| c == '.' || c == ':' || c == '/')
                        .next()
                        .unwrap_or(path);
                    let refused = same_pick == SamePick::Refused;
                    let guess = unique_global_function(g, tail)
                        .or_else(|| (!refused).then(|| unique_global_module(g, tail)).flatten());
                    if let Some(target_id) = guess {
                        import_edge(g, from_mod_id, target_id, "tail_unique", stmt.line);
                        g.symbols
                            .module_import_bindings
                            .entry(from_mod_id)
                            .or_default()
                            .insert(alias.clone().unwrap_or_else(|| tail.to_string()), target_id);
                    } else if refused {
                        same.unresolved();
                    }
                }
            }
            ImportTarget::Symbol { module, name, alias, level } => {
                let target_module_qname = resolve_module_reference(&stmt.from_module, module, *level);

                // Try `module::name` as a submodule first — matches Python's
                // `from pkg import mod` → edge to pkg.mod if submodule exists.
                let submodule_qname = if target_module_qname.is_empty() {
                    name.clone()
                } else {
                    format!("{target_module_qname}::{name}")
                };

                let bound = alias.clone().unwrap_or_else(|| name.clone());
                let mut sym_refused = false;

                if let Some(submodule_id) = g.symbols.module_by_qname.get(&submodule_qname).copied()
                {
                    // `from pkg import mod` where mod is a submodule.
                    import_edge(g, from_mod_id, submodule_id, "submodule", stmt.line);
                    g.symbols
                        .module_import_bindings
                        .entry(from_mod_id)
                        .or_default()
                        .insert(bound, submodule_id);
                } else if let Some((target_mod_id, rule)) = match g
                    .symbols
                    .module_by_qname
                    .get(&target_module_qname)
                {
                    Some(id) => Some((*id, "symbol")),
                    // LB.13: the module is an ambiguous same-stem pair; the
                    // name binds inside the sibling picked.
                    None => match same.pick_path(g, &target_module_qname, from_mod_id) {
                        SamePick::Bound(id) => Some((id, "same_stem")),
                        SamePick::Refused => {
                            sym_refused = true;
                            None
                        }
                        SamePick::NotAStem => None,
                    },
                } {
                    // `from pkg.mod import Name` — target is a symbol inside pkg.mod.
                    import_edge(g, from_mod_id, target_mod_id, rule, stmt.line);
                    if let Some(symbol_id) = g
                        .symbols
                        .module_symbols
                        .get(&target_mod_id)
                        .and_then(|t| t.get(name))
                        .copied()
                    {
                        g.symbols
                            .module_import_bindings
                            .entry(from_mod_id)
                            .or_default()
                            .insert(bound, symbol_id);
                    }
                } else {
                    // DF.2: the module is a declared namespace PACKAGE (PHP /
                    // C#): bind its member, never another namespace's
                    // same-named type. A same-stem pair no rule picked from
                    // keeps the tail's class guess, as before.
                    let step = if sym_refused {
                        NsStep::Fallthrough
                    } else {
                        ns.step(g, same, &target_module_qname, name, from_mod_id)
                    };
                    let (target, rule) = match step {
                        NsStep::Bind(id) => (Some(id), "namespace"),
                        NsStep::Refuse => (None, "namespace"),
                        // Tail fallback (Pattern B): `from a.b import Name`
                        // where `a.b` isn't a resolvable module (flat layout)
                        // — bind the imported symbol by its unique global
                        // name. Miss-only + ambiguity-safe.
                        NsStep::Fallthrough => (unique_global_function(g, name), "tail_unique"),
                    };
                    if let Some(symbol_id) = target {
                        import_edge(g, from_mod_id, symbol_id, rule, stmt.line);
                        g.symbols
                            .module_import_bindings
                            .entry(from_mod_id)
                            .or_default()
                            .insert(bound, symbol_id);
                    } else if sym_refused {
                        same.unresolved();
                    }
                }
            }
        }
    }
    if let Some(line) = ns.marker() {
        eprintln!("{line}");
    }
}

/// Go imports: the parser has already stripped the go.mod prefix and produced
/// `ImportTarget::Module { path }` with `path` = repo-local `::` qname for
/// imports that resolve inside this module. External imports keep the raw
/// `std::io`-style form and won't match anything.
///
/// An import path names a package, which is a DIRECTORY, while every Go file
/// is its own MODULE (`internal/store/store.go` -> `internal::store::store`).
/// So the lookup order is: a MODULE whose qname is the path (unchanged from
/// before LA.13b), then the package directory `path` itself, bound to one of
/// its files ([`GoPackages::import_target`]; the call-level hook reaches the
/// package's other files), then the tail fallback. A directory whose only
/// file is the importer binds nothing and skips the tail fallback, which
/// could only guess a same-named package elsewhere.
///
/// CI.3: an empty path is the repository-root package (dir `""`). It binds
/// the root dir's first non-test file by qname (no file is named after the
/// root dir), rule `package_dir`; with no root `.go` file it binds nothing
/// (the tail fallback has no name to look up).
///
/// DF.15: the importer binds an aliased import under its alias, an
/// un-aliased one under the imported package's `package` clause
/// ([`GoPackages::binding_name`]: `store` for `internal/v2store` declaring
/// `package store`, `nats` for a root package of module `.../nats.go`),
/// else the path's last segment; a root package with no clause binds no
/// name.
///
/// Returns what the `[go-package]` markers print (LA.13b, CI.3).
pub(crate) fn resolve_imports_go(
    g: &mut RepoGraph,
    imports: &[ImportStmt],
    packages: &GoPackages,
) -> GoImportStats {
    let mut stats = GoImportStats::default();
    for stmt in imports {
        let Some(from_mod_id) = g
            .symbols
            .module_by_qname
            .get(&stmt.from_module)
            .copied()
        else {
            continue;
        };
        let ImportTarget::Module { path, alias } = &stmt.target else {
            continue;
        };
        let root = path.is_empty();
        if root {
            stats.root_imports += 1;
        }
        let exact = if root { None } else { g.symbols.module_by_qname.get(path).copied() };
        let target = match exact {
            Some(exact) => Some((exact, "module")),
            None => match packages.import_target(path, from_mod_id) {
                DirImport::Bound(id) => {
                    stats.dir_bound += 1;
                    stats.root_bound += usize::from(root);
                    Some((id, "package_dir"))
                }
                DirImport::OnlyImporter => None,
                DirImport::NoDir if root => None,
                // Tail fallback (Pattern B): the go.mod-stripped path doesn't
                // match a module qname or a package directory — bind the
                // imported package by its unique short name (last `::`
                // segment). Miss-only + ambiguity-safe.
                DirImport::NoDir => {
                    unique_global_module(g, path.rsplit("::").next().unwrap_or(path))
                        .map(|id| (id, "tail_unique"))
                }
            },
        };
        let Some((target_id, rule)) = target else {
            continue;
        };
        import_edge(g, from_mod_id, target_id, rule, stmt.line);
        let bound = match alias {
            Some(alias) => alias.clone(),
            None => {
                // A root package without a clause binds no name.
                let Some(name) = packages.binding_name(path) else {
                    continue;
                };
                let derived = if root { None } else { path.rsplit("::").next() };
                stats.clause_named += usize::from(derived != Some(name));
                name.to_string()
            }
        };
        if bound.is_empty() {
            continue;
        }
        g.symbols
            .module_import_bindings
            .entry(from_mod_id)
            .or_default()
            .insert(bound, target_id);
    }
    stats
}

/// What [`resolve_imports_go`] bound, for the `[go-package]` markers.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GoImportStats {
    /// Imports bound by the package-directory step (LA.13b).
    pub(crate) dir_bound: usize,
    /// Imports of the repository-root package (an empty path, CI.3).
    pub(crate) root_imports: usize,
    /// Of `root_imports`: bound to a root-dir file (counted in `dir_bound`
    /// too).
    pub(crate) root_bound: usize,
    /// DF.15: un-aliased imports bound under a `package` clause name the
    /// path does not spell (a bound root import always: its path has none).
    pub(crate) clause_named: usize,
}

/// Ruby imports: `require 'foo/bar'` gives a slash-delimited path. Convert
/// slashes to `::` then look up directly (same shape as Go's resolver).
pub(crate) fn resolve_imports_slash(g: &mut RepoGraph, imports: &[ImportStmt]) {
    for stmt in imports {
        let Some(from_mod_id) = g
            .symbols
            .module_by_qname
            .get(&stmt.from_module)
            .copied()
        else {
            continue;
        };
        let ImportTarget::Module { path, alias } = &stmt.target else {
            continue;
        };
        let target_qname = path.replace('/', "::");
        let Some(target_id) = g.symbols.module_by_qname.get(&target_qname).copied() else {
            continue;
        };
        import_edge(g, from_mod_id, target_id, "module", stmt.line);
        let bound = alias
            .clone()
            .unwrap_or_else(|| target_qname.rsplit("::").next().unwrap_or(&target_qname).to_string());
        g.symbols
            .module_import_bindings
            .entry(from_mod_id)
            .or_default()
            .insert(bound, target_id);
    }
}

/// TypeScript imports: the parser keeps import sources as raw strings
/// (`./user`, `@angular/core`). `resolve_source(from_qname, raw)` converts a
/// raw source string to a module qname; `None` marks the import external.
///
/// LB.13: a source that resolves to an ambiguous same-stem pair (`./util`
/// beside `util.ts` + `util.js`) binds the sibling the specifier's own
/// extension names, else the one the importer's language loads first
/// ([`SameStem::pick`]).
pub(crate) fn resolve_imports_ts<R: Fn(&str, &str) -> Option<String>>(
    g: &mut RepoGraph,
    imports: &[ImportStmt],
    resolve_source: &R,
    same: &mut SameStem,
) {
    for stmt in imports {
        let Some(from_mod_id) = g
            .symbols
            .module_by_qname
            .get(&stmt.from_module)
            .copied()
        else {
            continue;
        };
        match &stmt.target {
            ImportTarget::Module { path, alias } => {
                let Some(target_qname) = resolve_source(&stmt.from_module, path) else {
                    continue;
                };
                let Some((target_id, rule)) =
                    ts_target(g, same, &target_qname, spec_ext(path), from_mod_id, "module")
                else {
                    continue;
                };
                import_edge(g, from_mod_id, target_id, rule, stmt.line);
                // Namespace import alias is the binding; bare side-effect has none.
                if let Some(a) = alias {
                    g.symbols
                        .module_import_bindings
                        .entry(from_mod_id)
                        .or_default()
                        .insert(a.clone(), target_id);
                }
            }
            ImportTarget::Symbol {
                module,
                name,
                alias,
                ..
            } => {
                let Some(target_qname) = resolve_source(&stmt.from_module, module) else {
                    continue;
                };
                let Some((target_mod_id, rule)) =
                    ts_target(g, same, &target_qname, spec_ext(module), from_mod_id, "symbol")
                else {
                    continue;
                };
                import_edge(g, from_mod_id, target_mod_id, rule, stmt.line);
                let bound = alias.clone().unwrap_or_else(|| name.clone());
                // Default import — bind to the module itself.
                // Named import — bind to the specific symbol inside that module.
                let target_id = if name == "default" {
                    Some(target_mod_id)
                } else {
                    g.symbols
                        .module_symbols
                        .get(&target_mod_id)
                        .and_then(|s| s.get(name))
                        .copied()
                };
                if let Some(t) = target_id {
                    g.symbols
                        .module_import_bindings
                        .entry(from_mod_id)
                        .or_default()
                        .insert(bound, t);
                }
            }
        }
    }
}

/// The MODULE a TS-family import source names: `target_qname` itself
/// (`rule`), else the same-stem sibling the specifier or the importer's
/// language names (LB.13, rule `same_stem`). A refused pick is counted
/// unresolved; the TS resolver has no tail fallback to try after it.
fn ts_target(
    g: &RepoGraph,
    same: &mut SameStem,
    target_qname: &str,
    explicit_ext: Option<&str>,
    importer: NodeId,
    rule: &'static str,
) -> Option<(NodeId, &'static str)> {
    if let Some(id) = g.symbols.module_by_qname.get(target_qname) {
        return Some((*id, rule));
    }
    match same.pick(g, target_qname, explicit_ext, importer) {
        SamePick::Bound(id) => Some((id, "same_stem")),
        SamePick::Refused => {
            same.unresolved();
            None
        }
        SamePick::NotAStem => None,
    }
}

/// Push one IMPORTS edge with its evidence (LC.3d): `graph:imports`, rule the
/// lookup that bound it: `module` (the import path is a module qname),
/// `submodule` (`from pkg import mod`), `symbol` (the module holding an
/// imported name), `package_dir` (a Go package directory, LA.13b),
/// `same_stem` (the sibling of an ambiguous same-stem pair the importer's
/// language loads, LB.13), `namespace` (the member of the declared namespace
/// PACKAGE a symbol import names, DF.2) or `tail_unique` (the repo-unique
/// short name, a guess the full path could not confirm).
/// `line` is the import statement's 0-based row (LC.3b, basis site).
fn import_edge(g: &mut RepoGraph, from: NodeId, to: NodeId, rule: &str, line: u32) {
    let ev = graph_evidence("graph:imports", rule).line(line);
    push_edge(g, from, to, edge_category::IMPORTS, ev);
}

/// Convert a (possibly relative) `from X import Y` module reference into an
/// absolute qname using `::` separators.
fn resolve_module_reference(from_module: &str, module_ref: &str, level: u32) -> String {
    if level == 0 {
        return module_ref.replace('.', "::");
    }
    // Relative: strip `level` trailing components from `from_module`, then
    // append `module_ref`. `level=1` pops 1 (the current file stays at package level).
    let mut parts: Vec<&str> = from_module.split("::").collect();
    for _ in 0..level {
        parts.pop();
    }
    let base = parts.join("::");
    if module_ref.is_empty() {
        base
    } else if base.is_empty() {
        module_ref.replace('.', "::")
    } else {
        format!("{base}::{}", module_ref.replace('.', "::"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::{build_dotted, build_typescript};
    use crate::test_support::repo;
    use glia_code_domain::evidence::Evidence;
    use glia_code_domain::{CallQualifier, CallSite, FileParse, GRAPH_TYPE, cell_type};
    use glia_core::{Cell, CellPayload, Confidence, EdgeCategoryId, Node, NodeKindId};

    #[test]
    fn relative_import_resolution() {
        assert_eq!(resolve_module_reference("myapp::users", "helpers", 1), "myapp::helpers");
        assert_eq!(resolve_module_reference("a::b::c", "d", 2), "a::d");
        assert_eq!(resolve_module_reference("a::b", "c.d", 0), "c::d");
        assert_eq!(resolve_module_reference("a::b::c", "", 1), "a::b");
    }

    // ---- LB.13: same-stem siblings of one graph ------------------------------

    fn gid(kind: NodeKindId, qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), kind, qname)
    }

    fn module(qname: &str) -> NodeId {
        gid(node_kind::MODULE, qname)
    }

    fn func(qname: &str) -> NodeId {
        gid(node_kind::FUNCTION, qname)
    }

    /// The parse of `path`: MODULE `qname` (nav name `name`) located at
    /// `path` by its POSITION, with FUNCTION children `fns`.
    fn file(path: &str, qname: &str, name: &str, fns: &[&str]) -> FileParse {
        let m = module(qname);
        let mut fp = FileParse::default();
        let position = Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(format!(
                r#"{{"file":"{path}","start_line":0,"end_line":9}}"#
            )),
        };
        fp.nodes.push(Node { id: m, repo: repo(), confidence: Confidence::Strong, cells: vec![position] });
        fp.nav.record(m, name, qname, node_kind::MODULE, None);
        for f in fns {
            let q = format!("{qname}::{f}");
            let id = func(&q);
            fp.nodes.push(Node { id, repo: repo(), confidence: Confidence::Strong, cells: vec![] });
            fp.nav.record(id, f, &q, node_kind::FUNCTION, Some(m));
        }
        fp
    }

    /// `import { fmt } from '<spec>'` in `fp`, and `<caller>` calling `fmt()`.
    fn imports_fmt(mut fp: FileParse, qname: &str, spec: &str, caller: &str) -> FileParse {
        fp.imports.push(ImportStmt {
            from_module: qname.to_string(),
            target: ImportTarget::Symbol {
                module: spec.to_string(),
                name: "fmt".to_string(),
                alias: None,
                level: 0,
            },
            line: 0,
        });
        fp.calls.push(CallSite {
            from: func(&format!("{qname}::{caller}")),
            qualifier: CallQualifier::Bare("fmt".to_string()),
            line: 2,
        });
        fp
    }

    /// A relative TS source resolver: the importer's directory plus the
    /// specifier, `.ts` / `.js` stripped (the engine's `resolve_ts_source`).
    fn ts_source(from: &str, spec: &str) -> Option<String> {
        let mut segs: Vec<&str> = from.split("::").collect();
        segs.pop();
        for part in spec.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    segs.pop();
                }
                p => segs.push(p.strip_suffix(".ts").or_else(|| p.strip_suffix(".js")).unwrap_or(p)),
            }
        }
        Some(segs.join("::"))
    }

    fn edges_of(g: &RepoGraph, category: EdgeCategoryId, from: NodeId) -> Vec<NodeId> {
        g.edges.iter().filter(|e| e.category == category && e.from == from).map(|e| e.to).collect()
    }

    fn ts_pair() -> Vec<FileParse> {
        vec![
            file("src/util.ts", "src::util.ts", "util", &["fmt", "pad"]),
            file("src/util.js", "src::util.js", "util", &["fmt", "legacy"]),
        ]
    }

    #[test]
    fn a_bare_ts_import_binds_the_sibling_the_importers_language_loads() {
        let mut parses = ts_pair();
        parses.push(imports_fmt(
            file("src/app.ts", "src::app", "app", &["render"]),
            "src::app",
            "./util",
            "render",
        ));
        parses.push(imports_fmt(
            file("src/old.js", "src::old", "old", &["draw"]),
            "src::old",
            "./util",
            "draw",
        ));
        // An explicit extension names its file whatever the importer.
        parses.push(imports_fmt(
            file("src/bridge.ts", "src::bridge", "bridge", &["wrap"]),
            "src::bridge",
            "./util.js",
            "wrap",
        ));
        let g = build_typescript(repo(), parses, ts_source).expect("build");
        let (ts, js) = (module("src::util.ts"), module("src::util.js"));
        assert_eq!(edges_of(&g, edge_category::IMPORTS, module("src::app")), [ts]);
        assert_eq!(edges_of(&g, edge_category::IMPORTS, module("src::old")), [js]);
        assert_eq!(edges_of(&g, edge_category::IMPORTS, module("src::bridge")), [js]);
        assert_eq!(edges_of(&g, edge_category::CALLS, func("src::app::render")), [func("src::util.ts::fmt")]);
        assert_eq!(edges_of(&g, edge_category::CALLS, func("src::old::draw")), [func("src::util.js::fmt")]);
        assert_eq!(edges_of(&g, edge_category::CALLS, func("src::bridge::wrap")), [func("src::util.js::fmt")]);
        // The pick is build-local: no bare alias lands in the symbol table.
        assert_eq!(g.symbols.module_by_qname.get("src::util"), None);
        // Each IMPORTS edge names the rule that bound it.
        for e in g.edges.iter().filter(|e| e.category == edge_category::IMPORTS) {
            let ev = Evidence::of(e).expect("evidence");
            assert_eq!((ev.emitter.as_str(), ev.rule.as_deref()), ("graph:imports", Some("same_stem")));
            assert_eq!(ev.line, Some(0));
        }
        // Replayed on the built graph: one explicit pick, two by importer.
        let mut same = same_stem_table(&g);
        assert_eq!(same.pick(&g, "src::util", Some("js"), module("src::app")), SamePick::Bound(js));
        assert_eq!(same.pick(&g, "src::util", None, module("src::app")), SamePick::Bound(ts));
        assert_eq!(same.pick(&g, "src::util", None, module("src::old")), SamePick::Bound(js));
        assert_eq!(
            same.stats,
            SameStemStats { explicit: 1, importer: 2, unresolved: 0 }
        );
        assert_eq!(
            same.marker().as_deref(),
            Some("[imports] same-stem picks: explicit=1 importer=2 unresolved=0 (stems=1 exts=js,ts)")
        );
    }

    /// `(:require [app.core])`: the dotted path leaves out the file's `clj/`
    /// directory, so the pair is found by suffix.
    fn requires_core(mut fp: FileParse, qname: &str) -> FileParse {
        fp.imports.push(ImportStmt {
            from_module: qname.to_string(),
            target: ImportTarget::Module { path: "app.core".to_string(), alias: None },
            line: 1,
        });
        fp
    }

    #[test]
    fn a_clojure_require_binds_its_platforms_sibling() {
        let parses = vec![
            file("clj/app/core.clj", "clj::app::core.clj", "core", &["f", "g"]),
            file("clj/app/core.cljs", "clj::app::core.cljs", "core", &["f", "h"]),
            file("clj/app/core.cljc", "clj::app::core.cljc", "core", &["k"]),
            requires_core(file("clj/app/server.clj", "clj::app::server", "server", &[]), "clj::app::server"),
            requires_core(file("clj/app/main.cljs", "clj::app::main", "main", &[]), "clj::app::main"),
            requires_core(file("clj/app/shared.cljc", "clj::app::shared", "shared", &[]), "clj::app::shared"),
            // A lone `core` elsewhere: the tail fallback must not take it for
            // the pair's once their own qnames stop ending in `core`.
            file("clj/lib/core.clj", "clj::lib::core", "core", &[]),
        ];
        let g = build_dotted(repo(), parses).expect("build");
        assert_eq!(
            edges_of(&g, edge_category::IMPORTS, module("clj::app::server")),
            [module("clj::app::core.clj")]
        );
        assert_eq!(
            edges_of(&g, edge_category::IMPORTS, module("clj::app::main")),
            [module("clj::app::core.cljs")]
        );
        // A .cljc file loads a different sibling per platform: unresolved.
        assert!(edges_of(&g, edge_category::IMPORTS, module("clj::app::shared")).is_empty());
        let bound = &g.symbols.module_import_bindings[&module("clj::app::server")];
        assert_eq!(bound.get("app"), Some(&module("clj::app::core.clj")));

        let mut same = same_stem_table(&g);
        assert_eq!(same.pick_path(&g, "app::core", module("clj::app::shared")), SamePick::Refused);
        assert_eq!(
            same.pick_path(&g, "app::core", module("clj::app::server")),
            SamePick::Bound(module("clj::app::core.clj"))
        );
        assert_eq!(
            same.pick_path(&g, "clj::app::core", module("clj::app::main")),
            SamePick::Bound(module("clj::app::core.cljs"))
        );
        // Not an ambiguous stem of this graph.
        assert_eq!(same.pick_path(&g, "lib::core", module("clj::app::server")), SamePick::NotAStem);
        assert_eq!(same.stats, SameStemStats { explicit: 0, importer: 2, unresolved: 0 });

        // The resolver's own count, replayed on the built symbol table: the
        // .cljc require is the one unresolved.
        let mut g = g;
        g.edges.clear();
        let imports: Vec<ImportStmt> = ["clj::app::server", "clj::app::main", "clj::app::shared"]
            .iter()
            .flat_map(|q| requires_core(FileParse::default(), q).imports)
            .collect();
        let mut same = same_stem_table(&g);
        resolve_imports_python(&mut g, &imports, &mut same);
        assert_eq!(same.stats, SameStemStats { explicit: 0, importer: 2, unresolved: 1 });
        assert_eq!(
            same.marker().as_deref(),
            Some("[imports] same-stem picks: explicit=0 importer=2 unresolved=1 (stems=1 exts=clj,cljc,cljs)")
        );
    }

    #[test]
    fn a_class_import_beside_a_kotlin_sibling_still_binds_the_class() {
        // `jvm/shop/Foo.java` (CLASS `jvm::shop::Foo`, LB.2's directory scope)
        // + `jvm/shop/Foo.kt`: Java imports name the class, so the pair's
        // empty order refuses the module but the class guess still binds.
        let mut java = file("jvm/shop/Foo.java", "jvm::shop::Foo.java", "Foo", &[]);
        let class = gid(node_kind::CLASS, "jvm::shop::Foo");
        java.nodes.push(Node { id: class, repo: repo(), confidence: Confidence::Strong, cells: vec![] });
        java.nav.record(class, "Foo", "jvm::shop::Foo", node_kind::CLASS, Some(module("jvm::shop::Foo.java")));
        let mut main = file("jvm/app/Main.java", "jvm::app::Main", "Main", &[]);
        main.imports.push(ImportStmt {
            from_module: "jvm::app::Main".to_string(),
            target: ImportTarget::Module { path: "shop.Foo".to_string(), alias: None },
            line: 2,
        });
        let parses = vec![
            java,
            file("jvm/shop/Foo.kt", "jvm::shop::Foo.kt", "Foo", &["topLevel"]),
            main,
        ];
        let g = build_dotted(repo(), parses).expect("build");
        assert_eq!(edges_of(&g, edge_category::IMPORTS, module("jvm::app::Main")), [class]);
        let mut same = same_stem_table(&g);
        assert_eq!(same.pick_path(&g, "shop::Foo", module("jvm::app::Main")), SamePick::Refused);
    }

    #[test]
    fn two_stems_ending_in_the_required_path_bind_neither() {
        let parses = vec![
            file("a/app/core.clj", "a::app::core.clj", "core", &[]),
            file("a/app/core.cljs", "a::app::core.cljs", "core", &[]),
            file("b/app/core.clj", "b::app::core.clj", "core", &[]),
            file("b/app/core.cljs", "b::app::core.cljs", "core", &[]),
            requires_core(file("a/app/server.clj", "a::app::server", "server", &[]), "a::app::server"),
        ];
        let g = build_dotted(repo(), parses).expect("build");
        assert!(edges_of(&g, edge_category::IMPORTS, module("a::app::server")).is_empty());
        let mut same = same_stem_table(&g);
        assert_eq!(same.pick_path(&g, "app::core", module("a::app::server")), SamePick::Refused);
    }

    #[test]
    fn a_graph_without_a_same_stem_pair_reads_no_position() {
        let parses = vec![
            file("src/util.ts", "src::util", "util", &["fmt"]),
            // A file-named MODULE alone in its bare form (LB.9b's cross-group
            // case) is its alias's, not an ambiguous stem.
            file("api/user.ts", "api::user.ts", "user", &[]),
            imports_fmt(file("src/app.ts", "src::app", "app", &["render"]), "src::app", "./util", "render"),
        ];
        let g = build_typescript(repo(), parses, ts_source).expect("build");
        let mut same = same_stem_table(&g);
        assert!(same.by_bare.is_empty());
        assert_eq!(same.pick(&g, "api::user", None, module("src::app")), SamePick::NotAStem);
        assert!(same.module_ext.is_none(), "no pick needed an importer's extension");
        assert_eq!(same.marker(), None);
        assert_eq!(edges_of(&g, edge_category::IMPORTS, module("src::app")), [module("src::util")]);
    }

    #[test]
    fn specifier_and_file_extensions() {
        assert_eq!(spec_ext("./util.js"), Some("js"));
        assert_eq!(spec_ext("'./util.ts'"), Some("ts"));
        assert_eq!(spec_ext("./util"), None);
        assert_eq!(spec_ext("../lib.v2/util"), None);
        assert_eq!(file_ext("clj/app/main.cljs"), Some("cljs"));
        assert_eq!(file_ext("Makefile"), None);
        assert_eq!(file_ext("a/b."), None);
    }

    // ---- DF.2: symbol imports bind by namespace ------------------------------

    fn class(qname: &str) -> NodeId {
        gid(node_kind::CLASS, qname)
    }

    fn method(qname: &str) -> NodeId {
        gid(node_kind::METHOD, qname)
    }

    /// The PACKAGE `ns` the file MODULE `m` of `fp` opens, as the php parser
    /// records a namespace: node, nav child of the file, CONTAINS edge.
    fn opens(fp: &mut FileParse, m: NodeId, ns: &str) -> NodeId {
        let pkg = gid(node_kind::PACKAGE, ns);
        fp.nodes.push(Node { id: pkg, repo: repo(), confidence: Confidence::Strong, cells: vec![] });
        let simple = ns.rsplit("::").next().unwrap_or(ns);
        fp.nav.record(pkg, simple, ns, node_kind::PACKAGE, Some(m));
        fp.edges.push(glia_core::Edge {
            from: m,
            to: pkg,
            category: edge_category::CONTAINS,
            confidence: Confidence::Strong,
            cells: vec![],
        });
        pkg
    }

    /// CLASS `qname` (nav name its last segment) under `parent`, with
    /// METHOD children `methods`.
    fn declares(fp: &mut FileParse, parent: NodeId, qname: &str, methods: &[&str]) {
        let name = qname.rsplit("::").next().unwrap_or(qname);
        let c = class(qname);
        fp.nodes.push(Node { id: c, repo: repo(), confidence: Confidence::Strong, cells: vec![] });
        fp.nav.record(c, name, qname, node_kind::CLASS, Some(parent));
        for m in methods {
            let q = format!("{qname}::{m}");
            let id = method(&q);
            fp.nodes.push(Node { id, repo: repo(), confidence: Confidence::Strong, cells: vec![] });
            fp.nav.record(id, m, &q, node_kind::METHOD, Some(c));
        }
    }

    /// A statement-form PHP file (`namespace <ns>;`) at `path`, MODULE
    /// `qname`, declaring CLASS `qname` (PSR-4: the class is
    /// directory-scoped, so its qname is the file's) with `methods`.
    fn php_class(path: &str, qname: &str, ns: &str, methods: &[&str]) -> FileParse {
        let name = qname.rsplit("::").next().unwrap_or(qname);
        let mut fp = file(path, qname, name, &[]);
        let m = module(qname);
        opens(&mut fp, m, ns);
        declares(&mut fp, m, qname, methods);
        fp
    }

    /// `use <ns>\<name>;` in the file MODULE `qname` of `fp`, and its class
    /// `<qname>`'s method `caller` calling `<name>::issue()`.
    fn uses(mut fp: FileParse, qname: &str, ns: &str, name: &str, caller: &str) -> FileParse {
        fp.imports.push(ImportStmt {
            from_module: qname.to_string(),
            target: ImportTarget::Symbol {
                module: ns.to_string(),
                name: name.to_string(),
                alias: None,
                level: 0,
            },
            line: 3,
        });
        fp.calls.push(CallSite {
            from: method(&format!("{qname}::{caller}")),
            qualifier: CallQualifier::Attribute { base: name.to_string(), name: "issue".to_string() },
            line: 9,
        });
        fp
    }

    /// The service `src/Orders/<svc>.php` (namespace App::Orders) whose
    /// method `run` calls `Invoicer::issue()` after `use <ns>\Invoicer;`.
    fn service(svc: &str, ext: &str, ns: &str) -> FileParse {
        let q = format!("src::Orders::{svc}");
        let fp = php_class(&format!("src/Orders/{svc}.{ext}"), &q, "App::Orders", &["run"]);
        uses(fp, &q, ns, "Invoicer", "run")
    }

    /// `(target, rule)` of every IMPORTS edge out of MODULE `from`.
    fn imports_of(g: &RepoGraph, from: &str) -> Vec<(NodeId, String)> {
        g.edges
            .iter()
            .filter(|e| e.category == edge_category::IMPORTS && e.from == module(from))
            .map(|e| (e.to, Evidence::of(e).and_then(|ev| ev.rule).unwrap_or_default()))
            .collect()
    }

    fn calls_of(g: &RepoGraph, svc: &str) -> Vec<NodeId> {
        edges_of(g, edge_category::CALLS, method(&format!("src::Orders::{svc}::run")))
    }

    /// The namespace step's own count, replayed on the built graph.
    /// Every symbol import of these tests misses its module, so each one
    /// reaches the step.
    fn replay(g: &RepoGraph, imports: &[ImportStmt]) -> (NamespaceStats, Option<String>) {
        let mut same = same_stem_table(g);
        let mut ns = Namespaces::default();
        for stmt in imports {
            if let ImportTarget::Symbol { module, name, .. } = &stmt.target {
                let from = module_id(g, &stmt.from_module);
                ns.step(g, &mut same, module, name, from);
            }
        }
        (ns.stats.clone(), ns.marker())
    }

    fn module_id(g: &RepoGraph, qname: &str) -> NodeId {
        g.symbols.module_by_qname[qname]
    }

    fn all_imports(parses: &[FileParse]) -> Vec<ImportStmt> {
        parses.iter().flat_map(|p| p.imports.clone()).collect()
    }

    fn billing_and_shipping() -> Vec<FileParse> {
        vec![
            php_class("src/Billing/Invoicer.php", "src::Billing::Invoicer", "App::Billing", &["issue"]),
            php_class("src/Shipping/Invoicer.php", "src::Shipping::Invoicer", "App::Shipping", &["issue"]),
        ]
    }

    #[test]
    fn same_short_name_binds_by_namespace() {
        let mut parses = billing_and_shipping();
        parses.push(service("OrderService", "php", "App::Billing"));
        parses.push(service("ShipmentService", "php", "App::Shipping"));
        let imports = all_imports(&parses);
        let g = build_dotted(repo(), parses).expect("build");
        let (billing, shipping) = (class("src::Billing::Invoicer"), class("src::Shipping::Invoicer"));
        assert_eq!(imports_of(&g, "src::Orders::OrderService"), [(billing, "namespace".to_string())]);
        assert_eq!(imports_of(&g, "src::Orders::ShipmentService"), [(shipping, "namespace".to_string())]);
        assert_eq!(calls_of(&g, "OrderService"), [method("src::Billing::Invoicer::issue")]);
        assert_eq!(calls_of(&g, "ShipmentService"), [method("src::Shipping::Invoicer::issue")]);
        // HEAD's short-name guess sees two Invoicers and binds neither.
        assert_eq!(unique_global_function(&g, "Invoicer"), None);
        let (stats, marker) = replay(&g, &imports);
        assert_eq!((stats.bound, stats.ambiguous, stats.missing, stats.refused_owned_root), (2, 0, 0, 0));
        assert_eq!(
            marker.as_deref(),
            Some("[imports] namespace symbols: bound=2 ambiguous=0 missing=0 refused_owned_root=0 (exts=php)")
        );
    }

    #[test]
    fn unique_name_in_declared_namespace_is_namespace_rule() {
        let mut parses = vec![php_class(
            "src/Billing/Invoicer.php",
            "src::Billing::Invoicer",
            "App::Billing",
            &["issue"],
        )];
        parses.push(service("OrderService", "php", "App::Billing"));
        let g = build_dotted(repo(), parses).expect("build");
        let billing = class("src::Billing::Invoicer");
        // The target HEAD's tail guess picked, now confirmed by the namespace.
        assert_eq!(unique_global_function(&g, "Invoicer"), Some(billing));
        assert_eq!(imports_of(&g, "src::Orders::OrderService"), [(billing, "namespace".to_string())]);
        assert_eq!(calls_of(&g, "OrderService"), [method("src::Billing::Invoicer::issue")]);
    }

    #[test]
    fn undeclared_namespace_under_owned_root_refuses_php() {
        let mut parses = vec![php_class(
            "src/Billing/Invoicer.php",
            "src::Billing::Invoicer",
            "App::Billing",
            &["issue"],
        )];
        parses.push(service("LegalService", "php", "App::Legal"));
        parses.push(service("OrderService", "php", "App::Billing"));
        let imports = all_imports(&parses);
        let g = build_dotted(repo(), parses).expect("build");
        assert!(imports_of(&g, "src::Orders::LegalService").is_empty());
        assert!(calls_of(&g, "LegalService").is_empty());
        // The control still binds.
        assert_eq!(calls_of(&g, "OrderService"), [method("src::Billing::Invoicer::issue")]);
        let (stats, marker) = replay(&g, &imports);
        assert_eq!((stats.bound, stats.refused_owned_root), (1, 1));
        assert_eq!(
            marker.as_deref(),
            Some("[imports] namespace symbols: bound=1 ambiguous=0 missing=0 refused_owned_root=1 (exts=php)")
        );
    }

    #[test]
    fn undeclared_namespace_non_php_keeps_tail() {
        let mut parses = vec![php_class(
            "src/Billing/Invoicer.php",
            "src::Billing::Invoicer",
            "App::Billing",
            &["issue"],
        )];
        // The same import shape from a `.py` importer: not PHP, so the
        // owned-root refusal never applies and the short name binds as HEAD.
        parses.push(service("LegalService", "py", "App::Legal"));
        let imports = all_imports(&parses);
        let g = build_dotted(repo(), parses).expect("build");
        let billing = class("src::Billing::Invoicer");
        assert_eq!(imports_of(&g, "src::Orders::LegalService"), [(billing, "tail_unique".to_string())]);
        assert_eq!(calls_of(&g, "LegalService"), [method("src::Billing::Invoicer::issue")]);
        let (stats, marker) = replay(&g, &imports);
        assert_eq!(stats, NamespaceStats::default());
        assert_eq!(marker, None);
    }

    #[test]
    fn vendor_namespace_keeps_tail() {
        let mut parses = vec![php_class(
            "src/Billing/Invoicer.php",
            "src::Billing::Invoicer",
            "App::Billing",
            &["issue"],
        )];
        // `use Illuminate\Support\Invoicer;`: no file declares an
        // `Illuminate` namespace, so the repo does not own that root.
        parses.push(service("VendorService", "php", "Illuminate::Support"));
        let g = build_dotted(repo(), parses).expect("build");
        let billing = class("src::Billing::Invoicer");
        assert_eq!(imports_of(&g, "src::Orders::VendorService"), [(billing, "tail_unique".to_string())]);
        let idx = NamespaceIndex::build(&g);
        assert_eq!(idx.member("Illuminate::Support", "Invoicer"), NsHit::NotDeclared);
        assert!(!idx.owns_root("Illuminate::Support"));
        assert!(idx.owns_root("App::Legal"));
    }

    #[test]
    fn braced_namespace_members_bind() {
        // `namespace App\Jobs { class Mailer { .. } }` in two files: the
        // class's nav parent is the PACKAGE itself, its qname the namespace's.
        let mut a = file("src/Jobs/Mailer.php", "src::Jobs::Mailer", "Mailer", &[]);
        let pkg = opens(&mut a, module("src::Jobs::Mailer"), "App::Jobs");
        declares(&mut a, pkg, "App::Jobs::Mailer", &["issue"]);
        let mut b = file("lib/Jobs/Queue.php", "lib::Jobs::Queue", "Queue", &[]);
        let pkg_b = opens(&mut b, module("lib::Jobs::Queue"), "App::Jobs");
        declares(&mut b, pkg_b, "App::Jobs::Queue", &["issue"]);
        // A same-named class of another namespace.
        let other = php_class("src/Mail/Mailer.php", "src::Mail::Mailer", "App::Mail", &["issue"]);
        let q = "src::Orders::OrderService";
        let svc = php_class("src/Orders/OrderService.php", q, "App::Orders", &["run"]);
        let svc = uses(svc, q, "App::Jobs", "Mailer", "run");
        let g = build_dotted(repo(), vec![a, b, other, svc]).expect("build");
        let mailer = class("App::Jobs::Mailer");
        assert_eq!(imports_of(&g, q), [(mailer, "namespace".to_string())]);
        assert_eq!(calls_of(&g, "OrderService"), [method("App::Jobs::Mailer::issue")]);
        let idx = NamespaceIndex::build(&g);
        assert_eq!(idx.member("App::Jobs", "Queue"), NsHit::Unique(class("App::Jobs::Queue")));
    }

    #[test]
    fn declared_namespace_without_the_member_binds_nothing() {
        // `use App\Billing\Invoicer;` while the only Invoicer is App\Legacy's.
        let mut parses = vec![
            php_class("src/Billing/Ledger.php", "src::Billing::Ledger", "App::Billing", &["post"]),
            php_class("src/Legacy/Invoicer.php", "src::Legacy::Invoicer", "App::Legacy", &["issue"]),
        ];
        parses.push(service("OrderService", "php", "App::Billing"));
        let imports = all_imports(&parses);
        let g = build_dotted(repo(), parses).expect("build");
        assert!(imports_of(&g, "src::Orders::OrderService").is_empty());
        assert!(calls_of(&g, "OrderService").is_empty());
        let (stats, _) = replay(&g, &imports);
        assert_eq!(stats.missing, 1);
    }

    #[test]
    fn a_class_two_files_declare_in_one_namespace_binds_nothing() {
        let mut parses = vec![
            php_class("src/Billing/Invoicer.php", "src::Billing::Invoicer", "App::Billing", &["issue"]),
            php_class("lib/Billing/Invoicer.php", "lib::Billing::Invoicer", "App::Billing", &["issue"]),
        ];
        parses.push(service("OrderService", "php", "App::Billing"));
        let imports = all_imports(&parses);
        let g = build_dotted(repo(), parses).expect("build");
        assert!(imports_of(&g, "src::Orders::OrderService").is_empty());
        let (stats, _) = replay(&g, &imports);
        assert_eq!(stats.ambiguous, 1);
    }

    #[test]
    fn a_namespace_import_keeps_the_head_path() {
        // `use App\Billing;` (PHP) / `using Shop.Core;` (C#) name a namespace,
        // not a member: HEAD's short-name lookup binds the PACKAGE, whether or
        // not the parent namespace `App` is itself declared.
        let mut root = file("src/Kernel.php", "src::Kernel", "Kernel", &[]);
        opens(&mut root, module("src::Kernel"), "App");
        let invoicer =
            php_class("src/Billing/Invoicer.php", "src::Billing::Invoicer", "App::Billing", &["issue"]);
        let mut svc = service("OrderService", "php", "App");
        if let Some(ImportStmt { target: ImportTarget::Symbol { name, .. }, .. }) = svc.imports.last_mut() {
            *name = "Billing".to_string();
        }
        let imports = svc.imports.clone();
        let g = build_dotted(repo(), vec![root, invoicer, svc]).expect("build");
        let billing_ns = gid(node_kind::PACKAGE, "App::Billing");
        assert_eq!(
            imports_of(&g, "src::Orders::OrderService"),
            [(billing_ns, "tail_unique".to_string())]
        );
        let idx = NamespaceIndex::build(&g);
        assert_eq!(idx.member("App", "Billing"), NsHit::Namespace);
        // `App` undeclared: still a namespace import, never the refusal.
        let mut no_root = NamespaceIndex::build(&g);
        no_root.members.remove("App");
        assert_eq!(no_root.member("App", "Billing"), NsHit::Namespace);
        let (stats, marker) = replay(&g, &imports);
        assert_eq!(stats, NamespaceStats::default());
        assert_eq!(marker, None);
    }
}
