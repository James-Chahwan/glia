//! Per-language graph builders plus the shared merge / nav / symbol-table
//! passes they all run.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use glia_code_domain::evidence::Evidence;
use glia_code_domain::{
    CallQualifier, CallSite, CodeNav, FileParse, GRAPH_TYPE, ImportStmt, ImportTarget, NavFact,
    UnresolvedRef, bare_module_qname, edge_category, node_kind,
};
use glia_core::{Cell, Confidence, Edge, EdgeCategoryId, NodeId, NodeKindId, RepoId};

use crate::calls::{
    EvidenceTally, emit_method_level_implements, enclosing_class_or_struct, enclosing_module,
    graph_evidence, push_edge, resolve_calls, resolve_refs, unique_global_type,
};
use crate::go_mounts::MountStats;
use crate::imports::{
    SameStem, resolve_imports_go, resolve_imports_python, resolve_imports_slash,
    resolve_imports_ts, same_stem_table,
};
use crate::rust_paths::{MOD_ITEM, RustCrate, RustIndex, resolve_imports_rust, rust_ev};
use crate::swift_scope::{SwiftModuleTypes, implicit_self};
use crate::types::{GraphError, RepoGraph, SymbolTable};

// ============================================================================
// Public entry point
// ============================================================================

/// Build a per-repo Python graph from a set of file-parse outputs.
///
/// Every builder ends with the LC.3d `[evidence-graph]` marker
/// ([`EvidenceTally::report`]): which branch of `resolve_calls` /
/// `resolve_refs` bound how many edges of this graph.
pub fn build_python(repo: RepoId, parses: Vec<FileParse>) -> Result<RepoGraph, GraphError> {
    let (mut g, all_imports, all_calls, all_refs) = merge_parses(repo, parses);
    build_symbol_table(&mut g);
    let mut same = same_stem_table(&g);
    resolve_imports_python(&mut g, &all_imports, &mut same);
    same.report();
    let mut tally = EvidenceTally::default();
    resolve_calls(&mut g, &all_calls, |_, _| None, &mut tally);
    resolve_refs(&mut g, &all_refs, &mut tally);
    emit_method_level_implements(&mut g);
    tally.report();
    Ok(g)
}

/// Build a per-repo Go graph. The engine gives every Go file its own MODULE
/// (the qname is the file path); a package is the set of files in one
/// directory.
///
/// LA.23d: a method declared in a different file from its receiver struct is
/// re-parented under that struct first ([`bind_split_go_receivers`]), so
/// `class_methods` holds it (SelfMethod and field-typed calls resolve) and
/// its file's `module_symbols` no longer lists it as a top-level function.
/// Its Bare / Attribute calls still resolve in its own file's scope
/// ([`resolve_go_calls`]).
///
/// LD.7b: an interface's embedded interfaces bind package-scoped and to an
/// INTERFACE only ([`resolve_go_embeds`]); then, Go interfaces being
/// satisfied implicitly, [`emit_go_implicit_implements`] derives each type ->
/// interface IMPLEMENTS edge from method names, signatures (CA.3b) and, for a
/// one-method interface or a `_test.go` side, package reachability, before
/// A6.6 pairs them method by method (at the type-level edge's Medium).
///
/// LA.13b: a package is a directory, so an import binds the imported
/// directory and a call every generic lookup missed resolves across the
/// package's files ([`GoPackages`]), with no qname or persisted-table change.
///
/// CA.2b: the same hook binds a method call whose receiver is a call chain,
/// a local / parameter, a package var or a struct-field chain on that
/// receiver's type ([`GoPackages::typed_receiver`]); `[go-receivers]` prints
/// what it bound.
///
/// CB.20: once calls are bound, every provisional mount ROUTE (a route on a
/// parameter- or field-held router group) is re-keyed to each prefix its
/// group receives through the resolved calls, one ROUTE per mount, before
/// the refs resolve ([`crate::go_mounts::bind`]); `[go-mounts]` prints what
/// it did.
pub fn build_go(repo: RepoId, parses: Vec<FileParse>) -> Result<RepoGraph, GraphError> {
    let (g, split, implicit, packages, receivers, mounts) = build_go_passes(repo, parses);
    if let Some(line) = split.marker() {
        eprintln!("{line}");
    }
    if let Some(line) = mounts.marker() {
        eprintln!("{line}");
    }
    if let Some(stats) = implicit {
        eprintln!("{}", stats.marker());
        eprintln!("{}", stats.filtered_marker());
    }
    if let Some(line) = packages.marker() {
        eprintln!("{line}");
    }
    eprintln!("{}", receivers.marker());
    eprintln!("{}", go_types_marker(&g.nav));
    if let Some(line) = go_sigs_marker(&g.nav) {
        eprintln!("{line}");
    }
    Ok(g)
}

/// CA.3a: `[go-sigs] methods=<M> with_signature=<S>` over a Go graph's merged
/// nav: M METHOD nodes (struct, split-receiver and interface methods), S of
/// them with a recorded normalised signature (a generic receiver's methods
/// and a generic interface's elements record none). `None` for a graph with
/// no METHOD. The facts the implicit-IMPLEMENTS signature check (CA.3b)
/// compares; only counted.
fn go_sigs_marker(nav: &CodeNav) -> Option<String> {
    let methods = nav
        .kind_by_id
        .iter()
        .filter(|(_, k)| **k == node_kind::METHOD);
    let (mut m, mut s) = (0usize, 0usize);
    for (id, _) in methods {
        m += 1;
        s += usize::from(nav.method_sigs.contains_key(id));
    }
    (m > 0).then(|| format!("[go-sigs] methods={m} with_signature={s}"))
}

/// CA.2a: `[go-types] return_types=<R> local_scopes=<L> locals=<N>
/// package_vars=<V>` over a Go graph's merged nav: R callables with a
/// recorded result type, L fn / METHOD scopes in `local_types` and N their
/// entries, V the package-level vars recorded under file MODULE scopes. The
/// facts the Go call hook (CA.2b) types receivers with; only counted.
fn go_types_marker(nav: &CodeNav) -> String {
    let (mut scopes, mut locals, mut vars) = (0usize, 0usize, 0usize);
    for (scope, names) in &nav.local_types {
        if nav.kind_by_id.get(scope) == Some(&node_kind::MODULE) {
            vars += names.len();
        } else {
            scopes += 1;
            locals += names.len();
        }
    }
    format!(
        "[go-types] return_types={} local_scopes={scopes} locals={locals} package_vars={vars}",
        nav.return_types.len()
    )
}

/// [`build_go`]'s graph with the CB.20 mount stats, for the go_mounts tests.
#[cfg(test)]
pub(crate) fn build_go_with_mounts(
    repo: RepoId,
    parses: Vec<FileParse>,
) -> (RepoGraph, MountStats) {
    let (g, _, _, _, _, mounts) = build_go_passes(repo, parses);
    (g, mounts)
}

/// What [`build_go_passes`] returns: the graph and the stats its markers
/// print.
type GoPasses =
    (RepoGraph, SplitStats, Option<GoImplicitStats>, GoPackageStats, ReceiverTally, MountStats);

/// [`build_go`]'s passes, returning the stats its markers print.
fn build_go_passes(repo: RepoId, parses: Vec<FileParse>) -> GoPasses {
    let (mut g, all_imports, all_calls, mut all_refs) = merge_parses(repo, parses);
    let split = bind_split_go_receivers(&mut g);
    build_symbol_table(&mut g);
    let packages = GoPackages::build(&g, &all_imports);
    let dir_bound_imports = resolve_imports_go(&mut g, &all_imports, &packages);
    let mut tally = EvidenceTally::default();
    let hook = |g: &RepoGraph, site: &CallSite| packages.resolve(g, site);
    resolve_go_calls(&mut g, &all_calls, &split, hook, &mut tally);
    let package_stats = packages.stats(dir_bound_imports);
    let receivers = packages.tally.clone();
    // CB.20: reads the CALLS edges just bound; moves the provisional ROUTEs'
    // HANDLED_BY refs (and copies them to each extra mount) before they resolve.
    let mounts = crate::go_mounts::bind(&mut g, &mut all_refs);
    let (embeds, refs): (Vec<UnresolvedRef>, Vec<UnresolvedRef>) =
        all_refs.into_iter().partition(|r| is_go_embed(&g.nav, r));
    resolve_refs(&mut g, &refs, &mut tally);
    resolve_go_embeds(&mut g, &embeds, &all_imports);
    let implicit = emit_go_implicit_implements(&mut g, &packages);
    emit_method_level_implements(&mut g);
    tally.report();
    (g, split, implicit, package_stats, receivers, mounts)
}

/// Build a per-repo TypeScript graph. TS import sources are raw strings
/// (`./user`, `@angular/core`) that the caller resolves to module qnames via
/// `resolve_source`. Returning `None` treats the import as external (no edge).
///
/// LB.13: a source resolving to the bare form of two same-stem siblings
/// (`./util` beside `util.ts` + `util.js`) binds the one the specifier's
/// extension names, else the one the importer's language loads first
/// (`imports::SameStem`); builders print the `[imports] same-stem
/// picks` marker when any such import was tried.
pub fn build_typescript<R>(
    repo: RepoId,
    parses: Vec<FileParse>,
    resolve_source: R,
) -> Result<RepoGraph, GraphError>
where
    R: Fn(&str, &str) -> Option<String>,
{
    let (mut g, all_imports, all_calls, all_refs) = merge_parses(repo, parses);
    build_symbol_table(&mut g);
    let mut same = same_stem_table(&g);
    resolve_imports_ts(&mut g, &all_imports, &resolve_source, &mut same);
    same.report();
    let mut tally = EvidenceTally::default();
    resolve_calls(&mut g, &all_calls, |_, _| None, &mut tally);
    resolve_refs(&mut g, &all_refs, &mut tally);
    emit_method_level_implements(&mut g);
    tally.report();
    Ok(g)
}

/// Build a per-repo Swift graph (CB.18): [`build_typescript`]'s passes plus
/// Swift's call scope ([`crate::swift_scope`]). Before `resolve_calls`,
/// [`implicit_self`] rewrites a bare call inside a type that names one of
/// the type's members (its own, or one an extension in another file adds)
/// into a self call, so the member wins over a same-named free function the
/// generic pass would bind through the caller file's symbols.
/// `resolve_calls`' `extra_hook` is [`SwiftModuleTypes::resolve`]: every
/// file of a module sees every type of the module without an import, so
/// `Formatter.money(n)` / `Formatter()` on a type declared in another file of
/// the directory bind, and a self call reaches a member of the type's other
/// node (a STRUCT / ENUM / protocol extended in another file). Prints the
/// `[swift-scope]` marker once.
pub fn build_swift<R>(
    repo: RepoId,
    parses: Vec<FileParse>,
    resolve_source: R,
) -> Result<RepoGraph, GraphError>
where
    R: Fn(&str, &str) -> Option<String>,
{
    let (mut g, all_imports, mut all_calls, all_refs) = merge_parses(repo, parses);
    build_symbol_table(&mut g);
    let mut same = same_stem_table(&g);
    resolve_imports_ts(&mut g, &all_imports, &resolve_source, &mut same);
    same.report();
    let types = SwiftModuleTypes::new(&g);
    let rewritten = implicit_self(&g, &types, &mut all_calls);
    let mut tally = EvidenceTally::default();
    resolve_calls(&mut g, &all_calls, |g, site| types.resolve(g, site), &mut tally);
    resolve_refs(&mut g, &all_refs, &mut tally);
    emit_method_level_implements(&mut g);
    eprintln!("{}", types.marker(rewritten));
    tally.report();
    Ok(g)
}

/// Build a per-repo graph for languages whose import paths are dotted
/// (`foo.bar.Baz`) or already normalised to `::` form. Reuses the Python
/// resolver because `.replace('.', "::")` is a no-op on already-`::` paths.
/// Covers Java, Kotlin, C#, PHP, Scala, Clojure, Elixir; Rust has
/// [`build_rust`].
pub fn build_dotted(repo: RepoId, parses: Vec<FileParse>) -> Result<RepoGraph, GraphError> {
    let (mut g, all_imports, all_calls, all_refs) = merge_parses(repo, parses);
    build_symbol_table(&mut g);
    let mut same = same_stem_table(&g);
    resolve_imports_python(&mut g, &all_imports, &mut same);
    same.report();
    let mut tally = EvidenceTally::default();
    resolve_calls(&mut g, &all_calls, |_, _| None, &mut tally);
    resolve_refs(&mut g, &all_refs, &mut tally);
    emit_method_level_implements(&mut g);
    tally.report();
    Ok(g)
}

/// Build a per-repo Rust graph. Same passes as [`build_dotted`], plus the
/// Rust path resolver in `resolve_calls`' `extra_hook` seam: a path call the
/// generic pass misses (`crate::a::f()`, `super::f()`, `Self::f()`,
/// `other_crate::f()`) is resolved against the module tree and `crates`, the
/// Cargo packages the walk found (LA.1a, [`crate::rust_paths`]).
///
/// LA.3: a Bare call inside an inline `mod x { .. }` first tries the
/// enclosing mods' own fns, innermost first (the generic order would bind a
/// same-named file-level fn), and the enum-variant USES refs `resolve_refs`
/// leaves are bound through the same path rules.
///
/// LA.1b: `use` trees resolve as Rust paths ([`resolve_imports_rust`], in
/// place of `resolve_imports_python`): workspace-crate uses, `pub use`
/// re-exports, aliases, globs and fn-body uses bind. A fn's own `use`s and
/// every glob stay in the returned bindings, which the pre-pass (a fn's
/// `use`s shadow the file's) and the hook (the file's globs, last) read.
pub fn build_rust(
    repo: RepoId,
    parses: Vec<FileParse>,
    crates: &[RustCrate],
) -> Result<RepoGraph, GraphError> {
    let (mut g, all_imports, all_calls, all_refs) = merge_parses(repo, parses);
    build_symbol_table(&mut g);
    let idx = RustIndex::build(&g, crates);
    let bindings = resolve_imports_rust(&mut g, &all_imports, &idx);
    // Scoped pre-pass: a hit is a CALLS edge now; a miss (and every other
    // site) keeps its original position for the generic pass.
    let mut rest: Vec<CallSite> = Vec::with_capacity(all_calls.len());
    let mut mod_scoped = 0usize;
    for site in all_calls {
        match idx.resolve_scoped_bare(&g, &site, &bindings) {
            Some((to, rule)) => {
                push_edge(&mut g, site.from, to, edge_category::CALLS, rust_ev(rule).line(site.line));
                mod_scoped += usize::from(rule == MOD_ITEM);
            }
            None => rest.push(site),
        }
    }
    let mut tally = EvidenceTally::default();
    resolve_calls(
        &mut g,
        &rest,
        |g, site| idx.resolve_call(g, site, &bindings),
        &mut tally,
    );
    resolve_refs(&mut g, &all_refs, &mut tally);
    idx.resolve_leftover_refs(&mut g, &bindings);
    emit_method_level_implements(&mut g);
    idx.report();
    idx.report_items(&g, mod_scoped);
    tally.report();
    Ok(g)
}

/// Build a per-repo graph for Ruby. `require 'foo/bar'` imports carry a
/// slash-delimited path; convert to `::` then resolve against the module
/// table the Go-style way.
pub fn build_ruby(repo: RepoId, parses: Vec<FileParse>) -> Result<RepoGraph, GraphError> {
    let (mut g, all_imports, all_calls, all_refs) = merge_parses(repo, parses);
    build_symbol_table(&mut g);
    resolve_imports_slash(&mut g, &all_imports);
    let mut tally = EvidenceTally::default();
    resolve_calls(&mut g, &all_calls, |_, _| None, &mut tally);
    resolve_refs(&mut g, &all_refs, &mut tally);
    emit_method_level_implements(&mut g);
    tally.report();
    Ok(g)
}

/// Build a per-repo C/C++ graph. Every C/C++ file is its own MODULE, named by
/// its file name (`src::Widget.h`, LB.10a); `resolve_source(from, spec)` maps
/// a quoted `#include` to the included file's MODULE qname (the engine's
/// include resolver). An include names a file, never a bare stem, so the
/// LB.13 same-stem table is empty.
///
/// LB.10c: an out-of-line member defined in another file than its class
/// (`void Widget::run() {}` in `Widget.cpp`) joins the class its header
/// declares ([`bind_out_of_line`]): one METHOD under the header's CLASS /
/// STRUCT, so `this->helper()` in the .cpp body resolves through the class.
/// A qualified definition whose qualifier is a namespace (`void shop::init()
/// {}`) becomes a FUNCTION of its file. `resolve_calls`' `extra_hook` is the
/// definition's own file ([`CppCallScope`]): a Bare call of a bound member is
/// looked up in the file that defines it, then every Bare call the generic
/// pass missed in the headers its file directly `#include`s.
pub fn build_c_cpp<R>(
    repo: RepoId,
    parses: Vec<FileParse>,
    resolve_source: R,
) -> Result<RepoGraph, GraphError>
where
    R: Fn(&str, &str) -> Option<String>,
{
    let (mut g, all_imports, mut all_calls, mut all_refs) = merge_parses(repo, parses);
    let (defining, members) = bind_out_of_line(&mut g, &mut all_calls, &mut all_refs);
    build_symbol_table(&mut g);
    resolve_imports_ts(&mut g, &all_imports, &resolve_source, &mut SameStem::default());
    let scope = CppCallScope::new(&g, defining);
    let mut tally = EvidenceTally::default();
    resolve_calls(&mut g, &all_calls, |g, site| scope.resolve(g, site), &mut tally);
    resolve_refs(&mut g, &all_refs, &mut tally);
    emit_method_level_implements(&mut g);
    members.report();
    scope.report();
    tally.report();
    Ok(g)
}

// ============================================================================
// Shared merge: multi-file modules with the same NodeId collapse — their cells
// stack on a single Module node (TS re-exports, or Go files when a caller
// passes one qname per package, as tests/go_smoke does; the engine gives each
// Go file its own MODULE, see [`GoPackages`]).
// ============================================================================

fn merge_parses(
    repo: RepoId,
    parses: Vec<FileParse>,
) -> (
    RepoGraph,
    Vec<ImportStmt>,
    Vec<CallSite>,
    Vec<UnresolvedRef>,
) {
    let mut g = RepoGraph {
        repo,
        nodes: Vec::new(),
        edges: Vec::new(),
        nav: CodeNav::default(),
        symbols: SymbolTable::default(),
        unresolved_calls: Vec::new(),
        unresolved_refs: Vec::new(),
        properties: HashSet::new(),
    };

    let mut all_imports: Vec<ImportStmt> = Vec::new();
    let mut all_calls: Vec<CallSite> = Vec::new();
    let mut all_refs: Vec<UnresolvedRef> = Vec::new();
    let mut index: HashMap<NodeId, usize> = HashMap::new();

    // CB.15: a PACKAGE several files open is one node under its first file
    // (merge_nav), so each member records the file that declared it, read
    // from its own parse before the nav merges.
    let shared = shared_packages(&parses);
    for p in parses {
        if !shared.is_empty() {
            record_home_modules(&p.nav, &shared, &mut g.symbols.home_module);
        }
        for n in p.nodes {
            if let Some(&idx) = index.get(&n.id) {
                // Duplicate NodeId — append cells onto the existing node.
                append_cells(&mut g.nodes[idx].cells, n.cells);
            } else {
                index.insert(n.id, g.nodes.len());
                g.nodes.push(n);
            }
        }
        g.edges.extend(p.edges);
        merge_nav(&mut g.nav, p.nav);
        all_imports.extend(p.imports);
        all_calls.extend(p.calls);
        all_refs.extend(p.refs);
        g.properties.extend(p.properties);
    }
    if !shared.is_empty() {
        eprintln!(
            "[home-module] shared packages={} members={}",
            shared.len(),
            g.symbols.home_module.len()
        );
    }

    // LB.3a: fold framework role overlays (SERVICE / COMPONENT / HOOK / ...)
    // into their same-qname declaration BEFORE any builder builds its symbol
    // table, so module symbols, INJECTS and CALLS bind to the declaration and
    // never to an edgeless marker. Every `build_*` goes through here.
    let stats = crate::roles::fold_role_overlays(&mut g, &mut all_calls, &mut all_refs);
    if stats.saw_role_nodes() {
        eprintln!("{}", stats.marker());
    }

    (g, all_imports, all_calls, all_refs)
}

fn append_cells(existing: &mut Vec<Cell>, incoming: Vec<Cell>) {
    existing.extend(incoming);
}

/// CB.15: the PACKAGE ids that more than one parse records - a C#
/// `namespace Shop.Orders { }` or a braced PHP `namespace App\Orders { }`
/// opened in two files. Every other language's PACKAGE qname carries its
/// file (Ruby / Elixir / Rust / Terraform / Solidity, C++ `<file>::ns`), so
/// this is empty for them and [`merge_parses`] records no home modules.
fn shared_packages(parses: &[FileParse]) -> HashSet<NodeId> {
    let mut seen: HashSet<NodeId> = HashSet::new();
    let mut shared: HashSet<NodeId> = HashSet::new();
    for p in parses {
        for (id, kind) in &p.nav.kind_by_id {
            if *kind == node_kind::PACKAGE && !seen.insert(*id) {
                shared.insert(*id);
            }
        }
    }
    shared
}

/// CB.15: `homes[node] = the MODULE of this parse` for every non-MODULE,
/// non-PACKAGE node of one parse whose PARSE-LOCAL nav chain crosses a
/// `shared` PACKAGE before it reaches that MODULE. Such a node's merged
/// chain runs through the one PACKAGE node, whose parent is the first file
/// that opened it; the entry keeps the node in its own file for call
/// resolution ([`crate::calls::enclosing_home_module`]). Every such node gets
/// its own entry, not only the PACKAGE's direct children: a C# partial class
/// is one CLASS id across files (first entry wins), and a METHOD declared in
/// the second file still resolves through the second file's `using`s. A
/// chain under per-file packages only records nothing, so C++ out-of-line
/// members keep LB.10c's header scope. Within a parse each node is visited
/// once, so the HashMap order never shows; across parses, file order.
fn record_home_modules(
    nav: &CodeNav,
    shared: &HashSet<NodeId>,
    homes: &mut HashMap<NodeId, NodeId>,
) {
    for (&id, &kind) in &nav.kind_by_id {
        if kind == node_kind::MODULE || kind == node_kind::PACKAGE {
            continue;
        }
        if let Some(module) = declaring_module(nav, id, shared) {
            homes.entry(id).or_insert(module);
        }
    }
}

/// The MODULE `id`'s parse-local chain ends at, when a `shared` PACKAGE lies
/// on the way; `None` otherwise, or when the chain reaches no MODULE (a
/// synthetic parse). Bounded by the nav's size, so a malformed cycle ends.
fn declaring_module(nav: &CodeNav, id: NodeId, shared: &HashSet<NodeId>) -> Option<NodeId> {
    let mut crossed = false;
    let mut cur = id;
    for _ in 0..=nav.parent_of.len() {
        cur = *nav.parent_of.get(&cur)?;
        match nav.kind_by_id.get(&cur).copied() {
            Some(k) if k == node_kind::MODULE => return crossed.then_some(cur),
            Some(k) if k == node_kind::PACKAGE => crossed |= shared.contains(&cur),
            _ => {}
        }
    }
    None
}

// ============================================================================
// Nav merge
// ============================================================================

fn merge_nav(dst: &mut CodeNav, src: CodeNav) {
    dst.name_by_id.extend(src.name_by_id);
    dst.qname_by_id.extend(src.qname_by_id);
    dst.kind_by_id.extend(src.kind_by_id);
    // CB.15: a PACKAGE several files open (C# `namespace`, braced PHP
    // `namespace { }`) keeps its FIRST file as nav parent, the file its first
    // POSITION cell names (append_cells keeps that one first); every other
    // record is last-wins. `children_of` still lists it under every file.
    for (k, v) in src.parent_of {
        if dst.kind_by_id.get(&k) == Some(&node_kind::PACKAGE) {
            dst.parent_of.entry(k).or_insert(v);
        } else {
            dst.parent_of.insert(k, v);
        }
    }
    for (k, v) in src.children_of {
        dst.children_of.entry(k).or_default().extend(v);
    }
    // A6.2a: per-owner merge, so a partial class split across files keeps
    // every file's declared fields.
    for (owner, fields) in src.field_types {
        dst.field_types.entry(owner).or_default().extend(fields);
    }
    // LA.35a: per-scope merge. A scope is one fn body, so it comes from one
    // file; `extend` only matters for a fn id two parses share.
    for (scope, locals) in src.local_types {
        dst.local_types.entry(scope).or_default().extend(locals);
    }
    // CB.6: per-scope merge in parser order. A scope comes from one file, so
    // its list moves whole; a fact already recorded for a scope two parses
    // share is kept once, as `CodeNav::record_fact` keeps it within a parse.
    for (scope, facts) in src.nav_facts {
        append_facts(dst.nav_facts.entry(scope).or_default(), facts);
    }
    // CA.2a: one result type per callable; a callable two parses share keeps
    // the first parse's, as `rename_nodes` keeps the surviving id's.
    for (f, ty) in src.return_types {
        dst.return_types.entry(f).or_insert(ty);
    }
    // CA.3a: one signature per METHOD; a METHOD two parses share keeps the
    // first parse's, as `rename_nodes` keeps the surviving id's.
    for (m, sig) in src.method_sigs {
        dst.method_sigs.entry(m).or_insert(sig);
    }
}

/// Append `facts` to a scope's list, dropping one already on it (CB.6).
fn append_facts(list: &mut Vec<NavFact>, facts: Vec<NavFact>) {
    if list.is_empty() {
        *list = facts;
        return;
    }
    for f in facts {
        if !list.contains(&f) {
            list.push(f);
        }
    }
}

// ============================================================================
// Symbol table
// ============================================================================

fn build_symbol_table(g: &mut RepoGraph) {
    for (id, qname) in &g.nav.qname_by_id {
        if g.nav.kind_by_id.get(id) == Some(&node_kind::MODULE) {
            g.symbols.module_by_qname.insert(qname.clone(), *id);
        }
    }
    register_bare_module_aliases(g);

    // module_symbols: for each module, bare name → node id for its top-level defs.
    // Walk children_of; if parent kind == MODULE, child goes in module_symbols.
    for (parent, children) in &g.nav.children_of {
        let parent_kind = g.nav.kind_by_id.get(parent).copied();
        // MODULE (file) and PACKAGE (namespace, e.g. C# `namespace Shop.Services`)
        // both scope top-level type/fn defs. Registering PACKAGE children lets a
        // namespace-scoped type resolve by name (Pattern E INJECTS: C# services
        // live under a PACKAGE node, not a MODULE). A type indexed under both its
        // file MODULE and its namespace PACKAGE is deduped by id in the global
        // lookups, so this doesn't create false ambiguity.
        if parent_kind == Some(node_kind::MODULE) || parent_kind == Some(node_kind::PACKAGE) {
            let entry = g.symbols.module_symbols.entry(*parent).or_default();
            for child in children {
                if let Some(name) = g.nav.name_by_id.get(child) {
                    entry.insert(name.clone(), *child);
                }
            }
        } else if parent_kind == Some(node_kind::CLASS)
            || parent_kind == Some(node_kind::STRUCT)
            || parent_kind == Some(node_kind::ENUM)
        {
            // An ENUM owns its METHOD children exactly like a CLASS / STRUCT
            // (LA.30a: Rust `impl Enum`, Swift, Java / TS enum bodies). Its
            // ATTRIBUTE members are NOT indexed here — methods only.
            let entry = g.symbols.class_methods.entry(*parent).or_default();
            for child in children {
                if let Some(name) = g.nav.name_by_id.get(child)
                    && g.nav.kind_by_id.get(child) == Some(&node_kind::METHOD)
                {
                    entry.insert(name.clone(), *child);
                }
            }
        } else if parent_kind == Some(node_kind::INTERFACE) {
            // A6.6: an INTERFACE's METHOD children (C# / Java interface
            // members, Rust trait fns) go in their OWN table, never
            // `class_methods`: `unique_global_method` scans `class_methods`
            // for the HANDLED_BY fallback, and a second `GetById` there would
            // make a previously-unique handler ambiguous and delete its edge.
            // An interface with no METHOD child gets no entry.
            for child in children {
                if let Some(name) = g.nav.name_by_id.get(child)
                    && g.nav.kind_by_id.get(child) == Some(&node_kind::METHOD)
                {
                    g.symbols
                        .interface_methods
                        .entry(*parent)
                        .or_default()
                        .insert(name.clone(), *child);
                }
            }
        }
    }
}

