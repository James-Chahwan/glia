//! CB.22: C/C++ include search paths.
//!
//! A real C/C++ tree keeps its public headers under `include/` and includes
//! them from `src/` as `<proj/x.hpp>` or `"proj/x.hpp"`, relying on `-I` from
//! the build. [`IncludeRoots::read`] gathers one repo's search directories, in
//! this order, each kept once (first position wins):
//!
//! 1. `compile_commands.json` at the repo root, in its depth-1 `build*/`,
//!    `out/` and `cmake-build-*/` dirs, and the same at every CMake project
//!    root: read from disk, never through the walk (the build dir is normally
//!    gitignored, so the walk skips it, as a tsconfig is read off the project
//!    dir, `TsAliasSet::read`). Every `-I<dir>` / `-I <dir>` / `-isystem` /
//!    `-iquote` of every entry, resolved against the entry's `directory`, in
//!    file order. A malformed file is one `warning:` line and is skipped.
//! 2. `include_directories(..)` / `target_include_directories(..)` of the
//!    `CMakeLists.txt` in every directory that holds a walked C/C++ file (the
//!    walk admits no `CMakeLists.txt`, so it is read beside the walked files).
//! 3. conventional: `include/` under the repo root and under each project
//!    root.
//!
//! Only a directory inside the repo that holds a walked C/C++ file (at any
//! depth) is kept, so a stale or out-of-tree entry costs nothing, and no path
//! climbs out of the repo root.
//!
//! [`IncludeResolver`] is the `build_c_cpp` import resolver over them: a
//! quoted include tries the includer's directory first
//! ([`super::lang_build::resolve_include_source`]), then each search
//! directory; an angle include (the parser keeps its brackets) tries the
//! search directories only. A pure function of (dirs, MODULE qnames), so the
//! build stays deterministic. Consumed after the parse cache, like the
//! tsconfig aliases, so the cache needs no key.

use std::cell::Cell;
use std::collections::{BTreeSet, HashSet};
use std::path::{Component, Path, PathBuf};

use glia_code_domain::project_roots::ProjectRoot;
use glia_code_domain::{FileParse, ImportTarget, node_kind};

use crate::extract::detect_language;

use super::lang_build::resolve_include_source;

/// The compile database's file name.
const COMPILE_COMMANDS: &str = "compile_commands.json";
/// The CMake list file read per walked directory.
const CMAKE_LISTS: &str = "CMakeLists.txt";
/// The CMake variables that name the list file's own directory.
const CMAKE_CURRENT_DIR_VARS: [&str; 2] =
    ["${CMAKE_CURRENT_SOURCE_DIR}", "${CMAKE_CURRENT_LIST_DIR}"];
/// The CMake variables read as the repo root.
const CMAKE_ROOT_DIR_VARS: [&str; 2] = ["${PROJECT_SOURCE_DIR}", "${CMAKE_SOURCE_DIR}"];

/// One repo's C/C++ include search directories and where they came from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct IncludeRoots {
    /// Repo-relative, `/`-separated, `""` for the repo root; search order.
    dirs: Vec<String>,
    /// Distinct kept dirs each source named, before the cross-source dedup.
    compile_commands: usize,
    cmake: usize,
    conventional: usize,
    /// The `warning:` lines `read` printed (a malformed compile database).
    warnings: Vec<String>,
}

impl IncludeRoots {
    /// Gather the search directories of the repo at `root` from its walked
    /// `files` (`(repo-relative path, text)`) and project `roots`. A repo
    /// with no C/C++ file reads nothing and gets none.
    ///
    /// Prints one line per malformed compile database:
    ///   `[c-includes] warning: skipped malformed compile_commands.json file=<path> repo=<label>: <error>`
    pub(super) fn read(
        root: &Path,
        files: &[(String, String)],
        roots: &[ProjectRoot],
        repo_label: &str,
    ) -> Self {
        let c_files: Vec<&str> = files
            .iter()
            .map(|(p, _)| p.as_str())
            .filter(|p| detect_language(p) == Some("c_cpp"))
            .collect();
        let mut out = Self::default();
        if c_files.is_empty() {
            return out;
        }
        let held = HeldDirs::of(&c_files);
        let forms = RootForms::of(root);
        let mut seen: HashSet<String> = HashSet::new();

        let from_db = compile_commands_dirs(root, roots, &forms, &mut out.warnings, repo_label);
        out.compile_commands = out.keep(from_db, &held, &mut seen);

        let mut from_cmake = Vec::new();
        for dir in &held.order {
            let Ok(text) = std::fs::read_to_string(root.join(dir).join(CMAKE_LISTS)) else {
                continue;
            };
            for arg in cmake_include_args(&text) {
                if let Some(d) = cmake_dir(&arg, dir, &forms) {
                    from_cmake.push(d);
                }
            }
        }
        out.cmake = out.keep(from_cmake, &held, &mut seen);

        let conventional = std::iter::once("")
            .chain(roots.iter().map(|r| r.rel_path.as_str()))
            .filter_map(|dir| join_rel(dir, "include"))
            .collect();
        out.conventional = out.keep(conventional, &held, &mut seen);

        for w in &out.warnings {
            eprintln!("{w}");
        }
        out
    }

