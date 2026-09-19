//! The per-language graph build of `build_graphs_for_repo`: a deterministic
//! language order, the `build_*` dispatch, one shared TS-family graph, and the
//! relative-import resolvers the `build_typescript` arm takes.

use std::collections::HashMap;

use repo_graph_code_domain::project_roots::ProjectRoot;
use repo_graph_code_domain::{FileParse, edge_category, node_kind, recv_stats};
use repo_graph_core::RepoId;
use repo_graph_graph::RepoGraph;
use repo_graph_graph::rust_paths::RustCrate;

use crate::extract::path_to_qname;

/// TS-family lang tags (typescript/angular/react/vue) share ONE module + symbol
/// space in a repo: an Angular component (`.component.ts` → "angular") injects a
/// service (`.service.ts` → "typescript"), and imports cross those tags. Build
/// them as a single graph so intra-repo ref/import resolution works across the
/// tag boundary (Pattern E DI, Pattern B imports). Other `_`-arm langs
/// (dart/swift/c_cpp/solidity/terraform) keep separate graphs — distinct symbol
/// spaces that must not cross-resolve. ts_family accumulates in the sorted lang
/// order and is built last, so graph/shard order stays deterministic.
const TS_FAMILY: &[&str] = &["angular", "react", "typescript", "vue"];

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

/// Build one repo's per-language graphs from its finished parses, in sorted
/// language order with the TS family last. Returns the graphs and the A7.0
/// `[di]` marker input (INJECTS refs per matrix row); graph build failures go
/// to `parse_errors`. Prints the A6.2a `[recv]` and A6.3 `[heritage]` markers
/// for `repo_label`.
/// `rust_crates` ([`rust_crates`]) feeds `build_rust`'s path resolver (LA.1a).
pub(super) fn build_language_graphs(
    parses_by_lang: HashMap<&'static str, Vec<FileParse>>,
    repo: RepoId,
    repo_label: &str,
    rust_crates: &[RustCrate],
    parse_errors: &mut Vec<String>,
) -> (Vec<RepoGraph>, Vec<(&'static str, usize)>) {
    let mut graphs = Vec::new();
    // Deterministic per-language build order: HashMap iteration is seeded per
    // process, and the resulting `graphs` order decides shard indices in
    // `write_sharded` (repo-<hash>-NN.gmap). Random order made every shard's
    // content hash flap across processes, so the write-side skip-unchanged-
    // shards optimization never fired (audit 2026-06-10 #5).
    let mut parses_by_lang: Vec<(&str, Vec<FileParse>)> = parses_by_lang.into_iter().collect();
    parses_by_lang.sort_unstable_by_key(|(lang, _)| *lang);
    // A14.2 `[kotlin] entities:` fired_on, counted off the parses so
    // cache-served files count too; the kotlin crate owns the line.
    if let Some((_, kotlin)) = parses_by_lang.iter().find(|(lang, _)| *lang == JVM_GUEST) {
        repo_graph_parser_kotlin::trace(kotlin, repo_label);
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
    let mut recv_bound: Vec<(&str, usize)> = Vec::new();
    let mut heritage: Vec<HeritageTally> = Vec::new();
    recv_stats::reset();
    let mut ts_family: Vec<FileParse> = Vec::new();
    for (lang, parses) in parses_by_lang {
        if TS_FAMILY.contains(&lang) {
            ts_family.extend(parses);
            continue;
        }
        let graph = match lang {
            "python" => repo_graph_graph::build_python(repo, parses),
            "go" => repo_graph_graph::build_go(repo, parses),
            "java" | "kotlin" | "csharp" | "php" | "scala" | "clojure" | "elixir" => {
                repo_graph_graph::build_dotted(repo, parses)
            }
            "rust" => repo_graph_graph::build_rust(repo, parses, rust_crates),
            "ruby" => repo_graph_graph::build_ruby(repo, parses),
            _ => repo_graph_graph::build_typescript(repo, parses, resolve_relative_source),
        };
        recv_bound.push((lang, recv_stats::take()));
        match graph {
            Ok(g) => {
                heritage.push(HeritageTally::of(lang, &g));
                graphs.push(g);
            }
            Err(e) => parse_errors.push(format!("{lang} graph: {e}")),
        }
    }
    if !ts_family.is_empty() {
        let graph = repo_graph_graph::build_typescript(repo, ts_family, resolve_ts_source);
        recv_bound.push(("typescript", recv_stats::take()));
        match graph {
            Ok(g) => {
                heritage.push(HeritageTally::of("typescript", &g));
                graphs.push(g);
            }
            Err(e) => parse_errors.push(format!("typescript graph: {e}")),
        }
    }
    // A6.2a fired_on marker, once per repo, every `recv_stats::LANGS` row
    // zero-filled (LA.35a adds `rust`: bound = the field- and local-typed
    // binds of `build_rust`, fields = the Rust parses' struct field types):
    //   `[recv] receiver-typed calls bound: csharp=N … rust=N (fields: csharp=F … rust=F) repo=<label>`
    recv_stats::flush_marker(&recv_bound, &recv_fields, repo_label);
    // A6.3 fired_on marker, once per repo (A6.4 / A6.5 read their rows here):
    //   `[heritage] refs bound: dart=N … typescript=N unresolved: dart=M … repo=<label>`
    if let Some(line) = heritage_marker(&mut heritage, repo_label) {
        eprintln!("{line}");
    }

    (graphs, di_refs)
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

fn is_heritage(category: repo_graph_core::EdgeCategoryId) -> bool {
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
pub(super) fn rust_crates(files: &[(String, String)], roots: &[ProjectRoot]) -> Vec<RustCrate> {
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
fn join_jvm_family(parses_by_lang: &mut Vec<(&str, Vec<FileParse>)>) {
    let Some(guest) = parses_by_lang.iter().position(|(lang, _)| *lang == JVM_GUEST) else {
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

/// Relative-import resolver for the non-TS `_`-arm languages (dart / c_cpp /
/// solidity). Handles dotted specifiers (`./x`, `../a/b`) AND bare filenames
/// that carry a source extension (`import 'models.dart'`, `#include
/// "mathutil.h"`) — both resolve against the importing file's directory to the
/// `path_to_qname` form. A bare specifier with no source extension (a package /
/// system import like `package:collection`, `import Foundation`, `<stdio.h>`)
/// is external → None. Superset of `resolve_ts_source`; kept separate so the
/// verified TS-family path is untouched.
fn resolve_relative_source(from_module: &str, specifier: &str) -> Option<String> {
    const SRC_EXT: &[&str] = &[
        ".dart", ".h", ".hpp", ".hh", ".hxx", ".sol", ".swift", ".ts", ".tsx", ".js", ".jsx",
    ];
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

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn heritage_marker_is_silent_without_heritage() {
        let mut t = vec![tally("go", 0, 0), tally("python", 0, 0)];
        assert_eq!(heritage_marker(&mut t, "fx"), None);
        assert_eq!(heritage_marker(&mut [], "fx"), None);
    }
}
