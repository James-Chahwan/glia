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
    pub ecosystem: &'static str,
    /// Repo-relative path of the manifest that named it.
    pub manifest: String,
}

impl ProjectRoot {
    /// Build a root, applying the label fallback chain: manifest label → the
    /// basename of `rel_path` → `"root"`. The basename is taken from the
    /// REPO-RELATIVE path, never the checkout directory, so the repo root is
    /// always `"root"` wherever the repo happens to be cloned.
    pub fn new(
        rel_path: String,
        ecosystem: &'static str,
        manifest_basename: &str,
        label: Option<String>,
    ) -> Self {
        let label = label
            .or_else(|| rel_path.rsplit('/').next().and_then(clean_label))
            .unwrap_or_else(|| "root".to_string());
        let manifest = if rel_path.is_empty() {
            manifest_basename.to_string()
        } else {
            format!("{rel_path}/{manifest_basename}")
        };
        Self {
            rel_path,
            label,
            ecosystem,
            manifest,
        }
    }
}

/// Manifest basename → ecosystem, in precedence order. The first entry present
/// (and confirmed, see [`confirms`]) decides the ecosystem; ties never happen
/// because the table is scanned in order.
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
    ("build.sbt", "sbt"),
    ("deps.edn", "clojure"),
    ("project.clj", "clojure"),
];

/// Extension-keyed manifests (.NET), checked after [`MANIFESTS`]. A solution
/// outranks a project file in the same directory.
pub const MANIFEST_EXTS: &[(&str, &str)] = &[
    ("sln", "dotnet"),
    ("csproj", "dotnet"),
    ("fsproj", "dotnet"),
    ("vbproj", "dotnet"),
];

/// Every manifest in one directory's entry names, in precedence order.
fn candidates(names: &[String]) -> impl Iterator<Item = (&'static str, &str)> {
    let by_name = MANIFESTS
        .iter()
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

    #[test]
    fn project_roots_marker_lists_ecosystems_by_name() {
        let roots = vec![
            ProjectRoot::new(String::new(), "npm", "package.json", None),
            ProjectRoot::new("services/api".into(), "go", "go.mod", None),
            ProjectRoot::new("apps/web".into(), "npm", "package.json", None),
        ];
        assert_eq!(marker(&roots), "3 project roots (go=1 npm=2)");
    }
}