// ============================================================================
// Go split-file receivers (LA.23d)
// ============================================================================

/// What [`bind_split_go_receivers`] did to one Go graph.
#[derive(Debug, Default)]
struct SplitStats {
    /// Re-parented methods in `g.nodes` order: `(method, its file MODULE,
    /// its STRUCT)`.
    bound: Vec<(NodeId, NodeId, NodeId)>,
    /// Receiver name declared by two or more types in the method's directory
    /// (a broken tree, or a package and its `_test` twin sharing a name).
    ambiguous: usize,
    /// Receiver name no STRUCT in the method's directory declares: a named
    /// non-struct type (`type Status int`, which the parser emits no node
    /// for), a lone INTERFACE, or a type outside the parsed tree.
    unmatched: usize,
}

impl SplitStats {
    /// `[go-recv] split-file methods bound: B (ambiguous=A unmatched=U)`, once
    /// per Go graph with any candidate method.
    fn marker(&self) -> Option<String> {
        (self.bound.len() + self.ambiguous + self.unmatched > 0).then(|| {
            format!(
                "[go-recv] split-file methods bound: {} (ambiguous={} unmatched={})",
                self.bound.len(),
                self.ambiguous,
                self.unmatched
            )
        })
    }
}

/// The receiver segment of a `visit_method` qname under its file MODULE:
/// `service::UserService::Get` under `service` -> `UserService`. Any other
/// shape -> `None`.
fn go_receiver_of<'a>(method_qname: &'a str, module_qname: &str) -> Option<&'a str> {
    let rest = method_qname.strip_prefix(module_qname)?.strip_prefix("::")?;
    let (recv, name) = rest.split_once("::")?;
    (!recv.is_empty() && !name.is_empty() && !name.contains("::")).then_some(recv)
}

/// Re-parent every Go METHOD the parser left under its file MODULE under its
/// receiver STRUCT. `visit_method` only sees the types of its own file, so a
/// method whose struct is declared in another file of the package falls back
/// to the MODULE.
///
/// A Go package is a directory and a MODULE qname is the file path, so a
/// node's package is its MODULE's qname minus the last `::` segment
/// (`svc::users::store` -> `svc::users`). A candidate's receiver is the
/// second-to-last segment of its qname ([`go_receiver_of`]). Exactly one
/// STRUCT / INTERFACE of that name in the directory, and that one a STRUCT,
/// binds: nav `parent_of` / `children_of` move to the struct (appended in
/// `g.nodes` order) and a struct -> method DEFINES edge is added, the
/// same-file shape. The file's module -> method DEFINES edge stays, since the
/// file still defines the method. Two same-named types in one directory bind
/// nothing: a tree that does not compile, or a package and its `_test` twin
/// (same directory) that both declare the name, as grpc-go's per-package
/// `type s struct` test suites do. The package clause is not in the parse,
/// so the directory cannot tell the two apart.
///
/// Runs before [`build_symbol_table`], which then indexes the method in
/// `class_methods[struct]` and leaves it out of its file's `module_symbols`.
/// Deterministic: candidates are visited in `g.nodes` order; the index is a
/// lookup table only.
fn bind_split_go_receivers(g: &mut RepoGraph) -> SplitStats {
    let mut stats = SplitStats::default();
    let mut binds: Vec<(NodeId, NodeId, NodeId)> = Vec::new();
    {
        let nav = &g.nav;
        let module_of = |id: &NodeId| -> Option<(NodeId, &str)> {
            let parent = *nav.parent_of.get(id)?;
            if nav.kind_by_id.get(&parent) != Some(&node_kind::MODULE) {
                return None;
            }
            Some((parent, nav.qname_by_id.get(&parent)?.as_str()))
        };
        let mut types: HashMap<(&str, &str), Vec<NodeId>> = HashMap::new();
        let mut candidates: Vec<(NodeId, NodeId, &str, &str)> = Vec::new();
        for n in &g.nodes {
            let Some(&kind) = nav.kind_by_id.get(&n.id) else { continue };
            let Some((module, module_qname)) = module_of(&n.id) else { continue };
            if kind == node_kind::STRUCT || kind == node_kind::INTERFACE {
                if let Some(name) = nav.name_by_id.get(&n.id) {
                    let ids = types.entry((go_package_dir(module_qname), name.as_str())).or_default();
                    if !ids.contains(&n.id) {
                        ids.push(n.id);
                    }
                }
            } else if kind == node_kind::METHOD
                && let Some(recv) = nav
                    .qname_by_id
                    .get(&n.id)
                    .and_then(|q| go_receiver_of(q, module_qname))
            {
                candidates.push((n.id, module, go_package_dir(module_qname), recv));
            }
        }
        for (method, module, dir, recv) in candidates {
            match types.get(&(dir, recv)).map(Vec::as_slice) {
                Some(&[only]) if nav.kind_by_id.get(&only) == Some(&node_kind::STRUCT) => {
                    binds.push((method, module, only));
                }
                Some([_, _, ..]) => stats.ambiguous += 1,
                _ => stats.unmatched += 1,
            }
        }
    }
    for &(method, module, strukt) in &binds {
        g.nav.parent_of.insert(method, strukt);
        if let Some(kids) = g.nav.children_of.get_mut(&module) {
            kids.retain(|k| *k != method);
        }
        g.nav.children_of.entry(strukt).or_default().push(method);
        let ev = go_ev("split_receiver");
        push_edge(g, strukt, method, edge_category::DEFINES, ev);
    }
    stats.bound = binds;
    stats
}

/// Go call resolution around [`bind_split_go_receivers`]. A bound method's
/// SelfMethod and receiver-field calls need its struct as their owner, which
/// the re-parented nav gives them. Its Bare and Attribute calls name the
/// package-level functions and imports of its OWN file (Go imports are per
/// file), and `resolve_calls` scopes them by walking to the nearest MODULE,
/// which through the struct is the struct's file. So those sites resolve in a
/// second pass with every bound method pointed back at its file MODULE; the
/// struct parents are restored after it. With no bound method this is one
/// `resolve_calls` over every site, as before LA.23d.
///
/// `hook` is the `extra_hook` of both passes ([`GoPackages::resolve`],
/// LA.13b). It scopes a site by its nearest MODULE too, so in the second pass
/// a bound method's Bare call reaches the other files of its OWN file's
/// package.
fn resolve_go_calls<H>(
    g: &mut RepoGraph,
    calls: &[CallSite],
    split: &SplitStats,
    hook: H,
    tally: &mut EvidenceTally,
) where
    H: Fn(&RepoGraph, &CallSite) -> Option<(NodeId, Evidence)> + Copy,
{
    if split.bound.is_empty() {
        resolve_calls(g, calls, hook, tally);
        return;
    }
    let bound: HashSet<NodeId> = split.bound.iter().map(|&(m, _, _)| m).collect();
    let (file_scoped, rest): (Vec<CallSite>, Vec<CallSite>) =
        calls.iter().cloned().partition(|s| {
            matches!(s.qualifier, CallQualifier::Bare(_) | CallQualifier::Attribute { .. })
                && under_bound_method(&g.nav, &bound, s.from)
        });
    resolve_calls(g, &rest, hook, tally);
    for &(method, module, _) in &split.bound {
        g.nav.parent_of.insert(method, module);
    }
    resolve_calls(g, &file_scoped, hook, tally);
    for &(method, _, strukt) in &split.bound {
        g.nav.parent_of.insert(method, strukt);
    }
}

/// True when `id` is in `bound` or sits under a node that is. The walk is
/// bounded by the nav's size, so a malformed parent cycle ends in `false`.
fn under_bound_method(nav: &CodeNav, bound: &HashSet<NodeId>, mut id: NodeId) -> bool {
    for _ in 0..=nav.parent_of.len() {
        if bound.contains(&id) {
            return true;
        }
        match nav.parent_of.get(&id) {
            Some(&parent) => id = parent,
            None => return false,
        }
    }
    false
}

/// LB.9b: a MODULE the engine named by its file name (`api::user.py`, because
/// `api/user.ts` shares its stem in another build group) is still imported by
/// its bare path (`from api.user import validate`, `import './user'`). Each
/// language graph has its own symbol table, so `api::user` here can only mean
/// this graph's file: register it as an alias of that MODULE.
///
/// An alias is registered only when exactly ONE file-named MODULE of this
/// graph has that bare form, and never over a real MODULE qname. Two
/// (`util.js` + `util.ts`, LB.13) register none: the import binds the
/// sibling its importer's language loads (`imports::SameStem`),
/// decided per build and never stored here. Decided by counts over a BTreeMap, so HashMap
/// order cannot pick a winner. The alias persists with the symbol table.
fn register_bare_module_aliases(g: &mut RepoGraph) {
    let mut bare: BTreeMap<String, Vec<NodeId>> = BTreeMap::new();
    for (qname, id) in &g.symbols.module_by_qname {
        let Some(name) = g.nav.name_by_id.get(id) else {
            continue;
        };
        if let Some(b) = bare_module_qname(qname, name) {
            bare.entry(b).or_default().push(*id);
        }
    }
    for (b, ids) in bare {
        if let [id] = ids[..]
            && !g.symbols.module_by_qname.contains_key(&b)
        {
            g.symbols.module_by_qname.insert(b, id);
        }
    }
}

/// A Go node's package: a package is a directory and a MODULE qname is the
/// file path, so it is the MODULE qname minus its last `::` segment
/// (`svc::users::store` -> `svc::users`, a root-level file -> `""`).
fn go_package_dir(module_qname: &str) -> &str {
    module_qname.rsplit_once("::").map_or("", |(dir, _)| dir)
}

// ============================================================================
// Go package = directory (LA.13b)
// ============================================================================

/// The Go packages of one graph, used only while resolving. A Go package is
/// a directory, but every Go file is its own MODULE (the engine passes the
/// file path as the qname: `internal/store/store.go` ->
/// `internal::store::store`), so on its own the generic pass sees one file
/// where Go sees the package: an import binds a single file of the imported
/// package, and a Bare call only finds a def in the caller's own file.
///
/// Neither qnames nor the persisted `module_symbols` change: one MODULE per
/// package would stack every file's cells onto one node and change every Go
/// id, and copying the package's symbols into each file's table would store
/// a 50-file package's table 50 times. Instead [`resolve_imports_go`] binds an
/// import of a directory to one of its files ([`GoPackages::import_target`]),
/// and [`GoPackages::resolve`], `resolve_calls`' `extra_hook`, searches the
/// rest of the package for a call every generic lookup missed. An edge that
/// resolved before still resolves to the same node.
///
/// A `_test.go` file (a MODULE whose last segment ends in `_test`) is built
/// only by `go test`: an importer never sees it and a non-test file never
/// calls into it. It may also be an external `package x_test`, which the
/// directory cannot tell apart (the package clause is not in the parse).
///
/// Deterministic: member lists are sorted by qname and a lookup binds only
/// when exactly one node answers, never the first of several.
pub(crate) struct GoPackages {
    /// Package dir -> its file MODULEs, sorted by qname.
    by_dir: HashMap<String, Vec<NodeId>>,
    /// Package dir -> its file named after the dir (Go's `store/store.go`
    /// convention), when it has one.
    dir_named: HashMap<String, NodeId>,
    /// File MODULE -> its package dir.
    dir_of: HashMap<NodeId, String>,
    /// The `_test.go` file MODULEs.
    tests: HashSet<NodeId>,
    /// Importing file MODULE -> local package name -> import path, for every
    /// `ImportTarget::Module` except a blank (`_`) or dot (`.`) import.
    import_path: HashMap<NodeId, HashMap<String, String>>,
    /// Calls [`GoPackages::package_call`] bound.
    sibling_calls: std::cell::Cell<usize>,
    /// METHOD -> the file MODULE that DEFINES it, for a method whose nav
    /// parent is not its file: a split-file method (LA.23d) sits under a
    /// struct of ANOTHER file, whose imports are not the ones its receiver
    /// facts and result type name packages by (CA.2b).
    method_file: HashMap<NodeId, NodeId>,
    /// Bare type name -> [`unique_global_type`]'s answer, memoised: the
    /// symbol table does not change while calls resolve.
    global_types: std::cell::RefCell<HashMap<String, Option<NodeId>>>,
    /// What [`GoPackages::typed_receiver`] bound and missed (CA.2b).
    tally: ReceiverTally,
}

/// Where an import of a package directory binds ([`GoPackages::import_target`]).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DirImport {
    /// No Go file sits in exactly that directory.
    NoDir,
    /// The directory's only file is the importer itself (an external
    /// `x_test` package importing `x` whose other files are not parsed).
    OnlyImporter,
    /// The file the IMPORTS edge and binding point at.
    Bound(NodeId),
}

/// What [`GoPackages`] did to one Go graph, for the `[go-package]` marker.
#[derive(Debug, Default, PartialEq, Eq)]
struct GoPackageStats {
    /// Package directories (a root-level file's package is `""`).
    dirs: usize,
    /// Directories holding two or more files.
    multi_file: usize,
    /// Imports bound by the package-directory step of
    /// [`resolve_imports_go`].
    dir_bound_imports: usize,
    /// Calls bound in another file of a package by [`GoPackages::resolve`].
    sibling_calls: usize,
}

impl GoPackageStats {
    /// `[go-package] dirs=D multi_file=M dir_bound_imports=I sibling_calls=S`,
    /// once per Go graph with any MODULE.
    fn marker(&self) -> Option<String> {
        (self.dirs > 0).then(|| {
            format!(
                "[go-package] dirs={} multi_file={} dir_bound_imports={} sibling_calls={}",
                self.dirs, self.multi_file, self.dir_bound_imports, self.sibling_calls
            )
        })
    }
}

impl GoPackages {
    /// Index the MODULEs of `g` (after [`build_symbol_table`]) by package
    /// directory, and the imports by importing file and local name.
    fn build(g: &RepoGraph, imports: &[ImportStmt]) -> Self {
        // One entry per MODULE, under the qname its package reads: a file
        // named by its file name (LB.9b, `infra::main.go`) is its bare form
        // (`infra::main`), so its stem still reads `_test` / dir-named, and
        // its bare alias in `module_by_qname` is not a second member.
        let mut modules: Vec<(String, NodeId)> = g
            .symbols
            .module_by_qname
            .iter()
            .filter(|(q, id)| g.nav.qname_by_id.get(*id) == Some(*q))
            .map(|(q, id)| {
                let bare = g.nav.name_by_id.get(id).and_then(|n| bare_module_qname(q, n));
                (bare.unwrap_or_else(|| q.clone()), *id)
            })
            .collect();
        modules.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.0.cmp(&b.1.0)));
        let mut by_dir: HashMap<String, Vec<NodeId>> = HashMap::new();
        let mut dir_named: HashMap<String, NodeId> = HashMap::new();
        let mut dir_of: HashMap<NodeId, String> = HashMap::new();
        let mut tests: HashSet<NodeId> = HashSet::new();
        for (qname, id) in &modules {
            let (qname, id) = (qname.as_str(), *id);
            let dir = go_package_dir(qname);
            let stem = qname.rsplit("::").next().unwrap_or(qname);
            by_dir.entry(dir.to_string()).or_default().push(id);
            dir_of.insert(id, dir.to_string());
            if stem.ends_with("_test") {
                tests.insert(id);
            } else if !dir.is_empty() && dir.rsplit("::").next() == Some(stem) {
                dir_named.insert(dir.to_string(), id);
            }
        }
        let mut import_path: HashMap<NodeId, HashMap<String, String>> = HashMap::new();
        for stmt in imports {
            let ImportTarget::Module { path, alias } = &stmt.target else {
                continue;
            };
            let local = match alias.as_deref() {
                Some("_") | Some(".") => continue,
                Some(a) => a,
                None => path.rsplit("::").next().unwrap_or(path),
            };
            if let Some(&from) = g.symbols.module_by_qname.get(&stmt.from_module) {
                import_path.entry(from).or_default().insert(local.to_string(), path.clone());
            }
        }
        let mut method_file: HashMap<NodeId, NodeId> = HashMap::new();
        for e in &g.edges {
            if e.category == edge_category::DEFINES
                && g.nav.kind_by_id.get(&e.from) == Some(&node_kind::MODULE)
                && g.nav.kind_by_id.get(&e.to) == Some(&node_kind::METHOD)
                && g.nav.parent_of.get(&e.to) != Some(&e.from)
            {
                method_file.insert(e.to, e.from);
            }
        }
        GoPackages {
            by_dir,
            dir_named,
            dir_of,
            tests,
            import_path,
            sibling_calls: std::cell::Cell::new(0),
            method_file,
            global_types: std::cell::RefCell::new(HashMap::new()),
            tally: ReceiverTally::default(),
        }
    }

    /// The file an import of package directory `dir` by file `importer`
    /// binds: the dir-named file, else the first non-test file by qname, else
    /// the first test file; never the importer itself.
    pub(crate) fn import_target(&self, dir: &str, importer: NodeId) -> DirImport {
        let Some(members) = self.by_dir.get(dir) else {
            return DirImport::NoDir;
        };
        let others = || members.iter().copied().filter(move |&m| m != importer);
        self.dir_named
            .get(dir)
            .copied()
            .filter(|&m| m != importer)
            .or_else(|| others().find(|m| !self.tests.contains(m)))
            .or_else(|| others().next())
            .map_or(DirImport::OnlyImporter, DirImport::Bound)
    }

    /// `resolve_calls`' `extra_hook`, consulted only after every generic
    /// lookup missed: a package call ([`GoPackages::package_call`]), else a
    /// method on a typed receiver ([`GoPackages::typed_receiver`], CA.2b). A
    /// base that is not an import no longer ends the hook.
    fn resolve(&self, g: &RepoGraph, site: &CallSite) -> Option<(NodeId, Evidence)> {
        self.package_call(g, site).or_else(|| self.typed_receiver(g, site))
    }

    /// A call into a package:
    ///
    /// * `Bare(name)`: the def `name` of another file in the caller's package
    ///   (a package's top-level names are one scope in Go). A test file's
    ///   defs answer only a test file.
    /// * `Attribute { base, name }` with `base` an import of the caller's
    ///   file: the exported def `name` of a non-test file of the imported
    ///   package (a test file of the same directory also sees its test
    ///   files, the `export_test.go` pattern). That package is the import
    ///   path when the tree has that directory; else, for an import the tail
    ///   fallback bound, the bound file's directory. An import bound to a
    ///   file whose qname IS the path (a file `a/b.go` for an import of `a/b`,
    ///   with no `a/b/` directory) is not widened: that file is not the
    ///   imported package.
    ///
    /// A METHOD never answers (Go calls one only through a value), nor do
    /// `init` (several per package, never callable) or `_`. Two answers (a
    /// build-tag pair, a package and its `_test` twin) bind nothing.
    ///
    /// The hit's evidence (LC.3d) is `graph:go_packages` with rule
    /// `package_sibling` (Bare) or `package_import` (Attribute).
    fn package_call(&self, g: &RepoGraph, site: &CallSite) -> Option<(NodeId, Evidence)> {
        let module = enclosing_module(&g.nav, site.from)?;
        let from_test = self.tests.contains(&module);
        let (hit, rule) = match &site.qualifier {
            CallQualifier::Bare(name) => (
                self.unique_in(g, self.dir_of.get(&module)?, name, module, from_test)?,
                "package_sibling",
            ),
            CallQualifier::Attribute { base, name }
                if name.chars().next().is_some_and(char::is_uppercase) =>
            {
                let dir = self.imported_dir(g, module, base)?;
                let same_dir = self.dir_of.get(&module).is_some_and(|d| d == dir);
                (
                    self.unique_in(g, dir, name, module, from_test && same_dir)?,
                    "package_import",
                )
            }
            _ => return None,
        };
        self.sibling_calls.set(self.sibling_calls.get() + 1);
        Some((hit, go_ev(rule)))
    }

    /// The package directory `module`'s import bound as `base` stands for
    /// (see [`GoPackages::resolve`]).
    fn imported_dir(&self, g: &RepoGraph, module: NodeId, base: &str) -> Option<&str> {
        let path = self.import_path.get(&module)?.get(base)?;
        if let Some((dir, _)) = self.by_dir.get_key_value(path) {
            return Some(dir);
        }
        let bound = *g.symbols.module_import_bindings.get(&module)?.get(base)?;
        if g.nav.kind_by_id.get(&bound) != Some(&node_kind::MODULE)
            || g.nav.qname_by_id.get(&bound) == Some(path)
        {
            return None;
        }
        self.dir_of.get(&bound).map(String::as_str)
    }

    /// The one non-METHOD top-level def `name` across the files of `dir`
    /// other than `caller`, test files included only `with_tests`.
    fn unique_in(
        &self,
        g: &RepoGraph,
        dir: &str,
        name: &str,
        caller: NodeId,
        with_tests: bool,
    ) -> Option<NodeId> {
        if name == "init" || name == "_" {
            return None;
        }
        let mut hit: Option<NodeId> = None;
        for &m in self.by_dir.get(dir)? {
            if m == caller || (!with_tests && self.tests.contains(&m)) {
                continue;
            }
            let Some(&id) = g.symbols.module_symbols.get(&m).and_then(|s| s.get(name)) else {
                continue;
            };
            if g.nav.kind_by_id.get(&id) == Some(&node_kind::METHOD) {
                continue;
            }
            match hit {
                Some(existing) if existing == id => {}
                Some(_) => return None,
                None => hit = Some(id),
            }
        }
        hit
    }

    fn stats(&self, dir_bound_imports: usize) -> GoPackageStats {
        GoPackageStats {
            dirs: self.by_dir.len(),
            multi_file: self.by_dir.values().filter(|m| m.len() > 1).count(),
            dir_bound_imports,
            sibling_calls: self.sibling_calls.get(),
        }
    }

    /// CA.3b: the directory-level import graph ([`DirImportGraph`]): each
    /// file's in-repo imports read through [`GoPackages::imported_dir`] (the
    /// tail-fallback binding included), each dir's union over its files
    /// (test files included: an in-package test can pass the package's own
    /// types on), and each dir's importers. An external `x_test` file's
    /// import of its own dir `x` is kept: that file reaches `x`'s imports
    /// only through it.
    pub(crate) fn dir_import_graph(&self, g: &RepoGraph) -> DirImportGraph<'_> {
        let mut of_file: HashMap<NodeId, BTreeSet<&str>> = HashMap::new();
        let mut of_dir: HashMap<&str, BTreeSet<&str>> = HashMap::new();
        let mut importers: HashMap<&str, BTreeSet<&str>> = HashMap::new();
        for (&module, locals) in &self.import_path {
            let Some(own) = self.dir_of.get(&module).map(String::as_str) else {
                continue;
            };
            for local in locals.keys() {
                let Some(dir) = self.imported_dir(g, module, local) else {
                    continue;
                };
                of_file.entry(module).or_default().insert(dir);
                of_dir.entry(own).or_default().insert(dir);
                importers.entry(dir).or_default().insert(own);
            }
        }
        DirImportGraph { of_file, of_dir, importers, closure: HashMap::new() }
    }

    /// The reachability-gate view of a type or interface `id`: its file
    /// MODULE, that file's package dir and whether it is a `_test.go` file.
    fn side(&self, g: &RepoGraph, id: NodeId) -> Option<GoSide<'_>> {
        let file = self.file_of(g, id)?;
        Some(GoSide {
            file,
            dir: self.dir_of.get(&file)?.as_str(),
            test: self.tests.contains(&file),
        })
    }
}

/// CA.3b: the directory-level import graph of one Go graph
/// ([`GoPackages::dir_import_graph`]), for the implicit-IMPLEMENTS
/// reachability gate ([`DirImportGraph::scope`]). Every set is a BTreeSet and
/// every map is only probed, so no answer depends on a HashMap's seed.
pub(crate) struct DirImportGraph<'p> {
    /// File MODULE -> the in-repo package dirs it imports.
    of_file: HashMap<NodeId, BTreeSet<&'p str>>,
    /// Package dir -> the union of its files' imports.
    of_dir: HashMap<&'p str, BTreeSet<&'p str>>,
    /// Package dir -> the dirs with a file importing it.
    importers: HashMap<&'p str, BTreeSet<&'p str>>,
    /// Package dir -> every dir it reaches through zero or more imports,
    /// memoised on first use (a walk with a seen set: an import cycle
    /// through a test file or a tail-fallback binding cannot loop).
    closure: HashMap<&'p str, BTreeSet<&'p str>>,
}

/// One side of a candidate type -> interface pair ([`GoPackages::side`]).
#[derive(Clone, Copy)]
struct GoSide<'p> {
    file: NodeId,
    dir: &'p str,
    test: bool,
}

/// [`DirImportGraph::scope`]'s verdict on a pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GoReach {
    /// Same package, a transitive import either way, or a shared importer.
    Reached,
    /// Only reached by assuming the repository-root package (dir `""`) is:
    /// the Go parser records no import OF it (`record_import` returns on an
    /// empty repo-local path), so every root-package pair would be lost.
    /// Turns off by itself once root imports are recorded (the root dir then
    /// has importers and closures that reach it).
    RootAssumed,
    Unreached,
}

impl<'p> DirImportGraph<'p> {
    /// Every dir `d` reaches through zero or more imports, `d` included.
    fn closure_of(&mut self, d: &'p str) -> &BTreeSet<&'p str> {
        let of_dir = &self.of_dir;
        self.closure.entry(d).or_insert_with(|| {
            let mut seen: BTreeSet<&'p str> = BTreeSet::new();
            let mut stack = vec![d];
            while let Some(x) = stack.pop() {
                if seen.insert(x) {
                    stack.extend(of_dir.get(x).into_iter().flatten().copied());
                }
            }
            seen
        })
    }

    /// True when a dir of `start` is `target` or imports it transitively.
    fn reaches(&mut self, start: &[&'p str], target: &str) -> bool {
        start.iter().any(|&s| self.closure_of(s).contains(target))
    }

    /// Where a side's code can pass a value on: the imports of its own file
    /// when that is a `_test.go` file (never importable, so its package's
    /// other imports are not its own), else of its whole package.
    fn start(&self, side: GoSide<'p>) -> Vec<&'p str> {
        let set = if side.test { self.of_file.get(&side.file) } else { self.of_dir.get(side.dir) };
        set.into_iter().flatten().copied().collect()
    }

    /// Whether a value of type `t` can reach code typed by interface `i`:
    /// the same package; else `i`'s package reached from `t`'s side (`i` not
    /// in a test file, which nothing imports); else `t`'s package reached
    /// from `i`'s side (`t` not in a test file); else, neither in a test
    /// file, a package importing both. Transitive, because Go lets a file
    /// pass a `T` to a parameter typed `I` without importing `I`'s package.
    fn scope(&mut self, t: GoSide<'p>, i: GoSide<'p>) -> GoReach {
        if t.dir == i.dir {
            return GoReach::Reached;
        }
        let (from_t, from_i) = (self.start(t), self.start(i));
        let shared = !t.test
            && !i.test
            && match (self.importers.get(t.dir), self.importers.get(i.dir)) {
                (Some(a), Some(b)) => !a.is_disjoint(b),
                _ => false,
            };
        if shared
            || (!i.test && self.reaches(&from_t, i.dir))
            || (!t.test && self.reaches(&from_i, t.dir))
        {
            GoReach::Reached
        } else if (!i.test && i.dir.is_empty()) || (!t.test && t.dir.is_empty()) {
            GoReach::RootAssumed
        } else {
            GoReach::Unreached
        }
    }
}

// ============================================================================
// Go typed receivers (CA.2b)
// ============================================================================

/// The longest receiver chain [`GoPackages::typed_receiver`] walks, in
/// segments (`a.b().c.d()` is four).
const RECV_MAX_SEGS: usize = 6;

/// How deep one receiver walk follows recorded type texts into each other (a
/// local's call chain, a callee's result type, a package var's initialiser):
/// bounds the work and ends a self-referential local (`a := a.Next()`).
const RECV_MAX_DEPTH: u8 = 4;

/// Which recorded fact typed the LAST hop of a receiver chain, written as the
/// EVIDENCE rule of the CALLS edge the hook draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RecvSource {
    /// A call's result type (`return_types`), or a conversion `pkg.T(x)`.
    Return,
    /// A parameter or local of the caller (`local_types[caller]`).
    Local,
    /// A package-level var (`local_types[<file MODULE>]`).
    PackageVar,
    /// A struct field's declared type (`field_types`), or the receiver's own
    /// struct (`self`).
    FieldChain,
}

impl RecvSource {
    fn rule(self) -> &'static str {
        match self {
            RecvSource::Return => "receiver_return",
            RecvSource::Local => "receiver_local",
            RecvSource::PackageVar => "receiver_package_var",
            RecvSource::FieldChain => "receiver_field_chain",
        }
    }
}

/// One link of a receiver chain as the Go parser normalises it (CA.2a
/// `chain_text`): a name `repo`, or a call `Repo()` (arguments elided).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Seg<'s> {
    Ident(&'s str),
    Call(&'s str),
}

/// Split a normalised chain (`Services.UserRepository()`, `self.deps.repo`,
/// `repo`) into its links. Anything else — the raw text of a chain through an
/// index, a type assertion or a literal, or a chain longer than
/// [`RECV_MAX_SEGS`] — is `None`.
fn receiver_segments(text: &str) -> Option<Vec<Seg<'_>>> {
    let mut segs = Vec::new();
    for part in text.split('.') {
        let (name, call) = match part.strip_suffix("()") {
            Some(name) => (name, true),
            None => (part, false),
        };
        let ident = name.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_alphanumeric() || c == '_');
        if !ident || segs.len() == RECV_MAX_SEGS {
            return None;
        }
        segs.push(if call { Seg::Call(name) } else { Seg::Ident(name) });
    }
    Some(segs)
}

