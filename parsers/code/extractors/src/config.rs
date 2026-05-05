//! Config / env-var extraction (v0.4.x — task #8).
//!
//! Emits `CONFIG_KEY` nodes per unique env-var name with two edge flavours:
//!
//!   - `READS_CONFIG`  — code module → key  (`os.environ['DB_URL']` etc.)
//!   - `DEFINES_CONFIG` — source module → key (Dockerfile `ENV`, `.env`, k8s)
//!
//! Single qname per name across the merged graph: `config:env:<NAME>`. The
//! flavor segment reserves room for future config-file / secrets-manager
//! tracks (`config:file:database.yml`, `config:secret:vault/path`).
//!
//! Recognised sources (deliberate v1 cut):
//!
//!   **Reads (in code, all langs):**
//!     - Python: `os.environ['X']`, `os.environ.get('X')`, `os.getenv('X')`
//!     - JS/TS:  `process.env.X`, `process.env['X']`, `import.meta.env.X`
//!     - Rust:   `std::env::var("X")`, `env::var("X")`
//!     - Go:     `os.Getenv("X")`, `os.LookupEnv("X")`
//!     - Ruby:   `ENV['X']`, `ENV.fetch('X')`
//!     - Java:   `System.getenv("X")`
//!     - PHP:    `getenv('X')`, `$_ENV['X']`
//!
//!   **Defines (separate file types via pipeline bypass):**
//!     - Dockerfile `ENV KEY=value` / `ENV KEY value`
//!     - `.env` files (KEY=value lines, # comments)
//!     - k8s YAML `env: - name: KEY` and `envFrom: - secretRef: name: ...`
//!     - docker-compose `environment: - KEY=value`
//!
//! Out of scope (deferred to v0.5+):
//!   - Spring `application.yml` / `application.properties`
//!   - Rails `config/database.yml`, .NET `appsettings.json`
//!   - Vault paths, AWS Secrets Manager ARNs, k8s `Secret` data
//!   - CI variable definitions (GHA `env:`, GitLab CI variables) — reads
//!     covered via shell `$VAR` if needed in v0.5+.

use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, edge_category, node_kind};
use repo_graph_core::{Confidence, Edge, Node, NodeId, RepoId};

pub struct ConfigNodes {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub nav: CodeNav,
}

#[derive(Debug, Clone, Copy)]
enum Side {
    Read,
    Define,
}

/// Extract env-var read references from a source-code file. The caller decides
/// the file type by extension; this scanner tries every language idiom because
/// idioms cross language boundaries (`os.getenv` exists in C, Python, and Ruby
/// shells; `process.env` shows up in TS-flavoured tooling).
pub fn extract_config_reads(
    source: &str,
    module_id: NodeId,
    repo: RepoId,
) -> ConfigNodes {
    let mut names = Vec::new();
    names.extend(scan_python_env(source));
    names.extend(scan_js_process_env(source));
    names.extend(scan_rust_env(source));
    names.extend(scan_go_env(source));
    names.extend(scan_ruby_env(source));
    names.extend(scan_java_system_getenv(source));
    names.extend(scan_php_env(source));
    build_nodes(names, Side::Read, module_id, repo)
}

/// Extract env-var definitions from a Dockerfile.
pub fn extract_dockerfile_defs(
    source: &str,
    module_id: NodeId,
    repo: RepoId,
) -> ConfigNodes {
    build_nodes(scan_dockerfile_env(source), Side::Define, module_id, repo)
}

/// Extract env-var definitions from a `.env`-style key/value file.
pub fn extract_dotenv_defs(
    source: &str,
    module_id: NodeId,
    repo: RepoId,
) -> ConfigNodes {
    build_nodes(scan_dotenv(source), Side::Define, module_id, repo)
}

/// Extract env-var definitions from a YAML manifest (k8s `env:`/`envFrom:`,
/// docker-compose `environment:`). Caller is responsible for content-gating
/// or path-gating to YAML files.
pub fn extract_yaml_env_defs(
    source: &str,
    module_id: NodeId,
    repo: RepoId,
) -> ConfigNodes {
    let mut names = Vec::new();
    names.extend(scan_k8s_env(source));
    names.extend(scan_compose_environment(source));
    build_nodes(names, Side::Define, module_id, repo)
}

