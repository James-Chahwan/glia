//! The per-language graph build of `build_graphs_for_repo`, as three stages
//! (DC.2): [`plan_groups`] (a deterministic group order: sorted languages,
//! the JVM join, the TS family last), [`build_groups`] (each group built on
//! the engine pool, or taken from a per-group reuse slot) and [`fold_groups`]
//! (the graphs and the per-repo markers, in plan order). The `build_*`
//! dispatch, one shared TS-family graph (built by `build_typescript_family`,
//! CH.1b; the `_` arm's languages keep `build_typescript`), and the import
//! resolvers those arms take (relative, plus the A6.8 tsconfig `paths`
//! aliases for the TS family; the own-package `package:` URIs of
//! [`DartPackages`] for Dart, DF.4a) and the includer-relative step of the
//! `#include` resolver `build_c_cpp` takes (its search roots are
//! [`super::c_includes`], CB.22).

use std::cell::Cell;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use glia_code_domain::project_roots::{LABEL_MANIFEST, ProjectRoot};
use glia_code_domain::{FileParse, cell_type, edge_category, evidence, node_kind, recv_stats};
use glia_core::{EdgeCategoryId, NodeId, RepoId};
use glia_graph::rust_paths::RustCrate;
use glia_graph::{GraphError, RepoGraph};

use crate::extract::{TS_FAMILY, build_group, path_to_qname};
use crate::passes::{GroupFill, fill_group_evidence};
use crate::walk::{WorkspacePackage, package_file, workspace_packages};

use super::c_includes::{IncludeRoots, angle_includes, c_cpp_module_qnames};

// TS-family lang tags (typescript/angular/react/vue, `extract::TS_FAMILY`)
// share ONE module + symbol space in a repo: an Angular component
// (`.component.ts` → "angular") injects a service (`.service.ts` →
// "typescript"), and imports cross those tags. They build as a single graph
// (their `extract::build_group` is "typescript") so intra-repo ref/import
// resolution works across the tag boundary (Pattern E DI, Pattern B imports).
// The `c_cpp`, `swift` (CB.18) and `dart` (DF.4a) arms and the `_`-arm langs
// (solidity/terraform) keep separate graphs — distinct symbol spaces that
// must not cross-resolve.
// ts_family accumulates in the sorted lang order and is the last pooled build
// (CA.7), so graph/shard order stays deterministic. The LB.9b module plan
// reads the same `build_group`.

/// A14.2: the JVM family. `.kt` parses under its own `kotlin` tag (its own
/// parser), but Kotlin and Java share one symbol space — a Kotlin controller
/// imports and injects a Java service and back — and resolution only happens
/// inside one graph, so both build as ONE `build_dotted` graph. Not TS_FAMILY's
/// build-last pattern: the synthetic `json` tag sorts between `java` and
/// `kotlin`, and building the family after the loop would move every Java
/// repo's graph to the end of `graphs`, shifting its shard index. The Kotlin
/// parses join the `java` entry in place instead (a Kotlin-only repo builds at
/// the `kotlin` slot), so a Java-only repo's graph list is untouched.
const JVM_HOST: &str = "java";
const JVM_GUEST: &str = "kotlin";

/// The TS family's build group ([`build_group`]), and the key its one pooled
/// build carries (CA.7).
const TS_GROUP: &str = "typescript";

/// One build group of a repo ([`plan_groups`]): the parses one graph is built
/// from, after every post-cache graft.
pub(crate) struct GroupInput {
    /// The plan key: the language tag (`java` for a JVM family that holds
    /// Java, so its Kotlin parses ride along), or [`TS_GROUP`] for the TS
    /// family. Unique within a plan; [`LangBuild::key`] of its build.
    pub(crate) key: &'static str,
    pub(crate) parses: Vec<FileParse>,
}

