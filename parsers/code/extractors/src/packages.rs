//! Package dependency extraction (v0.4.x — task #10).
//!
//! Emits `PACKAGE_DEP` nodes per declared dependency in a manifest file with
//! `DEPENDS_ON` edges from the manifest's module. `PackageResolver` then
//! pairs same-qname nodes across repos to surface "service A and service B
//! both depend on `react@^18`" cross-service signal.
//!
//! Single qname shape: `package:<ecosystem>:<name>`. Ecosystems:
//!   - `npm`        — package.json `dependencies` / `devDependencies` /
//!                    `peerDependencies`
//!   - `pypi`       — pyproject.toml `[project.dependencies]`,
//!                    `[tool.poetry.dependencies]`, requirements.txt
//!   - `cargo`      — Cargo.toml `[dependencies]` / `[dev-dependencies]` /
//!                    `[build-dependencies]`
//!   - `gomod`      — go.mod `require <name> <version>`
//!   - `rubygems`   — Gemfile `gem '<name>', ...`
//!   - `composer`   — composer.json `require` / `require-dev`
//!
//! Each extractor is a pure function over the file's source. Caller dispatches
//! by file basename via `extract_for_path`.
//!
//! Out of scope (deferred to v0.5+):
//!   - Maven `pom.xml` (XML parsing)
//!   - Gradle `build.gradle` / `*.gradle.kts` (Groovy/Kotlin DSL)
//!   - .NET `*.csproj` (XML PackageReference)
//!   - Lockfiles (`package-lock.json`, `Cargo.lock`, `go.sum`, etc.) — manifest
//!     declarations cover declared intent; lockfile-level analysis is a v0.5+
//!     refinement
//!   - Version-pinning analysis (we extract the name, not the constraint)
//!   - Org-internal-package routing (per TODO; depends on workflow story)

use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, edge_category, node_kind};
use repo_graph_core::{Confidence, Edge, Node, NodeId, RepoId};

pub struct PackageNodes {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub nav: CodeNav,
}

/// Detect manifest type from path basename and extract dependencies. Returns
/// an empty result when the path isn't a recognised manifest.
pub fn extract_for_path(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
) -> PackageNodes {
    let base = path.rsplit('/').next().unwrap_or(path);
    let names_with_eco: Vec<(&'static str, String)> = match base {
        "package.json" => scan_package_json(source)
            .into_iter()
            .map(|n| ("npm", n))
            .collect(),
        "pyproject.toml" => scan_pyproject(source)
            .into_iter()
            .map(|n| ("pypi", n))
            .collect(),
        "requirements.txt" => scan_requirements(source)
            .into_iter()
            .map(|n| ("pypi", n))
            .collect(),
        "Cargo.toml" => scan_cargo_toml(source)
            .into_iter()
            .map(|n| ("cargo", n))
            .collect(),
        "go.mod" => scan_go_mod(source)
            .into_iter()
            .map(|n| ("gomod", n))
            .collect(),
        "Gemfile" => scan_gemfile(source)
            .into_iter()
            .map(|n| ("rubygems", n))
            .collect(),
        "composer.json" => scan_composer_json(source)
            .into_iter()
            .map(|n| ("composer", n))
            .collect(),
        _ => Vec::new(),
    };
    build_nodes(names_with_eco, module_id, repo)
}

/// True when the path is a recognised manifest filename.
pub fn is_manifest_path(path: &str) -> bool {
    let base = path.rsplit('/').next().unwrap_or(path);
    matches!(
        base,
        "package.json"
            | "pyproject.toml"
            | "requirements.txt"
            | "Cargo.toml"
            | "go.mod"
            | "Gemfile"
            | "composer.json"
    )
}

fn build_nodes(
    deps: Vec<(&'static str, String)>,
    module_id: NodeId,
    repo: RepoId,
) -> PackageNodes {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut nav = CodeNav::default();
    let mut seen = std::collections::HashSet::new();
    for (eco, name) in deps {
        if name.is_empty() || name.len() > 256 {
            continue;
        }
        let qname = format!("package:{eco}:{name}");
        if !seen.insert(qname.clone()) {
            continue;
        }
        let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::PACKAGE_DEP, &qname);
        nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Medium,
            cells: vec![],
        });
        nav.record(id, &name, &qname, node_kind::PACKAGE_DEP, Some(module_id));
        edges.push(Edge {
            from: module_id,
            to: id,
            category: edge_category::DEPENDS_ON,
            confidence: Confidence::Medium,
        });
    }
    PackageNodes { nodes, edges, nav }
}

