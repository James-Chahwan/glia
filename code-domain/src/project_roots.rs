//! Manifest-rooted sub-projects inside one repo (A8.4).
//!
//! A repo can hold several independent projects — `apps/web` (npm),
//! `services/api` (go), `libs/core` (cargo) — and until now glia had no
//! vocabulary below `RepoId` for them. This module is that vocabulary: which
//! manifest roots a directory, and the human label it names.
//!
//! Detection rides the builder's walk (`engine::walk`), which hands [`detect_root`]
//! the directory's already-sorted entry names. That is what makes it correct as
//! well as cheap: the walker has already collapsed `node_modules`, build output
//! and nested repos to REGION anchors, so a vendored
//! `node_modules/left-pad/package.json` is never visited and can never become a
//! root. A separate directory walk would find hundreds of them.
//!
//! Emitting roots as `node_kind::PROJECT` nodes is A8.5; nothing here touches
//! the graph.

use std::collections::BTreeMap;
use std::path::Path;

/// One detected project root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectRoot {
    /// Repo-relative dir, `""` for the repo root itself.
    pub rel_path: String,
    /// Human label from the manifest, else the dir basename, else `"root"`.
    pub label: String,
    /// Which link of the fallback chain produced [`ProjectRoot::label`] (DD.4):
    /// [`LABEL_MANIFEST`], [`LABEL_DIR`], [`LABEL_ROOT`] or [`LABEL_DECLARED`].
    /// Only a [`LABEL_MANIFEST`] label is a name the manifest itself declared,
    /// so only it may be matched as the same thing as a registry package.
    pub label_source: &'static str,
    pub ecosystem: &'static str,
    /// Repo-relative path of the manifest that named it.
    pub manifest: String,
}

/// [`ProjectRoot::label_source`]: the manifest's own name (`package.json`
/// `name`, `go.mod` `module`, `Cargo.toml` `[package] name`, a .NET file stem).
pub const LABEL_MANIFEST: &str = "manifest";
/// [`ProjectRoot::label_source`]: no usable label, so the basename of the
/// repo-relative directory.
pub const LABEL_DIR: &str = "dir";
/// [`ProjectRoot::label_source`]: no usable label at the repo root, so `"root"`.
pub const LABEL_ROOT: &str = "root";
/// [`ProjectRoot::label_source`]: the label a `[[project]]` entry in
/// `.glia/overlay.toml` declared (LF.3a): on a declared root, or on a
/// detected root at the same path whose manifest names no project (a
/// [`LABEL_DIR`] or [`LABEL_ROOT`] label, the DE.9 hand-off taken by DG.12),
/// which keeps its ecosystem and manifest.
pub const LABEL_DECLARED: &str = "declared";

/// The tail of the label fallback chain [`ProjectRoot::new`] and
/// [`ProjectRoot::declared`] share: the basename of the repo-relative
/// `rel_path` ([`LABEL_DIR`]), else `"root"` ([`LABEL_ROOT`]).
fn fallback_label(rel_path: &str) -> (String, &'static str) {
    match rel_path.rsplit('/').next().and_then(clean_label) {
        Some(base) => (base, LABEL_DIR),
        None => ("root".to_string(), LABEL_ROOT),
    }
}

impl ProjectRoot {
    /// Build a root, applying the label fallback chain: manifest label → the
    /// basename of `rel_path` → `"root"`, recording which link produced it in
    /// [`ProjectRoot::label_source`]. The basename is taken from the
    /// REPO-RELATIVE path, never the checkout directory, so the repo root is
    /// always `"root"` wherever the repo happens to be cloned.
    pub fn new(
        rel_path: String,
        ecosystem: &'static str,
        manifest_basename: &str,
        label: Option<String>,
    ) -> Self {
        let (label, label_source) = match label {
            Some(l) => (l, LABEL_MANIFEST),
            None => fallback_label(&rel_path),
        };
        let manifest = if rel_path.is_empty() {
            manifest_basename.to_string()
        } else {
            format!("{rel_path}/{manifest_basename}")
        };
        Self {
            rel_path,
            label,
            label_source,
            ecosystem,
            manifest,
        }
    }

    /// A `[[project]]` root declared in `.glia/overlay.toml` (LF.3a):
    /// ecosystem [`DECLARED_ECOSYSTEM`], manifest the overlay file itself.
    /// `rel_path` is the loader's normalised path (`ProjectDecl::rel_path`).
    /// The label falls back like [`ProjectRoot::new`]: the declared label when
    /// it passes the manifest-label rules (no `::`, `${` or control chars, at
    /// most 200 chars), else the basename of `rel_path`, else `"root"`; its
    /// source is [`LABEL_DECLARED`], [`LABEL_DIR`] or [`LABEL_ROOT`].
    pub fn declared(rel_path: String, label: Option<&str>) -> Self {
        let (label, label_source) = match label.and_then(clean_label) {
            Some(l) => (l, LABEL_DECLARED),
            None => fallback_label(&rel_path),
        };
        Self {
            rel_path,
            label,
            label_source,
            ecosystem: DECLARED_ECOSYSTEM,
            manifest: crate::glia_config::OVERLAY_FILE.to_string(),
        }
    }

    /// A sub-project a root `build.sbt` declares (DG.12): ecosystem
    /// [`SBT_ECOSYSTEM`], `manifest` the repo-relative path of the DECLARING
    /// `build.sbt` (the member's own directory usually holds none), and the
    /// label the sbt project id [`sbt_subprojects`] read. The id is a name the
    /// manifest declared, so its source is [`LABEL_MANIFEST`] (DD.4); one that
    /// breaks the label rules falls back like [`ProjectRoot::new`].
    pub fn sbt_member(rel_path: String, label: &str, manifest: String) -> Self {
        let (label, label_source) = match clean_label(label) {
            Some(l) => (l, LABEL_MANIFEST),
            None => fallback_label(&rel_path),
        };
        Self {
            rel_path,
            label,
            label_source,
            ecosystem: SBT_ECOSYSTEM,
            manifest,
        }
    }
}

/// The ecosystem of a `[[project]]` root declared in `.glia/overlay.toml`
/// (LF.3a) rather than found by a manifest.
pub const DECLARED_ECOSYSTEM: &str = "declared";

/// The ecosystem a `build.sbt` roots ([`MANIFESTS`]), and of every sub-project
/// one declares ([`ProjectRoot::sbt_member`], DG.12).
pub const SBT_ECOSYSTEM: &str = "sbt";

/// The sbt build definition's basename, whose `val`s [`sbt_subprojects`] reads.
pub const SBT_MANIFEST: &str = "build.sbt";

/// Manifest basename → ecosystem, in precedence order. The first entry present
/// (and confirmed, see [`confirms`]) decides the ecosystem; ties never happen
/// because the table is scanned in order.
///
/// The last two rows (DE.9) are the weakest evidence of all: a
/// `requirements.txt` or `Pipfile` roots a python project only when its
/// directory also holds a `.py` file (the gate in `candidates`), and any other
/// manifest in the same directory decides first, so a `requirements.txt` kept
/// for tooling beside a `pom.xml` or `package.json` never re-labels that
/// project. Neither names the project ([`manifest_label`] gives `None`), so the
/// label is the directory basename.
pub const MANIFESTS: &[(&str, &str)] = &[
    ("package.json", "npm"),
    ("go.mod", "go"),
    ("Cargo.toml", "cargo"),
    ("pyproject.toml", "python"),
    ("setup.py", "python"),
    ("setup.cfg", "python"),
    ("pom.xml", "maven"),
    ("build.gradle", "gradle"),
    ("build.gradle.kts", "gradle"),
    ("pubspec.yaml", "dart"),
    ("mix.exs", "elixir"),
    ("composer.json", "php"),
    ("Gemfile", "ruby"),
    ("Package.swift", "swift"),
    ("CMakeLists.txt", "cmake"),
    (SBT_MANIFEST, SBT_ECOSYSTEM),
    ("deps.edn", "clojure"),
    ("project.clj", "clojure"),
    ("requirements.txt", "python"),
    ("Pipfile", "python"),
];

