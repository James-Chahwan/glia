//! The per-language graph build of `build_graphs_for_repo`: a deterministic
//! language order, the `build_*` dispatch, one shared TS-family graph, and the
//! relative-import resolvers the `build_typescript` arm takes.

use std::collections::HashMap;

use repo_graph_code_domain::{FileParse, edge_category, recv_stats};
use repo_graph_core::RepoId;
use repo_graph_graph::RepoGraph;

/// TS-family lang tags (typescript/angular/react/vue) share ONE module + symbol
/// space in a repo: an Angular component (`.component.ts` → "angular") injects a
/// service (`.service.ts` → "typescript"), and imports cross those tags. Build
/// them as a single graph so intra-repo ref/import resolution works across the
/// tag boundary (Pattern E DI, Pattern B imports). Other `_`-arm langs
/// (dart/swift/c_cpp/solidity/terraform) keep separate graphs — distinct symbol
/// spaces that must not cross-resolve. ts_family accumulates in the sorted lang
/// order and is built last, so graph/shard order stays deterministic.
const TS_FAMILY: &[&str] = &["angular", "react", "typescript", "vue"];

/// Build one repo's per-language graphs from its finished parses, in sorted
/// language order with the TS family last. Returns the graphs and the A7.0
/// `[di]` marker input (INJECTS refs per matrix row); graph build failures go
/// to `parse_errors`. Prints the A6.2a `[recv]` marker for `repo_label`.
pub(super) fn build_language_graphs(
    parses_by_lang: HashMap<&'static str, Vec<FileParse>>,
    repo: RepoId,
    repo_label: &str,
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
    let mut recv_bound: Vec<(&str, usize)> = Vec::new();
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
            "java" | "csharp" | "php" | "rust" | "scala" | "clojure" | "elixir" => {
                repo_graph_graph::build_dotted(repo, parses)
            }
            "ruby" => repo_graph_graph::build_ruby(repo, parses),
            _ => repo_graph_graph::build_typescript(repo, parses, resolve_relative_source),
        };
        recv_bound.push((lang, recv_stats::take()));
        match graph {
            Ok(g) => graphs.push(g),
            Err(e) => parse_errors.push(format!("{lang} graph: {e}")),
        }
    }
    if !ts_family.is_empty() {
        let graph = repo_graph_graph::build_typescript(repo, ts_family, resolve_ts_source);
        recv_bound.push(("typescript", recv_stats::take()));
        match graph {
            Ok(g) => graphs.push(g),
            Err(e) => parse_errors.push(format!("typescript graph: {e}")),
        }
    }
    // A6.2a fired_on marker, once per repo:
    //   `[recv] receiver-typed calls bound: csharp=N … (fields: csharp=F …) repo=<label>`
    recv_stats::flush_marker(&recv_bound, &recv_fields, repo_label);

    (graphs, di_refs)
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
