//! Import resolution — one pass per import-path dialect (dotted / Go / slash /
//! TypeScript), all of them writing IMPORTS edges and module import bindings.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use glia_code_domain::{
    ImportStmt, ImportTarget, bare_module_qname, edge_category, evidence, node_kind,
    same_stem_order,
};
use glia_core::NodeId;

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
pub(crate) fn resolve_imports_python(
    g: &mut RepoGraph,
    imports: &[ImportStmt],
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
                } else if let Some(symbol_id) = unique_global_function(g, name) {
                    // Tail fallback (Pattern B): `from a.b import Name` where `a.b`
                    // isn't a resolvable module (flat layout) — bind the imported
                    // symbol by its unique global name. Miss-only + ambiguity-safe.
                    import_edge(g, from_mod_id, symbol_id, "tail_unique", stmt.line);
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
/// CI.3: an empty path is the repository-root package (dir `""`), whose
/// binding name the parser wrote down as the alias (the import path's last
/// element, or the explicit alias). It binds the root dir's first non-test
/// file by qname (no file is named after the root dir), rule `package_dir`;
/// with no root `.go` file it binds nothing (the tail fallback has no name to
/// look up).
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
        let bound = alias
            .clone()
            .unwrap_or_else(|| path.rsplit("::").next().unwrap_or(path).to_string());
        // A root import without a name (only a hand-built ImportStmt; the
        // parser always names one) binds nothing.
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
/// language loads, LB.13) or `tail_unique` (the repo-unique short name, a
/// guess the full path could not confirm).
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
}