/// True for a Go exported name (upper-case first letter): the only names
/// another package can reach.
fn is_exported(name: &str) -> bool {
    name.chars().next().is_some_and(char::is_uppercase)
}

/// The METHOD `name` a value of type `ty` has: its declared method, else (an
/// INTERFACE) the interface's own METHOD, which A6.6's method-level
/// IMPLEMENTS carries on to the implementations.
fn method_on(g: &RepoGraph, ty: NodeId, name: &str) -> Option<NodeId> {
    g.symbols
        .class_methods
        .get(&ty)
        .and_then(|m| m.get(name).copied())
        .or_else(|| g.symbols.interface_methods.get(&ty).and_then(|m| m.get(name).copied()))
}

/// Where a receiver chain stands after a hop: a type, or an imported package
/// (only ever the chain's root).
#[derive(Clone, Copy)]
enum Hop<'p> {
    Type(NodeId),
    Package(&'p str),
}

/// The scope a chain's names are read in.
#[derive(Clone, Copy)]
struct RecvScope<'p> {
    /// The fn / METHOD whose locals the chain's root may name; `None` at
    /// package scope (a package var's initialiser, a callee's result type).
    func: Option<NodeId>,
    /// The file MODULE whose imports and package the chain's names resolve
    /// through.
    module: NodeId,
    /// The call site's own package dir.
    caller_dir: &'p str,
    /// The call site's file is a `_test.go` file.
    from_test: bool,
}

impl RecvScope<'_> {
    /// A test file of `dir` answers only a test caller in that same package
    /// ([`GoPackages::unique_in`]'s `with_tests` rule).
    fn with_tests(&self, dir: &str) -> bool {
        self.from_test && dir == self.caller_dir
    }
}

/// What [`GoPackages::typed_receiver`] did to one Go graph, as Cell counters
/// (the hook takes `&self`), for the `[go-receivers]` marker.
#[derive(Debug, Default, Clone)]
struct ReceiverTally {
    ret: std::cell::Cell<usize>,
    local: std::cell::Cell<usize>,
    package_var: std::cell::Cell<usize>,
    field_chain: std::cell::Cell<usize>,
    /// Sites whose receiver had a recorded type text (a local, package var,
    /// result or field type) but whose type or method did not resolve: a
    /// type of another module (`gin.Context`), an ambiguous name, a method
    /// promoted from an embedded field.
    typed_unbound: std::cell::Cell<usize>,
}

impl ReceiverTally {
    fn bound(&self, source: RecvSource) {
        let c = match source {
            RecvSource::Return => &self.ret,
            RecvSource::Local => &self.local,
            RecvSource::PackageVar => &self.package_var,
            RecvSource::FieldChain => &self.field_chain,
        };
        c.set(c.get() + 1);
    }

    /// `[go-receivers] bound=<n> (return=<a> local=<b> package_var=<c>
    /// field_chain=<d>) typed_unbound=<u>`, once per Go graph.
    fn marker(&self) -> String {
        let (r, l, p, f) =
            (self.ret.get(), self.local.get(), self.package_var.get(), self.field_chain.get());
        format!(
            "[go-receivers] bound={} (return={r} local={l} package_var={p} field_chain={f}) \
             typed_unbound={}",
            r + l + p + f,
            self.typed_unbound.get()
        )
    }
}

impl GoPackages {
    /// CA.2b: a method call on a typed receiver, the second half of
    /// [`GoPackages::resolve`] (so only after every generic lookup and the
    /// package call missed). The receiver is an `Attribute` base or a
    /// `ComplexReceiver` chain ([`receiver_segments`]); [`GoPackages::chain_type`]
    /// types it through the facts CA.2a recorded, and the method binds on
    /// that type ([`method_on`]). Every lookup is exact-name, kind-filtered
    /// and unique-or-nothing. Evidence `graph:go_packages` with rule
    /// `receiver_return` / `receiver_local` / `receiver_package_var` /
    /// `receiver_field_chain`: the fact that typed the chain's last hop.
    fn typed_receiver(&self, g: &RepoGraph, site: &CallSite) -> Option<(NodeId, Evidence)> {
        let (segs, name) = match &site.qualifier {
            CallQualifier::Attribute { base, name } => (receiver_segments(base)?, name),
            CallQualifier::ComplexReceiver { receiver, name } => {
                (receiver_segments(receiver)?, name)
            }
            _ => return None,
        };
        if name.is_empty() {
            return None;
        }
        let module = self.file_of(g, site.from)?;
        let scope = RecvScope {
            func: (g.nav.kind_by_id.get(&site.from) != Some(&node_kind::MODULE))
                .then_some(site.from),
            module,
            caller_dir: self.dir_of.get(&module)?,
            from_test: self.tests.contains(&module),
        };
        let mut typed = false;
        let hit = self
            .chain_type(g, scope, &segs, 0, &mut typed)
            .and_then(|(ty, source)| Some((method_on(g, ty, name)?, source)));
        match hit {
            Some((method, source)) => {
                self.tally.bound(source);
                Some((method, go_ev(source.rule())))
            }
            None => {
                if typed {
                    self.tally.typed_unbound.set(self.tally.typed_unbound.get() + 1);
                }
                None
            }
        }
    }

    /// The file MODULE a node's facts are written in: a split-file method's
    /// own file ([`GoPackages::method_file`]), else its nearest MODULE.
    fn file_of(&self, g: &RepoGraph, id: NodeId) -> Option<NodeId> {
        self.method_file.get(&id).copied().or_else(|| enclosing_module(&g.nav, id))
    }

    /// The type a receiver chain `segs` read in `scope` evaluates to, with
    /// the fact that typed its last hop. `typed` turns true once any
    /// recorded type text was consulted.
    fn chain_type<'a>(
        &'a self,
        g: &RepoGraph,
        scope: RecvScope<'a>,
        segs: &[Seg<'_>],
        depth: u8,
        typed: &mut bool,
    ) -> Option<(NodeId, RecvSource)> {
        if depth > RECV_MAX_DEPTH || segs.len() > RECV_MAX_SEGS {
            return None;
        }
        let (&root, rest) = segs.split_first()?;
        let (mut hop, mut source) = self.root_hop(g, scope, root, depth, typed)?;
        for &seg in rest {
            (hop, source) = self.next_hop(g, scope, hop, seg, depth, typed)?;
        }
        match hop {
            Hop::Type(ty) => Some((ty, source)),
            Hop::Package(_) => None,
        }
    }

    /// A chain's first link, Go's scoping innermost first:
    ///
    /// * `x` a local or parameter of the scope's fn: its recorded type text
    ///   ([`GoPackages::type_of_text`]); `""` (unknown, it shadows) -> `None`.
    /// * `self`: the receiver's own struct.
    /// * `x` an import of the scope's file: that package.
    /// * `x` a package var of the scope's package ([`GoPackages::package_var_type`]).
    /// * `f()`: the result type of the function `f` of the scope's file,
    ///   else of its package; a type `T(x)` is a conversion to `T`. A local
    ///   named `f` (a func value) shadows it: unknown.
    fn root_hop<'a>(
        &'a self,
        g: &RepoGraph,
        scope: RecvScope<'a>,
        seg: Seg<'_>,
        depth: u8,
        typed: &mut bool,
    ) -> Option<(Hop<'a>, RecvSource)> {
        let local = |name: &str| {
            let f = scope.func?;
            g.nav.local_types.get(&f)?.get(name).map(String::as_str)
        };
        match seg {
            Seg::Ident(x) => {
                if let Some(text) = local(x) {
                    if text.is_empty() {
                        return None;
                    }
                    *typed = true;
                    let ty = self.type_of_text(g, scope, text, depth + 1, typed)?;
                    return Some((Hop::Type(ty), RecvSource::Local));
                }
                if x == "self" {
                    let owner = enclosing_class_or_struct(&g.nav, scope.func?)?;
                    return Some((Hop::Type(owner), RecvSource::FieldChain));
                }
                if let Some(dir) = self.imported_dir(g, scope.module, x) {
                    return Some((Hop::Package(dir), RecvSource::Return));
                }
                let dir = self.dir_of.get(&scope.module)?;
                let ty = self.package_var_type(g, scope, dir, x, depth, typed)?;
                Some((Hop::Type(ty), RecvSource::PackageVar))
            }
            Seg::Call(f) => {
                if local(f).is_some() {
                    return None;
                }
                let dir = self.dir_of.get(&scope.module)?;
                let callee = g
                    .symbols
                    .module_symbols
                    .get(&scope.module)
                    .and_then(|s| s.get(f).copied())
                    .or_else(|| self.unique_in(g, dir, f, scope.module, scope.with_tests(dir)))?;
                let ty = self.callee_type(g, scope, callee, depth, typed)?;
                Some((Hop::Type(ty), RecvSource::Return))
            }
        }
    }

    /// A chain's next link from `hop`:
    ///
    /// * package + `F()`: the exported function `F` of that package -> its
    ///   result type; a type `T` -> `T` itself (a conversion `pkg.T(x)`).
    /// * package + `V`: the exported package var `V` of that package.
    /// * type `T` + `f`: `T`'s field `f` -> its declared type, a bare name
    ///   (LA.23c) looked up in `T`'s package, else repo-wide
    ///   ([`GoPackages::global_type`]).
    /// * type `T` + `m()`: `T`'s method `m` -> its result type, read in the
    ///   method's own file.
    fn next_hop<'a>(
        &'a self,
        g: &RepoGraph,
        scope: RecvScope<'a>,
        hop: Hop<'a>,
        seg: Seg<'_>,
        depth: u8,
        typed: &mut bool,
    ) -> Option<(Hop<'a>, RecvSource)> {
        match (hop, seg) {
            (Hop::Package(dir), Seg::Call(f)) => {
                if !is_exported(f) {
                    return None;
                }
                let callee = self.unique_in(g, dir, f, scope.module, scope.with_tests(dir))?;
                let ty = self.callee_type(g, scope, callee, depth, typed)?;
                Some((Hop::Type(ty), RecvSource::Return))
            }
            (Hop::Package(dir), Seg::Ident(v)) => {
                if !is_exported(v) {
                    return None;
                }
                let ty = self.package_var_type(g, scope, dir, v, depth, typed)?;
                Some((Hop::Type(ty), RecvSource::PackageVar))
            }
            (Hop::Type(owner), Seg::Ident(field)) => {
                let name = g
                    .nav
                    .field_types
                    .get(&owner)?
                    .get(field)
                    .filter(|t| !t.is_empty())?;
                *typed = true;
                let owner_dir = self.dir_of.get(&enclosing_module(&g.nav, owner)?)?;
                let ty = self
                    .unique_type_in(g, owner_dir, name, scope.with_tests(owner_dir))
                    .or_else(|| self.global_type(g, name, scope.from_test))?;
                Some((Hop::Type(ty), RecvSource::FieldChain))
            }
            (Hop::Type(owner), Seg::Call(m)) => {
                let method = method_on(g, owner, m)?;
                let ret = g.nav.return_types.get(&method)?;
                *typed = true;
                let at = RecvScope { func: None, module: self.file_of(g, method)?, ..scope };
                let ty = self.type_of_text(g, at, ret, depth + 1, typed)?;
                Some((Hop::Type(ty), RecvSource::Return))
            }
        }
    }

    /// What calling `callee` yields: a FUNCTION's recorded result type, read
    /// in the function's own file; a STRUCT / INTERFACE itself (a
    /// conversion). Any other kind -> `None`.
    fn callee_type(
        &self,
        g: &RepoGraph,
        scope: RecvScope<'_>,
        callee: NodeId,
        depth: u8,
        typed: &mut bool,
    ) -> Option<NodeId> {
        let kind = *g.nav.kind_by_id.get(&callee)?;
        if kind == node_kind::STRUCT || kind == node_kind::INTERFACE {
            return Some(callee);
        }
        if kind != node_kind::FUNCTION {
            return None;
        }
        let ret = g.nav.return_types.get(&callee)?;
        *typed = true;
        let at = RecvScope { func: None, module: self.file_of(g, callee)?, ..scope };
        self.type_of_text(g, at, ret, depth + 1, typed)
    }

    /// The type of package var `name` of package `dir`: every file of `dir`
    /// that declares it (a build-tag pair may declare it twice) must record
    /// one and the same type text, and that text must resolve, in each such
    /// file, to one and the same type. A test file's vars answer only a test
    /// caller in its own package.
    fn package_var_type(
        &self,
        g: &RepoGraph,
        scope: RecvScope<'_>,
        dir: &str,
        name: &str,
        depth: u8,
        typed: &mut bool,
    ) -> Option<NodeId> {
        let with_tests = scope.with_tests(dir);
        let mut text: Option<&str> = None;
        let mut answer: Option<NodeId> = None;
        for &m in self.by_dir.get(dir)? {
            if !with_tests && self.tests.contains(&m) {
                continue;
            }
            let Some(t) = g.nav.local_types.get(&m).and_then(|vars| vars.get(name)) else {
                continue;
            };
            match text {
                Some(seen) if seen != t => return None,
                _ => text = Some(t),
            }
            if t.is_empty() {
                return None;
            }
            *typed = true;
            let at = RecvScope { func: None, module: m, ..scope };
            let ty = self.type_of_text(g, at, t, depth + 1, typed)?;
            match answer {
                Some(a) if a != ty => return None,
                _ => answer = Some(ty),
            }
        }
        answer
    }

    /// The type a recorded type text names, read in `scope`: a call chain
    /// (`Services.UserRepository()`, a `self.<field>` alias) is walked as a
    /// chain; `pkg.T` is the STRUCT / INTERFACE `T` of the package the
    /// scope's file imports as `pkg`; a bare `T` is the one of the scope's
    /// own package, else the repo-unique type ([`GoPackages::global_type`]).
    fn type_of_text(
        &self,
        g: &RepoGraph,
        scope: RecvScope<'_>,
        text: &str,
        depth: u8,
        typed: &mut bool,
    ) -> Option<NodeId> {
        if depth > RECV_MAX_DEPTH {
            return None;
        }
        let segs = receiver_segments(text)?;
        let chain = segs.first() == Some(&Seg::Ident("self"))
            || segs.iter().any(|s| matches!(s, Seg::Call(_)));
        if chain {
            return self.chain_type(g, scope, &segs, depth, typed).map(|(ty, _)| ty);
        }
        match segs.as_slice() {
            [Seg::Ident(pkg), Seg::Ident(name)] => {
                let dir = self.imported_dir(g, scope.module, pkg)?;
                self.unique_type_in(g, dir, name, scope.with_tests(dir))
            }
            [Seg::Ident(name)] => {
                let dir = self.dir_of.get(&scope.module)?;
                self.unique_type_in(g, dir, name, scope.with_tests(dir))
                    .or_else(|| self.global_type(g, name, scope.from_test))
            }
            _ => None,
        }
    }

    /// [`GoPackages::unique_in`] for a type: the one STRUCT / INTERFACE
    /// `name` across every file of `dir` (the caller's own included), test
    /// files only `with_tests`. A same-named FUNCTION never answers.
    fn unique_type_in(
        &self,
        g: &RepoGraph,
        dir: &str,
        name: &str,
        with_tests: bool,
    ) -> Option<NodeId> {
        let mut hit: Option<NodeId> = None;
        for &m in self.by_dir.get(dir)? {
            if !with_tests && self.tests.contains(&m) {
                continue;
            }
            let Some(&id) = g.symbols.module_symbols.get(&m).and_then(|s| s.get(name)) else {
                continue;
            };
            let kind = g.nav.kind_by_id.get(&id);
            if kind != Some(&node_kind::STRUCT) && kind != Some(&node_kind::INTERFACE) {
                continue;
            }
            match hit {
                Some(existing) if existing == id => {}
                Some(_) => return None,
                None => hit = Some(id),
            }
        }
        hit
    }

    /// The repo-unique type `name` ([`unique_global_type`], memoised), unless
    /// it is declared in a test file and the caller is not one.
    fn global_type(&self, g: &RepoGraph, name: &str, from_test: bool) -> Option<NodeId> {
        let hit = *self
            .global_types
            .borrow_mut()
            .entry(name.to_string())
            .or_insert_with(|| unique_global_type(g, name));
        let hit = hit?;
        let in_test = enclosing_module(&g.nav, hit).is_some_and(|m| self.tests.contains(&m));
        (from_test || !in_test).then_some(hit)
    }
}

// ============================================================================
// Go implicit interface satisfaction (LD.7b)
// ============================================================================

/// An embed ref of the Go parser: INHERITS_FROM out of an INTERFACE (one
/// `type_elem` naming a single type).
fn is_go_embed(nav: &CodeNav, r: &UnresolvedRef) -> bool {
    r.category == edge_category::INHERITS_FROM
        && nav.kind_by_id.get(&r.from) == Some(&node_kind::INTERFACE)
}

/// Bind each Go embed ref to the INTERFACE it names, Go's way: `R` is the
/// interface `R` of the embedding interface's own package (directory), and
/// `pkg.R` the interface `R` of the package the file imports as `pkg` (its
/// import path, or a directory ending in it when the path has two or more
/// segments, for a go.mod below the repo root). Exactly one INTERFACE must
/// match, else the ref stays in `unresolved_refs`: another module's
/// interface (`io.Reader`), a predeclared one (`error`, `comparable`), a
/// constraint's exact type term (`interface{ MyStruct }`), or an ambiguous
/// name.
///
/// Not `resolve_refs`: its repo-wide by-name fallback binds any same-named
/// node, and on grpc-go bound `ServerStream` (a same-package interface) to a
/// DATA_ENTITY of that name. Edges are pushed in ref (parse) order.
fn resolve_go_embeds(g: &mut RepoGraph, embeds: &[UnresolvedRef], imports: &[ImportStmt]) {
    if embeds.is_empty() {
        return;
    }
    let mut bound: Vec<(NodeId, NodeId, &str, u32)> = Vec::new();
    let mut unbound: Vec<UnresolvedRef> = Vec::new();
    {
        let nav = &g.nav;
        let mut by_dir_name: HashMap<(&str, &str), Vec<NodeId>> = HashMap::new();
        let mut by_name: HashMap<&str, Vec<(&str, NodeId)>> = HashMap::new();
        for n in &g.nodes {
            if nav.kind_by_id.get(&n.id) != Some(&node_kind::INTERFACE) {
                continue;
            }
            let (Some(name), Some(parent)) = (nav.name_by_id.get(&n.id), nav.parent_of.get(&n.id)) else {
                continue;
            };
            let Some(module_qname) = nav.qname_by_id.get(parent) else {
                continue;
            };
            let dir = go_package_dir(module_qname);
            by_dir_name.entry((dir, name.as_str())).or_default().push(n.id);
            by_name.entry(name.as_str()).or_default().push((dir, n.id));
        }
        // (importing file's MODULE qname, local package name) -> import path.
        let mut import_paths: HashMap<(&str, &str), &str> = HashMap::new();
        for stmt in imports {
            let ImportTarget::Module { path, alias } = &stmt.target else {
                continue;
            };
            let local = match alias.as_deref() {
                Some("_") | Some(".") => continue,
                Some(a) => a,
                None => path.rsplit("::").next().unwrap_or(path),
            };
            import_paths.insert((stmt.from_module.as_str(), local), path.as_str());
        }
        let only = |ids: &[NodeId]| match ids {
            [one] => Some(*one),
            _ => None,
        };
        for r in embeds {
            let module_qname = nav.qname_by_id.get(&r.from_module).map_or("", String::as_str);
            let hit = match &r.qualifier {
                CallQualifier::Bare(name) => by_dir_name
                    .get(&(go_package_dir(module_qname), name.as_str()))
                    .and_then(|ids| only(ids))
                    .map(|id| (id, "embed_package")),
                CallQualifier::Attribute { base, name } => {
                    import_paths.get(&(module_qname, base.as_str())).and_then(|&path| {
                        let suffix = format!("::{path}");
                        let ids: Vec<NodeId> = by_name
                            .get(name.as_str())
                            .into_iter()
                            .flatten()
                            .filter(|(dir, _)| {
                                *dir == path || (path.contains("::") && dir.ends_with(&suffix))
                            })
                            .map(|&(_, id)| id)
                            .collect();
                        only(&ids).map(|id| (id, "embed_import"))
                    })
                }
                _ => None,
            };
            match hit {
                Some((to, rule)) if to != r.from => bound.push((r.from, to, rule, r.line)),
                _ => unbound.push(r.clone()),
            }
        }
    }
    for (from, to, rule, line) in bound {
        push_edge(g, from, to, edge_category::INHERITS_FROM, go_ev(rule).line(line));
    }
    g.unresolved_refs.extend(unbound);
}

/// The evidence of an edge a Go package-as-directory pass drew (LC.3d):
/// `graph:go_packages` with rule `split_receiver` (LA.23d), `package_sibling`
/// / `package_import` (the LA.13b call hook), `receiver_return` /
/// `receiver_local` / `receiver_package_var` / `receiver_field_chain` (the
/// CA.2b typed-receiver half of that hook) or `embed_package` /
/// `embed_import` (LD.7b interface embeds).
fn go_ev(rule: &str) -> Evidence {
    graph_evidence("graph:go_packages", rule)
}

/// Method sets of the predeclared interfaces a Go interface can embed, each
/// method with its normalised signature (CA.3a's `(<params>)(<results>)`
/// text). No parse declares them, so their embed ref stays unresolved; this
/// is what they contribute instead of leaving the embedding interface's set
/// unknown.
const GO_PREDECLARED_IFACES: &[(&str, &[(&str, &str)])] = &[("error", &[("Error", "()(string)")])];

/// What [`emit_go_implicit_implements`] did to one Go graph.
#[derive(Debug, Default, PartialEq, Eq)]
struct GoImplicitStats {
    /// IMPLEMENTS edges pushed.
    edges: usize,
    /// Interfaces with a known, non-empty method set: the ones matched.
    interfaces: usize,
    /// Distinct types given at least one IMPLEMENTS edge.
    types: usize,
    /// Distinct interface -> interface INHERITS_FROM edges (embeds) followed.
    embedded: usize,
    /// Interfaces not matched because an embed did not bind, so their method
    /// set is not fully known.
    open: usize,
    /// CA.3b: name-covering pairs rejected because a method's signature, known
    /// on both sides, differs from the interface's (a type alias is not
    /// resolved, so `ID` vs `string` counts here too).
    signature: usize,
    /// Pairs rejected by the reachability gate, neither side in a test file
    /// (a one-method interface).
    one_method: usize,
    /// Pairs rejected by the reachability gate with a side in a `_test.go`
    /// file.
    test_side: usize,
    /// Edges pushed whose every method signature was compared (EVIDENCE rule
    /// `method_signature`; the rest keep `method_set`).
    signature_checked: usize,
    /// Edges pushed only by assuming the repository-root package reachable
    /// ([`GoReach::RootAssumed`]).
    root_assumed: usize,
}

impl GoImplicitStats {
    /// `[iface] go implicit implements: E (interfaces=I types=T embedded=M
    /// open=O)`, once per Go graph that has an INTERFACE.
    fn marker(&self) -> String {
        format!(
            "[iface] go implicit implements: {} (interfaces={} types={} embedded={} open={})",
            self.edges, self.interfaces, self.types, self.embedded, self.open
        )
    }

    /// CA.3b fired_on, after [`GoImplicitStats::marker`]: `[iface] go implicit
    /// filtered: signature=S scope=P (one_method=O test_side=X)
    /// signature_checked=K root_assumed=R`.
    fn filtered_marker(&self) -> String {
        format!(
            "[iface] go implicit filtered: signature={} scope={} (one_method={} test_side={}) \
             signature_checked={} root_assumed={}",
            self.signature,
            self.one_method + self.test_side,
            self.one_method,
            self.test_side,
            self.signature_checked,
            self.root_assumed
        )
    }
}

