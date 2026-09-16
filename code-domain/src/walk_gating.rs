//! Shared directory gating for the repo walk — the ONE place that decides
//! whether a directory is descended into or collapsed to a single REGION anchor.
//!
//! Two consumers must agree or the graph lies about itself:
//!   * `engine::walk` — the builder's walk. It parses what it descends into and
//!     emits one REGION node (with an ORIGIN `provenance`) per collapse.
//!   * `repo_graph_store::is_gmap_stale` — the freshness check. It must scan
//!     exactly the tree the builder would read, or it regenerates the whole
//!     gmap for files no parser ever opens.
//!
//! Until this module landed the store carried a hand-copied duplicate of the
//! rules (`mod walk_gate`, packet A1.5's documented stopgap). `store` cannot
//! reach into `engine` — `engine` dev-depends on `store`, so that edge inverts
//! the layering — so the shared gate lives here, in the domain crate both
//! already depend on.
//!
//! [`gate_dir`] never reads `.gitignore` itself: the caller supplies the
//! verdict, so a real gitignore matcher (A8.2) swaps in without disturbing the
//! precedence rules.

use std::collections::HashSet;
use std::path::Path;

/// Why a directory was collapsed. The string form is the REGION node's ORIGIN
/// cell `provenance`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Collapse {
    /// A dependency tree (`node_modules`, `vendor`, `.venv`).
    Vendored,
    /// Compiler/bundler output (`target`, `dist`, `obj`, a hashed-chunk mirror).
    BuildOutput,
    /// A git submodule: a `.git` FILE pointing at `<super>/.git/modules/<name>`.
    Submodule,
    /// A linked worktree: a `.git` FILE pointing at `<main>/.git/worktrees/<n>`.
    Worktree,
    /// Some other repository's tree: a `.git` DIRECTORY.
    NestedRepo,
}

impl Collapse {
    /// The ORIGIN `provenance` string. Stable — consumers (engram, neuropil)
    /// filter on it.
    pub fn provenance(self) -> &'static str {
        match self {
            Collapse::Vendored => "vendored",
            Collapse::BuildOutput => "build_output",
            Collapse::Submodule => "submodule",
            Collapse::Worktree => "worktree",
            Collapse::NestedRepo => "nested_repo",
        }
    }
}

/// Which rule fired for one child directory. Drives the `[walk]` marker's
/// per-rule counters, so a surprising collapse is attributable without a
/// rebuild.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Gate {
    /// Ordinary source directory: walk into it.
    Descend,
    /// VCS internals / editor metadata: skip outright, not even a region.
    HardSkip,
    /// Name is always a dependency tree or build output.
    Always(Collapse),
    /// The top-level `.gitignore` names it.
    Ignored(Collapse),
    /// It directly holds several content-hashed bundler chunks.
    Bundle(Collapse),
    /// `bin`/`obj` next to a .NET project file.
    Dotnet(Collapse),
    /// It carries its own `.git`, so it is another repo's tree.
    Nested(Collapse),
}

impl Gate {
    /// The collapse to record, or `None` for [`Gate::Descend`]/[`Gate::HardSkip`]
    /// (neither produces a REGION node).
    pub fn collapse(self) -> Option<Collapse> {
        match self {
            Gate::Descend | Gate::HardSkip => None,
            Gate::Always(c)
            | Gate::Ignored(c)
            | Gate::Bundle(c)
            | Gate::Dotnet(c)
            | Gate::Nested(c) => Some(c),
        }
    }
}

/// VCS internals and editor metadata: no graph-meaningful content, skipped
/// outright (not even recorded as a region). Also guards FILES — a `.git` file
/// must never be read as source.
pub fn is_hard_skip(name: &str) -> bool {
    matches!(name, ".git" | ".hg" | ".svn" | ".idea" | ".vscode" | ".vs")
}

/// Directories always collapsed regardless of `.gitignore` — dependency trees
/// and conventional build output. `None` for an ordinary source directory.
pub fn always_region(name: &str) -> Option<Collapse> {
    match name {
        "node_modules" | "vendor" | "bower_components" | ".venv" | "site-packages" => {
            Some(Collapse::Vendored)
        }
        "target" | "dist" | "build" | "out" | "__pycache__" | ".cache" | ".next" | ".nuxt"
        | ".angular" | "coverage" | "TestResults" => Some(Collapse::BuildOutput),
        _ => None,
    }
}

