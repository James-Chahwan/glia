//! Repo walk + gating: which directories collapse to a single region anchor,
//! which files are queued for parsing, and the one-node-per-region graph.

use std::path::Path;

use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, node_kind};
use repo_graph_core::{Confidence, Node, NodeId, RepoId};

use crate::extract::detect_language;

/// A build-output / vendored / gitignored directory collapsed to a single
/// anchor node instead of being parsed file-by-file. Preserves the repo's
/// spatial map without the per-file flood. (glia-v2 G1/G2/G10)
pub(crate) struct RegionAnchor {
    /// Repo-relative path of the collapsed directory (`www`, `packages/x/dist`).
    rel_path: String,
    /// `vendored` | `build_output` — recorded in the anchor's ORIGIN cell.
    provenance: &'static str,
    /// The directory's own name (`www`, `node_modules`).
    region: String,
}

/// Walk the repo, classifying each directory as source-to-parse or a collapsed
/// region. Returns `(files_to_parse, region_anchors)`.
pub(crate) type WalkResult = (
    Vec<(String, String)>, // source files to parse
    Vec<RegionAnchor>,     // collapsed build/vendor regions
    Vec<(String, String)>, // markdown docs (rel_path, text) — G18
);

pub(crate) fn walk_source_files(root: &Path) -> WalkResult {
    let mut files = Vec::new();
    let mut regions = Vec::new();
    let mut md = Vec::new();
    let gitignore_dirs = load_gitignore_dirs(root);
    walk_dir(root, root, &gitignore_dirs, &mut files, &mut regions, &mut md);
    (files, regions, md)
}

/// VCS internals and editor metadata: no graph-meaningful content, skipped
/// outright (not even recorded as a region).
fn is_hard_skip(name: &str) -> bool {
    matches!(name, ".git" | ".hg" | ".svn" | ".idea" | ".vscode")
}

/// Provenance for directories always collapsed to a region anchor regardless of
/// `.gitignore` — dependency trees and conventional build output. `None` for an
/// ordinary source directory.
fn always_region(name: &str) -> Option<&'static str> {
    match name {
        "node_modules" | "vendor" | "bower_components" | ".venv" | "site-packages" => {
            Some("vendored")
        }
        "target" | "dist" | "build" | "out" | "__pycache__" | ".cache" | ".next" | ".nuxt"
        | ".angular" | "coverage" => Some("build_output"),
        _ => None,
    }
}

/// Directory names the repo's top-level `.gitignore` marks ignored. Build
/// mirrors a project gitignores (Capacitor's `www/`, `android/`) collapse to a
/// region anchor rather than being parsed as source. Only plain, non-glob,
/// non-negated entries are honored, matched by final path component against
/// directory names during the walk. (glia-v2 G10)
fn load_gitignore_dirs(root: &Path) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    let Ok(text) = std::fs::read_to_string(root.join(".gitignore")) else {
        return out;
    };
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty()
            || line.starts_with('#')
            || line.starts_with('!')
            || line.contains('*')
            || line.contains('?')
            || line.contains('[')
        {
            continue;
        }
        let trimmed = line.trim_matches('/');
        let comp = trimmed.rsplit('/').next().unwrap_or(trimmed);
        if !comp.is_empty() {
            out.insert(comp.to_string());
        }
    }
    out
}

/// True for a filename that looks like a bundler-emitted, content-hashed chunk
/// (`main.e188fddd19255ba1.js`, `1624.4e9cc6119b4878fe.js`, `styles.<hash>.css`)
/// — i.e. build output, not authored source. The signal is a dot-delimited
/// segment of ≥8 hex digits before a JS/CSS extension.
fn is_hashed_chunk(name: &str) -> bool {
    let ext_ok = name.ends_with(".js")
        || name.ends_with(".mjs")
        || name.ends_with(".css")
        || name.ends_with(".map");
    if !ext_ok {
        return false;
    }
    name.split('.').any(|seg| {
        seg.len() >= 8 && seg.bytes().all(|b| b.is_ascii_hexdigit())
    })
}