/// Go satisfies interfaces implicitly: a named type implements an interface
/// when its method set covers the interface's (own + embedded,
/// transitively). The edge is inferred, so every one is `Confidence::Medium`.
///
/// CA.3b gates a name-covering pair twice:
///
/// * Signatures (every pair): each interface method's normalised signature
///   (`nav.method_sigs`, CA.3a; the predeclared `error`'s `Error` is
///   `()(string)`) against the type's same-named method's. Both known and
///   different rejects the pair. All known and equal gives the edge EVIDENCE
///   rule `method_signature`; any unknown (a generic receiver, an element of
///   a generic interface) keeps today's name match, rule `method_set`.
/// * Reachability (a one-method interface, or a side declared in a
///   `_test.go` file, where a name match alone is noise): the two packages
///   must be linked by an import path ([`DirImportGraph::scope`]); the
///   repository-root package, whose imports the parser does not record, is
///   assumed reached ([`GoReach::RootAssumed`]).
///
/// The rest of the rules:
///
/// * An interface's own methods are its `interface_methods` (the parser's
///   `method_elem` METHOD children). Its embedded interfaces are its
///   INHERITS_FROM edges to another INTERFACE, walked depth-first with a
///   visited set, so an embedding cycle (which Go rejects) cannot loop.
/// * An embed that did not become such an edge leaves the set unknown and
///   the interface is skipped (`open`): an INHERITS_FROM ref still in
///   `unresolved_refs` (another module's `io.Reader`, `comparable`) or an edge
///   to a non-INTERFACE. The predeclared `error` is the exception and
///   contributes `Error` ([`GO_PREDECLARED_IFACES`]). Matching on the known
///   part instead would pair `type ReadCloser interface { io.Reader; Close()
///   error }` with every type that has a `Close`.
/// * An empty set (`interface{}`, a constraint of type terms only) is
///   implemented by nothing here.
/// * An unexported method name is package-scoped in Go: it matches only a
///   type in the package (directory, [`go_package_dir`]) of the interface
///   that declares it.
/// * Types are the STRUCT / CLASS owners in `class_methods`. Pointer and
///   value receivers both count toward a type's set, and methods promoted
///   from an embedded struct field are not seen.
///
/// Runs after [`resolve_go_embeds`] (the embed refs are bound) and before
/// `emit_method_level_implements`, which then pairs each new edge's methods.
/// Pairs are sorted by id and deduped before any edge is pushed (the method
/// tables are HashMaps with per-process seeds, and edge order feeds the
/// store's shard hashes), and an IMPLEMENTS edge already present is not
/// pushed again. `None` when the graph has no INTERFACE.
fn emit_go_implicit_implements(
    g: &mut RepoGraph,
    packages: &GoPackages,
) -> Option<GoImplicitStats> {
    let ifaces: Vec<NodeId> = g
        .nodes
        .iter()
        .map(|n| n.id)
        .filter(|id| g.nav.kind_by_id.get(id) == Some(&node_kind::INTERFACE))
        .collect();
    if ifaces.is_empty() {
        return None;
    }
    let mut stats = GoImplicitStats::default();
    // (type, interface, every signature compared, root package assumed).
    let mut pairs: Vec<(NodeId, NodeId, bool, bool)> = Vec::new();
    {
        let nav = &g.nav;
        let mut dirs = packages.dir_import_graph(g);
        let is_iface = |id: &NodeId| nav.kind_by_id.get(id) == Some(&node_kind::INTERFACE);
        let pkg_of = |id: &NodeId| -> Option<&str> {
            let parent = nav.parent_of.get(id)?;
            if nav.kind_by_id.get(parent) != Some(&node_kind::MODULE) {
                return None;
            }
            nav.qname_by_id.get(parent).map(|q| go_package_dir(q))
        };

        let mut embeds: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        let mut embed_pairs: HashSet<(NodeId, NodeId)> = HashSet::new();
        let mut open: HashSet<NodeId> = HashSet::new();
        for e in &g.edges {
            if e.category != edge_category::INHERITS_FROM || !is_iface(&e.from) {
                continue;
            }
            if !is_iface(&e.to) {
                open.insert(e.from);
            } else if embed_pairs.insert((e.from, e.to)) {
                embeds.entry(e.from).or_default().push(e.to);
            }
        }
        stats.embedded = embed_pairs.len();
        let mut predeclared: HashMap<NodeId, Vec<(&'static str, &'static str)>> = HashMap::new();
        for r in &g.unresolved_refs {
            if r.category != edge_category::INHERITS_FROM || !is_iface(&r.from) {
                continue;
            }
            let known = match &r.qualifier {
                CallQualifier::Bare(name) => {
                    GO_PREDECLARED_IFACES.iter().find(|(n, _)| n == name).map(|(_, ms)| *ms)
                }
                _ => None,
            };
            match known {
                Some(ms) => predeclared.entry(r.from).or_default().extend(ms.iter().copied()),
                None => {
                    open.insert(r.from);
                }
            }
        }

        // Method name -> the types declaring it, to narrow each interface's
        // candidates to the holders of its rarest method name.
        let mut by_method: HashMap<&str, Vec<NodeId>> = HashMap::new();
        for (owner, methods) in &g.symbols.class_methods {
            let kind = nav.kind_by_id.get(owner);
            if kind != Some(&node_kind::STRUCT) && kind != Some(&node_kind::CLASS) {
                continue;
            }
            for name in methods.keys() {
                by_method.entry(name.as_str()).or_default().push(*owner);
            }
        }

        for &iface in &ifaces {
            // (method name, declaring package when the name is unexported) ->
            // the signature of the interface METHOD declaring it (the first
            // in the embed walk), `None` when unknown.
            let mut set: BTreeMap<(&str, Option<&str>), Option<&str>> = BTreeMap::new();
            let mut visited: HashSet<NodeId> = HashSet::new();
            let mut stack = vec![iface];
            let mut known = true;
            'walk: while let Some(i) = stack.pop() {
                if !visited.insert(i) {
                    continue;
                }
                if open.contains(&i) {
                    known = false;
                    break;
                }
                for (name, mid) in g.symbols.interface_methods.get(&i).into_iter().flatten() {
                    let key = if name.chars().next().is_some_and(char::is_uppercase) {
                        (name.as_str(), None)
                    } else if let Some(pkg) = pkg_of(&i) {
                        (name.as_str(), Some(pkg))
                    } else {
                        known = false;
                        break 'walk;
                    };
                    set.entry(key).or_insert_with(|| nav.method_sigs.get(mid).map(String::as_str));
                }
                for &(name, sig) in predeclared.get(&i).into_iter().flatten() {
                    set.entry((name, None)).or_insert(Some(sig));
                }
                stack.extend(embeds.get(&i).into_iter().flatten().copied());
            }
            if !known {
                stats.open += 1;
                continue;
            }
            if set.is_empty() {
                continue;
            }
            stats.interfaces += 1;
            let Some(candidates) = set
                .keys()
                .map(|(name, _)| by_method.get(name).map_or(&[][..], Vec::as_slice))
                .min_by_key(|c| c.len())
            else {
                continue;
            };
            // The reachability gate's interface side, and whether it applies
            // to every pair of this interface (a one-method set).
            let iface_side = packages.side(g, iface);
            let one_method = set.len() == 1;
            for &ty in candidates {
                let Some(methods) = g.symbols.class_methods.get(&ty) else {
                    continue;
                };
                let covers = set.keys().all(|&(name, pkg)| {
                    methods.contains_key(name) && pkg.is_none_or(|p| pkg_of(&ty) == Some(p))
                });
                if !covers {
                    continue;
                }
                // R1: signatures, where both sides recorded one.
                let mut checked = true;
                let mut differs = false;
                for (&(name, _), &sig_i) in &set {
                    let sig_t = methods.get(name).and_then(|m| nav.method_sigs.get(m));
                    match (sig_i, sig_t) {
                        (Some(a), Some(b)) if a != b.as_str() => {
                            differs = true;
                            break;
                        }
                        (Some(_), Some(_)) => {}
                        _ => checked = false,
                    }
                }
                if differs {
                    stats.signature += 1;
                    continue;
                }
                // R2: an import path, for a one-method set or a test side.
                let mut root = false;
                if let (Some(t), Some(i)) = (packages.side(g, ty), iface_side)
                    && (one_method || t.test || i.test)
                {
                    match dirs.scope(t, i) {
                        GoReach::Reached => {}
                        GoReach::RootAssumed => root = true,
                        GoReach::Unreached => {
                            if t.test || i.test {
                                stats.test_side += 1;
                            } else {
                                stats.one_method += 1;
                            }
                            continue;
                        }
                    }
                }
                pairs.push((ty, iface, checked, root));
            }
        }
    }
    pairs.sort_unstable_by_key(|(a, b, _, _)| (a.0, b.0));
    pairs.dedup_by_key(|(a, b, _, _)| (*a, *b));
    let existing: HashSet<(NodeId, NodeId)> = g
        .edges
        .iter()
        .filter(|e| e.category == edge_category::IMPLEMENTS)
        .map(|e| (e.from, e.to))
        .collect();
    pairs.retain(|(a, b, _, _)| !existing.contains(&(*a, *b)));
    stats.types = pairs.iter().map(|&(ty, ..)| ty).collect::<HashSet<_>>().len();
    stats.edges = pairs.len();
    stats.signature_checked = pairs.iter().filter(|p| p.2).count();
    stats.root_assumed = pairs.iter().filter(|p| p.3).count();
    // LC.3d: `graph:iface` rule `method_signature` when every signature was
    // compared (CA.3b), else `method_set` (a match on names).
    let signature_ev = graph_evidence("graph:iface", "method_signature").to_cell();
    let names_ev = graph_evidence("graph:iface", "method_set").to_cell();
    for (from, to, checked, _) in pairs {
        let edge = Edge::new(from, to, edge_category::IMPLEMENTS, Confidence::Medium);
        let ev = if checked { &signature_ev } else { &names_ev };
        g.edges.push(edge.with_cell(ev.clone()));
    }
    Some(stats)
}

// ============================================================================
// C/C++ header / implementation join (LB.10c)
// ============================================================================

/// What [`bind_out_of_line`] did to one C/C++ graph.
#[derive(Debug, Default, PartialEq, Eq)]
struct OutOfLineStats {
    /// Members joined to a header CLASS / STRUCT: one METHOD under it.
    bound: usize,
    /// Of `bound`: found only by the tail of a type's C++ name (a definition
    /// after `using namespace shop;`).
    by_name: usize,
    /// Of `bound`: the parser's provisional qname was not the class's member
    /// qname and moved (a global class in another directory, a class in an
    /// outer namespace, `using namespace`).
    renamed: usize,
    /// Qualified definitions whose qualifier names a namespace: FUNCTIONs.
    namespace_fns: usize,
    /// Qualifiers naming two or more classes the definition's directory does
    /// not tell apart: left FUNCTIONs.
    ambiguous: usize,
    /// Qualifiers naming no class and no namespace of the graph (a nested
    /// class, a class outside the tree): left FUNCTIONs.
    unbound: usize,
}

impl OutOfLineStats {
    /// LB.10c fired_on, once per C/C++ graph with an out-of-line member
    /// defined in another file than its class:
    /// `[cpp-members] out-of-line members bound: B (by_name=K renamed=R
    /// namespace_fns=N ambiguous=A unbound=U)`.
    fn marker(&self) -> Option<String> {
        (self.bound + self.namespace_fns + self.ambiguous + self.unbound > 0).then(|| {
            format!(
                "[cpp-members] out-of-line members bound: {} (by_name={} renamed={} \
                 namespace_fns={} ambiguous={} unbound={})",
                self.bound,
                self.by_name,
                self.renamed,
                self.namespace_fns,
                self.ambiguous,
                self.unbound
            )
        })
    }

    fn report(&self) {
        if let Some(line) = self.marker() {
            eprintln!("{line}");
        }
    }
}

/// A class / struct an out-of-line definition can join: a type a header
/// declares, never a source file's own (translation-unit local) type.
struct TypeRef {
    id: NodeId,
    qname: String,
    /// Its MODULE's directory: the MODULE qname minus the file segment.
    dir: String,
}

/// What C++ finds for a definition's qualifier ([`CppTypes::lookup`]).
enum Qualifier<'t> {
    /// A class / struct; `true` when found only by the tail of its C++ name.
    Type(&'t TypeRef, bool),
    Namespace,
    Ambiguous,
    Unbound,
}

/// The join's lookup tables, built once for a graph with a candidate.
/// BTree-ordered, so a scan never depends on a hasher seed.
struct CppTypes {
    /// C++ name -> the header CLASS / STRUCT nodes of that name, in
    /// `g.nodes` order.
    types: BTreeMap<String, Vec<TypeRef>>,
    /// C++ names of the namespace blocks (PACKAGE nodes).
    namespaces: BTreeSet<String>,
}

impl CppTypes {
    fn build(g: &RepoGraph) -> Self {
        let nav = &g.nav;
        let mut types: BTreeMap<String, Vec<TypeRef>> = BTreeMap::new();
        let mut namespaces: BTreeSet<String> = BTreeSet::new();
        for n in &g.nodes {
            let Some(&kind) = nav.kind_by_id.get(&n.id) else {
                continue;
            };
            if kind == node_kind::PACKAGE {
                namespaces.insert(cpp_name(nav, n.id));
                continue;
            }
            if kind != node_kind::CLASS && kind != node_kind::STRUCT {
                continue;
            }
            let Some(qname) = nav.qname_by_id.get(&n.id) else {
                continue;
            };
            let Some(module_qname) =
                enclosing_module(nav, n.id).and_then(|m| nav.qname_by_id.get(&m))
            else {
                continue;
            };
            // LB.10b's source-file shape `<file module>::..`: its members are
            // bound in its own file, and no other file can define them.
            if qname
                .strip_prefix(module_qname.as_str())
                .is_some_and(|r| r.starts_with("::"))
            {
                continue;
            }
            types.entry(cpp_name(nav, n.id)).or_default().push(TypeRef {
                id: n.id,
                qname: qname.clone(),
                // A MODULE qname is `<dir>::<file name>`, like a Go file's.
                dir: go_package_dir(module_qname).to_string(),
            });
        }
        CppTypes { types, namespaces }
    }

    /// C++'s lookup of qualifier `q` for a definition in namespace `ns` of a
    /// file in directory `dir`: from the definition's namespace outward, the
    /// first scope where `<scope>::q` names a type or a namespace decides.
    /// Several types of one name (global classes of two directories) bind
    /// the one in the definition's directory, else none. When no scope
    /// answers, a unique type whose C++ name ends in `::q` binds (a
    /// definition after `using namespace`).
    fn lookup(&self, ns: &str, q: &str, dir: &str) -> Qualifier<'_> {
        for scope in cpp_ns_prefixes(ns) {
            let full = cpp_join(scope, q);
            if let Some(found) = self.types.get(&full) {
                let pick = match found.as_slice() {
                    [one] => Some(one),
                    many => {
                        let mut here = many.iter().filter(|t| t.dir == dir);
                        match (here.next(), here.next()) {
                            (Some(one), None) => Some(one),
                            _ => None,
                        }
                    }
                };
                return pick.map_or(Qualifier::Ambiguous, |t| Qualifier::Type(t, false));
            }
            if self.namespaces.contains(&full) {
                return Qualifier::Namespace;
            }
        }
        let tail = format!("::{q}");
        let mut hits = self
            .types
            .iter()
            .filter(|(name, _)| name.ends_with(&tail))
            .flat_map(|(_, found)| found);
        match (hits.next(), hits.next()) {
            (Some(one), None) => Qualifier::Type(one, true),
            _ => Qualifier::Unbound,
        }
    }
}

/// C++ name of a type or namespace: the nav names of its CLASS / STRUCT /
/// PACKAGE ancestors up to the MODULE, then its own (a PACKAGE name may
/// itself be `a::b`). `shop::Cart`, `Widget`, `Outer::Inner`. Bounded by the
/// nav's size, so a malformed parent cycle ends.
fn cpp_name(nav: &CodeNav, id: NodeId) -> String {
    let mut segs: Vec<&str> = Vec::new();
    let mut cur = Some(id);
    for _ in 0..=nav.parent_of.len() {
        let Some(at) = cur else { break };
        let scoped = nav.kind_by_id.get(&at).is_some_and(|k| {
            *k == node_kind::CLASS || *k == node_kind::STRUCT || *k == node_kind::PACKAGE
        });
        if !scoped {
            break;
        }
        if let Some(name) = nav.name_by_id.get(&at) {
            segs.push(name);
        }
        cur = nav.parent_of.get(&at).copied();
    }
    segs.reverse();
    segs.join("::")
}

/// `a::b`, or `b` alone when `a` is empty.
fn cpp_join(a: &str, b: &str) -> String {
    if a.is_empty() {
        b.to_string()
    } else {
        format!("{a}::{b}")
    }
}

/// The scopes C++ searches for a qualified definition's qualifier, innermost
/// first: `a::b` -> `a::b`, `a`, "" (the global namespace).
fn cpp_ns_prefixes(ns: &str) -> Vec<&str> {
    let mut out = vec![ns];
    let mut cur = ns;
    while let Some((outer, _)) = cur.rsplit_once("::") {
        out.push(outer);
        cur = outer;
    }
    if !ns.is_empty() {
        out.push("");
    }
    out
}

/// Where one out-of-line member ends up ([`bind_out_of_line`]).
struct Placement {
    /// The member as merged.
    x: NodeId,
    /// Its defining file's lexical scope (the MODULE or namespace PACKAGE).
    scope: NodeId,
    /// Every MODULE / PACKAGE whose nav children list it (two files that
    /// define one member merge into one node).
    lexical: Vec<NodeId>,
    kind: NodeKindId,
    qname: String,
    name: String,
    /// Nav parent after the join.
    parent: NodeId,
    /// The class it joined, if any.
    class: Option<NodeId>,
}

/// True for a node kind that owns C++ members.
fn is_cpp_type(nav: &CodeNav, id: NodeId) -> bool {
    nav.kind_by_id
        .get(&id)
        .is_some_and(|k| *k == node_kind::CLASS || *k == node_kind::STRUCT)
}

/// LB.10c: join each out-of-line C++ member to the class its header declares.
///
/// Candidates are the METHODs a MODULE / PACKAGE lists as a nav child: LB.10b
/// emits an out-of-line member whose class its file does not define as a
/// provisional METHOD named `Q::m` under its lexical scope, and when the
/// member's qname equals an inline METHOD of the header class the two parses
/// merged into one node, listed by both the CLASS and the file (`Folded`,
/// whichever parse's nav record won). A graph without a METHOD (C) returns
/// at once.
///
/// A folded member already sits at its class's qname and joins that class.
/// Any other is looked up from its lexical namespace outward
/// ([`CppTypes::lookup`]): a class binds (METHOD `<class qname>::m`, renamed
/// when the parser's guess differs), a namespace makes it FUNCTION
/// `<lexical scope>::Q::m` named `m`, and an ambiguous or unknown qualifier
/// leaves FUNCTION `<lexical scope>::Q::m` named `Q::m` (never a METHOD
/// without a class). A joined member moves under its class in the nav
/// (leaving every file's children list, so no file's `module_symbols` lists
/// it), gets a CLASS -> METHOD DEFINES edge and keeps its file's DEFINES (the
/// file still defines it, LA.23d's rule). The returned map is member ->
/// defining scope, for [`CppCallScope`].
///
/// Runs before [`build_symbol_table`]. Deterministic: candidates follow
/// `g.nodes` order, the lookup tables are BTree-ordered, and every HashMap
/// here is lookup-only.
fn bind_out_of_line(
    g: &mut RepoGraph,
    calls: &mut [CallSite],
    refs: &mut [UnresolvedRef],
) -> (HashMap<NodeId, NodeId>, OutOfLineStats) {
    let mut stats = OutOfLineStats::default();
    let mut defining: HashMap<NodeId, NodeId> = HashMap::new();
    if !g.nav.kind_by_id.values().any(|k| *k == node_kind::METHOD) {
        return (defining, stats);
    }
    let placements = place_out_of_line(g, &mut stats);
    if placements.is_empty() {
        return (defining, stats);
    }
    let renames: Vec<(NodeId, NodeId)> = placements
        .iter()
        .map(|p| {
            (
                p.x,
                NodeId::from_parts(GRAPH_TYPE, g.repo, p.kind, &p.qname),
            )
        })
        .filter(|(old, new)| old != new)
        .collect();
    let absorbed = rename_nodes(g, calls, refs, &renames);
    let remap: HashMap<NodeId, NodeId> = renames.iter().copied().collect();
    let mut defines: HashSet<(NodeId, NodeId)> = g
        .edges
        .iter()
        .filter(|e| e.category == edge_category::DEFINES)
        .map(|e| (e.from, e.to))
        .collect();
    for p in &placements {
        let id = remap.get(&p.x).copied().unwrap_or(p.x);
        // A record merged into an existing node keeps that node's nav.
        if !absorbed.contains(&p.x) {
            g.nav.name_by_id.insert(id, p.name.clone());
            g.nav.qname_by_id.insert(id, p.qname.clone());
            g.nav.kind_by_id.insert(id, p.kind);
            g.nav.parent_of.insert(id, p.parent);
        }
        let Some(class) = p.class else { continue };
        for scope in &p.lexical {
            if let Some(kids) = g.nav.children_of.get_mut(scope) {
                kids.retain(|k| *k != id);
                if kids.is_empty() {
                    g.nav.children_of.remove(scope);
                }
            }
        }
        let kids = g.nav.children_of.entry(class).or_default();
        if !kids.contains(&id) {
            kids.push(id);
        }
        if defines.insert((class, id)) {
            let ev = graph_evidence("graph:cpp_members", "out_of_line");
            push_edge(g, class, id, edge_category::DEFINES, ev);
        }
        defining.entry(id).or_insert(p.scope);
    }
    (defining, stats)
}

/// [`bind_out_of_line`]'s decisions, in `g.nodes` order, counted in `stats`.
fn place_out_of_line(g: &RepoGraph, stats: &mut OutOfLineStats) -> Vec<Placement> {
    let nav = &g.nav;
    // METHOD -> the file scopes (MODULE / PACKAGE) and the types listing it
    // as a nav child, each sorted by qname then id.
    let mut lexical: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    let mut typed: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    for (parent, kids) in &nav.children_of {
        let Some(&kind) = nav.kind_by_id.get(parent) else {
            continue;
        };
        let slot = if kind == node_kind::MODULE || kind == node_kind::PACKAGE {
            &mut lexical
        } else if kind == node_kind::CLASS || kind == node_kind::STRUCT {
            &mut typed
        } else {
            continue;
        };
        for kid in kids {
            if nav.kind_by_id.get(kid) == Some(&node_kind::METHOD) {
                let list = slot.entry(*kid).or_default();
                if !list.contains(parent) {
                    list.push(*parent);
                }
            }
        }
    }
    let qname_of = |id: &NodeId| nav.qname_by_id.get(id).map_or("", String::as_str);
    for list in lexical.values_mut().chain(typed.values_mut()) {
        list.sort_by(|a, b| qname_of(a).cmp(qname_of(b)).then(a.0.cmp(&b.0)));
    }

    let mut index: Option<CppTypes> = None;
    let mut out: Vec<Placement> = Vec::new();
    for n in &g.nodes {
        let x = n.id;
        let (Some(lex), Some(name), Some(qname)) = (
            lexical.get(&x),
            nav.name_by_id.get(&x),
            nav.qname_by_id.get(&x),
        ) else {
            continue;
        };
        let parent = nav.parent_of.get(&x).copied();
        let Some(scope) = parent
            .filter(|p| lex.contains(p))
            .or_else(|| lex.first().copied())
        else {
            continue;
        };
        let (q, m) = name.rsplit_once("::").unwrap_or(("", name.as_str()));
        let folded = parent
            .filter(|p| is_cpp_type(nav, *p))
            .or_else(|| typed.get(&x).and_then(|t| t.first().copied()));
        let scope_qname = qname_of(&scope);
        let function = |named: String| Placement {
            x,
            scope,
            lexical: lex.clone(),
            kind: node_kind::FUNCTION,
            qname: format!("{scope_qname}::{q}::{m}"),
            name: named,
            parent: scope,
            class: None,
        };
        let joined = |class: NodeId, qname: String| Placement {
            x,
            scope,
            lexical: lex.clone(),
            kind: node_kind::METHOD,
            qname,
            name: m.to_string(),
            parent: class,
            class: Some(class),
        };
        if let Some(class) = folded {
            stats.bound += 1;
            out.push(joined(class, qname.clone()));
            continue;
        }
        if q.is_empty() {
            // A plain-named METHOD under a file scope is not LB.10b's shape.
            continue;
        }
        let types = index.get_or_insert_with(|| CppTypes::build(g));
        let ns = if nav.kind_by_id.get(&scope) == Some(&node_kind::PACKAGE) {
            cpp_name(nav, scope)
        } else {
            String::new()
        };
        let dir = enclosing_module(nav, scope)
            .map(|module| go_package_dir(qname_of(&module)))
            .unwrap_or("");
        out.push(match types.lookup(&ns, q, dir) {
            Qualifier::Type(t, by_name) => {
                let member = format!("{}::{m}", t.qname);
                stats.bound += 1;
                stats.by_name += usize::from(by_name);
                stats.renamed += usize::from(member != *qname);
                joined(t.id, member)
            }
            Qualifier::Namespace => {
                stats.namespace_fns += 1;
                function(m.to_string())
            }
            Qualifier::Ambiguous => {
                stats.ambiguous += 1;
                function(name.clone())
            }
            Qualifier::Unbound => {
                stats.unbound += 1;
                function(name.clone())
            }
        });
    }
    out
}

/// Move nodes to new ids in one batched pass. `renames` holds `(old, new)`
/// pairs in candidate order; a node's id is `NodeId::from_parts(kind,
/// qname)`, so a kind or qname change moves every place the id lives: the
/// node record, every edge's `from` / `to`, the pending `calls` and `refs`
/// (`from`, `from_module`), the unresolved lists, `properties`, the symbol
/// table's `home_module` keys and values (CB.15), and the nav
/// (name / qname / kind / parent records, `parent_of` values, `children_of`
/// keys and entries, deduped keeping the first occurrence, the field /
/// local type tables, the per-scope `nav_facts`, deduped likewise, and the
/// callable's return type, the new id's own kept when both have one).
///
/// A new id that already exists absorbs the renamed record: its cells are
/// appended to the existing node's (merge_parses' duplicate rule) and the
/// existing nav record stays. Returns the old ids so absorbed. A rewritten
/// edge equal to another edge, cells included (the file's DEFINES of a
/// member two parses both emitted), is dropped: two sites of one key stay
/// two edges. Every other edge keeps its place and cells.
pub(crate) fn rename_nodes(
    g: &mut RepoGraph,
    calls: &mut [CallSite],
    refs: &mut [UnresolvedRef],
    renames: &[(NodeId, NodeId)],
) -> HashSet<NodeId> {
    let remap: HashMap<NodeId, NodeId> = renames
        .iter()
        .copied()
        .filter(|(old, new)| old != new)
        .collect();
    let mut absorbed: HashSet<NodeId> = HashSet::new();
    if remap.is_empty() {
        return absorbed;
    }
    let map = |id: NodeId| remap.get(&id).copied().unwrap_or(id);

    // Node records.
    let mut at: HashMap<NodeId, usize> = g
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| !remap.contains_key(&n.id))
        .map(|(i, n)| (n.id, i))
        .collect();
    let mut dropped = vec![false; g.nodes.len()];
    for i in 0..g.nodes.len() {
        let old = g.nodes[i].id;
        let Some(&new) = remap.get(&old) else {
            continue;
        };
        match at.get(&new) {
            Some(&j) => {
                let cells = std::mem::take(&mut g.nodes[i].cells);
                append_cells(&mut g.nodes[j].cells, cells);
                absorbed.insert(old);
                dropped[i] = true;
            }
            None => {
                g.nodes[i].id = new;
                at.insert(new, i);
            }
        }
    }
    let mut flags = dropped.into_iter();
    g.nodes.retain(|_| !flags.next().unwrap_or(false));

    // Edges.
    let mut rewritten = vec![false; g.edges.len()];
    for (e, r) in g.edges.iter_mut().zip(rewritten.iter_mut()) {
        let (from, to) = (map(e.from), map(e.to));
        *r = from != e.from || to != e.to;
        e.from = from;
        e.to = to;
    }
    if rewritten.contains(&true) {
        let keys: HashSet<(NodeId, NodeId, EdgeCategoryId)> = g
            .edges
            .iter()
            .zip(&rewritten)
            .filter(|(_, r)| **r)
            .map(|(e, _)| e.key())
            .collect();
        let mut seen: HashSet<Edge> = g
            .edges
            .iter()
            .zip(&rewritten)
            .filter(|(e, r)| !**r && keys.contains(&e.key()))
            .map(|(e, _)| e.clone())
            .collect();
        let keep: Vec<bool> = g
            .edges
            .iter()
            .zip(&rewritten)
            .map(|(e, &r)| !r || seen.insert(e.clone()))
            .collect();
        let mut flags = keep.into_iter();
        g.edges.retain(|_| flags.next().unwrap_or(true));
    }

    // Pending and unresolved sites, properties.
    for c in calls.iter_mut().chain(g.unresolved_calls.iter_mut()) {
        c.from = map(c.from);
    }
    for r in refs.iter_mut().chain(g.unresolved_refs.iter_mut()) {
        r.from = map(r.from);
        r.from_module = map(r.from_module);
    }
    if g.properties.iter().any(|p| remap.contains_key(p)) {
        g.properties = std::mem::take(&mut g.properties)
            .into_iter()
            .map(map)
            .collect();
    }
    // CB.15's home modules: a renamed node keeps its file, the new id's own
    // entry kept when both have one.
    if !g.symbols.home_module.is_empty() {
        for &(old, new) in renames {
            if old == new {
                continue;
            }
            if let Some(m) = g.symbols.home_module.remove(&old) {
                g.symbols.home_module.entry(new).or_insert(m);
            }
        }
        for m in g.symbols.home_module.values_mut() {
            *m = map(*m);
        }
    }

    // Nav.
    let nav = &mut g.nav;
    for &(old, new) in renames {
        if old == new {
            continue;
        }
        let name = nav.name_by_id.remove(&old);
        let qname = nav.qname_by_id.remove(&old);
        let kind = nav.kind_by_id.remove(&old);
        let parent = nav.parent_of.remove(&old);
        if !absorbed.contains(&old) {
            if let Some(v) = name {
                nav.name_by_id.insert(new, v);
            }
            if let Some(v) = qname {
                nav.qname_by_id.insert(new, v);
            }
            if let Some(v) = kind {
                nav.kind_by_id.insert(new, v);
            }
            if let Some(v) = parent {
                nav.parent_of.insert(new, v);
            }
        }
        if let Some(kids) = nav.children_of.remove(&old) {
            let list = nav.children_of.entry(new).or_default();
            for kid in kids {
                if !list.contains(&kid) {
                    list.push(kid);
                }
            }
        }
        if let Some(fields) = nav.field_types.remove(&old) {
            nav.field_types.entry(new).or_default().extend(fields);
        }
        if let Some(locals) = nav.local_types.remove(&old) {
            nav.local_types.entry(new).or_default().extend(locals);
        }
        if let Some(facts) = nav.nav_facts.remove(&old) {
            append_facts(nav.nav_facts.entry(new).or_default(), facts);
        }
        if let Some(ty) = nav.return_types.remove(&old) {
            nav.return_types.entry(new).or_insert(ty);
        }
        if let Some(sig) = nav.method_sigs.remove(&old) {
            nav.method_sigs.entry(new).or_insert(sig);
        }
    }
    for parent in nav.parent_of.values_mut() {
        *parent = map(*parent);
    }
    for kids in nav.children_of.values_mut() {
        if kids.iter().any(|k| remap.contains_key(k)) {
            let mut seen: HashSet<NodeId> = HashSet::new();
            *kids = kids
                .iter()
                .map(|k| map(*k))
                .filter(|k| seen.insert(*k))
                .collect();
        }
    }
    absorbed
}

