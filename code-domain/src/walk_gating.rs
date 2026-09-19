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
//!
//! The user's `[walk] skip` patterns (`.glia/overlay.toml`, LF.3a) ride the same
//! stack: both consumers build it through [`IgnoreStack::with_config`] (the
//! store via [`IgnoreStack::for_repo`]), so a user skip gates the freshness scan
//! exactly as it gates the walk. A skipped directory is a REGION with the
//! `excluded` provenance; a skipped file is simply not read.
//!
//! The same `.git` reading also answers "which repository is this?" for the
//! build: [`repo_identity`] turns a checkout into a path-independent key (its
//! normalised git remote, its git dir, or its directory name) that the engine
//! hashes into the `RepoId`, so every NodeId survives a re-spelled path, a
//! second clone, a linked worktree or a moved checkout (LB.1).

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
    /// A `[walk] skip` pattern in `.glia/overlay.toml` matches it: the user told
    /// glia not to read it (LF.3a).
    Excluded,
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
            Collapse::Excluded => "excluded",
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
    /// VCS internals / editor metadata / glia's control dir: skip outright,
    /// not even a region.
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
    /// A user `[walk] skip` pattern matches it (LF.3a).
    Config(Collapse),
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
            | Gate::Nested(c)
            | Gate::Config(c) => Some(c),
        }
    }
}

/// glia's control directory (LF.1d): the engine's output (`.glia/graph`) and
/// every input glia reads from outside the parse — `overlay.toml`, the cell
/// sidecar, vectors, the docs / history / test snapshots. [`is_hard_skip`]
/// covers it, so the walk never parses under it and never turns it into a
/// REGION, and the store's mtime scan never descends into it. Its readers
/// open their files directly, never through the walk; the store tracks the
/// inputs by content instead (`repo_graph_store::external_inputs_fingerprint`),
/// which is gitignore-blind by design.
///
/// Name-based like the rest of [`is_hard_skip`], so a nested `pkg/.glia` is
/// skipped too, though only the repo root's is ever read.
pub const CONTROL_DIR: &str = ".glia";