fn build_nodes(
    names: Vec<String>,
    side: Side,
    module_id: NodeId,
    repo: RepoId,
) -> ConfigNodes {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut nav = CodeNav::default();
    let mut seen = std::collections::HashSet::new();

    let category = match side {
        Side::Read => edge_category::READS_CONFIG,
        Side::Define => edge_category::DEFINES_CONFIG,
    };

    for name in names {
        if !is_valid_env_name(&name) {
            continue;
        }
        if !seen.insert(name.clone()) {
            continue;
        }
        let qname = format!("config:env:{name}");
        let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::CONFIG_KEY, &qname);
        nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Medium,
            cells: vec![],
        });
        nav.record(id, &name, &qname, node_kind::CONFIG_KEY, Some(module_id));
        edges.push(Edge {
            from: module_id,
            to: id,
            category,
            confidence: Confidence::Medium,
        });
    }

    ConfigNodes { nodes, edges, nav }
}

/// True if `s` is a plausible env-var name: nonempty, ≤ 128, leading char is
/// alpha/underscore, rest is alpha/digit/underscore. Rejects strings that
/// could leak through quoted-arg scans (e.g. integers, paths, format
/// templates).
fn is_valid_env_name(s: &str) -> bool {
    if s.is_empty() || s.len() > 128 {
        return false;
    }
    let first = s.as_bytes()[0];
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return false;
    }
    s.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

// ----------------------------------------------------------------------------
// Code-side scanners — one per language idiom.
// ----------------------------------------------------------------------------

fn scan_python_env(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    // os.environ['X'], os.environ["X"], os.environ.get('X', ...), os.getenv('X')
    for needle in [
        "os.environ[",
        "os.environ.get(",
        "os.getenv(",
        "environ[",
        "environ.get(",
        "getenv(",
    ] {
        for hit in capture_first_string_arg(source, needle) {
            out.push(hit);
        }
    }
    out
}

fn scan_js_process_env(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    // process.env.VAR — bare property access. Identifier-shaped name follows.
    let bytes = source.as_bytes();
    for needle in ["process.env.", "import.meta.env."] {
        let mut search_from = 0;
        while let Some(rel) = source[search_from..].find(needle) {
            let pos = search_from + rel + needle.len();
            // Read identifier chars.
            let mut j = pos;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            if j > pos {
                out.push(source[pos..j].to_string());
            }
            search_from = pos.max(search_from + needle.len());
        }
    }
    // process.env['VAR'] / process.env["VAR"] — bracketed string-literal form.
    for needle in ["process.env[", "import.meta.env["] {
        for hit in capture_first_string_arg(source, needle) {
            out.push(hit);
        }
    }
    out
}

fn scan_rust_env(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for needle in ["std::env::var(", "env::var("] {
        for hit in capture_first_string_arg(source, needle) {
            out.push(hit);
        }
    }
    out
}

fn scan_go_env(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for needle in ["os.Getenv(", "os.LookupEnv("] {
        for hit in capture_first_string_arg(source, needle) {
            out.push(hit);
        }
    }
    out
}

fn scan_ruby_env(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    // ENV['X'] / ENV["X"] / ENV.fetch('X', ...)
    for needle in ["ENV[", "ENV.fetch(", "ENV.fetch!("] {
        for hit in capture_first_string_arg(source, needle) {
            out.push(hit);
        }
    }
    out
}

fn scan_java_system_getenv(source: &str) -> Vec<String> {
    capture_first_string_arg(source, "System.getenv(")
}

fn scan_php_env(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    // getenv('X') and $_ENV['X']
    for needle in ["getenv(", "$_ENV[", "$_SERVER["] {
        for hit in capture_first_string_arg(source, needle) {
            out.push(hit);
        }
    }
    out
}

// ----------------------------------------------------------------------------
// Source-side scanners — Dockerfile, .env, YAML.
// ----------------------------------------------------------------------------

fn scan_dockerfile_env(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in source.lines() {
        let trimmed = strip_dockerfile_comment(line.trim_start());
        let upper_prefix = trimmed.get(..4).unwrap_or("").to_ascii_uppercase();
        if upper_prefix != "ENV " {
            continue;
        }
        let rest = trimmed[4..].trim();
        // Two forms: `ENV KEY=value [KEY2=value2 ...]` and `ENV KEY value`.
        if rest.contains('=') {
            // Multiple KEY=value pairs, possibly quoted values.
            for tok in dockerfile_env_pairs(rest) {
                out.push(tok);
            }
        } else if let Some(name) = rest.split_whitespace().next() {
            out.push(name.to_string());
        }
    }
    out
}