    /// Append `found`'s held dirs not yet searched; returns how many distinct
    /// held dirs `found` named (the per-source marker count).
    fn keep(&mut self, found: Vec<String>, held: &HeldDirs, seen: &mut HashSet<String>) -> usize {
        let mut named: HashSet<&str> = HashSet::new();
        for dir in &found {
            if !held.contains(dir) || !named.insert(dir.as_str()) {
                continue;
            }
            if seen.insert(dir.clone()) {
                self.dirs.push(dir.clone());
            }
        }
        named.len()
    }

    /// The `build_c_cpp` resolver over these dirs and the repo's C/C++
    /// MODULE qnames ([`c_cpp_module_qnames`]).
    pub(super) fn resolver<'a>(&'a self, modules: &'a BTreeSet<String>) -> IncludeResolver<'a> {
        IncludeResolver {
            dirs: &self.dirs,
            modules,
            via_roots: Cell::new(0),
        }
    }

    /// CB.22 fired_on marker, once per repo with C/C++ files, after its build:
    ///   `[c-includes] search roots=<n> (compile_commands=<a> cmake=<b> conventional=<c>) angle=<g> bound_via_roots=<r> repo=<label>`
    /// `n` = distinct search dirs, `a` / `b` / `c` = the dirs each source
    /// named (one dir two sources name counts in both), `g` = angle includes
    /// in the parses, `r` = includes bound through a search dir rather than
    /// the includer's own.
    pub(super) fn marker(&self, angle: usize, bound_via_roots: usize, repo_label: &str) -> String {
        format!(
            "[c-includes] search roots={} (compile_commands={} cmake={} conventional={}) angle={angle} bound_via_roots={bound_via_roots} repo={repo_label}",
            self.dirs.len(),
            self.compile_commands,
            self.cmake,
            self.conventional
        )
    }
}

/// The `build_c_cpp` import resolver ([`IncludeRoots::resolver`]). Used on
/// the one thread that runs the C/C++ build, so its count is a `Cell`.
pub(super) struct IncludeResolver<'a> {
    dirs: &'a [String],
    modules: &'a BTreeSet<String>,
    via_roots: Cell<usize>,
}

impl IncludeResolver<'_> {
    /// The MODULE qname `spec` (as the parser recorded it) names from
    /// `from_module`. Angle (`<shop/cart.hpp>`): the first search dir holding
    /// it, else None (`<vector>`). Quoted: the includer-relative MODULE when
    /// it exists, else the first search dir holding it, else the
    /// includer-relative qname, handed to the graph exactly as before CB.22.
    pub(super) fn resolve(&self, from_module: &str, spec: &str) -> Option<String> {
        let spec = spec.trim();
        if let Some(angle) = spec.strip_prefix('<').and_then(|s| s.strip_suffix('>')) {
            return self.through_roots(angle.trim());
        }
        let beside = resolve_include_source(from_module, spec);
        if beside.as_ref().is_some_and(|q| self.modules.contains(q)) {
            return beside;
        }
        self.through_roots(spec.trim_matches('"')).or(beside)
    }

    /// The first search dir + `spec` that is a C/C++ MODULE (every C/C++
    /// MODULE is named by its path, LB.10a).
    fn through_roots(&self, spec: &str) -> Option<String> {
        if spec.is_empty() || spec.starts_with(['/', '\\']) {
            return None;
        }
        let hit = self.dirs.iter().find_map(|dir| {
            let qname = join_rel(dir, spec)?.replace('/', "::");
            self.modules.contains(&qname).then_some(qname)
        })?;
        self.via_roots.set(self.via_roots.get() + 1);
        Some(hit)
    }

    /// Includes bound through a search dir so far.
    pub(super) fn bound_via_roots(&self) -> usize {
        self.via_roots.get()
    }
}

