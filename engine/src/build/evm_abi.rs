//! DT.10g: the cross-file ABI join, a post-cache graft of `apply_post_cache`.
//!
//! The EVM client extractor (DT.10c-k) reads a contract call's ABI only when
//! the calling file declares it. A client whose ABI binding another file
//! declares carries `Scope::Abi(<name>)` with no entries and, in its
//! `via:evm` ENDPOINT_HIT cell, the reference `abi_ref {name, from}`
//! (`glia_code_domain::evm::AbiRef`): a TS value import's exported name and
//! specifier, or `from = "."` for a Go constant of another file of the
//! package. This graft fills such a cell's `abi` with the declaring file's
//! entries and its `abi_from` with that file, so `resolver:evm` identifies the
//! call by ABI (rule `abi`, DERIVED) and drops the members the ABI does not
//! list (`not_in_abi`), instead of a name hint or nothing.
//!
//! It is repo-level and post-cache: an extractor sees one file, and
//! `merge_nav` drops an extractor's own nav facts, so the declarations ride on
//! the cached parses' `fp.nav` (engine/src/extract.rs) and the join reads them
//! here, on every build, cached parses included (the cache keeps the unjoined
//! cell). An edit to `abi.ts` alone therefore re-joins a cache-served
//! `pay.ts`, and `glia watch` re-runs it with the other post-cache grafts.
//!
//! 1. INDEX, over every parse of the repo; a parse's file is
//!    `http_owner::module_file` (the owner pass's reader):
//!    - TS: each MODULE's `NavFact::EvmAbi { name, entries }`, keyed
//!      (declaring file, name); a key declared twice is dropped;
//!    - Go: each `.go` parse's `NavFact::GoStringConst { name, value }` whose
//!      value reads as a JSON ABI through
//!      `glia_code_extractors::evm::read_go_const_abi` (as written, else
//!      unescaped per Go; only a value starting with `[` is tried), keyed
//!      (declaring file, name). A `_test.go` file's constants serve only
//!      `_test.go` callers.
//! 2. JOIN, for each RPC_CALL / EVENT_HANDLER with an `evm_call:` /
//!    `evm_event:` qname and each of its evm cells that has `abi_ref` and no
//!    `abi`:
//!    - TS (a relative `from`: `.`, `..`, `./..`, `../..`): resolved against
//!      the calling file's directory, `..` normalised (one above the repo
//!      root resolves nothing), then the candidates in order: the path as
//!      written, `.ts`, `.tsx`, `.js`, `.jsx`, `/index.ts`, `/index.js`, and
//!      for a written `.js` / `.jsx` the `.ts` / `.tsx` beside it (an ESM
//!      `./abi.js` naming `abi.ts`). The first candidate whose (path, name)
//!      is indexed wins.
//!    - TS, a bare specifier (GD.20): mapped through the repo's tsconfig
//!      `paths`, bundler aliases and workspace packages exactly as the TS
//!      graph build maps an import (`lang_build::resolve_ts_source_aliased`
//!      over the [`TsAliasSet`] `apply_post_cache` holds), then tried as
//!      above; a package nothing maps stays `unresolved`.
//!    - TS barrels (GD.20): a candidate file that declares no such ABI but
//!      re-exports (DT.1q's `NavFact::ReExportName` / `ReExportStar`, both on
//!      the MODULE) is followed as ECMA-262 ResolveExport reads it: a named
//!      re-export `export { <n> as <name> } from '<s>'` resolves `<n>` in
//!      `<s>`; otherwise, unless the barrel's own `export` declarations
//!      (`NavFact::Exports`) name it or it is `default`, every `export * from
//!      '<s>'` is tried and exactly one distinct declaration joins (two are
//!      `ambiguous`). A re-export source maps like an import (relative, then
//!      aliases); a (file, name) pair seen once ends a cycle and depth
//!      [`MAX_BARREL_DEPTH`] ends a chain. A from-less local export list
//!      (`const X = [..]; export { X }`, DT.1q's `ExportLocal`) is declared
//!      by the extractor itself (`exported_abis`), so it is an ordinary
//!      `EvmAbi` here.
//!    - Go (a `.go` caller): every `.go` file of the calling file's directory
//!      (tests only for a test caller). Exactly one (file, name): joined; two
//!      or more: `ambiguous`; none: `unresolved`.
//!
//!    A join rewrites the cell in place (`evm::client_hit_cell`): `abi` the
//!    entries, `abi_from` the declaring file. The node, its qname and its edges
//!    are unchanged. A cell's result depends only on the index, which is
//!    BTreeMaps built order-free, so the HashMap order of `parses_by_lang`
//!    reaches no output.
//! 3. MARKER, once per repo with a declaration or a referencing cell:
//!    `[evm-abi-join] joined=<j> unresolved=<u> ambiguous=<a> declarations=<d>
//!    via_barrel=<b> via_alias=<l> repo=<label>`; j / u / a count cells, d the
//!    indexed declarations, b / l the joined cells reached through a barrel /
//!    through an alias (GD.20).
//!
//! 4. INSTANCES (GD.31), after the cell join. A TS-family MODULE's
//!    `NavFact::EvmContract` facts (an exported contract instance: `export
//!    const escrow = Escrow__factory.connect(..)`, `new Contract(..)`,
//!    `getContract({..})`) are indexed like the ABIs, keyed (declaring file,
//!    exported name), a key declared twice dropped. Each importing file's
//!    `NavFact::EvmImportSite` facts are grouped by import (local, name,
//!    specifier) and the import resolved exactly as an ABI reference (step
//!    2: relative, aliases and workspace packages, barrels). An import that
//!    names no instance is an ordinary import and mints nothing; one that
//!    names an instance mints its accesses read in the instance's API (a
//!    `<local>.<m>(..)` call, a `.on(<local>.filters.<E>(), ..)` / `.on('<E>',
//!    ..)` subscription, the viem / web3 forms) as the in-file reader mints a
//!    site: RPC_CALL `evm_call:<scope>.<m>` / EVENT_HANDLER
//!    `evm_event:<scope>.<E>`, Confidence::Medium, nav parent the MODULE, one
//!    `via:evm` ENDPOINT_HIT per (lib, abi) with the sites' argument counts,
//!    then `anchor::attach` (the A5.8 owner edges and first-site POSITION) and
//!    the router's raw IMPORTS cell. The cell's ABI: the entries the exporting
//!    file knows, `abi_from` that file; else the entries its own reference
//!    joins to from the exporting file, `abi_from` the ABI's file; else none,
//!    `abi_from` the exporting file. With entries known, a member they do not
//!    list mints nothing. A local the importer declares or assigns, and an
//!    import two star re-exports give, count `unresolved`. A node the file
//!    already holds (it reads the same client itself) gains the new cell
//!    variants. Sites go in (line, local, name, specifier, ..) order, nodes in
//!    (side, scope, member) order, so the parse's node order is fixed.
//!    Marker, once per repo with an exported instance: `[evm-instances]
//!    exported=<e> importers=<i> sites=<s> events=<v> unresolved=<u>
//!    repo=<label>`; e the indexed instances, i the files that gained a site,
//!    s / v the call / event sites minted.
//!
//! It runs above the endpoint fold (nav facts are still present) and before
//! every stamp that adds a key to a client hit (CB.24's `hosts`), so the cell
//! it rewrites holds only the extractor's fields.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use glia_code_domain::evm;
use glia_code_domain::{FileParse, GRAPH_TYPE, NavFact, attach_imports_cell, evidence, node_kind};
use glia_code_extractors::anchor::{self, Anchor};
use glia_code_extractors::evm::read_go_const_abi;
use glia_core::{Cell, Confidence, Node, NodeId};

use super::lang_build::{TsAliasSet, resolve_ts_source_aliased};
use crate::extract::detect_language;
use crate::http_owner;

/// The longest re-export chain a TS join follows (module doc, step 2; the
/// depth DT.1q's barrel resolution caps at too).
const MAX_BARREL_DEPTH: usize = 32;

/// The export a star re-export never passes on.
const DEFAULT_EXPORT: &str = "default";

/// The extensions and index files a relative TS specifier is tried with, in
/// order, after the path as written.
const TS_CANDIDATES: &[&str] = &[".ts", ".tsx", ".js", ".jsx", "/index.ts", "/index.js"];

/// The per-repo tallies of the join (module doc, step 3).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct EvmAbiJoinStats {
    /// Cells whose ABI was filled.
    pub(super) joined: usize,
    /// Cells whose reference names no indexed declaration (a bare or alias
    /// TS specifier, a file or constant that holds no ABI).
    pub(super) unresolved: usize,
    /// Go cells whose constant two or more files of the package declare.
    pub(super) ambiguous: usize,
    /// Declarations indexed.
    pub(super) declarations: usize,
    /// Joined cells reached through a TS re-export barrel (GD.20).
    pub(super) via_barrel: usize,
    /// Joined cells whose specifier an alias mapped (GD.20).
    pub(super) via_alias: usize,
    /// The imported-instance step's tallies (GD.31).
    pub(super) instances: InstanceStats,
}