/// True when a directory is a built web-bundle mirror — it directly contains
/// several content-hashed chunk files. Catches Capacitor's copied bundle
/// (`android/app/src/main/assets/public`, `ios/App/App/public`) and any other
/// build mirror that `.gitignore` doesn't flag, regardless of path. (glia-v2 G10)
fn dir_is_build_bundle(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    let mut hashed = 0usize;
    for entry in entries.flatten() {
        if entry.file_type().map(|t| t.is_file()).unwrap_or(false)
            && is_hashed_chunk(&entry.file_name().to_string_lossy())
        {
            hashed += 1;
            if hashed >= 3 {
                return true;
            }
        }
    }
    false
}

fn walk_dir(
    root: &Path,
    dir: &Path,
    gitignore_dirs: &std::collections::HashSet<String>,
    files: &mut Vec<(String, String)>,
    regions: &mut Vec<RegionAnchor>,
    md: &mut Vec<(String, String)>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    // Sort by name: read_dir yields filesystem/inode order, which leaked into
    // node/edge Vec order (and so shard bytes) — stable-ish on one machine,
    // not reproducible across machines or after file churn (audit 2026-06-10).
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_unstable_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if is_hard_skip(&name) {
            continue;
        }
        if path.is_dir() {
            // Collapse vendored/build/gitignored directories to one anchor and
            // do NOT descend — categorise the region instead of dropping it or
            // emitting a node per file inside. (glia-v2 G1/G2/G10)
            let provenance = always_region(&name)
                .or_else(|| gitignore_dirs.contains(&name).then_some("build_output"))
                .or_else(|| dir_is_build_bundle(&path).then_some("build_output"));
            if let Some(provenance) = provenance {
                let rel = path.strip_prefix(root).unwrap_or(&path);
                regions.push(RegionAnchor {
                    rel_path: rel.to_string_lossy().to_string(),
                    provenance,
                    region: name,
                });
                continue;
            }
            walk_dir(root, &path, gitignore_dirs, files, regions, md);
        } else if path.is_file() {
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
            r.provenance, r.region
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
        assert!(is_hashed_chunk("main.e188fddd19255ba1.js"));
        assert!(is_hashed_chunk("1624.4e9cc6119b4878fe.js"));
        assert!(is_hashed_chunk("styles.0a1b2c3d4e5f6a7b.css"));
        // Authored source is not a hashed chunk.
        assert!(!is_hashed_chunk("app.component.ts"));
        assert!(!is_hashed_chunk("index.js"));
        assert!(!is_hashed_chunk("user_service.py"));
        // 8-hex-ish word but wrong extension.
        assert!(!is_hashed_chunk("deadbeef.txt"));
    }

    #[test]
    fn gitignore_dir_parsing() {
        let root = std::env::temp_dir().join(format!("glia_gi_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join(".gitignore"),
            "# comment\n/www\nandroid/\n*.log\n!keep\n/dist\nsrc/generated\n",
        )
        .unwrap();
        let dirs = load_gitignore_dirs(&root);
        assert!(dirs.contains("www"));
        assert!(dirs.contains("android"));
        assert!(dirs.contains("dist"));
        assert!(dirs.contains("generated")); // final component of src/generated
        assert!(!dirs.contains("keep")); // negation skipped
        assert!(dirs.iter().all(|d| !d.contains('*'))); // globs skipped
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

        let (files, regions, _md) = walk_source_files(&root);

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
        assert_eq!(nm.provenance, "vendored");
        let pub_region = regions
            .iter()
            .find(|r| r.rel_path.ends_with("assets/public"))
            .unwrap();
        assert_eq!(pub_region.provenance, "build_output");

        let _ = std::fs::remove_dir_all(&root);
    }
}