fn strip_dockerfile_comment(s: &str) -> &str {
    s.splitn(2, '#').next().unwrap_or(s).trim()
}

/// Split `KEY=value KEY2="quoted value" KEY3=value3` into the KEY tokens.
fn dockerfile_env_pairs(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Skip leading whitespace.
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        // Read KEY (alphanumeric / underscore).
        let key_start = i;
        while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
            i += 1;
        }
        if i > key_start {
            out.push(s[key_start..i].to_string());
        }
        // Skip past `=value` (handle quoted). Move to next whitespace at depth 0.
        if i < bytes.len() && bytes[i] == b'=' {
            i += 1;
            if i < bytes.len() && (bytes[i] == b'"' || bytes[i] == b'\'') {
                let delim = bytes[i];
                i += 1;
                while i < bytes.len() && bytes[i] != delim {
                    if bytes[i] == b'\\' && i + 1 < bytes.len() {
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                if i < bytes.len() {
                    i += 1;
                }
            } else {
                while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
            }
        } else {
            // No `=`, abort — single-arg form was handled by caller.
            break;
        }
    }
    out
}

fn scan_dotenv(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in source.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        // Optional `export ` prefix.
        let t = t.strip_prefix("export ").unwrap_or(t);
        if let Some(eq) = t.find('=') {
            let key = t[..eq].trim();
            if !key.is_empty() {
                out.push(key.to_string());
            }
        }
    }
    out
}

fn scan_k8s_env(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_env_block = false;
    let mut env_indent: usize = 0;

    for line in source.lines() {
        let indent = line.len() - line.trim_start().len();
        let t = line.trim();
        let t_stripped = t.strip_prefix("- ").unwrap_or(t);

        if t_stripped == "env:" || t_stripped.starts_with("env:") && t_stripped.ends_with(":") {
            in_env_block = true;
            env_indent = indent;
            continue;
        }
        if in_env_block && indent <= env_indent && !t.is_empty() {
            // Left the env block — but only if this line isn't indented past it.
            // A list item under env: starts with `- name:` and its indent is >
            // env_indent. A new sibling key resets state.
            if !t.starts_with('-') {
                in_env_block = false;
            }
        }
        if in_env_block {
            // `- name: KEY` or `name: KEY` (after stripping `- `).
            if let Some(rest) = t_stripped.strip_prefix("name:") {
                let v = rest.trim().trim_matches(|c| c == '"' || c == '\'');
                if !v.is_empty() {
                    out.push(v.to_string());
                }
            }
        }
    }
    out
}

fn scan_compose_environment(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_env_block = false;
    let mut env_indent: usize = 0;
    let mut block_is_list = false;

    for line in source.lines() {
        let indent = line.len() - line.trim_start().len();
        let t = line.trim();

        if t == "environment:" {
            in_env_block = true;
            env_indent = indent;
            block_is_list = false;
            continue;
        }
        if in_env_block {
            if t.is_empty() {
                continue;
            }
            if indent <= env_indent {
                in_env_block = false;
                continue;
            }
            // List form: `- KEY=value` or `- KEY`. Map form: `KEY: value`.
            if let Some(item) = t.strip_prefix("- ") {
                block_is_list = true;
                let key = item.split('=').next().unwrap_or(item).trim();
                if !key.is_empty() {
                    out.push(key.to_string());
                }
            } else if !block_is_list {
                if let Some(colon) = t.find(':') {
                    let key = t[..colon].trim();
                    if !key.is_empty() {
                        out.push(key.to_string());
                    }
                }
            }
        }
    }
    out
}

// ----------------------------------------------------------------------------
// Helpers
// ----------------------------------------------------------------------------

/// For every occurrence of `needle` in `source`, read the first quoted string
/// literal that follows the needle and push its inner text. `needle` should
/// end at the position immediately before the value (after `(`, `[`, etc.).
fn capture_first_string_arg(source: &str, needle: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut search_from = 0;
    while let Some(rel) = source[search_from..].find(needle) {
        let pos = search_from + rel;
        let after = &source[pos + needle.len()..];
        if let Some(name) = first_quoted(after) {
            out.push(name);
        }
        search_from = pos + needle.len();
    }
    out
}