/// [`MANIFESTS`] rows that root a project only beside a `.py` file (DE.9): a
/// Sphinx `docs/requirements.txt` next to `.md` / `.rst` pages is a build pin,
/// not a service. The check reads the entry names the walker already holds,
/// never the manifest text, so [`detect`] and [`detect_root`] agree.
const PY_SOURCE_GATED: &[&str] = &["requirements.txt", "Pipfile"];

/// Whether `names` holds a Python source file: an entry whose extension is
/// exactly `py` (`Path` gives a bare `.py` dotfile no extension).
fn has_py_source(names: &[String]) -> bool {
    names
        .iter()
        .any(|n| Path::new(n.as_str()).extension().is_some_and(|e| e == "py"))
}

/// Extension-keyed manifests (.NET), checked after [`MANIFESTS`]. A solution
/// outranks a project file in the same directory.
pub const MANIFEST_EXTS: &[(&str, &str)] = &[
    ("sln", "dotnet"),
    ("csproj", "dotnet"),
    ("fsproj", "dotnet"),
    ("vbproj", "dotnet"),
];

/// Every manifest in one directory's entry names, in precedence order. A
/// `requirements.txt` / `Pipfile` is a candidate only beside a `.py` file
/// (DE.9); the gate is computed once per directory, from the names alone.
fn candidates(names: &[String]) -> impl Iterator<Item = (&'static str, &str)> {
    let py_source = has_py_source(names);
    let by_name = MANIFESTS
        .iter()
        .filter(move |(base, _)| py_source || !PY_SOURCE_GATED.contains(base))
        .filter_map(|(base, eco)| names.iter().find(|n| n == base).map(|n| (*eco, n.as_str())));
    let by_ext = MANIFEST_EXTS.iter().filter_map(|(ext, eco)| {
        names
            .iter()
            .find(|n| {
                let p = Path::new(n.as_str());
                p.file_stem().is_some_and(|s| !s.is_empty())
                    && p.extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| e.eq_ignore_ascii_case(ext))
            })
            .map(|n| (*eco, n.as_str()))
    });
    by_name.chain(by_ext)
}

/// Scan one directory's already-listed entry names. `names` MUST be the sorted
/// file names the walker collected, so detection is deterministic and costs no
/// extra `read_dir`. Returns `(ecosystem, manifest_basename)`, or `None` for an
/// ordinary source directory. Name-level only — see [`detect_root`] for the
/// content check the walker applies.
pub fn detect(names: &[String]) -> Option<(&'static str, String)> {
    candidates(names)
        .next()
        .map(|(eco, n)| (eco, n.to_string()))
}

/// Whether a manifest's text really roots a project. Two basenames are too
/// common to trust by name: every CMake subdirectory has a `CMakeLists.txt`
/// (only one with `project(` starts a project), and `setup.cfg` is often pure
/// flake8/pytest config. Everything else is accepted on its name.
pub fn confirms(manifest_basename: &str, text: &str) -> bool {
    match manifest_basename {
        "CMakeLists.txt" => text.to_ascii_lowercase().lines().any(|l| {
            l.trim_start()
                .strip_prefix("project")
                .is_some_and(|r| r.trim_start().starts_with('('))
        }),
        "setup.cfg" => text
            .lines()
            .map(str::trim)
            .any(|l| l == "[metadata]" || l == "[options]"),
        _ => true,
    }
}

/// The walker's entry point: the highest-precedence confirmed manifest among
/// `names`, as `(ecosystem, manifest_basename, label)`. `read` returns a
/// manifest's text by basename (empty on error); it is only called for
/// candidates, so an ordinary directory costs no I/O.
pub fn detect_root(
    names: &[String],
    mut read: impl FnMut(&str) -> String,
) -> Option<(&'static str, String, Option<String>)> {
    for (eco, base) in candidates(names) {
        // .NET manifests (the extension-keyed ones) are labelled by file stem;
        // their text is never read.
        if !MANIFESTS.iter().any(|(b, _)| *b == base) {
            return Some((eco, base.to_string(), manifest_label(base, "")));
        }
        // A requirements.txt / Pipfile already passed the `.py` sibling gate
        // in `candidates`, which is its whole confirmation, and names no
        // project (DE.9): its text is never read either.
        if PY_SOURCE_GATED.contains(&base) {
            return Some((eco, base.to_string(), None));
        }
        let text = read(base);
        if confirms(base, &text) {
            return Some((eco, base.to_string(), manifest_label(base, &text)));
        }
    }
    None
}

/// Best-effort human label from a manifest's TEXT. Never fails; the caller
/// falls back to the directory basename.
pub fn manifest_label(manifest_basename: &str, text: &str) -> Option<String> {
    let raw = match manifest_basename {
        "package.json" | "composer.json" => json_top_level_str(text, "name"),
        "go.mod" => text.lines().map(str::trim).find_map(|l| {
            let rest = l.strip_prefix("module")?;
            if !rest.starts_with(char::is_whitespace) {
                return None;
            }
            let rest = rest.split("//").next().unwrap_or("").trim();
            Some(rest.trim_matches(|c| c == '"' || c == '`').to_string())
        }),
        "Cargo.toml" => section_value(text, &["package"], "name", true),
        "pyproject.toml" => section_value(text, &["project", "tool.poetry"], "name", true),
        "setup.cfg" => section_value(text, &["metadata"], "name", false),
        "pom.xml" => pom_artifact_id(text),
        "pubspec.yaml" => text.lines().find_map(|l| {
            let v = l
                .strip_prefix("name:")?
                .split(" #")
                .next()?
                .split_whitespace()
                .next()?;
            Some(v.trim_matches(|c| c == '"' || c == '\'').to_string())
        }),
        "mix.exs" => mix_app(text),
        other => {
            let p = Path::new(other);
            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
            if MANIFEST_EXTS
                .iter()
                .any(|(e, _)| e.eq_ignore_ascii_case(ext))
            {
                p.file_stem().and_then(|s| s.to_str()).map(str::to_string)
            } else {
                None
            }
        }
    };
    raw.as_deref().and_then(clean_label)
}

/// Labels are display strings that later land in a cell: bounded, printable,
/// and never containing the locked qname separator.
fn clean_label(s: &str) -> Option<String> {
    let s = s.trim();
    let ok = !s.is_empty()
        && s.chars().count() <= 200
        && !s.chars().any(char::is_control)
        && !s.contains("::")
        && !s.contains("${");
    ok.then(|| s.to_string())
}