/// LB.10c: a bound out-of-line member's enclosing MODULE is its class's
/// header, but its body still sees its own file: a Bare name the generic
/// chain missed is looked up in the defining file's lexical scope (its
/// namespace PACKAGE, then that file's MODULE). File-scoped PACKAGEs have
/// their own file's MODULE as parent, so the walk never leaves the defining
/// file.
fn defining_scope_call(
    g: &RepoGraph,
    defining: &HashMap<NodeId, NodeId>,
    site: &CallSite,
) -> Option<NodeId> {
    let CallQualifier::Bare(name) = &site.qualifier else {
        return None;
    };
    let mut scope = *defining.get(&site.from)?;
    for _ in 0..=g.nav.parent_of.len() {
        if let Some(hit) = g
            .symbols
            .module_symbols
            .get(&scope)
            .and_then(|s| s.get(name))
        {
            return Some(*hit);
        }
        if g.nav.kind_by_id.get(&scope) == Some(&node_kind::MODULE) {
            return None;
        }
        scope = *g.nav.parent_of.get(&scope)?;
    }
    None
}

/// `resolve_calls`' `extra_hook` for C/C++, consulted only after every
/// generic lookup missed, for a Bare call:
///
/// 1. from a member [`bind_out_of_line`] joined to its header class: the
///    defining file's lexical scope ([`defining_scope_call`]), evidence
///    `graph:cpp_members` rule `defining_scope`;
/// 2. the one top-level symbol of that name among the MODULEs the calling
///    file directly `#include`s: the IMPORTS edges its quoted includes bound
///    (a bound member's calling file is its defining file, not the header).
///    A quoted include is textual inclusion, so the header's top-level names
///    are visible; a name two included files define stays unresolved rather
///    than guess, and an include of an include is not followed. Evidence
///    `graph:c_includes` rule `include` (LB.10a's engine-side stopgap, moved
///    here).
struct CppCallScope {
    /// Joined member -> its defining file's lexical scope.
    defining: HashMap<NodeId, NodeId>,
    /// Including MODULE -> the MODULEs it includes, in edge order.
    includes: HashMap<NodeId, Vec<NodeId>>,
    /// Bare calls bound through an include.
    included: std::cell::Cell<usize>,
    /// Bare calls two or more included MODULEs could answer.
    ambiguous: std::cell::Cell<usize>,
}

impl CppCallScope {
    /// Index `g`'s include edges; after `resolve_imports_ts`.
    fn new(g: &RepoGraph, defining: HashMap<NodeId, NodeId>) -> Self {
        let mut includes: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for e in g
            .edges
            .iter()
            .filter(|e| e.category == edge_category::IMPORTS)
        {
            let to = includes.entry(e.from).or_default();
            if !to.contains(&e.to) {
                to.push(e.to);
            }
        }
        CppCallScope {
            defining,
            includes,
            included: std::cell::Cell::new(0),
            ambiguous: std::cell::Cell::new(0),
        }
    }

    fn resolve(&self, g: &RepoGraph, site: &CallSite) -> Option<(NodeId, Evidence)> {
        let CallQualifier::Bare(name) = &site.qualifier else {
            return None;
        };
        if let Some(to) = defining_scope_call(g, &self.defining, site) {
            return Some((to, graph_evidence("graph:cpp_members", "defining_scope")));
        }
        let file = match self.defining.get(&site.from) {
            Some(&scope) => enclosing_module(&g.nav, scope),
            None => enclosing_module(&g.nav, site.from),
        }?;
        let mut hits: Vec<NodeId> = self
            .includes
            .get(&file)
            .into_iter()
            .flatten()
            .filter_map(|h| g.symbols.module_symbols.get(h)?.get(name).copied())
            .collect();
        hits.sort_unstable_by_key(|id| id.0);
        hits.dedup();
        match hits[..] {
            [to] => {
                self.included.set(self.included.get() + 1);
                Some((to, graph_evidence("graph:c_includes", "include")))
            }
            [] => None,
            _ => {
                self.ambiguous.set(self.ambiguous.get() + 1);
                None
            }
        }
    }

    /// LB.10a fired_on, once per C/C++ graph that bound or refused an
    /// include call: `[c-includes] bare calls bound through a direct
    /// #include: N (ambiguous=A)`.
    fn marker(&self) -> Option<String> {
        let (bound, ambiguous) = (self.included.get(), self.ambiguous.get());
        (bound + ambiguous > 0).then(|| {
            format!(
                "[c-includes] bare calls bound through a direct #include: {bound} \
                 (ambiguous={ambiguous})"
            )
        })
    }

    fn report(&self) {
        if let Some(line) = self.marker() {
            eprintln!("{line}");
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::repo;
    use glia_code_domain::{GRAPH_TYPE, ImportTarget, edge_category};
    use glia_core::{Confidence, Node};

    #[test]
    fn empty_repo_builds_cleanly() {
        let g = build_python(repo(), vec![]).unwrap();
        assert!(g.nodes.is_empty());
        assert!(g.edges.is_empty());
    }

    #[test]
    fn build_dotted_resolves_java_style_imports() {
        // Two modules: com::foo (imports com::bar::Helper) and com::bar.
        let repo = repo();
        let foo_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MODULE, "com::foo");
        let bar_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MODULE, "com::bar");
        let helper_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::CLASS, "com::bar::Helper");

        let mut foo_nav = CodeNav::default();
        foo_nav.record(foo_id, "foo", "com::foo", node_kind::MODULE, None);
        let foo = FileParse {
            nodes: vec![Node { id: foo_id, repo, confidence: Confidence::Strong, cells: vec![] }],
            edges: vec![],
            imports: vec![ImportStmt {
                from_module: "com::foo".to_string(),
                target: ImportTarget::Symbol {
                    module: "com::bar".to_string(),
                    name: "Helper".to_string(),
                    alias: None,
                    level: 0,
                },
                line: 0,
            }],
            calls: vec![],
            refs: vec![],
            nav: foo_nav,
            properties: HashSet::new(),
        };

        let mut bar_nav = CodeNav::default();
        bar_nav.record(bar_id, "bar", "com::bar", node_kind::MODULE, None);
        bar_nav.record(helper_id, "Helper", "com::bar::Helper", node_kind::CLASS, Some(bar_id));
        let bar = FileParse {
            nodes: vec![
                Node { id: bar_id, repo, confidence: Confidence::Strong, cells: vec![] },
                Node { id: helper_id, repo, confidence: Confidence::Strong, cells: vec![] },
            ],
            edges: vec![],
            imports: vec![],
            calls: vec![],
            refs: vec![],
            nav: bar_nav,
            properties: HashSet::new(),
        };

