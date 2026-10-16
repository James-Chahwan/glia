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

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use glia_code_domain::project_roots::{LABEL_MANIFEST, ProjectRoot};
use glia_code_domain::{
    FileParse, cell_type, edge_category, evidence, node_kind, recv_stats,
};
use glia_core::{EdgeCategoryId, NodeId, RepoId};
use glia_graph::{GraphError, RepoGraph};
use glia_graph::rust_paths::RustCrate;

use crate::extract::{TS_FAMILY, build_group, path_to_qname};
use crate::passes::{GroupFill, fill_group_evidence};

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
            (matrix_row(*lang), n)
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
            (matrix_row(*lang), n)
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
        groups.push(GroupInput { key: TS_GROUP, parses: ts_family });
    }
    GroupPlan { groups, di_refs, recv_fields }
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
    let built: Vec<&'static str> =
        items.iter().filter(|(_, taken)| taken.is_none()).map(|(input, _)| input.key).collect();
    let reused = items.len() - built.len();
    let (builds, _threads) = crate::parallel::par_map_owned(items, |(input, taken)| match taken {
        Some(build) => Ok(build),
        None => crate::parallel::quiet(|| build_one_group(input, ctx)),
    });
    let builds: Vec<LangBuild> = builds
        .into_iter()
        .map(|build| build.unwrap_or_else(|payload| std::panic::resume_unwind(payload)))
        .collect();
    let keys = if built.is_empty() { "-".to_string() } else { built.join(",") };
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
        build_one(key, parses, |parses| {
            glia_graph::build_typescript_family(ctx.repo, parses, |from, spec| {
                resolve_ts_source_aliased(from, spec, ctx.ts_aliases)
            })
        })
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

    LanguageGraphs { graphs, keys, fills, pooled }
}