/// One repo's build groups in clean-build order, and the per-language marker
/// inputs counted off the parses before the JVM join ([`plan_groups`]).
pub(crate) struct GroupPlan {
    /// Sorted by language with the Kotlin parses joined into `java` in place
    /// (A14.2) and the TS family pushed last (CA.7). This order is the graph
    /// order, so the shard index: a caller that rebuilds some groups keeps it.
    pub(crate) groups: Vec<GroupInput>,
    /// The A7.0 `[di]` marker input: INJECTS refs per matrix row.
    pub(crate) di_refs: Vec<(&'static str, usize)>,
    /// The A6.2a `[recv]` marker input: declared field types per matrix row.
    pub(crate) recv_fields: Vec<(&'static str, usize)>,
}

/// What a group build reads besides its parses: the repo-level inputs a build
/// consults after the parse cache, so a cached parse needs no key for them.
#[derive(Clone, Copy)]
pub(crate) struct GroupCtx<'a> {
    pub(crate) repo: RepoId,
    /// Labels the build's markers (`[c-includes]`, `[groups]`); never reaches
    /// the graph.
    pub(crate) repo_label: &'a str,
    /// The Cargo packages ([`rust_crates`]), `build_rust`'s path resolver
    /// input (LA.1a).
    pub(crate) rust_crates: &'a [RustCrate],
    /// The TS family's import resolver aliases ([`resolve_ts_source_aliased`],
    /// A6.8).
    pub(crate) ts_aliases: &'a TsAliasSet,
    /// The C/C++ include search roots (CB.22, [`IncludeRoots::resolver`]).
    pub(crate) c_includes: &'a IncludeRoots,
    /// The repo's own Dart packages, the Dart import resolver's input
    /// ([`resolve_dart_source`], DF.4a).
    pub(crate) dart_packages: &'a DartPackages,
    /// FD.81: the repo's go.mod root dirs (`super::go_module_dirs`),
    /// `glia_graph::build_go_with`'s input: implicit IMPLEMENTS stays inside
    /// one Go module.
    pub(crate) go_module_dirs: &'a [String],
}

/// What [`fold_groups`] hands back.
pub(crate) struct LanguageGraphs {
    /// The per-group graphs, in plan order (sorted languages, the TS family
    /// last). A failed build leaves a `parse_errors` line and no graph.
    pub(crate) graphs: Vec<RepoGraph>,
    /// The plan key ([`GroupInput::key`]) of each graph in `graphs`, index
    /// for index: how a caller maps a graph back to its group.
    pub(crate) keys: Vec<&'static str>,
    /// DC.5: each graph's evidence fill, decided at its group build
    /// ([`fill_group_evidence`]), index for index with `graphs`: the build
    /// tail applies them in its Finalize fill (`CodeBuildCtx::group_fills`).
    pub(crate) fills: Vec<GroupFill>,
    /// LG.1c: the groups mapped on the engine pool, for the `[parallel]`
    /// line: every build group, the TS family's one graph included (CA.7).
    pub(crate) pooled: usize,
}

/// Stage 1 of a repo's language build: its finished (post-graft) parses as
/// build groups in clean-build order ([`GroupPlan`]). Prints the A14.2
/// `[kotlin] entities:` line for `repo_label`.
pub(crate) fn plan_groups(
    parses_by_lang: HashMap<&'static str, Vec<FileParse>>,
    repo_label: &str,
) -> GroupPlan {
    // Deterministic per-language build order: HashMap iteration is seeded per
    // process, and the resulting `graphs` order decides shard indices in
    // `write_sharded` (repo-<hash>-NN.gmap). Random order made every shard's
    // content hash flap across processes, so the write-side skip-unchanged-
    // shards optimization never fired (audit 2026-06-10 #5).
    let mut parses_by_lang: Vec<(&'static str, Vec<FileParse>)> =
        parses_by_lang.into_iter().collect();
    parses_by_lang.sort_unstable_by_key(|(lang, _)| *lang);
    // A14.2 `[kotlin] entities:` fired_on, counted off the parses so
    // cache-served files count too; the kotlin crate owns the line.
    if let Some((_, kotlin)) = parses_by_lang.iter().find(|(lang, _)| *lang == JVM_GUEST) {
        glia_parser_kotlin::trace(kotlin, repo_label);
    }
    // A7.0 `[di]` marker input: INJECTS refs per language, counted off the
    // parses themselves so cache-served files count too. The TS-family tags
    // report as `typescript`, their matrix row.
    let di_refs: Vec<(&str, usize)> = parses_by_lang
        .iter()
        .map(|(lang, parses)| {
            let n = parses
                .iter()
                .flat_map(|fp| &fp.refs)
                .filter(|r| r.category == edge_category::INJECTS)
                .count();
            (matrix_row(lang), n)
        })
        .collect();
    // A6.2a `[recv]` marker input: declared field types per matrix row, counted
    // off the parses (cache-served files count too), and the receiver-typed
    // binds `resolve_calls` records, taken after each language's build. The
    // generic pass does not know its language, hence the take-per-build.
    let recv_fields: Vec<(&str, usize)> = parses_by_lang
        .iter()
        .map(|(lang, parses)| {
            let n = parses
                .iter()
                .flat_map(|fp| fp.nav.field_types.values())
                .map(HashMap::len)
                .sum();
            (matrix_row(lang), n)
        })
        .collect();
    // The per-language marker inputs above keep `kotlin` as its own row; the
    // build below sees the JVM family as one graph.
    join_jvm_family(&mut parses_by_lang);
    let mut ts_family: Vec<FileParse> = Vec::new();
    let mut groups: Vec<GroupInput> = Vec::new();
    for (lang, parses) in parses_by_lang {
        if build_group(lang) == TS_GROUP {
            ts_family.extend(parses);
        } else {
            groups.push(GroupInput { key: lang, parses });
        }
    }
    // CA.7: the TS family is the LAST group; `par_map_owned` hands the
    // results back in input order, so it still folds last. Its key is the
    // build group, not a language tag of `groups` (no tag of the family
    // reaches the loop's `else` arm), and it takes the aliased resolver.
    if !ts_family.is_empty() {
        groups.push(GroupInput {
            key: TS_GROUP,
            parses: ts_family,
        });
    }
    GroupPlan {
        groups,
        di_refs,
        recv_fields,
    }
}

/// Stage 2: one [`LangBuild`] per group of a plan, in plan order. `reuse` is
/// aligned with `groups`: a `Some` whose [`LangBuild::key`] is its group's
/// key is taken as built; every other group (a `None`, a missing entry, a
/// reuse of another key) is built by [`build_one_group`]. A clean build
/// passes no reuse. A reused build is the caller's copy of a build made from
/// equal inputs, so the result is the clean build's either way.
///
/// LG.1c: the groups are mapped on the engine pool
/// ([`crate::parallel::par_map_owned`]), the TS family last (CA.7). The
/// per-build lines (`[evidence-lines]`, `[c-includes]` and the graph crate's
/// own) print on the thread that ran the build, right after it: under
/// `GLIA_THREADS=1` in the sequential order, on a pool interleaved across
/// languages. A panicking build is re-raised here, the first in plan order,
/// as the inline build let it propagate.
///
/// fired_on marker, once per repo, after the builds:
///   `[groups] repo=<label> built=<b> reused=<r> keys=<k1,k2,..>`
/// `keys` are the built groups' keys in plan order (`-` when none); a clean
/// build prints `reused=0`.
pub(crate) fn build_groups(
    groups: Vec<GroupInput>,
    reuse: Vec<Option<LangBuild>>,
    ctx: &GroupCtx<'_>,
) -> Vec<LangBuild> {
    let mut reuse = reuse.into_iter();
    let items: Vec<(GroupInput, Option<LangBuild>)> = groups
        .into_iter()
        .map(|input| {
            let taken = reuse.next().flatten().filter(|b| b.key() == input.key);
            (input, taken)
        })
        .collect();
    let built: Vec<&'static str> = items
        .iter()
        .filter(|(_, taken)| taken.is_none())
        .map(|(input, _)| input.key)
        .collect();
    let reused = items.len() - built.len();
    let (builds, _threads) = crate::parallel::par_map_owned(items, |(input, taken)| match taken {
        Some(build) => Ok(build),
        None => crate::parallel::quiet(|| build_one_group(input, ctx)),
    });
    let builds: Vec<LangBuild> = builds
        .into_iter()
        .map(|build| build.unwrap_or_else(|payload| std::panic::resume_unwind(payload)))
        .collect();
    let keys = if built.is_empty() {
        "-".to_string()
    } else {
        built.join(",")
    };
    eprintln!(
        "[groups] repo={} built={} reused={reused} keys={keys}",
        ctx.repo_label,
        built.len()
    );
    builds
}

/// Build one group's graph on THIS thread: [`build_one`] over the TS family's
/// `build_typescript_family` (with the aliased import resolver) or the
/// language's own `build_*` ([`build_solo`]).
fn build_one_group(input: GroupInput, ctx: &GroupCtx<'_>) -> LangBuild {
    let GroupInput { key, parses } = input;
    if key == TS_GROUP {
        // FH.5: the graph builder hands its workspace counts back through
        // the resolver (`WORKSPACE_REPORT`), on this thread.
        let report: Cell<Option<(usize, usize, usize)>> = Cell::new(None);
        let build = build_one(key, parses, |parses| {
            glia_graph::build_typescript_family(ctx.repo, parses, |from, spec| {
                if from == WORKSPACE_REPORT {
                    report.set(parse_workspace_report(spec));
                    return None;
                }
                resolve_ts_source_aliased(from, spec, ctx.ts_aliases)
            })
        });
        if let Some(line) = ctx
            .ts_aliases
            .workspace
            .marker(report.get(), ctx.repo_label)
        {
            eprintln!("{line}");
        }
        build
    } else {
        build_one(key, parses, |parses| build_solo(key, parses, ctx))
    }
}

/// Stage 3: fold the builds, in plan order, into the repo's graphs
/// ([`LanguageGraphs`]); a failed build's line goes to `parse_errors` at its
/// fold position (a reused failed build replays its own line), so the graphs
/// and the error order are the clean build's. Prints the A6.2a `[recv]` and
/// A6.3 `[heritage]` markers for `repo_label`; `recv_fields` is the plan's.
pub(crate) fn fold_groups(
    builds: Vec<LangBuild>,
    recv_fields: &[(&'static str, usize)],
    parse_errors: &mut Vec<String>,
    repo_label: &str,
) -> LanguageGraphs {
    let pooled = builds.len();
    let mut graphs = Vec::new();
    let mut keys = Vec::new();
    let mut fills = Vec::new();
    let mut recv_bound: Vec<(&str, usize)> = Vec::new();
    let mut heritage: Vec<HeritageTally> = Vec::new();
    for build in builds {
        build.fold(
            &mut graphs,
            &mut keys,
            &mut fills,
            &mut recv_bound,
            &mut heritage,
            parse_errors,
        );
    }
    // A6.2a fired_on marker, once per repo, every `recv_stats::LANGS` row
    // zero-filled (LA.35a adds `rust`: bound = the field- and local-typed
    // binds of `build_rust`, fields = the Rust parses' struct field types):
    //   `[recv] receiver-typed calls bound: csharp=N … rust=N (fields: csharp=F … rust=F) repo=<label>`
    recv_stats::flush_marker(&recv_bound, recv_fields, repo_label);
    // A6.3 fired_on marker, once per repo (A6.4 / A6.5 read their rows here):
    //   `[heritage] refs bound: dart=N … typescript=N unresolved: dart=M … repo=<label>`
    if let Some(line) = heritage_marker(&mut heritage, repo_label) {
        eprintln!("{line}");
    }

    LanguageGraphs {
        graphs,
        keys,
        fills,
        pooled,
    }
}

/// One non-TS build group's graph: the `build_*` its language takes, with
/// the repo-level inputs of `ctx` it reads. The C/C++ build prints CB.22's
/// `[c-includes]` line after it ([`IncludeRoots::marker`]), on the thread
/// that ran it. Dart builds through `build_dart` (DT.5h: the shared passes
/// plus its member reads) and takes [`resolve_dart_source`] (DF.4a);
/// Solidity, Terraform and every other `_`-arm language keep
/// `build_typescript` and [`resolve_relative_source`].
fn build_solo(
    lang: &'static str,
    parses: Vec<FileParse>,
    ctx: &GroupCtx<'_>,
) -> Result<RepoGraph, GraphError> {
    let GroupCtx {
        repo,
        repo_label,
        rust_crates,
        c_includes,
        dart_packages,
        go_module_dirs,
        ..
    } = *ctx;
    match lang {
        "python" => glia_graph::build_python(repo, parses),
        "go" => glia_graph::build_go_with(repo, parses, go_module_dirs),
        "java" | "kotlin" | "csharp" | "php" | "scala" | "clojure" => {
            glia_graph::build_dotted(repo, parses)
        }
        "elixir" => glia_graph::build_elixir(repo, parses),
        "rust" => glia_graph::build_rust(repo, parses, rust_crates),
        "ruby" => glia_graph::build_ruby(repo, parses),
        "lua" => glia_graph::build_lua(repo, parses),
        "c_cpp" => {
            let modules = c_cpp_module_qnames(&parses);
            let angle = angle_includes(&parses);
            let resolver = c_includes.resolver(&modules);
            let graph =
                glia_graph::build_c_cpp(repo, parses, |from, spec| resolver.resolve(from, spec));
            eprintln!(
                "{}",
                c_includes.marker(angle, resolver.bound_via_roots(), repo_label)
            );
            graph
        }
        "swift" => glia_graph::build_swift(repo, parses, resolve_relative_source),
        "dart" => glia_graph::build_dart(repo, parses, |from, spec| {
            resolve_dart_source(from, spec, dart_packages)
        }),
        _ => glia_graph::build_typescript(repo, parses, resolve_relative_source),
    }
}

/// One group build's result, made on the thread that ran the build and
/// folded into the repo's lists by [`fold_groups`], in plan order. `Clone`
/// so a caller can keep a pre-pass copy of a group for reuse
/// ([`build_groups`]); the clean build never clones one (the fold moves it).
#[derive(Clone)]
pub(crate) struct LangBuild {
    /// The plan key ([`GroupInput::key`]).
    lang: &'static str,
    /// The graph with its `graph:build` evidence stamped, its `[heritage]`
    /// tally and its evidence fill decided against its own nodes (DC.5,
    /// [`fill_group_evidence`]: cached here, so a reused group is never
    /// re-filled); or the `parse_errors` line of a failed build.
    graph: Result<(RepoGraph, HeritageTally, GroupFill), String>,
    /// The receiver-typed binds `recv_stats` counted during the build.
    recv_bound: usize,
}

impl LangBuild {
    /// The plan key of the group this build was made from.
    pub(crate) fn key(&self) -> &'static str {
        self.lang
    }

    /// A failed build of group `key` whose `parse_errors` line is `line`
    /// (DC.7b's `watch::tests::cached_error_keeps_clean_order` caches one: no
    /// `build_*` returns a `GraphError` today, so a test cannot get one from a
    /// real build).
    #[cfg(test)]
    pub(crate) fn failed(key: &'static str, line: &str) -> Self {
        Self {
            lang: key,
            graph: Err(line.to_string()),
            recv_bound: 0,
        }
    }

    /// Append this build to the repo's lists: the sequential loop's
    /// per-group step.
    fn fold(
        self,
        graphs: &mut Vec<RepoGraph>,
        keys: &mut Vec<&'static str>,
        fills: &mut Vec<GroupFill>,
        recv_bound: &mut Vec<(&'static str, usize)>,
        heritage: &mut Vec<HeritageTally>,
        parse_errors: &mut Vec<String>,
    ) {
        recv_bound.push((self.lang, self.recv_bound));
        match self.graph {
            Ok((g, h, fill)) => {
                heritage.push(h);
                graphs.push(g);
                keys.push(self.lang);
                fills.push(fill);
            }
            Err(e) => parse_errors.push(e),
        }
    }
}

/// Run one language's graph build on THIS thread, tally it and print its
/// LC.3b `[evidence-lines]` line, then decide its evidence fill (DC.5,
/// [`fill_group_evidence`]; after the tally, which counts basis `site` before
/// the fill). `recv_stats` is per thread (LG.1c) and the graph crate counts
/// on the build's own thread only (FH.21: a pooled pass records its chunks'
/// binds in its apply step, and [`EngineResolvePool`] keeps the count across
/// a build rayon runs on this thread while it waits), so the `reset` ..
/// `take` bracket holds exactly this build's receiver-typed binds. FH.21:
/// the build runs under [`EngineResolvePool`] when more than one worker can
/// take its chunks.
fn build_one(
    lang: &'static str,
    parses: Vec<FileParse>,
    build: impl FnOnce(Vec<FileParse>) -> Result<RepoGraph, GraphError>,
) -> LangBuild {
    let unattributed = unattributed_parse_edges(&parses);
    recv_stats::reset();
    let graph = if EngineResolvePool::threads_here() > 1 {
        glia_graph::with_resolve_pool(std::sync::Arc::new(EngineResolvePool), || build(parses))
    } else {
        build(parses)
    };
    let recv_bound = recv_stats::take();
    let graph = match graph {
        Ok(mut g) => {
            stamp_graph_edges(&mut g, unattributed);
            let h = HeritageTally::of(lang, &g);
            if let Some(line) = LineTally::of(&g).marker(lang) {
                eprintln!("{line}");
            }
            let fill = fill_group_evidence(&g);
            Ok((g, h, fill))
        }
        Err(e) => Err(format!("{lang} graph: {e}")),
    };
    LangBuild {
        lang,
        graph,
        recv_bound,
    }
}

/// FH.21: the engine pool as the graph crate's [`glia_graph::ResolvePool`]:
/// a language build's call / ref passes decide their sites in chunks through
/// [`crate::parallel::par_map_ordered`], which runs on the pool the build
/// itself runs on (a nested map on its worker) or on the engine pool, so one
/// large group (a TypeScript monorepo) no longer resolves on one core while
/// the others idle. [`build_one`] installs it only where more than one
/// worker can run ([`Self::threads_here`]); under `GLIA_THREADS=1` the passes
/// run as one chunk on the build's thread, as before.
///
/// While this thread waits for the chunks, rayon may run another queued job
/// on it, which can be another group's whole [`build_one`]: its `recv_stats`
/// reset .. take bracket would eat this build's count so far. [`Self::run`]
/// takes the count before the wait and puts it back after, and the graph
/// crate counts a chunk's receiver binds on the build's thread, never on the
/// worker that decided them. A chunk's panic is caught quietly on its
/// worker ([`crate::parallel::quiet`]) and re-raised here, on the build's
/// thread, inside the build's own quiet scope.
struct EngineResolvePool;

impl EngineResolvePool {
    /// The workers a map from this thread runs on: the current rayon pool's
    /// on one of its workers, else the engine pool's ([`crate::parallel::threads`]).
    fn threads_here() -> usize {
        if rayon::current_thread_index().is_some() {
            rayon::current_num_threads()
        } else {
            crate::parallel::threads()
        }
    }
}

impl glia_graph::ResolvePool for EngineResolvePool {
    fn threads(&self) -> usize {
        Self::threads_here()
    }

    fn run(&self, n: usize, job: &(dyn Fn(usize) + Sync)) {
        let saved = recv_stats::take();
        let jobs: Vec<usize> = (0..n).collect();
        let (results, _) =
            crate::parallel::par_map_ordered(&jobs, |&i| crate::parallel::quiet(|| job(i)));
        // Whatever a build run on this thread during the wait left behind
        // is its own (it took it); this build's count is `saved`.
        recv_stats::reset();
        for _ in 0..saved {
            recv_stats::record();
        }
        for result in results {
            if let Err(payload) = result {
                std::panic::resume_unwind(payload);
            }
        }
    }
}

/// An edge's identity without its cells ([`glia_core::Edge::key`]).
type EdgeKey = (NodeId, NodeId, EdgeCategoryId);

/// LC.3a: the keys, with multiplicity, of the parse edges that reach the graph
/// build with no EVIDENCE cell. Every stage before the build stamps its own
/// edges, so this is empty unless an emitter went unattributed. Only looked
/// up, never iterated.
fn unattributed_parse_edges(parses: &[FileParse]) -> HashMap<EdgeKey, usize> {
    let mut keys: HashMap<EdgeKey, usize> = HashMap::new();
    for e in parses.iter().flat_map(|fp| &fp.edges) {
        if !e.cells.iter().any(|c| c.kind == cell_type::EVIDENCE) {
            *keys.entry(e.key()).or_default() += 1;
        }
    }
    keys
}

/// LC.3a: stamp `graph:build` on the edges the graph build itself added
/// (resolved calls, refs, imports; LC.3d names each mechanism). An edge that
/// came in from a parse unstamped (`unattributed`, matched by key and
/// multiplicity) is NOT filed under the graph stage: it stays without
/// evidence, so the fill pass counts it `missing` and the corpus test names it.
fn stamp_graph_edges(g: &mut RepoGraph, mut unattributed: HashMap<EdgeKey, usize>) {
    if unattributed.is_empty() {
        evidence::stamp_missing(&mut g.edges, "graph:build");
        return;
    }
    let ev = evidence::Evidence::emitter("graph:build");
    for e in &mut g.edges {
        if e.cells.iter().any(|c| c.kind == cell_type::EVIDENCE) {
            continue;
        }
        if let Some(n) = unattributed.get_mut(&e.key())
            && *n > 0
        {
            *n -= 1;
            continue;
        }
        evidence::attach(e, ev.clone());
    }
}

/// The evidence of an edge bound from an `UnresolvedRef`, by
/// `(emitter, rule)` (`None`: any rule): `resolve_refs`, the nav link
/// resolver, Rust's leftover enum-variant refs and Go's interface embeds.
const REF_EMITTERS: &[(&str, Option<&str>)] = &[
    ("graph:refs", None),
    ("graph:nav", None),
    ("graph:rust_paths", Some("enum_variant")),
    ("graph:go_packages", Some("embed_package")),
    ("graph:go_packages", Some("embed_import")),
];

/// LC.3b: one built language graph's `[evidence-lines]` counts. `calls`:
/// CALLS edges into a declaration (a call-site-shaped target, an ENDPOINT
/// and the other [`evidence::SITE_KINDS`], is located at itself instead);
/// `imports`: IMPORTS edges; `refs`: edges bound from an `UnresolvedRef`
/// ([`REF_EMITTERS`]). Each `*_site` is how many carry the asserting
/// construct's own line (basis `site`), so a site count below its total names
/// an emitter that dropped the line.
#[derive(Debug, Default, PartialEq, Eq)]
struct LineTally {
    calls: usize,
    calls_site: usize,
    imports: usize,
    imports_site: usize,
    refs: usize,
    refs_site: usize,
}

impl LineTally {
    fn of(g: &RepoGraph) -> Self {
        let mut t = LineTally::default();
        for e in &g.edges {
            let ev = evidence::Evidence::of(e);
            let site = ev
                .as_ref()
                .is_some_and(|ev| ev.basis == evidence::Basis::Site);
            let (total, sited) = if e.category == edge_category::CALLS {
                let to_site_kind = g
                    .nav
                    .kind_by_id
                    .get(&e.to)
                    .is_some_and(|k| evidence::SITE_KINDS.contains(k));
                if to_site_kind {
                    continue;
                }
                (&mut t.calls, &mut t.calls_site)
            } else if e.category == edge_category::IMPORTS {
                (&mut t.imports, &mut t.imports_site)
            } else if ev.as_ref().is_some_and(|ev| {
                REF_EMITTERS.iter().any(|(emitter, rule)| {
                    ev.emitter == *emitter && rule.is_none_or(|r| ev.rule.as_deref() == Some(r))
                })
            }) {
                (&mut t.refs, &mut t.refs_site)
            } else {
                continue;
            };
            *total += 1;
            *sited += usize::from(site);
        }
        t
    }

    /// LC.3b fired_on, once per language graph built:
    /// `[evidence-lines] lang=<lang> calls=c site=cs imports=i site=is refs=r site=rs`.
    /// `None` for a graph with no such edge.
    fn marker(&self, lang: &str) -> Option<String> {
        (self.calls + self.imports + self.refs > 0).then(|| {
            format!(
                "[evidence-lines] lang={lang} calls={} site={} imports={} site={} refs={} site={}",
                self.calls,
                self.calls_site,
                self.imports,
                self.imports_site,
                self.refs,
                self.refs_site
            )
        })
    }
}

/// One built language graph's A6.3 `[heritage]` marker input.
///
/// `bound` counts TYPE-level INHERITS_FROM / IMPLEMENTS edges whose target is
/// a node of the graph: an edge into a parser-fabricated id (the pre-A6.3 TS
/// shape, `-> '?'`) counts nowhere, and the METHOD -> METHOD IMPLEMENTS edges
/// `emit_method_level_implements` derives from a bound pair are not refs.
/// `unresolved` counts the heritage refs `resolve_refs` left unbound (an
/// external base: `extends Component`, `implements OnInit`). The graph build
/// does not know its language, hence the tally per build here.
#[derive(Clone)]
struct HeritageTally {
    lang: &'static str,
    bound: usize,
    unresolved: usize,
}

impl HeritageTally {
    fn of(lang: &'static str, g: &RepoGraph) -> Self {
        let bound = g
            .edges
            .iter()
            .filter(|e| {
                is_heritage(e.category)
                    && g.nav.kind_by_id.contains_key(&e.to)
                    && g.nav.kind_by_id.get(&e.from) != Some(&node_kind::METHOD)
            })
            .count();
        let unresolved = g
            .unresolved_refs
            .iter()
            .filter(|r| is_heritage(r.category))
            .count();
        HeritageTally {
            lang,
            bound,
            unresolved,
        }
    }
}

fn is_heritage(category: glia_core::EdgeCategoryId) -> bool {
    category == edge_category::INHERITS_FROM || category == edge_category::IMPLEMENTS
}

/// `[heritage] refs bound: <lang>=N … unresolved: <lang>=M … repo=<label>`:
/// one token per language graph built, sorted by name, in both halves. None
/// when no graph holds a bound heritage edge or an unbound heritage ref.
fn heritage_marker(tallies: &mut [HeritageTally], repo_label: &str) -> Option<String> {
    if tallies.iter().all(|t| t.bound == 0 && t.unresolved == 0) {
        return None;
    }
    tallies.sort_by_key(|t| t.lang);
    let bound: Vec<String> = tallies
        .iter()
        .map(|t| format!("{}={}", t.lang, t.bound))
        .collect();
    let unresolved: Vec<String> = tallies
        .iter()
        .map(|t| format!("{}={}", t.lang, t.unresolved))
        .collect();
    Some(format!(
        "[heritage] refs bound: {} unresolved: {} repo={repo_label}",
        bound.join(" "),
        unresolved.join(" ")
    ))
}

/// The Cargo packages among the walk's project roots, as `build_rust` reads
/// them (LA.1a): the package name as a path identifier (`-` -> `_`), its dir
/// as a qname prefix, its `src/lib.rs` and every other crate root it owns
/// (`src/main.rs`, `src/bin/*.rs`, `src/bin/*/main.rs`, `tests/*.rs`,
/// `examples/*.rs`, `benches/*.rs`, at exactly that depth) as module qnames.
/// A root with none of them (a `[workspace]`-only manifest) is skipped. Sorted
/// by (dir, name); `files` is the walk's list, so only walked files count.
pub(crate) fn rust_crates(files: &[(String, String)], roots: &[ProjectRoot]) -> Vec<RustCrate> {
    let mut crates = Vec::new();
    for root in roots.iter().filter(|r| r.ecosystem == "cargo") {
        let prefix = if root.rel_path.is_empty() {
            String::new()
        } else {
            format!("{}/", root.rel_path)
        };
        let mut lib_root = None;
        let mut other_roots = Vec::new();
        for (path, _) in files {
            let Some(rel) = path
                .strip_prefix(prefix.as_str())
                .filter(|r| r.ends_with(".rs"))
            else {
                continue;
            };
            let segs: Vec<&str> = rel.split('/').collect();
            match segs.as_slice() {
                ["src", "lib.rs"] => lib_root = Some(path_to_qname(path)),
                ["src", "main.rs"]
                | ["src", "bin", _]
                | ["src", "bin", _, "main.rs"]
                | ["tests" | "examples" | "benches", _] => other_roots.push(path_to_qname(path)),
                _ => {}
            }
        }
        if lib_root.is_none() && other_roots.is_empty() {
            continue;
        }
        other_roots.sort();
        let mut c = RustCrate::default();
        c.name = root.label.replace('-', "_");
        c.dir = root.rel_path.replace('/', "::");
        c.lib_root = lib_root;
        c.other_roots = other_roots;
        crates.push(c);
    }
    crates.sort_by(|a, b| (&a.dir, &a.name).cmp(&(&b.dir, &b.name)));
    crates
}

/// Move the `kotlin` parses onto the end of the `java` entry when both exist
/// (see [`JVM_HOST`]). `parses_by_lang` is sorted, so the Java entry keeps its
/// slot and the Kotlin one disappears; with no Java, nothing moves.
fn join_jvm_family(parses_by_lang: &mut Vec<(&'static str, Vec<FileParse>)>) {
    let Some(guest) = parses_by_lang
        .iter()
        .position(|(lang, _)| *lang != JVM_HOST && build_group(lang) == JVM_HOST)
    else {
        return;
    };
    let Some(host) = parses_by_lang
        .iter()
        .position(|(lang, _)| *lang == JVM_HOST)
    else {
        return;
    };
    let (_, kotlin) = parses_by_lang.remove(guest);
    // `host < guest` in sorted order, so the removal left `host` in place.
    parses_by_lang[host].1.extend(kotlin);
}

/// The matrix row a parse-language tag reports under: the TS-family tags
/// (angular / react / vue) are the `typescript` row.
fn matrix_row(lang: &str) -> &str {
    if TS_FAMILY.contains(&lang) {
        "typescript"
    } else {
        lang
    }
}

/// Resolve a TS/JS relative import specifier to the in-repo module qname it
/// targets — the inverse of `path_to_qname` (drop extension, `/`→`::`). Bare or
/// scoped specifiers (`@angular/core`, `lodash`) are external → None (no edge).
/// This is the resolver the engine previously stubbed with `|_,_| None`, which
/// is why NO TS/JS/Angular/React/Vue import ever became a category-3 IMPORTS
/// edge — imports lived only as `Symbol.imports` cells (handoff Pattern B).
fn resolve_ts_source(from_module: &str, specifier: &str) -> Option<String> {
    let spec = specifier.trim().trim_matches(|c| c == '"' || c == '\'');
    if !spec.starts_with('.') {
        return None; // external package — no intra-repo edge
    }
    // Directory of the importing module = its qname minus the final (file) segment.
    let mut segs: Vec<String> = from_module.split("::").map(String::from).collect();
    segs.pop();
    for part in spec.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                segs.pop();
            }
            p => {
                let p = p
                    .strip_suffix(".ts")
                    .or_else(|| p.strip_suffix(".tsx"))
                    .or_else(|| p.strip_suffix(".js"))
                    .or_else(|| p.strip_suffix(".jsx"))
                    .unwrap_or(p);
                segs.push(p.to_string());
            }
        }
    }
    if segs.is_empty() {
        return None;
    }
    Some(segs.join("::"))
}

// ---------------------------------------------------------------------------
// A6.8: tsconfig `paths` aliases
// ---------------------------------------------------------------------------

/// The files one project dir's aliases are read from, in order: the first
/// that declares `compilerOptions.paths` wins (`tsconfig.base.json` is Nx's
/// workspace-root convention).
const TSCONFIG_FILES: [&str; 2] = ["tsconfig.json", "tsconfig.base.json"];

/// A6.8: one tsconfig's `compilerOptions.paths`, as the TS-family import
/// resolver maps a non-relative specifier through it
/// ([`resolve_ts_source_aliased`]). A tsconfig never reaches the walk's file
/// list (the `.json` sniff does not admit it), so it is read off each project
/// dir, like a go.mod ([`TsAliasSet::read`]).
///
/// v1 limitations: only the FIRST target of each `paths` array is used, so a
/// repo relying on a fallback target does not bind (never mis-binds); an
/// `extends` chain is not followed (a file takes the aliases of the nearest
/// enclosing dir whose tsconfig declares `paths`, [`TsAliasSet::scope_of`]);
/// a `baseUrl` with no `paths` maps nothing. An alias naming a directory
/// maps to the directory's qname, and the graph builder binds it, like a
/// relative specifier, to the directory's index module when no same-stem
/// file exists (DT.1b, IMPORTS rule `dir_index`: `@core/models` ->
/// `src::app::core::models::index`; a file `models.ts` wins, as tsc orders
/// them).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct TsAliases {
    /// The tsconfig's dir, repo-relative, `/`-separated, `""` at the root.
    dir: String,
    /// The same dir as a module qname prefix, `""` at the root.
    dir_qname: String,
    /// The file the aliases came from ([`TSCONFIG_FILES`]).
    file: &'static str,
    /// `compilerOptions.baseUrl` as written, `.` when absent (TS 4.1+ then
    /// resolves `paths` against the tsconfig's own dir).
    base_url: String,
    /// `(key, first target)`, in match order: every wildcard-free key first
    /// (TS matches those exactly before any pattern), then the wildcard keys
    /// by longest literal prefix, then by key.
    entries: Vec<(String, String)>,
}

impl TsAliases {
    /// The module qname `spec` names through the first key that matches it,
    /// or `None`: no key matches, or the best match's target leaves the repo.
    fn map(&self, spec: &str) -> Option<String> {
        let target = self
            .entries
            .iter()
            .find_map(|(key, target)| match key.split_once('*') {
                None => (key == spec).then(|| target.clone()),
                Some((prefix, suffix)) => spec
                    .strip_prefix(prefix)
                    .and_then(|rest| rest.strip_suffix(suffix))
                    .map(|star| target.replacen('*', star, 1)),
            })?;
        ts_path_qname(&format!("{}/{}/{target}", self.dir, self.base_url))
    }