/// VCS internals, editor metadata and glia's own [`CONTROL_DIR`]: no source to
/// parse, skipped outright (not even recorded as a region). Also guards FILES —
/// a `.git` file must never be read as source.
pub fn is_hard_skip(name: &str) -> bool {
    matches!(name, ".git" | ".hg" | ".svn" | ".idea" | ".vscode" | ".vs") || name == CONTROL_DIR
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
///
/// The user's `[walk] skip` matcher (LF.3a) is held beside the `.gitignore`
/// layers but answered separately ([`IgnoreStack::is_excluded`]): it only ever
/// ADDS a skip, so a `!pattern` there cannot un-ignore a gitignored path, and
/// [`gate_dir`] checks it before every name rule, so it cannot un-collapse one
/// either.
#[derive(Default, Clone)]
pub struct IgnoreStack {
    /// `(directory as the walker spelled it, its matcher)`. The directory is
    /// kept verbatim because `GitignoreBuilder` strips a leading `./` from its
    /// root, and the under-root guard in `is_ignored` must compare like with
    /// like.
    layers: Vec<(PathBuf, Arc<Gitignore>)>,
    /// The root-anchored `[walk] skip` matcher, `None` without one. One `Arc`,
    /// so the per-directory clones in the store's scan stay pointer copies.
    config: Option<Arc<ConfigSkip>>,
    /// `.gitignore` files successfully loaded.
    pub files: usize,
    /// Ignore + whitelist globs across every loaded file.
    pub patterns: usize,
    /// FILES (not directories) skipped because a pattern matched them.
    pub skipped_files: usize,
    /// `[walk] skip` patterns in the config matcher.
    pub config_patterns: usize,
    /// FILES (not directories) skipped because a `[walk] skip` pattern matched.
    pub excluded_files: usize,
}

/// The user's `[walk] skip` patterns compiled into one matcher rooted at the
/// repo root.
struct ConfigSkip {
    /// The repo root as the walker spelled it (see `IgnoreStack::layers`).
    root: PathBuf,
    matcher: Gitignore,
}

impl IgnoreStack {
    /// An empty stack carrying the user's `[walk] skip` patterns (LF.3a).
    ///
    /// Gitignore syntax and semantics, relative to `root` (the repo root,
    /// spelled the way the walker spells the paths below it): a pattern with no
    /// slash (`legacy`, `*.gen.py`) matches that name at ANY depth, one with a
    /// leading or inner slash (`/legacy`, `tools/old`) only at that path from
    /// the root, and a trailing slash (`fixtures/`) only directories. A `!`
    /// line re-includes only against the config's own earlier patterns: it
    /// never un-skips a hard skip, a collapsed region or a gitignored path.
    ///
    /// The loader (`glia_config::load`) has already compiled every pattern
    /// with the same `add_line`, so none fails here. A matcher that fails to
    /// build is reported and leaves the stack without config skips.
    pub fn with_config(root: &Path, walk: &crate::glia_config::WalkConfig) -> Self {
        let mut stack = Self::default();
        if walk.skip.is_empty() {
            return stack;
        }
        let mut builder = GitignoreBuilder::new(root);
        for pattern in &walk.skip {
            // Already validated by the loader; a failure here drops the line.
            let _ = builder.add_line(None, pattern);
        }
        match builder.build() {
            Ok(matcher) => {
                stack.config_patterns = matcher.num_ignores() as usize + matcher.num_whitelists() as usize;
                if !matcher.is_empty() {
                    stack.config = Some(Arc::new(ConfigSkip { root: root.to_path_buf(), matcher }));
                }
            }
            Err(e) => eprintln!("[walk] warning: {} [walk] skip: {e}", crate::glia_config::OVERLAY_FILE),
        }
        stack
    }

    /// [`with_config`](Self::with_config) on the repo's own
    /// `.glia/overlay.toml`, loaded silently: the build reports loader errors,
    /// never a freshness check. What the store's scan constructs, so it gates
    /// exactly what the walk gates.
    pub fn for_repo(root: &Path) -> Self {
        match crate::glia_config::load(root) {
            Some(loaded) => Self::with_config(root, &loaded.config.walk),
            None => Self::default(),
        }
    }

    /// True when a user `[walk] skip` pattern matches `path` (the child path
    /// as the walker built it, under the root given to
    /// [`with_config`](Self::with_config)).
    pub fn is_excluded(&self, path: &Path, is_dir: bool) -> bool {
        let Some(cfg) = &self.config else { return false };
        // `matched_path_or_any_parents` ASSERTS the path is under the root.
        if !path.starts_with(&cfg.root) {
            return false;
        }
        cfg.matcher.matched_path_or_any_parents(path, is_dir).is_ignore()
    }

    /// [`is_excluded`](Self::is_excluded) for a FILE, counting the skip for
    /// [`config_marker`](Self::config_marker).
    pub fn exclude_file(&mut self, path: &Path) -> bool {
        let hit = self.is_excluded(path, false);
        if hit {
            self.excluded_files += 1;
        }
        hit
    }

    /// `config skip 2 patterns, excluded 1 files`
    pub fn config_marker(&self) -> String {
        format!(
            "config skip {} patterns, excluded {} files",
            self.config_patterns, self.excluded_files
        )
    }

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

/// Which rule produced a [`RepoIdentity`]. The string form is the `source=`
/// field of the engine's `[repo-id]` marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentitySource {
    /// `git:<normalised remote url>[/<path within the checkout>]`.
    GitRemote,
    /// A git checkout with no usable remote:
    /// `gitdir:<main checkout dir name>[/<path within the checkout>]`.
    GitLocal,
    /// No `.git` anywhere up the ancestor chain: `dir:<basename>`.
    Directory,
}

impl IdentitySource {
    pub fn as_str(self) -> &'static str {
        match self {
            IdentitySource::GitRemote => "git-remote",
            IdentitySource::GitLocal => "git-local",
            IdentitySource::Directory => "dir",
        }
    }
}

/// A repo's path-independent identity: `key` is what the engine feeds to
/// `RepoId::from_canonical`, `source` the rule that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoIdentity {
    pub key: String,
    pub source: IdentitySource,
}

/// Ancestor levels [`repo_identity`] climbs looking for a `.git` entry.
const MAX_GIT_ANCESTORS: usize = 64;