/// Every C/C++ MODULE qname of `parses`, the targets an include can bind.
pub(super) fn c_cpp_module_qnames(parses: &[FileParse]) -> BTreeSet<String> {
    parses
        .iter()
        .flat_map(|fp| {
            fp.nav
                .kind_by_id
                .iter()
                .filter(|(_, k)| **k == node_kind::MODULE)
                .filter_map(|(id, _)| fp.nav.qname_by_id.get(id).cloned())
        })
        .collect()
}

/// Angle includes (`<...>`) among `parses`' imports, for the marker.
pub(super) fn angle_includes(parses: &[FileParse]) -> usize {
    parses
        .iter()
        .flat_map(|fp| &fp.imports)
        .filter(|i| matches!(&i.target, ImportTarget::Module { path, .. } if path.starts_with('<')))
        .count()
}

/// The directories that hold a walked C/C++ file, at any depth: every
/// ancestor dir of one (`""` is the repo root), and the same set in the order
/// the walk first reaches it (files are in walk order, each file's ancestors
/// root first).
struct HeldDirs {
    set: HashSet<String>,
    order: Vec<String>,
}

impl HeldDirs {
    fn of(files: &[&str]) -> Self {
        let mut set = HashSet::new();
        let mut order = Vec::new();
        for file in files {
            let file = file.replace('\\', "/");
            let mut dir = String::new();
            let mut parts: Vec<&str> = file.split('/').filter(|s| !s.is_empty()).collect();
            parts.pop(); // the file itself
            for (i, part) in std::iter::once("").chain(parts).enumerate() {
                if i > 1 {
                    dir.push('/');
                }
                dir.push_str(part);
                if set.insert(dir.clone()) {
                    order.push(dir.clone());
                }
            }
        }
        Self { set, order }
    }

    fn contains(&self, dir: &str) -> bool {
        self.set.contains(dir)
    }
}