    /// The tsconfig's repo-relative path, for the marker.
    fn shown_path(&self) -> String {
        if self.dir.is_empty() {
            self.file.to_string()
        } else {
            format!("{}/{}", self.dir, self.file)
        }
    }
}

/// A6.8: every project dir's tsconfig `paths` of one repo: the repo root and
/// each walked project root (A8.4), so an Angular app under `web/` resolves
/// through `web/tsconfig.json`. Only dirs that declare `paths` are kept,
/// sorted by dir.
///
/// FH.5: it also holds the repo's workspace packages ([`WorkspaceSet`]), the
/// resolver's last step for a bare specifier no `paths` key maps, so a watch
/// session's group fingerprint (the set's `Debug`) moves with them.
///
/// FH.16: and every project dir's bundler config ([`BundlerScope`]): webpack
/// / vite `resolve.alias` entries and webpack `resolve.modules` roots, read
/// after the tsconfig `paths` and before the workspace packages.
#[derive(Debug, Clone, Default)]
pub(crate) struct TsAliasSet {
    scopes: Vec<TsAliases>,
    bundler: Vec<BundlerScope>,
    workspace: WorkspaceSet,
}

impl TsAliasSet {
    /// Read the aliases of the repo root and of every project root under
    /// `root` ([`read_tsconfig_paths`]).
    ///
    /// fired_on marker, once per tsconfig that declares `paths`:
    ///   `[tsconfig] <n> path aliases (baseUrl=<b>) file=<repo-relative path> repo=<label>`
    pub(crate) fn read(root: &Path, roots: &[ProjectRoot], repo_label: &str) -> Self {
        let dirs: BTreeSet<&str> = std::iter::once("")
            .chain(roots.iter().map(|r| r.rel_path.as_str()))
            .collect();
        let mut scopes = Vec::new();
        let mut bundler = Vec::new();
        for dir in dirs {
            if let Some(scope) = BundlerScope::read(root, dir) {
                bundler.push(scope);
            }
            let mut aliases = read_tsconfig_paths(&root.join(dir));
            if aliases.entries.is_empty() {
                continue;
            }
            aliases.dir = dir.to_string();
            aliases.dir_qname = dir.replace('/', "::");
            eprintln!(
                "[tsconfig] {} path aliases (baseUrl={}) file={} repo={repo_label}",
                aliases.entries.len(),
                aliases.base_url,
                aliases.shown_path()
            );
            scopes.push(aliases);
        }
        Self {
            scopes,
            bundler,
            workspace: WorkspaceSet::read(root, roots),
        }
    }

    /// FH.16: the bundler config a module resolves through: the deepest
    /// scope whose dir holds it.
    fn bundler_of(&self, from_module: &str) -> Option<&BundlerScope> {
        self.bundler
            .iter()
            .filter(|s| in_dir(from_module, &s.dir_qname))
            .max_by_key(|s| s.dir_qname.len())
    }

    /// The aliases a module resolves through: the deepest scope whose dir
    /// holds it.
    fn scope_of(&self, from_module: &str) -> Option<&TsAliases> {
        self.scopes
            .iter()
            .filter(|s| in_dir(from_module, &s.dir_qname))
            .max_by_key(|s| s.dir_qname.len())
    }

    /// Every `paths` key of every scope, as written (`@core/*`), then every
    /// bundler alias key as a `paths` key would name it (`common$` ->
    /// `common`; `@` -> `@` and `@/*`), for the A16.4 IMPORTS-cell filter
    /// (`LocalModuleIndex::add_alias_prefix`). A module root adds no key: what
    /// it maps depends on the files under it, never on the specifier alone.
    pub(super) fn keys(&self) -> impl Iterator<Item = &str> + '_ {
        let paths = self
            .scopes
            .iter()
            .flat_map(|s| s.entries.iter().map(|(k, _)| k.as_str()));
        let bundler = self
            .bundler
            .iter()
            .flat_map(|s| s.keys.iter().map(String::as_str));
        paths.chain(bundler)
    }
}

/// `from_module` sits in the dir whose module qname prefix is `dir_qname`
/// (`""` holds every module).
fn in_dir(from_module: &str, dir_qname: &str) -> bool {
    dir_qname.is_empty()
        || from_module
            .strip_prefix(dir_qname)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with("::"))
}

/// A6.8: the `paths` of `<dir>/tsconfig.json`, else of
/// `<dir>/tsconfig.base.json` when the first declares none. Written per dir so
/// the per-project-root pass ([`TsAliasSet::read`]) calls it once per root.
/// Never fails: a missing file or one with no `paths` gives no aliases; a file
/// that is not JSONC prints `[tsconfig] unparsed file=<path>` and gives none.
pub(super) fn read_tsconfig_paths(dir: &Path) -> TsAliases {
    for file in TSCONFIG_FILES {
        let path = dir.join(file);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        match parse_tsconfig_paths(&text) {
            Some((base_url, entries)) if !entries.is_empty() => {
                return TsAliases {
                    file,
                    base_url,
                    entries,
                    ..TsAliases::default()
                };
            }
            Some(_) => {}
            None => eprintln!("[tsconfig] unparsed file={}", path.display()),
        }
    }
    TsAliases::default()
}

