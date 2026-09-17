//! Repo walk + gating: which directories collapse to a single region anchor,
//! which files are queued for parsing, and the one-node-per-region graph.

use std::path::Path;

use repo_graph_code_domain::project_roots::{self, ProjectRoot};
use repo_graph_code_domain::walk_gating::{self, Collapse, Gate, GateCounts, IgnoreStack};
use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, node_kind};
use repo_graph_core::{Confidence, Node, NodeId, RepoId};

use crate::extract::detect_language;

/// A build-output / vendored / gitignored directory collapsed to a single
/// anchor node instead of being parsed file-by-file. Preserves the repo's
/// spatial map without the per-file flood. (glia-v2 G1/G2/G10)
pub(crate) struct RegionAnchor {
    /// Repo-relative path of the collapsed directory (`www`, `packages/x/dist`).
    rel_path: String,
    /// Why it collapsed — rendered into the anchor's ORIGIN cell `provenance`.
    provenance: Collapse,
    /// The directory's own name (`www`, `node_modules`).
    region: String,
}

/// Walk the repo, classifying each directory as source-to-parse or a collapsed
/// region. Returns `(files_to_parse, region_anchors, markdown, project_roots)`.
pub(crate) type WalkResult = (
    Vec<(String, String)>, // source files to parse
    Vec<RegionAnchor>,     // collapsed build/vendor regions
    Vec<(String, String)>, // markdown docs (rel_path, text) — G18
    Vec<ProjectRoot>,      // manifest-rooted sub-projects, sorted by rel_path — A8.4
);

/// The engine's own output directory, `<root>/.ai/repo-graph` — where the store
/// writes the gmap and `cache.rs` keeps `parse_cache.bin` (the literal mirrors
/// `cache::gmap_dir` and `repo_graph_store::default_gmap_dir`). Skipped outright,
/// like `store::scan_for_newer` skips it by prefix: otherwise a repo that
/// gitignores it grows a `region:.ai/repo-graph` only after its FIRST persisted
/// build, so the graph depends on build history. (A8.2 → A8.4 hand-off)
/// Only the root's copy is ours; `.ai` itself is authored and still walked.
fn is_self_output(root: &Path, parent: &Path, name: &str) -> bool {
    name == "repo-graph"
        && parent.file_name().is_some_and(|n| n == ".ai")
        && parent.parent() == Some(root)
}

pub(crate) fn walk_source_files(root: &Path) -> WalkResult {
    let mut files = Vec::new();
    let mut regions = Vec::new();
    let mut md = Vec::new();
    let mut roots = Vec::new();
    let mut counts = GateCounts::default();
    // Per-directory `.gitignore` layers: pushed on the way down, popped on the
    // way back up, so each verdict sees exactly the files git would. (A8.2)
    let mut ignores = IgnoreStack::default();
    let pushed = ignores.push_dir(root);
    walk_dir(
        root,
        root,
        &mut ignores,
        &mut files,
        &mut regions,
        &mut md,
        &mut roots,
        &mut counts,
    );
    if pushed {
        ignores.pop();
    }
    // Explicit, although the name-sorted walk already discovers roots in a
    // stable order: A8.5's node order (and so shard bytes) keys on this Vec,
    // and implicit order always leaks (audit 2026-06-10).
    roots.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    if !roots.is_empty() {
        eprintln!("[roots] {}", project_roots::marker(&roots));
    }
    // Attributable collapse tally. Gated on non-zero so a region-free repo stays
    // quiet on the hot path, matching the `[incremental]` / `[gmap]` precedent.
    if counts.total() > 0 {
        eprintln!("[walk] {}", counts.marker());
    }
    if ignores.files > 0 {
        eprintln!("[walk] {}", ignores.marker());
    }
    (files, regions, md, roots)
}