/// One non-TS build group's graph: the `build_*` its language takes, with
/// the repo-level inputs of `ctx` it reads. The C/C++ build prints CB.22's
/// `[c-includes]` line after it ([`IncludeRoots::marker`]), on the thread
/// that ran it. Dart takes [`resolve_dart_source`] (DF.4a); Solidity,
/// Terraform and every other `_`-arm language keep
/// [`resolve_relative_source`].
fn build_solo(
    lang: &'static str,
    parses: Vec<FileParse>,
    ctx: &GroupCtx<'_>,
) -> Result<RepoGraph, GraphError> {
    let GroupCtx { repo, repo_label, rust_crates, c_includes, dart_packages, .. } = *ctx;
    match lang {
        "python" => glia_graph::build_python(repo, parses),
        "go" => glia_graph::build_go(repo, parses),
        "java" | "kotlin" | "csharp" | "php" | "scala" | "clojure" => {
            glia_graph::build_dotted(repo, parses)
        }
        "elixir" => glia_graph::build_elixir(repo, parses),
        "rust" => glia_graph::build_rust(repo, parses, rust_crates),
        "ruby" => glia_graph::build_ruby(repo, parses),
        "c_cpp" => {
            let modules = c_cpp_module_qnames(&parses);
            let angle = angle_includes(&parses);
            let resolver = c_includes.resolver(&modules);
            let graph =
                glia_graph::build_c_cpp(repo, parses, |from, spec| resolver.resolve(from, spec));
            eprintln!("{}", c_includes.marker(angle, resolver.bound_via_roots(), repo_label));
            graph
        }
        "swift" => glia_graph::build_swift(repo, parses, resolve_relative_source),
        "dart" => glia_graph::build_typescript(repo, parses, |from, spec| {
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
        Self { lang: key, graph: Err(line.to_string()), recv_bound: 0 }
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
/// the fill). `recv_stats` is per thread (LG.1c) and the graph crate spawns no
/// threads, so the `reset` .. `take` bracket holds exactly this build's
/// receiver-typed binds.
fn build_one(
    lang: &'static str,
    parses: Vec<FileParse>,
    build: impl FnOnce(Vec<FileParse>) -> Result<RepoGraph, GraphError>,
) -> LangBuild {
    let unattributed = unattributed_parse_edges(&parses);
    recv_stats::reset();
    let graph = build(parses);
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
            let site = ev.as_ref().is_some_and(|ev| ev.basis == evidence::Basis::Site);
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
        let unresolved = g.unresolved_refs.iter().filter(|r| is_heritage(r.category)).count();
        HeritageTally { lang, bound, unresolved }
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
    let bound: Vec<String> = tallies.iter().map(|t| format!("{}={}", t.lang, t.bound)).collect();
    let unresolved: Vec<String> =
        tallies.iter().map(|t| format!("{}={}", t.lang, t.unresolved)).collect();
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
            let Some(rel) = path.strip_prefix(prefix.as_str()).filter(|r| r.ends_with(".rs"))
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
    let Some(host) = parses_by_lang.iter().position(|(lang, _)| *lang == JVM_HOST) else {
        return;
    };
    let (_, kotlin) = parses_by_lang.remove(guest);
    // `host < guest` in sorted order, so the removal left `host` in place.
    parses_by_lang[host].1.extend(kotlin);
}

/// The matrix row a parse-language tag reports under: the TS-family tags
/// (angular / react / vue) are the `typescript` row.
fn matrix_row<'a>(lang: &'a str) -> &'a str {
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
        let target = self.entries.iter().find_map(|(key, target)| match key.split_once('*') {
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
#[derive(Debug, Clone, Default)]
pub(crate) struct TsAliasSet {
    scopes: Vec<TsAliases>,
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
        for dir in dirs {
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
        Self { scopes }
    }

    /// The aliases a module resolves through: the deepest scope whose dir
    /// holds it.
    fn scope_of(&self, from_module: &str) -> Option<&TsAliases> {
        self.scopes
            .iter()
            .filter(|s| {
                s.dir_qname.is_empty()
                    || from_module
                        .strip_prefix(s.dir_qname.as_str())
                        .is_some_and(|rest| rest.is_empty() || rest.starts_with("::"))
            })
            .max_by_key(|s| s.dir_qname.len())
    }

    /// Every `paths` key of every scope, as written (`@core/*`), for the
    /// A16.4 IMPORTS-cell filter (`LocalModuleIndex::add_alias_prefix`).
    pub(super) fn keys(&self) -> impl Iterator<Item = &str> + '_ {
        self.scopes.iter().flat_map(|s| s.entries.iter().map(|(k, _)| k.as_str()))
    }
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
                return TsAliases { file, base_url, entries, ..TsAliases::default() };
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
    aliases.scope_of(from_module)?.map(spec)
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
        Self { by_name, resolved: AtomicUsize::new(0) }
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
    let parts: Vec<&str> = rest.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
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
                let stem = SRC_EXT
                    .iter()
                    .find_map(|e| p.strip_suffix(e))
                    .unwrap_or(p);
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
            ("src::cart.cpp", "../include/shop/cart.hpp", Some("include::shop::cart.hpp")),
            ("main.cpp", "mathutil.h", Some("mathutil.h")),
            ("src::a.cpp", "./detail/b.h", Some("src::detail::b.h")),
            ("src::a.cpp", "\"quoted.h\"", Some("src::quoted.h")),
            ("win::a.cpp", "sub\\b.h", Some("win::sub::b.h")),
            ("a.cpp", "../x.h", None),
            ("src::a.c", "/usr/include/x.h", None),
            ("src::a.c", "", None),
        ] {
            assert_eq!(resolve_include_source(from, spec).as_deref(), want, "{from} {spec}");
        }
    }

    fn tally(lang: &'static str, bound: usize, unresolved: usize) -> HeritageTally {
        HeritageTally { lang, bound, unresolved }
    }

    /// Every language graph built gets one token per half, sorted by name
    /// (the TS family is built last but sorts in place), so A6.4 / A6.5 grep
    /// ` solidity=` / ` dart=` off the same line.
    #[test]
    fn heritage_marker_lists_every_built_language_sorted() {
        let mut t = vec![tally("solidity", 0, 1), tally("dart", 0, 0), tally("typescript", 2, 0)];
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
            edge(1, 2, edge_category::CALLS, Some(Evidence::emitter("graph:build"))),
            edge(1, 3, edge_category::CALLS, Some(Evidence::emitter("parser:python"))),
            edge(1, 2, edge_category::IMPORTS, Some(site("graph:imports"))),
            edge(1, 2, edge_category::HANDLED_BY, Some(site("graph:refs"))),
            edge(1, 2, edge_category::USES, Some(Evidence::emitter("graph:rust_paths").rule("enum_variant"))),
            edge(1, 2, edge_category::USES, Some(site("graph:rust_paths").rule("glob"))),
            edge(1, 2, edge_category::DEFINES, None),
        ];
        let t = LineTally::of(&g);
        assert_eq!(
            t,
            LineTally { calls: 2, calls_site: 1, imports: 1, imports_site: 1, refs: 2, refs_site: 1 }
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
        TsAliasSet { scopes }
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
        assert_eq!(resolve_ts_source_aliased(page, "@angular/core", &aliases), None);
        assert_eq!(resolve_ts_source_aliased(page, "@environment", &aliases), None, "exact is whole");
        assert_eq!(resolve_ts_source_aliased(page, "rxjs", &aliases), None);
        // No tsconfig: every bare specifier is a package, as before A6.8.
        assert_eq!(resolve_ts_source_aliased(page, "@core/x", &TsAliasSet::default()), None);
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
                ("@env".to_string(), "environments/environment.ts".to_string()),
                ("@core/*".to_string(), "app/core/*".to_string()),
            ],
            "exact keys first"
        );
        let stripped = strip_jsonc(ANGULAR);
        assert!(stripped.contains("./dist//out"), "a `//` inside a string stays: {stripped}");
        assert!(!stripped.contains("angular.dev"), "a block comment goes");
        // No `compilerOptions.paths` is no aliases, not a parse failure; bad
        // JSON is a failure; `baseUrl` defaults to the tsconfig's dir.
        assert_eq!(parse_tsconfig_paths("{ // x\n}"), Some((".".to_string(), vec![])));
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
        assert_eq!(r("@app/shared/button").as_deref(), Some("libs::shared::src::button"));
        assert_eq!(r("@app/home").as_deref(), Some("libs::app::home"));
        assert_eq!(r("@app/config").as_deref(), Some("config::index"), "exact beats `@app/*`");
        assert_eq!(r("logo.svg").as_deref(), Some("assets::logo.svg"), "prefix + suffix pattern");
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
        assert_eq!(resolve_ts_source_aliased("api::main", "@app/core", &aliases), None);
    }

    fn write(dir: &Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    #[test]
    fn read_falls_back_to_tsconfig_base_json() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "tsconfig.json", r#"{"extends":"./tsconfig.base.json"}"#);
        write(
            tmp.path(),
            "tsconfig.base.json",
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@org/ui":["libs/ui/src/index.ts"]}}}"#,
        );
        let a = read_tsconfig_paths(tmp.path());
        assert_eq!(a.file, "tsconfig.base.json");
        assert_eq!(a.entries.len(), 1);
        assert_eq!(read_tsconfig_paths(&tmp.path().join("missing")), TsAliases::default());
    }

    /// End to end, in a nested Angular project root (the quokka-stack shape):
    /// the alias import is an IMPORTS edge, and it leaves the importer's
    /// IMPORTS library cell while the real package stays.
    #[test]
    fn an_aliased_import_is_an_edge_and_leaves_the_library_cell() {
        use glia_core::CellPayload;

        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "web/package.json", r#"{"name":"web","dependencies":{"@angular/core":"18.0.0"}}"#);
        write(
            tmp.path(),
            "web/tsconfig.json",
            "{\n  // Angular CLI\n  \"compilerOptions\": {\"baseUrl\": \"./\", \"paths\": {\"@app/core\": [\"src/app/core/index.ts\"],}}\n}\n",
        );
        write(tmp.path(), "web/src/app/core/index.ts", "export class AuthService {}\n");
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
mod dart_import_tests {
    use super::*;

    /// A `pubspec.yaml` root at `rel` declaring `name: <name>`, as the walk
    /// detects it.
    fn pubspec(rel: &str, name: &str) -> ProjectRoot {
        ProjectRoot::new(rel.to_string(), "dart", "pubspec.yaml", Some(name.to_string()))
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
            resolve_dart_source("apps::mobile::lib::cart", "package:shop/src/util.dart", &nested)
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
            assert_eq!(resolve_dart_source("lib::cart", spec, &root), None, "{spec}");
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
            assert_eq!(resolve_dart_source(from, spec, &root).as_deref(), want, "{spec}");
            assert_eq!(
                resolve_dart_source(from, spec, &root),
                resolve_relative_source(from, spec),
                "{spec}"
            );
        }
        assert_eq!(root.marker("fx"), None, "a relative import is not an own-package URI");
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
        assert_eq!(resolve_dart_source("apps::twin_a::lib::a", "package:twin/b.dart", &p), None);
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
            ProjectRoot::new("crates/core".into(), "cargo", "Cargo.toml", Some("core".into())),
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
        write(tmp.path(), "lib/src/util.dart", "String shout(String s) {\n  return s;\n}\n");
        write(tmp.path(), "lib/src/money.dart", "double zero() {\n  return 0.0;\n}\n");
        write(tmp.path(), "lib/src/fmt.dart", "String formatPrice(double x) {\n  return '';\n}\n");
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
            m.all_edges().find(|e| e.category == cat && e.from == f && e.to == t)
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
            edge(edge_category::CALLS, "lib::cart::total", "lib::src::util::shout").is_some(),
            "the `show` binding behind the import binds the call"
        );
        assert!(
            edge(edge_category::CALLS, "lib::cart::total", "lib::src::money::zero").is_some(),
            "the `as` prefix behind the import binds the call"
        );
        let cart = id_of("lib::cart").expect("importer");
        let targets: Vec<NodeId> = m
            .all_edges()
            .filter(|e| e.category == edge_category::IMPORTS && e.from == cart)
            .map(|e| e.to)
            .collect();
        assert_eq!(targets.len(), 3, "no edge for `dart:async` or `package:flutter`");
    }
}