/// `(baseUrl, paths entries in match order)` of one tsconfig's text, or `None`
/// when it is not JSONC. A key with an empty array or a non-string first
/// target is skipped.
fn parse_tsconfig_paths(text: &str) -> Option<(String, Vec<(String, String)>)> {
    let v: serde_json::Value = serde_json::from_str(&strip_jsonc(text)).ok()?;
    let opts = v.get("compilerOptions");
    let base_url = opts
        .and_then(|o| o.get("baseUrl"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .unwrap_or(".")
        .to_string();
    let mut entries: Vec<(String, String)> = opts
        .and_then(|o| o.get("paths"))
        .and_then(serde_json::Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(key, targets)| {
            let first = targets.as_array()?.first()?.as_str()?.trim();
            let key = key.trim();
            (!key.is_empty() && !first.is_empty()).then(|| (key.to_string(), first.to_string()))
        })
        .collect();
    let rank = |key: &str| match key.split_once('*') {
        None => (false, Reverse(0)),
        Some((prefix, _)) => (true, Reverse(prefix.len())),
    };
    entries.sort_by(|a, b| rank(&a.0).cmp(&rank(&b.0)).then_with(|| a.0.cmp(&b.0)));
    Some((base_url, entries))
}

/// tsconfig is JSONC: `//` and `/* */` comments and trailing commas, which
/// serde_json rejects. Drops all three outside string literals, so the `//`
/// of a URL inside a string (or inside a comment) survives or goes with it.
fn strip_jsonc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_str = false;
    while let Some(c) = chars.next() {
        if in_str {
            out.push(c);
            if c == '\\' {
                if let Some(escaped) = chars.next() {
                    out.push(escaped);
                }
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                out.push(c);
            }
            '/' if chars.peek() == Some(&'/') => {
                // A line comment: skip to its newline, which is kept.
                if chars.by_ref().any(|n| n == '\n') {
                    out.push('\n');
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut prev = '\0';
                for n in chars.by_ref() {
                    if prev == '*' && n == '/' {
                        break;
                    }
                    prev = n;
                }
                out.push(' ');
            }
            '}' | ']' => {
                // A trailing comma: the last significant char before the close.
                let end = out.trim_end().len();
                if out[..end].ends_with(',') {
                    out.truncate(end - 1);
                }
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

/// A repo-relative `/`-path (`web/./src/app/x.ts`) as the module qname
/// `path_to_qname` gives its file (`web::src::app::x`): `.` segments dropped,
/// `..` climbs, a source extension on the last segment stripped. `None` when
/// it climbs out of the repo or names nothing.
fn ts_path_qname(path: &str) -> Option<String> {
    let mut segs: Vec<&str> = Vec::new();
    for part in path.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                segs.pop()?;
            }
            p => segs.push(p),
        }
    }
    let last = segs.pop()?;
    let stem = [".ts", ".tsx", ".js", ".jsx"]
        .iter()
        .find_map(|e| last.strip_suffix(e))
        .unwrap_or(last);
    if stem.is_empty() {
        return None;
    }
    segs.push(stem);
    Some(segs.join("::"))
}

/// A6.8: the TS-family import resolver. A `.`-leading specifier goes to
/// [`resolve_ts_source`] unchanged; any other maps through the tsconfig
/// `paths` of the importer's scope (`@core/auth.service` under
/// `"@core/*": ["app/core/*"]` with `baseUrl` `./src` ->
/// `src::app::core::auth.service`). An unmatched bare specifier is a package:
/// `None`. A mapped qname no module has binds only its directory's index
/// module (DT.1b, `<qname>::index`): `resolve_imports_ts` looks both up
/// strictly, never by a tail guess.
pub(super) fn resolve_ts_source_aliased(
    from_module: &str,
    specifier: &str,
    aliases: &TsAliasSet,
) -> Option<String> {
    let spec = specifier.trim().trim_matches(|c| c == '"' || c == '\'');
    if spec.starts_with('.') {
        return resolve_ts_source(from_module, specifier);
    }
    aliases
        .scope_of(from_module)
        .and_then(|scope| scope.map(spec))
        .or_else(|| {
            aliases
                .bundler_of(from_module)
                .and_then(|scope| scope.map(spec, &aliases.workspace.root))
        })
        .or_else(|| aliases.workspace.resolve(spec))
}

// ---------------------------------------------------------------------------
// FH.16: bundler module roots and aliases
// ---------------------------------------------------------------------------

/// FH.16: the bundler configs read beside a project dir's `package.json`,
/// in read order: a dir with both a webpack and a vite config merges them in
/// this order. FH.16b: a `vitest.config.*` is read last, by the vite reader
/// ([`is_vite_config`]); an alias key a webpack or vite config already
/// declared stays theirs ([`BundlerScope::read`]). `vitest.workspace.*` is
/// not read.
pub(crate) const BUNDLER_CONFIGS: [&str; 12] = [
    "webpack.config.js",
    "webpack.config.cjs",
    "webpack.config.mjs",
    "webpack.config.ts",
    "vite.config.js",
    "vite.config.ts",
    "vite.config.mjs",
    "vitest.config.js",
    "vitest.config.ts",
    "vitest.config.mjs",
    "vitest.config.cjs",
    "vitest.config.mts",
];

/// FH.16b: `file` (a [`BUNDLER_CONFIGS`] name) is read as a vite config: a
/// vite or a vitest one.
fn is_vite_config(file: &str) -> bool {
    file.starts_with("vite.") || is_vitest_config(file)
}

/// FH.16b: `file` (a [`BUNDLER_CONFIGS`] name) is a `vitest.config.*`.
fn is_vitest_config(file: &str) -> bool {
    file.starts_with("vitest.")
}

/// FH.16: `base` (a file name) is a bundler config [`TsAliasSet::read`]
/// reads, so a watch session re-plans when one changes
/// (`watch::durability`, reason `bundler`).
pub(crate) fn is_bundler_config(base: &str) -> bool {
    BUNDLER_CONFIGS.contains(&base)
}

/// FH.16: the extensions a module root binds a file of: the TS family's
/// sources. A bare `x/y.css` under a module root stays a package import.
const MODULE_ROOT_EXTS: [&str; 10] = [
    "ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs", "vue", "svelte",
];

/// FH.16: one bundler `resolve.alias` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BundlerAlias {
    /// The key without webpack's exact-match `$`.
    key: String,
    /// The key ended in `$`: only the specifier `key` itself matches.
    exact: bool,
    /// The target, a repo-relative `/`-path.
    target: String,
}

/// FH.16: one project dir's bundler configs ([`BUNDLER_CONFIGS`]), read as
/// text, never evaluated ([`read_bundler_config`]): the `resolve.alias`
/// entries (webpack and vite) that map to an in-repo path, and webpack's
/// `resolve.modules` roots other than `node_modules`. The TS-family resolver
/// reads them after a tsconfig `paths` miss: aliases first, then the module
/// roots in declaration order, each binding a bare specifier only when the
/// file it names under that root exists ([`BundlerScope::map`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct BundlerScope {
    /// The config dir as a module qname prefix, `""` at the repo root.
    dir_qname: String,
    /// In declaration order.
    aliases: Vec<BundlerAlias>,
    /// Repo-relative dirs, in declaration order, deduplicated.
    module_roots: Vec<String>,
    /// The alias keys as `paths` keys ([`TsAliasSet::keys`]).
    keys: Vec<String>,
}

impl BundlerScope {
    /// The bundler configs of `dir` (repo-relative), or `None` when it holds
    /// no `package.json` or no config yields an alias or a module root.
    ///
    /// fired_on marker, once per config file read:
    ///   `[ts-aliases] bundler file=<f> module_roots=<m> aliases=<a> skipped=<s>`
    fn read(root: &Path, dir: &str) -> Option<Self> {
        let base = root.join(dir);
        if !base.join("package.json").is_file() {
            return None;
        }
        let mut scope = Self {
            dir_qname: dir.replace('/', "::"),
            ..Self::default()
        };
        for file in BUNDLER_CONFIGS {
            let Ok(text) = std::fs::read_to_string(base.join(file)) else {
                continue;
            };
            let read = read_bundler_config(&text, dir, is_vite_config(file));
            let shown = if dir.is_empty() {
                file.to_string()
            } else {
                format!("{dir}/{file}")
            };
            eprintln!(
                "[ts-aliases] bundler file={shown} module_roots={} aliases={} skipped={}",
                read.module_roots.len(),
                read.aliases.len(),
                read.skipped
            );
            for alias in read.aliases {
                // FH.16b: vitest's config usually spreads the vite one's
                // aliases; where both declare a key, the earlier config's
                // entry stands.
                if is_vitest_config(file) && scope.aliases.iter().any(|a| a.key == alias.key) {
                    continue;
                }
                scope.keys.push(alias.key.clone());
                if !alias.exact {
                    scope.keys.push(format!("{}/*", alias.key));
                }
                scope.aliases.push(alias);
            }
            for r in read.module_roots {
                if !scope.module_roots.contains(&r) {
                    scope.module_roots.push(r);
                }
            }
        }
        (!scope.aliases.is_empty() || !scope.module_roots.is_empty()).then_some(scope)
    }

    /// The module qname bare specifier `spec` names: through the first alias
    /// whose key is `spec` or a `/`-segment prefix of it (an exact key only
    /// `spec` itself), else through the first module root holding the file
    /// ([`package_file`]: the file, a source extension, `<spec>/index.<ext>`).
    fn map(&self, spec: &str, root: &Path) -> Option<String> {
        if spec.is_empty() || spec.starts_with(['.', '/', '#']) || spec.contains(':') {
            return None;
        }
        let aliased = self.aliases.iter().find_map(|a| {
            if spec == a.key {
                return Some(a.target.clone());
            }
            let rest = spec.strip_prefix(a.key.as_str())?.strip_prefix('/')?;
            (!a.exact).then(|| format!("{}/{rest}", a.target))
        });
        if let Some(path) = aliased {
            return ts_path_qname(&path);
        }
        self.module_roots.iter().find_map(|dir| {
            let file = package_file(root, dir, spec)?;
            let ext = file.rsplit_once('.').map(|(_, e)| e)?;
            MODULE_ROOT_EXTS
                .contains(&ext)
                .then(|| path_to_qname(&file))
        })
    }
}

/// FH.16: what one bundler config's text yields ([`read_bundler_config`]).
#[derive(Debug, Default, PartialEq, Eq)]
struct BundlerRead {
    aliases: Vec<BundlerAlias>,
    module_roots: Vec<String>,
    /// Alias entries and module-root elements read as something other than
    /// an in-repo path (a computed value, a package request, a regex key, a
    /// path leaving the repo), and `modules` values holding no array.
    skipped: usize,
}

/// FH.16: the `resolve: { modules, alias }` objects of one webpack / vite
/// config (`dir` its repo-relative dir), read off its tokens
/// ([`js_tokens`]). A value is a path when it is a string literal,
/// `path.join` / `path.resolve(__dirname, "<lit>", ..)`,
/// `fileURLToPath(new URL("<lit>", import.meta.url))`, or a `const` (or a
/// member of a `const` object literal) holding one, one level, same file. A
/// `modules` value contributes the elements of every array literal it holds
/// (both arms of a ternary), so a conditional root that is not a path is
/// skipped while the others stay; `node_modules` entries are ignored.
fn read_bundler_config(text: &str, dir: &str, vite: bool) -> BundlerRead {
    let toks = js_tokens(text);
    let mut consts: BTreeMap<&str, usize> = BTreeMap::new();
    for w in 0..toks.len().saturating_sub(3) {
        if let (JsTok::Ident(kw), JsTok::Ident(name), JsTok::Punct('=')) =
            (&toks[w], &toks[w + 1], &toks[w + 2])
            && matches!(kw.as_str(), "const" | "let" | "var")
            && toks.get(w + 3) != Some(&JsTok::Punct('='))
        {
            consts.entry(name.as_str()).or_insert(w + 3);
        }
    }
    let cx = ConfigCx {
        toks: &toks,
        consts,
        dir,
        vite,
    };
    let mut out = BundlerRead::default();
    for i in 0..toks.len().saturating_sub(2) {
        if cx.is_ident(i, "resolve") && cx.is_punct(i + 1, ':') && cx.is_punct(i + 2, '{') {
            for (key, (start, end)) in cx.properties(i + 2) {
                match key.as_str() {
                    "modules" => cx.read_modules(start, end, &mut out),
                    "alias" => cx.read_aliases(start, end, &mut out),
                    _ => {}
                }
            }
        }
    }
    out
}

/// FH.16: one bundler config's token, [`js_tokens`].
#[derive(Debug, Clone, PartialEq, Eq)]
enum JsTok {
    /// A string literal (or a template literal with no substitution),
    /// unescaped.
    Str(String),
    Ident(String),
    Punct(char),
    /// A number, a regex or a template literal with a substitution.
    Other,
}

/// FH.16: a JS / TS config's tokens, comments dropped. A `/` opens a regex
/// literal where an operand is expected (after an operator, an opening
/// bracket, a `,` / `:` / `;` or a keyword), so a quote inside a regex never
/// opens a string.
fn js_tokens(text: &str) -> Vec<JsTok> {
    let chars: Vec<char> = text.chars().collect();
    let mut toks: Vec<JsTok> = Vec::new();
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        let next = chars.get(i + 1).copied();
        if c.is_whitespace() {
            i += 1;
        } else if c == '/' && next == Some('/') {
            while chars.get(i).is_some_and(|&c| c != '\n') {
                i += 1;
            }
        } else if c == '/' && next == Some('*') {
            i += 2;
            while i < chars.len() && !(chars[i - 1] == '*' && chars[i] == '/') {
                i += 1;
            }
            i += 1;
        } else if c == '"' || c == '\'' {
            let mut s = String::new();
            i += 1;
            while let Some(&d) = chars.get(i) {
                i += 1;
                match d {
                    '\\' => {
                        if let Some(&e) = chars.get(i) {
                            s.push(e);
                            i += 1;
                        }
                    }
                    '\n' => break,
                    _ if d == c => break,
                    _ => s.push(d),
                }
            }
            toks.push(JsTok::Str(s));
        } else if c == '`' {
            let (mut s, mut depth, mut plain) = (String::new(), 0usize, true);
            i += 1;
            while let Some(&d) = chars.get(i) {
                i += 1;
                match d {
                    '\\' => {
                        if let Some(&e) = chars.get(i) {
                            s.push(e);
                            i += 1;
                        }
                    }
                    '$' if depth == 0 && chars.get(i) == Some(&'{') => {
                        plain = false;
                        depth = 1;
                        i += 1;
                    }
                    '{' if depth > 0 => depth += 1,
                    '}' if depth > 0 => depth -= 1,
                    '`' if depth == 0 => break,
                    _ => s.push(d),
                }
            }
            toks.push(if plain { JsTok::Str(s) } else { JsTok::Other });
        } else if c == '/' && regex_allowed(toks.last()) {
            let mut class = false;
            i += 1;
            while let Some(&d) = chars.get(i) {
                i += 1;
                match d {
                    '\\' => i += 1,
                    '[' => class = true,
                    ']' => class = false,
                    '/' if !class => break,
                    '\n' => break,
                    _ => {}
                }
            }
            while chars.get(i).is_some_and(|c| c.is_ascii_alphabetic()) {
                i += 1;
            }
            toks.push(JsTok::Other);
        } else if c.is_alphabetic() || c == '_' || c == '$' {
            let start = i;
            while chars
                .get(i)
                .is_some_and(|&c| c.is_alphanumeric() || c == '_' || c == '$')
            {
                i += 1;
            }
            toks.push(JsTok::Ident(chars[start..i].iter().collect()));
        } else if c.is_ascii_digit() {
            while chars
                .get(i)
                .is_some_and(|&c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
            {
                i += 1;
            }
            toks.push(JsTok::Other);
        } else {
            toks.push(JsTok::Punct(c));
            i += 1;
        }
    }
    toks
}

/// A `/` after `prev` opens a regex literal rather than dividing.
fn regex_allowed(prev: Option<&JsTok>) -> bool {
    match prev {
        None => true,
        Some(JsTok::Punct(p)) => "(,=:[!&|?{};+-*%<>~^".contains(*p),
        Some(JsTok::Ident(k)) => matches!(
            k.as_str(),
            "return" | "typeof" | "case" | "do" | "else" | "in" | "of" | "new" | "void" | "throw"
        ),
        Some(_) => false,
    }
}

/// FH.16: the reading context of one bundler config.
struct ConfigCx<'a> {
    toks: &'a [JsTok],
    /// `const` / `let` / `var` name -> its initialiser's first token, the
    /// first declaration of the name (looked up, never iterated).
    consts: BTreeMap<&'a str, usize>,
    dir: &'a str,
    vite: bool,
}

/// FH.16: a value read off a config ([`ConfigCx::value`]).
enum ConfigVal {
    /// A string literal, as written.
    Lit(String),
    /// A path relative to the config's dir (`__dirname`-rooted).
    Path(String),
    Unknown,
}

