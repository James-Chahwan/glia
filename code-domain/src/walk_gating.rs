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
//! verdict from an [`IgnoreStack`], so the matcher and the precedence rules stay
//! independent. Both consumers push/pop the same per-directory layers, so they
//! agree on nested `.gitignore` files, negation, anchoring and globs too (A8.2).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ignore::gitignore::{Gitignore, GitignoreBuilder};

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
    /// A `.gitignore` (the root's or a nested one) matches it.
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

/// Git's `.gitignore` semantics for the repo walk: one matcher per directory
/// that carries a `.gitignore`, outermost first. Uses only the `ignore` crate's
/// MATCHER — never its walker, which prunes ignored directories without a hook
/// to record them as REGION anchors, hides dotfiles by default, and would
/// reorder the name-sorted traversal the shard bytes depend on.
///
/// Git's rule, which [`IgnoreStack::is_ignored`] follows: the DEEPEST
/// `.gitignore` with a matching pattern decides, the last matching line inside
/// that file wins, and so a whitelist (`!pat`) in a deeper file overrides an
/// ignore in an outer one. A pattern with a slash is anchored to the directory
/// of the file that holds it, so a nested rule never leaks upward or sideways.
///
/// `.git/info/exclude` and the global excludes file are deliberately NOT read:
/// they are per-checkout, so honouring them would make the graph depend on the
/// machine it was built on (and bench fixtures have no `.git`).
///
/// Every failure (unreadable file, bad glob) is logged and degrades to "not
/// ignored" — a malformed `.gitignore` must never sink a build.
#[derive(Default, Clone)]
pub struct IgnoreStack {
    /// `(directory as the walker spelled it, its matcher)`. The directory is
    /// kept verbatim because `GitignoreBuilder` strips a leading `./` from its
    /// root, and the under-root guard in `is_ignored` must compare like with
    /// like.
    layers: Vec<(PathBuf, Arc<Gitignore>)>,
    /// `.gitignore` files successfully loaded.
    pub files: usize,
    /// Ignore + whitelist globs across every loaded file.
    pub patterns: usize,
    /// FILES (not directories) skipped because a pattern matched them.
    pub skipped_files: usize,
}

impl IgnoreStack {
    /// Push `dir`'s own `.gitignore` if it has one. Returns true when a layer
    /// was pushed — the caller MUST [`pop`](Self::pop) after descending.
    ///
    /// `dir` must be spelled the way the walker spells the paths below it
    /// (both absolute, or both relative to the same base), because matching is
    /// relative to it.
    pub fn push_dir(&mut self, dir: &Path) -> bool {
        let gi_path = dir.join(".gitignore");
        if !gi_path.is_file() {
            return false;
        }
        let mut builder = GitignoreBuilder::new(dir);
        // `add` keeps every line that parsed and reports the rest, so a single
        // bad glob costs that line, not the file.
        // The error already names the file and line.
        if let Some(e) = builder.add(&gi_path) {
            eprintln!("[walk] warning: {e}");
        }
        match builder.build() {
            Ok(gi) => {
                self.files += 1;
                self.patterns += (gi.num_ignores() + gi.num_whitelists()) as usize;
                // A comments-only file is loaded but has nothing to say.
                if gi.is_empty() {
                    return false;
                }
                self.layers.push((dir.to_path_buf(), Arc::new(gi)));
                true
            }
            Err(e) => {
                eprintln!("[walk] warning: {}: {e}", gi_path.display());
                false
            }
        }
    }

    /// Drop the innermost layer (pair with a `push_dir` that returned true).
    pub fn pop(&mut self) {
        self.layers.pop();
    }

    /// True when git would ignore `path`. `path` is the child path as the
    /// walker built it (`dir.join(name)`), never a repo-relative one.
    pub fn is_ignored(&self, path: &Path, is_dir: bool) -> bool {
        for (root, gi) in self.layers.iter().rev() {
            // `matched_path_or_any_parents` ASSERTS the path is under the
            // matcher's root; a layer that does not cover `path` has no say.
            if !path.starts_with(root) {
                continue;
            }
            let m = gi.matched_path_or_any_parents(path, is_dir);
            if m.is_whitelist() {
                return false;
            }
            if m.is_ignore() {
                return true;
            }
        }
        false
    }

    /// [`is_ignored`](Self::is_ignored) for a FILE, counting the skip for the
    /// marker. File-level gating is what drops committed-but-ignored build
    /// output (`*.min.js`, `*_pb2.py`) that sits beside authored source.
    pub fn skip_file(&mut self, path: &Path) -> bool {
        let hit = self.is_ignored(path, false);
        if hit {
            self.skipped_files += 1;
        }
        hit
    }