/// The per-repo tallies of the imported-instance step (GD.31, module doc
/// step 4).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct InstanceStats {
    /// Exported contract instances indexed.
    pub(super) exported: usize,
    /// Importing files that gained a client site.
    pub(super) importers: usize,
    /// Call sites minted.
    pub(super) sites: usize,
    /// Event sites minted.
    pub(super) events: usize,
    /// Imported locals naming an instance that mint nothing: the importer
    /// declares or assigns the local, or two star re-exports give it.
    pub(super) unresolved: usize,
}

impl EvmAbiJoinStats {
    /// fired_on marker, once per repo with a declaration or a referencing
    /// cell (module doc).
    pub(super) fn report(&self, repo_label: &str) {
        if self.declarations + self.joined + self.unresolved + self.ambiguous > 0 {
            eprintln!(
                "[evm-abi-join] joined={} unresolved={} ambiguous={} declarations={} via_barrel={} via_alias={} repo={repo_label}",
                self.joined,
                self.unresolved,
                self.ambiguous,
                self.declarations,
                self.via_barrel,
                self.via_alias
            );
        }
        let i = &self.instances;
        if i.exported > 0 {
            eprintln!(
                "[evm-instances] exported={} importers={} sites={} events={} unresolved={} repo={repo_label}",
                i.exported, i.importers, i.sites, i.events, i.unresolved
            );
        }
    }
}

/// One Go ABI constant of the index.
#[derive(Debug, Clone, PartialEq, Eq)]
struct GoDecl {
    file: String,
    /// Declared in a `_test.go` file.
    test: bool,
    entries: Vec<String>,
}

/// The declarations the join reads (module doc, step 1).
#[derive(Debug, Default)]
struct Index {
    /// TS: (declaring file, exported name) -> entries; `None` once a key is
    /// declared twice.
    ts: BTreeMap<(String, String), Option<Vec<String>>>,
    /// Go: (directory, constant name) -> its declarations, in file order.
    go: BTreeMap<(String, String), Vec<GoDecl>>,
    /// TS barrels (GD.20): file -> its re-exports, sorted.
    reexports: BTreeMap<String, Vec<ReExport>>,
    /// TS (GD.20): file -> every name its own `export` declarations export.
    exports: BTreeMap<String, BTreeSet<String>>,
    /// TS (GD.31): (declaring file, exported name) -> the contract instance;
    /// `None` once a key is declared twice.
    instances: BTreeMap<(String, String), Option<Instance>>,
}

/// One re-export of a TS barrel (DT.1q's facts, GD.20).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum ReExport {
    /// `export { <name> as <exported> } from '<source>'`.
    Name {
        source: String,
        name: String,
        exported: String,
    },
    /// `export * from '<source>'`.
    Star { source: String },
}

/// What a TS (file, name) lookup found (GD.20): an ABI's entries or (GD.31)
/// a contract instance.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Found<T> {
    Hit {
        value: T,
        file: String,
        barrel: bool,
        alias: bool,
    },
    Miss,
    Ambiguous,
}

impl<T> Found<T> {
    /// The same result, reached through a barrel and (when `alias`) an alias.
    fn through(self, alias: bool) -> Found<T> {
        match self {
            Found::Hit {
                value,
                file,
                alias: a,
                ..
            } => Found::Hit {
                value,
                file,
                barrel: true,
                alias: a || alias,
            },
            other => other,
        }
    }
}

/// One exported TS contract instance of the index (GD.31): what
/// `NavFact::EvmContract` recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Instance {
    scope: evm::Scope,
    lib: String,
    api: String,
    abi: Option<Vec<String>>,
    abi_ref: Option<evm::AbiRef>,
}

impl Index {
    fn build(parses_by_lang: &HashMap<&'static str, Vec<FileParse>>) -> Index {
        let mut index = Index::default();
        for fp in parses_by_lang.values().flatten() {
            if fp.nav.nav_facts.is_empty() {
                continue;
            }
            let Some(file) = http_owner::module_file(fp) else {
                continue;
            };
            let go = file.ends_with(".go");
            for fact in fp.nav.nav_facts.values().flatten() {
                match fact {
                    NavFact::EvmAbi { name, entries } if !go => {
                        index
                            .ts
                            .entry((file.clone(), name.clone()))
                            .and_modify(|e| *e = None)
                            .or_insert_with(|| Some(entries.clone()));
                    }
                    NavFact::ReExportName {
                        source,
                        name,
                        exported,
                        ..
                    } if !go => {
                        index
                            .reexports
                            .entry(file.clone())
                            .or_default()
                            .push(ReExport::Name {
                                source: source.clone(),
                                name: name.clone(),
                                exported: exported.clone(),
                            })
                    }
                    NavFact::ReExportStar { source, .. } if !go => index
                        .reexports
                        .entry(file.clone())
                        .or_default()
                        .push(ReExport::Star {
                            source: source.clone(),
                        }),
                    NavFact::Exports { names } if !go => index
                        .exports
                        .entry(file.clone())
                        .or_default()
                        .extend(names.iter().cloned()),
                    NavFact::EvmContract {
                        name,
                        scope_key,
                        lib,
                        api,
                        abi,
                        abi_ref,
                    } if !go => {
                        let Some(scope) = evm::Scope::parse(scope_key) else {
                            continue;
                        };
                        let instance = Instance {
                            scope,
                            lib: lib.clone(),
                            api: api.clone(),
                            abi: abi.clone(),
                            abi_ref: abi_ref.clone(),
                        };
                        index
                            .instances
                            .entry((file.clone(), name.clone()))
                            .and_modify(|e| *e = None)
                            .or_insert_with(|| Some(instance));
                    }
                    NavFact::GoStringConst { name, value } if go => {
                        let Some(entries) = read_go_const_abi(value) else {
                            continue;
                        };
                        let mut keys: Vec<String> =
                            entries.iter().map(evm::AbiEntry::to_key).collect();
                        keys.sort();
                        keys.dedup();
                        let decl = GoDecl {
                            file: file.clone(),
                            test: file.ends_with("_test.go"),
                            entries: keys,
                        };
                        index
                            .go
                            .entry((parent(&file).to_string(), name.clone()))
                            .or_default()
                            .push(decl);
                    }
                    _ => {}
                }
            }
        }
        for decls in index.go.values_mut() {
            decls.sort_by(|a, b| (&a.file, &a.entries).cmp(&(&b.file, &b.entries)));
            decls.dedup();
        }
        for reexports in index.reexports.values_mut() {
            reexports.sort();
            reexports.dedup();
        }
        index
    }

    /// Indexed declarations: the TS keys kept, every Go constant.
    fn declarations(&self) -> usize {
        self.ts.values().filter(|e| e.is_some()).count()
            + self.go.values().map(Vec::len).sum::<usize>()
    }

    /// What one reference from `caller` resolves to.
    fn resolve(&self, caller: &str, abi_ref: &evm::AbiRef, aliases: &TsAliasSet) -> Resolved {
        if caller.ends_with(".go") {
            return self.resolve_go(caller, abi_ref);
        }
        let Some((path, alias)) = ts_target(caller, &abi_ref.from, aliases) else {
            return Resolved::Unresolved;
        };
        let mut seen = BTreeSet::new();
        match self.lookup(&self.ts, &path, &abi_ref.name, aliases, 0, &mut seen) {
            Found::Hit {
                value: entries,
                file,
                barrel,
                alias: through_alias,
            } => Resolved::Joined {
                entries,
                file,
                barrel,
                alias: alias || through_alias,
            },
            Found::Miss => Resolved::Unresolved,
            Found::Ambiguous => Resolved::Ambiguous,
        }
    }

    /// The contract instance export `name` of the TS module `from` names
    /// from `caller` (GD.31): mapped and followed exactly as an ABI
    /// reference is ([`Index::resolve`]).
    fn resolve_instance(
        &self,
        caller: &str,
        name: &str,
        from: &str,
        aliases: &TsAliasSet,
    ) -> Found<Instance> {
        let Some((path, alias)) = ts_target(caller, from, aliases) else {
            return Found::Miss;
        };
        let mut seen = BTreeSet::new();
        match self.lookup(&self.instances, &path, name, aliases, 0, &mut seen) {
            Found::Hit {
                value,
                file,
                barrel,
                alias: a,
            } => Found::Hit {
                value,
                file,
                barrel,
                alias: alias || a,
            },
            other => other,
        }
    }