/// Directory names the repo's top-level `.gitignore` marks ignored. Only plain,
/// non-glob, non-negated entries are honoured, matched by final path component
/// against directory names during the walk. (glia-v2 G10)
pub fn load_gitignore_dirs(root: &Path) -> HashSet<String> {
    let mut out = HashSet::new();
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
/// (`main.e188fddd19255ba1.js`, `styles.<hash>.css`) — build output, not
/// authored source. The signal is a dot-delimited segment of >= 8 hex digits
/// before a JS/CSS extension.
pub fn is_hashed_chunk(name: &str) -> bool {
    let ext_ok = name.ends_with(".js")
        || name.ends_with(".mjs")
        || name.ends_with(".css")
        || name.ends_with(".map");
    if !ext_ok {
        return false;
    }
    name.split('.')
        .any(|seg| seg.len() >= 8 && seg.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// True when a directory is a built web-bundle mirror — it directly contains
/// several content-hashed chunk files. Catches Capacitor's copied bundle and any
/// other build mirror `.gitignore` doesn't flag, regardless of path. (glia-v2 G10)
pub fn dir_is_build_bundle(dir: &Path) -> bool {
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

/// `.NET` build output: `bin`/`obj` that belong to a project, not a Python
/// repo's `bin/` of console scripts or a Go project's `bin/`. The gate is the
/// evidence, so it must be found: either MSBuild's own `obj/project.assets.json`
/// or a project file sitting next to the directory.
pub fn dotnet_build_dir(dir: &Path, name: &str) -> Option<Collapse> {
    if !matches!(name, "bin" | "obj") {
        return None;
    }
    if dir.join("project.assets.json").is_file() {
        return Some(Collapse::BuildOutput);
    }
    // One read_dir on the parent, returning at the first project sibling. On IO
    // error: no evidence, so no collapse — never a panic.
    let entries = std::fs::read_dir(dir.parent()?).ok()?;
    for entry in entries.flatten() {
        let sib = entry.file_name();
        let sib = sib.to_string_lossy();
        if sib == "Directory.Build.props"
            || sib.ends_with(".csproj")
            || sib.ends_with(".vbproj")
            || sib.ends_with(".fsproj")
            || sib.ends_with(".sln")
        {
            return Some(Collapse::BuildOutput);
        }
    }
    None
}

/// A directory carrying its own `.git` is another repository's tree: walking it
/// into the PARENT repo's graph mints duplicate same-named symbols that
/// `MergedGraph::resolve_name` then has to pick between (the documented
/// impact/trace flapping class), and feeds submodule sources to the parent's
/// module-prefix resolution.
///
/// A `.git` DIRECTORY is a nested clone. A `.git` FILE is a link: git submodules
/// point at `<super>/.git/modules/<name>`, linked worktrees at
/// `<main>/.git/worktrees/<name>`. An unparsable `.git` file still collapses —
/// its mere presence means "another repo's tree".
///
/// Mirrors, but deliberately does not import, the CLI's `resolve_gitdir_file`:
/// that one resolves the path, here only the classification matters.
pub fn nested_repo(dir: &Path) -> Option<Collapse> {
    // One stat, not two: this runs for every directory the walk considers.
    let g = dir.join(".git");
    let Ok(md) = std::fs::metadata(&g) else {
        return None;
    };
    if md.is_dir() {
        return Some(Collapse::NestedRepo);
    }
    if !md.is_file() {
        return None;
    }
    let Ok(text) = std::fs::read_to_string(&g) else {
        return Some(Collapse::Submodule);
    };
    let target = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("gitdir:"))
        .map(str::trim)
        .unwrap_or("");
    if target.contains("/worktrees/") {
        Some(Collapse::Worktree)
    } else {
        Some(Collapse::Submodule)
    }
}

/// One decision for a CHILD directory. Precedence, first match wins:
/// hard-skip -> nested repo -> always-region -> `ignored` -> dotnet -> bundle
/// -> descend.
///
/// `ignored` is the caller's `.gitignore` verdict for this directory name.
pub fn gate_dir(dir: &Path, name: &str, ignored: bool) -> Gate {
    if is_hard_skip(name) {
        return Gate::HardSkip;
    }
    // Before the name rules: a submodule named `build` is still a submodule,
    // and its provenance is the more useful fact.
    if let Some(c) = nested_repo(dir) {
        return Gate::Nested(c);
    }
    if let Some(c) = always_region(name) {
        return Gate::Always(c);
    }
    if ignored {
        return Gate::Ignored(Collapse::BuildOutput);
    }
    if let Some(c) = dotnet_build_dir(dir, name) {
        return Gate::Dotnet(c);
    }
    if dir_is_build_bundle(dir) {
        return Gate::Bundle(Collapse::BuildOutput);
    }
    Gate::Descend
}

/// True when the builder collapses this directory to a region instead of
/// descending. The store's freshness scan handles [`is_hard_skip`] separately
/// (a hard-skipped directory produces no node at all, so not even its own mtime
/// is observable), which is why that case is NOT folded in here.
pub fn dir_is_gated(dir: &Path, name: &str, gitignore: &HashSet<String>) -> bool {
    gate_dir(dir, name, gitignore.contains(name)).collapse().is_some()
}

/// Per-rule collapse tally behind the `[walk]` marker.
#[derive(Default, Clone, Copy, Debug)]
pub struct GateCounts {
    pub always: usize,
    pub gitignore: usize,
    pub bundle: usize,
    pub dotnet: usize,
    pub nested: usize,
}

impl GateCounts {
    pub fn record(&mut self, g: Gate) {
        match g {
            Gate::Descend | Gate::HardSkip => {}
            Gate::Always(_) => self.always += 1,
            Gate::Ignored(_) => self.gitignore += 1,
            Gate::Bundle(_) => self.bundle += 1,
            Gate::Dotnet(_) => self.dotnet += 1,
            Gate::Nested(_) => self.nested += 1,
        }
    }

    pub fn total(&self) -> usize {
        self.always + self.gitignore + self.bundle + self.dotnet + self.nested
    }

    /// `collapsed 5 regions (always=2 gitignore=1 bundle=0 dotnet=1 nested=1)`
    pub fn marker(&self) -> String {
        format!(
            "collapsed {} regions (always={} gitignore={} bundle={} dotnet={} nested={})",
            self.total(),
            self.always,
            self.gitignore,
            self.bundle,
            self.dotnet,
            self.nested
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("glia_gate_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn hard_skip_and_always_region_names() {
        assert!(is_hard_skip(".git") && is_hard_skip(".vs"));
        assert!(!is_hard_skip("src"));
        assert_eq!(always_region("node_modules"), Some(Collapse::Vendored));
        assert_eq!(always_region("TestResults"), Some(Collapse::BuildOutput));
        assert_eq!(always_region("src"), None);
        assert_eq!(Collapse::Submodule.provenance(), "submodule");
        assert_eq!(Collapse::Worktree.provenance(), "worktree");
        assert_eq!(Collapse::NestedRepo.provenance(), "nested_repo");
    }

    #[test]
    fn dotnet_gate_needs_evidence() {
        let root = tmp("dotnet");
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::create_dir_all(root.join("obj")).unwrap();
        // No project file anywhere: a Python repo's bin/ of console scripts.
        assert_eq!(dotnet_build_dir(&root.join("bin"), "bin"), None);
        assert_eq!(gate_dir(&root.join("bin"), "bin", false), Gate::Descend);
        // obj/project.assets.json is MSBuild's own marker — evidence on its own.
        std::fs::write(root.join("obj/project.assets.json"), "{}").unwrap();
        assert_eq!(
            gate_dir(&root.join("obj"), "obj", false),
            Gate::Dotnet(Collapse::BuildOutput)
        );
        // A project sibling covers bin/ too.
        std::fs::write(root.join("Api.csproj"), "<Project/>").unwrap();
        assert_eq!(
            gate_dir(&root.join("bin"), "bin", false),
            Gate::Dotnet(Collapse::BuildOutput)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn nested_git_classification() {
        let root = tmp("nested");
        let sub = root.join("sdk");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(nested_repo(&sub), None);
        std::fs::write(sub.join(".git"), "gitdir: ../../.git/modules/sdk\n").unwrap();
        assert_eq!(gate_dir(&sub, "sdk", false), Gate::Nested(Collapse::Submodule));
        std::fs::write(sub.join(".git"), "gitdir: /repo/.git/worktrees/wt\n").unwrap();
        assert_eq!(gate_dir(&sub, "sdk", false), Gate::Nested(Collapse::Worktree));
        // Unparsable content still means "another repo's tree".
        std::fs::write(sub.join(".git"), "garbage\n").unwrap();
        assert_eq!(nested_repo(&sub), Some(Collapse::Submodule));
        std::fs::remove_file(sub.join(".git")).unwrap();
        std::fs::create_dir_all(sub.join(".git")).unwrap();
        assert_eq!(gate_dir(&sub, "sdk", false), Gate::Nested(Collapse::NestedRepo));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn counts_render_the_marker() {
        let mut c = GateCounts::default();
        c.record(Gate::Descend);
        c.record(Gate::HardSkip);
        c.record(Gate::Dotnet(Collapse::BuildOutput));
        c.record(Gate::Dotnet(Collapse::BuildOutput));
        assert_eq!(c.total(), 2);
        assert_eq!(
            c.marker(),
            "collapsed 2 regions (always=0 gitignore=0 bundle=0 dotnet=2 nested=0)"
        );
    }
}