/// What a checkout IS, not where it sits. First rule that applies wins:
///
/// 1. **git remote** — the nearest `.git` up from `root` names a common git dir
///    whose `config` has a remote: `origin`, else the first `[remote "…"]` in
///    file order. Key `git:<normalise_remote_url>` plus `/<rel>` when `root` is
///    a subdirectory of the checkout (`rel` is `/`-separated). Two clones of one
///    remote, and every linked worktree of one clone, agree.
/// 2. **git local** — a checkout with no remote: `gitdir:<name>` (+ `/<rel>`),
///    where `name` is the main checkout's directory name (the common dir is
///    shared by every worktree, so they agree), or the common dir's own name,
///    minus `.git`, when it is not literally `.git` (a bare repo's worktree, a
///    submodule's `.git/modules/<name>`).
/// 3. **directory** — no `.git` anywhere up the chain: `dir:<basename>`.
///
/// A `.git` DIRECTORY is the common git dir. A `.git` FILE is followed through
/// its `gitdir:` line (relative to the file's directory unless absolute); a
/// linked worktree's gitdir holds a `commondir` pointing at the shared dir, a
/// submodule's does not and keeps its own `config`.
///
/// Consequences to know: a home directory that is itself a git repo (dotfiles)
/// gives every non-git project under it a `git:<dotfiles remote>/<rel>` key —
/// stable, but path-shaped. Mirror clones with different remotes (GitHub vs
/// GitLab) get different keys. Two unrelated non-git directories with the same
/// basename share a key when built separately; inside one multi-repo build the
/// engine disambiguates them.
///
/// Best-effort and side-effect free: no git binary, no network, and any IO
/// error falls through to the next rule. Userinfo in a remote URL (tokens,
/// passwords) never reaches the key.
pub fn repo_identity(root: &Path) -> RepoIdentity {
    let abs = std::fs::canonicalize(root)
        .or_else(|_| std::path::absolute(root))
        .unwrap_or_else(|_| root.to_path_buf());
    let Some((toplevel, common)) = find_git_checkout(&abs) else {
        return RepoIdentity {
            key: format!("dir:{}", path_name(&abs)),
            source: IdentitySource::Directory,
        };
    };
    let rel = rel_slash_path(&abs, &toplevel);
    let with_rel = |base: String| {
        if rel.is_empty() { base } else { format!("{base}/{rel}") }
    };
    if let Some(url) = common.as_deref().and_then(|c| remote_url(&c.join("config")))
        && let Some(norm) = normalise_remote_url(&url)
    {
        return RepoIdentity {
            key: with_rel(format!("git:{norm}")),
            source: IdentitySource::GitRemote,
        };
    }
    let name = match common.as_deref() {
        Some(c) if c.file_name().is_some_and(|n| n == ".git") => {
            c.parent().map(path_name).unwrap_or_else(|| path_name(&toplevel))
        }
        Some(c) => {
            let own = path_name(c);
            own.strip_suffix(".git").filter(|s| !s.is_empty()).map(str::to_string).unwrap_or(own)
        }
        None => path_name(&toplevel),
    };
    RepoIdentity {
        key: with_rel(format!("gitdir:{name}")),
        source: IdentitySource::GitLocal,
    }
}

/// The nearest ancestor of `abs` (itself included) holding a `.git` entry, and
/// the common git dir that entry leads to (`None` when a `.git` file cannot be
/// resolved — the checkout is still a git checkout, just an unreadable one).
fn find_git_checkout(abs: &Path) -> Option<(PathBuf, Option<PathBuf>)> {
    for dir in abs.ancestors().take(MAX_GIT_ANCESTORS) {
        let g = dir.join(".git");
        let Ok(md) = std::fs::metadata(&g) else { continue };
        if md.is_dir() {
            return Some((dir.to_path_buf(), Some(normalise_dir(&g))));
        }
        if md.is_file() {
            return Some((dir.to_path_buf(), common_dir_of_git_file(&g, dir)));
        }
    }
    None
}

/// Follow a `.git` FILE: its `gitdir:` target, then that dir's `commondir`
/// when present (linked worktrees), else the gitdir itself (submodules).
fn common_dir_of_git_file(git_file: &Path, holder: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(git_file).ok()?;
    let target = text.lines().find_map(|l| l.trim().strip_prefix("gitdir:"))?.trim();
    if target.is_empty() {
        return None;
    }
    let gitdir = holder.join(target);
    let common = match std::fs::read_to_string(gitdir.join("commondir")) {
        Ok(c) if !c.trim().is_empty() => gitdir.join(c.trim()),
        _ => gitdir,
    };
    Some(normalise_dir(&common))
}