impl ConfigCx<'_> {
    fn tok(&self, i: usize) -> Option<&JsTok> {
        self.toks.get(i)
    }

    fn is_punct(&self, i: usize, c: char) -> bool {
        self.tok(i) == Some(&JsTok::Punct(c))
    }

    fn is_ident(&self, i: usize, name: &str) -> bool {
        matches!(self.tok(i), Some(JsTok::Ident(n)) if n == name)
    }

    /// The end of the expression starting at `i`: the index of the first
    /// `,` / `;` or unbalanced closer at its depth (or the token count).
    fn expr_end(&self, i: usize) -> usize {
        let mut depth = 0usize;
        let mut j = i;
        while let Some(t) = self.tok(j) {
            match t {
                JsTok::Punct('(' | '[' | '{') => depth += 1,
                JsTok::Punct(')' | ']' | '}') => {
                    if depth == 0 {
                        return j;
                    }
                    depth -= 1;
                }
                JsTok::Punct(',' | ';') if depth == 0 => return j,
                _ => {}
            }
            j += 1;
        }
        j
    }

    /// The `(key, value span)` properties of the object literal opened at
    /// `open`, in order; a shorthand `key` spans its own token; a spread or
    /// computed key is passed over.
    fn properties(&self, open: usize) -> Vec<(String, (usize, usize))> {
        let mut out = Vec::new();
        let mut i = open + 1;
        while let Some(t) = self.tok(i) {
            if matches!(t, JsTok::Punct('}')) {
                break;
            }
            let key = match t {
                JsTok::Ident(k) | JsTok::Str(k) => Some(k.clone()),
                _ => None,
            };
            let colon = self.is_punct(i + 1, ':');
            let end = self.expr_end(if colon { i + 2 } else { i });
            match key {
                Some(k) if colon => out.push((k, (i + 2, end))),
                Some(k) if end == i + 1 => out.push((k, (i, end))),
                _ => {}
            }
            if !self.is_punct(end, ',') {
                break;
            }
            i = end + 1;
        }
        out
    }

    /// The value spanning `[start, end)`; `follow` allows one hop through a
    /// `const` name or a `const` object's member.
    fn value(&self, start: usize, end: usize, follow: bool) -> ConfigVal {
        let span = &self.toks[start.min(end)..end];
        match span {
            [JsTok::Str(s)] => return ConfigVal::Lit(s.clone()),
            [JsTok::Ident(d)] if d == "__dirname" => return ConfigVal::Path(String::new()),
            [JsTok::Ident(name)] if follow => {
                return self.const_value(name, None);
            }
            [JsTok::Ident(name), JsTok::Punct('.'), JsTok::Ident(member)] if follow => {
                return self.const_value(name, Some(member));
            }
            _ => {}
        }
        // `path.join(..)` / `path.resolve(..)` / bare `join(..)` / `resolve(..)`.
        let call = match span {
            [
                JsTok::Ident(_),
                JsTok::Punct('.'),
                JsTok::Ident(f),
                JsTok::Punct('('),
                ..,
            ] => Some((f.as_str(), start + 4)),
            [JsTok::Ident(f), JsTok::Punct('('), ..] => Some((f.as_str(), start + 2)),
            _ => None,
        };
        let Some((f, args)) = call else {
            return ConfigVal::Unknown;
        };
        if f == "fileURLToPath" {
            return self.file_url(args, end);
        }
        if !matches!(f, "join" | "resolve") || !self.is_punct(end.saturating_sub(1), ')') {
            return ConfigVal::Unknown;
        }
        let mut parts: Vec<String> = Vec::new();
        let mut i = args;
        let mut first = true;
        while i < end - 1 {
            let e = self.expr_end(i);
            let v = if first {
                self.value(i, e, follow)
            } else {
                self.value(i, e, false)
            };
            match (first, v) {
                (true, ConfigVal::Path(p)) => parts.push(p),
                (false, ConfigVal::Lit(s)) if !(f == "resolve" && s.starts_with('/')) => {
                    parts.push(s)
                }
                _ => return ConfigVal::Unknown,
            }
            first = false;
            i = e + 1;
        }
        if first {
            return ConfigVal::Unknown;
        }
        ConfigVal::Path(parts.join("/"))
    }

    /// `fileURLToPath(new URL("<lit>", import.meta.url))`, its argument list
    /// opening at `args`.
    fn file_url(&self, args: usize, end: usize) -> ConfigVal {
        let at = |k: usize| self.tok(args + k);
        let shape = self.is_ident(args, "new")
            && self.is_ident(args + 1, "URL")
            && self.is_punct(args + 2, '(')
            && self.is_punct(args + 4, ',')
            && self.is_ident(args + 5, "import")
            && self.is_ident(args + 7, "meta")
            && self.is_ident(args + 9, "url")
            && self.is_punct(args + 10, ')')
            && args + 12 == end;
        match at(3) {
            Some(JsTok::Str(s)) if shape => ConfigVal::Path(s.clone()),
            _ => ConfigVal::Unknown,
        }
    }

    /// The value of `const name`, or of its object literal's `member`.
    fn const_value(&self, name: &str, member: Option<&str>) -> ConfigVal {
        let Some(&start) = self.consts.get(name) else {
            return ConfigVal::Unknown;
        };
        let end = self.expr_end(start);
        let Some(member) = member else {
            return self.value(start, end, false);
        };
        if !self.is_punct(start, '{') {
            return ConfigVal::Unknown;
        }
        self.properties(start)
            .into_iter()
            .find(|(k, _)| k == member)
            .map_or(ConfigVal::Unknown, |(_, (s, e))| self.value(s, e, false))
    }

    /// The value span a `const` name's initialiser stands for, when the
    /// span is that one name; else the span itself.
    fn deref(&self, start: usize, end: usize) -> (usize, usize) {
        match self.toks.get(start..end) {
            Some([JsTok::Ident(name)]) => self
                .consts
                .get(name.as_str())
                .map_or((start, end), |&s| (s, self.expr_end(s))),
            _ => (start, end),
        }
    }

    /// `rel` (relative to the config's dir) as a repo-relative dir, or
    /// `None` when it leaves the repo.
    fn repo_path(&self, rel: &str) -> Option<String> {
        let mut segs: Vec<&str> = Vec::new();
        for part in self.dir.split('/').chain(rel.split(['/', '\\'])) {
            match part {
                "" | "." => {}
                ".." => {
                    segs.pop()?;
                }
                p => segs.push(p),
            }
        }
        Some(segs.join("/"))
    }

    /// A value as a repo-relative path: a `__dirname`-rooted path, or a
    /// literal that is relative (`./x`, `../x`, or for `modules` a bare
    /// `src`, which webpack looks up from the config's dir); vite reads a
    /// `/src` alias against its root, the config's dir.
    fn as_path(&self, v: ConfigVal, bare_ok: bool) -> Option<String> {
        let rel = match v {
            ConfigVal::Path(p) => p,
            ConfigVal::Lit(s) if s.starts_with("./") || s.starts_with("../") || s == "." => s,
            ConfigVal::Lit(s) if self.vite && s.starts_with('/') => s,
            ConfigVal::Lit(s) if bare_ok && !s.starts_with('/') && !s.is_empty() => s,
            _ => return None,
        };
        self.repo_path(&rel)
    }

    /// `resolve.modules` spanning `[start, end)`: every array literal at the
    /// value's top level, element by element.
    fn read_modules(&self, start: usize, end: usize, out: &mut BundlerRead) {
        let (start, end) = self.deref(start, end);
        let mut arrays = Vec::new();
        let mut depth = 0usize;
        for i in start..end {
            match self.tok(i) {
                Some(JsTok::Punct('[')) if depth == 0 => {
                    arrays.push(i);
                    depth += 1;
                }
                Some(JsTok::Punct('(' | '[' | '{')) => depth += 1,
                Some(JsTok::Punct(')' | ']' | '}')) => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        if arrays.is_empty() {
            out.skipped += 1;
            return;
        }
        for open in arrays {
            let mut i = open + 1;
            while i < end && !self.is_punct(i, ']') {
                let e = self.expr_end(i);
                if e > i {
                    let v = self.value(i, e, true);
                    let node_modules = match &v {
                        ConfigVal::Lit(s) | ConfigVal::Path(s) => {
                            s.trim_end_matches('/').rsplit('/').next() == Some("node_modules")
                        }
                        ConfigVal::Unknown => false,
                    };
                    if !node_modules {
                        match self.as_path(v, true) {
                            Some(p) if !out.module_roots.contains(&p) => out.module_roots.push(p),
                            Some(_) => {}
                            None => out.skipped += 1,
                        }
                    }
                }
                if !self.is_punct(e, ',') {
                    break;
                }
                i = e + 1;
            }
        }
    }

    /// `resolve.alias` spanning `[start, end)`: an object of `key: path`
    /// entries, or vite's array of `{ find, replacement }`.
    fn read_aliases(&self, start: usize, end: usize, out: &mut BundlerRead) {
        let (start, end) = self.deref(start, end);
        let push = |key: &str, v: ConfigVal, out: &mut BundlerRead| {
            let (key, exact) = match key.strip_suffix('$') {
                Some(k) => (k, true),
                None => (key, false),
            };
            match self.as_path(v, false) {
                Some(target) if !key.is_empty() => out.aliases.push(BundlerAlias {
                    key: key.to_string(),
                    exact,
                    target,
                }),
                _ => out.skipped += 1,
            }
        };
        if self.is_punct(start, '{') {
            for (key, (s, e)) in self.properties(start) {
                push(&key, self.value(s, e, true), out);
            }
        } else if self.is_punct(start, '[') {
            let mut i = start + 1;
            while i < end && !self.is_punct(i, ']') {
                let e = self.expr_end(i);
                let props = if self.is_punct(i, '{') {
                    self.properties(i)
                } else {
                    Vec::new()
                };
                let find = props.iter().find(|(k, _)| k == "find");
                let repl = props.iter().find(|(k, _)| k == "replacement");
                match (find, repl) {
                    (Some((_, (fs, fe))), Some((_, (rs, re)))) => match self.value(*fs, *fe, false)
                    {
                        ConfigVal::Lit(key) => push(&key, self.value(*rs, *re, true), out),
                        _ => out.skipped += 1,
                    },
                    _ => out.skipped += 1,
                }
                if !self.is_punct(e, ',') {
                    break;
                }
                i = e + 1;
            }
        } else {
            out.skipped += 1;
        }
    }
}

// ---------------------------------------------------------------------------
// FH.5: workspace packages
// ---------------------------------------------------------------------------

/// FH.5: the qname tags of `glia_graph`'s TS import pass (its
/// `imports::WORKSPACE_ENTRY` / `WORKSPACE_SUBPATH` / `WORKSPACE_REPORT`,
/// crate-private there): a qname [`WorkspaceSet::resolve`] mapped carries one
/// of the first two, and the pass stamps the import IMPORTS rule
/// `workspace_package` and hands its counts back as a call with the third as
/// `from_module`. `ts_family_binds_a_workspace_package` pins the round trip.
const WORKSPACE_ENTRY: &str = "\u{0}workspace:entry\u{0}";
const WORKSPACE_SUBPATH: &str = "\u{0}workspace:subpath\u{0}";
const WORKSPACE_REPORT: &str = "\u{0}workspace:report\u{0}";

/// FH.5: one repo's workspace packages ([`workspace_packages`], read off the
/// walk's project roots), by name, as the TS-family resolver's last step:
/// a bare specifier equal to a package name (or `name/subpath`) maps to that
/// package's in-repo module, before the external-package `None`. A
/// published package of the same name is shadowed, as the package manager
/// links the workspace member.
#[derive(Debug, Clone, Default)]
pub(crate) struct WorkspaceSet {
    /// The repo root, for a subpath's file probe ([`package_file`]).
    root: PathBuf,
    /// Sorted by (name, dir).
    packages: Vec<WorkspacePackage>,
    /// Name -> its index in `packages`; `None` when two packages of the repo
    /// declare the name, which then maps to nothing bindable (no guess).
    by_name: BTreeMap<String, Option<usize>>,
}

impl WorkspaceSet {
    /// The workspace packages under `root` ([`workspace_packages`]).
    pub(crate) fn read(root: &Path, roots: &[ProjectRoot]) -> Self {
        Self::of(root, workspace_packages(root, roots))
    }

    fn of(root: &Path, packages: Vec<WorkspacePackage>) -> Self {
        let mut by_name: BTreeMap<String, Option<usize>> = BTreeMap::new();
        for (i, p) in packages.iter().enumerate() {
            by_name
                .entry(p.name.clone())
                .and_modify(|slot| *slot = None)
                .or_insert(Some(i));
        }
        Self {
            root: root.to_path_buf(),
            packages,
            by_name,
        }
    }

    /// The tagged module qname bare specifier `spec` names, or `None` when
    /// its package is no workspace package. `name` alone maps to the entry's
    /// module; `name/sub` through the `exports` subpath `./sub` (exact, else
    /// the longest-prefix `*` pattern), else the directory `<dir>/sub`
    /// ([`package_file`], then the path's qname: the graph builder binds the
    /// module, its same-stem sibling or the directory's index). A package
    /// with no entry file, or a name two packages declare, maps to an empty
    /// tagged qname: the builder counts it unbound.
    fn resolve(&self, spec: &str) -> Option<String> {
        let (name, sub) = split_package_specifier(spec)?;
        let slot = *self.by_name.get(name)?;
        let tag = if sub.is_empty() {
            WORKSPACE_ENTRY
        } else {
            WORKSPACE_SUBPATH
        };
        let Some(pkg) = slot.and_then(|i| self.packages.get(i)) else {
            return Some(tag.to_string());
        };
        let qname = if sub.is_empty() {
            pkg.entry.as_deref().map(path_to_qname).unwrap_or_default()
        } else {
            self.subpath_qname(pkg, sub)
        };
        Some(format!("{tag}{qname}"))
    }

    /// The module qname `<pkg>/<sub>` names ([`WorkspaceSet::resolve`]).
    fn subpath_qname(&self, pkg: &WorkspacePackage, sub: &str) -> String {
        let key = format!("./{sub}");
        let target = pkg
            .subpaths
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, t)| t.clone())
            .or_else(|| {
                pkg.subpaths
                    .iter()
                    .filter_map(|(k, t)| {
                        let (prefix, suffix) = k.split_once('*')?;
                        let star = key.strip_prefix(prefix)?.strip_suffix(suffix)?;
                        Some((prefix.len(), t.replacen('*', star, 1)))
                    })
                    .max_by_key(|(len, _)| *len)
                    .map(|(_, t)| t)
            })
            .unwrap_or_else(|| sub.to_string());
        if let Some(file) = package_file(&self.root, &pkg.dir, &target) {
            return path_to_qname(&file);
        }
        let rel = target.trim().trim_start_matches("./");
        let path = if pkg.dir.is_empty() {
            rel.to_string()
        } else {
            format!("{}/{rel}", pkg.dir)
        };
        ts_path_qname(&path).unwrap_or_default()
    }

    /// FH.5 fired_on, once per TS-family graph build of a repo with a
    /// workspace package (`report` is the graph builder's counts, `None`
    /// when no import named one):
    ///   `[imports] workspace packages=<p> bound=<b> subpath=<s> unbound=<u> repo=<label>`
    fn marker(&self, report: Option<(usize, usize, usize)>, repo_label: &str) -> Option<String> {
        let packages = self.packages.len();
        let (bound, subpath, unbound) = report.unwrap_or_default();
        (packages > 0).then(|| {
            format!(
                "[imports] workspace packages={packages} bound={bound} subpath={subpath} \
                 unbound={unbound} repo={repo_label}"
            )
        })
    }
}

/// FH.5: a bare specifier as `(package name, subpath)`: `@scope/name/a/b` ->
/// (`@scope/name`, `a/b`), `name/a` -> (`name`, `a`), `name` -> (`name`, ``).
/// `None` for anything no package name starts: a relative or absolute path,
/// a URL or `node:` builtin, a bare `@scope`.
fn split_package_specifier(spec: &str) -> Option<(&str, &str)> {
    if spec.is_empty() || spec.starts_with(['.', '/', '#']) || spec.contains(':') {
        return None;
    }
    let cut = if spec.starts_with('@') {
        let slash = spec.find('/')?;
        spec[slash + 1..].find('/').map(|i| slash + 1 + i)
    } else {
        spec.find('/')
    };
    let (name, sub) = match cut {
        Some(i) => (&spec[..i], spec[i + 1..].trim_end_matches('/')),
        None => (spec, ""),
    };
    (!name.ends_with('/')).then_some((name, sub))
}

/// FH.5: the graph builder's `WORKSPACE_REPORT` specifier, `"<bound>
/// <subpath> <unbound>"`, read back.
fn parse_workspace_report(spec: &str) -> Option<(usize, usize, usize)> {
    let mut it = spec.split(' ').map(str::parse::<usize>);
    match (it.next(), it.next(), it.next(), it.next()) {
        (Some(Ok(b)), Some(Ok(s)), Some(Ok(u)), None) => Some((b, s, u)),
        _ => None,
    }
}

/// DF.4a: the repo's own Dart packages, read off the walk's project roots
/// ([`DartPackages::from_roots`]): each `pubspec.yaml` root's declared
/// `name:` and the qname of its directory, for [`resolve_dart_source`].
///
/// Read-only during the build but for the resolved-URI counter, an atomic:
/// the Dart group builds on the engine pool (LG.1c), so one `&DartPackages`
/// is shared across threads and the graph never depends on the count.
#[derive(Debug, Default)]
pub(crate) struct DartPackages {
    /// Package name -> the qname of its project root (`""` for the repo
    /// root); `None` when two pubspecs of the repo claim the name, which
    /// then resolves nowhere (no guess between them).
    by_name: BTreeMap<String, Option<String>>,
    /// Own-package URI lookups [`resolve_dart_source`] mapped since the last
    /// [`DartPackages::marker`]: one per import statement the graph builder
    /// resolves (a `show A, B` import is two, CB.17).
    resolved: AtomicUsize,
}

impl DartPackages {
    /// The `dart`-ecosystem roots (`pubspec.yaml`) whose label is the
    /// pubspec's own `name:` ([`LABEL_MANIFEST`]): `package:<name>/` names
    /// that package's `lib/`, and a pubspec with no `name:` (whose label
    /// fell back to its directory) names no package Dart could import.
    pub(crate) fn from_roots(roots: &[ProjectRoot]) -> Self {
        let mut by_name: BTreeMap<String, Option<String>> = BTreeMap::new();
        for root in roots
            .iter()
            .filter(|r| r.ecosystem == "dart" && r.label_source == LABEL_MANIFEST)
        {
            let qname = root.rel_path.replace(['/', '\\'], "::");
            by_name
                .entry(root.label.clone())
                .and_modify(|slot| *slot = None)
                .or_insert(Some(qname));
        }
        Self {
            by_name,
            resolved: AtomicUsize::new(0),
        }
    }

    /// The project-root qname of in-repo package `name`, unless two pubspecs
    /// claim it.
    fn root_of(&self, name: &str) -> Option<&str> {
        self.by_name.get(name)?.as_deref()
    }

    /// The package names that resolve (claimed by exactly one pubspec).
    fn packages(&self) -> usize {
        self.by_name.values().filter(|root| root.is_some()).count()
    }

    /// DF.4a fired_on marker, printed once per repo build after the language
    /// builds, when any own-package URI resolved; takes the count, so the
    /// next build of the same packages counts from zero:
    ///   `[dart-imports] own-package uris resolved=<n> packages=<k> repo=<label>`
    /// `n` = own-package lookups mapped, `k` = the repo's resolvable
    /// package names.
    pub(crate) fn marker(&self, repo_label: &str) -> Option<String> {
        let n = self.resolved.swap(0, Ordering::Relaxed);
        (n > 0).then(|| {
            format!(
                "[dart-imports] own-package uris resolved={n} packages={} repo={repo_label}",
                self.packages()
            )
        })
    }
}