fn first_quoted(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut i = 0;
    // Allow leading whitespace before the quote.
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    if i >= bytes.len() {
        return None;
    }
    let c = bytes[i];
    if c != b'\'' && c != b'"' && c != b'`' {
        return None;
    }
    let delim = c;
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
        Some(s[start..j].to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module_id(repo: RepoId) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MODULE, "test")
    }

    fn config_keys(out: &ConfigNodes) -> Vec<String> {
        out.nav.qname_by_id.values().cloned().collect()
    }

    #[test]
    fn python_env_reads_all_idioms() {
        let repo = RepoId(1);
        let src = r#"
import os
db_url = os.environ['DATABASE_URL']
key = os.environ.get("API_KEY", "default")
secret = os.getenv('JWT_SECRET')
"#;
        let out = extract_config_reads(src, module_id(repo), repo);
        let keys = config_keys(&out);
        assert!(keys.contains(&"config:env:DATABASE_URL".to_string()));
        assert!(keys.contains(&"config:env:API_KEY".to_string()));
        assert!(keys.contains(&"config:env:JWT_SECRET".to_string()));
    }

    #[test]
    fn js_process_env_dot_and_bracket() {
        let repo = RepoId(1);
        let src = r#"
const db = process.env.DATABASE_URL;
const key = process.env['API_KEY'];
const flag = import.meta.env.VITE_FEATURE_X;
"#;
        let out = extract_config_reads(src, module_id(repo), repo);
        let keys = config_keys(&out);
        assert!(keys.contains(&"config:env:DATABASE_URL".to_string()));
        assert!(keys.contains(&"config:env:API_KEY".to_string()));
        assert!(keys.contains(&"config:env:VITE_FEATURE_X".to_string()));
    }

    #[test]
    fn rust_env_var() {
        let repo = RepoId(1);
        let src = r#"
let url = std::env::var("DATABASE_URL").unwrap();
let key = env::var("API_KEY").ok();
"#;
        let out = extract_config_reads(src, module_id(repo), repo);
        let keys = config_keys(&out);
        assert!(keys.contains(&"config:env:DATABASE_URL".to_string()));
        assert!(keys.contains(&"config:env:API_KEY".to_string()));
    }

    #[test]
    fn go_env_lookups() {
        let repo = RepoId(1);
        let src = r#"
url := os.Getenv("DATABASE_URL")
key, ok := os.LookupEnv("API_KEY")
"#;
        let out = extract_config_reads(src, module_id(repo), repo);
        let keys = config_keys(&out);
        assert!(keys.contains(&"config:env:DATABASE_URL".to_string()));
        assert!(keys.contains(&"config:env:API_KEY".to_string()));
    }

    #[test]
    fn ruby_env_brackets_and_fetch() {
        let repo = RepoId(1);
        let src = r#"
db = ENV['DATABASE_URL']
key = ENV.fetch('API_KEY')
secret = ENV.fetch!('REQUIRED_SECRET')
"#;
        let out = extract_config_reads(src, module_id(repo), repo);
        let keys = config_keys(&out);
        assert!(keys.contains(&"config:env:DATABASE_URL".to_string()));
        assert!(keys.contains(&"config:env:API_KEY".to_string()));
        assert!(keys.contains(&"config:env:REQUIRED_SECRET".to_string()));
    }

    #[test]
    fn java_system_getenv() {
        let repo = RepoId(1);
        let src = r#"
String url = System.getenv("DATABASE_URL");
String key = System.getenv("API_KEY");
"#;
        let out = extract_config_reads(src, module_id(repo), repo);
        let keys = config_keys(&out);
        assert!(keys.contains(&"config:env:DATABASE_URL".to_string()));
        assert!(keys.contains(&"config:env:API_KEY".to_string()));
    }

    #[test]
    fn php_env_idioms() {
        let repo = RepoId(1);
        let src = r#"
<?php
$db = getenv('DATABASE_URL');
$key = $_ENV['API_KEY'];
"#;
        let out = extract_config_reads(src, module_id(repo), repo);
        let keys = config_keys(&out);
        assert!(keys.contains(&"config:env:DATABASE_URL".to_string()));
        assert!(keys.contains(&"config:env:API_KEY".to_string()));
    }

    #[test]
    fn rejects_invalid_env_names() {
        let repo = RepoId(1);
        // `process.env['1bad']` — leading digit; `process.env['has-dash']` —
        // hyphen. Both fail `is_valid_env_name`.
        let src = r#"
const a = process.env['1bad'];
const b = process.env['has-dash'];
const c = process.env.GOOD_NAME;
"#;
        let out = extract_config_reads(src, module_id(repo), repo);
        let keys = config_keys(&out);
        assert!(keys.contains(&"config:env:GOOD_NAME".to_string()));
        assert!(!keys.contains(&"config:env:1bad".to_string()));
        assert!(!keys.contains(&"config:env:has-dash".to_string()));
    }

    #[test]
    fn dockerfile_env_kv_and_split() {
        let repo = RepoId(1);
        let src = r#"
FROM python:3.11
ENV PYTHONUNBUFFERED=1
ENV DATABASE_URL=postgres://localhost/app
ENV LOG_LEVEL info
ENV NODE_ENV=production PORT=3000
# ENV COMMENTED=should-not-match
"#;
        let out = extract_dockerfile_defs(src, module_id(repo), repo);
        let keys = config_keys(&out);
        assert!(keys.contains(&"config:env:PYTHONUNBUFFERED".to_string()));
        assert!(keys.contains(&"config:env:DATABASE_URL".to_string()));
        assert!(keys.contains(&"config:env:LOG_LEVEL".to_string()));
        assert!(keys.contains(&"config:env:NODE_ENV".to_string()));
        assert!(keys.contains(&"config:env:PORT".to_string()));
        assert!(!keys.contains(&"config:env:COMMENTED".to_string()));
    }

    #[test]
    fn dotenv_basic() {
        let repo = RepoId(1);
        let src = r#"
# database
DATABASE_URL=postgres://localhost/app
API_KEY="secret-value"
export NODE_ENV=production

# blank line above
EMPTY=
"#;
        let out = extract_dotenv_defs(src, module_id(repo), repo);
        let keys = config_keys(&out);
        assert!(keys.contains(&"config:env:DATABASE_URL".to_string()));
        assert!(keys.contains(&"config:env:API_KEY".to_string()));
        assert!(keys.contains(&"config:env:NODE_ENV".to_string()));
        assert!(keys.contains(&"config:env:EMPTY".to_string()));
    }

    #[test]
    fn k8s_env_block_extracts_names() {
        let repo = RepoId(1);
        let src = r#"
apiVersion: apps/v1
kind: Deployment
spec:
  template:
    spec:
      containers:
      - name: api
        env:
        - name: DATABASE_URL
          value: postgres://db
        - name: LOG_LEVEL
          valueFrom:
            configMapKeyRef:
              name: log-config
              key: level
"#;
        let out = extract_yaml_env_defs(src, module_id(repo), repo);
        let keys = config_keys(&out);
        assert!(keys.contains(&"config:env:DATABASE_URL".to_string()));
        assert!(keys.contains(&"config:env:LOG_LEVEL".to_string()));
    }

    #[test]
    fn compose_environment_list_form() {
        let repo = RepoId(1);
        let src = r#"
services:
  api:
    image: api:latest
    environment:
      - DATABASE_URL=postgres://db
      - NODE_ENV=production
      - PORT
"#;
        let out = extract_yaml_env_defs(src, module_id(repo), repo);
        let keys = config_keys(&out);
        assert!(keys.contains(&"config:env:DATABASE_URL".to_string()));
        assert!(keys.contains(&"config:env:NODE_ENV".to_string()));
        assert!(keys.contains(&"config:env:PORT".to_string()));
    }

    #[test]
    fn compose_environment_map_form() {
        let repo = RepoId(1);
        let src = r#"
services:
  api:
    environment:
      DATABASE_URL: postgres://db
      LOG_LEVEL: info
"#;
        let out = extract_yaml_env_defs(src, module_id(repo), repo);
        let keys = config_keys(&out);
        assert!(keys.contains(&"config:env:DATABASE_URL".to_string()));
        assert!(keys.contains(&"config:env:LOG_LEVEL".to_string()));
    }
}