/// `canonicalize`, or a lexical `..` / `.` fold when the dir does not exist, so
/// `<main>/.git/worktrees/wt2/../..` names `<main>/.git` either way.
fn normalise_dir(p: &Path) -> PathBuf {
    if let Ok(c) = std::fs::canonicalize(p) {
        return c;
    }
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Last path component, or the whole path when it has none (`/`).
fn path_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.to_string_lossy().into_owned())
}

/// `abs` below `top`, `/`-separated, `""` at the top itself.
fn rel_slash_path(abs: &Path, top: &Path) -> String {
    let Ok(rel) = abs.strip_prefix(top) else {
        return String::new();
    };
    rel.components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// The `url` of `[remote "origin"]` in a git config file, else of the first
/// remote in file order. A minimal reader: section headers (`[remote "x"]` and
/// the legacy `[remote.x]`), `key = value` lines, `#` / `;` comments, quoted
/// values. `include` directives are not followed.
fn remote_url(config: &Path) -> Option<String> {
    let text = std::fs::read_to_string(config).ok()?;
    let mut remotes: Vec<(String, String)> = Vec::new();
    let mut section: Option<String> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            let header = rest.split(']').next().unwrap_or("").trim();
            section = remote_section_name(header);
            continue;
        }
        let Some(remote) = section.as_ref() else { continue };
        let Some((k, v)) = line.split_once('=') else { continue };
        if !k.trim().eq_ignore_ascii_case("url") {
            continue;
        }
        let value = config_value(v);
        if !value.is_empty() && !remotes.iter().any(|(r, _)| r == remote) {
            remotes.push((remote.clone(), value));
        }
    }
    remotes
        .iter()
        .find(|(r, _)| r == "origin")
        .or_else(|| remotes.first())
        .map(|(_, u)| u.clone())
}

/// `remote "name"` / `remote.name` → `name`; any other section → `None`.
fn remote_section_name(header: &str) -> Option<String> {
    let (sect, sub) = match header.split_once(char::is_whitespace) {
        Some((s, sub)) => (s, sub.trim().trim_matches('"').to_string()),
        None => match header.split_once('.') {
            Some((s, sub)) => (s, sub.to_string()),
            None => (header, String::new()),
        },
    };
    (sect.eq_ignore_ascii_case("remote") && !sub.is_empty()).then_some(sub)
}

/// A config value: quotes removed, backslash escapes kept literal, cut at the
/// first unquoted `#` / `;`, trimmed.
fn config_value(v: &str) -> String {
    let mut out = String::new();
    let mut quoted = false;
    let mut chars = v.trim().chars();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => quoted = !quoted,
            '\\' => {
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            }
            '#' | ';' if !quoted => break,
            c => out.push(c),
        }
    }
    out.trim().to_string()
}