// ----------------------------------------------------------------------------
// package.json — JSON object with `dependencies`/`devDependencies`/
// `peerDependencies`/`optionalDependencies`. Substring-walk: locate each key,
// descend into the immediately following `{ ... }` object, pull `"name":`
// properties.
// ----------------------------------------------------------------------------

fn scan_package_json(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for needle in [
        "\"dependencies\"",
        "\"devDependencies\"",
        "\"peerDependencies\"",
        "\"optionalDependencies\"",
    ] {
        let mut search_from = 0;
        while let Some(rel) = source[search_from..].find(needle) {
            let key_pos = search_from + rel;
            // Skip past the colon to the opening brace.
            let after_key = &source[key_pos + needle.len()..];
            let Some(brace_off) = after_key.find('{') else {
                search_from = key_pos + needle.len();
                continue;
            };
            let body_start = key_pos + needle.len() + brace_off + 1;
            let Some(close_off) = balanced_close(&source[body_start..], b'{', b'}') else {
                search_from = body_start;
                continue;
            };
            let body = &source[body_start..body_start + close_off];
            for name in extract_top_level_string_keys(body) {
                out.push(name);
            }
            search_from = body_start + close_off + 1;
        }
    }
    out
}

/// Find the offset of the close character matching the implicit open at byte 0
/// (caller has already consumed the opener). Skips strings.
fn balanced_close(s: &str, open: u8, close: u8) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut depth = 1i32;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        } else if c == b'"' || c == b'\'' || c == b'`' {
            let delim = c;
            i += 1;
            while i < bytes.len() && bytes[i] != delim {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                } else {
                    i += 1;
                }
            }
        }
        i += 1;
    }
    None
}