#[allow(clippy::too_many_arguments)]
fn walk_dir(
    root: &Path,
    dir: &Path,
    ignores: &mut IgnoreStack,
    files: &mut Vec<(String, String)>,
    regions: &mut Vec<RegionAnchor>,
    md: &mut Vec<(String, String)>,
    roots: &mut Vec<ProjectRoot>,
    counts: &mut GateCounts,
) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    // Sort by name: read_dir yields filesystem/inode order, which leaked into
    // node/edge Vec order (and so shard bytes) — stable-ish on one machine,
    // not reproducible across machines or after file churn (audit 2026-06-10).
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_unstable_by_key(|e| e.file_name());
    // Project-root detection rides the walk (A8.4): it scans the entry names
    // already in hand, and a collapsed region (`node_modules/*/package.json`)
    // is never visited, so it can never become a root.
    let names: Vec<String> = entries
        .iter()
        .filter(|e| e.file_type().is_ok_and(|t| !t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    if let Some((ecosystem, manifest, label)) = project_roots::detect_root(&names, |base| {
        std::fs::read_to_string(dir.join(base)).unwrap_or_default()
    }) {
        let rel = dir.strip_prefix(root).unwrap_or(dir).to_string_lossy().to_string();
        roots.push(ProjectRoot::new(rel, ecosystem, &manifest, label));
    }
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        // Also guards FILES: a `.git`/`.hg` file must never be read as source.
        if walk_gating::is_hard_skip(&name) {
            continue;
        }
        if path.is_dir() {
            if is_self_output(root, dir, &name) {
                let rel = path.strip_prefix(root).unwrap_or(&path);
                eprintln!("[walk] skipped engine output {}", rel.display());
                continue;
            }
            // Collapse vendored / build / gitignored / other-repo directories to
            // one anchor and do NOT descend — categorise the region instead of
            // dropping it or emitting a node per file inside. The rules live in
            // `code_domain::walk_gating` so `store::is_gmap_stale` scans exactly
            // this tree. (glia-v2 G1/G2/G10, A8.1)
            let gate = walk_gating::gate_dir(&path, &name, ignores.is_ignored(&path, true));
            counts.record(gate);
            if let Some(provenance) = gate.collapse() {
                let rel = path.strip_prefix(root).unwrap_or(&path);
                regions.push(RegionAnchor {
                    rel_path: rel.to_string_lossy().to_string(),
                    provenance,
                    region: name,
                });
                continue;
            }
            if gate == Gate::HardSkip {
                continue;
            }
            let pushed = ignores.push_dir(&path);
            walk_dir(root, &path, ignores, files, regions, md, roots, counts);
            if pushed {
                ignores.pop();
            }
        } else if path.is_file() {
            // File-level gitignore: committed-but-ignored output (`*.min.js`,
            // `*_pb2.py`) beside authored source never reaches a parser. Before
            // the markdown branch, so an ignored doc is not ingested either.
            if ignores.skip_file(&path) {
                continue;
            }
            let rel = path.strip_prefix(root).unwrap_or(&path);
            let rel_str = rel.to_string_lossy().to_string();
            // Markdown docs (G18) — collected separately; the include/skip rules
            // are applied in `build_docs_graph`.
            if rel_str.to_ascii_lowercase().ends_with(".md")
                && std::fs::metadata(&path).map(|m| m.len() <= 500_000).unwrap_or(false)
                && let Ok(text) = std::fs::read_to_string(&path)
            {
                md.push((rel_str.clone(), text));
                continue;
            }
            let matches_lang = detect_language(&rel_str).is_some();
            let matches_bypass = is_bypass_path(&rel_str);
            if (matches_lang || matches_bypass)
                && let Ok(source) = std::fs::read_to_string(&path)
            {
                files.push((rel_str, source));
            }
        }
    }
}