    /// `gitignore 2 files, 4 patterns, skipped 1 files`
    pub fn marker(&self) -> String {
        format!(
            "gitignore {} files, {} patterns, skipped {} files",
            self.files, self.patterns, self.skipped_files
        )
    }
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
/// `ignored` is the caller's `.gitignore` verdict for this directory
/// ([`IgnoreStack::is_ignored`] with `is_dir = true`).
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
pub fn dir_is_gated(dir: &Path, name: &str, ignores: &IgnoreStack) -> bool {
    gate_dir(dir, name, ignores.is_ignored(dir, true)).collapse().is_some()
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

    /// Mirror of the walkers' contract: push the directory's layer, query its
    /// children, pop.
    fn verdict(stack: &mut IgnoreStack, dirs: &[&Path], path: &Path, is_dir: bool) -> bool {
        let pushed: Vec<bool> = dirs.iter().map(|d| stack.push_dir(d)).collect();
        let hit = stack.is_ignored(path, is_dir);
        for p in pushed.into_iter().rev() {
            if p {
                stack.pop();
            }
        }
        hit
    }

    #[test]
    fn ignore_stack_anchoring_globs_negation() {
        let root = tmp("gi_semantics");
        std::fs::write(root.join(".gitignore"), "# c\n/generated\ndist-*\n!dist-keep\n*.min.js\n")
            .unwrap();
        std::fs::create_dir_all(root.join("src/generated")).unwrap();
        let mut st = IgnoreStack::default();
        let src = root.join("src");
        // Anchored: only the root's own `generated`.
        assert!(verdict(&mut st, &[&root], &root.join("generated"), true));
        assert!(!verdict(&mut st, &[&root, &src], &src.join("generated"), true));
        // Globs match at any depth; negation re-includes.
        assert!(verdict(&mut st, &[&root], &root.join("dist-x"), true));
        assert!(verdict(&mut st, &[&root, &src], &src.join("dist-y"), true));
        assert!(!verdict(&mut st, &[&root], &root.join("dist-keep"), true));
        // File-level patterns, and a file under a directory pattern.
        assert!(verdict(&mut st, &[&root], &root.join("vendor.min.js"), false));
        assert!(!verdict(&mut st, &[&root], &root.join("app.js"), false));
        assert!(verdict(&mut st, &[&root], &root.join("generated/x.py"), false));
        assert!(st.layers.is_empty(), "every push was popped");
        // 4 patterns (the comment is not one), counted once per push.
        let mut one = IgnoreStack::default();
        assert!(one.push_dir(&root));
        assert_eq!((one.files, one.patterns), (1, 4));
        assert!(one.skip_file(&root.join("a.min.js")));
        assert!(!one.skip_file(&root.join("a.js")));
        assert_eq!(one.marker(), "gitignore 1 files, 4 patterns, skipped 1 files");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn ignore_stack_nested_files_scope_and_override() {
        let root = tmp("gi_nested");
        let pkg = root.join("pkg");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(root.join(".gitignore"), "cache/\n").unwrap();
        std::fs::write(pkg.join(".gitignore"), "generated/\n!cache/\n").unwrap();
        let mut st = IgnoreStack::default();
        // The nested rule applies below its own directory...
        assert!(verdict(&mut st, &[&root, &pkg], &pkg.join("generated"), true));
        // ...and never leaks upward.
        assert!(!verdict(&mut st, &[&root], &root.join("generated"), true));
        // A deeper whitelist beats an outer ignore; the outer rule still holds
        // everywhere the deeper file does not reach.
        assert!(!verdict(&mut st, &[&root, &pkg], &pkg.join("cache"), true));
        assert!(verdict(&mut st, &[&root], &root.join("cache"), true));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn ignore_stack_degrades_instead_of_panicking() {
        let root = tmp("gi_degrade");
        // No .gitignore: nothing pushed, nothing ignored.
        let mut st = IgnoreStack::default();
        assert!(!st.push_dir(&root));
        assert!(!st.is_ignored(&root.join("x"), true));
        // An unclosed alternation is a bad glob: that line is dropped, the
        // good line still applies, and nothing panics.
        std::fs::write(root.join(".gitignore"), "{broken\nout-*\n").unwrap();
        assert!(st.push_dir(&root));
        assert!(st.is_ignored(&root.join("out-1"), true));
        // A path the layer does not cover (would trip the crate's assert).
        assert!(!st.is_ignored(Path::new("/definitely/elsewhere/x"), false));
        st.pop();
        // A `.gitignore` DIRECTORY is not a file: skipped.
        let odd = root.join("odd");
        std::fs::create_dir_all(odd.join(".gitignore")).unwrap();
        assert!(!st.push_dir(&odd));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn ignore_stack_relative_dot_root() {
        // `GitignoreBuilder` strips a leading `./` from its root, so the
        // under-root guard must compare against the walker's own spelling.
        // Build a `./..`-relative spelling of the temp dir WITHOUT changing the
        // process cwd (tests run in parallel).
        let root = tmp("gi_rel");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join(".gitignore"), "/gen\n").unwrap();
        std::fs::write(root.join("sub/.gitignore"), "/deep\n").unwrap();
        let cwd = std::env::current_dir().unwrap();
        let mut rel = PathBuf::from(".");
        for _ in cwd.components().skip(1) {
            rel.push("..");
        }
        rel.push(root.strip_prefix("/").unwrap());
        assert!(rel.join(".gitignore").is_file(), "{}", rel.display());
        let mut st = IgnoreStack::default();
        let sub = rel.join("sub");
        assert!(verdict(&mut st, &[&rel], &rel.join("gen"), true));
        assert!(!verdict(&mut st, &[&rel, &sub], &sub.join("gen"), true));
        assert!(verdict(&mut st, &[&rel, &sub], &sub.join("deep"), true));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn dir_is_gated_uses_the_matcher() {
        let root = tmp("gi_gated");
        std::fs::create_dir_all(root.join("src/generated")).unwrap();
        std::fs::write(root.join(".gitignore"), "/generated\n").unwrap();
        let mut st = IgnoreStack::default();
        st.push_dir(&root);
        assert!(dir_is_gated(&root.join("generated"), "generated", &st));
        assert!(!dir_is_gated(&root.join("src/generated"), "generated", &st));
        // always_region still outranks the matcher: `build` collapses anywhere.
        assert!(dir_is_gated(&root.join("src/build"), "build", &st));
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
