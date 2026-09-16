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

use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, cell_type, edge_category, node_kind};
use repo_graph_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};

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

/// One env-var occurrence on its way to a `CONFIG_KEY` node. The define side
/// carries the literal value it declared; the read side never can, because
/// `os.environ["X"]` states no value (A13.7).
struct ConfigDef {
    name: String,
    value: Option<String>,
    /// Which syntax produced this — `dockerfile` | `dotenv` | `k8s` |
    /// `compose`, empty on the read side. Discriminates the ENV-cell payload
    /// so one cell type can carry several provenances without ambiguity.
    source: &'static str,
}

impl ConfigDef {
    /// A name with no value attached (read sites, k8s `valueFrom:`, bare
    /// compose `- KEY` passthrough).
    fn bare(name: impl Into<String>, source: &'static str) -> Self {
        ConfigDef { name: name.into(), value: None, source }
    }
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
    let defs = names.into_iter().map(|n| ConfigDef::bare(n, "")).collect();
    build_nodes(defs, Side::Read, module_id, repo)
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
    let mut defs = scan_k8s_env(source);
    defs.extend(scan_compose_environment(source));
    build_nodes(defs, Side::Define, module_id, repo)
}

fn build_nodes(
    defs: Vec<ConfigDef>,
    side: Side,
    module_id: NodeId,
    repo: RepoId,
) -> ConfigNodes {
    let mut nodes: Vec<Node> = Vec::new();
    let mut edges = Vec::new();
    let mut nav = CodeNav::default();
    // name -> index into `nodes`, so a repeat of a key that arrived value-less
    // can still be upgraded by a later definition that has one.
    let mut seen: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let (mut valued, mut redacted) = (0usize, 0usize);

    let category = match side {
        Side::Read => edge_category::READS_CONFIG,
        Side::Define => edge_category::DEFINES_CONFIG,
    };

    for def in defs {
        if !is_valid_env_name(&def.name) {
            continue;
        }
        // The read side states no value, so it never attaches a cell.
        let cell = match side {
            Side::Read => None,
            Side::Define => env_cell(&def),
        };
        if let Some(&idx) = seen.get(&def.name) {
            // First definition wins the node; the first non-None value wins
            // the cell (`.env` then `.env.local`, k8s then compose).
            if let (true, Some(c)) = (nodes[idx].cells.is_empty(), cell) {
                if c.1 {
                    redacted += 1;
                }
                valued += 1;
                nodes[idx].cells.push(c.0);
            }
            continue;
        }
        let qname = format!("config:env:{}", def.name);
        let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::CONFIG_KEY, &qname);
        let cells = match cell {
            Some((c, was_redacted)) => {
                valued += 1;
                if was_redacted {
                    redacted += 1;
                }
                vec![c]
            }
            None => vec![],
        };
        seen.insert(def.name.clone(), nodes.len());
        nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Medium,
            cells,
        });
        nav.record(id, &def.name, &qname, node_kind::CONFIG_KEY, Some(module_id));
        edges.push(Edge {
            from: module_id,
            to: id,
            category,
            confidence: Confidence::Medium,
        });
    }

    if matches!(side, Side::Define) && !nodes.is_empty() {
        eprintln!(
            "[config] defined={} valued={valued} redacted={redacted}",
            nodes.len()
        );
    }

    ConfigNodes { nodes, edges, nav }
}

/// Longest value kept on an ENV cell. A `.gmap` is a shipped artefact; a
/// multi-kilobyte inlined cert or JSON blob is noise there, not signal.
const VALUE_CAP: usize = 200;

/// Env-var names whose value must never reach the graph. Substring, not exact:
/// `STRIPE_SECRET_KEY`, `JWT_TOKEN` and `DB_PASSWD` all have to hit.
const SECRET_NEEDLES: [&str; 12] = [
    "SECRET", "PASSWORD", "PASSWD", "TOKEN", "APIKEY", "API_KEY", "PRIVATE_KEY",
    "CREDENTIAL", "ACCESS_KEY", "SESSION_KEY", "SALT", "SIGNING",
];