/// Walk a JSON object body and yield top-level (depth-0) `"key":` strings.
fn extract_top_level_string_keys(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let mut depth = 0i32;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b'{' | b'[' => depth += 1,
            b'}' | b']' => depth -= 1,
            b'"' if depth == 0 => {
                let start = i + 1;
                let mut j = start;
                while j < bytes.len() && bytes[j] != b'"' {
                    if bytes[j] == b'\\' && j + 1 < bytes.len() {
                        j += 2;
                    } else {
                        j += 1;
                    }
                }
                if j >= bytes.len() {
                    break;
                }
                let key = &body[start..j];
                // Confirm followed by `:` (after optional whitespace) to
                // distinguish keys from value strings.
                let mut k = j + 1;
                while k < bytes.len() && (bytes[k] == b' ' || bytes[k] == b'\t' || bytes[k] == b'\n') {
                    k += 1;
                }
                if k < bytes.len() && bytes[k] == b':' {
                    out.push(key.to_string());
                }
                i = j + 1;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    out
}

// ----------------------------------------------------------------------------
// pyproject.toml — `[project]` PEP-621 deps OR `[tool.poetry.dependencies]`.
// Both layouts captured via a section walker.
// ----------------------------------------------------------------------------

fn scan_pyproject(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    let sections = parse_toml_sections(source);
    for section in &sections {
        match section.name.as_str() {
            "project" => {
                // PEP-621: `dependencies = ["foo>=1.0", "bar~=2.0"]`.
                if let Some(list) = read_toml_array_field(&section.body, "dependencies") {
                    for item in list {
                        if let Some(name) = pep508_package_name(&item) {
                            out.push(name);
                        }
                    }
                }
                if let Some(list) = read_toml_array_field(&section.body, "optional-dependencies") {
                    for item in list {
                        if let Some(name) = pep508_package_name(&item) {
                            out.push(name);
                        }
                    }
                }
            }
            // Poetry's `[tool.poetry.dependencies]` is a key/value table.
            "tool.poetry.dependencies"
            | "tool.poetry.dev-dependencies"
            | "tool.poetry.group.dev.dependencies" => {
                for key in toml_table_keys(&section.body) {
                    if key != "python" {
                        out.push(key);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

#[derive(Debug)]
struct TomlSection {
    name: String,
    body: String,
}

/// Split a TOML document into sections by `[header]` lines. Returns the
/// section names + raw body text (between header and next section).
fn parse_toml_sections(source: &str) -> Vec<TomlSection> {
    let mut out = Vec::new();
    let mut current_name = String::new();
    let mut current_body = String::new();
    for line in source.lines() {
        let t = line.trim_start();
        if t.starts_with('[') && !t.starts_with("[[") {
            if !current_name.is_empty() {
                out.push(TomlSection {
                    name: std::mem::take(&mut current_name),
                    body: std::mem::take(&mut current_body),
                });
            }
            if let Some(end) = t.find(']') {
                current_name = t[1..end].trim().to_string();
            }
        } else if !current_name.is_empty() {
            current_body.push_str(line);
            current_body.push('\n');
        }
    }
    if !current_name.is_empty() {
        out.push(TomlSection {
            name: current_name,
            body: current_body,
        });
    }
    out
}

/// Read `field = ["a", "b"]` from a TOML section body. Returns the inner
/// strings stripped of quotes. Multi-line arrays supported.
fn read_toml_array_field(body: &str, field: &str) -> Option<Vec<String>> {
    let needle = format!("{field} =");
    let pos = body.find(&needle)?;
    let after = body[pos + needle.len()..].trim_start();
    if !after.starts_with('[') {
        return None;
    }
    let close = balanced_close(&after[1..], b'[', b']')?;
    let inner = &after[1..1 + close];
    let mut out = Vec::new();
    let bytes = inner.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' || bytes[i] == b'\'' {
            let delim = bytes[i];
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j] != delim {
                if bytes[j] == b'\\' && j + 1 < bytes.len() {
                    j += 2;
                } else {
                    j += 1;
                }
            }
            if j < bytes.len() {
                out.push(inner[start..j].to_string());
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    Some(out)
}

/// Top-level keys in a TOML table body — line-leading `key = ` patterns,
/// excluding nested-table headers.
fn toml_table_keys(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in body.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') || t.starts_with('[') {
            continue;
        }
        if let Some(eq) = t.find('=') {
            let key = t[..eq].trim();
            // Strip optional quotes for keys like `"foo-bar" = ...`.
            let key = key.trim_matches(|c| c == '"' || c == '\'');
            if !key.is_empty() && is_pep503_or_ident(key) {
                out.push(key.to_string());
            }
        }
    }
    out
}

fn is_pep503_or_ident(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// Extract the bare name from a PEP-508 spec, e.g. `requests>=2.0` → `requests`.
fn pep508_package_name(spec: &str) -> Option<String> {
    let s = spec.trim();
    if s.is_empty() {
        return None;
    }
    // Stop at any of: ; (env marker), [ (extras), <, >, =, ~, !, space.
    let end = s
        .find(|c: char| {
            matches!(
                c,
                ';' | '[' | '<' | '>' | '=' | '~' | '!' | ' ' | '\t' | '@' | ','
            )
        })
        .unwrap_or(s.len());
    let name = s[..end].trim().to_string();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

// ----------------------------------------------------------------------------
// requirements.txt — one PEP-508 spec per line, `#` comments.
// ----------------------------------------------------------------------------

fn scan_requirements(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in source.lines() {
        let t = line.split('#').next().unwrap_or(line).trim();
        if t.is_empty() {
            continue;
        }
        // Skip pip directives (`-r other.txt`, `-e .`, `--index-url ...`).
        if t.starts_with('-') {
            continue;
        }
        if let Some(name) = pep508_package_name(t) {
            out.push(name);
        }
    }
    out
}

// ----------------------------------------------------------------------------
// Cargo.toml — `[dependencies]`, `[dev-dependencies]`, `[build-dependencies]`,
// plus `[target.'cfg(...)'.dependencies]` (skipped — niche).
// ----------------------------------------------------------------------------

fn scan_cargo_toml(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for section in parse_toml_sections(source) {
        match section.name.as_str() {
            "dependencies" | "dev-dependencies" | "build-dependencies" => {
                for key in toml_table_keys(&section.body) {
                    out.push(key);
                }
            }
            _ => {}
        }
    }
    out
}

// ----------------------------------------------------------------------------
// go.mod — `require <name> <version>` or a `require ( ... )` block.
// ----------------------------------------------------------------------------

fn scan_go_mod(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_block = false;
    for line in source.lines() {
        let t = line.split("//").next().unwrap_or(line).trim();
        if t.is_empty() {
            continue;
        }
        if t == "require (" {
            in_block = true;
            continue;
        }
        if t == ")" && in_block {
            in_block = false;
            continue;
        }
        if in_block {
            // Each line: `name version` (optional `// indirect`).
            if let Some(name) = t.split_whitespace().next() {
                out.push(name.to_string());
            }
        } else if let Some(rest) = t.strip_prefix("require ") {
            // Single-line `require name version`.
            if let Some(name) = rest.split_whitespace().next() {
                out.push(name.to_string());
            }
        }
    }
    out
}

// ----------------------------------------------------------------------------
// Gemfile — `gem '<name>', '<version>'`, optionally with `:require => false`.
// ----------------------------------------------------------------------------

fn scan_gemfile(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in source.lines() {
        let t = line.split('#').next().unwrap_or(line).trim();
        if !t.starts_with("gem ") && !t.starts_with("gem\t") {
            continue;
        }
        let rest = &t[4..];
        if let Some(name) = first_quoted(rest) {
            out.push(name);
        }
    }
    out
}

fn first_quoted(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    if i >= bytes.len() {
        return None;
    }
    let c = bytes[i];
    if c != b'\'' && c != b'"' {
        return None;
    }
    let delim = c;
    let start = i + 1;
    let mut j = start;
    while j < bytes.len() && bytes[j] != delim {
        j += 1;
    }
    if j < bytes.len() {
        Some(s[start..j].to_string())
    } else {
        None
    }
}

// ----------------------------------------------------------------------------
// composer.json — same shape as package.json (`require` / `require-dev`).
// ----------------------------------------------------------------------------

fn scan_composer_json(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for needle in ["\"require\"", "\"require-dev\""] {
        let mut search_from = 0;
        while let Some(rel) = source[search_from..].find(needle) {
            let key_pos = search_from + rel;
            let after_key = &source[key_pos + needle.len()..];
            let Some(brace_off) = after_key.find('{') else {
                search_from = key_pos + needle.len();
                continue;
            };
            let body_start = key_pos + needle.len() + brace_off + 1;
            let Some(close_off) = balanced_close(&source[body_start..], b'{', b'}') else {
                search_from = body_start;
                continue;
            };
            let body = &source[body_start..body_start + close_off];
            for name in extract_top_level_string_keys(body) {
                if name != "php" {
                    out.push(name);
                }
            }
            search_from = body_start + close_off + 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module_id(repo: RepoId) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MODULE, "test")
    }

    fn pkg_qnames(out: &PackageNodes) -> Vec<String> {
        out.nav.qname_by_id.values().cloned().collect()
    }

    #[test]
    fn package_json_dependencies_and_dev() {
        let repo = RepoId(1);
        let src = r#"{
  "name": "myapp",
  "dependencies": {
    "react": "^18.2.0",
    "react-dom": "^18.2.0",
    "@types/node": "^20.0.0"
  },
  "devDependencies": {
    "vitest": "^1.0.0",
    "@vitejs/plugin-react": "^4.0.0"
  }
}
"#;
        let out = extract_for_path(src, "package.json", module_id(repo), repo);
        let q = pkg_qnames(&out);
        assert!(q.contains(&"package:npm:react".to_string()));
        assert!(q.contains(&"package:npm:react-dom".to_string()));
        assert!(q.contains(&"package:npm:@types/node".to_string()));
        assert!(q.contains(&"package:npm:vitest".to_string()));
        assert!(q.contains(&"package:npm:@vitejs/plugin-react".to_string()));
    }

    #[test]
    fn pyproject_pep621_and_poetry() {
        let repo = RepoId(1);
        let src = r#"
[project]
name = "myapp"
dependencies = [
    "requests>=2.0",
    "django~=5.0",
    "pydantic[email]>=2.0",
]

[tool.poetry.dependencies]
python = "^3.11"
fastapi = "^0.110.0"
uvicorn = { extras = ["standard"], version = "^0.27.0" }

[tool.poetry.dev-dependencies]
pytest = "^7.0"
"#;
        let out = extract_for_path(src, "pyproject.toml", module_id(repo), repo);
        let q = pkg_qnames(&out);
        assert!(q.contains(&"package:pypi:requests".to_string()));
        assert!(q.contains(&"package:pypi:django".to_string()));
        assert!(q.contains(&"package:pypi:pydantic".to_string()));
        assert!(q.contains(&"package:pypi:fastapi".to_string()));
        assert!(q.contains(&"package:pypi:uvicorn".to_string()));
        assert!(q.contains(&"package:pypi:pytest".to_string()));
        // python is filtered out (not a real dep).
        assert!(!q.contains(&"package:pypi:python".to_string()));
    }

    #[test]
    fn requirements_txt_basic() {
        let repo = RepoId(1);
        let src = r#"
# core
requests==2.31.0
django>=4.0,<5.0
pydantic[email]==2.5.0
# tooling
pytest~=7.4

-r dev-requirements.txt
-e .
"#;
        let out = extract_for_path(src, "requirements.txt", module_id(repo), repo);
        let q = pkg_qnames(&out);
        assert!(q.contains(&"package:pypi:requests".to_string()));
        assert!(q.contains(&"package:pypi:django".to_string()));
        assert!(q.contains(&"package:pypi:pydantic".to_string()));
        assert!(q.contains(&"package:pypi:pytest".to_string()));
        // Pip directives skipped.
        assert_eq!(q.len(), 4);
    }

    #[test]
    fn cargo_toml_dependencies() {
        let repo = RepoId(1);
        let src = r#"
[package]
name = "myapp"
version = "0.1.0"

[dependencies]
serde = { version = "1", features = ["derive"] }
tokio = "1.35"
anyhow = "1"

[dev-dependencies]
proptest = "1.0"

[build-dependencies]
cc = "1.0"
"#;
        let out = extract_for_path(src, "Cargo.toml", module_id(repo), repo);
        let q = pkg_qnames(&out);
        assert!(q.contains(&"package:cargo:serde".to_string()));
        assert!(q.contains(&"package:cargo:tokio".to_string()));
        assert!(q.contains(&"package:cargo:anyhow".to_string()));
        assert!(q.contains(&"package:cargo:proptest".to_string()));
        assert!(q.contains(&"package:cargo:cc".to_string()));
    }

    #[test]
    fn go_mod_block_and_single_line() {
        let repo = RepoId(1);
        let src = r#"
module github.com/foo/bar

go 1.21

require github.com/gin-gonic/gin v1.9.1

require (
    github.com/spf13/cobra v1.7.0
    go.uber.org/zap v1.26.0 // indirect
    github.com/stretchr/testify v1.8.4
)
"#;
        let out = extract_for_path(src, "go.mod", module_id(repo), repo);
        let q = pkg_qnames(&out);
        assert!(q.contains(&"package:gomod:github.com/gin-gonic/gin".to_string()));
        assert!(q.contains(&"package:gomod:github.com/spf13/cobra".to_string()));
        assert!(q.contains(&"package:gomod:go.uber.org/zap".to_string()));
        assert!(q.contains(&"package:gomod:github.com/stretchr/testify".to_string()));
    }

    #[test]
    fn gemfile_basic() {
        let repo = RepoId(1);
        let src = r#"
source 'https://rubygems.org'

gem 'rails', '~> 7.0'
gem "puma", "~> 6.0"
gem 'pg'
# gem 'commented-out', '1.0'

group :development do
  gem 'rspec-rails', '~> 6.0'
end
"#;
        let out = extract_for_path(src, "Gemfile", module_id(repo), repo);
        let q = pkg_qnames(&out);
        assert!(q.contains(&"package:rubygems:rails".to_string()));
        assert!(q.contains(&"package:rubygems:puma".to_string()));
        assert!(q.contains(&"package:rubygems:pg".to_string()));
        assert!(q.contains(&"package:rubygems:rspec-rails".to_string()));
        assert!(!q.contains(&"package:rubygems:commented-out".to_string()));
    }

    #[test]
    fn composer_json_require_blocks() {
        let repo = RepoId(1);
        let src = r#"{
  "name": "vendor/myapp",
  "require": {
    "php": "^8.2",
    "laravel/framework": "^11.0",
    "guzzlehttp/guzzle": "^7.0"
  },
  "require-dev": {
    "phpunit/phpunit": "^10.0"
  }
}
"#;
        let out = extract_for_path(src, "composer.json", module_id(repo), repo);
        let q = pkg_qnames(&out);
        assert!(q.contains(&"package:composer:laravel/framework".to_string()));
        assert!(q.contains(&"package:composer:guzzlehttp/guzzle".to_string()));
        assert!(q.contains(&"package:composer:phpunit/phpunit".to_string()));
        // php platform constraint filtered.
        assert!(!q.contains(&"package:composer:php".to_string()));
    }

    #[test]
    fn unknown_manifest_returns_empty() {
        let repo = RepoId(1);
        let out = extract_for_path("anything", "random.txt", module_id(repo), repo);
        assert!(out.nodes.is_empty());
    }

    #[test]
    fn dedupes_duplicate_dep_listings() {
        // Same package in dependencies AND devDependencies — collapse to one.
        let repo = RepoId(1);
        let src = r#"{
  "dependencies": { "react": "18.0" },
  "peerDependencies": { "react": "18.0" }
}
"#;
        let out = extract_for_path(src, "package.json", module_id(repo), repo);
        let count = pkg_qnames(&out)
            .iter()
            .filter(|q| q.as_str() == "package:npm:react")
            .count();
        assert_eq!(count, 1);
    }
}