    /// The declaration of export `name` of the TS module at `path` in
    /// `table` (GD.20, module doc step 2): the first candidate file that
    /// declares it or re-exports it.
    fn lookup<T: Clone + PartialEq>(
        &self,
        table: &BTreeMap<(String, String), Option<T>>,
        path: &str,
        name: &str,
        aliases: &TsAliasSet,
        depth: usize,
        seen: &mut BTreeSet<(String, String)>,
    ) -> Found<T> {
        for candidate in ts_candidates(path) {
            match table.get(&(candidate.clone(), name.to_string())) {
                Some(Some(value)) => {
                    return Found::Hit {
                        value: value.clone(),
                        file: candidate,
                        barrel: false,
                        alias: false,
                    };
                }
                Some(None) => return Found::Miss,
                None => {}
            }
            if let Some(found) = self.through_barrel(table, &candidate, name, aliases, depth, seen)
            {
                return found;
            }
        }
        Found::Miss
    }

    /// Export `name` of barrel `file` through its re-exports; `None` when
    /// `file` re-exports nothing.
    fn through_barrel<T: Clone + PartialEq>(
        &self,
        table: &BTreeMap<(String, String), Option<T>>,
        file: &str,
        name: &str,
        aliases: &TsAliasSet,
        depth: usize,
        seen: &mut BTreeSet<(String, String)>,
    ) -> Option<Found<T>> {
        let reexports = self.reexports.get(file)?;
        if depth >= MAX_BARREL_DEPTH || !seen.insert((file.to_string(), name.to_string())) {
            return Some(Found::Miss);
        }
        let named = reexports.iter().find_map(|r| match r {
            ReExport::Name {
                source,
                name: inner,
                exported,
            } if exported == name => Some((source, inner)),
            _ => None,
        });
        if let Some((source, inner)) = named {
            let Some((target, alias)) = ts_target(file, source, aliases) else {
                return Some(Found::Miss);
            };
            return Some(
                self.lookup(table, &target, inner, aliases, depth + 1, seen)
                    .through(alias),
            );
        }
        if name == DEFAULT_EXPORT || self.exports.get(file).is_some_and(|e| e.contains(name)) {
            return Some(Found::Miss);
        }
        let mut hits: Vec<Found<T>> = Vec::new();
        let mut ambiguous = false;
        for r in reexports {
            let ReExport::Star { source } = r else {
                continue;
            };
            let Some((target, alias)) = ts_target(file, source, aliases) else {
                continue;
            };
            match self
                .lookup(table, &target, name, aliases, depth + 1, seen)
                .through(alias)
            {
                hit @ Found::Hit { .. } => {
                    if !hits.contains(&hit) {
                        hits.push(hit);
                    }
                }
                Found::Ambiguous => ambiguous = true,
                Found::Miss => {}
            }
        }
        Some(match (hits.len(), ambiguous) {
            (1, false) => hits.swap_remove(0),
            (0, false) => Found::Miss,
            _ => Found::Ambiguous,
        })
    }

    fn resolve_go(&self, caller: &str, abi_ref: &evm::AbiRef) -> Resolved {
        if abi_ref.from != evm::GO_PACKAGE {
            return Resolved::Unresolved;
        }
        let test_caller = caller.ends_with("_test.go");
        let key = (parent(caller).to_string(), abi_ref.name.clone());
        let visible: Vec<&GoDecl> = self
            .go
            .get(&key)
            .into_iter()
            .flatten()
            .filter(|d| !d.test || test_caller)
            .collect();
        match visible.as_slice() {
            [] => Resolved::Unresolved,
            [one] => Resolved::Joined {
                entries: one.entries.clone(),
                file: one.file.clone(),
                barrel: false,
                alias: false,
            },
            _ => Resolved::Ambiguous,
        }
    }
}

/// What one reference resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Resolved {
    Joined {
        entries: Vec<String>,
        file: String,
        /// Reached through a TS barrel (GD.20).
        barrel: bool,
        /// A specifier an alias mapped (GD.20).
        alias: bool,
    },
    Unresolved,
    Ambiguous,
}

/// The directory of a repo-relative file (`""` at the root).
fn parent(file: &str) -> &str {
    file.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// The repo-relative path a relative TS specifier names from `caller`;
/// `None` for a bare or alias specifier and for one that climbs above the
/// repo root.
fn relative_target(caller: &str, spec: &str) -> Option<String> {
    let relative = spec == "." || spec == ".." || spec.starts_with("./") || spec.starts_with("../");
    if !relative {
        return None;
    }
    let mut parts: Vec<&str> = parent(caller)
        .split('/')
        .filter(|p| !p.is_empty())
        .collect();
    for seg in spec.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            seg => parts.push(seg),
        }
    }
    Some(parts.join("/"))
}

/// The repo-relative path a TS specifier names from `file`, and whether an
/// alias mapped it (GD.20): a relative specifier as [`relative_target`]
/// reads it, any other through `aliases` as the TS graph build maps an
/// import; `None` for a package nothing maps.
fn ts_target(file: &str, spec: &str, aliases: &TsAliasSet) -> Option<(String, bool)> {
    if let Some(path) = relative_target(file, spec) {
        return Some((path, false));
    }
    if spec.starts_with('.') {
        return None;
    }
    let module = file
        .rsplit_once('.')
        .filter(|(stem, _)| !stem.ends_with('/') && !stem.is_empty())
        .map_or(file, |(stem, _)| stem)
        .replace('/', "::");
    let qname = resolve_ts_source_aliased(&module, spec, aliases)?;
    let qname = untag_workspace(&qname)?;
    Some((qname.replace("::", "/"), true))
}

/// A resolved TS qname without the FH.5 workspace-package tag the alias
/// resolver puts before one (`\0workspace:entry\0<qname>`,
/// `\0workspace:subpath\0<qname>`; the TS graph build reads and strips it
/// the same way); `None` for a package with no entry (an empty qname). GD.31:
/// before it a workspace specifier resolved to the tagged text, which names
/// no file, so GD.20's ABI join through a workspace package joined nothing.
fn untag_workspace(qname: &str) -> Option<&str> {
    let qname = match qname.strip_prefix('\0') {
        Some(tagged) => tagged.split_once('\0')?.1,
        None => qname,
    };
    (!qname.is_empty()).then_some(qname)
}

/// The files a resolved TS path is tried as, in order (module doc, step 2).
fn ts_candidates(path: &str) -> Vec<String> {
    let mut out = vec![path.to_string()];
    out.extend(TS_CANDIDATES.iter().map(|ext| format!("{path}{ext}")));
    for (written, source) in [(".js", ".ts"), (".jsx", ".tsx")] {
        if let Some(stem) = path.strip_suffix(written) {
            out.push(format!("{stem}{source}"));
        }
    }
    out
}

/// Join every referencing evm cell of the repo's parses to the declaration
/// it names (module doc).
pub(super) fn join_evm_abis(
    parses_by_lang: &mut HashMap<&'static str, Vec<FileParse>>,
    aliases: &TsAliasSet,
) -> EvmAbiJoinStats {
    let index = Index::build(parses_by_lang);
    let mut stats = EvmAbiJoinStats {
        declarations: index.declarations(),
        ..EvmAbiJoinStats::default()
    };
    for fp in parses_by_lang.values_mut().flatten() {
        let mut caller: Option<Option<String>> = None;
        for i in 0..fp.nodes.len() {
            let id = fp.nodes[i].id;
            let client = fp
                .nav
                .kind_by_id
                .get(&id)
                .is_some_and(|k| *k == node_kind::RPC_CALL || *k == node_kind::EVENT_HANDLER)
                && fp
                    .nav
                    .qname_by_id
                    .get(&id)
                    .is_some_and(|q| evm::split_client(q).is_some());
            if !client {
                continue;
            }
            for j in 0..fp.nodes[i].cells.len() {
                let Some(mut hit) = evm::read_client_hit(&fp.nodes[i].cells[j]) else {
                    continue;
                };
                let Some(abi_ref) = hit.abi_ref.as_ref().filter(|_| hit.facts.abi.is_none()) else {
                    continue;
                };
                let Some(file) = caller
                    .get_or_insert_with(|| http_owner::module_file(fp))
                    .clone()
                else {
                    stats.unresolved += 1;
                    continue;
                };
                match index.resolve(&file, abi_ref, aliases) {
                    Resolved::Joined {
                        entries,
                        file,
                        barrel,
                        alias,
                    } => {
                        stats.joined += 1;
                        stats.via_barrel += usize::from(barrel);
                        stats.via_alias += usize::from(alias);
                        hit.facts.abi = Some(entries);
                        hit.abi_from = Some(file);
                        fp.nodes[i].cells[j] = evm::client_hit_cell(&hit);
                    }
                    Resolved::Unresolved => stats.unresolved += 1,
                    Resolved::Ambiguous => stats.ambiguous += 1,
                }
            }
        }
    }
    stats.instances = mint_instances(parses_by_lang, &index, aliases);
    stats
}

/// One `NavFact::EvmImportSite` of an importing file (GD.31).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ImportSite {
    line: u32,
    local: String,
    name: String,
    from: String,
    api: String,
    event: bool,
    member: String,
    args: Option<u32>,
    reassigned: bool,
}