fn is_secret_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    SECRET_NEEDLES.iter().any(|needle| upper.contains(needle))
}

/// `scheme://user:pass@host/path` -> `scheme://***@host/path`. The HOST is
/// deliberately KEPT: area A11 pairs a config value to a service by hostname,
/// and a fully scrubbed URL would be unpairable. Only userinfo that actually
/// carries a `:` password is masked, and only inside the authority (an `@`
/// after the first `/`, `?` or `#` belongs to the path, not to credentials).
fn mask_userinfo(value: &str) -> Option<String> {
    let scheme_end = value.find("://")? + 3;
    let rest = &value[scheme_end..];
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let at = rest[..authority_end].rfind('@')?;
    if !rest[..at].contains(':') {
        return None;
    }
    Some(format!("{}***{}", &value[..scheme_end], &rest[at..]))
}

/// The ENV cell for one define-side declaration, plus whether the stored
/// payload had to be altered. Returns `None` when the site declared no value.
///
/// REDACTION IS NOT OPTIONAL. `.gmap` files are written to `.ai/repo-graph/`,
/// shipped to consumers, and rendered verbatim into the dense text handed to
/// an LLM (projection-text renders every cell), so a secret-named key records
/// only that a value exists — never the value itself.
fn env_cell(def: &ConfigDef) -> Option<(Cell, bool)> {
    let raw = def.value.as_deref()?;
    let (stored, redacted) = if is_secret_name(&def.name) {
        (None, true)
    } else {
        let mut redacted = false;
        let mut v = match mask_userinfo(raw) {
            Some(masked) => {
                redacted = true;
                masked
            }
            None => raw.to_string(),
        };
        if v.chars().count() > VALUE_CAP {
            v = v.chars().take(VALUE_CAP).collect();
            redacted = true;
        }
        (Some(v), redacted)
    };
    let json = match stored {
        Some(v) => format!(
            "{{\"value\":\"{}\",\"source\":\"{}\",\"redacted\":{redacted}}}",
            escape_json(&v),
            def.source
        ),
        None => format!("{{\"source\":\"{}\",\"redacted\":true}}", def.source),
    };
    Some((
        Cell {
            kind: cell_type::ENV,
            payload: CellPayload::Json(json),
        },
        redacted,
    ))
}

/// JSON string-body escaping without pulling serde into the hot path (the
/// cron.rs idiom, extended to drop control characters — a `.env` value can
/// legally carry a tab, and a raw control byte is invalid JSON).
fn escape_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// Normalise a raw right-hand side into a stored value: trim, strip one layer
/// of matching quotes, drop a trailing ` # comment` on an unquoted value, and
/// report `None` for an empty one so `KEY=` stays value-less.
fn clean_value(raw: &str) -> Option<String> {
    let t = raw.trim();
    let b = t.as_bytes();
    let quoted = b.len() >= 2
        && (b[0] == b'"' || b[0] == b'\'')
        && b[b.len() - 1] == b[0];
    let t = if quoted {
        &t[1..t.len() - 1]
    } else {
        // Whitespace-delimited so a URL fragment (`http://h/#frag`) survives.
        match t.find(" #") {
            Some(i) => &t[..i],
            None => t,
        }
    };
    let t = t.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
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

fn scan_dockerfile_env(source: &str) -> Vec<ConfigDef> {
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
            out.extend(dockerfile_env_pairs(rest));
        } else if let Some(name) = rest.split_whitespace().next() {
            // `ENV KEY the rest of the line` — everything after the key is the
            // value, spaces included.
            out.push(ConfigDef {
                name: name.to_string(),
                value: clean_value(&rest[name.len()..]),
                source: "dockerfile",
            });
        }
    }
    out
}

fn strip_dockerfile_comment(s: &str) -> &str {
    s.splitn(2, '#').next().unwrap_or(s).trim()
}