/// DF.4a: the Dart import resolver. `package:<name>/<rest>.dart` with
/// `<name>` one of the repo's own packages ([`DartPackages`]) names the file
/// `<rest>.dart` under that package's `lib/`, so its module
/// `<root qname>::lib::<rest, / -> ::>` (`package:shop/src/util.dart` ->
/// `lib::src::util`; under a project at `apps/mobile`,
/// `apps::mobile::lib::src::util`). Any other `package:` URI (another
/// package, or a name two pubspecs claim), a `dart:` library, or a package
/// URI that is not a `.dart` file under `lib/` is external: `None`.
/// Everything else (`src/x.dart`, `../lib/cart.dart`) is
/// [`resolve_relative_source`], unchanged.
fn resolve_dart_source(from_module: &str, specifier: &str, pkgs: &DartPackages) -> Option<String> {
    let spec = specifier.trim().trim_matches(|c| c == '"' || c == '\'');
    if spec.starts_with("dart:") {
        return None;
    }
    let Some(uri) = spec.strip_prefix("package:") else {
        return resolve_relative_source(from_module, specifier);
    };
    let (name, rest) = uri.split_once('/')?;
    let root = pkgs.root_of(name)?;
    let parts: Vec<&str> = rest
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    // A package URI never leaves its package's `lib/`.
    if parts.contains(&"..") {
        return None;
    }
    let (file, dirs) = parts.split_last()?;
    let stem = file.strip_suffix(".dart").filter(|s| !s.is_empty())?;
    let mut segs: Vec<&str> = root.split("::").filter(|s| !s.is_empty()).collect();
    segs.push("lib");
    segs.extend(dirs.iter().copied());
    segs.push(stem);
    pkgs.resolved.fetch_add(1, Ordering::Relaxed);
    Some(segs.join("::"))
}

/// Relative-import resolver for the `swift` arm and the non-TS `_`-arm
/// languages (solidity / terraform), and the non-`package:` half of
/// [`resolve_dart_source`]. Handles dotted specifiers (`./x`,
/// `../a/b`) AND bare filenames that carry a source extension (`import
/// 'models.dart'`) — both resolve against the importing file's directory to
/// the `path_to_qname` form. A bare specifier with no source extension (a
/// package / system import like `package:collection`, `import Foundation`) is
/// external → None.
/// Superset of `resolve_ts_source`; kept separate so the verified TS-family
/// path is untouched. C/C++ has [`resolve_include_source`].
fn resolve_relative_source(from_module: &str, specifier: &str) -> Option<String> {
    const SRC_EXT: &[&str] = &[".dart", ".sol", ".swift", ".ts", ".tsx", ".js", ".jsx"];
    let spec = specifier
        .trim()
        .trim_matches(|c| c == '"' || c == '\'' || c == '<' || c == '>');
    let has_src_ext = SRC_EXT.iter().any(|e| spec.ends_with(e));
    if !spec.starts_with('.') && !has_src_ext {
        return None; // external package / system header
    }
    let mut segs: Vec<String> = from_module.split("::").map(String::from).collect();
    segs.pop(); // directory of the importing file
    for part in spec.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                segs.pop();
            }
            p => {
                let stem = SRC_EXT.iter().find_map(|e| p.strip_suffix(e)).unwrap_or(p);
                segs.push(stem.to_string());
            }
        }
    }
    if segs.is_empty() {
        return None;
    }
    Some(segs.join("::"))
}

/// LB.10a: a quoted `#include "x/y.h"` names a FILE relative to the including
/// file's directory, and every C/C++ MODULE is `<dir>::<file name>`
/// (`route::ModuleQnames`), so the target keeps its extension. An absolute
/// path, or one that climbs above the repo root, is outside the repo → None.
/// The includer-relative first step of CB.22's
/// [`super::c_includes::IncludeResolver`], which searches the repo's include
/// roots after it (and alone for an angle include).
pub(super) fn resolve_include_source(from_module: &str, specifier: &str) -> Option<String> {
    let spec = specifier.trim().trim_matches('"');
    if spec.is_empty() || spec.starts_with(['/', '\\']) {
        return None;
    }
    let mut segs: Vec<&str> = from_module.split("::").collect();
    segs.pop(); // the including file itself
    for part in spec.split(['/', '\\']) {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// LB.10a: an include resolves against the includer's directory to the
    /// header's file-named MODULE; nothing outside the repo resolves.
    #[test]
    fn resolve_include_source_keeps_the_file_name() {
        for (from, spec, want) in [
            ("src::main.cpp", "Widget.h", Some("src::Widget.h")),
            (
                "src::cart.cpp",
                "../include/shop/cart.hpp",
                Some("include::shop::cart.hpp"),
            ),
            ("main.cpp", "mathutil.h", Some("mathutil.h")),
            ("src::a.cpp", "./detail/b.h", Some("src::detail::b.h")),
            ("src::a.cpp", "\"quoted.h\"", Some("src::quoted.h")),
            ("win::a.cpp", "sub\\b.h", Some("win::sub::b.h")),
            ("a.cpp", "../x.h", None),
            ("src::a.c", "/usr/include/x.h", None),
            ("src::a.c", "", None),
        ] {
            assert_eq!(
                resolve_include_source(from, spec).as_deref(),
                want,
                "{from} {spec}"
            );
        }
    }

    fn tally(lang: &'static str, bound: usize, unresolved: usize) -> HeritageTally {
        HeritageTally {
            lang,
            bound,
            unresolved,
        }
    }

    /// Every language graph built gets one token per half, sorted by name
    /// (the TS family is built last but sorts in place), so A6.4 / A6.5 grep
    /// ` solidity=` / ` dart=` off the same line.
    #[test]
    fn heritage_marker_lists_every_built_language_sorted() {
        let mut t = vec![
            tally("solidity", 0, 1),
            tally("dart", 0, 0),
            tally("typescript", 2, 0),
        ];
        assert_eq!(
            heritage_marker(&mut t, "fx").as_deref(),
            Some(
                "[heritage] refs bound: dart=0 solidity=0 typescript=2 \
                 unresolved: dart=0 solidity=1 typescript=0 repo=fx"
            )
        );
    }

    fn edge(
        from: u64,
        to: u64,
        category: EdgeCategoryId,
        ev: Option<evidence::Evidence>,
    ) -> glia_core::Edge {
        let e = glia_core::Edge::new(
            NodeId(from),
            NodeId(to),
            category,
            glia_core::Confidence::Strong,
        );
        match ev {
            Some(ev) => e.with_cell(ev.to_cell()),
            None => e,
        }
    }

    /// Calls into an ENDPOINT are not counted; a site is basis `site` only;
    /// refs are recognised by their resolver's emitter (and rule), whatever
    /// their category.
    #[test]
    fn line_tally_counts_sites_per_kind() {
        use evidence::Evidence;
        let mut g = RepoGraph {
            repo: RepoId(1),
            nodes: Vec::new(),
            edges: Vec::new(),
            nav: Default::default(),
            symbols: Default::default(),
            unresolved_calls: Vec::new(),
            unresolved_refs: Vec::new(),
            properties: Default::default(),
        };
        g.nav.kind_by_id.insert(NodeId(2), node_kind::FUNCTION);
        g.nav.kind_by_id.insert(NodeId(3), node_kind::ENDPOINT);
        let site = |e: &str| Evidence::emitter(e).line(4);
        g.edges = vec![
            edge(1, 2, edge_category::CALLS, Some(site("graph:calls"))),
            edge(
                1,
                2,
                edge_category::CALLS,
                Some(Evidence::emitter("graph:build")),
            ),
            edge(
                1,
                3,
                edge_category::CALLS,
                Some(Evidence::emitter("parser:python")),
            ),
            edge(1, 2, edge_category::IMPORTS, Some(site("graph:imports"))),
            edge(1, 2, edge_category::HANDLED_BY, Some(site("graph:refs"))),
            edge(
                1,
                2,
                edge_category::USES,
                Some(Evidence::emitter("graph:rust_paths").rule("enum_variant")),
            ),
            edge(
                1,
                2,
                edge_category::USES,
                Some(site("graph:rust_paths").rule("glob")),
            ),
            edge(1, 2, edge_category::DEFINES, None),
        ];
        let t = LineTally::of(&g);
        assert_eq!(
            t,
            LineTally {
                calls: 2,
                calls_site: 1,
                imports: 1,
                imports_site: 1,
                refs: 2,
                refs_site: 1
            }
        );
        assert_eq!(
            t.marker("rust").as_deref(),
            Some("[evidence-lines] lang=rust calls=2 site=1 imports=1 site=1 refs=2 site=1")
        );
        assert_eq!(LineTally::default().marker("go"), None);
    }

    #[test]
    fn heritage_marker_is_silent_without_heritage() {
        let mut t = vec![tally("go", 0, 0), tally("python", 0, 0)];
        assert_eq!(heritage_marker(&mut t, "fx"), None);
        assert_eq!(heritage_marker(&mut [], "fx"), None);
    }
}

#[cfg(test)]
mod ts_alias_tests {
    use super::*;

    /// A tsconfig as Angular CLI writes it: JSONC, `baseUrl` `./src`.
    const ANGULAR: &str = r#"/* To learn more see: https://angular.dev/reference/configs */
{
  "compileOnSave": false,
  "compilerOptions": {
    // non-relative imports resolve from ./src
    "baseUrl": "./src",
    "paths": {
      "@core/*": ["app/core/*"],
      "@env": ["environments/environment.ts"],
    },
    "outDir": "./dist//out", /* a `//` inside a string is not a comment */
  },
}
"#;

    /// One scope per `(dir, tsconfig text)`, as `TsAliasSet::read` builds it.
    fn set_of(scopes: &[(&str, &str)]) -> TsAliasSet {
        let scopes = scopes
            .iter()
            .map(|(dir, text)| {
                let (base_url, entries) = parse_tsconfig_paths(text).expect("jsonc");
                TsAliases {
                    dir: dir.to_string(),
                    dir_qname: dir.replace('/', "::"),
                    file: "tsconfig.json",
                    base_url,
                    entries,
                }
            })
            .collect();
        TsAliasSet {
            scopes,
            ..TsAliasSet::default()
        }
    }

    #[test]
    fn a_wildcard_alias_maps_through_base_url() {
        let aliases = set_of(&[("", ANGULAR)]);
        let page = "src::app::feature::page";
        assert_eq!(
            resolve_ts_source_aliased(page, "@core/auth.service", &aliases),
            Some("src::app::core::auth.service".to_string())
        );
        assert_eq!(
            resolve_ts_source_aliased(page, "'@core/auth.service'", &aliases).as_deref(),
            Some("src::app::core::auth.service"),
            "the parser's quotes"
        );
        assert_eq!(
            resolve_ts_source_aliased(page, "@env", &aliases).as_deref(),
            Some("src::environments::environment"),
            "an exact key; the target's extension goes"
        );
        assert_eq!(
            resolve_ts_source_aliased(page, "@angular/core", &aliases),
            None
        );
        assert_eq!(
            resolve_ts_source_aliased(page, "@environment", &aliases),
            None,
            "exact is whole"
        );
        assert_eq!(resolve_ts_source_aliased(page, "rxjs", &aliases), None);
        // No tsconfig: every bare specifier is a package, as before A6.8.
        assert_eq!(
            resolve_ts_source_aliased(page, "@core/x", &TsAliasSet::default()),
            None
        );
    }

    #[test]
    fn a_relative_specifier_is_resolve_ts_source_unchanged() {
        let aliases = set_of(&[("", ANGULAR)]);
        for (from, spec) in [
            ("src::app::feature::page", "./page.module"),
            ("src::app::feature::page", "../core/auth.service"),
            ("src::app::feature::page", "../../../x.ts"),
            ("src::main", "\"./util.js\""),
            ("main", "../escape"),
            ("main", "."),
        ] {
            assert_eq!(
                resolve_ts_source_aliased(from, spec, &aliases),
                resolve_ts_source(from, spec),
                "{from} {spec}"
            );
        }
    }

    #[test]
    fn jsonc_comments_and_trailing_commas_parse() {
        let (base_url, entries) = parse_tsconfig_paths(ANGULAR).expect("jsonc");
        assert_eq!(base_url, "./src");
        assert_eq!(
            entries,
            [
                (
                    "@env".to_string(),
                    "environments/environment.ts".to_string()
                ),
                ("@core/*".to_string(), "app/core/*".to_string()),
            ],
            "exact keys first"
        );
        let stripped = strip_jsonc(ANGULAR);
        assert!(
            stripped.contains("./dist//out"),
            "a `//` inside a string stays: {stripped}"
        );
        assert!(!stripped.contains("angular.dev"), "a block comment goes");
        // No `compilerOptions.paths` is no aliases, not a parse failure; bad
        // JSON is a failure; `baseUrl` defaults to the tsconfig's dir.
        assert_eq!(
            parse_tsconfig_paths("{ // x\n}"),
            Some((".".to_string(), vec![]))
        );
        assert_eq!(parse_tsconfig_paths("{ \"compilerOptions\": "), None);
        let (base_url, _) =
            parse_tsconfig_paths(r#"{"compilerOptions":{"paths":{"~/*":["./src/*"]}}}"#).unwrap();
        assert_eq!(base_url, ".");
    }

    /// Exact keys before patterns, then the longest literal prefix (the TS
    /// rule); only a `paths` array's first target; never out of the repo.
    #[test]
    fn match_order_first_target_and_repo_bounds() {
        let aliases = set_of(&[(
            "",
            r#"{"compilerOptions":{"paths":{
                "@app/*": ["libs/app/*"],
                "@app/shared/*": ["libs/shared/src/*", "fallback/*"],
                "@app/config": ["config/index.ts"],
                "*.svg": ["assets/*.svg"],
                "@up/*": ["../../outside/*"]
            }}}"#,
        )]);
        let from = "apps::web::main";
        let r = |spec| resolve_ts_source_aliased(from, spec, &aliases);
        assert_eq!(
            r("@app/shared/button").as_deref(),
            Some("libs::shared::src::button")
        );
        assert_eq!(r("@app/home").as_deref(), Some("libs::app::home"));
        assert_eq!(
            r("@app/config").as_deref(),
            Some("config::index"),
            "exact beats `@app/*`"
        );
        assert_eq!(
            r("logo.svg").as_deref(),
            Some("assets::logo.svg"),
            "prefix + suffix pattern"
        );
        assert_eq!(r("@up/x"), None, "climbs out of the repo");
    }

    /// A file resolves through the deepest project dir whose tsconfig
    /// declares `paths`; that dir anchors the targets.
    #[test]
    fn the_nearest_scope_wins_and_anchors_targets() {
        let web = r#"{"compilerOptions":{"baseUrl":"./","paths":{"@app/core":["src/app/core/index.ts"]}}}"#;
        let root = r#"{"compilerOptions":{"paths":{"@lib/*":["libs/*"]}}}"#;
        let aliases = set_of(&[("", root), ("web", web)]);
        assert_eq!(
            resolve_ts_source_aliased("web::src::app::page", "@app/core", &aliases).as_deref(),
            Some("web::src::app::core::index")
        );
        assert_eq!(
            resolve_ts_source_aliased("web::src::app::page", "@lib/x", &aliases),
            None,
            "a nested tsconfig's `paths` replace the root's"
        );
        assert_eq!(
            resolve_ts_source_aliased("webapp::main", "@lib/x", &aliases).as_deref(),
            Some("libs::x"),
            "`webapp` is not under `web`"
        );
        assert_eq!(
            resolve_ts_source_aliased("api::main", "@app/core", &aliases),
            None
        );
    }

    fn write(dir: &Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    #[test]
    fn read_falls_back_to_tsconfig_base_json() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "tsconfig.json",
            r#"{"extends":"./tsconfig.base.json"}"#,
        );
        write(
            tmp.path(),
            "tsconfig.base.json",
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@org/ui":["libs/ui/src/index.ts"]}}}"#,
        );
        let a = read_tsconfig_paths(tmp.path());
        assert_eq!(a.file, "tsconfig.base.json");
        assert_eq!(a.entries.len(), 1);
        assert_eq!(
            read_tsconfig_paths(&tmp.path().join("missing")),
            TsAliases::default()
        );
    }

    /// End to end, in a nested Angular project root (the quokka-stack shape):
    /// the alias import is an IMPORTS edge, and it leaves the importer's
    /// IMPORTS library cell while the real package stays.
    #[test]
    fn an_aliased_import_is_an_edge_and_leaves_the_library_cell() {
        use glia_core::CellPayload;

        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "web/package.json",
            r#"{"name":"web","dependencies":{"@angular/core":"18.0.0"}}"#,
        );
        write(
            tmp.path(),
            "web/tsconfig.json",
            "{\n  // Angular CLI\n  \"compilerOptions\": {\"baseUrl\": \"./\", \"paths\": {\"@app/core\": [\"src/app/core/index.ts\"],}}\n}\n",
        );
        write(
            tmp.path(),
            "web/src/app/core/index.ts",
            "export class AuthService {}\n",
        );
        write(
            tmp.path(),
            "web/src/app/page.ts",
            "import { Injectable } from '@angular/core';\nimport { AuthService } from '@app/core';\nexport class Page { constructor(private a: AuthService) {} }\n",
        );
        let result = crate::build::generate_one(tmp.path().to_str().unwrap()).unwrap();
        let m = &result.merged;
        let module = |q: &str| {
            m.graphs.iter().find_map(|g| {
                let id = g.symbols.module_by_qname.get(q)?;
                g.nodes.iter().find(|n| n.id == *id)
            })
        };
        let page = module("web::src::app::page").expect("importer");
        let core = module("web::src::app::core::index").expect("alias target");
        assert!(
            m.all_edges().any(|e| e.category == edge_category::IMPORTS
                && e.from == page.id
                && e.to == core.id),
            "the alias import binds"
        );
        let cell = page
            .cells
            .iter()
            .find(|c| c.kind == cell_type::IMPORTS)
            .map(|c| match &c.payload {
                CellPayload::Json(s) | CellPayload::Text(s) => s.clone(),
                CellPayload::Bytes(_) => String::new(),
            });
        assert_eq!(cell.as_deref(), Some(r#"["@angular/core"]"#));
    }
}