/// `key = "value"` inside any of `sections` (a TOML / INI line scan). A header
/// resets the section, so the scan stops reading a section at the next `[`.
/// `quoted` demands a TOML string, which rejects `name = { workspace = true }`.
fn section_value(text: &str, sections: &[&str], key: &str, quoted: bool) -> Option<String> {
    let mut inside = false;
    for line in text.lines().map(str::trim) {
        if let Some(h) = line.strip_prefix('[') {
            let header = h
                .trim_start_matches('[')
                .split(']')
                .next()
                .unwrap_or("")
                .trim();
            inside = sections.contains(&header);
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        if !inside || k.trim() != key {
            continue;
        }
        let v = v.trim();
        let Some(q) = v.chars().next().filter(|c| *c == '"' || *c == '\'') else {
            return (!quoted).then(|| v.to_string());
        };
        return v[1..].split(q).next().map(str::to_string);
    }
    None
}

/// The project's own `<artifactId>`: a child of `<project>` (or top level),
/// never the one inside `<parent>`, `<dependencies>` or a plugin. A tag-level
/// scan with an element stack — no XML dependency.
fn pom_artifact_id(text: &str) -> Option<String> {
    let mut stack: Vec<&str> = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        rest = &rest[open..];
        if let Some(after) = rest.strip_prefix("<!--") {
            rest = after.split_once("-->").map_or("", |(_, r)| r);
            continue;
        }
        let close = rest.find('>')?;
        let tag = &rest[1..close];
        rest = &rest[close + 1..];
        if tag.starts_with('?') || tag.starts_with('!') || tag.ends_with('/') {
            continue;
        }
        if let Some(name) = tag.strip_prefix('/') {
            if stack.last() == Some(&name.trim()) {
                stack.pop();
            }
            continue;
        }
        let name = tag.split_whitespace().next().unwrap_or("");
        if name == "artifactId" && (stack.is_empty() || stack == ["project"]) {
            return rest.split('<').next().map(|v| v.trim().to_string());
        }
        stack.push(name);
    }
    None
}