/// Split `KEY=value KEY2="quoted value" KEY3=value3` into KEY/value pairs.
/// The walk already had to find each value's end to locate the next key; A13.7
/// only keeps the span it was skipping.
fn dockerfile_env_pairs(s: &str) -> Vec<ConfigDef> {
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
        let key_end = i;
        // Walk past `=value` (handle quoted). Move to next whitespace at depth 0.
        let mut value = None;
        let had_eq = i < bytes.len() && bytes[i] == b'=';
        if had_eq {
            i += 1;
            let value_start = i;
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
            value = clean_value(&s[value_start..i]);
        }
        if key_end > key_start {
            out.push(ConfigDef {
                name: s[key_start..key_end].to_string(),
                value,
                source: "dockerfile",
            });
        }
        if !had_eq {
            // No `=`, abort — single-arg form was handled by caller.
            break;
        }
    }
    out
}

fn scan_dotenv(source: &str) -> Vec<ConfigDef> {
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
                out.push(ConfigDef {
                    name: key.to_string(),
                    value: clean_value(&t[eq + 1..]),
                    source: "dotenv",
                });
            }
        }
    }
    out
}

fn scan_k8s_env(source: &str) -> Vec<ConfigDef> {
    let mut out: Vec<ConfigDef> = Vec::new();
    let mut in_env_block = false;
    let mut env_indent: usize = 0;
    // Column of the `name:` key of the list item currently being read, so a
    // `value:` line is only paired when it is that key's SIBLING. A
    // `valueFrom:`/`secretRef:` item has no sibling `value:` and so stays
    // value-less, and the `name:` nested inside a `secretKeyRef:` cannot
    // capture the outer item's value.
    let mut key_col: Option<usize> = None;

    for line in source.lines() {
        let indent = line.len() - line.trim_start().len();
        let t = line.trim();
        let t_stripped = t.strip_prefix("- ").unwrap_or(t);

        if t_stripped == "env:" || t_stripped.starts_with("env:") && t_stripped.ends_with(":") {
            in_env_block = true;
            env_indent = indent;
            key_col = None;
            continue;
        }
        if in_env_block && indent <= env_indent && !t.is_empty() {
            // Left the env block — but only if this line isn't indented past it.
            // A list item under env: starts with `- name:` and its indent is >
            // env_indent. A new sibling key resets state.
            if !t.starts_with('-') {
                in_env_block = false;
                key_col = None;
            }
        }
        if in_env_block {
            // `- name: KEY` or `name: KEY` (after stripping `- `).
            if let Some(rest) = t_stripped.strip_prefix("name:") {
                let v = rest.trim().trim_matches(|c| c == '"' || c == '\'');
                if !v.is_empty() {
                    key_col = Some(indent + (t.len() - t_stripped.len()));
                    out.push(ConfigDef::bare(v, "k8s"));
                }
            } else if let Some(rest) = t_stripped.strip_prefix("value:")
                && key_col == Some(indent)
                && let Some(last) = out.last_mut()
                && last.value.is_none()
            {
                last.value = clean_value(rest);
            }
        }
    }
    out
}