/// One cell variant of a minted client (GD.31): a [`evm::ClientHit`] less
/// its argument counts.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct HitKey {
    lib: String,
    abi: Option<Vec<String>>,
    abi_ref: Option<evm::AbiRef>,
    abi_from: Option<String>,
}

impl HitKey {
    fn into_hit(self, args: Vec<u32>) -> evm::ClientHit {
        evm::ClientHit {
            facts: evm::ClientFacts {
                lib: self.lib,
                args,
                abi: self.abi,
            },
            abi_ref: self.abi_ref,
            abi_from: self.abi_from,
        }
    }
}

/// One client node an importing file gains (GD.31): its scope, the
/// argument counts per cell variant and the rows of its sites.
#[derive(Debug, Default)]
struct MintedClient {
    variants: BTreeMap<HitKey, BTreeSet<u32>>,
    lines: Vec<u32>,
}

/// GD.31, module doc step 4: mint every importing file's clients of the
/// exported contract instances its imports name.
fn mint_instances(
    parses_by_lang: &mut HashMap<&'static str, Vec<FileParse>>,
    index: &Index,
    aliases: &TsAliasSet,
) -> InstanceStats {
    let mut stats = InstanceStats {
        exported: index.instances.values().filter(|i| i.is_some()).count(),
        ..InstanceStats::default()
    };
    if stats.exported == 0 {
        return stats;
    }
    for fp in parses_by_lang.values_mut().flatten() {
        mint_file(fp, index, aliases, &mut stats);
    }
    stats
}

/// The ABI fields an instance gives its importers' cells: the entries the
/// exporting file knows, or those its own reference joins to (resolved from
/// the exporting file), with the file they were read from; the exporting
/// file when no ABI is known.
fn instance_abi(
    instance: &Instance,
    decl_file: &str,
    index: &Index,
    aliases: &TsAliasSet,
) -> (Option<Vec<String>>, Option<evm::AbiRef>, Option<String>) {
    if let Some(abi) = &instance.abi {
        return (
            Some(abi.clone()),
            instance.abi_ref.clone(),
            Some(decl_file.to_string()),
        );
    }
    let Some(abi_ref) = &instance.abi_ref else {
        return (None, None, Some(decl_file.to_string()));
    };
    match index.resolve(decl_file, abi_ref, aliases) {
        Resolved::Joined { entries, file, .. } => {
            (Some(entries), Some(abi_ref.clone()), Some(file))
        }
        Resolved::Unresolved | Resolved::Ambiguous => (None, Some(abi_ref.clone()), None),
    }
}

/// Whether ABI `keys` list `member` as a function (a call) or an event.
fn abi_lists(keys: &[String], member: &str, event: bool) -> bool {
    keys.iter()
        .filter_map(|k| evm::AbiEntry::from_key(k))
        .any(|e| {
            e.name == member
                && if event {
                    e.kind == evm::EntryKind::Event
                } else {
                    e.kind == evm::EntryKind::Fn
                }
        })
}

/// One importing file's clients (GD.31): its `EvmImportSite`s grouped by
/// import, each import resolved to an exported instance, the sites read in
/// the instance's API minted as its clients and anchored by the A5.8 pass.
fn mint_file(fp: &mut FileParse, index: &Index, aliases: &TsAliasSet, stats: &mut InstanceStats) {
    // (module id, site): the facts sit on the file MODULE; sorted, so the
    // HashMap order of `nav_facts` reaches nothing.
    let mut facts: Vec<(u64, ImportSite)> = Vec::new();
    for (scope, list) in &fp.nav.nav_facts {
        for fact in list {
            if let NavFact::EvmImportSite {
                local,
                name,
                from,
                api,
                event,
                member,
                args,
                line,
                reassigned,
            } = fact
            {
                facts.push((
                    scope.0,
                    ImportSite {
                        line: *line,
                        local: local.clone(),
                        name: name.clone(),
                        from: from.clone(),
                        api: api.clone(),
                        event: *event,
                        member: member.clone(),
                        args: *args,
                        reassigned: *reassigned,
                    },
                ));
            }
        }
    }
    if facts.is_empty() {
        return;
    }
    let Some(file) = http_owner::module_file(fp) else {
        return;
    };
    facts.sort();
    let module_id = NodeId(facts[0].0);
    let Some(repo) = fp.nodes.iter().find(|n| n.id == module_id).map(|n| n.repo) else {
        return;
    };
    let mut imports: BTreeMap<(&str, &str, &str), Vec<&ImportSite>> = BTreeMap::new();
    for (_, site) in &facts {
        imports
            .entry((&site.local, &site.name, &site.from))
            .or_default()
            .push(site);
    }
    let mut clients: BTreeMap<(evm::ClientSide, String, String), (evm::Scope, MintedClient)> =
        BTreeMap::new();
    for ((_, name, from), sites) in imports {
        let (instance, decl_file) = match index.resolve_instance(&file, name, from, aliases) {
            Found::Hit { value, file, .. } => (value, file),
            Found::Miss => continue,
            Found::Ambiguous => {
                stats.unresolved += 1;
                continue;
            }
        };
        if decl_file == file {
            continue;
        }
        if sites.iter().any(|s| s.reassigned) {
            stats.unresolved += 1;
            continue;
        }
        let (abi, abi_ref, abi_from) = instance_abi(&instance, &decl_file, index, aliases);
        for site in sites.iter().filter(|s| s.api == instance.api) {
            if abi
                .as_deref()
                .is_some_and(|keys| !abi_lists(keys, &site.member, site.event))
            {
                continue;
            }
            let side = if site.event {
                stats.events += 1;
                evm::ClientSide::Event
            } else {
                stats.sites += 1;
                evm::ClientSide::Call
            };
            let (_, client) = clients
                .entry((side, instance.scope.key(), site.member.clone()))
                .or_insert_with(|| (instance.scope.clone(), MintedClient::default()));
            let key = HitKey {
                lib: instance.lib.clone(),
                abi: abi.clone(),
                abi_ref: abi_ref.clone(),
                abi_from: abi_from.clone(),
            };
            client.variants.entry(key).or_default().extend(site.args);
            client.lines.push(site.line);
        }
    }
    if clients.is_empty() {
        return;
    }
    stats.importers += 1;
    let mut anchors: Vec<Anchor> = Vec::new();
    let first_new = fp.nodes.len();
    for ((side, key, member), (scope, client)) in clients {
        let (kind, qname) = match side {
            evm::ClientSide::Call => (node_kind::RPC_CALL, evm::call_qname(&scope, &member)),
            evm::ClientSide::Event => (node_kind::EVENT_HANDLER, evm::event_qname(&scope, &member)),
        };
        let id = NodeId::from_parts(GRAPH_TYPE, repo, kind, &qname);
        let cells: Vec<Cell> = client
            .variants
            .into_iter()
            .map(|(key, args)| evm::client_hit_cell(&key.into_hit(args.into_iter().collect())))
            .collect();
        anchors.extend(client.lines.iter().map(|&line| Anchor { node: id, line }));
        if let Some(node) = fp.nodes.iter_mut().find(|n| n.id == id) {
            // The file reads the same client itself: its new variants go
            // after its last evm cell.
            for cell in cells {
                if node.cells.contains(&cell) {
                    continue;
                }
                let at = node
                    .cells
                    .iter()
                    .rposition(|c| evm::read_client_hit(c).is_some())
                    .map_or(0, |i| i + 1);
                node.cells.insert(at, cell);
            }
            continue;
        }
        fp.nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Medium,
            cells,
        });
        fp.nav.record(
            id,
            &format!("{key}.{member}"),
            &qname,
            kind,
            Some(module_id),
        );
    }
    // A5.8: the owner edges and first-site POSITION every in-file client gets
    // (`anchor::attach` only appends, and skips what the parse holds).
    let first_edge = fp.edges.len();
    anchor::attach(fp, &file, module_id, &mut anchors);
    if let Some(added) = fp.edges.get_mut(first_edge..) {
        evidence::stamp_missing(added, "extractor:anchor");
    }
    // The router's raw G15 IMPORTS cell, as every other node of the file has
    // (`filter_imports_cells` rewrites it with the rest of the file's).
    if fp.nodes.len() > first_new
        && let Some(lang) = detect_language(&file)
    {
        let mut extra = FileParse {
            nodes: fp.nodes.split_off(first_new),
            imports: fp.imports.clone(),
            ..Default::default()
        };
        attach_imports_cell(&mut extra, lang);
        fp.nodes.extend(extra.nodes);
    }
}

#[cfg(test)]
mod tests {
    use glia_code_domain::evm::{AbiRef, ClientFacts, ClientHit, Scope, read_client_hits};
    use glia_code_domain::project_roots::ProjectRoot;
    use glia_code_domain::{cell_type, edge_category};
    use glia_core::{CellPayload, NodeKindId, RepoId};