/// `app: :my_app` from a mix.exs `project/0` keyword list.
fn mix_app(text: &str) -> Option<String> {
    text.match_indices("app:").find_map(|(i, _)| {
        let word_start = text[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
        let atom: String = text[i + 4..]
            .trim_start()
            .strip_prefix(':')?
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        (word_start && !atom.is_empty()).then_some(atom)
    })
}

/// A top-level string member of a JSON object — just enough JSON to read
/// `package.json`'s `name` without a serde dependency in this crate. Nested
/// objects (`"author": {"name": …}`) are skipped by depth.
fn json_top_level_str(text: &str, key: &str) -> Option<String> {
    let b = text.trim_start_matches('\u{feff}').trim_start().as_bytes();
    if b.first() != Some(&b'{') {
        return None;
    }
    let (mut i, mut depth, mut want_key) = (0usize, 0usize, false);
    while i < b.len() {
        match b[i] {
            b'{' | b'[' => {
                depth += 1;
                want_key = depth == 1;
            }
            b'}' | b']' => depth = depth.saturating_sub(1),
            b',' if depth == 1 => want_key = true,
            b'"' => {
                let (s, end) = json_string(b, i)?;
                i = end;
                if depth == 1 && std::mem::take(&mut want_key) && s == key {
                    // `i` <= len (json_string's contract), so these slices are in bounds.
                    let skip_ws = |from: usize| {
                        from + b[from..]
                            .iter()
                            .take_while(|c| c.is_ascii_whitespace())
                            .count()
                    };
                    let colon = skip_ws(i);
                    if b.get(colon) != Some(&b':') {
                        return None;
                    }
                    return json_string(b, skip_ws(colon + 1)).map(|(s, _)| s);
                }
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Decode the JSON string starting at the `"` at `b[start]`. Returns the text
/// and the index just past the closing quote. `\u` escapes outside the BMP
/// (surrogates) give `None`, which the caller turns into the basename fallback.
fn json_string(b: &[u8], start: usize) -> Option<(String, usize)> {
    if b.get(start) != Some(&b'"') {
        return None;
    }
    let mut out = Vec::new();
    let mut i = start + 1;
    loop {
        match *b.get(i)? {
            b'"' => return String::from_utf8(out).ok().map(|s| (s, i + 1)),
            b'\\' => {
                let e = *b.get(i + 1)?;
                i += 2;
                let c = match e {
                    b'"' => '"',
                    b'\\' => '\\',
                    b'/' => '/',
                    b'b' => '\u{8}',
                    b'f' => '\u{c}',
                    b'n' => '\n',
                    b'r' => '\r',
                    b't' => '\t',
                    b'u' => {
                        let hex = std::str::from_utf8(b.get(i..i + 4)?).ok()?;
                        i += 4;
                        char::from_u32(u32::from_str_radix(hex, 16).ok()?)?
                    }
                    _ => return None,
                };
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
}

/// The sub-projects a root `build.sbt` declares (DG.12), as `(dir, id)` in
/// declaration order: `dir` is the member's directory relative to the
/// `build.sbt`, `id` its sbt project id. sbt's canonical multi-project build
/// declares every project in ONE root `build.sbt` and the sub-directories carry
/// no manifest of their own, so the per-directory [`detect_root`] finds only
/// the root.
///
/// A token scan, comments (`//`, nesting `/* */`) dropped and strings read
/// whole. Each form is a top-level `val` / `lazy val` (an optional `: Type`
/// before the `=`) whose right side starts, after any `(`, with:
/// - `project in file("d")` or `project.in(file("d"))`, settings chained
///   before or after it (`.settings(..)`, `.dependsOn(..)`, a leading-`.`
///   line);
/// - a bare `project` with no `in`: sbt's default base, the directory named
///   like the val;
/// - `Project("id", file("d"))` / `Project(id = "id", base = file("d"))`
///   (sbt 0.13), whose id is the string sbt names the project by.
///
/// For the first two forms the id is the val name, as sbt's `project` macro
/// takes it. A directory sbt computes (`file(base + "/api")`, an
/// interpolated or escaped string, a `Seq` of projects) is not read, and that
/// val gives no member. `./x`, `x/` and `x//y` normalise to `x` / `x/y`;
/// `file(".")` (the root itself, normalised to empty) is not a member; a
/// `..` segment, an absolute or drive path, `~`, `${`, `\`, `::` or a control
/// character rejects the val. A directory two vals name is listed once, for
/// the first.
pub fn sbt_subprojects(text: &str) -> Vec<(String, String)> {
    let toks = sbt_tokens(text);
    let mut out: Vec<(String, String)> = Vec::new();
    let mut depth = 0usize;
    for (i, t) in toks.iter().enumerate() {
        match t {
            SbtTok::Punct('(' | '[' | '{') => depth += 1,
            SbtTok::Punct(')' | ']' | '}') => depth = depth.saturating_sub(1),
            SbtTok::Ident(v) if v == "val" && depth == 0 => {
                if let Some((dir, id)) = sbt_val(&toks, i + 1)
                    && !dir.is_empty()
                    && !out.iter().any(|(d, _)| *d == dir)
                {
                    out.push((dir, id));
                }
            }
            _ => {}
        }
    }
    out
}

/// One `build.sbt` token. A `Str` is a string literal's text and whether it is
/// plain: an escape (`\`) makes it not, since its text is not the path sbt
/// reads.
#[derive(Clone, Debug, PartialEq, Eq)]
enum SbtTok {
    Ident(String),
    Str { text: String, plain: bool },
    Punct(char),
    Newline,
}

/// Lex `text` into [`SbtTok`]s over chars (never byte offsets). An
/// identifier is a run of alphanumerics, `_` and `$` (numbers lex as one
/// too); a backquoted name is one identifier; a char literal is dropped;
/// every other non-space char is a `Punct`. A block comment spanning lines
/// leaves one `Newline`, so it still ends a statement.
fn sbt_tokens(text: &str) -> Vec<SbtTok> {
    let c: Vec<char> = text.chars().collect();
    let ident = |ch: char| ch.is_alphanumeric() || ch == '_' || ch == '$';
    let triple = |i: usize| c.get(i..i + 3).is_some_and(|s| s == ['"', '"', '"']);
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        let next = c.get(i + 1).copied();
        if ch == '\n' {
            out.push(SbtTok::Newline);
            i += 1;
        } else if ch.is_whitespace() {
            i += 1;
        } else if ch == '/' && next == Some('/') {
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
        } else if ch == '/' && next == Some('*') {
            // Scala block comments nest.
            let (mut depth, mut newline) = (0usize, false);
            while i < c.len() {
                if c[i] == '/' && c.get(i + 1) == Some(&'*') {
                    depth += 1;
                    i += 2;
                } else if c[i] == '*' && c.get(i + 1) == Some(&'/') {
                    depth = depth.saturating_sub(1);
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    newline |= c[i] == '\n';
                    i += 1;
                }
            }
            if newline {
                out.push(SbtTok::Newline);
            }
        } else if triple(i) {
            let start = i + 3;
            let mut j = start;
            while j < c.len() && !triple(j) {
                j += 1;
            }
            out.push(SbtTok::Str {
                text: c[start..j].iter().collect(),
                plain: true,
            });
            i = (j + 3).min(c.len());
        } else if ch == '"' {
            let (mut j, mut plain) = (i + 1, true);
            while j < c.len() && c[j] != '"' && c[j] != '\n' {
                if c[j] == '\\' {
                    plain = false;
                    j += 1;
                }
                j += 1;
            }
            let end = j.min(c.len());
            out.push(SbtTok::Str {
                text: c[i + 1..end].iter().collect(),
                plain,
            });
            i = if c.get(end) == Some(&'"') { end + 1 } else { end };
        } else if ch == '\'' {
            // A char literal (`'x'`, `'\n'`) is dropped, so a quote inside one
            // never opens a string; a lone `'` (a Scala 2 symbol) is a Punct.
            if c.get(i + 2) == Some(&'\'') {
                i += 3;
            } else if next == Some('\\') {
                let close = (i + 2..c.len().min(i + 9)).find(|&j| c[j] == '\'');
                i = close.map_or(i + 1, |j| j + 1);
            } else {
                out.push(SbtTok::Punct('\''));
                i += 1;
            }
        } else if ch == '`' {
            let mut j = i + 1;
            while j < c.len() && c[j] != '`' && c[j] != '\n' {
                j += 1;
            }
            out.push(SbtTok::Ident(c[i + 1..j.min(c.len())].iter().collect()));
            i = if c.get(j) == Some(&'`') { j + 1 } else { j };
        } else if ident(ch) {
            let mut j = i;
            while j < c.len() && ident(c[j]) {
                j += 1;
            }
            out.push(SbtTok::Ident(c[i..j].iter().collect()));
            i = j;
        } else {
            out.push(SbtTok::Punct(ch));
            i += 1;
        }
    }
    out
}

/// Where a declared sbt project lives: its own directory (`in file("d")`,
/// `base = file("d")`), or sbt's default base, the directory named like the val.
enum SbtBase {
    Default,
    Dir(String),
}

/// The index of the first token at or after `i` that is not a `Newline`.
fn sbt_skip_newlines(t: &[SbtTok], mut i: usize) -> usize {
    while matches!(t.get(i), Some(SbtTok::Newline)) {
        i += 1;
    }
    i
}

/// The tokens inside the bracket group opening at `t[open]`, and the index
/// just past its closer; `None` when the group never closes.
fn sbt_group(t: &[SbtTok], open: usize) -> Option<(&[SbtTok], usize)> {
    let mut depth = 0usize;
    for (j, tok) in t.iter().enumerate().skip(open) {
        match tok {
            SbtTok::Punct('(' | '[' | '{') => depth += 1,
            SbtTok::Punct(')' | ']' | '}') => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some((&t[open + 1..j], j + 1));
                }
            }
            _ => {}
        }
    }
    None
}

/// `t` with its `Newline`s dropped: inside brackets they are whitespace.
fn sbt_dense(t: &[SbtTok]) -> Vec<&SbtTok> {
    t.iter().filter(|x| **x != SbtTok::Newline).collect()
}

/// A single plain string literal, `"d"`.
fn sbt_str_arg(t: &[SbtTok]) -> Option<String> {
    match sbt_dense(t)[..] {
        [SbtTok::Str { text, plain: true }] => Some(text.clone()),
        _ => None,
    }
}

/// Exactly `file("d")`.
fn sbt_file_arg(t: &[SbtTok]) -> Option<String> {
    match sbt_dense(t)[..] {
        [
            SbtTok::Ident(f),
            SbtTok::Punct('('),
            SbtTok::Str { text, plain: true },
            SbtTok::Punct(')'),
        ] if f == "file" => Some(text.clone()),
        _ => None,
    }
}

/// The `(dir, id)` of the val whose name is `t[i]`, or `None` when its right
/// side declares no project sbt_subprojects reads.
fn sbt_val(t: &[SbtTok], mut i: usize) -> Option<(String, String)> {
    let SbtTok::Ident(name) = t.get(i)? else {
        return None;
    };
    i += 1;
    // An optional `: Type` before the `=`, on the val's own line.
    loop {
        match t.get(i)? {
            SbtTok::Punct('=') => {
                if matches!(t.get(i + 1), Some(SbtTok::Punct('=' | '>'))) {
                    return None;
                }
                i += 1;
                break;
            }
            SbtTok::Newline | SbtTok::Punct(';') => return None,
            _ => i += 1,
        }
    }
    let (base, id) = sbt_project_expr(t, i)?;
    let dir = match base {
        SbtBase::Default => name.clone(),
        SbtBase::Dir(d) => d,
    };
    Some((sbt_dir(&dir)?, id.unwrap_or_else(|| name.clone())))
}

/// The right side of a val starting at `t[i]`: a `project` chain or a
/// `Project(..)` call, after any `(`. The id is `Some` only for `Project(..)`.
fn sbt_project_expr(t: &[SbtTok], i: usize) -> Option<(SbtBase, Option<String>)> {
    let mut i = sbt_skip_newlines(t, i);
    let mut open = 0usize;
    while matches!(t.get(i), Some(SbtTok::Punct('('))) {
        open += 1;
        i = sbt_skip_newlines(t, i + 1);
    }
    match t.get(i)? {
        SbtTok::Ident(h) if h == "project" => sbt_project_chain(t, i + 1, open).map(|b| (b, None)),
        SbtTok::Ident(h) if h == "Project" && matches!(t.get(i + 1), Some(SbtTok::Punct('('))) => {
            let (args, _) = sbt_group(t, i + 1)?;
            sbt_project_apply(args)
        }
        _ => None,
    }
}

/// The calls chained on `project` from `t[i]`, `open` parens still open:
/// `in file("d")` (infix) or `.in(file("d"))` names the base (the last one
/// wins, as in sbt), any other `.method(..)` / `.method { .. }` is skipped. A
/// newline outside the parens continues the chain only before a leading `.`.
/// An `in` whose argument is not `file("d")` is a computed base: `None`.
fn sbt_project_chain(t: &[SbtTok], mut i: usize, mut open: usize) -> Option<SbtBase> {
    let mut base = SbtBase::Default;
    loop {
        let j = sbt_skip_newlines(t, i);
        if j > i && open == 0 && !matches!(t.get(j), Some(SbtTok::Punct('.'))) {
            break;
        }
        match t.get(j) {
            Some(SbtTok::Punct(')')) if open > 0 => {
                open -= 1;
                i = j + 1;
            }
            Some(SbtTok::Punct('.')) => {
                let Some(SbtTok::Ident(method)) = t.get(j + 1) else {
                    break;
                };
                let k = j + 2;
                if matches!(t.get(k), Some(SbtTok::Punct('(' | '{'))) {
                    let (inner, after) = sbt_group(t, k)?;
                    if method == "in" {
                        base = SbtBase::Dir(sbt_file_arg(inner)?);
                    }
                    i = after;
                } else {
                    i = k;
                }
            }
            Some(SbtTok::Ident(w)) if w == "in" => match (t.get(j + 1), t.get(j + 2)) {
                (Some(SbtTok::Ident(f)), Some(SbtTok::Punct('('))) if f == "file" => {
                    let (inner, after) = sbt_group(t, j + 2)?;
                    base = SbtBase::Dir(sbt_str_arg(inner)?);
                    i = after;
                }
                _ => return None,
            },
            _ => break,
        }
    }
    Some(base)
}

/// `Project("id", file("d"))` / `Project(id = "id", base = file("d"))`: the
/// args split at their top-level commas, each named (`base = ..`) or by
/// position (id first, base second). A base that is not `file("d")` gives
/// `None`; an id that is not a plain string leaves the val name.
fn sbt_project_apply(args: &[SbtTok]) -> Option<(SbtBase, Option<String>)> {
    let mut parts: Vec<&[SbtTok]> = Vec::new();
    let (mut depth, mut start) = (0usize, 0usize);
    for (j, tok) in args.iter().enumerate() {
        match tok {
            SbtTok::Punct('(' | '[' | '{') => depth += 1,
            SbtTok::Punct(')' | ']' | '}') => depth = depth.saturating_sub(1),
            SbtTok::Punct(',') if depth == 0 => {
                parts.push(&args[start..j]);
                start = j + 1;
            }
            _ => {}
        }
    }
    parts.push(&args[start..]);
    let (mut id, mut base) = (None, None);
    let mut position = 0usize;
    for part in parts {
        let p = sbt_skip_newlines(part, 0);
        let named = match (part.get(p), part.get(p + 1), part.get(p + 2)) {
            (Some(SbtTok::Ident(n)), Some(SbtTok::Punct('=')), next)
                if !matches!(next, Some(SbtTok::Punct('=' | '>'))) =>
            {
                Some((n.as_str(), &part[p + 2..]))
            }
            _ => None,
        };
        let (slot, value) = match named {
            Some((n, v)) => (n, v),
            None => {
                position += 1;
                (if position == 1 { "id" } else if position == 2 { "base" } else { "" }, part)
            }
        };
        match slot {
            "id" => id = sbt_str_arg(value),
            "base" => base = Some(sbt_file_arg(value)?),
            _ => {}
        }
    }
    Some((SbtBase::Dir(base?), id))
}

/// A declared directory, normalised (`./x`, `x/`, `x//y`), or `None` when sbt
/// would resolve it outside the build or it cannot be a walked path: a `..`
/// segment, an absolute, drive or `~` path, `${`, `\`, `::` or a control
/// char. The root itself (`.`) is `Some("")`.
fn sbt_dir(raw: &str) -> Option<String> {
    let rejected = raw.starts_with('/')
        || raw.starts_with('~')
        || raw.as_bytes().get(1) == Some(&b':')
        || raw.contains("${")
        || raw.contains('\\')
        || raw.contains("::")
        || raw.chars().any(char::is_control);
    if rejected {
        return None;
    }
    let mut segs: Vec<&str> = Vec::new();
    for s in raw.split('/') {
        match s {
            "" | "." => {}
            ".." => return None,
            s => segs.push(s),
        }
    }
    Some(segs.join("/"))
}

/// Manifests [`project_name`] reads, in priority order.
const PROJECT_NAME_MANIFESTS: [&str; 5] = [
    "Cargo.toml",
    "package.json",
    "pyproject.toml",
    "composer.json",
    "go.mod",
];

/// Human project name for `repo` (G18): the first manifest that yields a
/// label, in priority order `Cargo.toml` (`[package].name`), `package.json`
/// (top-level `name`), `pyproject.toml` (`[project]` or `[tool.poetry]` `name`),
/// `composer.json` (`name`), `go.mod` (the last `/` segment of the module
/// path); else the directory name.
///
/// Labels come from [`manifest_label`], so a name holding `::`, `${` or a
/// control character is skipped exactly as it is for a project root. `None`
/// only when no manifest names the project and `repo` has no final component
/// (`/`, `.`, `..`).
///
/// Moved here from the core crate (LD.10): core carried three private hand
/// parsers duplicating [`manifest_label`], and reading package manifests is a
/// code concept.
pub fn project_name(repo: &Path) -> Option<String> {
    PROJECT_NAME_MANIFESTS
        .iter()
        .find_map(|base| {
            let text = std::fs::read_to_string(repo.join(base)).ok()?;
            let label = manifest_label(base, &text)?;
            if *base != "go.mod" {
                return Some(label);
            }
            label
                .rsplit('/')
                .next()
                .filter(|seg| !seg.is_empty())
                .map(str::to_string)
        })
        .or_else(|| repo.file_name().and_then(|s| s.to_str()).map(str::to_string))
}

/// `3 project roots (cargo=1 go=1 npm=1)` — the `[roots]` marker body.
/// Ecosystems are listed by name so the line is deterministic.
pub fn marker(roots: &[ProjectRoot]) -> String {
    let mut by_eco: BTreeMap<&str, usize> = BTreeMap::new();
    for r in roots {
        *by_eco.entry(r.ecosystem).or_default() += 1;
    }
    let parts: Vec<String> = by_eco.iter().map(|(e, n)| format!("{e}={n}")).collect();
    format!("{} project roots ({})", roots.len(), parts.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(ns: &[&str]) -> Vec<String> {
        ns.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn project_roots_detect_by_name() {
        assert_eq!(
            detect(&names(&["README.md", "package.json"])),
            Some(("npm", "package.json".into()))
        );
        assert_eq!(detect(&names(&["main.py"])), None);
        // Precedence is the table order, not the (sorted) name order.
        assert_eq!(
            detect(&names(&["Cargo.toml", "package.json"])),
            Some(("npm", "package.json".into()))
        );
        assert_eq!(
            detect(&names(&["Api.csproj", "Program.cs"])),
            Some(("dotnet", "Api.csproj".into()))
        );
        assert_eq!(
            detect(&names(&["Api.csproj", "Shop.sln"])),
            Some(("dotnet", "Shop.sln".into()))
        );
        // A bare extension is not a project file.
        assert_eq!(detect(&names(&[".csproj"])), None);
    }

    #[test]
    fn project_roots_manifest_labels() {
        assert_eq!(
            manifest_label("package.json", r#"{"name":"@shop/web"}"#),
            Some("@shop/web".into())
        );
        // Nested `name` keys are not the package name.
        assert_eq!(
            manifest_label(
                "package.json",
                "{\n  \"author\": {\"name\": \"x\"},\n  \"deps\": [\"name\"],\n  \"name\" : \"web\"\n}"
            ),
            Some("web".into())
        );
        assert_eq!(manifest_label("package.json", r#"{"private": true}"#), None);
        assert_eq!(manifest_label("package.json", "not json"), None);
        assert_eq!(
            manifest_label("composer.json", r#"{"name":"acme\/shop"}"#),
            Some("acme/shop".into())
        );
        assert_eq!(
            manifest_label("go.mod", "module github.com/shop/api\n\ngo 1.22\n"),
            Some("github.com/shop/api".into())
        );
        assert_eq!(
            manifest_label("go.mod", "module \"example.com/q\" // quoted\n"),
            Some("example.com/q".into())
        );
        assert_eq!(manifest_label("go.mod", "modules x\n"), None);
        assert_eq!(
            manifest_label("Cargo.toml", "[workspace]\nmembers=[]\n"),
            None
        );
        assert_eq!(
            manifest_label(
                "Cargo.toml",
                "[package]\nname = \"core\"\n[dependencies]\nname = \"x\"\n"
            ),
            Some("core".into())
        );
        assert_eq!(
            manifest_label("Cargo.toml", "[package]\nname.workspace = true\n"),
            None
        );
        assert_eq!(
            manifest_label(
                "pyproject.toml",
                "[build-system]\nname = \"no\"\n[tool.poetry]\nname = 'svc'\n"
            ),
            Some("svc".into())
        );
        assert_eq!(
            manifest_label("setup.cfg", "[metadata]\nname = billing\n"),
            Some("billing".into())
        );
        assert_eq!(
            manifest_label(
                "pom.xml",
                "<parent><artifactId>p</artifactId></parent><artifactId>svc</artifactId>"
            ),
            Some("svc".into())
        );
        let pom = r#"<?xml version="1.0"?>
<project xmlns="x">
  <!-- <artifactId>c</artifactId> -->
  <parent>
    <artifactId>p</artifactId>
  </parent>
  <dependencies><dependency><artifactId>d</artifactId></dependency></dependencies>
  <artifactId> orders </artifactId>
</project>
"#;
        assert_eq!(manifest_label("pom.xml", pom), Some("orders".into()));
        assert_eq!(
            manifest_label("pubspec.yaml", "# app\nname: quokka_app # the app\n"),
            Some("quokka_app".into())
        );
        assert_eq!(
            manifest_label(
                "mix.exs",
                "def project do\n  [\n    myapp: :no,\n    app: :shop_web,\n  ]\nend\n"
            ),
            Some("shop_web".into())
        );
        assert_eq!(
            manifest_label("Api.Gateway.csproj", ""),
            Some("Api.Gateway".into())
        );
        assert_eq!(manifest_label("Gemfile", "source 'x'\n"), None);
    }

    #[test]
    fn project_roots_label_rejects_separator_and_control_chars() {
        assert_eq!(manifest_label("package.json", r#"{"name":"a::b"}"#), None);
        assert_eq!(manifest_label("package.json", r#"{"name":"a\nb"}"#), None);
        assert_eq!(
            manifest_label(
                "package.json",
                &format!(r#"{{"name":"{}"}}"#, "x".repeat(201))
            ),
            None
        );
        assert_eq!(manifest_label("package.json", r#"{"name":"   "}"#), None);
        // The fallback chain: rejected label → dir basename → "root".
        let r = ProjectRoot::new("apps/web".into(), "npm", "package.json", None);
        assert_eq!(
            (r.label.as_str(), r.manifest.as_str()),
            ("web", "apps/web/package.json")
        );
        let r = ProjectRoot::new(String::new(), "go", "go.mod", None);
        assert_eq!((r.label.as_str(), r.manifest.as_str()), ("root", "go.mod"));
    }

    /// DD.4: each root records which link of the label fallback chain named
    /// it, so a basename fallback is never mistaken for a manifest's own name.
    #[test]
    fn label_source_follows_the_fallback() {
        let r = ProjectRoot::new("apps/web".into(), "npm", "package.json", Some("@shop/web".into()));
        assert_eq!((r.label.as_str(), r.label_source), ("@shop/web", LABEL_MANIFEST));
        let r = ProjectRoot::new("packages/utils".into(), "npm", "package.json", None);
        assert_eq!((r.label.as_str(), r.label_source), ("utils", LABEL_DIR));
        let r = ProjectRoot::new(String::new(), "go", "go.mod", None);
        assert_eq!((r.label.as_str(), r.label_source), ("root", LABEL_ROOT));
        let r = ProjectRoot::declared("libs/x".into(), Some("X"));
        assert_eq!((r.label.as_str(), r.label_source), ("X", LABEL_DECLARED));
        let r = ProjectRoot::declared("libs/x".into(), None);
        assert_eq!((r.label.as_str(), r.label_source), ("x", LABEL_DIR));
        // A declared label that breaks the label rules falls back to the dir.
        let r = ProjectRoot::declared("svc/api".into(), Some("a::b"));
        assert_eq!((r.label.as_str(), r.label_source), ("api", LABEL_DIR));
        let r = ProjectRoot::declared(String::new(), None);
        assert_eq!((r.label.as_str(), r.label_source), ("root", LABEL_ROOT));
        // The walker's own path: a manifest with no name falls back to the dir.
        let (eco, base, label) =
            detect_root(&names(&["package.json"]), |_| r#"{"private":true}"#.into()).unwrap();
        let r = ProjectRoot::new("packages/utils".into(), eco, &base, label);
        assert_eq!((r.label.as_str(), r.label_source), ("utils", LABEL_DIR));
        let distinct: std::collections::BTreeSet<_> =
            [LABEL_MANIFEST, LABEL_DIR, LABEL_ROOT, LABEL_DECLARED].into_iter().collect();
        assert_eq!(distinct.len(), 4, "the four sources are distinct strings");
    }

    #[test]
    fn project_roots_content_confirmation() {
        let read_none = |_: &str| String::new();
        // A CMakeLists.txt without project() is a subdirectory, not a root.
        assert_eq!(detect_root(&names(&["CMakeLists.txt"]), read_none), None);
        let cmake =
            |_: &str| "cmake_minimum_required(VERSION 3.20)\nPROJECT (engine CXX)\n".to_string();
        assert_eq!(
            detect_root(&names(&["CMakeLists.txt"]), cmake),
            Some(("cmake", "CMakeLists.txt".into(), None))
        );
        // Tool-only setup.cfg does not root; a real one beside it does not matter.
        assert_eq!(
            detect_root(&names(&["setup.cfg"]), |_| "[flake8]\nmax-line = 100\n"
                .into()),
            None
        );
        // An unconfirmed candidate falls through to the next one.
        let mixed = |b: &str| {
            if b == "build.sbt" {
                "name := \"x\"".into()
            } else {
                String::new()
            }
        };
        assert_eq!(
            detect_root(&names(&["CMakeLists.txt", "build.sbt"]), mixed),
            Some(("sbt", "build.sbt".into(), None))
        );
        // .NET is labelled by stem with no read at all.
        let mut reads = 0;
        let got = detect_root(&names(&["Api.csproj"]), |_| {
            reads += 1;
            String::new()
        });
        assert_eq!(
            got,
            Some(("dotnet", "Api.csproj".into(), Some("Api".into())))
        );
        assert_eq!(reads, 0);
    }

    /// DE.9: a requirements.txt or Pipfile roots a python project only beside
    /// a `.py` file, at the lowest precedence, labelled by the directory, and
    /// `detect` (names) and `detect_root` (the walker's path) agree.
    #[test]
    fn project_roots_requirements_and_pipfile_need_a_py_sibling() {
        let mut reads = Vec::new();
        let mut root = |ns: &[&str]| {
            detect_root(&names(ns), |b| {
                reads.push(b.to_string());
                String::new()
            })
        };
        assert_eq!(
            root(&["app.py", "requirements.txt"]),
            Some(("python", "requirements.txt".into(), None))
        );
        assert_eq!(
            root(&["Pipfile", "main.py"]),
            Some(("python", "Pipfile".into(), None))
        );
        // A Sphinx docs pin with no Python source is no project.
        assert_eq!(root(&["index.md", "requirements.txt"]), None);
        assert_eq!(root(&["Pipfile", "README.rst"]), None);
        // A bare `.py` dotfile is not a source file; `x.pyc` / `x.pyi` are not either.
        assert_eq!(root(&[".py", "requirements.txt"]), None);
        assert_eq!(root(&["requirements.txt", "x.pyc", "y.pyi"]), None);
        // The name check is the whole confirmation: no manifest text is read.
        assert!(reads.is_empty(), "read {reads:?}");
        // Lowest precedence: any other manifest in the directory decides.
        let pkg = |_: &str| r#"{"name":"web"}"#.to_string();
        assert_eq!(
            detect_root(&names(&["package.json", "requirements.txt", "x.py"]), pkg),
            Some(("npm", "package.json".into(), Some("web".into())))
        );
        assert_eq!(
            detect_root(&names(&["Pipfile", "pyproject.toml", "x.py"]), |_| String::new()),
            Some(("python", "pyproject.toml".into(), None))
        );
        // ... unless it is not confirmed: a tool-only setup.cfg falls through.
        assert_eq!(
            detect_root(&names(&["app.py", "requirements.txt", "setup.cfg"]), |_| {
                "[flake8]\n".into()
            }),
            Some(("python", "requirements.txt".into(), None))
        );
        // requirements.txt outranks Pipfile, both outrank .NET's extension rows.
        assert_eq!(
            detect_root(&names(&["Pipfile", "requirements.txt", "x.py"]), |_| String::new()),
            Some(("python", "requirements.txt".into(), None))
        );
        assert_eq!(
            detect(&names(&["Api.csproj", "requirements.txt", "x.py"])),
            Some(("python", "requirements.txt".into()))
        );
        // Name-level detect agrees with the walker's path.
        assert_eq!(
            detect(&names(&["app.py", "requirements.txt"])),
            Some(("python", "requirements.txt".into()))
        );
        assert_eq!(
            detect(&names(&["Pipfile", "main.py"])),
            Some(("python", "Pipfile".into()))
        );
        assert_eq!(detect(&names(&["index.md", "requirements.txt"])), None);
        assert_eq!(
            detect(&names(&["package.json", "requirements.txt", "x.py"])),
            Some(("npm", "package.json".into()))
        );
        assert_eq!(manifest_label("requirements.txt", "flask==3.0.0\n"), None);
        assert_eq!(manifest_label("Pipfile", "[packages]\nrequests = \"*\"\n"), None);
        // No label, so the directory basename names it (DD.4 label_source dir).
        let (eco, base, label) =
            detect_root(&names(&["requirements.txt", "server.py"]), |_| String::new()).unwrap();
        let r = ProjectRoot::new("svc/email".into(), eco, &base, label);
        assert_eq!(
            (r.label.as_str(), r.label_source, r.ecosystem, r.manifest.as_str()),
            ("email", LABEL_DIR, "python", "svc/email/requirements.txt")
        );
        // A repo-root requirements.txt is a `root`-labelled python project.
        let r = ProjectRoot::new(String::new(), eco, &base, None);
        assert_eq!((r.label.as_str(), r.label_source), ("root", LABEL_ROOT));
    }

    #[test]
    fn project_roots_marker_lists_ecosystems_by_name() {
        let roots = vec![
            ProjectRoot::new(String::new(), "npm", "package.json", None),
            ProjectRoot::new("services/api".into(), "go", "go.mod", None),
            ProjectRoot::new("apps/web".into(), "npm", "package.json", None),
        ];
        assert_eq!(marker(&roots), "3 project roots (go=1 npm=2)");
    }

    /// A fresh temp dir named `glia_pn_<name>_<pid>`, removed on drop. The pid
    /// keeps two concurrent `cargo test` runs from clearing each other's files.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("glia_pn_{name}_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn write(&self, file: &str, text: &str) -> &Self {
            std::fs::write(self.0.join(file), text).unwrap();
            self
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn project_name_cargo_toml() {
        let tmp = Scratch::new("cargo");
        tmp.write("Cargo.toml", "[package]\nname = \"my-lib\"\nversion = \"0.1\"\n");
        assert_eq!(project_name(&tmp.0), Some("my-lib".to_string()));
    }

    #[test]
    fn project_name_cargo_skips_other_sections() {
        let tmp = Scratch::new("cargo_other");
        tmp.write("Cargo.toml", "[workspace]\nname = \"wrong\"\n[package]\nname = \"right\"\n");
        assert_eq!(project_name(&tmp.0), Some("right".to_string()));
    }

    #[test]
    fn project_name_package_json() {
        let tmp = Scratch::new("pkg");
        tmp.write("package.json", r#"{ "version": "1.0", "name": "my-app", "deps": {} }"#);
        assert_eq!(project_name(&tmp.0), Some("my-app".to_string()));
    }

    #[test]
    fn project_name_pyproject() {
        let tmp = Scratch::new("py");
        tmp.write(
            "pyproject.toml",
            "[build-system]\nrequires = [\"setuptools\"]\n[project]\nname = \"my_py\"\n",
        );
        assert_eq!(project_name(&tmp.0), Some("my_py".to_string()));
    }

    #[test]
    fn project_name_go_mod_last_segment() {
        let tmp = Scratch::new("go");
        tmp.write("go.mod", "module github.com/me/coolservice\n\ngo 1.22\n");
        assert_eq!(project_name(&tmp.0), Some("coolservice".to_string()));
    }

    #[test]
    fn project_name_falls_back_to_dirname() {
        let tmp = Scratch::new("fallback");
        let dir = format!("glia_pn_fallback_{}", std::process::id());
        assert_eq!(project_name(&tmp.0), Some(dir));
    }

    #[test]
    fn project_name_priority_cargo_over_package_json() {
        let tmp = Scratch::new("priority");
        tmp.write("Cargo.toml", "[package]\nname = \"rust-one\"\n")
            .write("package.json", r#"{"name":"js-one"}"#);
        assert_eq!(project_name(&tmp.0), Some("rust-one".to_string()));
    }

    /// Where `manifest_label` is stricter than core's old hand parsers: a
    /// nested `name` is not the package name, a label-less manifest falls
    /// through to the next one, and a qname separator is never a name.
    #[test]
    fn project_name_uses_manifest_label_rules() {
        let tmp = Scratch::new("label_rules");
        tmp.write(
            "package.json",
            r#"{"author": {"name": "someone"}, "name": "shop-web"}"#,
        );
        assert_eq!(project_name(&tmp.0), Some("shop-web".to_string()));

        let tmp = Scratch::new("label_fallthrough");
        tmp.write("Cargo.toml", "[workspace]\nmembers = []\n")
            .write("pyproject.toml", "[tool.poetry]\nname = 'svc'\n");
        assert_eq!(project_name(&tmp.0), Some("svc".to_string()));

        let tmp = Scratch::new("label_reject");
        tmp.write("package.json", r#"{"name":"a::b"}"#)
            .write("go.mod", "module \"example.com/q\" // quoted\n");
        assert_eq!(project_name(&tmp.0), Some("q".to_string()));

        assert_eq!(project_name(Path::new("/")), None);
    }

    fn sbt(text: &str) -> Vec<(String, String)> {
        sbt_subprojects(text)
    }

    fn pairs(ps: &[(&str, &str)]) -> Vec<(String, String)> {
        ps.iter().map(|(d, i)| (d.to_string(), i.to_string())).collect()
    }

    /// DG.12: every declaration form sbt documents, comments and strings read
    /// as Scala does, the root and every path sbt resolves outside the build
    /// rejected.
    #[test]
    fn sbt_subprojects_forms() {
        // The matrix fixture: two members, the root `file(".")` skipped.
        assert_eq!(
            sbt(concat!(
                "lazy val api = project.in(file(\"api\"))\n",
                "lazy val worker = project.in(file(\"worker\"))\n",
                "lazy val root = project.in(file(\".\")).aggregate(api, worker)\n",
            )),
            pairs(&[("api", "api"), ("worker", "worker")])
        );
        // sbt docs' multi-project forms: infix `in` in parens, settings
        // chained after it on leading-`.` lines, a typed val, a bare
        // `project` (dir = the val), `(project)`, nested dirs, normalised.
        let text = r#"
ThisBuild / scalaVersion := "2.13.12"
lazy val root = (project in file("."))
  .aggregate(core, util)
  .settings(name := "hello")
lazy val core = (project in file("modules/core/"))
  .settings(libraryDependencies += "org" %% "x" % "1")
  .dependsOn(util % "compile->compile;test->test")
lazy val util: Project = project.settings(name := "util")
val bare = project
lazy val paren = (project)
  .enablePlugins(JavaAppPackaging)
lazy val dotted = project.in(file("./svc/dotted"))
lazy val late = project
  .settings(publish := {})
  .in(file("late-dir"))
"#;
        assert_eq!(
            sbt(text),
            pairs(&[
                ("modules/core", "core"),
                ("util", "util"),
                ("bare", "bare"),
                ("paren", "paren"),
                ("svc/dotted", "dotted"),
                ("late-dir", "late"),
            ])
        );
        // sbt 0.13: the id is the string sbt names the project by, positional
        // or named, in either order.
        assert_eq!(
            sbt(concat!(
                "lazy val a = Project(\"core-api\", file(\"api\"))\n",
                "lazy val b = Project(id = \"svc\", base = file(\"svc\")).settings(x)\n",
                "lazy val c = Project(base = file(\"c-dir\"), id = \"cee\")\n",
                "lazy val d = Project(idOf(\"d\"), file(\"d-dir\"))\n",
            )),
            pairs(&[("api", "core-api"), ("svc", "svc"), ("c-dir", "cee"), ("d-dir", "d")])
        );
        // Comments: line, block, nested block; a `//` inside a string is text.
        assert_eq!(
            sbt(concat!(
                "// lazy val gone = project.in(file(\"gone\"))\n",
                "/* lazy val also = project\n /* nested */ lazy val still = project */\n",
                "lazy val kept = project.in(file(\"a//b\")) // trailing\n",
                "lazy val url = project.settings(homepage := Some(url(\"https://x/y\")))\n",
            )),
            pairs(&[("a/b", "kept"), ("url", "url")])
        );
        // Rejected: `..`, absolute, home, drive, `${`, escapes, computed and
        // interpolated dirs; not a project; a val inside a block.
        assert_eq!(
            sbt(concat!(
                "lazy val up = project.in(file(\"../shared\"))\n",
                "lazy val mid = project.in(file(\"a/../b\"))\n",
                "lazy val abs = project in file(\"/opt/x\")\n",
                "lazy val home = project in file(\"~/x\")\n",
                "lazy val win = project in file(\"C:/x\")\n",
                "lazy val env = project in file(\"${HOME}\")\n",
                "lazy val esc = project in file(\"a\\\\b\")\n",
                "lazy val calc = project.in(file(base + \"/api\"))\n",
                "lazy val interp = project.in(file(s\"$base/api\"))\n",
                "lazy val ext = RootProject(file(\"other\"))\n",
                "lazy val settings = Seq(scalacOptions += \"-deprecation\")\n",
                "val version = \"1.0\"\n",
                "object Build { lazy val inner = project }\n",
                "lazy val dup = project.in(file(\"x\"))\n",
                "lazy val dup2 = project.in(file(\"x\"))\n",
                "lazy val dot = project in file(\".\")\n",
                "lazy val empty = project in file(\"\")\n",
            )),
            pairs(&[("x", "dup")])
        );
        // Strings and char literals never open a declaration; `==` / `=>` are
        // not a val's `=`.
        assert_eq!(
            sbt(concat!(
                "val q = '\"'\n",
                "val s = \"\"\"lazy val no = project\"\"\"\n",
                "lazy val yes = project\n",
                "val f = (x: Int) => x\n",
            )),
            pairs(&[("yes", "yes")])
        );
        // The lexer walks chars, never byte offsets: a non-ASCII id and dir
        // round-trip.
        assert_eq!(
            sbt("lazy val ü = project in file(\"dienst-ü\")\n"),
            pairs(&[("dienst-ü", "ü")])
        );
        assert!(sbt("").is_empty());
        assert!(sbt("name := \"single\"\nscalaVersion := \"3.3.1\"\n").is_empty());
        assert!(sbt("lazy val broken = project.in(file(\"x\"").is_empty(), "an unclosed group");
    }

    /// DG.12: a member is an sbt root labelled by its project id (a manifest
    /// name, DD.4) whose manifest is the declaring build.sbt.
    #[test]
    fn sbt_member_label_and_manifest() {
        let r = ProjectRoot::sbt_member("api".into(), "api", "build.sbt".into());
        assert_eq!(
            (r.rel_path.as_str(), r.label.as_str(), r.label_source, r.ecosystem, r.manifest.as_str()),
            ("api", "api", LABEL_MANIFEST, "sbt", "build.sbt")
        );
        let r = ProjectRoot::sbt_member("scala/svc/core".into(), "core-api", "scala/build.sbt".into());
        assert_eq!(
            (r.label.as_str(), r.label_source, r.manifest.as_str()),
            ("core-api", LABEL_MANIFEST, "scala/build.sbt")
        );
        // An id that breaks the label rules falls back to the directory.
        let r = ProjectRoot::sbt_member("svc/core".into(), "a::b", "build.sbt".into());
        assert_eq!((r.label.as_str(), r.label_source), ("core", LABEL_DIR));
        // The ecosystem is the one a build.sbt roots.
        let (eco, base, label) =
            detect_root(&names(&["build.sbt"]), |_| "lazy val api = project\n".into()).unwrap();
        assert_eq!((eco, base.as_str(), label), (SBT_ECOSYSTEM, "build.sbt", None));
        assert_eq!(marker(&[r]), "1 project roots (sbt=1)");
    }

    /// LF.3a: a `[[project]]` root names the overlay file as its manifest and
    /// falls back to the basename when the label breaks the label rules.
    #[test]
    fn declared_roots_carry_the_overlay_as_manifest() {
        let r = ProjectRoot::declared("tools/migrator".into(), Some("migrator"));
        assert_eq!(
            (r.rel_path.as_str(), r.label.as_str(), r.ecosystem, r.manifest.as_str()),
            ("tools/migrator", "migrator", "declared", ".glia/overlay.toml")
        );
        assert_eq!(ProjectRoot::declared("tools/migrator".into(), None).label, "migrator");
        assert_eq!(ProjectRoot::declared("svc/api".into(), Some("a::b")).label, "api");
        assert_eq!(ProjectRoot::declared("svc/api".into(), Some("  Billing  ")).label, "Billing");
        assert_eq!(marker(&[r]), "1 project roots (declared=1)");
    }
}