fn scan_compose_environment(source: &str) -> Vec<ConfigDef> {
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
                // `- KEY=value`, or `- KEY` (pass the host's value through).
                let (key, value) = match item.find('=') {
                    Some(eq) => (item[..eq].trim(), clean_value(&item[eq + 1..])),
                    None => (item.trim(), None),
                };
                if !key.is_empty() {
                    out.push(ConfigDef { name: key.to_string(), value, source: "compose" });
                }
            } else if !block_is_list {
                // Map form: `KEY: value`.
                if let Some(colon) = t.find(':') {
                    let key = t[..colon].trim();
                    if !key.is_empty() {
                        out.push(ConfigDef {
                            name: key.to_string(),
                            value: clean_value(&t[colon + 1..]),
                            source: "compose",
                        });
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

    // ------------------------------------------------------------------
    // A13.7 — define-side VALUES on an ENV cell, with mandatory redaction.
    // ------------------------------------------------------------------

    /// The ENV-cell payload for `name`, or `None` if that key carries no ENV
    /// cell. This is the exact shape area A11 consumes via `node_cells`.
    fn env_payload(out: &ConfigNodes, name: &str) -> Option<String> {
        let want = format!("config:env:{name}");
        let id = *out
            .nav
            .qname_by_id
            .iter()
            .find(|(_, q)| **q == want)
            .map(|(id, _)| id)?;
        let node = out.nodes.iter().find(|n| n.id == id)?;
        node.cells
            .iter()
            .find(|c| c.kind == cell_type::ENV)
            .map(|c| match &c.payload {
                CellPayload::Text(s) | CellPayload::Json(s) => s.clone(),
                CellPayload::Bytes(_) => String::new(),
            })
    }

    #[test]
    fn dotenv_value_lands_on_env_cell() {
        let repo = RepoId(1);
        let src = "API_URL=http://users-svc:8080\nexport FEATURE_FLAG=\"on\"\nEMPTY=\n";
        let out = extract_dotenv_defs(src, module_id(repo), repo);
        assert_eq!(
            env_payload(&out, "API_URL").as_deref(),
            Some(r#"{"value":"http://users-svc:8080","source":"dotenv","redacted":false}"#)
        );
        // `export ` prefix stripped and one layer of quotes removed.
        assert_eq!(
            env_payload(&out, "FEATURE_FLAG").as_deref(),
            Some(r#"{"value":"on","source":"dotenv","redacted":false}"#)
        );
        // `KEY=` declares the key but no value — node stays cell-free.
        assert_eq!(env_payload(&out, "EMPTY"), None);
    }

    #[test]
    fn dockerfile_env_multi_pair_values() {
        let repo = RepoId(1);
        let src = concat!(
            "FROM python:3.11\n",
            "ENV QUEUE_URL=amqp://rabbit:5672/ LOG_LEVEL=debug\n",
            "ENV GREETING=\"hello world\"\n",
            "ENV APP_HOME /srv/app\n",
        );
        let out = extract_dockerfile_defs(src, module_id(repo), repo);
        assert!(env_payload(&out, "QUEUE_URL").unwrap().contains(r#""value":"amqp://rabbit:5672/""#));
        // The SECOND pair on the same line keeps its own value.
        assert!(env_payload(&out, "LOG_LEVEL").unwrap().contains(r#""value":"debug""#));
        assert!(env_payload(&out, "GREETING").unwrap().contains(r#""value":"hello world""#));
        // Space form: everything after the key is the value.
        assert!(env_payload(&out, "APP_HOME").unwrap().contains(r#""value":"/srv/app""#));
        assert!(env_payload(&out, "QUEUE_URL").unwrap().contains(r#""source":"dockerfile""#));
    }

    #[test]
    fn k8s_env_value_from_sibling_line() {
        let repo = RepoId(1);
        let src = concat!(
            "spec:\n",
            "  containers:\n",
            "    - name: api\n",
            "      env:\n",
            "        - name: CACHE_HOST\n",
            "          value: redis.svc.cluster.local:6379\n",
            "        - name: DB_PASSWORD\n",
            "          valueFrom:\n",
            "            secretKeyRef:\n",
            "              name: db-secret\n",
            "              key: password\n",
        );
        let out = extract_yaml_env_defs(src, module_id(repo), repo);
        assert!(
            env_payload(&out, "CACHE_HOST")
                .unwrap()
                .contains(r#""value":"redis.svc.cluster.local:6379""#)
        );
        assert!(env_payload(&out, "CACHE_HOST").unwrap().contains(r#""source":"k8s""#));
        // `valueFrom:` declares no literal, so DB_PASSWORD gets no cell — and
        // the nested `secretKeyRef.name` must not capture the item's value.
        assert_eq!(env_payload(&out, "DB_PASSWORD"), None);
    }

    #[test]
    fn compose_list_and_map_values() {
        let repo = RepoId(1);
        let src = concat!(
            "services:\n",
            "  api:\n",
            "    environment:\n",
            "      - DATABASE_HOST=db.internal\n",
            "      - PORT\n",
        );
        let out = extract_yaml_env_defs(src, module_id(repo), repo);
        assert!(env_payload(&out, "DATABASE_HOST").unwrap().contains(r#""source":"compose""#));
        // Passthrough form `- KEY` names a key whose value comes from the host.
        assert_eq!(env_payload(&out, "PORT"), None);

        let map_src = concat!(
            "services:\n",
            "  api:\n",
            "    environment:\n",
            "      LOG_LEVEL: info\n",
        );
        let out = extract_yaml_env_defs(map_src, module_id(repo), repo);
        assert!(env_payload(&out, "LOG_LEVEL").unwrap().contains(r#""value":"info""#));
    }

    #[test]
    fn secret_named_key_is_redacted() {
        let repo = RepoId(1);
        let src = concat!(
            "STRIPE_SECRET_KEY=sk_live_4eC39HqLyjWDarjtT1zdp7dc\n",
            "JWT_TOKEN=eyJhbGciOiJIUzI1NiJ9\n",
            "DB_PASSWD=hunter2\n",
            "SIGNING_SALT=abc\n",
        );
        let out = extract_dotenv_defs(src, module_id(repo), repo);
        for key in ["STRIPE_SECRET_KEY", "JWT_TOKEN", "DB_PASSWD", "SIGNING_SALT"] {
            let payload = env_payload(&out, key).unwrap_or_else(|| panic!("{key} has no cell"));
            assert_eq!(payload, r#"{"source":"dotenv","redacted":true}"#);
        }
        // Belt and braces: not one secret byte reached the graph.
        let all: String = out
            .nodes
            .iter()
            .flat_map(|n| n.cells.iter())
            .map(|c| match &c.payload {
                CellPayload::Text(s) | CellPayload::Json(s) => s.clone(),
                CellPayload::Bytes(_) => String::new(),
            })
            .collect();
        for leak in ["sk_live_", "eyJhbGci", "hunter2", "abc"] {
            assert!(!all.contains(leak), "{leak} leaked into a cell: {all}");
        }
    }

    #[test]
    fn db_url_userinfo_is_masked_host_kept() {
        let repo = RepoId(1);
        let src = concat!(
            "DATABASE_URL=postgres://appuser:s3cr3t@db.internal:5432/app\n",
            "REDIS_URL=redis://:p4ss@cache.svc:6379/0\n",
            "PLAIN_URL=http://users-svc:8080/health#top\n",
        );
        let out = extract_dotenv_defs(src, module_id(repo), repo);
        let db = env_payload(&out, "DATABASE_URL").unwrap();
        assert!(db.contains(r#""value":"postgres://***@db.internal:5432/app""#), "{db}");
        assert!(db.contains(r#""redacted":true"#), "{db}");
        assert!(!db.contains("s3cr3t"), "{db}");
        let redis = env_payload(&out, "REDIS_URL").unwrap();
        assert!(redis.contains(r#""value":"redis://***@cache.svc:6379/0""#), "{redis}");
        // No credentials => untouched, and an `#` inside the path is not a
        // comment.
        let plain = env_payload(&out, "PLAIN_URL").unwrap();
        assert!(plain.contains(r#""value":"http://users-svc:8080/health#top""#), "{plain}");
        assert!(plain.contains(r#""redacted":false"#), "{plain}");
    }

    #[test]
    fn long_value_is_capped_and_flagged() {
        let repo = RepoId(1);
        let src = format!("BLOB={}\n", "x".repeat(500));
        let out = extract_dotenv_defs(&src, module_id(repo), repo);
        let payload = env_payload(&out, "BLOB").unwrap();
        assert!(payload.contains(&format!(r#""value":"{}""#, "x".repeat(VALUE_CAP))));
        assert!(payload.contains(r#""redacted":true"#));
    }

    #[test]
    fn read_side_config_key_has_no_env_cell() {
        let repo = RepoId(1);
        let src = "import os\nurl = os.environ['API_URL']\nkey = os.getenv('DB_PASSWORD')\n";
        let out = extract_config_reads(src, module_id(repo), repo);
        assert_eq!(out.nodes.len(), 2);
        assert!(
            out.nodes.iter().all(|n| n.cells.is_empty()),
            "a read site states no value, so it must attach no cell"
        );
    }
}