    use super::*;

    const REPO: RepoId = RepoId(23);

    fn position(file: &str) -> Cell {
        Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(format!(
                r#"{{"file":"{file}","start_line":0,"end_line":1}}"#
            )),
        }
    }

    fn push(fp: &mut FileParse, kind: NodeKindId, q: &str, cells: Vec<Cell>) -> NodeId {
        let id = NodeId::from_parts(GRAPH_TYPE, REPO, kind, q);
        fp.nodes.push(Node {
            id,
            repo: REPO,
            confidence: Confidence::Medium,
            cells,
        });
        fp.nav.record(id, q, q, kind, None);
        id
    }

    /// A parse of `file`: its MODULE first, as the router hands it over.
    fn parse(file: &str) -> (FileParse, NodeId) {
        let mut fp = FileParse::default();
        let q = file
            .rsplit_once('.')
            .map_or(file, |(stem, _)| stem)
            .replace('/', "::");
        let m = push(&mut fp, node_kind::MODULE, &q, vec![position(file)]);
        (fp, m)
    }

    fn keys(keys: &[&str]) -> Vec<String> {
        keys.iter().map(|k| (*k).to_string()).collect()
    }

    /// A TS module exporting ABI `name`.
    fn ts_decl(file: &str, name: &str, entries: &[&str]) -> FileParse {
        let (mut fp, m) = parse(file);
        fp.nav.record_fact(
            m,
            NavFact::EvmAbi {
                name: name.into(),
                entries: keys(entries),
            },
        );
        fp
    }

    /// A Go file declaring the string constant `name = value`.
    fn go_decl(file: &str, name: &str, value: &str) -> FileParse {
        let (mut fp, m) = parse(file);
        fp.nav.record_fact(
            m,
            NavFact::GoStringConst {
                name: name.into(),
                value: value.into(),
            },
        );
        fp
    }

    /// A client file calling `scope.member` with one unjoined cell naming
    /// `abi_ref`.
    fn client(
        file: &str,
        scope: &str,
        member: &str,
        lib: &str,
        abi_ref: AbiRef,
    ) -> (FileParse, NodeId) {
        let (mut fp, _) = parse(file);
        let hit = ClientHit {
            facts: ClientFacts {
                lib: lib.into(),
                args: vec![1],
                abi: None,
            },
            abi_ref: Some(abi_ref),
            abi_from: None,
        };
        let q = evm::call_qname(&Scope::Abi(scope.into()), member);
        let id = push(
            &mut fp,
            node_kind::RPC_CALL,
            &q,
            vec![position(file), evm::client_hit_cell(&hit)],
        );
        (fp, id)
    }

    fn ts_ref(name: &str, from: &str) -> AbiRef {
        AbiRef {
            name: name.into(),
            from: from.into(),
        }
    }

    fn go_ref(name: &str) -> AbiRef {
        AbiRef {
            name: name.into(),
            from: evm::GO_PACKAGE.into(),
        }
    }

    /// The evm hits of `id` across every parse.
    fn hits(by_lang: &HashMap<&'static str, Vec<FileParse>>, id: NodeId) -> Vec<ClientHit> {
        by_lang
            .values()
            .flatten()
            .flat_map(|fp| fp.nodes.iter().filter(move |n| n.id == id))
            .flat_map(|n| read_client_hits(&n.cells))
            .collect()
    }

    fn by_lang(
        lang: &'static str,
        parses: Vec<FileParse>,
    ) -> HashMap<&'static str, Vec<FileParse>> {
        HashMap::from([(lang, parses)])
    }

    const ESCROW: &[&str] = &["f:deposit/1", "f:release/1"];
    const VAULT: &[&str] = &["f:release/1", "f:withdraw/1"];

    #[test]
    fn ts_relative_import_joins_the_exported_abi() {
        let (pay, id) = client(
            "web/src/pay.ts",
            "SHOP_ABI",
            "release",
            "ethers",
            ts_ref("SHOP_ABI", "./abi"),
        );
        let mut parses = by_lang(
            "typescript",
            vec![pay, ts_decl("web/src/abi.ts", "SHOP_ABI", ESCROW)],
        );
        let stats = join_evm_abis(&mut parses, &TsAliasSet::default());
        assert_eq!(
            stats,
            EvmAbiJoinStats {
                joined: 1,
                unresolved: 0,
                ambiguous: 0,
                declarations: 1,
                ..EvmAbiJoinStats::default()
            }
        );
        let got = hits(&parses, id);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].facts.abi, Some(keys(ESCROW)));
        assert_eq!(got[0].abi_from.as_deref(), Some("web/src/abi.ts"));
        assert_eq!(
            got[0].abi_ref,
            Some(ts_ref("SHOP_ABI", "./abi")),
            "the reference stays"
        );
        let again = join_evm_abis(&mut parses, &TsAliasSet::default());
        assert_eq!(
            again.joined, 0,
            "a joined cell has its ABI: a second run joins nothing"
        );
    }

    #[test]
    fn ts_import_follows_the_specifier_not_the_name() {
        let (pay, id) = client(
            "src/pay.ts",
            "SHOP_ABI",
            "release",
            "ethers",
            ts_ref("SHOP_ABI", "./abi"),
        );
        let (old, old_id) = client(
            "src/admin/old.ts",
            "SHOP_ABI",
            "withdraw",
            "ethers",
            ts_ref("SHOP_ABI", "../legacy-abi.js"),
        );
        let mut parses = by_lang(
            "typescript",
            vec![
                ts_decl("src/legacy-abi.ts", "SHOP_ABI", VAULT),
                pay,
                old,
                ts_decl("src/abi/index.ts", "SHOP_ABI", ESCROW),
            ],
        );
        let stats = join_evm_abis(&mut parses, &TsAliasSet::default());
        assert_eq!(
            stats,
            EvmAbiJoinStats {
                joined: 2,
                unresolved: 0,
                ambiguous: 0,
                declarations: 2,
                ..EvmAbiJoinStats::default()
            }
        );
        let got = hits(&parses, id);
        assert_eq!(
            got[0].facts.abi,
            Some(keys(ESCROW)),
            "`./abi` is src/abi/index.ts"
        );
        assert_eq!(got[0].abi_from.as_deref(), Some("src/abi/index.ts"));
        let got = hits(&parses, old_id);
        assert_eq!(
            got[0].facts.abi,
            Some(keys(VAULT)),
            "an ESM `.js` specifier names the `.ts` file"
        );
        assert_eq!(got[0].abi_from.as_deref(), Some("src/legacy-abi.ts"));
    }

    #[test]
    fn ts_alias_specifier_stays_unresolved() {
        let (alias, a) = client(
            "src/pay.ts",
            "SHOP_ABI",
            "release",
            "ethers",
            ts_ref("SHOP_ABI", "@app/abi"),
        );
        let (pkg, p) = client(
            "src/b.ts",
            "ERC20_ABI",
            "decimals",
            "viem",
            ts_ref("ERC20_ABI", "abis"),
        );
        let (above, u) = client(
            "src/c.ts",
            "X_ABI",
            "f",
            "ethers",
            ts_ref("X_ABI", "../../abi"),
        );
        let (named, n) = client("src/d.ts", "OTHER", "f", "ethers", ts_ref("OTHER", "./abi"));
        let mut parses = by_lang(
            "typescript",
            vec![
                alias,
                pkg,
                above,
                named,
                ts_decl("src/abi.ts", "SHOP_ABI", ESCROW),
            ],
        );
        let stats = join_evm_abis(&mut parses, &TsAliasSet::default());
        assert_eq!(
            stats,
            EvmAbiJoinStats {
                joined: 0,
                unresolved: 4,
                ambiguous: 0,
                declarations: 1,
                ..EvmAbiJoinStats::default()
            }
        );
        for id in [a, p, u, n] {
            let got = hits(&parses, id);
            assert_eq!(
                (got[0].facts.abi.as_ref(), got[0].abi_from.as_ref()),
                (None, None)
            );
        }
    }

    #[test]
    fn ts_key_declared_twice_is_dropped() {
        let (pay, id) = client(
            "src/pay.ts",
            "SHOP_ABI",
            "release",
            "ethers",
            ts_ref("SHOP_ABI", "./abi"),
        );
        let (mut abi, m) = parse("src/abi.ts");
        abi.nav.record_fact(
            m,
            NavFact::EvmAbi {
                name: "SHOP_ABI".into(),
                entries: keys(ESCROW),
            },
        );
        abi.nav.record_fact(
            m,
            NavFact::EvmAbi {
                name: "SHOP_ABI".into(),
                entries: keys(VAULT),
            },
        );
        let mut parses = by_lang("typescript", vec![pay, abi]);
        let stats = join_evm_abis(&mut parses, &TsAliasSet::default());
        assert_eq!(
            stats,
            EvmAbiJoinStats {
                joined: 0,
                unresolved: 1,
                ambiguous: 0,
                declarations: 0,
                ..EvmAbiJoinStats::default()
            }
        );
        assert_eq!(hits(&parses, id)[0].facts.abi, None);
    }

    /// Kina's `escrowABI` shape: a raw-string JSON ABI.
    const RAW: &str = r#"[{"type":"function","name":"deposit","inputs":[{"name":"orderId","type":"bytes32"}]},{"type":"function","name":"release","inputs":[{"name":"orderId","type":"bytes32"}]}]"#;

    #[test]
    fn go_same_directory_string_const_joins() {
        let (cl, id) = client(
            "api/chain/client.go",
            "shopABI",
            "release",
            "geth",
            go_ref("shopABI"),
        );
        let mut parses = by_lang(
            "go",
            vec![
                cl,
                go_decl("api/chain/abi.go", "shopABI", RAW),
                go_decl("api/other/abi.go", "shopABI", RAW),
                go_decl("api/chain/paths.go", "apiBase", "/api/v1"),
            ],
        );
        let stats = join_evm_abis(&mut parses, &TsAliasSet::default());
        assert_eq!(
            stats,
            EvmAbiJoinStats {
                joined: 1,
                unresolved: 0,
                ambiguous: 0,
                declarations: 2,
                ..EvmAbiJoinStats::default()
            },
            "another directory is another package; a path constant is no ABI"
        );
        let got = hits(&parses, id);
        assert_eq!(got[0].facts.abi, Some(keys(ESCROW)));
        assert_eq!(got[0].abi_from.as_deref(), Some("api/chain/abi.go"));
    }

    #[test]
    fn go_interpreted_string_const_is_unescaped() {
        let interpreted = RAW.replace('"', "\\\"");
        let (cl, id) = client(
            "chain/client.go",
            "shopABI",
            "release",
            "geth",
            go_ref("shopABI"),
        );
        let mut parses = by_lang(
            "go",
            vec![cl, go_decl("chain/abi.go", "shopABI", &interpreted)],
        );
        let stats = join_evm_abis(&mut parses, &TsAliasSet::default());
        assert_eq!(stats.joined, 1);
        assert_eq!(hits(&parses, id)[0].facts.abi, Some(keys(ESCROW)));
    }

    #[test]
    fn go_two_declarations_are_ambiguous() {
        let (cl, id) = client(
            "chain/client.go",
            "shopABI",
            "release",
            "geth",
            go_ref("shopABI"),
        );
        let mut parses = by_lang(
            "go",
            vec![
                cl,
                go_decl("chain/abi.go", "shopABI", RAW),
                go_decl("chain/abi_v2.go", "shopABI", RAW),
            ],
        );
        let stats = join_evm_abis(&mut parses, &TsAliasSet::default());
        assert_eq!(
            stats,
            EvmAbiJoinStats {
                joined: 0,
                unresolved: 0,
                ambiguous: 1,
                declarations: 2,
                ..EvmAbiJoinStats::default()
            }
        );
        assert_eq!(hits(&parses, id)[0].facts.abi, None);
    }

    #[test]
    fn go_test_file_const_serves_only_tests() {
        let (cl, app) = client(
            "chain/client.go",
            "shopABI",
            "release",
            "geth",
            go_ref("shopABI"),
        );
        let (t, test) = client(
            "chain/client_test.go",
            "shopABI",
            "deposit",
            "geth",
            go_ref("shopABI"),
        );
        let mut parses = by_lang(
            "go",
            vec![cl, t, go_decl("chain/fixture_test.go", "shopABI", RAW)],
        );
        let stats = join_evm_abis(&mut parses, &TsAliasSet::default());
        assert_eq!(
            stats,
            EvmAbiJoinStats {
                joined: 1,
                unresolved: 1,
                ambiguous: 0,
                declarations: 1,
                ..EvmAbiJoinStats::default()
            }
        );
        assert_eq!(
            hits(&parses, app)[0].facts.abi,
            None,
            "a test constant never serves app code"
        );
        assert_eq!(
            hits(&parses, test)[0].abi_from.as_deref(),
            Some("chain/fixture_test.go")
        );
    }

    #[test]
    fn joined_cell_round_trips() {
        let (pay, id) = client(
            "src/pay.ts",
            "SHOP_ABI",
            "release",
            "ethers",
            ts_ref("SHOP_ABI", "./abi"),
        );
        let mut parses = by_lang(
            "typescript",
            vec![pay, ts_decl("src/abi.ts", "SHOP_ABI", ESCROW)],
        );
        join_evm_abis(&mut parses, &TsAliasSet::default());
        let cell = parses["typescript"][0]
            .nodes
            .iter()
            .find(|n| n.id == id)
            .and_then(|n| n.cells.iter().find(|c| c.kind == cell_type::ENDPOINT_HIT))
            .cloned()
            .expect("the client's evm cell");
        assert_eq!(
            cell.payload,
            CellPayload::Json(
                r#"{"via":"evm","lib":"ethers","args":[1],"abi":["f:deposit/1","f:release/1"],"abi_ref":{"name":"SHOP_ABI","from":"./abi"},"abi_from":"src/abi.ts"}"#
                    .into()
            )
        );
        let read = read_client_hits(std::slice::from_ref(&cell));
        assert_eq!(read.len(), 1);
        assert_eq!(
            evm::client_hit_cell(&read[0]),
            cell,
            "read back and rewritten: the same bytes"
        );
        assert_eq!(
            evm::read_client_cells(&[cell])[0].abi,
            Some(keys(ESCROW)),
            "the resolver's reader sees the ABI"
        );
    }

    #[test]
    fn marker_is_quiet_without_declarations_or_references() {
        let (fp, _) = parse("src/app.ts");
        let mut parses = by_lang("typescript", vec![fp]);
        assert_eq!(
            join_evm_abis(&mut parses, &TsAliasSet::default()),
            EvmAbiJoinStats::default()
        );
    }

    // GD.20: barrels and aliases.

    /// A TS module recording `facts` on its MODULE.
    fn ts_facts(file: &str, facts: Vec<NavFact>) -> FileParse {
        let (mut fp, m) = parse(file);
        for fact in facts {
            fp.nav.record_fact(m, fact);
        }
        fp
    }

    fn star(source: &str) -> NavFact {
        NavFact::ReExportStar {
            source: source.into(),
            line: 0,
        }
    }

    fn named(source: &str, name: &str, exported: &str) -> NavFact {
        NavFact::ReExportName {
            source: source.into(),
            name: name.into(),
            exported: exported.into(),
            line: 0,
        }
    }

    /// The `abi_from` of `id`'s single evm hit.
    fn abi_from(by_lang: &HashMap<&'static str, Vec<FileParse>>, id: NodeId) -> Option<String> {
        let hits = hits(by_lang, id);
        assert_eq!(hits.len(), 1);
        hits[0].abi_from.clone()
    }

    #[test]
    fn ts_join_follows_named_and_star_barrels() {
        let (use_ts, a) = client(
            "web/use.ts",
            "escrowAbi",
            "release",
            "ethers",
            ts_ref("escrowAbi", "./abi"),
        );
        let (codec, b) = client(
            "web/codec.ts",
            "VAULT",
            "release",
            "viem",
            ts_ref("VAULT", "./abi/index.js"),
        );
        let mut parses = by_lang(
            "typescript",
            vec![
                use_ts,
                codec,
                ts_facts(
                    "web/abi/index.ts",
                    vec![
                        named("./escrow", "ESCROW", "escrowAbi"),
                        star("./vault"),
                        star("./empty"),
                    ],
                ),
                ts_decl("web/abi/escrow.ts", "ESCROW", ESCROW),
                ts_decl("web/abi/vault.ts", "VAULT", VAULT),
            ],
        );
        let stats = join_evm_abis(&mut parses, &TsAliasSet::default());
        assert_eq!(
            stats,
            EvmAbiJoinStats {
                joined: 2,
                declarations: 2,
                via_barrel: 2,
                ..EvmAbiJoinStats::default()
            }
        );
        assert_eq!(abi_from(&parses, a).as_deref(), Some("web/abi/escrow.ts"));
        assert_eq!(abi_from(&parses, b).as_deref(), Some("web/abi/vault.ts"));
        assert_eq!(hits(&parses, b)[0].facts.abi, Some(keys(VAULT)));
    }

    #[test]
    fn ts_star_barrels_refuse_ambiguity_shadowing_default_and_cycles() {
        let refs = [
            ("web/a.ts", "SHOP", "./both"),
            ("web/b.ts", "SHADOWED", "./shadow"),
            ("web/c.ts", "default", "./both"),
            ("web/d.ts", "LOOP", "./loop1"),
        ];
        let mut files = Vec::new();
        let mut ids = Vec::new();
        for (file, name, from) in refs {
            let (fp, id) = client(file, name, "release", "ethers", ts_ref(name, from));
            files.push(fp);
            ids.push(id);
        }
        files.extend([
            ts_facts("web/both.ts", vec![star("./x"), star("./y")]),
            ts_decl("web/x.ts", "SHOP", ESCROW),
            ts_decl("web/y.ts", "SHOP", VAULT),
            ts_decl("web/x.ts", "default", ESCROW),
            ts_facts(
                "web/shadow.ts",
                vec![
                    star("./x2"),
                    NavFact::Exports {
                        names: vec!["SHADOWED".into()],
                    },
                ],
            ),
            ts_decl("web/x2.ts", "SHADOWED", ESCROW),
            ts_facts("web/loop1.ts", vec![star("./loop2")]),
            ts_facts("web/loop2.ts", vec![star("./loop1")]),
        ]);
        let mut parses = by_lang("typescript", files);
        let stats = join_evm_abis(&mut parses, &TsAliasSet::default());
        assert_eq!(
            stats,
            EvmAbiJoinStats {
                unresolved: 3,
                ambiguous: 1,
                declarations: 4,
                ..EvmAbiJoinStats::default()
            },
            "two stars, a shadowing own export, default through a star, a cycle"
        );
        for id in ids {
            assert_eq!(hits(&parses, id)[0].facts.abi, None);
        }
    }

    #[test]
    fn ts_join_maps_a_tsconfig_alias() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("tsconfig.json"),
            r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@abi/*": ["web/abi/*"] } } }"#,
        )
        .expect("tsconfig");
        let aliases = TsAliasSet::read(dir.path(), &[], "fx");
        let (pay, id) = client(
            "web/pay.ts",
            "escrowAbi",
            "release",
            "ethers",
            ts_ref("escrowAbi", "@abi/index"),
        );
        let (pkg, other) = client(
            "web/pkg.ts",
            "X",
            "release",
            "ethers",
            ts_ref("X", "some-package"),
        );
        let mut parses = by_lang(
            "typescript",
            vec![
                pay,
                pkg,
                ts_facts("web/abi/index.ts", vec![star("./escrow")]),
                ts_decl("web/abi/escrow.ts", "escrowAbi", ESCROW),
            ],
        );
        let stats = join_evm_abis(&mut parses, &aliases);
        assert_eq!(
            stats,
            EvmAbiJoinStats {
                joined: 1,
                unresolved: 1,
                declarations: 1,
                via_barrel: 1,
                via_alias: 1,
                ..EvmAbiJoinStats::default()
            }
        );
        assert_eq!(abi_from(&parses, id).as_deref(), Some("web/abi/escrow.ts"));
        assert_eq!(hits(&parses, other)[0].facts.abi, None);
        // Without the aliases the same reference stays unjoined.
        let (pay, _) = client(
            "web/pay.ts",
            "escrowAbi",
            "release",
            "ethers",
            ts_ref("escrowAbi", "@abi/index"),
        );
        let mut bare = by_lang(
            "typescript",
            vec![pay, ts_decl("web/abi/escrow.ts", "escrowAbi", ESCROW)],
        );
        assert_eq!(
            join_evm_abis(&mut bare, &TsAliasSet::default()).unresolved,
            1
        );
    }

    // ---- GD.31: imported contract instances ----------------------------

    /// Real parses of `files` (path, source), as the router hands them over.
    fn parsed(files: &[(&str, &str)]) -> HashMap<&'static str, Vec<FileParse>> {
        let parses = files
            .iter()
            .map(|(path, src)| {
                let mut fp =
                    crate::extract::parse_one(src, path, "typescript", REPO).expect("parses");
                let module = fp
                    .nodes
                    .iter()
                    .find(|n| fp.nav.kind_by_id.get(&n.id) == Some(&node_kind::MODULE))
                    .map(|n| n.id)
                    .expect("module");
                let mut stats = crate::extract::ExtractStats::default();
                crate::extract::apply_cross_cutting_extractors(
                    &mut fp,
                    src,
                    path,
                    "typescript",
                    module,
                    REPO,
                    &mut stats,
                );
                fp
            })
            .collect();
        by_lang("typescript", parses)
    }

    /// `(qname, evm hits, file, POSITION row)` of every evm client of the
    /// parse of `file`, in node order.
    fn clients_of(
        parses: &HashMap<&'static str, Vec<FileParse>>,
        file: &str,
    ) -> Vec<(String, Vec<ClientHit>)> {
        let fp = parses
            .values()
            .flatten()
            .find(|fp| http_owner::module_file(fp).as_deref() == Some(file))
            .expect("file parsed");
        fp.nodes
            .iter()
            .filter_map(|n| {
                let q = fp.nav.qname_by_id.get(&n.id)?;
                evm::split_client(q)?;
                Some((q.clone(), read_client_hits(&n.cells)))
            })
            .collect()
    }

    /// The edges of `file`'s parse as `(from qname, category, to qname)`.
    fn edges_of(
        parses: &HashMap<&'static str, Vec<FileParse>>,
        file: &str,
    ) -> Vec<(String, glia_core::EdgeCategoryId, String)> {
        let fp = parses
            .values()
            .flatten()
            .find(|fp| http_owner::module_file(fp).as_deref() == Some(file))
            .expect("file parsed");
        let q = |id: &NodeId| fp.nav.qname_by_id.get(id).cloned().unwrap_or_default();
        fp.edges
            .iter()
            .map(|e| (q(&e.from), e.category, q(&e.to)))
            .collect()
    }

    const CONTRACTS_TS: &str = "import { JsonRpcProvider, Wallet } from 'ethers';
import { Escrow__factory } from './typechain-types';

const provider = new JsonRpcProvider('http://localhost:8545');
const signer = Wallet.createRandom().connect(provider);
export const escrow = Escrow__factory.connect(ESCROW_ADDRESS, signer);
export const fmt = { amount: (n: bigint): string => n.toString() };
";

    const APP_TS: &str = "import { escrow, fmt } from 'SPEC';

export async function checkout(payee: string, amount: bigint): Promise<string> {
  await escrow.deposit(payee, { value: amount });
  await escrow.release(payee, amount, MEMO);
  void escrow.on(escrow.filters.Released(), () => undefined);
  console.log(fmt.amount(amount));
  return escrow.arbiter();
}
";

    fn instance_stats(
        exported: usize,
        importers: usize,
        sites: usize,
        events: usize,
        unresolved: usize,
    ) -> InstanceStats {
        InstanceStats {
            exported,
            importers,
            sites,
            events,
            unresolved,
        }
    }

    #[test]
    fn ts_imported_instance_mints_the_importers_clients() {
        let app = APP_TS.replace("SPEC", "../api/contracts");
        let mut parses = parsed(&[
            ("web/src/app.ts", &app),
            ("web/api/contracts.ts", CONTRACTS_TS),
        ]);
        assert!(
            clients_of(&parses, "web/src/app.ts").is_empty(),
            "the per-file reader reads no client in a file that imports no client library"
        );
        let stats = join_evm_abis(&mut parses, &TsAliasSet::default());
        assert_eq!(stats.instances, instance_stats(1, 1, 3, 1, 0));
        let got = clients_of(&parses, "web/src/app.ts");
        let names: Vec<&str> = got.iter().map(|(q, _)| q.as_str()).collect();
        assert_eq!(
            names,
            [
                "evm_call:Escrow.arbiter",
                "evm_call:Escrow.deposit",
                "evm_call:Escrow.release",
                "evm_event:Escrow.Released",
            ],
            "`fmt.amount` and the ethers member `on` are no clients"
        );
        for (q, hits) in &got {
            assert_eq!(hits.len(), 1, "{q}");
            assert_eq!(hits[0].facts.lib, "typechain", "{q}");
            assert_eq!(
                hits[0].abi_from.as_deref(),
                Some("web/api/contracts.ts"),
                "{q}"
            );
        }
        let deposit = &got[1].1[0];
        assert_eq!(deposit.facts.args, vec![2], "the call's argument count");
        let edges = edges_of(&parses, "web/src/app.ts");
        for client in [
            "evm_call:Escrow.arbiter",
            "evm_call:Escrow.deposit",
            "evm_call:Escrow.release",
        ] {
            assert!(
                edges.contains(&(
                    "web::src::app::checkout".into(),
                    edge_category::USES,
                    client.into()
                )),
                "{client}: the calling function owns it (A5.8): {edges:?}"
            );
        }
        assert!(edges.contains(&(
            "evm_event:Escrow.Released".into(),
            edge_category::HANDLED_BY,
            "web::src::app::checkout".into()
        )));
        let fp = parses
            .values()
            .flatten()
            .find(|fp| http_owner::module_file(fp).as_deref() == Some("web/src/app.ts"))
            .expect("app");
        let deposit = fp
            .nodes
            .iter()
            .find(|n| {
                fp.nav.qname_by_id.get(&n.id).map(String::as_str) == Some("evm_call:Escrow.deposit")
            })
            .expect("deposit");
        let kinds: Vec<_> = deposit.cells.iter().map(|c| c.kind).collect();
        assert_eq!(
            kinds,
            [
                cell_type::ENDPOINT_HIT,
                cell_type::POSITION,
                cell_type::IMPORTS
            ],
            "an in-file client's cell order"
        );
        // A second run (a held cache replays the parse) adds nothing.
        let nodes = fp.nodes.len();
        let edges = fp.edges.len();
        join_evm_abis(&mut parses, &TsAliasSet::default());
        let fp = parses
            .values()
            .flatten()
            .find(|fp| http_owner::module_file(fp).as_deref() == Some("web/src/app.ts"))
            .expect("app");
        assert_eq!((fp.nodes.len(), fp.edges.len()), (nodes, edges));
    }

    #[test]
    fn ts_imported_instance_through_a_barrel_and_a_workspace_package() {
        let dir = tempfile::tempdir().expect("tempdir");
        let write = |rel: &str, text: &str| {
            let p = dir.path().join(rel);
            std::fs::create_dir_all(p.parent().expect("parent")).expect("dir");
            std::fs::write(p, text).expect("write");
        };
        write("package.json", r#"{"name":"sibyl","private":true}"#);
        write(
            "pnpm-workspace.yaml",
            "packages:\n  - 'apps/*'\n  - 'packages/*'\n",
        );
        write(
            "packages/api-client/package.json",
            r#"{"name":"@sibyl/api-client","main":"src/index.ts","types":"src/index.ts"}"#,
        );
        write(
            "apps/web/package.json",
            r#"{"name":"@sibyl/web","dependencies":{"@sibyl/api-client":"workspace:*"}}"#,
        );
        let roots: Vec<ProjectRoot> = ["", "apps/web", "packages/api-client"]
            .iter()
            .map(|r| ProjectRoot::new((*r).to_string(), "npm", "package.json", None))
            .collect();
        let app = APP_TS.replace("SPEC", "@sibyl/api-client");
        let files = [
            ("apps/web/src/app.ts", app.as_str()),
            ("packages/api-client/src/contracts.ts", CONTRACTS_TS),
            (
                "packages/api-client/src/index.ts",
                "export * from './contracts';\nexport * from './typechain-types';\n",
            ),
        ];
        for (path, src) in files {
            write(path, src);
        }
        let aliases = TsAliasSet::read(dir.path(), &roots, "fx");
        let mut parses = parsed(&files);
        let stats = join_evm_abis(&mut parses, &aliases);
        assert_eq!(stats.instances, instance_stats(1, 1, 3, 1, 0));
        let got = clients_of(&parses, "apps/web/src/app.ts");
        assert_eq!(got.len(), 4);
        assert_eq!(
            got[0].1[0].abi_from.as_deref(),
            Some("packages/api-client/src/contracts.ts")
        );
        // Without the workspace packages the bare specifier names nothing.
        let mut bare = parsed(&[
            ("apps/web/src/app.ts", &app),
            ("packages/api-client/src/contracts.ts", CONTRACTS_TS),
        ]);
        let stats = join_evm_abis(&mut bare, &TsAliasSet::default());
        assert_eq!(stats.instances, instance_stats(1, 0, 0, 0, 0));
    }

    #[test]
    fn ts_imported_instance_through_a_tsconfig_alias_and_a_namespace() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("tsconfig.json"),
            r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@chain/*": ["web/chain/*"] } } }"#,
        )
        .expect("tsconfig");
        let aliases = TsAliasSet::read(dir.path(), &[], "fx");
        let app = APP_TS.replace("SPEC", "@chain/contracts");
        let ns = "import * as chain from '../chain/contracts';
export function pay(p: string) {
  return chain.escrow.deposit(p);
}
";
        let mut parses = parsed(&[
            ("web/src/app.ts", &app),
            ("web/src/ns.ts", ns),
            ("web/chain/contracts.ts", CONTRACTS_TS),
        ]);
        let stats = join_evm_abis(&mut parses, &aliases);
        assert_eq!(stats.instances, instance_stats(1, 2, 4, 1, 0));
        let ns_clients: Vec<String> = clients_of(&parses, "web/src/ns.ts")
            .into_iter()
            .map(|(q, h)| format!("{q} args={:?}", h[0].facts.args))
            .collect();
        assert_eq!(ns_clients, ["evm_call:Escrow.deposit args=[1]"]);
    }

    #[test]
    fn ts_importer_with_its_own_contract_and_the_exporters_sites_mint_once() {
        let exporter = format!(
            "{CONTRACTS_TS}export async function warm() {{\n  return escrow.deposit(ADDR);\n}}\n"
        );
        let app = "import { Contract } from 'ethers';
import { escrow } from './contracts';
const VAULT_ABI = ['function withdraw(uint256 a)'];
export async function pay(signer: any) {
  const vault = new Contract(ADDR, VAULT_ABI, signer);
  await vault.withdraw(1);
  await escrow.deposit(ADDR);
}
";
        let mut parses = parsed(&[("web/app.ts", app), ("web/contracts.ts", &exporter)]);
        let stats = join_evm_abis(&mut parses, &TsAliasSet::default());
        assert_eq!(stats.instances, instance_stats(1, 1, 1, 0, 0));
        let got: Vec<String> = clients_of(&parses, "web/app.ts")
            .into_iter()
            .map(|(q, h)| format!("{q} cells={}", h.len()))
            .collect();
        assert_eq!(
            got,
            [
                "evm_call:abi:VAULT_ABI.withdraw cells=1",
                "evm_call:Escrow.deposit cells=1"
            ],
            "the file's own contract once, the imported one once"
        );
        let own: Vec<String> = clients_of(&parses, "web/contracts.ts")
            .into_iter()
            .map(|(q, h)| format!("{q} from={:?}", h[0].abi_from))
            .collect();
        assert_eq!(
            own,
            ["evm_call:Escrow.deposit from=None"],
            "the exporter's own site is the in-file reader's, unchanged"
        );
    }

    #[test]
    fn ts_import_of_no_instance_or_a_reassigned_one_mints_nothing() {
        let abi_only = "export const SHOP_ABI = ['function release(uint256 id)'];\n";
        let app = "import { fmt, escrow } from './contracts';
import { SHOP_ABI } from './abi';
export function f() {
  fmt.amount(1n);
  SHOP_ABI.release(1);
}
";
        let shadow = "import { escrow } from './contracts';
export function g() {
  let escrow = other();
  escrow.deposit(1);
}
";
        let mut parses = parsed(&[
            ("web/app.ts", app),
            ("web/shadow.ts", shadow),
            ("web/abi.ts", abi_only),
            ("web/contracts.ts", CONTRACTS_TS),
        ]);
        let stats = join_evm_abis(&mut parses, &TsAliasSet::default());
        assert_eq!(
            stats.instances,
            instance_stats(1, 0, 0, 0, 1),
            "a plain object, an ABI const: no instance; a redeclared local: unresolved"
        );
        assert!(clients_of(&parses, "web/app.ts").is_empty());
        assert!(clients_of(&parses, "web/shadow.ts").is_empty());
    }

    #[test]
    fn ts_instance_with_an_abi_drops_members_it_does_not_list() {
        let exporter = "import { Contract } from 'ethers';
import { SHOP_ABI } from './abi';
export const shop = new Contract(ADDR, SHOP_ABI, null);
";
        let app = "import { shop } from './shop';
export async function f() {
  await shop.release(1);
  await shop.withdraw(1);
}
";
        let mut parses = parsed(&[
            ("web/app.ts", app),
            ("web/shop.ts", exporter),
            (
                "web/abi.ts",
                "export const SHOP_ABI = ['function release(uint256 id)'];\n",
            ),
        ]);
        let stats = join_evm_abis(&mut parses, &TsAliasSet::default());
        assert_eq!(stats.instances, instance_stats(1, 1, 1, 0, 0));
        let got = clients_of(&parses, "web/app.ts");
        assert_eq!(got.len(), 1, "withdraw is not in the ABI: {got:?}");
        assert_eq!(got[0].0, "evm_call:abi:SHOP_ABI.release");
        let hit = &got[0].1[0];
        assert_eq!(hit.facts.abi, Some(vec!["f:release/1".to_string()]));
        assert_eq!(
            hit.abi_from.as_deref(),
            Some("web/abi.ts"),
            "the ABI's own file"
        );
        assert_eq!(hit.abi_ref, Some(ts_ref("SHOP_ABI", "./abi")));
    }

    #[test]
    fn instances_marker_fires_only_with_an_export() {
        let none = EvmAbiJoinStats::default();
        assert_eq!(none.instances.exported, 0);
        let mut parses = parsed(&[("web/app.ts", &APP_TS.replace("SPEC", "./contracts"))]);
        let stats = join_evm_abis(&mut parses, &TsAliasSet::default());
        assert_eq!(stats.instances, InstanceStats::default());
    }
}