/// Build a one-node-per-region graph from the collapsed [`RegionAnchor`]s. Each
/// node carries an `ORIGIN` cell `{provenance, region}` so consumers filter by
/// coordinate instead of string-matching keys. (glia-v2 G1/G10)
pub(crate) fn build_region_graph(regions: &[RegionAnchor], repo: RepoId) -> repo_graph_graph::RepoGraph {
    use repo_graph_code_domain::cell_type;
    use repo_graph_core::{Cell, CellPayload};

    let mut nodes = Vec::new();
    let mut nav = CodeNav::default();
    for r in regions {
        let qname = format!("region:{}", r.rel_path);
        let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::REGION, &qname);
        let origin = format!(
            r#"{{"provenance":"{}","region":"{}"}}"#,
            r.provenance.provenance(),
            r.region
        );
        nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Strong,
            cells: vec![Cell {
                kind: cell_type::ORIGIN,
                payload: CellPayload::Json(origin),
            }],
        });
        nav.record(id, &r.region, &qname, node_kind::REGION, None);
    }
    repo_graph_graph::RepoGraph {
        repo,
        nodes,
        edges: Vec::new(),
        nav,
        symbols: Default::default(),
        unresolved_calls: Vec::new(),
        unresolved_refs: Vec::new(),
        properties: Default::default(),
    }
}

/// The qname of a detected project root: `project:<rel_path>`, with `.` for the
/// repo root. An empty tail would be ambiguous. A walked relative path never
/// contains `::`, so the locked separator never appears inside the qname. (A8.5)
pub(crate) fn project_qname(rel_path: &str) -> String {
    format!("project:{}", if rel_path.is_empty() { "." } else { rel_path })
}

/// Build one edge-less `PROJECT` node per detected [`ProjectRoot`] (A8.5). It
/// follows the same pattern as [`build_region_graph`]. The nav NAME is the
/// manifest label (`@shop/web`), which is what `--scope <label>` and service
/// labelling look up. The qname is derived from the path, so it stays stable
/// when a label changes. The single `ORIGIN` cell is
/// `{provenance:"project_root", ecosystem, manifest, label, path}`. Because the
/// nodes have ZERO edges, blast radius, trace and liveness cannot fan out
/// through them.
pub(crate) fn build_project_graph(roots: &[ProjectRoot], repo: RepoId) -> repo_graph_graph::RepoGraph {
    use repo_graph_code_domain::cell_type;
    use repo_graph_core::{Cell, CellPayload};

    let mut nodes = Vec::with_capacity(roots.len());
    let mut nav = CodeNav::default();
    for r in roots {
        let qname = project_qname(&r.rel_path);
        let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::PROJECT, &qname);
        // `json!` rather than a format string: a label is manifest text, and a
        // quote in it must not produce malformed JSON.
        let origin = serde_json::json!({
            "provenance": "project_root",
            "ecosystem": r.ecosystem,
            "manifest": r.manifest,
            "label": r.label,
            "path": r.rel_path,
        })
        .to_string();
        nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Strong,
            cells: vec![Cell {
                kind: cell_type::ORIGIN,
                payload: CellPayload::Json(origin),
            }],
        });
        nav.record(id, &r.label, &qname, node_kind::PROJECT, None);
    }
    eprintln!("[roots] emitted {} PROJECT nodes", nodes.len());
    repo_graph_graph::RepoGraph {
        repo,
        nodes,
        edges: Vec::new(),
        nav,
        symbols: Default::default(),
        unresolved_calls: Vec::new(),
        unresolved_refs: Vec::new(),
        properties: Default::default(),
    }
}

fn is_bypass_path(path: &str) -> bool {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    if matches!(ext, "yml" | "yaml") {
        return true;
    }
    is_dockerfile_path(path)
        || is_dotenv_path(path)
        || repo_graph_code_extractors::packages::is_manifest_path(path)
}

pub(crate) fn is_dockerfile_path(path: &str) -> bool {
    let base = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    base == "dockerfile"
        || base.ends_with(".dockerfile")
        || base.starts_with("dockerfile.")
}

pub(crate) fn is_dotenv_path(path: &str) -> bool {
    let base = path.rsplit('/').next().unwrap_or(path);
    base == ".env" || base.starts_with(".env.")
}

#[cfg(test)]
mod walk_tests {
    use super::*;