#[cfg(test)]
mod workspace_tests {
    use super::*;
    use glia_code_domain::evidence::Evidence;

    /// A scratch tree under the system temp dir, removed on drop.
    struct Tree(PathBuf);

    impl Tree {
        fn new(tag: &str) -> Self {
            let d = std::env::temp_dir().join(format!("glia_lbws_{tag}_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            Tree(d)
        }

        fn write(&self, rel: &str, text: &str) -> &Self {
            let p = self.0.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
            self
        }

        fn read(&self, src: &str) -> String {
            std::fs::read_to_string(self.0.join(src)).unwrap()
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn npm(rel: &str) -> ProjectRoot {
        ProjectRoot::new(rel.to_string(), "npm", "package.json", None)
    }

    /// The pnpm-workspace-sdk fixture's shape: an SDK whose manifest names
    /// its build output, a source-only UI package, a web app importing both.
    fn sdk_tree(tag: &str) -> (Tree, Vec<ProjectRoot>) {
        let t = Tree::new(tag);
        t.write("package.json", r#"{"name":"acme","private":true}"#)
            .write("pnpm-workspace.yaml", "packages:\n  - 'packages/*'\n  - web\n")
            .write(
                "packages/sdk/package.json",
                r#"{"name":"@acme/sdk","main":"dist/index.js","types":"dist/index.d.ts",
                    "exports":{".":{"types":"./dist/index.d.ts","import":"./dist/index.js"},
                               "./fetch-client":"./dist/fetch-client.js",
                               "./models/*":"./dist/models/*.js"}}"#,
            )
            .write(
                "packages/sdk/src/index.ts",
                "export function updateMyUser(name: string): string {\n  return name;\n}\n",
            )
            .write(
                "packages/sdk/src/fetch-client.ts",
                "export function getServerVersion(): string {\n  return '1';\n}\n",
            )
            .write(
                "packages/sdk/src/models/user.ts",
                "export function userLabel(): string {\n  return 'u';\n}\n",
            )
            .write("packages/ui/package.json", r#"{"name":"@acme/ui"}"#)
            .write(
                "packages/ui/src/index.ts",
                "export function formatName(n: string): string {\n  return n.trim();\n}\n",
            )
            .write("packages/ui/src/theme/index.ts", "export const dark = 1;\n")
            .write("packages/nosrc/package.json", r#"{"name":"@acme/nosrc"}"#)
            .write(
                "web/package.json",
                r#"{"name":"@acme/web","dependencies":{"@acme/sdk":"workspace:*"}}"#,
            )
            .write(
                "web/src/profile.ts",
                "import { updateMyUser } from '@acme/sdk';\n\
                 import { getServerVersion } from '@acme/sdk/fetch-client';\n\
                 import { userLabel } from '@acme/sdk/models/user';\n\
                 import { formatName } from '@acme/ui';\n\
                 import { nothing } from '@acme/nosrc';\n\
                 import { debounce } from 'lodash';\n\
                 export function saveProfile(n: string): string {\n  return updateMyUser(formatName(n)) + getServerVersion() + userLabel();\n}\n",
            );
        let roots = ["", "packages/nosrc", "packages/sdk", "packages/ui", "web"]
            .iter()
            .map(|r| npm(r))
            .collect();
        (t, roots)
    }

    fn untag(raw: Option<String>) -> Option<(&'static str, String)> {
        let raw = raw?;
        for (tag, name) in [(WORKSPACE_ENTRY, "entry"), (WORKSPACE_SUBPATH, "subpath")] {
            if let Some(q) = raw.strip_prefix(tag) {
                return Some((name, q.to_string()));
            }
        }
        Some(("untagged", raw))
    }

    #[test]
    fn bare_specifiers_map_through_workspace_packages() {
        let (t, roots) = sdk_tree("resolve");
        let set = TsAliasSet {
            workspace: WorkspaceSet::read(&t.0, &roots),
            ..TsAliasSet::default()
        };
        let r = |spec: &str| untag(resolve_ts_source_aliased("web::src::profile", spec, &set));
        let q = |tag: &'static str, qname: &str| Some((tag, qname.to_string()));
        assert_eq!(r("@acme/sdk"), q("entry", "packages::sdk::src::index"));
        assert_eq!(
            r("'@acme/sdk'"),
            q("entry", "packages::sdk::src::index"),
            "quotes"
        );
        assert_eq!(
            r("@acme/sdk/fetch-client"),
            q("subpath", "packages::sdk::src::fetch-client"),
            "an exact exports subpath, its output mapped to source"
        );
        assert_eq!(
            r("@acme/sdk/models/user"),
            q("subpath", "packages::sdk::src::models::user"),
            "a `*` pattern"
        );
        assert_eq!(
            r("@acme/ui"),
            q("entry", "packages::ui::src::index"),
            "src/index.*"
        );
        assert_eq!(
            r("@acme/ui/src/theme"),
            q("subpath", "packages::ui::src::theme::index"),
            "no exports: the directory"
        );
        assert_eq!(
            r("@acme/ui/missing"),
            q("subpath", "packages::ui::missing"),
            "a subpath no file holds keeps its qname for the builder to miss"
        );
        assert_eq!(r("@acme/nosrc"), q("entry", ""), "no entry file: unbound");
        assert_eq!(r("lodash"), None);
        assert_eq!(r("@acme/gen"), None, "no workspace package");
        assert_eq!(r("node:fs"), None);
        assert_eq!(r("@acme"), None);
        assert_eq!(
            r("./util"),
            q("untagged", "web::src::util"),
            "relative is unchanged"
        );
        // A tsconfig `paths` key wins over a workspace name, as tsc reads it.
        let mut aliased = set.clone();
        let (base_url, entries) = parse_tsconfig_paths(
            r#"{"compilerOptions":{"paths":{"@acme/sdk":["web/src/sdk-shim.ts"]}}}"#,
        )
        .unwrap();
        aliased.scopes.push(TsAliases {
            file: "tsconfig.json",
            base_url,
            entries,
            ..TsAliases::default()
        });
        assert_eq!(
            untag(resolve_ts_source_aliased(
                "web::src::profile",
                "@acme/sdk",
                &aliased
            )),
            q("untagged", "web::src::sdk-shim")
        );
    }

    #[test]
    fn two_packages_of_one_name_bind_nothing() {
        let t = Tree::new("dup");
        t.write("a/package.json", r#"{"name":"dup","main":"index.js"}"#)
            .write("a/index.js", "\n")
            .write("b/package.json", r#"{"name":"dup","main":"index.js"}"#)
            .write("b/index.js", "\n");
        let set = WorkspaceSet::read(&t.0, &[npm("a"), npm("b")]);
        assert_eq!(untag(set.resolve("dup")), Some(("entry", String::new())));
        assert_eq!(
            set.marker(None, "fx").as_deref(),
            Some("[imports] workspace packages=2 bound=0 subpath=0 unbound=0 repo=fx")
        );
        assert_eq!(WorkspaceSet::default().marker(Some((1, 0, 0)), "fx"), None);
        assert_eq!(parse_workspace_report("3 1 2"), Some((3, 1, 2)));
        assert_eq!(parse_workspace_report("3 1"), None);
    }

    /// The round trip with `glia_graph`'s TS import pass: the tagged qnames
    /// bind with IMPORTS rule `workspace_package`, named imports bind their
    /// symbols, and the pass's counts come back for the marker.
    #[test]
    fn ts_family_binds_a_workspace_package() {
        let (t, roots) = sdk_tree("build");
        let repo = RepoId(7);
        let parses: Vec<FileParse> = [
            "packages/sdk/src/index.ts",
            "packages/sdk/src/fetch-client.ts",
            "packages/sdk/src/models/user.ts",
            "packages/ui/src/index.ts",
            "packages/ui/src/theme/index.ts",
            "web/src/profile.ts",
        ]
        .iter()
        .map(|p| crate::extract::parse_one(&t.read(p), p, "typescript", repo).unwrap())
        .collect();
        let aliases = TsAliasSet::read(&t.0, &roots, "fx");
        let ctx = GroupCtx {
            repo,
            repo_label: "fx",
            rust_crates: &[],
            ts_aliases: &aliases,
            c_includes: &IncludeRoots::default(),
            dart_packages: &DartPackages::default(),
            go_module_dirs: &[],
        };
        let build = build_one_group(
            GroupInput {
                key: TS_GROUP,
                parses,
            },
            &ctx,
        );
        let (g, _, _) = build.graph.as_ref().expect("built");
        let id = |q: &str| {
            *g.symbols
                .module_by_qname
                .get(q)
                .unwrap_or_else(|| panic!("{q}"))
        };
        let qname = |n: NodeId| g.nav.qname_by_id.get(&n).cloned().unwrap_or_default();
        let web = id("web::src::profile");
        let mut imports: Vec<(String, Option<String>)> = g
            .edges
            .iter()
            .filter(|e| e.category == edge_category::IMPORTS && e.from == web)
            .map(|e| (qname(e.to), Evidence::of(e).and_then(|ev| ev.rule)))
            .collect();
        imports.sort();
        let ws = Some("workspace_package".to_string());
        assert_eq!(
            imports,
            [
                ("packages::sdk::src::fetch-client".to_string(), ws.clone()),
                ("packages::sdk::src::index".to_string(), ws.clone()),
                ("packages::sdk::src::models::user".to_string(), ws.clone()),
                ("packages::ui::src::index".to_string(), ws.clone()),
            ]
        );
        let save = g
            .nav
            .qname_by_id
            .iter()
            .find(|(_, q)| q.as_str() == "web::src::profile::saveProfile")
            .map(|(id, _)| *id)
            .expect("saveProfile");
        let mut calls: Vec<String> = g
            .edges
            .iter()
            .filter(|e| e.category == edge_category::CALLS && e.from == save)
            .map(|e| qname(e.to))
            .collect();
        calls.sort();
        assert_eq!(
            calls,
            [
                "packages::sdk::src::fetch-client::getServerVersion",
                "packages::sdk::src::index::updateMyUser",
                "packages::sdk::src::models::user::userLabel",
                "packages::ui::src::index::formatName",
            ]
        );
        // The pass's counts, as the marker prints them: four bound (two
        // through a subpath), `@acme/nosrc` unbound; lodash is no workspace
        // package and counts nowhere.
        let report = std::cell::Cell::new(None);
        let resolver = |from: &str, spec: &str| {
            if from == WORKSPACE_REPORT {
                report.set(parse_workspace_report(spec));
                return None;
            }
            resolve_ts_source_aliased(from, spec, &aliases)
        };
        let parses: Vec<FileParse> = ["packages/sdk/src/index.ts", "web/src/profile.ts"]
            .iter()
            .map(|p| crate::extract::parse_one(&t.read(p), p, "typescript", repo).unwrap())
            .collect();
        glia_graph::build_typescript_family(repo, parses, resolver).unwrap();
        assert_eq!(
            report.get(),
            Some((1, 0, 4)),
            "only the sdk entry is in this graph"
        );
        assert_eq!(
            aliases.workspace.marker(Some((4, 2, 1)), "fx").as_deref(),
            Some("[imports] workspace packages=5 bound=4 subpath=2 unbound=1 repo=fx")
        );
    }
}

#[cfg(test)]
mod dart_import_tests {
    use super::*;

    /// A `pubspec.yaml` root at `rel` declaring `name: <name>`, as the walk
    /// detects it.
    fn pubspec(rel: &str, name: &str) -> ProjectRoot {
        ProjectRoot::new(
            rel.to_string(),
            "dart",
            "pubspec.yaml",
            Some(name.to_string()),
        )
    }

    fn pkgs(roots: &[ProjectRoot]) -> DartPackages {
        DartPackages::from_roots(roots)
    }

    /// An own-package URI names the file under that package's `lib/`, at the
    /// repo root and under a nested project root, quoted or not; each mapped
    /// lookup counts toward the marker, which takes the count.
    #[test]
    fn resolve_dart_source_maps_own_package() {
        let root = pkgs(&[pubspec("", "shop")]);
        assert_eq!(
            resolve_dart_source("lib::cart", "package:shop/src/util.dart", &root).as_deref(),
            Some("lib::src::util")
        );
        assert_eq!(
            resolve_dart_source("test::cart_test", "'package:shop/cart.dart'", &root).as_deref(),
            Some("lib::cart"),
            "from outside lib/, the same module"
        );
        assert_eq!(
            root.marker("fx").as_deref(),
            Some("[dart-imports] own-package uris resolved=2 packages=1 repo=fx")
        );
        assert_eq!(root.marker("fx"), None, "the marker takes the count");

        let nested = pkgs(&[pubspec("apps/mobile", "shop")]);
        assert_eq!(
            resolve_dart_source(
                "apps::mobile::lib::cart",
                "package:shop/src/util.dart",
                &nested
            )
            .as_deref(),
            Some("apps::mobile::lib::src::util")
        );
        assert_eq!(
            resolve_dart_source("tools::gen", "package:shop/./a/b/c.dart", &nested).as_deref(),
            Some("apps::mobile::lib::a::b::c"),
            "the importer's own directory plays no part"
        );
    }

    /// Another package's URI and a `dart:` library are external, as is a
    /// package URI that names no `.dart` file under `lib/`; none counts.
    #[test]
    fn foreign_package_is_external() {
        let root = pkgs(&[pubspec("", "shop")]);
        for spec in [
            "package:flutter/material.dart",
            "package:shop_extras/x.dart",
            "dart:async",
            "'dart:io'",
            "package:shop",
            "package:shop/",
            "package:shop/src/util",
            "package:shop/.dart",
            "package:shop/../web/main.dart",
        ] {
            assert_eq!(
                resolve_dart_source("lib::cart", spec, &root),
                None,
                "{spec}"
            );
        }
        assert_eq!(root.marker("fx"), None, "nothing resolved, no marker");
        let none = DartPackages::default();
        assert_eq!(
            resolve_dart_source("lib::cart", "package:shop/src/util.dart", &none),
            None,
            "no pubspec, no own package"
        );
    }

    /// A relative or bare `x.dart` import resolves exactly as
    /// `resolve_relative_source` resolves it.
    #[test]
    fn relative_dart_import_unchanged() {
        let root = pkgs(&[pubspec("", "shop")]);
        for (from, spec, want) in [
            ("lib::cart", "src/fmt.dart", Some("lib::src::fmt")),
            ("test::cart_test", "../lib/cart.dart", Some("lib::cart")),
            ("lib::src::fmt", "./money.dart", Some("lib::src::money")),
            ("lib::cart", "\"models.dart\"", Some("lib::models")),
        ] {
            assert_eq!(
                resolve_dart_source(from, spec, &root).as_deref(),
                want,
                "{spec}"
            );
            assert_eq!(
                resolve_dart_source(from, spec, &root),
                resolve_relative_source(from, spec),
                "{spec}"
            );
        }
        assert_eq!(
            root.marker("fx"),
            None,
            "a relative import is not an own-package URI"
        );
    }

    /// A name two pubspecs claim resolves to neither; the repo's other
    /// packages are untouched.
    #[test]
    fn duplicate_package_name_maps_nothing() {
        let p = pkgs(&[
            pubspec("apps/twin_a", "twin"),
            pubspec("apps/twin_b", "twin"),
            pubspec("", "shop"),
        ]);
        assert_eq!(
            resolve_dart_source("apps::twin_a::lib::a", "package:twin/b.dart", &p),
            None
        );
        assert_eq!(
            resolve_dart_source("apps::twin_a::lib::a", "package:shop/cart.dart", &p).as_deref(),
            Some("lib::cart")
        );
        assert_eq!(
            p.marker("fx").as_deref(),
            Some("[dart-imports] own-package uris resolved=1 packages=1 repo=fx")
        );
    }

    /// Only `pubspec.yaml` roots that declare their `name:` are packages: an
    /// npm or Cargo root of the same name and a pubspec whose label fell back
    /// to its directory name none.
    #[test]
    fn dart_packages_from_roots() {
        let roots = [
            pubspec("", "shop"),
            ProjectRoot::new("web".into(), "npm", "package.json", Some("web".into())),
            ProjectRoot::new(
                "crates/core".into(),
                "cargo",
                "Cargo.toml",
                Some("core".into()),
            ),
            ProjectRoot::new("apps/nameless".into(), "dart", "pubspec.yaml", None),
            pubspec("pkgs/ui", "ui"),
        ];
        let p = pkgs(&roots);
        assert_eq!(p.root_of("shop"), Some(""));
        assert_eq!(p.root_of("ui"), Some("pkgs::ui"));
        assert_eq!(p.root_of("web"), None);
        assert_eq!(p.root_of("core"), None);
        assert_eq!(p.root_of("nameless"), None);
        assert_eq!(p.packages(), 2);
    }

    fn write(dir: &Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    /// End to end through `generate_one`: an own-package import is an
    /// IMPORTS edge (`graph:imports`; rule `symbol` for a `show` import,
    /// `module` for an `as` one), the `show` / `as` binding behind it binds
    /// the call, a relative import keeps its edge, and a foreign `package:` /
    /// `dart:` import binds nothing.
    #[test]
    fn an_own_package_import_is_an_edge_and_binds_its_show_names() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "pubspec.yaml", "name: shop\n");
        write(
            tmp.path(),
            "lib/cart.dart",
            concat!(
                "import 'dart:async';\n",
                "import 'package:flutter/material.dart';\n",
                "import 'package:shop/src/util.dart' show shout;\n",
                "import 'package:shop/src/money.dart' as m;\n",
                "import 'src/fmt.dart';\n",
                "\n",
                "String total(double x) {\n",
                "  return shout(formatPrice(x + m.zero()));\n",
                "}\n",
            ),
        );
        write(
            tmp.path(),
            "lib/src/util.dart",
            "String shout(String s) {\n  return s;\n}\n",
        );
        write(
            tmp.path(),
            "lib/src/money.dart",
            "double zero() {\n  return 0.0;\n}\n",
        );
        write(
            tmp.path(),
            "lib/src/fmt.dart",
            "String formatPrice(double x) {\n  return '';\n}\n",
        );
        let result = crate::build::generate_one(tmp.path().to_str().unwrap()).unwrap();
        let m = &result.merged;
        let id_of = |q: &str| {
            m.graphs.iter().find_map(|g| {
                g.nodes
                    .iter()
                    .find(|n| g.nav.qname_by_id.get(&n.id).is_some_and(|nq| nq == q))
                    .map(|n| n.id)
            })
        };
        let edge = |cat, from: &str, to: &str| {
            let (Some(f), Some(t)) = (id_of(from), id_of(to)) else {
                return None;
            };
            m.all_edges()
                .find(|e| e.category == cat && e.from == f && e.to == t)
        };
        for (target, rule) in [("lib::src::util", "symbol"), ("lib::src::money", "module")] {
            let imports = edge(edge_category::IMPORTS, "lib::cart", target)
                .unwrap_or_else(|| panic!("the own-package import of {target} binds"));
            let ev = evidence::Evidence::of(imports).expect("evidence");
            assert_eq!(ev.emitter, "graph:imports");
            assert_eq!(ev.rule.as_deref(), Some(rule), "{target}");
        }
        assert!(
            edge(edge_category::IMPORTS, "lib::cart", "lib::src::fmt").is_some(),
            "the relative import keeps its edge"
        );
        assert!(
            edge(
                edge_category::CALLS,
                "lib::cart::total",
                "lib::src::util::shout"
            )
            .is_some(),
            "the `show` binding behind the import binds the call"
        );
        assert!(
            edge(
                edge_category::CALLS,
                "lib::cart::total",
                "lib::src::money::zero"
            )
            .is_some(),
            "the `as` prefix behind the import binds the call"
        );
        let cart = id_of("lib::cart").expect("importer");
        let targets: Vec<NodeId> = m
            .all_edges()
            .filter(|e| e.category == edge_category::IMPORTS && e.from == cart)
            .map(|e| e.to)
            .collect();
        assert_eq!(
            targets.len(),
            3,
            "no edge for `dart:async` or `package:flutter`"
        );
    }
}

#[cfg(test)]
mod bundler_tests {
    use super::*;

    fn write(dir: &Path, rel: &str, text: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    /// The webpack-resolve-modules fixture's config: the root lives in a
    /// const object's member, `node_modules` twice (a literal and a path).
    const FIXTURE: &str = "const path = require('path');\n\nconst PATHS = {\n  src: path.join(__dirname, 'src'),\n  modules: path.join(__dirname, 'node_modules'),\n};\n\nmodule.exports = {\n  entry: './src/page.js',\n  resolve: {\n    modules: [PATHS.src, 'node_modules', PATHS.modules],\n  },\n};\n";

    /// A conditional `modules` list (both arms read), a root assigned
    /// outside the const literal (skipped), package aliases (skipped), and a
    /// regex holding a quote that must not open a string.
    const CONDITIONAL: &str = r#"/* header with a "quote */
const path = require("path");
const isCustom = !!process.env.CUSTOM_SRC;
const PATHS = {
  src: path.join(__dirname, "src"),
  modules: path.join(__dirname, "node_modules"), // trailing comment
  build: path.join(__dirname, "../assets/build"),
};
if (isCustom) {
  PATHS.custom = path.join(__dirname, process.env.CUSTOM_SRC);
}
const config = {
  module: { rules: [{ test: /'\.js$/, exclude: [/\.map$/] }] },
  resolve: {
    modules: isCustom ? [PATHS.custom, PATHS.src, "node_modules", PATHS.modules] : [PATHS.src, "node_modules", PATHS.modules],
    preferRelative: true,
    alias: {
      "vue$": "vue/dist/vue.runtime.esm-bundler.js",
      "hls.js": "hls.js/dist/hls.light.min.js",
      common$: path.resolve(__dirname, "src/common/index.js"),
      "@": `${__dirname}/src`,
    },
  },
};
module.exports = config;
"#;

    #[test]
    fn webpack_modules_read_through_a_const_member() {
        let r = read_bundler_config(FIXTURE, "web", false);
        assert_eq!(r.module_roots, ["web/src"]);
        assert!(r.aliases.is_empty());
        assert_eq!(r.skipped, 0);
    }

    #[test]
    fn a_conditional_list_reads_both_arms_and_counts_what_it_skips() {
        let r = read_bundler_config(CONDITIONAL, "frontend", false);
        assert_eq!(r.module_roots, ["frontend/src"]);
        assert_eq!(
            r.aliases,
            [BundlerAlias {
                key: "common".into(),
                exact: true,
                target: "frontend/src/common/index.js".into(),
            }]
        );
        // PATHS.custom, the two package aliases, the templated `@`.
        assert_eq!(r.skipped, 4);
    }

    #[test]
    fn vite_aliases_object_array_and_url_forms() {
        let object = "import { fileURLToPath, URL } from 'node:url'\nexport default defineConfig({\n  resolve: {\n    alias: {\n      '@': fileURLToPath(new URL('./src', import.meta.url)),\n      '~lib': '/lib',\n    },\n  },\n})\n";
        let r = read_bundler_config(object, "", true);
        let got: Vec<(&str, &str)> = r
            .aliases
            .iter()
            .map(|a| (a.key.as_str(), a.target.as_str()))
            .collect();
        assert_eq!(got, [("@", "src"), ("~lib", "lib")]);
        assert_eq!(r.skipped, 0);
        // A `/lib` alias is an absolute path to webpack: skipped there.
        assert_eq!(read_bundler_config(object, "", false).skipped, 1);

        let array = "import path from 'path'\nconst alias = [\n  { find: '@', replacement: path.resolve(__dirname, './src') },\n  { find: /^~/, replacement: '' },\n]\nexport default { resolve: { alias } }\n";
        let r = read_bundler_config(array, "app", true);
        let got: Vec<(&str, &str)> = r
            .aliases
            .iter()
            .map(|a| (a.key.as_str(), a.target.as_str()))
            .collect();
        assert_eq!(got, [("@", "app/src")]);
        assert_eq!(r.skipped, 1, "the regex `find`");
        // A `modules` value holding no array, and a path leaving the repo.
        let odd = "module.exports = { resolve: { modules: roots(), alias: { x: path.join(__dirname, '../../out') } } }";
        let r = read_bundler_config(odd, "web", false);
        assert!(r.module_roots.is_empty() && r.aliases.is_empty());
        assert_eq!(r.skipped, 2);
    }

    #[test]
    fn module_roots_bind_only_files_that_exist() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, "package.json", "{}");
        write(root, "web/package.json", "{\"name\":\"web\"}");
        write(root, "web/webpack.config.js", CONDITIONAL);
        write(root, "web/src/common/api.js", "export const a = 1;\n");
        write(root, "web/src/common/index.js", "export const b = 1;\n");
        write(
            root,
            "web/src/component/notify.vue",
            "<template></template>\n",
        );
        write(root, "web/src/css/app.css", "a {}\n");
        write(root, "web/src/vue.js", "export default 1;\n");
        write(
            root,
            "tsconfig.json",
            r#"{"compilerOptions":{"paths":{"common/api":["web/src/other.ts"]}}}"#,
        );
        let roots = [
            ProjectRoot::new(String::new(), "npm", "package.json", None),
            ProjectRoot::new("web".into(), "npm", "package.json", None),
        ];
        let set = TsAliasSet::read(root, &roots, "fx");
        let from = "web::src::page";
        let r = |spec: &str| resolve_ts_source_aliased(from, spec, &set);
        // tsconfig `paths` first.
        assert_eq!(r("common/api").as_deref(), Some("web::src::other"));
        // The exact alias, then the module root.
        assert_eq!(r("common").as_deref(), Some("web::src::common::index"));
        assert_eq!(
            r("common/index").as_deref(),
            Some("web::src::common::index")
        );
        assert_eq!(
            r("component/notify.vue").as_deref(),
            Some("web::src::component::notify")
        );
        // An in-repo file shadows the package of its name only when it exists.
        assert_eq!(r("vue").as_deref(), Some("web::src::vue"));
        assert_eq!(r("react"), None);
        // A non-source file under a root stays a package import.
        assert_eq!(r("css/app.css"), None);
        // A module outside the config's dir does not read it.
        assert_eq!(
            resolve_ts_source_aliased("tools::x", "common/index", &set),
            None
        );
        let keys: Vec<&str> = set.keys().collect();
        assert_eq!(keys, ["common/api", "common"]);
        assert!(is_bundler_config("vite.config.ts") && is_bundler_config("vitest.config.js"));
        assert!(!is_bundler_config("vitest.workspace.ts"));
    }

    /// FH.16b: a `vitest.config.*` is a vite-read bundler config. An alias
    /// declared only there binds; a key both declare binds through the vite
    /// config's target (vitest's usually spreads it).
    #[test]
    fn vitest_config_aliases_bind_and_vite_wins() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, "package.json", "{\"name\":\"web\"}");
        write(
            root,
            "vite.config.ts",
            "import { defineConfig } from 'vite'\nimport path from 'path'\nexport default defineConfig({ resolve: { alias: { '@': path.resolve(__dirname, './src') } } })\n",
        );
        write(
            root,
            "vitest.config.ts",
            "import { defineConfig } from 'vitest/config'\nimport { fileURLToPath } from 'node:url'\nexport default defineConfig({\n  resolve: {\n    alias: [\n      { find: '@', replacement: fileURLToPath(new URL('./test-src', import.meta.url)) },\n      { find: '~test', replacement: '/tests/helpers' },\n    ],\n  },\n  test: { environment: 'jsdom' },\n})\n",
        );
        let roots = [ProjectRoot::new(String::new(), "npm", "package.json", None)];
        let set = TsAliasSet::read(root, &roots, "fx");
        let r = |spec: &str| resolve_ts_source_aliased("src::page", spec, &set);
        // Declared only in vitest.config.ts.
        assert_eq!(r("~test/render").as_deref(), Some("tests::helpers::render"));
        // Declared in both: the vite target.
        assert_eq!(r("@/lib/api").as_deref(), Some("src::lib::api"));
        let keys: Vec<&str> = set.keys().collect();
        assert_eq!(keys, ["@", "@/*", "~test", "~test/*"]);
    }

    /// The fixture end to end through the TS-family build: the IMPORTS edge
    /// and the named import's CALLS.
    #[test]
    fn ts_family_binds_through_a_module_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, "package.json", "{\"name\":\"web\"}");
        write(root, "webpack.config.js", FIXTURE);
        let api = "export function getUser(id) {\n  return fetch('/api/users/' + id);\n}\n";
        let page = "import { getUser } from 'common/api';\n\nexport function show() {\n  return getUser(1);\n}\n";
        write(root, "src/common/api.js", api);
        write(root, "src/page.js", page);
        let roots = [ProjectRoot::new(String::new(), "npm", "package.json", None)];
        let set = TsAliasSet::read(root, &roots, "fx");
        let repo = RepoId(3);
        let parses: Vec<FileParse> = [("src/common/api.js", api), ("src/page.js", page)]
            .iter()
            .map(|(p, text)| crate::extract::parse_one(text, p, "typescript", repo).unwrap())
            .collect();
        let g = glia_graph::build_typescript_family(repo, parses, |from, spec| {
            resolve_ts_source_aliased(from, spec, &set)
        })
        .unwrap();
        let qname = |n: NodeId| g.nav.qname_by_id.get(&n).cloned().unwrap_or_default();
        let mut edges: Vec<(String, String)> = g
            .edges
            .iter()
            .filter(|e| e.category == edge_category::IMPORTS || e.category == edge_category::CALLS)
            .map(|e| (qname(e.from), qname(e.to)))
            .filter(|(f, _)| f.starts_with("src::page"))
            .collect();
        edges.sort();
        assert_eq!(
            edges,
            [
                ("src::page".to_string(), "src::common::api".to_string()),
                (
                    "src::page::show".to_string(),
                    "src::common::api::getUser".to_string()
                ),
            ]
        );
    }
}