        let g = build_dotted(repo, vec![foo, bar]).unwrap();
        assert!(
            g.edges.iter().any(|e|
                e.from == foo_id && e.to == bar_id && e.category == edge_category::IMPORTS
            ),
            "expected IMPORTS edge from com::foo to com::bar"
        );
        let foo_bindings = g.symbols.module_import_bindings.get(&foo_id).unwrap();
        assert_eq!(foo_bindings.get("Helper").copied(), Some(helper_id));
    }

    #[test]
    fn build_ruby_resolves_slash_requires() {
        let repo = repo();
        let app_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MODULE, "app");
        let foo_bar_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MODULE, "foo::bar");

        let mut app_nav = CodeNav::default();
        app_nav.record(app_id, "app", "app", node_kind::MODULE, None);
        let app = FileParse {
            nodes: vec![Node { id: app_id, repo, confidence: Confidence::Strong, cells: vec![] }],
            edges: vec![],
            imports: vec![ImportStmt {
                from_module: "app".to_string(),
                target: ImportTarget::Module {
                    path: "foo/bar".to_string(),
                    alias: None,
                },
                line: 0,
            }],
            calls: vec![],
            refs: vec![],
            nav: app_nav,
            properties: HashSet::new(),
        };

        let mut foo_bar_nav = CodeNav::default();
        foo_bar_nav.record(foo_bar_id, "bar", "foo::bar", node_kind::MODULE, None);
        let foo_bar = FileParse {
            nodes: vec![Node { id: foo_bar_id, repo, confidence: Confidence::Strong, cells: vec![] }],
            edges: vec![],
            imports: vec![],
            calls: vec![],
            refs: vec![],
            nav: foo_bar_nav,
            properties: HashSet::new(),
        };

        let g = build_ruby(repo, vec![app, foo_bar]).unwrap();
        assert!(
            g.edges.iter().any(|e|
                e.from == app_id && e.to == foo_bar_id && e.category == edge_category::IMPORTS
            ),
            "expected IMPORTS edge from app to foo::bar (slash → ::)"
        );
    }

    // ---- A6.6: interface method table + method-level IMPLEMENTS -------------

    /// C# shape: `interface IUserService { GetById(); }` +
    /// `class UserService : IUserService { GetById(); Load(); }`, the class-level
    /// heritage arriving as a Bare IMPLEMENTS ref (bound by resolve_refs).
    /// Returns (file, iface, iface_get, impl_cls, impl_get, impl_load).
    fn iface_and_impl() -> (FileParse, [NodeId; 5]) {
        let r = repo();
        let m = NodeId::from_parts(GRAPH_TYPE, r, node_kind::MODULE, "svc");
        let iface = NodeId::from_parts(GRAPH_TYPE, r, node_kind::INTERFACE, "svc::IUserService");
        let iface_get =
            NodeId::from_parts(GRAPH_TYPE, r, node_kind::METHOD, "svc::IUserService::GetById");
        let cls = NodeId::from_parts(GRAPH_TYPE, r, node_kind::CLASS, "svc::UserService");
        let cls_get =
            NodeId::from_parts(GRAPH_TYPE, r, node_kind::METHOD, "svc::UserService::GetById");
        let cls_load =
            NodeId::from_parts(GRAPH_TYPE, r, node_kind::METHOD, "svc::UserService::Load");
        let mut nav = CodeNav::default();
        nav.record(m, "svc", "svc", node_kind::MODULE, None);
        nav.record(iface, "IUserService", "svc::IUserService", node_kind::INTERFACE, Some(m));
        nav.record(iface_get, "GetById", "svc::IUserService::GetById", node_kind::METHOD, Some(iface));
        nav.record(cls, "UserService", "svc::UserService", node_kind::CLASS, Some(m));
        nav.record(cls_get, "GetById", "svc::UserService::GetById", node_kind::METHOD, Some(cls));
        nav.record(cls_load, "Load", "svc::UserService::Load", node_kind::METHOD, Some(cls));
        let node = |id| Node { id, repo: r, confidence: Confidence::Strong, cells: vec![] };
        let file = FileParse {
            nodes: [m, iface, iface_get, cls, cls_get, cls_load].into_iter().map(node).collect(),
            edges: vec![],
            imports: vec![],
            calls: vec![],
            refs: vec![UnresolvedRef {
                from: cls,
                from_module: m,
                qualifier: glia_code_domain::CallQualifier::Bare("IUserService".to_string()),
                category: edge_category::IMPLEMENTS,
                line: 0,
            }],
            nav,
            properties: HashSet::new(),
        };
        (file, [iface, iface_get, cls, cls_get, cls_load])
    }

    fn implements(g: &RepoGraph) -> Vec<(NodeId, NodeId)> {
        g.edges
            .iter()
            .filter(|e| e.category == edge_category::IMPLEMENTS)
            .map(|e| (e.from, e.to))
            .collect()
    }

    /// The INTERFACE's methods land in their own table and never in
    /// `class_methods`, which `unique_global_method` scans for HANDLED_BY.
    #[test]
    fn interface_methods_are_not_in_class_methods() {
        let (file, [iface, iface_get, cls, cls_get, _]) = iface_and_impl();
        let g = build_dotted(repo(), vec![file]).unwrap();
        assert!(!g.symbols.class_methods.contains_key(&iface));
        assert_eq!(g.symbols.interface_methods[&iface].get("GetById").copied(), Some(iface_get));
        assert_eq!(g.symbols.class_methods[&cls].get("GetById").copied(), Some(cls_get));
        assert!(!g.symbols.interface_methods.contains_key(&cls));
    }

    /// Class-level `UserService -> IUserService` pairs the same-named method:
    /// `UserService::GetById -> IUserService::GetById`. `Load` has no interface
    /// counterpart and pairs with nothing.
    #[test]
    fn method_level_implements_pairs_same_named_methods() {
        let (file, [iface, iface_get, cls, cls_get, _]) = iface_and_impl();
        let g = build_dotted(repo(), vec![file]).unwrap();
        assert_eq!(implements(&g), vec![(cls, iface), (cls_get, iface_get)]);
    }

    /// Every builder runs the pass, and a second run adds nothing: an
    /// IMPLEMENTS pair already present is never pushed twice.
    #[test]
    fn method_level_implements_is_idempotent_and_runs_in_every_builder() {
        for build in [build_python, build_go, build_dotted, build_ruby] {
            let (file, [_, iface_get, _, cls_get, _]) = iface_and_impl();
            let mut g = build(repo(), vec![file]).unwrap();
            assert!(implements(&g).contains(&(cls_get, iface_get)));
            let before = g.edges.len();
            crate::calls::emit_method_level_implements(&mut g);
            assert_eq!(g.edges.len(), before, "re-running the pass must not duplicate");
        }
        let (file, [_, iface_get, _, cls_get, _]) = iface_and_impl();
        let g = build_typescript(repo(), vec![file], |_, _| None).unwrap();
        assert!(implements(&g).contains(&(cls_get, iface_get)));
    }

    /// Two implementors of one interface, several methods: the emitted edge
    /// order is a pure function of the ids (sorted), not of HashMap seeds.
    #[test]
    fn method_level_implements_order_is_sorted() {
        let r = repo();
        let m = NodeId::from_parts(GRAPH_TYPE, r, node_kind::MODULE, "svc");
        let iface = NodeId::from_parts(GRAPH_TYPE, r, node_kind::INTERFACE, "svc::I");
        let mut nav = CodeNav::default();
        nav.record(m, "svc", "svc", node_kind::MODULE, None);
        nav.record(iface, "I", "svc::I", node_kind::INTERFACE, Some(m));
        let mut ids = vec![m, iface];
        let mut edges = vec![];
        for name in ["a", "b", "c", "d"] {
            let q = format!("svc::I::{name}");
            let id = NodeId::from_parts(GRAPH_TYPE, r, node_kind::METHOD, &q);
            nav.record(id, name, &q, node_kind::METHOD, Some(iface));
            ids.push(id);
        }
        for cls_name in ["X", "Y"] {
            let cq = format!("svc::{cls_name}");
            let cls = NodeId::from_parts(GRAPH_TYPE, r, node_kind::CLASS, &cq);
            nav.record(cls, cls_name, &cq, node_kind::CLASS, Some(m));
            ids.push(cls);
            edges.push(glia_core::Edge {
                from: cls,
                to: iface,
                category: edge_category::IMPLEMENTS,
                confidence: Confidence::Strong,
                cells: Vec::new(),
            });
            for name in ["a", "b", "c", "d"] {
                let q = format!("{cq}::{name}");
                let id = NodeId::from_parts(GRAPH_TYPE, r, node_kind::METHOD, &q);
                nav.record(id, name, &q, node_kind::METHOD, Some(cls));
                ids.push(id);
            }
        }
        let node = |id| Node { id, repo: r, confidence: Confidence::Strong, cells: vec![] };
        let file = FileParse {
            nodes: ids.into_iter().map(node).collect(),
            edges,
            imports: vec![],
            calls: vec![],
            refs: vec![],
            nav,
            properties: HashSet::new(),
        };
        let g = build_dotted(r, vec![file]).unwrap();
        let method_level: Vec<_> = implements(&g).into_iter().filter(|(_, to)| *to != iface).collect();
        assert_eq!(method_level.len(), 8, "2 implementors x 4 methods");
        let mut sorted = method_level.clone();
        sorted.sort_unstable_by_key(|(a, b)| (a.0, b.0));
        assert_eq!(method_level, sorted);
    }

    /// The HANDLED_BY guard: a route handler `h.GetById` is unique among
    /// CLASS / STRUCT methods even though the interface declares `GetById`
    /// too. Were interface methods merged into `class_methods`, the lookup
    /// would turn ambiguous and the HANDLED_BY edge would vanish.
    #[test]
    fn interface_method_does_not_make_handled_by_ambiguous() {
        let (mut file, [_, _, _, cls_get, _]) = iface_and_impl();
        let r = repo();
        let m = NodeId::from_parts(GRAPH_TYPE, r, node_kind::MODULE, "svc");
        let route = NodeId::from_parts(GRAPH_TYPE, r, node_kind::ROUTE, "svc::GET /users/{id}");
        file.nav.record(route, "GET /users/{id}", "svc::GET /users/{id}", node_kind::ROUTE, Some(m));
        file.nodes.push(Node { id: route, repo: r, confidence: Confidence::Strong, cells: vec![] });
        file.refs.push(UnresolvedRef {
            from: route,
            from_module: m,
            qualifier: glia_code_domain::CallQualifier::Attribute {
                base: "h".to_string(),
                name: "GetById".to_string(),
            },
            category: edge_category::HANDLED_BY,
            line: 0,
        });
        let g = build_dotted(r, vec![file]).unwrap();
        let handled: Vec<_> = g
            .edges
            .iter()
            .filter(|e| e.category == edge_category::HANDLED_BY)
            .map(|e| (e.from, e.to))
            .collect();
        assert_eq!(handled, vec![(route, cls_get)]);
    }

    // ---- LA.23d: Go split-file receivers ------------------------------------

    fn gid(kind: glia_core::NodeKindId, qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), kind, qname)
    }

    /// One Go file shaped the way the Go parser emits it: MODULE `module` plus
    /// `(kind, qname, parent qname)` items, where a `None` parent is the
    /// MODULE (a METHOD there is one whose receiver type this file does not
    /// declare) and `Some(q)` names an earlier item of the same file. Every
    /// item gets its parent -> item DEFINES edge.
    fn go_file(
        module: &str,
        items: &[(glia_core::NodeKindId, &str, Option<&str>)],
    ) -> FileParse {
        let r = repo();
        let m = gid(node_kind::MODULE, module);
        let mut nav = CodeNav::default();
        nav.record(m, module.rsplit("::").next().unwrap_or(module), module, node_kind::MODULE, None);
        let mut ids: HashMap<&str, NodeId> = HashMap::new();
        let mut nodes = vec![Node { id: m, repo: r, confidence: Confidence::Strong, cells: vec![] }];
        let mut edges = vec![];
        for &(kind, qname, parent) in items {
            let id = gid(kind, qname);
            let parent_id = parent.map_or(m, |p| ids[p]);
            let name = qname.rsplit("::").next().unwrap_or(qname);
            nav.record(id, name, qname, kind, Some(parent_id));
            nodes.push(Node { id, repo: r, confidence: Confidence::Strong, cells: vec![] });
            edges.push(glia_core::Edge {
                from: parent_id,
                to: id,
                category: edge_category::DEFINES,
                confidence: Confidence::Strong,
                cells: Vec::new(),
            });
            ids.insert(qname, id);
        }
        FileParse {
            nodes,
            edges,
            imports: vec![],
            calls: vec![],
            refs: vec![],
            nav,
            properties: HashSet::new(),
        }
    }

    fn has_edge(g: &RepoGraph, from: NodeId, to: NodeId, category: glia_core::EdgeCategoryId) -> bool {
        g.edges.iter().any(|e| e.from == from && e.to == to && e.category == category)
    }

    /// The fixtures/go-split-receiver shape: `types.go` declares `UserRepo`
    /// (with `Find`) and `UserService { repo *UserRepo }`; `service.go`
    /// declares `(s *UserService) Get` calling `s.audit(id)` and
    /// `s.repo.Find(id)`, and `audit`.
    fn fixture_shape() -> Vec<FileParse> {
        let mut types = go_file(
            "types",
            &[
                (node_kind::STRUCT, "types::UserRepo", None),
                (node_kind::METHOD, "types::UserRepo::Find", Some("types::UserRepo")),
                (node_kind::STRUCT, "types::UserService", None),
            ],
        );
        types.nav.record_field_type(gid(node_kind::STRUCT, "types::UserService"), "repo", "UserRepo");
        let mut service = go_file(
            "service",
            &[
                (node_kind::METHOD, "service::UserService::Get", None),
                (node_kind::METHOD, "service::UserService::audit", None),
            ],
        );
        let get = gid(node_kind::METHOD, "service::UserService::Get");
        service.calls = vec![
            CallSite { from: get, qualifier: CallQualifier::SelfMethod("audit".to_string()), line: 0 },
            CallSite {
                from: get,
                qualifier: CallQualifier::ComplexReceiver {
                    receiver: "self.repo".to_string(),
                    name: "Find".to_string(),
                },
                line: 0,
            },
        ];
        vec![types, service]
    }

    /// A method whose struct lives in another file of the package moves under
    /// the struct: nav parent / children, a struct -> method DEFINES next to
    /// the kept module -> method one, `class_methods` instead of
    /// `module_symbols`, and its self-call and field-typed call both bind.
    #[test]
    fn split_method_rebinds_under_its_struct_and_resolves_calls() {
        let (svc_struct, find) =
            (gid(node_kind::STRUCT, "types::UserService"), gid(node_kind::METHOD, "types::UserRepo::Find"));
        let (service, get, audit) = (
            gid(node_kind::MODULE, "service"),
            gid(node_kind::METHOD, "service::UserService::Get"),
            gid(node_kind::METHOD, "service::UserService::audit"),
        );

        let (mut g, _, _, _) = merge_parses(repo(), fixture_shape());
        let stats = bind_split_go_receivers(&mut g);
        assert_eq!(stats.bound, vec![(get, service, svc_struct), (audit, service, svc_struct)]);
        assert_eq!(
            stats.marker().as_deref(),
            Some("[go-recv] split-file methods bound: 2 (ambiguous=0 unmatched=0)")
        );

        let g = build_go(repo(), fixture_shape()).unwrap();
        assert_eq!(g.nav.parent_of[&get], svc_struct);
        assert_eq!(g.nav.parent_of[&audit], svc_struct);
        assert_eq!(g.nav.children_of[&svc_struct], vec![get, audit]);
        assert!(!g.nav.children_of[&service].contains(&get));
        for m in [get, audit] {
            assert!(has_edge(&g, svc_struct, m, edge_category::DEFINES), "struct -> method DEFINES");
            assert!(has_edge(&g, service, m, edge_category::DEFINES), "the file still defines it");
        }
        assert_eq!(g.symbols.class_methods[&svc_struct].get("Get").copied(), Some(get));
        assert!(g.symbols.module_symbols.get(&service).is_none_or(|s| !s.contains_key("Get")));
        assert!(has_edge(&g, get, audit, edge_category::CALLS), "SelfMethod across files");
        assert!(has_edge(&g, get, find, edge_category::CALLS), "field-typed call across files");
        assert!(g.unresolved_calls.is_empty());
    }

    /// Package identity is the directory: a receiver `T` binds the `T` of its
    /// own directory, never a same-named `T` of another package, and a
    /// directory with no `T` binds nothing.
    #[test]
    fn same_named_struct_in_another_package_is_not_bound() {
        let parses = vec![
            go_file("a::types", &[(node_kind::STRUCT, "a::types::T", None)]),
            go_file("b::types", &[(node_kind::STRUCT, "b::types::T", None)]),
            go_file("b::svc", &[(node_kind::METHOD, "b::svc::T::M", None)]),
            go_file("c::svc", &[(node_kind::METHOD, "c::svc::T::M", None)]),
        ];
        let (mut g, _, _, _) = merge_parses(repo(), parses);
        let stats = bind_split_go_receivers(&mut g);
        let (b_m, c_m) = (gid(node_kind::METHOD, "b::svc::T::M"), gid(node_kind::METHOD, "c::svc::T::M"));
        assert_eq!(stats.bound, vec![(b_m, gid(node_kind::MODULE, "b::svc"), gid(node_kind::STRUCT, "b::types::T"))]);
        assert_eq!((stats.ambiguous, stats.unmatched), (0, 1));
        assert_eq!(g.nav.parent_of[&c_m], gid(node_kind::MODULE, "c::svc"));
        assert!(g.nav.children_of.get(&gid(node_kind::STRUCT, "a::types::T")).is_none_or(|k| k.is_empty()));
    }

    /// Two files of one directory each declaring `type T struct` (a tree that
    /// does not compile) leave a third file's `T` method where it was.
    #[test]
    fn ambiguous_receiver_type_binds_nothing() {
        let parses = vec![
            go_file("x::one", &[(node_kind::STRUCT, "x::one::T", None)]),
            go_file("x::two", &[(node_kind::STRUCT, "x::two::T", None)]),
            go_file("x::three", &[(node_kind::METHOD, "x::three::T::M", None)]),
        ];
        let (mut g, _, _, _) = merge_parses(repo(), parses);
        let before = g.edges.len();
        let stats = bind_split_go_receivers(&mut g);
        assert!(stats.bound.is_empty());
        assert_eq!((stats.ambiguous, stats.unmatched), (1, 0));
        assert_eq!(
            stats.marker().as_deref(),
            Some("[go-recv] split-file methods bound: 0 (ambiguous=1 unmatched=0)")
        );
        assert_eq!(g.edges.len(), before);
        assert_eq!(g.nav.parent_of[&gid(node_kind::METHOD, "x::three::T::M")], gid(node_kind::MODULE, "x::three"));
    }

    /// A method declared beside its struct is already under it: not a
    /// candidate, no edge added, no marker.
    #[test]
    fn same_file_method_is_untouched() {
        let parses = vec![go_file(
            "s",
            &[(node_kind::STRUCT, "s::T", None), (node_kind::METHOD, "s::T::M", Some("s::T"))],
        )];
        let (mut g, _, _, _) = merge_parses(repo(), parses);
        let (edges, parent) = (g.edges.len(), g.nav.parent_of.clone());
        let stats = bind_split_go_receivers(&mut g);
        assert!(stats.bound.is_empty());
        assert_eq!(stats.marker(), None);
        assert_eq!(g.edges.len(), edges);
        assert_eq!(g.nav.parent_of, parent);
    }

    /// Go imports and package-level functions are per file, so a bound
    /// method's Bare / Attribute calls keep resolving in its OWN file
    /// (`handlers.go`), not the struct's (`server.go`, which imports nothing),
    /// while its self-call binds through the struct. A bare `Handle()` from a
    /// function of that file no longer lands on the method.
    #[test]
    fn split_method_bare_and_import_calls_keep_their_file_scope() {
        let mut handlers = go_file(
            "app::handlers",
            &[
                (node_kind::METHOD, "app::handlers::Server::Handle", None),
                (node_kind::METHOD, "app::handlers::Server::helper", None),
                (node_kind::FUNCTION, "app::handlers::writeJSON", None),
                (node_kind::FUNCTION, "app::handlers::run", None),
            ],
        );
        handlers.imports = vec![ImportStmt {
            from_module: "app::handlers".to_string(),
            target: ImportTarget::Module { path: "store".to_string(), alias: None },
            line: 0,
        }];
        let (handle, helper, write_json, run) = (
            gid(node_kind::METHOD, "app::handlers::Server::Handle"),
            gid(node_kind::METHOD, "app::handlers::Server::helper"),
            gid(node_kind::FUNCTION, "app::handlers::writeJSON"),
            gid(node_kind::FUNCTION, "app::handlers::run"),
        );
        let site = |from, qualifier| CallSite { from, qualifier, line: 0 };
        handlers.calls = vec![
            site(handle, CallQualifier::Bare("writeJSON".to_string())),
            site(
                handle,
                CallQualifier::Attribute { base: "store".to_string(), name: "Validate".to_string() },
            ),
            site(handle, CallQualifier::SelfMethod("helper".to_string())),
            site(run, CallQualifier::Bare("Handle".to_string())),
        ];
        let parses = vec![
            go_file("store::store", &[(node_kind::FUNCTION, "store::store::Validate", None)]),
            go_file("app::server", &[(node_kind::STRUCT, "app::server::Server", None)]),
            handlers,
        ];
        let g = build_go(repo(), parses).unwrap();
        let server = gid(node_kind::STRUCT, "app::server::Server");
        assert_eq!(g.nav.parent_of[&handle], server, "struct parent restored after the file pass");
        assert!(has_edge(&g, handle, write_json, edge_category::CALLS), "same-file bare call");
        assert!(
            has_edge(&g, handle, gid(node_kind::FUNCTION, "store::store::Validate"), edge_category::CALLS),
            "import of the method's own file"
        );
        assert!(has_edge(&g, handle, helper, edge_category::CALLS), "self-call through the struct");
        assert!(!g.edges.iter().any(|e| e.from == run && e.category == edge_category::CALLS));
        assert_eq!(g.unresolved_calls.len(), 1, "only the bare `Handle()` stays unresolved");
    }

    // ---- LD.7b: Go implicit interface satisfaction --------------------------

    /// An embed ref the Go parser emits: INHERITS_FROM out of `iface` (an
    /// INTERFACE of `module`).
    fn embed(module: &str, iface: &str, qualifier: CallQualifier) -> UnresolvedRef {
        UnresolvedRef {
            from: gid(node_kind::INTERFACE, iface),
            from_module: gid(node_kind::MODULE, module),
            qualifier,
            category: edge_category::INHERITS_FROM,
            line: 0,
        }
    }

    /// `build_go` with the implicit pass's stats.
    fn go_implicit(parses: Vec<FileParse>) -> (RepoGraph, Option<GoImplicitStats>) {
        let (g, _, stats, _, _, _) = build_go_passes(repo(), parses);
        (g, stats)
    }

    fn implements_edge(g: &RepoGraph, from: NodeId, to: NodeId) -> Option<Confidence> {
        g.edges
            .iter()
            .find(|e| e.from == from && e.to == to && e.category == edge_category::IMPLEMENTS)
            .map(|e| e.confidence)
    }

    /// The fixtures/go-implicit-iface shape: `store.go` declares `Store { Get;
    /// Put }`, `mem.go` declares `MemStore` with both methods and `ReadOnly`
    /// with `Get` only.
    fn implicit_iface_shape() -> Vec<FileParse> {
        vec![
            go_file(
                "store",
                &[
                    (node_kind::INTERFACE, "store::Store", None),
                    (node_kind::METHOD, "store::Store::Get", Some("store::Store")),
                    (node_kind::METHOD, "store::Store::Put", Some("store::Store")),
                    (node_kind::FUNCTION, "store::Use", None),
                ],
            ),
            go_file(
                "mem",
                &[
                    (node_kind::STRUCT, "mem::MemStore", None),
                    (node_kind::METHOD, "mem::MemStore::Get", Some("mem::MemStore")),
                    (node_kind::METHOD, "mem::MemStore::Put", Some("mem::MemStore")),
                    (node_kind::STRUCT, "mem::ReadOnly", None),
                    (node_kind::METHOD, "mem::ReadOnly::Get", Some("mem::ReadOnly")),
                ],
            ),
        ]
    }

    /// A type implements an interface only when it declares every one of its
    /// methods; the edge is Medium (names, not signatures), and A6.6 then
    /// pairs the methods of the new edge.
    #[test]
    fn go_implicit_implements_requires_full_method_set() {
        let (store, mem_store, read_only) = (
            gid(node_kind::INTERFACE, "store::Store"),
            gid(node_kind::STRUCT, "mem::MemStore"),
            gid(node_kind::STRUCT, "mem::ReadOnly"),
        );
        let (_, stats) = go_implicit(implicit_iface_shape());
        assert_eq!(
            stats.map(|s| s.marker()).as_deref(),
            Some("[iface] go implicit implements: 1 (interfaces=1 types=1 embedded=0 open=0)")
        );

        let g = build_go(repo(), implicit_iface_shape()).unwrap();
        assert_eq!(implements_edge(&g, mem_store, store), Some(Confidence::Medium));
        assert_eq!(implements_edge(&g, read_only, store), None, "ReadOnly lacks Put");
        for name in ["Get", "Put"] {
            let (from, to) = (
                gid(node_kind::METHOD, &format!("mem::MemStore::{name}")),
                gid(node_kind::METHOD, &format!("store::Store::{name}")),
            );
            assert!(implements_edge(&g, from, to).is_some(), "method-level {name}");
        }
        let (ro_get, store_get) =
            (gid(node_kind::METHOD, "mem::ReadOnly::Get"), gid(node_kind::METHOD, "store::Store::Get"));
        assert_eq!(implements_edge(&g, ro_get, store_get), None);
    }

    /// Several interfaces and implementors: the whole edge Vec is identical
    /// across builds (the method tables' HashMap seeds differ per map).
    #[test]
    fn go_implicit_implements_is_deterministic() {
        let shape = || {
            let mut items: Vec<(glia_core::NodeKindId, String, Option<String>)> = Vec::new();
            for i in ["A", "B", "C", "D"] {
                let q = format!("pkg::I{i}");
                items.push((node_kind::INTERFACE, q.clone(), None));
                for m in ["Open", "Close", "Read"] {
                    items.push((node_kind::METHOD, format!("{q}::{m}{i}"), Some(q.clone())));
                }
            }
            for t in ["W", "X", "Y", "Z"] {
                let q = format!("pkg::{t}");
                items.push((node_kind::STRUCT, q.clone(), None));
                for i in ["A", "B", "C", "D"] {
                    for m in ["Open", "Close", "Read"] {
                        items.push((node_kind::METHOD, format!("{q}::{m}{i}"), Some(q.clone())));
                    }
                }
            }
            let borrowed: Vec<_> =
                items.iter().map(|(k, q, p)| (*k, q.as_str(), p.as_deref())).collect();
            vec![go_file("pkg", &borrowed)]
        };
        let first = build_go(repo(), shape()).unwrap();
        let second = build_go(repo(), shape()).unwrap();
        let implements: Vec<_> =
            first.edges.iter().filter(|e| e.category == edge_category::IMPLEMENTS).collect();
        let from_kind = |e: &&Edge| first.nav.kind_by_id.get(&e.from).copied();
        let (types, methods): (Vec<_>, Vec<_>) =
            implements.into_iter().partition(|e| from_kind(e) == Some(node_kind::STRUCT));
        assert_eq!(types.len(), 16, "4 types x 4 interfaces");
        assert!(types.iter().all(|e| e.confidence == Confidence::Medium));
        // CA.3b: the method-level pairs ride on the Medium type-level edges.
        assert_eq!(methods.len(), 48, "16 type-level edges x 3 methods");
        assert!(methods.iter().all(|e| e.confidence == Confidence::Medium));
        assert_eq!(first.edges, second.edges);
    }

    /// The fixtures/go-iface-embed shape: `Store` embeds `Reader`, so its set
    /// is {Get, Put}; `Mem` has both and implements both interfaces, `Half`
    /// has only `Put` and implements neither.
    #[test]
    fn go_embedded_interface_method_set_is_transitive() {
        let mut file = go_file(
            "store",
            &[
                (node_kind::INTERFACE, "store::Reader", None),
                (node_kind::METHOD, "store::Reader::Get", Some("store::Reader")),
                (node_kind::INTERFACE, "store::Store", None),
                (node_kind::METHOD, "store::Store::Put", Some("store::Store")),
                (node_kind::STRUCT, "store::Mem", None),
                (node_kind::METHOD, "store::Mem::Get", Some("store::Mem")),
                (node_kind::METHOD, "store::Mem::Put", Some("store::Mem")),
                (node_kind::STRUCT, "store::Half", None),
                (node_kind::METHOD, "store::Half::Put", Some("store::Half")),
            ],
        );
        file.refs = vec![embed("store", "store::Store", CallQualifier::Bare("Reader".to_string()))];
        let (reader, store, mem, half) = (
            gid(node_kind::INTERFACE, "store::Reader"),
            gid(node_kind::INTERFACE, "store::Store"),
            gid(node_kind::STRUCT, "store::Mem"),
            gid(node_kind::STRUCT, "store::Half"),
        );
        let (g, stats) = go_implicit(vec![file]);
        assert!(has_edge(&g, store, reader, edge_category::INHERITS_FROM));
        assert_eq!(implements_edge(&g, mem, store), Some(Confidence::Medium));
        assert_eq!(implements_edge(&g, mem, reader), Some(Confidence::Medium));
        assert_eq!(implements_edge(&g, half, store), None, "Half lacks the embedded Get");
        assert_eq!(implements_edge(&g, half, reader), None);
        assert_eq!(
            stats.map(|s| s.marker()).as_deref(),
            Some("[iface] go implicit implements: 2 (interfaces=2 types=1 embedded=1 open=0)")
        );
    }

    /// An embed that does not bind (`io.Reader` from outside the parse)
    /// leaves the interface's method set unknown: no type implements it on
    /// its own `Close` alone. The predeclared `error` is known: `Error`.
    #[test]
    fn go_unbound_embed_leaves_interface_unmatched() {
        let mut file = go_file(
            "port",
            &[
                (node_kind::INTERFACE, "port::ReadCloser", None),
                (node_kind::METHOD, "port::ReadCloser::Close", Some("port::ReadCloser")),
                (node_kind::INTERFACE, "port::CodedError", None),
                (node_kind::METHOD, "port::CodedError::Code", Some("port::CodedError")),
                (node_kind::STRUCT, "port::File", None),
                (node_kind::METHOD, "port::File::Close", Some("port::File")),
                (node_kind::METHOD, "port::File::Code", Some("port::File")),
                (node_kind::STRUCT, "port::Fault", None),
                (node_kind::METHOD, "port::Fault::Code", Some("port::Fault")),
                (node_kind::METHOD, "port::Fault::Error", Some("port::Fault")),
            ],
        );
        file.refs = vec![
            embed(
                "port",
                "port::ReadCloser",
                CallQualifier::Attribute { base: "io".to_string(), name: "Reader".to_string() },
            ),
            embed("port", "port::CodedError", CallQualifier::Bare("error".to_string())),
        ];
        let (read_closer, coded, file_ty, fault) = (
            gid(node_kind::INTERFACE, "port::ReadCloser"),
            gid(node_kind::INTERFACE, "port::CodedError"),
            gid(node_kind::STRUCT, "port::File"),
            gid(node_kind::STRUCT, "port::Fault"),
        );
        let (g, stats) = go_implicit(vec![file]);
        assert_eq!(implements_edge(&g, file_ty, read_closer), None);
        assert_eq!(implements_edge(&g, file_ty, coded), None, "File has Code but no Error");
        assert_eq!(implements_edge(&g, fault, coded), Some(Confidence::Medium));
        assert_eq!(
            stats.map(|s| s.marker()).as_deref(),
            Some("[iface] go implicit implements: 1 (interfaces=1 types=1 embedded=0 open=1)")
        );
    }

    /// An unexported method name belongs to its package: a type in another
    /// directory with the same `isSealed` does not implement the interface.
    #[test]
    fn go_unexported_method_matches_only_its_package() {
        let parses = vec![
            go_file(
                "a::sealed",
                &[
                    (node_kind::INTERFACE, "a::sealed::Sealed", None),
                    (node_kind::METHOD, "a::sealed::Sealed::isSealed", Some("a::sealed::Sealed")),
                ],
            ),
            go_file(
                "a::impl",
                &[
                    (node_kind::STRUCT, "a::impl::T", None),
                    (node_kind::METHOD, "a::impl::T::isSealed", Some("a::impl::T")),
                ],
            ),
            go_file(
                "b::impl",
                &[
                    (node_kind::STRUCT, "b::impl::U", None),
                    (node_kind::METHOD, "b::impl::U::isSealed", Some("b::impl::U")),
                ],
            ),
        ];
        let sealed = gid(node_kind::INTERFACE, "a::sealed::Sealed");
        let (g, _) = go_implicit(parses);
        let same_pkg = gid(node_kind::STRUCT, "a::impl::T");
        assert_eq!(implements_edge(&g, same_pkg, sealed), Some(Confidence::Medium));
        assert_eq!(implements_edge(&g, gid(node_kind::STRUCT, "b::impl::U"), sealed), None);
    }

    /// Embeds bind to an INTERFACE only, package-scoped: `Reader` is the
    /// same-directory interface in another file (not a same-named DATA_ENTITY
    /// of the embedding file, not an interface of another package); `store.
    /// Writer` is the interface of the imported package, also when the go.mod
    /// sits below the repo root (`backend::internal::store`); `io.Closer` and
    /// an ambiguous name stay unresolved.
    #[test]
    fn go_embed_binds_package_scoped_interfaces_only() {
        let mut api = go_file(
            "backend::svc::api",
            &[
                (node_kind::INTERFACE, "backend::svc::api::Port", None),
                (node_kind::DATA_ENTITY, "backend::svc::api::Reader", None),
            ],
        );
        api.imports = vec![
            ImportStmt {
                from_module: "backend::svc::api".to_string(),
                target: ImportTarget::Module { path: "internal::store".to_string(), alias: None },
                line: 0,
            },
            ImportStmt {
                from_module: "backend::svc::api".to_string(),
                target: ImportTarget::Module { path: "io".to_string(), alias: None },
                line: 0,
            },
        ];
        let port = "backend::svc::api::Port";
        api.refs = vec![
            embed("backend::svc::api", port, CallQualifier::Bare("Reader".to_string())),
            embed(
                "backend::svc::api",
                port,
                CallQualifier::Attribute { base: "store".to_string(), name: "Writer".to_string() },
            ),
            embed(
                "backend::svc::api",
                port,
                CallQualifier::Attribute { base: "io".to_string(), name: "Closer".to_string() },
            ),
            embed("backend::svc::api", port, CallQualifier::Bare("Dup".to_string())),
        ];
        let parses = vec![
            api,
            go_file("backend::svc::read", &[(node_kind::INTERFACE, "backend::svc::read::Reader", None)]),
            go_file("backend::other::read", &[(node_kind::INTERFACE, "backend::other::read::Reader", None)]),
            go_file(
                "backend::internal::store::write",
                &[(node_kind::INTERFACE, "backend::internal::store::write::Writer", None)],
            ),
            go_file("backend::svc::dup_a", &[(node_kind::INTERFACE, "backend::svc::dup_a::Dup", None)]),
            go_file("backend::svc::dup_b", &[(node_kind::INTERFACE, "backend::svc::dup_b::Dup", None)]),
        ];
        let (g, _) = go_implicit(parses);
        let from = gid(node_kind::INTERFACE, port);
        let mut embedded: Vec<NodeId> = g
            .edges
            .iter()
            .filter(|e| e.from == from && e.category == edge_category::INHERITS_FROM)
            .map(|e| e.to)
            .collect();
        embedded.sort_unstable_by_key(|id| id.0);
        let mut expect = vec![
            gid(node_kind::INTERFACE, "backend::svc::read::Reader"),
            gid(node_kind::INTERFACE, "backend::internal::store::write::Writer"),
        ];
        expect.sort_unstable_by_key(|id| id.0);
        assert_eq!(embedded, expect);
        let unbound: Vec<&CallQualifier> =
            g.unresolved_refs.iter().filter(|r| r.from == from).map(|r| &r.qualifier).collect();
        assert_eq!(
            unbound,
            vec![
                &CallQualifier::Attribute { base: "io".to_string(), name: "Closer".to_string() },
                &CallQualifier::Bare("Dup".to_string()),
            ]
        );
    }

    /// An empty interface is implemented by nothing, and a graph with no
    /// INTERFACE reports no marker.
    #[test]
    fn go_empty_interface_and_no_interface() {
        let file = go_file(
            "any",
            &[
                (node_kind::INTERFACE, "any::Anything", None),
                (node_kind::STRUCT, "any::T", None),
                (node_kind::METHOD, "any::T::Run", Some("any::T")),
            ],
        );
        let (g, stats) = go_implicit(vec![file]);
        assert!(!g.edges.iter().any(|e| e.category == edge_category::IMPLEMENTS));
        assert_eq!(
            stats.map(|s| s.marker()).as_deref(),
            Some("[iface] go implicit implements: 0 (interfaces=0 types=0 embedded=0 open=0)")
        );
        let (_, none) = go_implicit(vec![go_file("x", &[(node_kind::STRUCT, "x::T", None)])]);
        assert_eq!(none, None);
    }

    // ---- CA.3b: signature and reachability gates ----------------------------

    /// Go sources parsed as the engine parses them (go.mod `example.com/scope`
    /// at the repo root): `a/b.go` is MODULE `a::b`, so its package dir is `a`.
    fn go_sources(files: &[(&str, &str)]) -> Vec<FileParse> {
        files
            .iter()
            .map(|(rel, src)| {
                let qname = rel.trim_end_matches(".go").replace('/', "::");
                glia_parser_go::parse_file(src, rel, &qname, "example.com/scope", repo())
                    .expect("parse")
            })
            .collect()
    }

    /// The EVIDENCE rule of the IMPLEMENTS edge `from -> to`.
    fn implements_rule(g: &RepoGraph, from: NodeId, to: NodeId) -> Option<String> {
        g.edges
            .iter()
            .find(|e| e.from == from && e.to == to && e.category == edge_category::IMPLEMENTS)
            .and_then(Evidence::of)
            .and_then(|ev| ev.rule)
    }

    fn filtered(stats: &Option<GoImplicitStats>) -> Option<String> {
        stats.as_ref().map(GoImplicitStats::filtered_marker)
    }

    const STORE_GO: &str = "package store\n\ntype Store interface {\n\tGet(key string) string\n\
                            \tPut(key, value string)\n}\n";
    const CLOSER_GO: &str = "package a\n\ntype Closer interface {\n\tClose()\n}\n";
    const FILE_GO: &str = "package b\n\ntype File struct{}\n\nfunc (f *File) Close() {}\n";

    /// A type with every one of an interface's method names but other
    /// signatures does not implement it (parameters or, the Kina
    /// `FindByID` case, results only); equal signatures under other parameter
    /// names do, with EVIDENCE rule `method_signature`.
    #[test]
    fn go_implicit_signature_mismatch_is_rejected() {
        let other = "package other\n\ntype Wrong struct{}\n\n\
                     func (w *Wrong) Get(key int) int { return key }\n\
                     func (w *Wrong) Put(key, value int) {}\n";
        let mem = "package mem\n\ntype Mem struct{}\n\n\
                   func (m *Mem) Get(k string) string { return k }\n\
                   func (m *Mem) Put(k string, v string) {}\n";
        let loader = "package store\n\ntype User struct{}\n\ntype KYCDocument struct{}\n\n\
                      type UserLoader interface {\n\tFindByID(id string) (*User, error)\n}\n\n\
                      type UserRepo struct{}\n\n\
                      func (r *UserRepo) FindByID(id string) (*User, error) { return nil, nil }\n\n\
                      type KYCRepo struct{}\n\n\
                      func (r *KYCRepo) FindByID(id string) (*KYCDocument, error) { return nil, nil }\n";
        let parses = go_sources(&[
            ("store/store.go", STORE_GO),
            ("store/loader.go", loader),
            ("other/other.go", other),
            ("mem/mem.go", mem),
        ]);
        let (g, stats) = go_implicit(parses);
        let store = gid(node_kind::INTERFACE, "store::store::Store");
        let (wrong, mem_ty) =
            (gid(node_kind::STRUCT, "other::other::Wrong"), gid(node_kind::STRUCT, "mem::mem::Mem"));
        assert_eq!(implements_edge(&g, wrong, store), None, "names match, signatures do not");
        assert_eq!(
            implements_edge(
                &g,
                gid(node_kind::METHOD, "other::other::Wrong::Get"),
                gid(node_kind::METHOD, "store::store::Store::Get")
            ),
            None
        );
        assert_eq!(implements_edge(&g, mem_ty, store), Some(Confidence::Medium));
        assert_eq!(implements_rule(&g, mem_ty, store).as_deref(), Some("method_signature"));
        let user_loader = gid(node_kind::INTERFACE, "store::loader::UserLoader");
        assert!(implements_edge(&g, gid(node_kind::STRUCT, "store::loader::UserRepo"), user_loader).is_some());
        assert_eq!(
            implements_edge(&g, gid(node_kind::STRUCT, "store::loader::KYCRepo"), user_loader),
            None,
            "the result type differs"
        );
        assert_eq!(
            filtered(&stats).as_deref(),
            Some(
                "[iface] go implicit filtered: signature=2 scope=0 (one_method=0 test_side=0) \
                 signature_checked=2 root_assumed=0"
            )
        );
    }

    /// With a signature unknown on either side (no parse recorded one, or a
    /// method on a generic receiver) the name match stands, rule `method_set`.
    #[test]
    fn go_implicit_unknown_signature_keeps_the_name_match() {
        let (store, mem_store) =
            (gid(node_kind::INTERFACE, "store::Store"), gid(node_kind::STRUCT, "mem::MemStore"));
        let (g, stats) = go_implicit(implicit_iface_shape());
        assert_eq!(implements_rule(&g, mem_store, store).as_deref(), Some("method_set"));
        assert_eq!(
            filtered(&stats).as_deref(),
            Some(
                "[iface] go implicit filtered: signature=0 scope=0 (one_method=0 test_side=0) \
                 signature_checked=0 root_assumed=0"
            )
        );

        let boxed = "package box\n\ntype Box[T any] struct{}\n\n\
                     func (b *Box[T]) Get(key string) string { return key }\n\
                     func (b *Box[T]) Put(key, value string) {}\n";
        let (g, _) = go_implicit(go_sources(&[("store/store.go", STORE_GO), ("box/box.go", boxed)]));
        let (store, box_ty) =
            (gid(node_kind::INTERFACE, "store::store::Store"), gid(node_kind::STRUCT, "box::box::Box"));
        assert_eq!(implements_rule(&g, box_ty, store).as_deref(), Some("method_set"));
    }

    /// A one-method interface pairs only across an import path: no import
    /// either way rejects; the type's package importing the interface's, the
    /// interface's importing the type's, or a chain of imports keeps it.
    #[test]
    fn go_implicit_scope_one_method_needs_an_import_path() {
        let (closer, file) =
            (gid(node_kind::INTERFACE, "a::closer::Closer"), gid(node_kind::STRUCT, "b::file::File"));
        let (g, stats) = go_implicit(go_sources(&[("a/closer.go", CLOSER_GO), ("b/file.go", FILE_GO)]));
        assert_eq!(implements_edge(&g, file, closer), None);
        assert_eq!(
            filtered(&stats).as_deref(),
            Some(
                "[iface] go implicit filtered: signature=0 scope=1 (one_method=1 test_side=0) \
                 signature_checked=0 root_assumed=0"
            )
        );

        let b_imports_a = "package b\n\nimport \"example.com/scope/a\"\n\ntype File struct{}\n\n\
                           func (f *File) Close() {}\n\nvar _ a.Closer = (*File)(nil)\n";
        let (g, stats) =
            go_implicit(go_sources(&[("a/closer.go", CLOSER_GO), ("b/file.go", b_imports_a)]));
        assert_eq!(implements_edge(&g, file, closer), Some(Confidence::Medium), "b imports a");
        assert_eq!(implements_rule(&g, file, closer).as_deref(), Some("method_signature"));
        assert_eq!(
            filtered(&stats).as_deref(),
            Some(
                "[iface] go implicit filtered: signature=0 scope=0 (one_method=0 test_side=0) \
                 signature_checked=1 root_assumed=0"
            )
        );

        let a_imports_b = "package a\n\nimport \"example.com/scope/b\"\n\n\
                           type Closer interface {\n\tClose()\n}\n\nfunc Wrap(f *b.File) Closer { return f }\n";
        let (g, _) = go_implicit(go_sources(&[("a/closer.go", a_imports_b), ("b/file.go", FILE_GO)]));
        assert!(implements_edge(&g, file, closer).is_some(), "a imports b");

        let b_imports_c = "package b\n\nimport \"example.com/scope/c\"\n\ntype File struct{}\n\n\
                           func (f *File) Close() {}\n\nfunc Use() { c.Take(&File{}) }\n";
        let c_imports_a = "package c\n\nimport \"example.com/scope/a\"\n\nfunc Take(x a.Closer) {}\n";
        let (g, _) = go_implicit(go_sources(&[
            ("a/closer.go", CLOSER_GO),
            ("b/file.go", b_imports_c),
            ("c/c.go", c_imports_a),
        ]));
        assert!(implements_edge(&g, file, closer).is_some(), "b -> c -> a, transitively");
    }

    /// A package importing both sides wires them: the pair is kept though
    /// neither imports the other.
    #[test]
    fn go_implicit_scope_shared_importer() {
        let main = "package main\n\nimport (\n\t\"example.com/scope/a\"\n\t\"example.com/scope/b\"\n)\n\n\
                    func main() { var c a.Closer = &b.File{}; c.Close() }\n";
        let (g, stats) = go_implicit(go_sources(&[
            ("a/closer.go", CLOSER_GO),
            ("b/file.go", FILE_GO),
            ("cmd/main.go", main),
        ]));
        let (closer, file) =
            (gid(node_kind::INTERFACE, "a::closer::Closer"), gid(node_kind::STRUCT, "b::file::File"));
        assert_eq!(implements_edge(&g, file, closer), Some(Confidence::Medium));
        assert_eq!(stats.map(|s| (s.one_method, s.test_side)), Some((0, 0)));
    }

    /// An interface declared in a `_test.go` file pairs with a type of its
    /// own directory (test or not) and with a type of a package the test
    /// file's own imports reach, never with an unreached one; a test file's
    /// package imports are not its own.
    #[test]
    fn go_implicit_scope_test_interface() {
        let x_test = "package tests\n\nimport \"testing\"\n\n\
                      type Closable interface {\n\tClose()\n}\n\n\
                      type fake struct{}\n\nfunc (f fake) Close() {}\n\n\
                      func TestClose(t *testing.T) { var c Closable = fake{}; c.Close() }\n";
        let conn = "package tests\n\nimport \"example.com/scope/b\"\n\n\
                    type Conn struct{ f *b.File }\n\nfunc (c *Conn) Close() {}\n";
        let closable = gid(node_kind::INTERFACE, "tests::x_test::Closable");
        let (fake, conn_ty, file) = (
            gid(node_kind::STRUCT, "tests::x_test::fake"),
            gid(node_kind::STRUCT, "tests::conn::Conn"),
            gid(node_kind::STRUCT, "b::file::File"),
        );
        let (g, stats) = go_implicit(go_sources(&[
            ("tests/x_test.go", x_test),
            ("tests/conn.go", conn),
            ("b/file.go", FILE_GO),
        ]));
        assert_eq!(implements_edge(&g, fake, closable), Some(Confidence::Medium), "same file");
        assert_eq!(implements_edge(&g, conn_ty, closable), Some(Confidence::Medium), "same dir");
        assert_eq!(
            implements_edge(&g, file, closable),
            None,
            "conn.go imports b, but the test file does not"
        );
        assert_eq!(stats.map(|s| (s.one_method, s.test_side)), Some((0, 1)));

        let x_test_imports_b = x_test.replace(
            "import \"testing\"",
            "import (\n\t\"testing\"\n\n\t\"example.com/scope/b\"\n)\n\nvar _ = b.File{}",
        );
        let (g, _) = go_implicit(go_sources(&[
            ("tests/x_test.go", x_test_imports_b.as_str()),
            ("b/file.go", FILE_GO),
        ]));
        assert!(implements_edge(&g, file, closable).is_some(), "the test file imports b");
    }

    /// The Go parser records no import of the repository-root package, so a
    /// root-package side counts as reached (`root_assumed`).
    #[test]
    fn go_implicit_root_package_is_assumed_reached() {
        let root = "package scope\n\ntype Closer interface {\n\tClose()\n}\n";
        let b = "package b\n\nimport \"example.com/scope\"\n\ntype File struct{}\n\n\
                 func (f *File) Close() {}\n\nvar _ scope.Closer = (*File)(nil)\n";
        let parses = go_sources(&[("closer.go", root), ("b/file.go", b)]);
        assert!(parses[1].imports.is_empty(), "the root import is not recorded");
        let (g, stats) = go_implicit(parses);
        let (closer, file) =
            (gid(node_kind::INTERFACE, "closer::Closer"), gid(node_kind::STRUCT, "b::file::File"));
        assert_eq!(implements_edge(&g, file, closer), Some(Confidence::Medium));
        assert_eq!(
            filtered(&stats).as_deref(),
            Some(
                "[iface] go implicit filtered: signature=0 scope=0 (one_method=0 test_side=0) \
                 signature_checked=1 root_assumed=1"
            )
        );
    }

    /// A6.6's method pairs of a Go implicit (Medium) type-level edge are
    /// Medium too, as `why` and the implementors owner walk read them.
    #[test]
    fn go_method_level_pairs_inherit_medium() {
        let g = build_go(repo(), implicit_iface_shape()).unwrap();
        for name in ["Get", "Put"] {
            let (from, to) = (
                gid(node_kind::METHOD, &format!("mem::MemStore::{name}")),
                gid(node_kind::METHOD, &format!("store::Store::{name}")),
            );
            assert_eq!(implements_edge(&g, from, to), Some(Confidence::Medium), "{name}");
            assert_eq!(implements_rule(&g, from, to).as_deref(), Some("same_name"));
        }
    }

    // ---- LA.13b: Go package = directory -------------------------------------

    /// The fixtures/go-package-multifile-calls shape: `cmd/main.go` imports
    /// `internal/store` and calls `store.Save()` / `store.Load()`; `Save`
    /// (store.go) calls `helper()` (load.go).
    fn multifile_package_shape() -> Vec<FileParse> {
        let mut main = go_file("cmd::main", &[(node_kind::FUNCTION, "cmd::main::main", None)]);
        main.imports = vec![ImportStmt {
            from_module: "cmd::main".to_string(),
            target: ImportTarget::Module { path: "internal::store".to_string(), alias: None },
            line: 0,
        }];
        let from = gid(node_kind::FUNCTION, "cmd::main::main");
        let attr = |name: &str| CallQualifier::Attribute { base: "store".to_string(), name: name.to_string() };
        main.calls = vec![
            CallSite { from, qualifier: attr("Save"), line: 0 },
            CallSite { from, qualifier: attr("Load"), line: 0 },
        ];
        let mut store =
            go_file("internal::store::store", &[(node_kind::FUNCTION, "internal::store::store::Save", None)]);
        store.calls = vec![CallSite {
            from: gid(node_kind::FUNCTION, "internal::store::store::Save"),
            qualifier: CallQualifier::Bare("helper".to_string()),
            line: 0,
        }];
        let load = go_file(
            "internal::store::load",
            &[
                (node_kind::FUNCTION, "internal::store::load::Load", None),
                (node_kind::FUNCTION, "internal::store::load::helper", None),
            ],
        );
        vec![main, store, load]
    }

    /// The `[go-package]` marker counts the package directories, the
    /// multi-file ones, the imports the directory step bound and the calls
    /// bound in a sibling file; the fixture prints the packet's marker.
    #[test]
    fn go_package_stats_and_marker() {
        let (g, _, _, stats, receivers, _) = build_go_passes(repo(), multifile_package_shape());
        assert_eq!(
            stats,
            GoPackageStats { dirs: 2, multi_file: 1, dir_bound_imports: 1, sibling_calls: 2 }
        );
        assert_eq!(
            stats.marker().as_deref(),
            Some("[go-package] dirs=2 multi_file=1 dir_bound_imports=1 sibling_calls=2")
        );
        let main = gid(node_kind::FUNCTION, "cmd::main::main");
        let save = gid(node_kind::FUNCTION, "internal::store::store::Save");
        assert!(has_edge(&g, main, save, edge_category::CALLS));
        assert!(has_edge(&g, main, gid(node_kind::FUNCTION, "internal::store::load::Load"), edge_category::CALLS));
        assert!(has_edge(&g, save, gid(node_kind::FUNCTION, "internal::store::load::helper"), edge_category::CALLS));
        assert!(g.unresolved_calls.is_empty());
        assert_eq!(
            receivers.marker(),
            "[go-receivers] bound=0 (return=0 local=0 package_var=0 field_chain=0) typed_unbound=0",
            "package calls are not typed-receiver binds"
        );

        let (_, _, _, empty, _, _) = build_go_passes(repo(), vec![]);
        assert_eq!(empty.marker(), None, "no MODULE, no marker");
    }

    // ---- CA.2b: Go typed receivers -----------------------------------------

    /// The `[go-receivers]` marker: bound is the sum of the four sources.
    #[test]
    fn receiver_tally_marker_shape() {
        let tally = ReceiverTally::default();
        for (source, n) in [
            (RecvSource::Return, 3),
            (RecvSource::Local, 2),
            (RecvSource::PackageVar, 1),
            (RecvSource::FieldChain, 1),
        ] {
            for _ in 0..n {
                tally.bound(source);
            }
        }
        tally.typed_unbound.set(2);
        assert_eq!(
            tally.marker(),
            "[go-receivers] bound=7 (return=3 local=2 package_var=1 field_chain=1) typed_unbound=2"
        );
    }

    /// A receiver chain splits into names and calls; raw fallback text (an
    /// index, an assertion, a literal) and an over-long chain do not parse.
    #[test]
    fn receiver_segments_parse_normalised_chains_only() {
        use Seg::{Call, Ident};
        assert_eq!(
            receiver_segments("Services.UserRepository()"),
            Some(vec![Ident("Services"), Call("UserRepository")])
        );
        assert_eq!(
            receiver_segments("self.deps.repo"),
            Some(vec![Ident("self"), Ident("deps"), Ident("repo")])
        );
        assert_eq!(receiver_segments("repo"), Some(vec![Ident("repo")]));
        for raw in ["a[0]", "x.(T)", "\"s\".f", "a()()", "a..b", "", "1x", "a b"] {
            assert_eq!(receiver_segments(raw), None, "{raw:?}");
        }
        assert!(receiver_segments("a.b.c.d.e.f").is_some());
        assert_eq!(receiver_segments("a.b.c.d.e.f.g"), None, "longer than RECV_MAX_SEGS");
    }

    /// The bench fixture go-return-type-receivers, parsed as the engine
    /// does: one bind per source kind it exercises, and no typed site left
    /// unbound.
    #[test]
    fn go_receiver_tally_counts_the_fixture() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .map(|p| p.join("bench/substrate-gap/fixtures/go-return-type-receivers"))
            .expect("workspace root");
        let parses: Vec<FileParse> = [
            "handlers/users.go",
            "repositories/order_repository.go",
            "repositories/user_repository.go",
            "services/provider.go",
        ]
        .iter()
        .map(|rel| {
            let src = std::fs::read_to_string(root.join(rel)).expect("fixture file");
            let qname = rel.trim_end_matches(".go").replace('/', "::");
            glia_parser_go::parse_file(&src, rel, &qname, "example.com/shop", repo()).expect("parse")
        })
        .collect();
        let (_, _, _, _, receivers, _) = build_go_passes(repo(), parses);
        assert_eq!(
            receivers.marker(),
            "[go-receivers] bound=4 (return=1 local=2 package_var=1 field_chain=0) typed_unbound=0"
        );
    }

    /// An import of a directory binds its dir-named file, else its first
    /// non-test file by qname, else its first test file, never the importer,
    /// whatever order the files arrive in; a root-level file's package is
    /// `""`.
    #[test]
    fn go_import_target_prefers_dir_named_then_non_test_files() {
        let m = |q: &str| gid(node_kind::MODULE, q);
        for order in [[0, 1, 2, 3, 4, 5, 6], [6, 5, 4, 3, 2, 1, 0]] {
            let files = [
                go_file("a::store::zeta", &[]),
                go_file("a::store::store", &[]),
                go_file("b::util::y", &[]),
                go_file("b::util::a_test", &[]),
                go_file("c::only::only_test", &[]),
                go_file("c::only::more_test", &[]),
                go_file("main", &[]),
            ];
            let parses: Vec<FileParse> = order.iter().map(|&i| files[i].clone()).collect();
            let (mut g, imports, _, _) = merge_parses(repo(), parses);
            build_symbol_table(&mut g);
            let pk = GoPackages::build(&g, &imports);
            let importer = m("main");
            assert_eq!(pk.import_target("a::store", importer), DirImport::Bound(m("a::store::store")));
            assert_eq!(pk.import_target("b::util", importer), DirImport::Bound(m("b::util::y")));
            assert_eq!(pk.import_target("c::only", importer), DirImport::Bound(m("c::only::more_test")));
            assert_eq!(
                pk.import_target("c::only", m("c::only::more_test")),
                DirImport::Bound(m("c::only::only_test"))
            );
            assert_eq!(pk.import_target("a", importer), DirImport::NoDir);
            assert_eq!(pk.import_target("", m("a::store::zeta")), DirImport::Bound(importer));
            assert_eq!(pk.import_target("", importer), DirImport::OnlyImporter);
        }
    }

    // ---- LB.9b: bare-path aliases of file-named MODULEs ---------------------

    /// A MODULE `qname` with nav name `name` (a file-named MODULE's name is
    /// its stem) plus FUNCTION children `fns` (simple names).
    fn named_module(qname: &str, name: &str, fns: &[&str]) -> FileParse {
        let r = repo();
        let m = gid(node_kind::MODULE, qname);
        let mut fp = FileParse::default();
        fp.nodes.push(Node { id: m, repo: r, confidence: Confidence::Strong, cells: vec![] });
        fp.nav.record(m, name, qname, node_kind::MODULE, None);
        for f in fns {
            let q = format!("{qname}::{f}");
            let id = gid(node_kind::FUNCTION, &q);
            fp.nodes.push(Node { id, repo: r, confidence: Confidence::Strong, cells: vec![] });
            fp.nav.record(id, f, &q, node_kind::FUNCTION, Some(m));
        }
        fp
    }

    /// `api/main.py`: `from api.user import validate`.
    fn py_importer() -> FileParse {
        let mut main = named_module("api::main", "main", &[]);
        main.imports = vec![ImportStmt {
            from_module: "api::main".to_string(),
            target: ImportTarget::Symbol {
                module: "api.user".to_string(),
                name: "validate".to_string(),
                alias: None,
                level: 0,
            },
            line: 0,
        }];
        main
    }

    #[test]
    fn a_file_named_module_answers_to_its_bare_path() {
        let g = build_python(
            repo(),
            vec![py_importer(), named_module("api::user.py", "user", &["validate"])],
        )
        .unwrap();
        let user = gid(node_kind::MODULE, "api::user.py");
        assert_eq!(g.symbols.module_by_qname.get("api::user"), Some(&user));
        assert_eq!(g.symbols.module_by_qname.get("api::user.py"), Some(&user));
        assert!(
            has_edge(&g, gid(node_kind::MODULE, "api::main"), user, edge_category::IMPORTS),
            "the bare-path import binds the file-named MODULE"
        );
        let bound = g.symbols.module_import_bindings[&gid(node_kind::MODULE, "api::main")]
            .get("validate")
            .copied();
        assert_eq!(bound, Some(gid(node_kind::FUNCTION, "api::user.py::validate")));
    }

    #[test]
    fn two_file_named_modules_with_one_bare_path_register_no_alias() {
        // `util.js` + `util.ts` beside a `util.py`: both TS files are
        // file-named and share `a::util`; neither is guessed.
        let g = build_python(
            repo(),
            vec![
                named_module("a::util.js", "util", &[]),
                named_module("a::util.ts", "util", &[]),
            ],
        )
        .unwrap();
        assert_eq!(g.symbols.module_by_qname.get("a::util"), None);
        assert_eq!(g.symbols.module_by_qname.len(), 2);
    }

    #[test]
    fn a_real_module_keeps_its_qname_over_an_alias() {
        let g = build_python(
            repo(),
            vec![
                named_module("api::user", "user", &[]),
                named_module("api::user.py", "user", &[]),
            ],
        )
        .unwrap();
        assert_eq!(
            g.symbols.module_by_qname.get("api::user"),
            Some(&gid(node_kind::MODULE, "api::user"))
        );
    }

    #[test]
    fn go_packages_read_a_file_named_module_once_by_its_bare_form() {
        // `infra/main.go` + `infra/main.tf`, `infra/main_test.go` + `.py`:
        // one member each, the test file still a test, the alias not a member.
        let (g, _, _, stats, _, _) = build_go_passes(
            repo(),
            vec![
                named_module("infra::main.go", "main", &[]),
                named_module("infra::main_test.go", "main_test", &[]),
            ],
        );
        assert_eq!(g.symbols.module_by_qname.len(), 4, "two MODULEs, two aliases");
        assert_eq!(stats.dirs, 1);
        assert_eq!(stats.multi_file, 1);
        let (mut g2, imports, _, _) = merge_parses(
            repo(),
            vec![
                named_module("infra::main.go", "main", &[]),
                named_module("infra::main_test.go", "main_test", &[]),
                named_module("cmd::run", "run", &[]),
            ],
        );
        build_symbol_table(&mut g2);
        let pk = GoPackages::build(&g2, &imports);
        assert_eq!(pk.by_dir["infra"].len(), 2);
        assert!(pk.tests.contains(&gid(node_kind::MODULE, "infra::main_test.go")));
        assert_eq!(
            pk.import_target("infra", gid(node_kind::MODULE, "cmd::run")),
            DirImport::Bound(gid(node_kind::MODULE, "infra::main.go"))
        );
    }

    // ---- LB.10c: C/C++ out-of-line members ----------------------------------

    /// One C/C++ file shaped the way LB.10b's parser emits it: MODULE
    /// `module` (nav name its stem) plus `(kind, qname, nav name, parent)`
    /// items, where a `None` parent is the MODULE and `Some(q)` an earlier
    /// item of this file (a namespace PACKAGE, a CLASS). Each item gets its
    /// parent -> item DEFINES edge and one CODE cell `<module>|<qname>`, so a
    /// merged node shows whose cells it carries.
    fn cpp_file(
        module: &str,
        items: &[(glia_core::NodeKindId, &str, &str, Option<&str>)],
    ) -> FileParse {
        let r = repo();
        let m = gid(node_kind::MODULE, module);
        let file = module.rsplit("::").next().unwrap_or(module);
        let stem = file.split('.').next().unwrap_or(file);
        let mut nav = CodeNav::default();
        nav.record(m, stem, module, node_kind::MODULE, None);
        let mut ids: HashMap<&str, NodeId> = HashMap::new();
        let mut nodes = vec![Node {
            id: m,
            repo: r,
            confidence: Confidence::Strong,
            cells: vec![],
        }];
        let mut edges = vec![];
        for &(kind, qname, name, parent) in items {
            let id = gid(kind, qname);
            let parent_id = parent.map_or(m, |p| ids[p]);
            nav.record(id, name, qname, kind, Some(parent_id));
            nodes.push(Node {
                id,
                repo: r,
                confidence: Confidence::Strong,
                cells: vec![code_cell(module, qname)],
            });
            edges.push(Edge::new(
                parent_id,
                id,
                edge_category::DEFINES,
                Confidence::Strong,
            ));
            ids.insert(qname, id);
        }
        FileParse {
            nodes,
            edges,
            nav,
            ..FileParse::default()
        }
    }

    fn code_cell(module: &str, qname: &str) -> Cell {
        Cell {
            kind: glia_code_domain::cell_type::CODE,
            payload: glia_core::CellPayload::Text(format!("{module}|{qname}")),
        }
    }

    fn include(from: &str, spec: &str) -> ImportStmt {
        ImportStmt {
            from_module: from.to_string(),
            target: ImportTarget::Module {
                path: spec.to_string(),
                alias: None,
            },
            line: 0,
        }
    }

    /// The engine's include resolver, reduced: `spec` relative to the
    /// including file's directory, file name kept.
    fn include_source(from: &str, spec: &str) -> Option<String> {
        let mut segs: Vec<&str> = from.split("::").collect();
        segs.pop();
        for part in spec.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    segs.pop()?;
                }
                p => segs.push(p),
            }
        }
        (!segs.is_empty()).then(|| segs.join("::"))
    }

    fn bare(from: NodeId, name: &str, line: u32) -> CallSite {
        CallSite {
            from,
            qualifier: CallQualifier::Bare(name.to_string()),
            line,
        }
    }

    fn cpp_stats(parses: Vec<FileParse>) -> OutOfLineStats {
        let (mut g, _, mut calls, mut refs) = merge_parses(repo(), parses);
        bind_out_of_line(&mut g, &mut calls, &mut refs).1
    }

    fn only_edge<'g>(
        g: &'g RepoGraph,
        from: NodeId,
        to: NodeId,
        category: glia_core::EdgeCategoryId,
    ) -> &'g Edge {
        let hits: Vec<&Edge> = g
            .edges
            .iter()
            .filter(|e| (e.from, e.to, e.category) == (from, to, category))
            .collect();
        assert_eq!(hits.len(), 1, "one {category:?} edge {from:?} -> {to:?}");
        hits[0]
    }

    fn rule_of(e: &Edge) -> (String, Option<String>) {
        let ev = glia_code_domain::evidence::Evidence::of(e).expect("EVIDENCE cell");
        (ev.emitter, ev.rule)
    }

    /// CB.6: `nav_facts` merge per scope in parse order, a fact two parses
    /// both recorded for one scope kept once, and a renamed scope's facts
    /// follow it onto the new id after that id's own.
    #[test]
    fn nav_facts_merge_per_scope_and_follow_a_rename() {
        let fact = |name: &str| NavFact::DeclaresFn {
            ns: String::new(),
            name: name.to_string(),
        };
        let (s, t) = (NodeId(11), NodeId(12));
        let mut a = FileParse::default();
        a.nav.record_fact(s, fact("a"));
        a.nav.record_fact(s, fact("b"));
        let mut b = FileParse::default();
        b.nav.record_fact(s, fact("b"));
        b.nav.record_fact(s, fact("c"));
        b.nav.record_fact(t, fact("a"));
        b.nav.record_fact(t, fact("d"));
        let (mut g, _, mut calls, mut refs) = merge_parses(repo(), vec![a, b]);
        assert_eq!(g.nav.nav_facts[&s], [fact("a"), fact("b"), fact("c")]);
        assert_eq!(g.nav.nav_facts[&t], [fact("a"), fact("d")]);

        rename_nodes(&mut g, &mut calls, &mut refs, &[(t, s)]);
        assert!(!g.nav.nav_facts.contains_key(&t), "the old scope's facts moved");
        assert_eq!(
            g.nav.nav_facts[&s],
            [fact("a"), fact("b"), fact("c"), fact("d")]
        );
    }

    /// CA.2a: `[go-types]` counts the Go parser's receiver-type facts over
    /// the merged nav of every file: result types, fn / METHOD scopes and
    /// their locals, and package vars under a file MODULE.
    #[test]
    fn go_types_marker_counts() {
        let mut vars = go_file(
            "app::vars",
            &[(node_kind::FUNCTION, "app::vars::New", None)],
        );
        let (module, new) = (
            gid(node_kind::MODULE, "app::vars"),
            gid(node_kind::FUNCTION, "app::vars::New"),
        );
        vars.nav.record_local_type(module, "svc", "New()");
        vars.nav.record_return_type(new, "Service");
        let mut h = go_file("app::h", &[(node_kind::FUNCTION, "app::h::Find", None)]);
        let find = gid(node_kind::FUNCTION, "app::h::Find");
        h.nav
            .record_return_type(find, "repositories.UserRepository");
        h.nav
            .record_local_type(find, "repo", "repositories.UserRepository");
        h.nav.record_local_type(find, "n", "");
        let g = build_go(repo(), vec![vars, h]).expect("build");
        assert_eq!(
            go_types_marker(&g.nav),
            "[go-types] return_types=2 local_scopes=1 locals=2 package_vars=1"
        );
        assert_eq!(
            go_types_marker(&CodeNav::default()),
            "[go-types] return_types=0 local_scopes=0 locals=0 package_vars=0"
        );
    }

    /// CA.3a: `[go-sigs]` counts the METHOD nodes of a Go build and those
    /// with a recorded signature: a generic receiver's method records none,
    /// and a method split from its struct into another file keeps its own.
    #[test]
    fn go_sigs_marker_counts() {
        let parse = |rel: &str, src: &str| {
            let qname = rel.trim_end_matches(".go").replace('/', "::");
            glia_parser_go::parse_file(src, rel, &qname, "example.com/shop", repo()).expect("parse")
        };
        let store = "package store\n\ntype Store struct{}\n\ntype Box[T any] struct{}\n\n\
                     func (s *Store) Get(key string) string { return key }\n\n\
                     func (b *Box[T]) Put(v T) {}\n";
        let g = build_go(repo(), vec![parse("store/store.go", store)]).expect("build");
        assert_eq!(
            go_sigs_marker(&g.nav).as_deref(),
            Some("[go-sigs] methods=2 with_signature=1")
        );
        let split = "package store\n\nfunc (s *Store) Del(key string) error { return nil }\n";
        let g = build_go(
            repo(),
            vec![parse("store/store.go", store), parse("store/del.go", split)],
        )
        .expect("build");
        assert_eq!(
            go_sigs_marker(&g.nav).as_deref(),
            Some("[go-sigs] methods=3 with_signature=2")
        );
        let del = gid(node_kind::METHOD, "store::del::Store::Del");
        assert_eq!(g.nav.method_sigs[&del], "(string)(error)");
        assert_eq!(go_sigs_marker(&CodeNav::default()), None);
    }

    /// CA.3a: `method_sigs` merge keeping the first parse's entry for a
    /// METHOD two parses share, and a renamed METHOD's signature follows it
    /// unless the surviving id already holds one.
    #[test]
    fn method_sigs_merge_first_and_follow_a_rename() {
        let (s, t, u) = (NodeId(21), NodeId(22), NodeId(23));
        let mut a = FileParse::default();
        a.nav.record_method_sig(s, "(string)(string)");
        let mut b = FileParse::default();
        b.nav.record_method_sig(s, "(int)(int)");
        b.nav.record_method_sig(t, "()(error)");
        b.nav.record_method_sig(u, "()()");
        let (mut g, _, mut calls, mut refs) = merge_parses(repo(), vec![a, b]);
        assert_eq!(g.nav.method_sigs[&s], "(string)(string)");
        rename_nodes(&mut g, &mut calls, &mut refs, &[(t, s), (u, NodeId(24))]);
        assert!(!g.nav.method_sigs.contains_key(&t) && !g.nav.method_sigs.contains_key(&u));
        assert_eq!(g.nav.method_sigs[&s], "(string)(string)", "the survivor keeps its own");
        assert_eq!(g.nav.method_sigs[&NodeId(24)], "()()");
    }

    /// No trace of `old` anywhere an id lives.
    fn assert_gone(g: &RepoGraph, old: NodeId) {
        assert!(g.nodes.iter().all(|n| n.id != old), "node record");
        assert!(
            g.edges.iter().all(|e| e.from != old && e.to != old),
            "edges"
        );
        let nav = &g.nav;
        assert!(!nav.name_by_id.contains_key(&old) && !nav.qname_by_id.contains_key(&old));
        assert!(!nav.kind_by_id.contains_key(&old) && !nav.parent_of.contains_key(&old));
        assert!(
            nav.parent_of.values().all(|p| *p != old),
            "parent_of values"
        );
        assert!(!nav.children_of.contains_key(&old), "children_of key");
        assert!(
            nav.children_of.values().flatten().all(|k| *k != old),
            "children_of entries"
        );
        assert!(!nav.local_types.contains_key(&old) && !g.properties.contains(&old));
        assert!(!nav.nav_facts.contains_key(&old), "nav_facts key");
        assert!(!nav.return_types.contains_key(&old), "return_types key");
        assert!(!nav.method_sigs.contains_key(&old), "method_sigs key");
        assert!(
            g.unresolved_calls.iter().all(|c| c.from != old),
            "unresolved calls"
        );
        assert!(
            g.unresolved_refs
                .iter()
                .all(|r| r.from != old && r.from_module != old)
        );
    }

    /// The fixtures/cpp-out-of-line-members Widget half: `Widget.h` declares
    /// `class Widget { void run(); int helper() {..} }`; `Widget.cpp` defines
    /// `static int file_helper()` and `void Widget::run() { this->helper();
    /// file_helper(); }`, a provisional METHOD under its MODULE.
    fn widget_shape() -> Vec<FileParse> {
        let h = cpp_file(
            "src::Widget.h",
            &[
                (node_kind::CLASS, "src::Widget", "Widget", None),
                (
                    node_kind::METHOD,
                    "src::Widget::helper",
                    "helper",
                    Some("src::Widget"),
                ),
            ],
        );
        let mut cpp = cpp_file(
            "src::Widget.cpp",
            &[
                (
                    node_kind::FUNCTION,
                    "src::Widget.cpp::file_helper",
                    "file_helper",
                    None,
                ),
                (node_kind::METHOD, "src::Widget::run", "Widget::run", None),
            ],
        );
        let run = gid(node_kind::METHOD, "src::Widget::run");
        cpp.imports = vec![include("src::Widget.cpp", "Widget.h")];
        cpp.calls = vec![
            CallSite {
                from: run,
                qualifier: CallQualifier::SelfMethod("helper".to_string()),
                line: 5,
            },
            bare(run, "file_helper", 6),
        ];
        vec![h, cpp]
    }

    /// The out-of-line half joins the header's CLASS: one METHOD, nav parent
    /// the class, a CLASS -> METHOD DEFINES next to the kept file DEFINES,
    /// `class_methods` instead of the file's `module_symbols`; `this->helper()`
    /// binds through the class and the file's static helper through the hook.
    #[test]
    fn out_of_line_member_joins_its_header_class() {
        let (class, helper, run) = (
            gid(node_kind::CLASS, "src::Widget"),
            gid(node_kind::METHOD, "src::Widget::helper"),
            gid(node_kind::METHOD, "src::Widget::run"),
        );
        let (cpp, file_helper) = (
            gid(node_kind::MODULE, "src::Widget.cpp"),
            gid(node_kind::FUNCTION, "src::Widget.cpp::file_helper"),
        );
        let stats = cpp_stats(widget_shape());
        assert_eq!(
            stats,
            OutOfLineStats {
                bound: 1,
                ..OutOfLineStats::default()
            }
        );
        assert_eq!(
            stats.marker().as_deref(),
            Some(
                "[cpp-members] out-of-line members bound: 1 (by_name=0 renamed=0 namespace_fns=0 ambiguous=0 unbound=0)"
            )
        );
        assert_eq!(OutOfLineStats::default().marker(), None);

        let g = build_c_cpp(repo(), widget_shape(), include_source).expect("build");
        assert_eq!(g.nav.parent_of[&run], class);
        assert_eq!(g.nav.name_by_id[&run], "run");
        assert_eq!(g.nav.qname_by_id[&run], "src::Widget::run");
        assert_eq!(g.nav.kind_by_id[&run], node_kind::METHOD);
        assert_eq!(g.nav.children_of[&class], vec![helper, run]);
        assert!(!g.nav.children_of[&cpp].contains(&run));
        let joined = only_edge(&g, class, run, edge_category::DEFINES);
        assert_eq!(
            rule_of(joined),
            (
                "graph:cpp_members".to_string(),
                Some("out_of_line".to_string())
            )
        );
        only_edge(&g, cpp, run, edge_category::DEFINES);
        assert_eq!(g.symbols.class_methods[&class].get("run"), Some(&run));
        let file_symbols = &g.symbols.module_symbols[&cpp];
        assert!(!file_symbols.contains_key("run") && !file_symbols.contains_key("Widget::run"));
        let this_call = only_edge(&g, run, helper, edge_category::CALLS);
        assert_eq!(
            rule_of(this_call),
            ("graph:calls".to_string(), Some("self_method".to_string()))
        );
        let own_file = only_edge(&g, run, file_helper, edge_category::CALLS);
        assert_eq!(
            rule_of(own_file),
            (
                "graph:cpp_members".to_string(),
                Some("defining_scope".to_string())
            )
        );
        assert!(g.unresolved_calls.is_empty(), "{:?}", g.unresolved_calls);
    }

    /// `void shop::init() {}` names a namespace: LB.10b's provisional METHOD
    /// `src::shop::init` becomes FUNCTION `src::cart.cpp::shop::init` named
    /// `init`, under its file, with its cells, DEFINES and call sites.
    #[test]
    fn namespace_qualified_definition_is_a_function() {
        let shape = || {
            let hpp = cpp_file(
                "include::shop::cart.hpp",
                &[(
                    node_kind::PACKAGE,
                    "include::shop::cart.hpp::shop",
                    "shop",
                    None,
                )],
            );
            let mut cpp = cpp_file(
                "src::cart.cpp",
                &[
                    (node_kind::FUNCTION, "src::cart.cpp::audit", "audit", None),
                    (node_kind::METHOD, "src::shop::init", "shop::init", None),
                ],
            );
            cpp.calls = vec![bare(gid(node_kind::METHOD, "src::shop::init"), "audit", 3)];
            vec![hpp, cpp]
        };
        assert_eq!(
            cpp_stats(shape()),
            OutOfLineStats {
                namespace_fns: 1,
                ..OutOfLineStats::default()
            }
        );
        let g = build_c_cpp(repo(), shape(), include_source).expect("build");
        let old = gid(node_kind::METHOD, "src::shop::init");
        let init = gid(node_kind::FUNCTION, "src::cart.cpp::shop::init");
        let (cpp, audit) = (
            gid(node_kind::MODULE, "src::cart.cpp"),
            gid(node_kind::FUNCTION, "src::cart.cpp::audit"),
        );
        assert_gone(&g, old);
        let node = g
            .nodes
            .iter()
            .find(|n| n.id == init)
            .expect("FUNCTION node");
        assert_eq!(
            node.cells,
            vec![code_cell("src::cart.cpp", "src::shop::init")]
        );
        assert_eq!(g.nav.kind_by_id[&init], node_kind::FUNCTION);
        assert_eq!(g.nav.name_by_id[&init], "init");
        assert_eq!(g.nav.qname_by_id[&init], "src::cart.cpp::shop::init");
        assert_eq!(g.nav.parent_of[&init], cpp);
        assert_eq!(g.nav.children_of[&cpp], vec![audit, init]);
        only_edge(&g, cpp, init, edge_category::DEFINES);
        only_edge(&g, init, audit, edge_category::CALLS);
        assert!(g.nav.kind_by_id.values().all(|k| *k != node_kind::METHOD));
    }

    /// A global class of another directory (`include/Widget.h`) binds, and
    /// the provisional `src::Widget::run` moves to `include::Widget::run`
    /// everywhere its id lived: node, edges, pending calls and refs (`from`,
    /// `from_module`), properties, local types.
    #[test]
    fn cross_directory_class_is_bound_and_renamed() {
        let old = gid(node_kind::METHOD, "src::Widget::run");
        let cpp = gid(node_kind::MODULE, "src::w.cpp");
        let shape = || {
            let h = cpp_file(
                "include::Widget.h",
                &[(node_kind::CLASS, "include::Widget", "Widget", None)],
            );
            let mut w = cpp_file(
                "src::w.cpp",
                &[(node_kind::METHOD, "src::Widget::run", "Widget::run", None)],
            );
            w.calls = vec![bare(old, "nowhere", 2)];
            let uses = |from, from_module| UnresolvedRef {
                from,
                from_module,
                qualifier: CallQualifier::Bare("Missing".to_string()),
                category: edge_category::USES,
                line: 1,
            };
            w.refs = vec![uses(old, cpp), uses(cpp, old)];
            w.properties.insert(old);
            w.nav.record_local_type(old, "w", "Widget");
            w.nav.record_return_type(old, "Widget");
            w.nav.record_method_sig(old, "()()");
            vec![h, w]
        };
        assert_eq!(
            cpp_stats(shape()),
            OutOfLineStats {
                bound: 1,
                renamed: 1,
                ..OutOfLineStats::default()
            }
        );
        let g = build_c_cpp(repo(), shape(), include_source).expect("build");
        let (class, run) = (
            gid(node_kind::CLASS, "include::Widget"),
            gid(node_kind::METHOD, "include::Widget::run"),
        );
        assert_gone(&g, old);
        assert_eq!(g.nodes.iter().filter(|n| n.id == run).count(), 1);
        assert_eq!(g.nav.qname_by_id[&run], "include::Widget::run");
        assert_eq!(
            (g.nav.name_by_id[&run].as_str(), g.nav.parent_of[&run]),
            ("run", class)
        );
        assert_eq!(g.nav.children_of[&class], vec![run]);
        assert!(
            !g.nav.children_of.contains_key(&cpp),
            "the file's only child left"
        );
        only_edge(&g, class, run, edge_category::DEFINES);
        only_edge(&g, cpp, run, edge_category::DEFINES);
        assert_eq!(
            g.unresolved_calls
                .iter()
                .map(|c| c.from)
                .collect::<Vec<_>>(),
            vec![run]
        );
        let refs: Vec<(NodeId, NodeId)> = g
            .unresolved_refs
            .iter()
            .map(|r| (r.from, r.from_module))
            .collect();
        assert_eq!(refs, vec![(run, cpp), (cpp, run)]);
        assert!(g.properties.contains(&run));
        assert_eq!(g.nav.local_types[&run]["w"], "Widget");
        assert_eq!(g.nav.return_types[&run], "Widget");
        assert_eq!(g.nav.method_sigs[&run], "()()");
    }

    /// Two global `Widget` classes (a/, b/) and a definition in c/: neither
    /// directory decides, so HEAD's FUNCTION stays; a definition in a/ binds
    /// a/'s class.
    #[test]
    fn ambiguous_global_class_stays_a_function() {
        let shape = |dir: &str| {
            let (file, member) = (format!("{dir}::w.cpp"), format!("{dir}::Widget::run"));
            vec![
                cpp_file(
                    "a::Widget.h",
                    &[(node_kind::CLASS, "a::Widget", "Widget", None)],
                ),
                cpp_file(
                    "b::Widget.h",
                    &[(node_kind::CLASS, "b::Widget", "Widget", None)],
                ),
                cpp_file(&file, &[(node_kind::METHOD, &member, "Widget::run", None)]),
            ]
        };
        assert_eq!(
            cpp_stats(shape("c")),
            OutOfLineStats {
                ambiguous: 1,
                ..OutOfLineStats::default()
            }
        );
        let g = build_c_cpp(repo(), shape("c"), include_source).expect("build");
        let f = gid(node_kind::FUNCTION, "c::w.cpp::Widget::run");
        assert_gone(&g, gid(node_kind::METHOD, "c::Widget::run"));
        assert_eq!(g.nav.name_by_id[&f], "Widget::run");
        assert_eq!(g.nav.parent_of[&f], gid(node_kind::MODULE, "c::w.cpp"));
        assert!(g.nav.kind_by_id.values().all(|k| *k != node_kind::METHOD));

        assert_eq!(
            cpp_stats(shape("a")),
            OutOfLineStats {
                bound: 1,
                ..OutOfLineStats::default()
            }
        );
        let g = build_c_cpp(repo(), shape("a"), include_source).expect("build");
        let run = gid(node_kind::METHOD, "a::Widget::run");
        assert_eq!(g.nav.parent_of[&run], gid(node_kind::CLASS, "a::Widget"));
    }

    /// A class a source file declares (`src::a.cpp::Helper`, LB.10b's
    /// translation-unit shape) is never a target of another file's
    /// definition: `Helper::go` in b.cpp stays HEAD's FUNCTION.
    #[test]
    fn tu_local_class_of_another_file_is_never_a_target() {
        let shape = || {
            vec![
                cpp_file(
                    "src::a.cpp",
                    &[(node_kind::CLASS, "src::a.cpp::Helper", "Helper", None)],
                ),
                cpp_file(
                    "src::b.cpp",
                    &[(node_kind::METHOD, "src::Helper::go", "Helper::go", None)],
                ),
            ]
        };
        assert_eq!(
            cpp_stats(shape()),
            OutOfLineStats {
                unbound: 1,
                ..OutOfLineStats::default()
            }
        );
        let g = build_c_cpp(repo(), shape(), include_source).expect("build");
        let f = gid(node_kind::FUNCTION, "src::b.cpp::Helper::go");
        assert_gone(&g, gid(node_kind::METHOD, "src::Helper::go"));
        assert_eq!(g.nav.name_by_id[&f], "Helper::go");
        assert_eq!(g.nav.parent_of[&f], gid(node_kind::MODULE, "src::b.cpp"));
        assert!(
            !g.nav
                .children_of
                .contains_key(&gid(node_kind::CLASS, "src::a.cpp::Helper"))
        );
    }

    /// Renamed onto an id a header inline METHOD already holds: one node
    /// carrying both cell lists (the existing first), the header's nav record,
    /// no second CLASS -> METHOD DEFINES.
    #[test]
    fn rename_into_an_existing_id_merges_cells() {
        let shape = || {
            vec![
                cpp_file(
                    "include::Widget.h",
                    &[
                        (node_kind::CLASS, "include::Widget", "Widget", None),
                        (
                            node_kind::METHOD,
                            "include::Widget::run",
                            "run",
                            Some("include::Widget"),
                        ),
                    ],
                ),
                cpp_file(
                    "src::w.cpp",
                    &[(node_kind::METHOD, "src::Widget::run", "Widget::run", None)],
                ),
            ]
        };
        assert_eq!(
            cpp_stats(shape()),
            OutOfLineStats {
                bound: 1,
                renamed: 1,
                ..OutOfLineStats::default()
            }
        );
        let g = build_c_cpp(repo(), shape(), include_source).expect("build");
        let (class, run) = (
            gid(node_kind::CLASS, "include::Widget"),
            gid(node_kind::METHOD, "include::Widget::run"),
        );
        assert_gone(&g, gid(node_kind::METHOD, "src::Widget::run"));
        let merged: Vec<&Node> = g.nodes.iter().filter(|n| n.id == run).collect();
        assert_eq!(merged.len(), 1);
        assert_eq!(
            merged[0].cells,
            vec![
                code_cell("include::Widget.h", "include::Widget::run"),
                code_cell("src::w.cpp", "src::Widget::run")
            ]
        );
        assert_eq!(
            (g.nav.name_by_id[&run].as_str(), g.nav.parent_of[&run]),
            ("run", class)
        );
        assert_eq!(g.nav.children_of[&class], vec![run]);
        // The header parse's own DEFINES (no graph evidence) is the only one.
        let header_defines = only_edge(&g, class, run, edge_category::DEFINES);
        assert!(glia_code_domain::evidence::Evidence::of(header_defines).is_none());
        only_edge(
            &g,
            gid(node_kind::MODULE, "src::w.cpp"),
            run,
            edge_category::DEFINES,
        );
    }

    /// LB.10b merges an out-of-line member into a same-qname inline METHOD of
    /// the header class (the RunLoop `= delete` shape), and whichever parse
    /// merges last wins its nav record. Either order joins the class the
    /// same way, and the member still sees its file's static helper.
    #[test]
    fn folded_member_joins_its_class_in_either_parse_order() {
        let (class, member) = (
            gid(node_kind::CLASS, "src::RunLoop"),
            gid(node_kind::METHOD, "src::RunLoop::RunLoop"),
        );
        let (cpp, tick) = (
            gid(node_kind::MODULE, "src::loop.cpp"),
            gid(node_kind::FUNCTION, "src::loop.cpp::tick"),
        );
        let header = || {
            cpp_file(
                "src::loop.h",
                &[
                    (node_kind::CLASS, "src::RunLoop", "RunLoop", None),
                    (
                        node_kind::METHOD,
                        "src::RunLoop::RunLoop",
                        "RunLoop",
                        Some("src::RunLoop"),
                    ),
                ],
            )
        };
        let source = || {
            let mut f = cpp_file(
                "src::loop.cpp",
                &[
                    (node_kind::FUNCTION, "src::loop.cpp::tick", "tick", None),
                    (
                        node_kind::METHOD,
                        "src::RunLoop::RunLoop",
                        "RunLoop::RunLoop",
                        None,
                    ),
                ],
            );
            f.calls = vec![bare(member, "tick", 4)];
            f
        };
        let mut navs = Vec::new();
        for parses in [vec![header(), source()], vec![source(), header()]] {
            let (mut g, _, mut calls, mut refs) = merge_parses(repo(), parses.clone());
            let (defining, stats) = bind_out_of_line(&mut g, &mut calls, &mut refs);
            assert_eq!(
                stats,
                OutOfLineStats {
                    bound: 1,
                    ..OutOfLineStats::default()
                }
            );
            assert_eq!(defining.get(&member), Some(&cpp));
            let g = build_c_cpp(repo(), parses, include_source).expect("build");
            assert_eq!(g.nav.children_of[&class], vec![member]);
            assert_eq!(g.nav.children_of[&cpp], vec![tick]);
            only_edge(&g, class, member, edge_category::DEFINES);
            only_edge(&g, cpp, member, edge_category::DEFINES);
            only_edge(&g, member, tick, edge_category::CALLS);
            assert!(!g.symbols.module_symbols[&cpp].contains_key("RunLoop"));
            navs.push((g.nav.name_by_id[&member].clone(), g.nav.parent_of[&member]));
        }
        assert_eq!(navs, vec![("RunLoop".to_string(), class); 2]);
    }

    /// The include hook (LB.10a's stopgap, moved into `build_c_cpp`): a Bare
    /// call binds the one directly included file's top-level symbol, two
    /// includes answering is refused, a name no include answers stays
    /// unresolved, and a joined member looks in ITS file's includes, not its
    /// header's.
    #[test]
    fn bare_call_binds_through_a_direct_include() {
        let (f, sq) = (
            gid(node_kind::FUNCTION, "src::a.cpp::f"),
            gid(node_kind::FUNCTION, "src::a.h::sq"),
        );
        let (go, twice) = (
            gid(node_kind::METHOD, "src::W::go"),
            gid(node_kind::FUNCTION, "src::util.h::twice"),
        );
        let a_h = cpp_file(
            "src::a.h",
            &[
                (node_kind::FUNCTION, "src::a.h::sq", "sq", None),
                (node_kind::FUNCTION, "src::a.h::dup", "dup", None),
            ],
        );
        let b_h = cpp_file(
            "src::b.h",
            &[(node_kind::FUNCTION, "src::b.h::dup", "dup", None)],
        );
        let mut a_cpp = cpp_file(
            "src::a.cpp",
            &[(node_kind::FUNCTION, "src::a.cpp::f", "f", None)],
        );
        a_cpp.imports = vec![include("src::a.cpp", "a.h"), include("src::a.cpp", "b.h")];
        a_cpp.calls = vec![bare(f, "sq", 2), bare(f, "dup", 2), bare(f, "nowhere", 2)];
        let w_h = cpp_file("src::w.h", &[(node_kind::CLASS, "src::W", "W", None)]);
        let util_h = cpp_file(
            "src::util.h",
            &[(node_kind::FUNCTION, "src::util.h::twice", "twice", None)],
        );
        let mut w_cpp = cpp_file(
            "src::w.cpp",
            &[(node_kind::METHOD, "src::W::go", "W::go", None)],
        );
        w_cpp.imports = vec![
            include("src::w.cpp", "w.h"),
            include("src::w.cpp", "util.h"),
        ];
        w_cpp.calls = vec![bare(go, "twice", 7)];
        let parses = vec![a_h, b_h, a_cpp, w_h, util_h, w_cpp];

        let (mut g, imports, mut calls, mut refs) = merge_parses(repo(), parses);
        let (defining, _) = bind_out_of_line(&mut g, &mut calls, &mut refs);
        build_symbol_table(&mut g);
        resolve_imports_ts(&mut g, &imports, &include_source, &mut SameStem::default());
        let scope = CppCallScope::new(&g, defining);
        let mut tally = EvidenceTally::default();
        resolve_calls(&mut g, &calls, |g, site| scope.resolve(g, site), &mut tally);
        assert_eq!(
            scope.marker().as_deref(),
            Some("[c-includes] bare calls bound through a direct #include: 2 (ambiguous=1)")
        );
        assert_eq!(g.nav.parent_of[&go], gid(node_kind::CLASS, "src::W"));
        for (from, to, line) in [(f, sq, 2), (go, twice, 7)] {
            let e = only_edge(&g, from, to, edge_category::CALLS);
            let ev = glia_code_domain::evidence::Evidence::of(e).expect("evidence");
            assert_eq!(
                (ev.emitter.as_str(), ev.rule.as_deref(), ev.line),
                ("graph:c_includes", Some("include"), Some(line))
            );
        }
        let calls: Vec<&Edge> = g
            .edges
            .iter()
            .filter(|e| e.category == edge_category::CALLS)
            .collect();
        assert_eq!(calls.len(), 2);
        assert_eq!(
            g.unresolved_calls.len(),
            2,
            "dup (two includes) and nowhere"
        );
    }

    /// The join is a pure function of the parses: two builds give identical
    /// node and edge vectors (cells and order included).
    #[test]
    fn c_cpp_build_is_deterministic() {
        let shape = || {
            let mut parses = widget_shape();
            parses.push(cpp_file(
                "include::Gadget.h",
                &[(node_kind::STRUCT, "include::Gadget", "Gadget", None)],
            ));
            parses.push(cpp_file(
                "src::gadget.cpp",
                &[
                    (node_kind::METHOD, "src::Gadget::spin", "Gadget::spin", None),
                    (node_kind::METHOD, "src::Gone::x", "Gone::x", None),
                ],
            ));
            parses
        };
        let (a, b) = (
            build_c_cpp(repo(), shape(), include_source).expect("a"),
            build_c_cpp(repo(), shape(), include_source).expect("b"),
        );
        assert_eq!(a.nodes, b.nodes);
        assert_eq!(a.edges, b.edges);
        assert!(
            a.nodes
                .iter()
                .any(|n| n.id == gid(node_kind::METHOD, "include::Gadget::spin"))
        );
        assert!(
            a.nodes
                .iter()
                .any(|n| n.id == gid(node_kind::FUNCTION, "src::gadget.cpp::Gone::x"))
        );
    }

    // ---- CB.15: a namespace PACKAGE several files open ------------------------

    /// One file shaped the way the PHP / C# parsers emit a block namespace
    /// (`namespace App\Orders { }`, `namespace Shop.Orders { }`): MODULE
    /// `module`, PACKAGE `package` under it, then [`go_file`]'s items with a
    /// `None` parent meaning the PACKAGE; `(namespace, Name)` `use` imports
    /// of the file and `(from, Base, m)` calls `Base::m()`.
    fn namespaced_file(
        module: &str,
        package: &str,
        items: &[(NodeKindId, &str, Option<&str>)],
        uses: &[(&str, &str)],
        calls: &[(&str, &str, &str)],
    ) -> FileParse {
        let mut all = vec![(node_kind::PACKAGE, package, None)];
        all.extend(items.iter().map(|&(k, q, p)| (k, q, Some(p.unwrap_or(package)))));
        let mut fp = go_file(module, &all);
        fp.imports = uses
            .iter()
            .map(|&(ns, name)| ImportStmt {
                from_module: module.to_string(),
                target: ImportTarget::Symbol {
                    module: ns.to_string(),
                    name: name.to_string(),
                    alias: None,
                    level: 0,
                },
                line: 0,
            })
            .collect();
        fp.calls = calls
            .iter()
            .map(|&(from, base, name)| CallSite {
                from: gid(node_kind::METHOD, from),
                qualifier: CallQualifier::Attribute { base: base.to_string(), name: name.to_string() },
                line: 0,
            })
            .collect();
        fp
    }

    /// A class `<package>::<class>` with one METHOD `m`, as `namespaced_file` items.
    fn class_with<'a>(class: &'a str, method: &'a str) -> [(NodeKindId, &'a str, Option<&'a str>); 2] {
        [(node_kind::CLASS, class, None), (node_kind::METHOD, method, Some(class))]
    }

    /// bench/substrate-gap/fixtures/php-shared-namespace-bindings: two files
    /// open `App\Billing` (Invoicer, Ledger) and two open `App\Orders`, each
    /// importing a different billing class.
    fn php_shared_namespace_shape() -> Vec<FileParse> {
        vec![
            namespaced_file(
                "src::Billing::Invoicer",
                "App::Billing",
                &class_with("App::Billing::Invoicer", "App::Billing::Invoicer::issue"),
                &[],
                &[],
            ),
            namespaced_file(
                "src::Billing::Ledger",
                "App::Billing",
                &class_with("App::Billing::Ledger", "App::Billing::Ledger::issue"),
                &[],
                &[],
            ),
            namespaced_file(
                "src::Orders::OrderService",
                "App::Orders",
                &class_with("App::Orders::OrderService", "App::Orders::OrderService::place"),
                &[("App::Billing", "Invoicer")],
                &[("App::Orders::OrderService::place", "Invoicer", "issue")],
            ),
            namespaced_file(
                "src::Orders::ShipmentService",
                "App::Orders",
                &class_with("App::Orders::ShipmentService", "App::Orders::ShipmentService::ship"),
                &[("App::Billing", "Ledger")],
                &[("App::Orders::ShipmentService::ship", "Ledger", "issue")],
            ),
        ]
    }

    /// Each member of a namespace two files open resolves its calls through
    /// its OWN file's `use`: `OrderService::place` binds `Invoicer::issue`
    /// (HEAD read ShipmentService.php's bindings, the last file merged, and
    /// bound nothing), and never the other file's `Ledger::issue`.
    #[test]
    fn shared_namespace_member_uses_its_own_file_bindings() {
        let g = build_dotted(repo(), php_shared_namespace_shape()).unwrap();
        let (place, ship) = (
            gid(node_kind::METHOD, "App::Orders::OrderService::place"),
            gid(node_kind::METHOD, "App::Orders::ShipmentService::ship"),
        );
        let (invoicer_issue, ledger_issue) = (
            gid(node_kind::METHOD, "App::Billing::Invoicer::issue"),
            gid(node_kind::METHOD, "App::Billing::Ledger::issue"),
        );
        assert!(has_edge(&g, place, invoicer_issue, edge_category::CALLS), "own file's use");
        assert!(has_edge(&g, ship, ledger_issue, edge_category::CALLS), "control");
        assert!(!has_edge(&g, place, ledger_issue, edge_category::CALLS), "another file's use");
        assert!(g.unresolved_calls.is_empty());
        let order_file = gid(node_kind::MODULE, "src::Orders::OrderService");
        assert_eq!(g.symbols.home_module.get(&place), Some(&order_file));
        assert_eq!(
            g.symbols.home_module.get(&gid(node_kind::CLASS, "App::Orders::OrderService")),
            Some(&order_file)
        );
        assert_eq!(crate::calls::enclosing_home_module(&g, place), Some(order_file));
    }

    /// A PACKAGE two files open hangs under the FIRST file merged (the one
    /// its first POSITION names; HEAD: the last), and both files still list
    /// it as a nav child. Merge order decides, so the reverse order flips it.
    #[test]
    fn shared_package_parent_is_the_first_file() {
        let orders = gid(node_kind::PACKAGE, "App::Orders");
        let (first, second) = (
            gid(node_kind::MODULE, "src::Orders::OrderService"),
            gid(node_kind::MODULE, "src::Orders::ShipmentService"),
        );
        let (g, _, _, _) = merge_parses(repo(), php_shared_namespace_shape());
        assert_eq!(g.nav.parent_of[&orders], first);
        assert_eq!(
            g.nav.parent_of[&gid(node_kind::PACKAGE, "App::Billing")],
            gid(node_kind::MODULE, "src::Billing::Invoicer")
        );
        for file in [first, second] {
            assert!(g.nav.children_of[&file].contains(&orders), "every file lists it");
        }
        let mut reversed = php_shared_namespace_shape();
        reversed.reverse();
        let (g, _, _, _) = merge_parses(repo(), reversed);
        assert_eq!(g.nav.parent_of[&orders], second);
    }

    /// A C# block namespace only one file opens: no home module is recorded,
    /// every node's call scope is `enclosing_module`'s, the nav is the
    /// parse's own, and the `using`-bound call resolves as on HEAD.
    #[test]
    fn single_file_namespace_unchanged() {
        let parses = || {
            vec![
                namespaced_file(
                    "Billing::Invoicer",
                    "Shop::Billing",
                    &class_with("Shop::Billing::Invoicer", "Shop::Billing::Invoicer::Issue"),
                    &[],
                    &[],
                ),
                namespaced_file(
                    "Orders::OrderService",
                    "Shop::Orders",
                    &class_with("Shop::Orders::OrderService", "Shop::Orders::OrderService::Place"),
                    &[("Shop::Billing", "Invoicer")],
                    &[("Shop::Orders::OrderService::Place", "Invoicer", "Issue")],
                ),
            ]
        };
        let g = build_dotted(repo(), parses()).unwrap();
        assert!(g.symbols.home_module.is_empty());
        for id in g.nav.kind_by_id.keys() {
            assert_eq!(
                crate::calls::enclosing_home_module(&g, *id),
                enclosing_module(&g.nav, *id)
            );
        }
        let mut parse_nav: HashMap<NodeId, NodeId> = HashMap::new();
        for p in parses() {
            parse_nav.extend(p.nav.parent_of);
        }
        assert_eq!(g.nav.parent_of, parse_nav);
        assert!(has_edge(
            &g,
            gid(node_kind::METHOD, "Shop::Orders::OrderService::Place"),
            gid(node_kind::METHOD, "Shop::Billing::Invoicer::Issue"),
            edge_category::CALLS
        ));
    }

    /// Only a member under a SHARED package gets an entry: a Python-shaped
    /// class directly under its MODULE and a class under a per-file PACKAGE
    /// (a Ruby `module`, a C++ `<file>::ns`) record nothing, even in a build
    /// where another namespace is shared; the PACKAGEs and MODULEs themselves
    /// never do.
    #[test]
    fn home_module_skips_non_package_parents() {
        let mut parses = php_shared_namespace_shape();
        parses.push(go_file(
            "app::models",
            &[
                (node_kind::CLASS, "app::models::User", None),
                (node_kind::METHOD, "app::models::User::save", Some("app::models::User")),
            ],
        ));
        parses.push(namespaced_file(
            "app::shop",
            "app::shop::Shop",
            &class_with("app::shop::Shop::Cart", "app::shop::Shop::Cart::total"),
            &[],
            &[],
        ));
        let (g, _, _, _) = merge_parses(repo(), parses);
        for q in [
            (node_kind::CLASS, "app::models::User"),
            (node_kind::METHOD, "app::models::User::save"),
            (node_kind::CLASS, "app::shop::Shop::Cart"),
            (node_kind::METHOD, "app::shop::Shop::Cart::total"),
            (node_kind::PACKAGE, "App::Orders"),
            (node_kind::MODULE, "src::Orders::OrderService"),
        ] {
            assert!(!g.symbols.home_module.contains_key(&gid(q.0, q.1)), "{q:?}");
        }
        // The eight members of the two shared namespaces: 4 classes, 4 methods.
        assert_eq!(g.symbols.home_module.len(), 8);
    }

    /// A C# partial class two files declare is one CLASS id (its entry: the
    /// first file), and a METHOD the second file declares still resolves
    /// through the second file's `using` - its own entry is found before the
    /// walk reaches the class.
    #[test]
    fn partial_class_member_keeps_its_own_file() {
        let class = "Shop::Orders::OrderService";
        let parses = vec![
            namespaced_file(
                "Billing::Invoicer",
                "Shop::Billing",
                &class_with("Shop::Billing::Invoicer", "Shop::Billing::Invoicer::Issue"),
                &[],
                &[],
            ),
            namespaced_file(
                "Billing::Ledger",
                "Shop::Billing",
                &class_with("Shop::Billing::Ledger", "Shop::Billing::Ledger::Issue"),
                &[],
                &[],
            ),
            namespaced_file(
                "Orders::OrderService",
                "Shop::Orders",
                &class_with(class, "Shop::Orders::OrderService::Place"),
                &[("Shop::Billing", "Invoicer")],
                &[("Shop::Orders::OrderService::Place", "Invoicer", "Issue")],
            ),
            namespaced_file(
                "Orders::OrderService.Shipping",
                "Shop::Orders",
                &class_with(class, "Shop::Orders::OrderService::Ship"),
                &[("Shop::Billing", "Ledger")],
                &[("Shop::Orders::OrderService::Ship", "Ledger", "Issue")],
            ),
        ];
        let g = build_dotted(repo(), parses).unwrap();
        let (place, ship) = (
            gid(node_kind::METHOD, "Shop::Orders::OrderService::Place"),
            gid(node_kind::METHOD, "Shop::Orders::OrderService::Ship"),
        );
        let (first, second) = (
            gid(node_kind::MODULE, "Orders::OrderService"),
            gid(node_kind::MODULE, "Orders::OrderService.Shipping"),
        );
        assert_eq!(g.symbols.home_module.get(&gid(node_kind::CLASS, class)), Some(&first));
        assert_eq!(g.symbols.home_module.get(&ship), Some(&second));
        assert!(has_edge(
            &g,
            place,
            gid(node_kind::METHOD, "Shop::Billing::Invoicer::Issue"),
            edge_category::CALLS
        ));
        assert!(has_edge(
            &g,
            ship,
            gid(node_kind::METHOD, "Shop::Billing::Ledger::Issue"),
            edge_category::CALLS
        ));
        assert!(g.unresolved_calls.is_empty());
    }
}