    #[test]
    fn hashed_chunk_detection() {
        assert!(walk_gating::is_hashed_chunk("main.e188fddd19255ba1.js"));
        assert!(walk_gating::is_hashed_chunk("1624.4e9cc6119b4878fe.js"));
        assert!(walk_gating::is_hashed_chunk("styles.0a1b2c3d4e5f6a7b.css"));
        // Authored source is not a hashed chunk.
        assert!(!walk_gating::is_hashed_chunk("app.component.ts"));
        assert!(!walk_gating::is_hashed_chunk("index.js"));
        assert!(!walk_gating::is_hashed_chunk("user_service.py"));
        // 8-hex-ish word but wrong extension.
        assert!(!walk_gating::is_hashed_chunk("deadbeef.txt"));
    }

    /// Git's `.gitignore` semantics end to end through the builder walk (A8.2).
    /// Replaces `gitignore_dir_parsing`, which pinned the old final-component
    /// match that collapsed every `generated` directory anywhere in the tree.
    #[test]
    fn gitignore_matcher_semantics() {
        let root = std::env::temp_dir().join(format!("glia_gi_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["gen", "src/gen", "dist-x", "dist-keep", "src/generated", "notes/generated"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::write(root.join(".gitignore"), "/gen\ndist-*\n!dist-keep\n*.min.js\n").unwrap();
        std::fs::write(root.join("src/.gitignore"), "generated/\n").unwrap();
        for f in [
            "app.js",
            "vendor.min.js",
            "gen/g.py",
            "src/gen/builder.py",
            "dist-x/d.py",
            "dist-keep/k.py",
            "src/generated/out.py",
            "notes/generated/n.py",
        ] {
            std::fs::write(root.join(f), "x = 1\n").unwrap();
        }

        let (files, regions, _md, _roots) = walk_source_files(&root);
        let parsed: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
        let region_paths: Vec<&str> = regions.iter().map(|r| r.rel_path.as_str()).collect();

        // Anchoring: `/gen` collapses the root `gen` only.
        assert!(region_paths.contains(&"gen"), "{region_paths:?}");
        assert!(parsed.contains(&"src/gen/builder.py"), "{parsed:?}");
        // Globs: `dist-*` collapses `dist-x`.
        assert!(region_paths.contains(&"dist-x"), "{region_paths:?}");
        assert!(!parsed.contains(&"dist-x/d.py"), "{parsed:?}");
        // Negation: `!dist-keep` stays source.
        assert!(parsed.contains(&"dist-keep/k.py"), "{parsed:?}");
        // Nested file: `src/.gitignore` collapses `src/generated`...
        assert!(region_paths.contains(&"src/generated"), "{region_paths:?}");
        // ...and does not leak upward to a sibling tree.
        assert!(parsed.contains(&"notes/generated/n.py"), "{parsed:?}");
        assert!(!region_paths.contains(&"notes/generated"), "{region_paths:?}");
        // File-level: `*.min.js` drops the bundle beside the authored file.
        assert!(parsed.contains(&"app.js"), "{parsed:?}");
        assert!(!parsed.contains(&"vendor.min.js"), "{parsed:?}");
        // Exactly the four sources above, and gitignore-attributed regions only.
        assert_eq!(parsed.len(), 4, "{parsed:?}");
        assert_eq!(regions.len(), 3, "{region_paths:?}");
        assert!(regions.iter().all(|r| r.provenance == Collapse::BuildOutput));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn walk_collapses_build_and_vendor_regions() {
        let root = std::env::temp_dir().join(format!("glia_walk_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src/app")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/left-pad")).unwrap();
        std::fs::create_dir_all(root.join("www")).unwrap();
        std::fs::create_dir_all(root.join("android/app/src/main/assets/public")).unwrap();
        std::fs::write(root.join(".gitignore"), "/www\n").unwrap();
        // authored source
        std::fs::write(root.join("src/app/a.ts"), "export class A {}").unwrap();
        // vendored dep (must NOT be parsed)
        std::fs::write(root.join("node_modules/left-pad/index.js"), "module.exports=1").unwrap();
        // gitignored build mirror
        std::fs::write(root.join("www/main.abc12345def0.js"), "var a=1").unwrap();
        // capacitor bundle mirror NOT in .gitignore — caught by hash-chunk probe
        let pub_dir = root.join("android/app/src/main/assets/public");
        for h in ["1624.4e9cc6119b4878fe", "1102.7837812dd7ed4d51", "2075.f756d5b13b56050a"] {
            std::fs::write(pub_dir.join(format!("{h}.js")), "var b=2").unwrap();
        }

        let (files, regions, _md, _roots) = walk_source_files(&root);

        // Only the authored source file is queued for parsing.
        assert_eq!(files.len(), 1, "files: {files:?}");
        assert!(files[0].0.ends_with("a.ts"));

        let region_paths: Vec<&str> = regions.iter().map(|r| r.rel_path.as_str()).collect();
        assert!(region_paths.contains(&"node_modules"), "{region_paths:?}");
        assert!(region_paths.contains(&"www"), "{region_paths:?}");
        assert!(
            region_paths.contains(&"android/app/src/main/assets/public"),
            "hash-chunk bundle mirror should collapse: {region_paths:?}"
        );
        // node_modules is vendored; the bundle mirrors are build_output.
        let nm = regions.iter().find(|r| r.rel_path == "node_modules").unwrap();
        assert_eq!(nm.provenance, Collapse::Vendored);
        let pub_region = regions
            .iter()
            .find(|r| r.rel_path.ends_with("assets/public"))
            .unwrap();
        assert_eq!(pub_region.provenance, Collapse::BuildOutput);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Fresh temp root per test name; `walk_collapses_*` already owns the
    /// pid-suffixed `glia_walk_` prefix, so these take their own.
    fn walk_tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("glia_walk_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn dotnet_bin_obj_collapse_next_to_a_project() {
        let root = walk_tmp("dotnet");
        std::fs::create_dir_all(root.join("obj/Debug/net8.0")).unwrap();
        std::fs::create_dir_all(root.join("bin/Debug")).unwrap();
        std::fs::write(root.join("Api.csproj"), "<Project/>").unwrap();
        std::fs::write(root.join("Program.cs"), "public class Startup {}").unwrap();
        std::fs::write(root.join("obj/project.assets.json"), "{}").unwrap();
        std::fs::write(
            root.join("obj/Debug/net8.0/Api.AssemblyInfo.cs"),
            "internal sealed class PhantomAssemblyInfo {}",
        )
        .unwrap();
        std::fs::write(root.join("bin/Debug/Api.g.cs"), "internal sealed class PhantomBinClass {}")
            .unwrap();

        let (files, regions, _md, _roots) = walk_source_files(&root);

        // PRECISION: the generated C# never reaches a parser. grade.py has no
        // expect_absent_nodes, so this half of the fix is provable only here.
        assert!(
            files.iter().all(|(p, _)| !p.starts_with("obj/") && !p.starts_with("bin/")),
            "generated .NET output must not be parsed: {files:?}"
        );
        assert!(files.iter().any(|(p, _)| p == "Program.cs"), "{files:?}");
        for dir in ["obj", "bin"] {
            let r = regions
                .iter()
                .find(|r| r.rel_path == dir)
                .unwrap_or_else(|| panic!("no region for {dir}: {:?}", regions.len()));
            assert_eq!(r.provenance, Collapse::BuildOutput);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn dotnet_gate_requires_a_project_sibling() {
        let root = walk_tmp("nodotnet");
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(root.join("bin/console_entry.py"), "x = 1\n").unwrap();

        let (files, regions, _md, _roots) = walk_source_files(&root);

        // A Python repo's `bin/` of console scripts is authored source.
        assert!(files.iter().any(|(p, _)| p == "bin/console_entry.py"), "{files:?}");
        assert!(regions.is_empty(), "no project file => no collapse: {}", regions.len());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn nested_git_becomes_a_region() {
        let root = walk_tmp("nested");
        std::fs::create_dir_all(root.join("app")).unwrap();
        std::fs::create_dir_all(root.join("libs/sdk")).unwrap();
        std::fs::write(root.join("app/main.py"), "def run():\n    pass\n").unwrap();
        std::fs::write(root.join("libs/sdk/.git"), "gitdir: ../../.git/modules/sdk\n").unwrap();
        std::fs::write(root.join("libs/sdk/vendor_client.py"), "class VendorClient:\n    pass\n")
            .unwrap();

        let (files, regions, _md, _roots) = walk_source_files(&root);

        // The submodule's symbols must not be minted into the PARENT RepoId —
        // that is the duplicate-name pair `resolve_name` had to pick between.
        assert!(
            files.iter().all(|(p, _)| !p.starts_with("libs/sdk/")),
            "submodule source must not be parsed into the parent: {files:?}"
        );
        assert!(files.iter().any(|(p, _)| p == "app/main.py"), "{files:?}");
        let r = regions.iter().find(|r| r.rel_path == "libs/sdk").unwrap();
        assert_eq!(r.provenance, Collapse::Submodule);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A8.4 acceptance: detection rides the walk, so the vendored manifest under
    /// the collapsed `node_modules` is never a root, and roots come back sorted.
    #[test]
    fn project_roots_ride_the_walk() {
        let root = walk_tmp("roots");
        for d in ["apps/web", "services/api", "node_modules/left-pad", "libs/plain"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::write(root.join("package.json"), r#"{"name":"@shop/monorepo"}"#).unwrap();
        std::fs::write(root.join("apps/web/package.json"), r#"{"private":true}"#).unwrap();
        std::fs::write(root.join("apps/web/index.ts"), "export const x = 1;\n").unwrap();
        std::fs::write(root.join("services/api/go.mod"), "module github.com/shop/api\n\ngo 1.22\n").unwrap();
        std::fs::write(root.join("services/api/main.go"), "package main\n").unwrap();
        std::fs::write(root.join("node_modules/left-pad/package.json"), r#"{"name":"left-pad"}"#).unwrap();
        std::fs::write(root.join("libs/plain/util.py"), "x = 1\n").unwrap();

        let (_files, regions, _md, roots) = walk_source_files(&root);

        let rels: Vec<&str> = roots.iter().map(|r| r.rel_path.as_str()).collect();
        assert_eq!(rels, ["", "apps/web", "services/api"]);
        let got: Vec<(&str, &str, &str)> =
            roots.iter().map(|r| (r.ecosystem, r.label.as_str(), r.manifest.as_str())).collect();
        assert_eq!(
            got,
            [
                ("npm", "@shop/monorepo", "package.json"),
                ("npm", "web", "apps/web/package.json"),
                ("go", "github.com/shop/api", "services/api/go.mod"),
            ]
        );
        assert!(regions.iter().any(|r| r.rel_path == "node_modules"));
        assert_eq!(project_roots::marker(&roots), "3 project roots (go=1 npm=2)");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A8.5: each detected root is ONE PROJECT node, labelled by its manifest
    /// and keyed by its path. No edge touches it, so blast radius never
    /// returns it, and a function that shares its label still wins name
    /// resolution. `generate_one` and `generate_many` put the project graph at
    /// the same shard index.
    #[test]
    fn project_roots_become_edgeless_project_nodes() {
        use std::collections::HashSet;

        use crate::answers::blast_radius_by_qname;
        use crate::build::{generate_many, generate_one};

        let root = walk_tmp("projects");
        for d in ["apps/web", "services/api", "libs/core/src", "node_modules/left-pad"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        let files = [
            ("package.json", r#"{"name":"shop-monorepo","private":true}"#),
            ("README.md", "# Shop\n\nThe `shop-core` crate and the `apisvc` module.\n"),
            ("apps/web/package.json", r#"{"name":"@shop/web"}"#),
            ("apps/web/index.ts", "export function webEntry(): number { return 1; }\n"),
            // The Go module label `apisvc` is also the name of a function.
            ("services/api/go.mod", "module apisvc\n\ngo 1.22\n"),
            ("services/api/main.go", "package main\n\nfunc main() { apisvc() }\n\nfunc apisvc() {}\n"),
            ("libs/core/Cargo.toml", "[package]\nname = \"shop-core\"\n"),
            ("libs/core/src/lib.rs", "pub fn core_fn() {}\n"),
            ("node_modules/left-pad/package.json", r#"{"name":"left-pad"}"#),
        ];
        for (rel, body) in files {
            std::fs::write(root.join(rel), body).unwrap();
        }
        let path = root.to_str().unwrap().to_string();
        let r = generate_one(&path).unwrap();
        let m = &r.merged;

        let mut projects: Vec<(String, String, NodeId, String)> = m
            .graphs
            .iter()
            .flat_map(|g| {
                g.nodes
                    .iter()
                    .filter(move |n| g.nav.kind_by_id.get(&n.id) == Some(&node_kind::PROJECT))
                    .map(move |n| {
                        let origin = match &n.cells[..] {
                            [c] if c.kind == repo_graph_code_domain::cell_type::ORIGIN => {
                                match &c.payload {
                                    repo_graph_core::CellPayload::Json(j) => j.clone(),
                                    other => panic!("ORIGIN must be JSON, got {other:?}"),
                                }
                            }
                            other => panic!("exactly one ORIGIN cell, got {other:?}"),
                        };
                        (g.nav.qname_by_id[&n.id].clone(), g.nav.name_by_id[&n.id].clone(), n.id, origin)
                    })
            })
            .collect();
        projects.sort_by(|a, b| a.0.cmp(&b.0));
        let got: Vec<(&str, &str)> =
            projects.iter().map(|(q, n, ..)| (q.as_str(), n.as_str())).collect();
        assert_eq!(
            got,
            [
                ("project:.", "shop-monorepo"),
                ("project:apps/web", "@shop/web"),
                ("project:libs/core", "shop-core"),
                ("project:services/api", "apisvc"),
            ],
            "one PROJECT per root; the vendored node_modules manifest is not one"
        );
        assert!(projects.iter().all(|(q, ..)| !q.contains("::")), "{got:?}");
        assert_eq!(project_qname(""), "project:.");

        let origin: serde_json::Value = serde_json::from_str(&projects[0].3).unwrap();
        assert_eq!(
            origin,
            serde_json::json!({
                "provenance": "project_root",
                "ecosystem": "npm",
                "manifest": "package.json",
                "label": "shop-monorepo",
                "path": "",
            })
        );

        // The literal substrings that walk-project-roots/key.json's
        // expect_cells grade.
        let origin_of = |q: &str| projects.iter().find(|p| p.0 == q).map(|p| p.3.as_str()).unwrap();
        assert!(origin_of("project:.").contains(r#""manifest":"package.json""#));
        assert!(origin_of("project:libs/core").contains(r#""provenance":"project_root""#));
        assert!(origin_of("project:services/api").contains(r#""ecosystem":"go""#));

        // No edge touches a PROJECT node, including the doc-linker's DOCUMENTS
        // edges from a README that names two of the labels.
        let ids: HashSet<NodeId> = projects.iter().map(|p| p.2).collect();
        let touching: Vec<_> = m
            .all_edges()
            .filter(|e| ids.contains(&e.from) || ids.contains(&e.to))
            .collect();
        assert!(touching.is_empty(), "{touching:?}");

        // Blast radius from a nearby function never includes a PROJECT node.
        // The function `apisvc` shares a label with a project but has a CALLS
        // edge, and PROJECT has degree 0, so name resolution picks the
        // function.
        let func = m.resolve_name("apisvc").unwrap();
        assert!(!ids.contains(&func), "the degree-0 PROJECT must lose pick_primary");
        for seed in ["apisvc", "main", "core_fn", "webEntry"] {
            let hits = blast_radius_by_qname(m, seed, "both", 4, None, false, None).unwrap();
            assert!(hits.iter().all(|h| h.kind != "PROJECT"), "{seed}: {:?}",
                hits.iter().map(|h| &h.qname).collect::<Vec<_>>());
        }
        let apisvc = blast_radius_by_qname(m, "apisvc", "both", 4, None, false, None).unwrap();
        assert!(!apisvc.is_empty(), "the control seed has a real neighbour (main)");

        // Same shard slot from both entry points (regions, projects, docs).
        let slot = |m: &repo_graph_graph::MergedGraph| {
            m.graphs.iter().position(|g| {
                g.nodes.first().is_some_and(|n| g.nav.kind_by_id.get(&n.id) == Some(&node_kind::PROJECT))
            })
        };
        let many = generate_many(std::slice::from_ref(&path)).unwrap();
        assert_eq!(slot(m), slot(&many.merged));
        assert!(slot(m).is_some());
        assert_eq!(m.graphs.len(), many.merged.graphs.len());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The A8.2 hand-off: the engine's own `<root>/.ai/repo-graph` is neither
    /// walked nor a region, gitignored or not, while the rest of `.ai` is.
    #[test]
    fn engine_output_dir_is_skipped() {
        let root = walk_tmp("selfout");
        std::fs::create_dir_all(root.join(".ai/repo-graph")).unwrap();
        std::fs::create_dir_all(root.join("pkg/.ai/repo-graph")).unwrap();
        std::fs::write(root.join(".gitignore"), "**/.ai/repo-graph/\n").unwrap();
        std::fs::write(root.join(".ai/repo-graph/leak.py"), "x = 1\n").unwrap();
        std::fs::write(root.join(".ai/notes.md"), "# Notes\n").unwrap();
        std::fs::write(root.join("pkg/.ai/repo-graph/other.py"), "y = 2\n").unwrap();

        let (files, regions, md, _roots) = walk_source_files(&root);

        let region_paths: Vec<&str> = regions.iter().map(|r| r.rel_path.as_str()).collect();
        assert!(!region_paths.contains(&".ai/repo-graph"), "{region_paths:?}");
        assert!(files.iter().all(|(p, _)| !p.starts_with(".ai/")), "{files:?}");
        assert!(md.iter().any(|(p, _)| p == ".ai/notes.md"), "authored .ai is still walked");
        // Only the ROOT copy is ours; a nested one stays under the usual gates.
        assert_eq!(region_paths, ["pkg/.ai/repo-graph"]);

        // Without a gitignore it is still never parsed.
        std::fs::remove_file(root.join(".gitignore")).unwrap();
        let (files, regions, _md, _roots) = walk_source_files(&root);
        assert!(files.iter().all(|(p, _)| !p.starts_with(".ai/")), "{files:?}");
        assert!(regions.is_empty(), "{}", regions.len());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn worktree_and_nested_clone_get_their_own_provenance() {
        let root = walk_tmp("worktree");
        std::fs::create_dir_all(root.join("wt")).unwrap();
        std::fs::create_dir_all(root.join("clone/.git")).unwrap();
        std::fs::write(root.join("wt/.git"), "gitdir: /main/.git/worktrees/wt\n").unwrap();
        std::fs::write(root.join("wt/a.py"), "x = 1\n").unwrap();
        std::fs::write(root.join("clone/b.py"), "y = 2\n").unwrap();

        let (files, regions, _md, _roots) = walk_source_files(&root);

        assert!(files.is_empty(), "no source outside the two other repos: {files:?}");
        let wt = regions.iter().find(|r| r.rel_path == "wt").unwrap();
        assert_eq!(wt.provenance, Collapse::Worktree);
        assert_eq!(wt.provenance.provenance(), "worktree");
        let clone = regions.iter().find(|r| r.rel_path == "clone").unwrap();
        assert_eq!(clone.provenance, Collapse::NestedRepo);
        let _ = std::fs::remove_dir_all(&root);
    }
}