/// `base` (repo-relative, `/`-separated) joined with `rel` (`/` or `\`),
/// `.` and `..` folded; None when it climbs above the repo root.
fn join_rel(base: &str, rel: &str) -> Option<String> {
    let mut segs: Vec<&str> = base.split('/').filter(|s| !s.is_empty()).collect();
    for part in rel.split(['/', '\\']) {
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

/// The repo root as an absolute path, canonical and as given, for turning an
/// absolute path from a compile database or a CMake list into a
/// repo-relative one.
struct RootForms(Vec<PathBuf>);

impl RootForms {
    fn of(root: &Path) -> Self {
        let mut forms = Vec::new();
        for form in [
            std::fs::canonicalize(root).ok(),
            std::path::absolute(root).ok(),
        ]
        .into_iter()
        .flatten()
        .filter_map(|p| fold_dots(&p))
        {
            if !forms.contains(&form) {
                forms.push(form);
            }
        }
        Self(forms)
    }

    /// `abs` as a repo-relative `/`-separated dir, or None outside the repo:
    /// lexically first, then through its canonical form (a symlinked prefix).
    fn repo_relative(&self, abs: &Path) -> Option<String> {
        let lexical = fold_dots(abs)?;
        self.strip(&lexical)
            .or_else(|| self.strip(&std::fs::canonicalize(&lexical).ok()?))
    }

    fn strip(&self, path: &Path) -> Option<String> {
        let rest = self.0.iter().find_map(|r| path.strip_prefix(r).ok())?;
        let mut segs = Vec::new();
        for c in rest.components() {
            match c {
                Component::Normal(s) => segs.push(s.to_str()?),
                _ => return None,
            }
        }
        Some(segs.join("/"))
    }
}

/// An absolute path with `.` and `..` folded lexically; None for a relative
/// one.
fn fold_dots(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// compile_commands.json
// ---------------------------------------------------------------------------

/// The compile databases of the repo root and of every CMake project root:
/// `<dir>/compile_commands.json`, then the same in each depth-1 `build*`,
/// `out` and `cmake-build-*` dir, by name. Returns their include dirs,
/// repo-relative, in file order.
fn compile_commands_dirs(
    root: &Path,
    roots: &[ProjectRoot],
    forms: &RootForms,
    warnings: &mut Vec<String>,
    repo_label: &str,
) -> Vec<String> {
    let project_dirs: BTreeSet<&str> = roots
        .iter()
        .filter(|r| r.ecosystem == "cmake")
        .map(|r| r.rel_path.as_str())
        .collect();
    let mut dbs: Vec<PathBuf> = Vec::new();
    for dir in std::iter::once("").chain(project_dirs.into_iter().filter(|d| !d.is_empty())) {
        let base = root.join(dir);
        dbs.push(base.join(COMPILE_COMMANDS));
        let mut build_dirs: Vec<PathBuf> = std::fs::read_dir(&base)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter(|e| {
                e.file_name().to_str().is_some_and(|n| {
                    n.starts_with("build") || n == "out" || n.starts_with("cmake-build-")
                })
            })
            .map(|e| e.path())
            .collect();
        build_dirs.sort();
        dbs.extend(build_dirs.into_iter().map(|d| d.join(COMPILE_COMMANDS)));
    }
    let mut out = Vec::new();
    for db in dbs {
        if !db.is_file() {
            continue;
        }
        match read_compile_commands(&db) {
            Ok(dirs) => out.extend(dirs.iter().filter_map(|d| forms.repo_relative(d))),
            Err(e) => warnings.push(format!(
                "[c-includes] warning: skipped malformed {COMPILE_COMMANDS} file={} repo={repo_label}: {e}",
                db.display()
            )),
        }
    }
    out
}

/// One compile database entry: only the fields the include flags need
/// (`file` and `output` are ignored).
#[derive(serde::Deserialize)]
struct CompileEntry {
    #[serde(default)]
    directory: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    arguments: Option<Vec<String>>,
}

/// Streams the top-level array one entry at a time, so a large database is
/// never held as a whole.
struct EntryVisitor<F>(F);

impl<'de, F: FnMut(CompileEntry)> serde::de::Visitor<'de> for EntryVisitor<F> {
    type Value = ();

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a JSON array of compile commands")
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(mut self, mut seq: A) -> Result<(), A::Error> {
        while let Some(entry) = seq.next_element::<CompileEntry>()? {
            (self.0)(entry);
        }
        Ok(())
    }
}

/// The include dirs of one compile database, absolute and in file order,
/// each once; `Err` for a file that is not a JSON array of entries (nothing
/// of it is used then).
fn read_compile_commands(db: &Path) -> Result<Vec<PathBuf>, String> {
    use serde::Deserializer as _;
    let file = std::fs::File::open(db).map_err(|e| e.to_string())?;
    // Absolute, so a relative `directory` still resolves when the repo path
    // was given relative.
    let db_dir = std::path::absolute(db)
        .map_err(|e| e.to_string())?
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut de = serde_json::Deserializer::from_reader(std::io::BufReader::new(file));
    de.deserialize_seq(EntryVisitor(|entry: CompileEntry| {
        let directory = match entry.directory.as_deref().map(Path::new) {
            Some(d) if d.is_absolute() => d.to_path_buf(),
            Some(d) => db_dir.join(d),
            None => db_dir.clone(),
        };
        let args = match (entry.arguments, entry.command) {
            (Some(args), _) => args,
            (None, Some(cmd)) => shell_words(&cmd),
            (None, None) => Vec::new(),
        };
        for flag in include_flags(&args) {
            let dir = directory.join(flag);
            if seen.insert(dir.clone()) {
                dirs.push(dir);
            }
        }
    }))
    .map_err(|e| e.to_string())?;
    de.end().map_err(|e| e.to_string())?;
    Ok(dirs)
}

/// The directory of every `-I<dir>`, `-I <dir>`, `-isystem[ ]<dir>` and
/// `-iquote[ ]<dir>` in `args`, in order. A sysroot-relative `=dir` and the
/// obsolete `-I-` name no directory.
fn include_flags(args: &[String]) -> Vec<&str> {
    let mut out = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let Some(rest) = ["-isystem", "-iquote", "-I"]
            .iter()
            .find_map(|flag| arg.strip_prefix(flag))
        else {
            continue;
        };
        let value = if rest.is_empty() {
            it.next().map(String::as_str)
        } else {
            Some(rest)
        };
        if let Some(v) = value.map(str::trim)
            && !v.is_empty()
            && v != "-"
            && !v.starts_with('=')
        {
            out.push(v);
        }
    }
    out
}

/// A compile database `command` split as a POSIX shell would: whitespace
/// separates words, single quotes are literal, double quotes and a backslash
/// escape.
fn shell_words(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut chars = cmd.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (Some('"'), '\\') => match chars.next() {
                Some(n @ ('"' | '\\' | '$' | '`')) => cur.push(n),
                Some(n) => {
                    cur.push('\\');
                    cur.push(n);
                }
                None => cur.push('\\'),
            },
            (Some(_), c) => cur.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                in_word = true;
            }
            (None, '\\') => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
                in_word = true;
            }
            (None, c) if c.is_whitespace() => {
                if in_word {
                    out.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            (None, c) => {
                cur.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        out.push(cur);
    }
    out
}

// ---------------------------------------------------------------------------
// CMakeLists.txt
// ---------------------------------------------------------------------------

/// One CMake list token: a command name or unquoted argument, a quoted or
/// bracket argument, or a paren.
#[derive(Debug, PartialEq, Eq)]
enum CmakeTok {
    Word(String),
    Quoted(String),
    Open,
    Close,
}

/// The directory arguments of every `include_directories(..)` and
/// `target_include_directories(..)` in a CMake list, in order: the
/// `AFTER` / `BEFORE` / `SYSTEM` / scope keywords and the target name
/// dropped, variables and generator expressions still unexpanded.
fn cmake_include_args(text: &str) -> Vec<String> {
    let toks = cmake_tokens(text);
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let command = match &toks[i] {
            CmakeTok::Word(w) if toks.get(i + 1) == Some(&CmakeTok::Open) => w.to_ascii_lowercase(),
            _ => {
                i += 1;
                continue;
            }
        };
        // The command's arguments: every token up to its matching `)`.
        let mut depth = 0usize;
        let mut args: Vec<&str> = Vec::new();
        let mut j = i + 1;
        while j < toks.len() {
            match &toks[j] {
                CmakeTok::Open => depth += 1,
                CmakeTok::Close => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                CmakeTok::Word(w) | CmakeTok::Quoted(w) if depth == 1 => args.push(w),
                _ => {}
            }
            j += 1;
        }
        let dirs: &[&str] = match command.as_str() {
            "include_directories" => &args,
            "target_include_directories" => args.get(1..).unwrap_or_default(),
            _ => &[],
        };
        out.extend(
            dirs.iter()
                .filter(|a| {
                    !matches!(
                        **a,
                        "AFTER" | "BEFORE" | "SYSTEM" | "PUBLIC" | "PRIVATE" | "INTERFACE"
                    )
                })
                .map(|a| a.to_string()),
        );
        i = j + 1;
    }
    out
}

/// Tokenise a CMake list: `#` line and `#[[..]]` bracket comments dropped,
/// `"..."` and `[[..]]` arguments one token each.
fn cmake_tokens(text: &str) -> Vec<CmakeTok> {
    let mut toks = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            c if c.is_whitespace() => {
                chars.next();
            }
            '#' => {
                chars.next();
                let mut probe = chars.clone();
                if chars.peek() == Some(&'[') && bracket_body(&mut probe).is_some() {
                    chars = probe;
                    continue;
                }
                for n in chars.by_ref() {
                    if n == '\n' {
                        break;
                    }
                }
            }
            '(' => {
                chars.next();
                toks.push(CmakeTok::Open);
            }
            ')' => {
                chars.next();
                toks.push(CmakeTok::Close);
            }
            '"' => {
                chars.next();
                let mut s = String::new();
                while let Some(n) = chars.next() {
                    match n {
                        '"' => break,
                        '\\' => {
                            if let Some(e) = chars.next() {
                                s.push(e);
                            }
                        }
                        n => s.push(n),
                    }
                }
                toks.push(CmakeTok::Quoted(s));
            }
            '[' => {
                let mut probe = chars.clone();
                match bracket_body(&mut probe) {
                    Some(body) => {
                        chars = probe;
                        toks.push(CmakeTok::Quoted(body));
                    }
                    None => toks.push(CmakeTok::Word(unquoted(&mut chars))),
                }
            }
            _ => toks.push(CmakeTok::Word(unquoted(&mut chars))),
        }
    }
    toks
}