/// A git remote URL reduced to `host/owner/repo` (or the bare path for a local
/// remote), so every spelling of one remote agrees:
/// `git@github.com:Example/Shop.git`, `https://user:tok@github.com/example/shop`
/// and `ssh://git@github.com:22/Example/Shop.git/` all give
/// `github.com/example/shop`.
///
/// Strips the scheme (`https://`, `ssh://`, `git://`, `git+ssh://`, `file://`,
/// any `<scheme>://`), everything up to the LAST `@` of the authority
/// (userinfo, including tokens — never emitted), a numeric `:port`, a query or
/// fragment, trailing `/` and `.git`; rewrites the scp form `host:path` to
/// `host/path`; lowercases. `None` when nothing is left.
pub fn normalise_remote_url(url: &str) -> Option<String> {
    let url = url.trim();
    let (authority, path) = match url.split_once("://") {
        Some((_, rest)) => match rest.split_once('/') {
            Some((auth, path)) => (auth, path),
            None => (rest, ""),
        },
        None => match scp_split(url) {
            Some((auth, path)) => (auth, path),
            None => ("", url),
        },
    };
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = match host.rsplit_once(':') {
        Some((h, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => h,
        Some((h, "")) => h,
        _ => host,
    };
    let path = path.split(['?', '#']).next().unwrap_or("");
    let mut path = path.trim_matches('/');
    while let Some(p) = path.strip_suffix(".git") {
        path = p.trim_end_matches('/');
    }
    let joined = match (host.is_empty(), path.is_empty()) {
        (true, true) => return None,
        (true, false) => path.to_string(),
        (false, true) => host.to_string(),
        (false, false) => format!("{host}/{path}"),
    };
    Some(joined.to_ascii_lowercase())
}

/// The scp-like `[user@]host:path` form: a `:` before any `/`, and not a
/// one-letter Windows drive (`C:\repo`).
fn scp_split(url: &str) -> Option<(&str, &str)> {
    let colon = url.find(':')?;
    if url[..colon].contains('/') {
        return None;
    }
    let (auth, path) = (&url[..colon], &url[colon + 1..]);
    let host_part = auth.rsplit_once('@').map_or(auth, |(_, h)| h);
    if host_part.len() == 1 && host_part.bytes().all(|b| b.is_ascii_alphabetic()) {
        return None;
    }
    Some((auth, path))
}

/// One decision for a CHILD directory. Precedence, first match wins:
/// hard-skip -> nested repo -> `excluded` -> always-region -> `ignored` ->
/// dotnet -> bundle -> descend.
///
/// `ignored` is the caller's `.gitignore` verdict for this directory
/// ([`IgnoreStack::is_ignored`] with `is_dir = true`); `excluded` its user
/// `[walk] skip` verdict ([`IgnoreStack::is_excluded`], LF.3a).
pub fn gate_dir(dir: &Path, name: &str, ignored: bool, excluded: bool) -> Gate {
    if is_hard_skip(name) {
        return Gate::HardSkip;
    }
    // Before the name rules: a submodule named `build` is still a submodule,
    // and its provenance is the more useful fact.
    if let Some(c) = nested_repo(dir) {
        return Gate::Nested(c);
    }
    // A user skip is the more specific fact than a name rule: `vendor` that
    // the config names is recorded as `excluded`, not `vendored`. It can only
    // add a collapse: it is never asked to descend.
    if excluded {
        return Gate::Config(Collapse::Excluded);
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
    gate_dir(dir, name, ignores.is_ignored(dir, true), ignores.is_excluded(dir, true))
        .collapse()
        .is_some()
}

/// Per-rule collapse tally behind the `[walk]` marker.
#[derive(Default, Clone, Copy, Debug)]
pub struct GateCounts {
    pub always: usize,
    pub gitignore: usize,
    pub bundle: usize,
    pub dotnet: usize,
    pub nested: usize,
    /// User `[walk] skip` collapses (LF.3a).
    pub config: usize,
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
            Gate::Config(_) => self.config += 1,
        }
    }

    pub fn total(&self) -> usize {
        self.always + self.gitignore + self.bundle + self.dotnet + self.nested + self.config
    }

    /// `collapsed 5 regions (always=2 gitignore=1 bundle=0 dotnet=1 nested=1 config=0)`
    pub fn marker(&self) -> String {
        format!(
            "collapsed {} regions (always={} gitignore={} bundle={} dotnet={} nested={} config={})",
            self.total(),
            self.always,
            self.gitignore,
            self.bundle,
            self.dotnet,
            self.nested,
            self.config
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
        // LF.1d: glia's control dir, by name at any depth; only the exact name.
        assert!(is_hard_skip(CONTROL_DIR) && is_hard_skip(".glia"));
        assert!(!is_hard_skip(".glia2") && !is_hard_skip("glia"));
        assert_eq!(gate_dir(Path::new("r/.glia"), ".glia", true, true), Gate::HardSkip);
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
        assert_eq!(gate_dir(&root.join("bin"), "bin", false, false), Gate::Descend);
        // obj/project.assets.json is MSBuild's own marker — evidence on its own.
        std::fs::write(root.join("obj/project.assets.json"), "{}").unwrap();
        assert_eq!(
            gate_dir(&root.join("obj"), "obj", false, false),
            Gate::Dotnet(Collapse::BuildOutput)
        );
        // A project sibling covers bin/ too.
        std::fs::write(root.join("Api.csproj"), "<Project/>").unwrap();
        assert_eq!(
            gate_dir(&root.join("bin"), "bin", false, false),
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
        assert_eq!(gate_dir(&sub, "sdk", false, false), Gate::Nested(Collapse::Submodule));
        std::fs::write(sub.join(".git"), "gitdir: /repo/.git/worktrees/wt\n").unwrap();
        assert_eq!(gate_dir(&sub, "sdk", false, false), Gate::Nested(Collapse::Worktree));
        // Unparsable content still means "another repo's tree".
        std::fs::write(sub.join(".git"), "garbage\n").unwrap();
        assert_eq!(nested_repo(&sub), Some(Collapse::Submodule));
        std::fs::remove_file(sub.join(".git")).unwrap();
        std::fs::create_dir_all(sub.join(".git")).unwrap();
        assert_eq!(gate_dir(&sub, "sdk", false, false), Gate::Nested(Collapse::NestedRepo));
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
            "collapsed 2 regions (always=0 gitignore=0 bundle=0 dotnet=2 nested=0 config=0)"
        );
        c.record(Gate::Config(Collapse::Excluded));
        assert_eq!(
            c.marker(),
            "collapsed 3 regions (always=0 gitignore=0 bundle=0 dotnet=2 nested=0 config=1)"
        );
        assert_eq!(Collapse::Excluded.provenance(), "excluded");
    }

    fn walk_cfg(skip: &[&str]) -> crate::glia_config::WalkConfig {
        crate::glia_config::WalkConfig { skip: skip.iter().map(|s| s.to_string()).collect() }
    }

    /// LF.3a: `[walk] skip` is gitignore syntax rooted at the repo root.
    #[test]
    fn config_skip_is_root_anchored_gitignore_syntax() {
        let root = tmp("cfg_anchor");
        let st = IgnoreStack::with_config(&root, &walk_cfg(&["legacy", "/tools/old", "*.gen.py", "fixtures/"]));
        assert_eq!(st.config_patterns, 4);
        // No slash: any depth, directory or file.
        assert!(st.is_excluded(&root.join("legacy"), true));
        assert!(st.is_excluded(&root.join("pkg/legacy"), true));
        assert!(st.is_excluded(&root.join("legacy/old.py"), false), "a file under a skipped dir");
        // A slash anchors at the root only.
        assert!(st.is_excluded(&root.join("tools/old"), true));
        assert!(!st.is_excluded(&root.join("pkg/tools/old"), true));
        // Globs, and a trailing slash that only matches directories.
        assert!(st.is_excluded(&root.join("src/api.gen.py"), false));
        assert!(!st.is_excluded(&root.join("src/api.py"), false));
        assert!(st.is_excluded(&root.join("fixtures"), true));
        assert!(!st.is_excluded(&root.join("fixtures"), false));
        // Outside the root: no say (the crate would assert).
        assert!(!st.is_excluded(Path::new("/definitely/elsewhere/legacy"), true));
        // The gitignore half of the stack is untouched by config patterns.
        assert!(!st.is_ignored(&root.join("legacy"), true));
        // No patterns: no matcher, nothing excluded.
        let none = IgnoreStack::with_config(&root, &walk_cfg(&[]));
        assert!(!none.is_excluded(&root.join("legacy"), true));
        assert_eq!(none.config_patterns, 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// LF.3a: config only adds skips. It outranks the name rules (the more
    /// specific fact) but never un-skips a hard skip, un-collapses a region or
    /// re-includes a gitignored path.
    #[test]
    fn config_skip_extends_never_replaces() {
        let root = tmp("cfg_extend");
        std::fs::create_dir_all(root.join("node_modules")).unwrap();
        std::fs::create_dir_all(root.join("gen")).unwrap();
        std::fs::write(root.join(".gitignore"), "/gen\n").unwrap();
        let mut st = IgnoreStack::with_config(&root, &walk_cfg(&["!node_modules", "!gen", "vendor"]));
        assert!(st.push_dir(&root));
        assert!(!st.is_excluded(&root.join("node_modules"), true));
        assert!(dir_is_gated(&root.join("node_modules"), "node_modules", &st), "still vendored");
        assert!(dir_is_gated(&root.join("gen"), "gen", &st), "still gitignored");
        assert_eq!(
            gate_dir(&root.join("node_modules"), "node_modules", false, st.is_excluded(&root.join("node_modules"), true)),
            Gate::Always(Collapse::Vendored)
        );
        // The more specific fact: a user skip on `vendor` records `excluded`.
        assert_eq!(gate_dir(&root.join("vendor"), "vendor", false, true), Gate::Config(Collapse::Excluded));
        assert_eq!(gate_dir(&root.join("gen"), "gen", true, true), Gate::Config(Collapse::Excluded));
        // A hard skip and another repo's tree keep their own verdicts.
        assert_eq!(gate_dir(&root.join(".git"), ".git", false, true), Gate::HardSkip);
        let sub = root.join("sdk");
        std::fs::create_dir_all(sub.join(".git")).unwrap();
        assert_eq!(gate_dir(&sub, "sdk", false, true), Gate::Nested(Collapse::NestedRepo));
        // File skips are counted for the marker.
        let mut cfg = IgnoreStack::with_config(&root, &walk_cfg(&["*.gen.py"]));
        assert!(cfg.exclude_file(&root.join("a.gen.py")));
        assert!(!cfg.exclude_file(&root.join("a.py")));
        assert_eq!(cfg.config_marker(), "config skip 1 patterns, excluded 1 files");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// LF.3a: `for_repo` reads the repo's own `.glia/overlay.toml`, silently,
    /// and a clone of the stack shares the one matcher.
    #[test]
    fn for_repo_loads_the_overlay_walk_section() {
        let root = tmp("cfg_repo");
        assert!(!IgnoreStack::for_repo(&root).is_excluded(&root.join("legacy"), true), "no file");
        std::fs::create_dir_all(root.join(".glia")).unwrap();
        std::fs::write(root.join(".glia/overlay.toml"), "version = 1\n[walk]\nskip = [\"legacy\"]\n").unwrap();
        let st = IgnoreStack::for_repo(&root);
        assert!(st.is_excluded(&root.join("legacy"), true));
        assert!(dir_is_gated(&root.join("legacy"), "legacy", &st));
        let copy = st.clone();
        assert!(Arc::ptr_eq(
            st.config.as_ref().expect("matcher"),
            copy.config.as_ref().expect("matcher")
        ));
        // A file the loader rejects as a whole (no version) skips nothing.
        std::fs::write(root.join(".glia/overlay.toml"), "[walk]\nskip = [\"legacy\"]\n").unwrap();
        assert!(!IgnoreStack::for_repo(&root).is_excluded(&root.join("legacy"), true));
        let _ = std::fs::remove_dir_all(&root);
    }

    fn write(dir: &Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    const SHOP_CONFIG: &str = "[core]\n\tbare = false\n[remote \"origin\"]\n\turl = git@github.com:Example/Shop.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n";

    #[test]
    fn remote_urls_normalise_to_one_spelling() {
        let shop = Some("github.com/example/shop".to_string());
        for url in [
            "git@github.com:Example/Shop.git",
            "https://user:tok@github.com/example/shop",
            "ssh://git@github.com:22/Example/Shop.git/",
            "https://github.com/Example/Shop",
            "git+ssh://git@github.com/Example/Shop.git",
            "git://github.com/example/shop.git",
            "  https://github.com/example/shop/  ",
            "https://x-access-token:tok@github.com/example/shop.git?tok=1#tok",
        ] {
            let got = normalise_remote_url(url);
            assert_eq!(got, shop, "{url}");
            assert!(!got.unwrap_or_default().contains("tok"), "userinfo leaked from {url}");
        }
        assert_eq!(
            normalise_remote_url("file:///srv/git/Shop.git").as_deref(),
            Some("srv/git/shop")
        );
        assert_eq!(normalise_remote_url("/srv/git/shop.git/").as_deref(), Some("srv/git/shop"));
        assert_eq!(
            normalise_remote_url("git@gitlab.example.com:group/sub/proj.git").as_deref(),
            Some("gitlab.example.com/group/sub/proj")
        );
        assert_eq!(normalise_remote_url(""), None);
        assert_eq!(normalise_remote_url("   "), None);
    }

    #[test]
    fn no_git_dir_is_its_basename() {
        let root = tmp("ident_dir");
        let app = root.join("app");
        std::fs::create_dir_all(app.join("x")).unwrap();
        let id = repo_identity(&app);
        assert_eq!(id, RepoIdentity { key: "dir:app".into(), source: IdentitySource::Directory });
        assert_eq!(id.source.as_str(), "dir");
        // Spelling does not matter: `.` and `x/..` resolve to the same dir.
        assert_eq!(repo_identity(&app.join(".")), id);
        assert_eq!(repo_identity(&app.join("x").join("..")), id);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn worktree_and_main_checkout_share_a_key() {
        let root = tmp("ident_wt");
        let main = root.join("main");
        let wt2 = root.join("wt2");
        write(&main, ".git/config", SHOP_CONFIG);
        write(&main, ".git/worktrees/wt2/commondir", "../..\n");
        write(
            &wt2,
            ".git",
            &format!("gitdir: {}\n", main.join(".git/worktrees/wt2").display()),
        );
        let want = RepoIdentity {
            key: "git:github.com/example/shop".into(),
            source: IdentitySource::GitRemote,
        };
        assert_eq!(repo_identity(&main), want);
        assert_eq!(repo_identity(&wt2), want);
        assert_eq!(want.source.as_str(), "git-remote");
        // A relative gitdir resolves against the `.git` file's own directory.
        write(&wt2, ".git", "gitdir: ../main/.git/worktrees/wt2\n");
        assert_eq!(repo_identity(&wt2), want);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn subdir_of_a_checkout_keeps_its_path_within() {
        let root = tmp("ident_sub");
        let top = root.join("shop");
        write(&top, ".git/config", SHOP_CONFIG);
        std::fs::create_dir_all(top.join("services/api")).unwrap();
        assert_eq!(
            repo_identity(&top.join("services/api")).key,
            "git:github.com/example/shop/services/api"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn submodule_git_file_reads_its_own_config() {
        let root = tmp("ident_submod");
        let sup = root.join("super");
        write(&sup, ".git/config", SHOP_CONFIG);
        write(
            &sup,
            ".git/modules/sdk/config",
            "[remote \"origin\"]\n\turl = https://github.com/Example/SDK.git\n",
        );
        let sdk = sup.join("vendor/sdk");
        write(&sdk, ".git", "gitdir: ../../.git/modules/sdk\n");
        assert_eq!(repo_identity(&sdk).key, "git:github.com/example/sdk");
        // No remote in the submodule's config: its own git dir names it, not
        // the `modules` directory that holds every submodule.
        write(&sup, ".git/modules/sdk/config", "[core]\n\tbare = false\n");
        assert_eq!(
            repo_identity(&sdk),
            RepoIdentity { key: "gitdir:sdk".into(), source: IdentitySource::GitLocal }
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn remote_choice_and_the_git_local_fallback() {
        let root = tmp("ident_local");
        let repo = root.join("tool");
        // origin wins over an earlier remote; `pushurl` is not `url`.
        write(
            &repo,
            ".git/config",
            "[remote \"upstream\"]\n\turl = https://github.com/up/tool\n[remote \"origin\"]\n\tpushurl = https://github.com/push/tool\n\turl = \"https://github.com/me/tool.git\" # mine\n",
        );
        assert_eq!(repo_identity(&repo).key, "git:github.com/me/tool");
        // No origin: the first remote in file order.
        write(
            &repo,
            ".git/config",
            "[remote.upstream]\n\turl = https://github.com/up/tool\n[remote \"fork\"]\n\turl = https://github.com/fork/tool\n",
        );
        assert_eq!(repo_identity(&repo).key, "git:github.com/up/tool");
        // No remote at all: the checkout's directory name, from the common dir.
        write(&repo, ".git/config", "[core]\n\tbare = false\n");
        std::fs::create_dir_all(repo.join("pkg")).unwrap();
        assert_eq!(
            repo_identity(&repo),
            RepoIdentity { key: "gitdir:tool".into(), source: IdentitySource::GitLocal }
        );
        assert_eq!(repo_identity(&repo.join("pkg")).key, "gitdir:tool/pkg");
        assert_eq!(IdentitySource::GitLocal.as_str(), "git-local");
        let _ = std::fs::remove_dir_all(&root);
    }
}