/// An unquoted CMake argument: up to whitespace, a paren, `#` or `"`.
fn unquoted(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut s = String::new();
    while let Some(&n) = chars.peek() {
        if n.is_whitespace() || matches!(n, '(' | ')' | '#' | '"') {
            break;
        }
        s.push(n);
        chars.next();
    }
    s
}

/// A bracket body `[=*[ .. ]=*]` starting at the iterator's `[`: consumes it
/// and returns the body, or None (iterator state then unspecified) when the
/// `[` opens no bracket.
fn bracket_body(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<String> {
    if chars.next() != Some('[') {
        return None;
    }
    let mut level = 0usize;
    while chars.peek() == Some(&'=') {
        chars.next();
        level += 1;
    }
    if chars.next() != Some('[') {
        return None;
    }
    let close = format!("]{}]", "=".repeat(level));
    let mut body = String::new();
    for n in chars.by_ref() {
        body.push(n);
        if body.ends_with(&close) {
            body.truncate(body.len() - close.len());
            return Some(body);
        }
    }
    Some(body)
}

/// One CMake include argument of the list in repo dir `list_dir`, as a
/// repo-relative dir: relative to `list_dir` (CMake's rule), after
/// `${CMAKE_CURRENT_SOURCE_DIR}` / `${CMAKE_CURRENT_LIST_DIR}` (the list's
/// dir) or `${PROJECT_SOURCE_DIR}` / `${CMAKE_SOURCE_DIR}` (the repo root)
/// as its prefix. A generator expression (`$<..>`), any other variable, or a
/// path outside the repo gives None.
fn cmake_dir(arg: &str, list_dir: &str, forms: &RootForms) -> Option<String> {
    let arg = arg.trim();
    if arg.is_empty() || arg.contains("$<") {
        return None;
    }
    let prefixed = CMAKE_CURRENT_DIR_VARS
        .iter()
        .find_map(|v| arg.strip_prefix(v).map(|rest| (list_dir, rest)))
        .or_else(|| {
            CMAKE_ROOT_DIR_VARS
                .iter()
                .find_map(|v| arg.strip_prefix(v).map(|rest| ("", rest)))
        });
    let (base, rest) = match prefixed {
        Some((base, rest)) if rest.is_empty() || rest.starts_with(['/', '\\']) => (base, rest),
        Some(_) => return None,
        None if Path::new(arg).is_absolute() => return forms.repo_relative(Path::new(arg)),
        None => (list_dir, arg),
    };
    if rest.contains('$') {
        return None;
    }
    join_rel(base, rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    use glia_code_domain::ImportStmt;
    use glia_core::NodeId;

    /// A C/C++ parse holding one MODULE `qname` and the given include paths.
    fn parse(qname: &str, includes: &[&str]) -> FileParse {
        let mut fp = FileParse::default();
        let id = NodeId(1);
        fp.nav.kind_by_id.insert(id, node_kind::MODULE);
        fp.nav.qname_by_id.insert(id, qname.to_string());
        fp.imports = includes
            .iter()
            .map(|p| ImportStmt {
                from_module: qname.to_string(),
                target: ImportTarget::Module {
                    path: p.to_string(),
                    alias: None,
                },
                line: 0,
            })
            .collect();
        fp
    }

    fn write(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, text).expect("write");
    }

    /// The walked files of `rels`, as the walk hands them over.
    fn walked(rels: &[&str]) -> Vec<(String, String)> {
        rels.iter()
            .map(|r| (r.to_string(), String::new()))
            .collect()
    }

    fn cmake_root(rel: &str) -> ProjectRoot {
        ProjectRoot::new(rel.to_string(), "cmake", CMAKE_LISTS, None)
    }

    fn roots_of(dirs: &[&str]) -> IncludeRoots {
        IncludeRoots {
            dirs: dirs.iter().map(|d| d.to_string()).collect(),
            ..IncludeRoots::default()
        }
    }

    fn modules(qnames: &[&str]) -> BTreeSet<String> {
        qnames.iter().map(|q| q.to_string()).collect()
    }

    /// Both `-I` forms, `-isystem`, `-iquote`, `arguments` and `command`,
    /// each relative to its entry's `directory`; a dir outside the repo and a
    /// dir holding no walked file are dropped.
    #[test]
    fn compile_commands_flags() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let abs = std::fs::canonicalize(root).expect("canonical root");
        let build = abs.join("build");
        let db = format!(
            r#"[
  {{"directory": "{b}", "file": "../src/a.cpp", "command": "c++ -I../include -I ../third/api \"-isystem\" ../sys -DX=1 -c ../src/a.cpp"}},
  {{"directory": "{b}", "file": "../src/b.cpp", "arguments": ["c++", "-iquote", "../quoted", "-I/usr/include", "-I../include", "-I../missing", "-c", "../src/b.cpp"]}},
  {{"directory": "{r}", "file": "src/c.cpp", "arguments": ["c++", "-Igen", "-c", "src/c.cpp"]}}
]"#,
            b = build.display(),
            r = abs.display()
        );
        write(root, "build/compile_commands.json", &db);
        let files = walked(&[
            "include/p/a.h",
            "third/api/b.h",
            "sys/c.h",
            "quoted/d.h",
            "gen/e.h",
            "src/a.cpp",
        ]);
        let got = IncludeRoots::read(root, &files, &[], "repo");
        assert_eq!(got.dirs, ["include", "third/api", "sys", "quoted", "gen"]);
        assert_eq!(
            (got.compile_commands, got.cmake, got.conventional),
            (5, 0, 1)
        );
        assert!(got.warnings.is_empty(), "{:?}", got.warnings);
    }

    /// `target_include_directories(<t> [SYSTEM] [BEFORE] PUBLIC|PRIVATE|INTERFACE ..)`
    /// and `include_directories(..)`: plain relative dirs are the list's,
    /// `${CMAKE_CURRENT_SOURCE_DIR}` is the list's dir, `${PROJECT_SOURCE_DIR}`
    /// the repo root; a generator expression and another variable are skipped.
    #[test]
    fn cmake_target_include_directories() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        write(
            root,
            "lib/CMakeLists.txt",
            "# include_directories(commented)\nadd_library(core src/core.cpp)\n\
             target_include_directories(core SYSTEM BEFORE PUBLIC include\n  PRIVATE \"${CMAKE_CURRENT_SOURCE_DIR}/api\"\n  INTERFACE $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/gen> ${OTHER_DIR}/x)\n\
             INCLUDE_DIRECTORIES(AFTER ${PROJECT_SOURCE_DIR}/common)\n",
        );
        let files = walked(&[
            "common/c.h",
            "lib/api/a.h",
            "lib/gen/g.h",
            "lib/include/core.h",
            "lib/src/core.cpp",
        ]);
        let got = IncludeRoots::read(root, &files, &[], "repo");
        assert_eq!(got.dirs, ["lib/include", "lib/api", "common"]);
        assert_eq!(
            (got.compile_commands, got.cmake, got.conventional),
            (0, 3, 0)
        );
        assert_eq!(
            cmake_include_args(
                "target_include_directories(t PUBLIC $<INSTALL_INTERFACE:include> a)"
            ),
            ["$<INSTALL_INTERFACE:include>", "a"]
        );
        assert_eq!(
            cmake_dir("$<INSTALL_INTERFACE:include>", "", &RootForms(Vec::new())),
            None
        );
    }

    /// `include/` under the repo root and under a project root, only when a
    /// walked file lives there.
    #[test]
    fn conventional_include_root() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let files = walked(&[
            "include/shop/cart.hpp",
            "engine/include/e.h",
            "src/cart.cpp",
        ]);
        let roots = [cmake_root("engine"), cmake_root("tools")];
        let got = IncludeRoots::read(tmp.path(), &files, &roots, "repo");
        assert_eq!(got.dirs, ["include", "engine/include"]);
        assert_eq!(got.conventional, 2);
        // No C/C++ file: nothing is read, nothing is searched.
        let none = IncludeRoots::read(tmp.path(), &walked(&["include/x.py"]), &roots, "repo");
        assert_eq!(none, IncludeRoots::default());
    }

    /// Sources in order compile_commands, CMake, conventional; a dir two
    /// sources name keeps its first position and counts in both.
    #[test]
    fn order_and_dedup() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let abs = std::fs::canonicalize(root).expect("canonical root");
        write(
            root,
            "compile_commands.json",
            &format!(
                r#"[{{"directory": "{r}", "arguments": ["cc", "-Iapi", "-Iinclude", "-Iapi"]}}]"#,
                r = abs.display()
            ),
        );
        write(
            root,
            "CMakeLists.txt",
            "include_directories(include vendor api)\n",
        );
        let files = walked(&["api/a.h", "include/i.h", "vendor/v.h", "main.c"]);
        let got = IncludeRoots::read(root, &files, &[], "repo");
        assert_eq!(got.dirs, ["api", "include", "vendor"]);
        assert_eq!(
            (got.compile_commands, got.cmake, got.conventional),
            (2, 3, 1)
        );
        assert_eq!(
            got.marker(4, 3, "r"),
            "[c-includes] search roots=3 (compile_commands=2 cmake=3 conventional=1) angle=4 bound_via_roots=3 repo=r"
        );
    }

    /// A quoted include binds the header beside the includer before a
    /// same-named one under a search root; one that misses the includer's
    /// dir falls back to the search root.
    #[test]
    fn quoted_prefers_includer_dir() {
        let roots = roots_of(&["include"]);
        let mods = modules(&[
            "src::util.hpp",
            "include::util.hpp",
            "include::shop::cart.hpp",
            "src::cart.cpp",
        ]);
        let r = roots.resolver(&mods);
        assert_eq!(
            r.resolve("src::cart.cpp", "util.hpp").as_deref(),
            Some("src::util.hpp")
        );
        assert_eq!(r.bound_via_roots(), 0);
        assert_eq!(
            r.resolve("src::cart.cpp", "shop/cart.hpp").as_deref(),
            Some("include::shop::cart.hpp")
        );
        assert_eq!(r.bound_via_roots(), 1);
        // Nowhere: the includer-relative qname, as before CB.22.
        assert_eq!(
            r.resolve("src::cart.cpp", "missing.h").as_deref(),
            Some("src::missing.h")
        );
        assert_eq!(r.bound_via_roots(), 1);
    }

    /// An angle include never binds the includer's own dir.
    #[test]
    fn angle_skips_includer_dir() {
        let mods = modules(&["src::util.hpp", "include::util.hpp", "src::cart.cpp"]);
        let roots = roots_of(&["include"]);
        let r = roots.resolver(&mods);
        assert_eq!(
            r.resolve("src::cart.cpp", "<util.hpp>").as_deref(),
            Some("include::util.hpp")
        );
        let bare = roots_of(&[]);
        let r = bare.resolver(&mods);
        assert_eq!(r.resolve("src::cart.cpp", "<util.hpp>"), None);
        assert_eq!(r.bound_via_roots(), 0);
        let parses = [parse(
            "src::cart.cpp",
            &["<util.hpp>", "util.hpp", "<vector>"],
        )];
        assert_eq!(angle_includes(&parses), 2);
        assert_eq!(c_cpp_module_qnames(&parses), modules(&["src::cart.cpp"]));
    }

    /// A system header no repo file names is unresolved; a search root never
    /// lets a path climb out of the repo.
    #[test]
    fn unknown_system_header_is_none() {
        let roots = roots_of(&["", "include"]);
        let mods = modules(&["include::shop::cart.hpp", "src::cart.cpp", "cart.hpp"]);
        let r = roots.resolver(&mods);
        for spec in [
            "<vector>",
            "<stdio.h>",
            "<../../cart.hpp>",
            "</usr/include/stdio.h>",
            "<>",
        ] {
            assert_eq!(r.resolve("src::cart.cpp", spec), None, "{spec}");
        }
        assert_eq!(
            r.resolve("src::cart.cpp", "<shop/cart.hpp>").as_deref(),
            Some("include::shop::cart.hpp")
        );
        assert_eq!(join_rel("include", "../../x.h"), None);
    }

    /// A compile database that is not a JSON array of entries is one
    /// `warning:` line and contributes nothing; the other sources still read.
    #[test]
    fn malformed_compile_commands_warns() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        write(
            root,
            "build/compile_commands.json",
            r#"[{"directory": "/x", "arguments": ["-Iinclude"]}, {"#,
        );
        write(root, "out/compile_commands.json", r#"{"not": "an array"}"#);
        let files = walked(&["include/a.h", "src/a.c"]);
        let got = IncludeRoots::read(root, &files, &[], "repo");
        assert_eq!(got.warnings.len(), 2, "{:?}", got.warnings);
        assert!(
            got.warnings.iter().all(|w| w.starts_with(
                "[c-includes] warning: skipped malformed compile_commands.json file="
            ) && w.contains(" repo=repo: ")),
            "{:?}",
            got.warnings
        );
        assert_eq!(got.compile_commands, 0);
        assert_eq!(got.dirs, ["include"], "the conventional root still reads");
        assert_eq!(
            shell_words(r#"cc -I"a b" '-Ic d' -I\ e"#),
            ["cc", "-Ia b", "-Ic d", "-I e"]
        );
    }
}
