//! Config / env-var extraction (v0.4.x — task #8).
//!
//! Emits `CONFIG_KEY` nodes per unique env-var name with two edge flavours:
//!
//!   - `READS_CONFIG`  — code module → key  (`os.environ['DB_URL']` etc.);
//!     the engine then re-homes it to the innermost FUNCTION / METHOD holding
//!     the read (LE.4b, `anchor::rehome_to_owner` over [`ConfigNodes::sites`]),
//!     so only a read at module scope keeps the module edge.
//!   - `DEFINES_CONFIG` — source module → key (Dockerfile `ENV`, `.env`, k8s)
//!
//! Single qname per name across the merged graph: `config:<flavor>:<rest>`.
//! [`Flavor`] picks the segment and the name validator:
//!
//!   - `config:env:<NAME>` — an environment variable (everything below).
//!   - `config:secret:<provider>/<ref>` — a secrets-manager reference (A13.8):
//!     `vault`, `aws_sm`, `gcp_sm`, `azure_kv` from code (`secrets_flags.rs`),
//!     `k8s` from manifests here. The provider is joined with `/`, not a fourth
//!     `:` segment, so anything reasoning about `config:<flavor>:<rest>` holds.
//!   - `config:flag:<key>` — a feature flag (A13.8). No provider in the qname:
//!     an SDK check (LaunchDarkly, Unleash, ...) and a Flipt `flags:` file
//!     declaring the same key must pair across repos (`ConfigResolver` pairs on
//!     the full qname). The provider rides the ENV cell's `source` instead.
//!
//! A `config:file:database.yml` track stays reserved.
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
//!     - k8s YAML `env: - name: KEY`
//!     - docker-compose `environment: - KEY=value`
//!
//!   **Secrets and flags in YAML (A13.8, same `extract_yaml_env_defs` walk):**
//!     - READ  `config:secret:k8s/<name>` — a pod's `secretKeyRef: {name:}`
//!       and `envFrom: - secretRef: {name:}` name the Secret they consume.
//!     - DEFINE `config:secret:k8s/<name>` — a `kind: Secret` / `SealedSecret`
//!       manifest's `metadata.name`. Its `data:` is never read.
//!     - DEFINE `config:flag:<key>` — a Flipt-style top-level `flags:` list,
//!       one per item-level `- key: <k>` (nested variant keys excluded).
//!
//! Out of scope:
//!   - Spring `application.yml` / `application.properties`
//!   - Rails `config/database.yml`, .NET `appsettings.json` (and so the .NET
//!     `IConfiguration["Section:Key"]` read side)
//!   - CI variable definitions (GHA `env:`, GitLab CI variables) — reads
//!     covered via shell `$VAR` if needed in v0.5+.

use glia_code_domain::{CodeNav, GRAPH_TYPE, cell_type, edge_category, node_kind};
use glia_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};

pub struct ConfigNodes {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub nav: CodeNav,
    /// LE.4b: every env read [`extract_config_reads`] kept, as
    /// `(CONFIG_KEY id, byte offset of the read expression)`, in scan order
    /// (one per match, so a key read twice has two sites). The engine turns
    /// the offsets into lines and re-homes each `module -> key` READS_CONFIG
    /// edge to the function holding its reads. Empty on the define side and
    /// for secrets / flags.
    pub sites: Vec<(NodeId, usize)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Side {
    Read,
    Define,
}

/// Which `config:<flavor>:` track a key lives on (A13.8). The flavor picks the
/// qname segment AND the name validator, so a secret path (`prod/db-creds`)
/// or a flag key (`new-checkout`) is not dropped by the env-name rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flavor {
    /// `config:env:<NAME>` — `[A-Za-z_][A-Za-z0-9_]*`.
    Env,
    /// `config:secret:<provider>/<ref>` — see [`is_valid_secret_ref`].
    Secret,
    /// `config:flag:<key>` — see [`is_valid_flag_key`].
    Flag,
}

impl Flavor {
    fn accepts(self, def: &ConfigDef) -> bool {
        match self {
            Flavor::Env => is_valid_env_name(&def.name),
            Flavor::Secret => !def.source.is_empty() && is_valid_secret_ref(&def.name),
            Flavor::Flag => is_valid_flag_key(&def.name),
        }
    }

    fn qname(self, def: &ConfigDef) -> String {
        format!("config:{}:{}", self.segment(), self.display_name(def))
    }

    fn segment(self) -> &'static str {
        match self {
            Flavor::Env => "env",
            Flavor::Secret => "secret",
            Flavor::Flag => "flag",
        }
    }

    /// The node's simple name: the qname past `config:<flavor>:`. A secret
    /// keeps its provider (`k8s/db-secret`, `aws_sm/prod/db-creds`) — the
    /// same ref under two providers is two secrets, and a bare Secret name
    /// must not read as an env-var or a same-file symbol of that name.
    fn display_name(self, def: &ConfigDef) -> String {
        match self {
            Flavor::Secret => format!("{}/{}", def.source, def.name),
            Flavor::Env | Flavor::Flag => def.name.clone(),
        }
    }
}

/// One config-key occurrence on its way to a `CONFIG_KEY` node. The env
/// define side carries the literal value it declared; the read side never
/// can, because `os.environ["X"]` states no value (A13.7). A secret or flag
/// never carries a value on either side (A13.8).
pub(crate) struct ConfigDef {
    pub(crate) name: String,
    value: Option<String>,
    /// Which syntax produced this — `dockerfile` | `dotenv` | `k8s` |
    /// `compose`, empty on the env read side. For a secret or flag it is the
    /// PROVIDER (`vault`, `aws_sm`, `launchdarkly`, `flipt`, ...). It
    /// discriminates the ENV-cell payload so one cell type can carry several
    /// provenances without ambiguity.
    pub(crate) source: &'static str,
    pub(crate) flavor: Flavor,
}

impl ConfigDef {
    /// A name with no value attached (read sites, k8s `valueFrom:`, bare
    /// compose `- KEY` passthrough).
    fn bare(name: impl Into<String>, source: &'static str) -> Self {
        ConfigDef { name: name.into(), value: None, source, flavor: Flavor::Env }
    }

    /// A secrets-manager reference `config:secret:<provider>/<name>`.
    pub(crate) fn secret(name: impl Into<String>, provider: &'static str) -> Self {
        ConfigDef { name: name.into(), value: None, source: provider, flavor: Flavor::Secret }
    }

    /// A feature-flag key `config:flag:<name>`, `provider` on the cell.
    pub(crate) fn flag(name: impl Into<String>, provider: &'static str) -> Self {
        ConfigDef { name: name.into(), value: None, source: provider, flavor: Flavor::Flag }
    }
}

/// Extract env-var read references from a source-code file. The caller decides
/// the file type by extension; this scanner tries every language idiom because
/// idioms cross language boundaries (`os.getenv` exists in C, Python, and Ruby
/// shells; `process.env` shows up in TS-flavoured tooling).
///
/// Emits one `module -> key` READS_CONFIG edge per key, plus one
/// [`ConfigNodes::sites`] entry per valid read so the engine can re-home the
/// edge to the reading function (LE.4b).
pub fn extract_config_reads(
    source: &str,
    module_id: NodeId,
    repo: RepoId,
) -> ConfigNodes {
    let mut reads = Vec::new();
    reads.extend(scan_python_env(source));
    reads.extend(scan_js_process_env(source));
    reads.extend(scan_rust_env(source));
    reads.extend(scan_go_env(source));
    reads.extend(scan_ruby_env(source));
    reads.extend(scan_java_system_getenv(source));
    reads.extend(scan_php_env(source));
    let mut sites = Vec::new();
    let mut defs = Vec::with_capacity(reads.len());
    for (name, offset) in reads {
        let def = ConfigDef::bare(name, "");
        // The same validator and qname `build_sided` applies, so a site
        // always names a node this call emits.
        if Flavor::Env.accepts(&def) {
            let qname = Flavor::Env.qname(&def);
            let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::CONFIG_KEY, &qname);
            sites.push((id, offset));
        }
        defs.push(def);
    }
    let mut out = build_nodes(defs, Side::Read, module_id, repo);
    out.sites = sites;
    out
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
    let k8s = scan_k8s_env(source);
    let mut items: Vec<(ConfigDef, Side)> =
        k8s.env.into_iter().map(|d| (d, Side::Define)).collect();
    items.extend(scan_compose_environment(source).into_iter().map(|d| (d, Side::Define)));
    // A13.8: the Secrets a workload consumes, the Secrets a manifest declares,
    // and the flags a Flipt file declares. One call, so a file that both
    // declares and consumes the same Secret yields ONE node with both edges.
    items.extend(k8s.secret_reads.into_iter().map(|d| (d, Side::Read)));
    items.extend(scan_k8s_secret_manifests(source).into_iter().map(|d| (d, Side::Define)));
    items.extend(scan_flag_definitions(source).into_iter().map(|d| (d, Side::Define)));
    let out = build_sided(items, module_id, repo);
    if let Some(marker) = crate::secrets_flags::marker(&[&out], "yaml") {
        eprintln!("{marker}");
    }
    out
}

/// Every def on one `side` — the shape every single-sided caller wants.
pub(crate) fn build_nodes(
    defs: Vec<ConfigDef>,
    side: Side,
    module_id: NodeId,
    repo: RepoId,
) -> ConfigNodes {
    build_sided(defs.into_iter().map(|d| (d, side)).collect(), module_id, repo)
}

/// The one CONFIG_KEY emit path: node + READS_CONFIG / DEFINES_CONFIG edge +
/// dedupe by qname + ENV cell. A key seen on both sides in one file is one
/// node with one edge per side.
fn build_sided(items: Vec<(ConfigDef, Side)>, module_id: NodeId, repo: RepoId) -> ConfigNodes {
    let mut nodes: Vec<Node> = Vec::new();
    let mut edges = Vec::new();
    let mut nav = CodeNav::default();
    // qname -> index into `nodes`, so a repeat of a key that arrived
    // value-less can still be upgraded by a later definition that has one.
    let mut seen: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut edged: std::collections::HashSet<(usize, Side)> = std::collections::HashSet::new();
    // The `[config]` marker counts the env track only; secrets and flags
    // report through `[secrets]`.
    let (mut env_defined, mut valued, mut redacted) = (0usize, 0usize, 0usize);

    for (def, side) in items {
        if !def.flavor.accepts(&def) {
            continue;
        }
        let counts_env = def.flavor == Flavor::Env && side == Side::Define;
        // The env read side states no value, so it never attaches a cell; a
        // secret or flag records its provider and never a value.
        let cell = match (def.flavor, side) {
            (Flavor::Env, Side::Read) => None,
            (Flavor::Env, Side::Define) => env_cell(&def),
            (Flavor::Secret | Flavor::Flag, _) => Some((provider_cell(def.source), true)),
        };
        let qname = def.flavor.qname(&def);
        let idx = if let Some(&idx) = seen.get(&qname) {
            // First definition wins the node; the first non-None value wins
            // the cell (`.env` then `.env.local`, k8s then compose).
            if let (true, Some(c)) = (nodes[idx].cells.is_empty(), cell) {
                if counts_env {
                    valued += 1;
                    if c.1 {
                        redacted += 1;
                    }
                }
                nodes[idx].cells.push(c.0);
            }
            idx
        } else {
            let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::CONFIG_KEY, &qname);
            let cells = match cell {
                Some((c, was_redacted)) => {
                    if counts_env {
                        valued += 1;
                        if was_redacted {
                            redacted += 1;
                        }
                    }
                    vec![c]
                }
                None => vec![],
            };
            if counts_env {
                env_defined += 1;
            }
            seen.insert(qname.clone(), nodes.len());
            nodes.push(Node {
                id,
                repo,
                confidence: Confidence::Medium,
                cells,
            });
            let name = def.flavor.display_name(&def);
            nav.record(id, &name, &qname, node_kind::CONFIG_KEY, Some(module_id));
            nodes.len() - 1
        };
        if edged.insert((idx, side)) {
            edges.push(Edge {
                from: module_id,
                to: nodes[idx].id,
                category: match side {
                    Side::Read => edge_category::READS_CONFIG,
                    Side::Define => edge_category::DEFINES_CONFIG,
                },
                confidence: Confidence::Medium,
                cells: Vec::new(),
            });
        }
    }

    if env_defined > 0 {
        eprintln!("[config] defined={env_defined} valued={valued} redacted={redacted}");
    }

    ConfigNodes { nodes, edges, nav, sites: Vec::new() }
}

/// The ENV cell of a secret or flag key: its provider, and that a value
/// exists which the graph does not hold. The same `{source, redacted}` shape
/// A13.7 gives a secret-named env key, so consumers decode one schema.
fn provider_cell(provider: &str) -> Cell {
    Cell {
        kind: cell_type::ENV,
        payload: CellPayload::Json(format!(
            "{{\"source\":\"{}\",\"redacted\":true}}",
            escape_json(provider)
        )),
    }
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
/// REDACTION IS NOT OPTIONAL. `.gmap` files are persisted inside the repo,
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

/// A secrets-manager reference: 1..=200 chars with no whitespace, no quote or
/// backslash, and no `$` / `{` / `}` / `%` — each of those marks an
/// unresolved interpolation (`${env}`, `{{ .Values.x }}`, `%s`) rather than
/// a real path. A trailing `/` is a prefix awaiting concatenation.
pub(crate) fn is_valid_secret_ref(s: &str) -> bool {
    let n = s.chars().count();
    (1..=200).contains(&n)
        && !s.ends_with('/')
        && !s.chars().any(|c| {
            c.is_whitespace()
                || c.is_control()
                || matches!(c, '$' | '{' | '}' | '%' | '"' | '\'' | '`' | '\\')
        })
}

/// A feature-flag key: `[A-Za-z0-9][A-Za-z0-9._-]{1,63}`, and either carries a
/// `-` / `_` / `.` or is at least 4 chars — so `.variation("a")` and other
/// short noise never mint a flag.
pub(crate) fn is_valid_flag_key(s: &str) -> bool {
    let b = s.as_bytes();
    if !(2..=64).contains(&b.len()) || !b[0].is_ascii_alphanumeric() {
        return false;
    }
    let sep = |c: &u8| matches!(c, b'.' | b'_' | b'-');
    b.iter().all(|c| c.is_ascii_alphanumeric() || sep(c)) && (b.len() >= 4 || b.iter().any(sep))
}

// ----------------------------------------------------------------------------
// Code-side scanners — one per language idiom. Each returns `(name, offset)`:
// the byte offset where the read expression's needle starts (LE.4b), always a
// char boundary because it is a `str::find` match.
// ----------------------------------------------------------------------------

fn scan_python_env(source: &str) -> Vec<(String, usize)> {
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

fn scan_js_process_env(source: &str) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    // process.env.VAR — bare property access. Identifier-shaped name follows.
    let bytes = source.as_bytes();
    for needle in ["process.env.", "import.meta.env."] {
        let mut search_from = 0;
        while let Some(rel) = source[search_from..].find(needle) {
            let at = search_from + rel;
            let pos = at + needle.len();
            // Read identifier chars.
            let mut j = pos;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            if j > pos {
                out.push((source[pos..j].to_string(), at));
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

fn scan_rust_env(source: &str) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    for needle in ["std::env::var(", "env::var("] {
        for hit in capture_first_string_arg(source, needle) {
            out.push(hit);
        }
    }
    out
}

fn scan_go_env(source: &str) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    for needle in ["os.Getenv(", "os.LookupEnv("] {
        for hit in capture_first_string_arg(source, needle) {
            out.push(hit);
        }
    }
    out
}

fn scan_ruby_env(source: &str) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    // ENV['X'] / ENV["X"] / ENV.fetch('X', ...)
    for needle in ["ENV[", "ENV.fetch(", "ENV.fetch!("] {
        for hit in capture_first_string_arg(source, needle) {
            out.push(hit);
        }
    }
    out
}

fn scan_java_system_getenv(source: &str) -> Vec<(String, usize)> {
    capture_first_string_arg(source, "System.getenv(")
}

fn scan_php_env(source: &str) -> Vec<(String, usize)> {
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
                flavor: Flavor::Env,
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
                flavor: Flavor::Env,
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
                    flavor: Flavor::Env,
                });
            }
        }
    }
    out
}

/// One walk of a k8s manifest: the env vars its containers declare, and the
/// Secrets they consume (A13.8).
struct K8sScan {
    env: Vec<ConfigDef>,
    /// `config:secret:k8s/<name>` READS — `secretKeyRef:` / `secretRef:`.
    secret_reads: Vec<ConfigDef>,
}

/// The object-reference keys whose nested `name:` names a referenced object,
/// never an env var — and whether that object is a Secret.
const K8S_REF_KEYS: [(&str, bool); 6] = [
    ("secretKeyRef:", true),
    ("secretRef:", true),
    ("configMapKeyRef:", false),
    ("configMapRef:", false),
    ("fieldRef:", false),
    ("resourceFieldRef:", false),
];

/// `name:` out of an inline flow mapping — `{name: db-secret, key: password}`.
fn flow_mapping_name(rest: &str) -> Option<&str> {
    let body = rest.trim().strip_prefix('{')?.strip_suffix('}')?;
    body.split(',').find_map(|kv| {
        let v = kv.trim().strip_prefix("name:")?;
        Some(v.trim().trim_matches(|c| c == '"' || c == '\''))
    })
}

fn scan_k8s_env(source: &str) -> K8sScan {
    let mut out: Vec<ConfigDef> = Vec::new();
    let mut secret_reads: Vec<ConfigDef> = Vec::new();
    let mut in_env_block = false;
    let mut env_indent: usize = 0;
    // Column of the `name:` key of the list item currently being read, so a
    // `value:` line is only paired when it is that key's SIBLING. A
    // `valueFrom:`/`secretRef:` item has no sibling `value:` and so stays
    // value-less, and the `name:` nested inside a `secretKeyRef:` cannot
    // capture the outer item's value.
    let mut key_col: Option<usize> = None;
    // A13.8: inside a `secretKeyRef:` / `configMapKeyRef:` / ... block — its
    // key column, and whether it references a Secret. Its nested `name:` is
    // the referenced object: a Secret READ, or nothing — never an env var.
    let mut ref_block: Option<(usize, bool)> = None;

    for line in source.lines() {
        let indent = line.len() - line.trim_start().len();
        let t = line.trim();
        let t_stripped = t.strip_prefix("- ").unwrap_or(t);
        // Column of the key text itself, past any `- ` list marker.
        let col = indent + (t.len() - t_stripped.len());

        if t_stripped == "env:" || t_stripped.starts_with("env:") && t_stripped.ends_with(":") {
            in_env_block = true;
            env_indent = indent;
            key_col = None;
            ref_block = None;
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
        if let Some((ref_col, is_secret)) = ref_block
            && !t.is_empty()
        {
            if col <= ref_col {
                ref_block = None;
            } else {
                if is_secret && let Some(rest) = t_stripped.strip_prefix("name:") {
                    let v = rest.trim().trim_matches(|c| c == '"' || c == '\'');
                    secret_reads.push(ConfigDef::secret(v, "k8s"));
                }
                continue;
            }
        }
        if let Some((key, is_secret)) =
            K8S_REF_KEYS.iter().find(|(k, _)| t_stripped.starts_with(k))
        {
            let rest = &t_stripped[key.len()..];
            if rest.trim().is_empty() {
                ref_block = Some((col, *is_secret));
            } else if *is_secret && let Some(v) = flow_mapping_name(rest) {
                secret_reads.push(ConfigDef::secret(v, "k8s"));
            }
            continue;
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
    K8sScan { env: out, secret_reads }
}

/// Leading-space count and trimmed text of every non-blank, non-comment line.
fn yaml_lines(source: &str) -> impl Iterator<Item = (usize, &str)> {
    source.lines().filter_map(|line| {
        let t = line.trim();
        (!t.is_empty() && !t.starts_with('#')).then(|| (line.len() - line.trim_start().len(), t))
    })
}

/// A13.8 define side: every `kind: Secret` / `kind: SealedSecret` document
/// declares the Secret named by its `metadata.name`. Only the name is read —
/// never `data:` / `stringData:` — so no secret byte can reach the graph.
fn scan_k8s_secret_manifests(source: &str) -> Vec<ConfigDef> {
    let mut out = Vec::new();
    for doc in source.split("\n---") {
        let (mut is_secret, mut name) = (false, None);
        let (mut in_meta, mut meta_col) = (false, None::<usize>);
        for (indent, t) in yaml_lines(doc) {
            if indent == 0 {
                in_meta = t == "metadata:";
                if let Some(kind) = t.strip_prefix("kind:") {
                    let kind = kind.trim().trim_matches(|c| c == '"' || c == '\'');
                    is_secret = matches!(kind, "Secret" | "SealedSecret");
                }
                continue;
            }
            if in_meta
                && name.is_none()
                && indent == *meta_col.get_or_insert(indent)
                && let Some(rest) = t.strip_prefix("name:")
            {
                name = Some(rest.trim().trim_matches(|c| c == '"' || c == '\''));
            }
        }
        if let (true, Some(n)) = (is_secret, name) {
            out.push(ConfigDef::secret(n, "k8s"));
        }
    }
    out
}

/// A13.8 define side: a Flipt-style top-level `flags:` list declares one flag
/// per ITEM-level `key:` (`- key: new-checkout`, or a `key:` sibling of the
/// item's first line). A nested `variants: - key: blue` sits deeper than the
/// item and is skipped, as is every other top-level list (`segments:`).
fn scan_flag_definitions(source: &str) -> Vec<ConfigDef> {
    let mut out = Vec::new();
    let mut in_flags = false;
    // Column of the flags list's `-`, and of the current item's first key.
    let (mut dash_col, mut item_key_col) = (None::<usize>, None::<usize>);
    for (indent, t) in yaml_lines(source) {
        if indent == 0 && !t.starts_with('-') {
            in_flags = t == "flags:";
            (dash_col, item_key_col) = (None, None);
            continue;
        }
        if !in_flags {
            continue;
        }
        let key_line = if let Some(item) = t.strip_prefix('-') {
            if indent != *dash_col.get_or_insert(indent) {
                continue;
            }
            let item_trim = item.trim_start();
            item_key_col = Some(indent + (t.len() - item_trim.len()));
            item_trim
        } else if Some(indent) == item_key_col {
            t
        } else {
            continue;
        };
        if let Some(rest) = key_line.strip_prefix("key:") {
            let v = rest.trim().trim_matches(|c| c == '"' || c == '\'');
            out.push(ConfigDef::flag(v, "flipt"));
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
                    out.push(ConfigDef {
                        name: key.to_string(),
                        value,
                        source: "compose",
                        flavor: Flavor::Env,
                    });
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
                            flavor: Flavor::Env,
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
/// literal that follows the needle and push its inner text, with the byte
/// offset where that occurrence of `needle` starts. `needle` should end at the
/// position immediately before the value (after `(`, `[`, etc.).
fn capture_first_string_arg(source: &str, needle: &str) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    let mut search_from = 0;
    while let Some(rel) = source[search_from..].find(needle) {
        let pos = search_from + rel;
        let after = &source[pos + needle.len()..];
        if let Some(name) = first_quoted(after) {
            out.push((name, pos));
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
        // A13.8: that nested `name:` is the Secret the pod READS — never an
        // env var of its own.
        let keys = config_keys(&out);
        assert!(keys.contains(&"config:secret:k8s/db-secret".to_string()), "{keys:?}");
        assert!(!keys.iter().any(|k| k == "config:env:db-secret"), "{keys:?}");
        // Its simple name keeps the provider, so no node is NAMED `db-secret`.
        let names: Vec<&String> = out.nav.name_by_id.values().collect();
        assert!(names.iter().any(|n| *n == "k8s/db-secret"), "{names:?}");
        assert!(!names.iter().any(|n| *n == "db-secret"), "{names:?}");
        assert_eq!(edge_to(&out, "config:secret:k8s/db-secret"), vec![edge_category::READS_CONFIG]);
        assert_eq!(
            edge_to(&out, "config:env:DB_PASSWORD"),
            vec![edge_category::DEFINES_CONFIG]
        );
    }

    /// Categories of every edge into the node with `qname`.
    fn edge_to(out: &ConfigNodes, qname: &str) -> Vec<glia_core::EdgeCategoryId> {
        let Some(id) = out.nav.qname_by_id.iter().find(|(_, q)| *q == qname).map(|(id, _)| *id)
        else {
            return vec![];
        };
        out.edges.iter().filter(|e| e.to == id).map(|e| e.category).collect()
    }

    #[test]
    fn k8s_ref_blocks_never_leak_env_names() {
        let repo = RepoId(1);
        let src = concat!(
            "spec:\n",
            "  containers:\n",
            "    - name: api\n",
            "      env:\n",
            "        - name: LOG_LEVEL\n",
            "          valueFrom:\n",
            "            configMapKeyRef:\n",
            "              name: appconfig\n",
            "              key: level\n",
            "        - name: API_TOKEN\n",
            "          valueFrom:\n",
            "            secretKeyRef: {name: api-keys, key: token}\n",
            "        - name: AFTER\n",
            "          value: \"1\"\n",
            "      envFrom:\n",
            "        - secretRef:\n",
            "            name: app-secrets\n",
            "        - configMapRef:\n",
            "            name: app-config\n",
        );
        let out = extract_yaml_env_defs(src, module_id(repo), repo);
        let mut keys = config_keys(&out);
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "config:env:AFTER",
                "config:env:API_TOKEN",
                "config:env:LOG_LEVEL",
                "config:secret:k8s/api-keys",
                "config:secret:k8s/app-secrets",
            ]
        );
        // The item after a ref block still pairs its own sibling value.
        assert!(env_payload(&out, "AFTER").unwrap().contains(r#""value":"1""#));
    }

    #[test]
    fn k8s_secret_manifest_defines_by_metadata_name() {
        let repo = RepoId(1);
        let src = concat!(
            "apiVersion: v1\n",
            "kind: Secret\n",
            "metadata:\n",
            "  labels:\n",
            "    name: not-this\n",
            "  name: db-secret\n",
            "type: Opaque\n",
            "data:\n",
            "  password: aHVudGVyMg==\n",
            "---\n",
            "apiVersion: v1\n",
            "kind: ConfigMap\n",
            "metadata:\n",
            "  name: plain-config\n",
            "---\n",
            "kind: Deployment\n",
            "metadata:\n",
            "  name: api\n",
            "spec:\n",
            "  template:\n",
            "    spec:\n",
            "      containers:\n",
            "        - name: api\n",
            "          env:\n",
            "            - name: DB_PASSWORD\n",
            "              valueFrom:\n",
            "                secretKeyRef:\n",
            "                  name: db-secret\n",
            "                  key: password\n",
        );
        let out = extract_yaml_env_defs(src, module_id(repo), repo);
        let mut keys = config_keys(&out);
        keys.sort();
        assert_eq!(keys, vec!["config:env:DB_PASSWORD", "config:secret:k8s/db-secret"]);
        // Declared AND consumed in one file: one node, one edge per side.
        let mut cats = edge_to(&out, "config:secret:k8s/db-secret");
        cats.sort_by_key(|c| c.0);
        let mut want = vec![edge_category::READS_CONFIG, edge_category::DEFINES_CONFIG];
        want.sort_by_key(|c| c.0);
        assert_eq!(cats, want);
        assert_eq!(out.nodes.len(), 2);
        // The Secret's data never reaches a cell.
        let secret = out
            .nodes
            .iter()
            .find(|n| out.nav.qname_by_id.get(&n.id).map(String::as_str)
                == Some("config:secret:k8s/db-secret"))
            .expect("secret node");
        for c in &secret.cells {
            if let CellPayload::Json(s) = &c.payload {
                assert_eq!(s, r#"{"source":"k8s","redacted":true}"#);
            }
        }
    }

    #[test]
    fn flipt_flags_list_defines_item_keys_only() {
        let repo = RepoId(1);
        let src = concat!(
            "version: \"1.2\"\n",
            "namespace: default\n",
            "flags:\n",
            "  - key: new-checkout\n",
            "    name: New Checkout\n",
            "    type: VARIANT_FLAG_TYPE\n",
            "    variants:\n",
            "      - key: blue-variant\n",
            "        name: Blue\n",
            "  - name: Dark Mode\n",
            "    key: dark_mode\n",
            "segments:\n",
            "  - key: beta-users\n",
        );
        let out = extract_yaml_env_defs(src, module_id(repo), repo);
        let mut keys = config_keys(&out);
        keys.sort();
        assert_eq!(keys, vec!["config:flag:dark_mode", "config:flag:new-checkout"]);
        assert_eq!(edge_to(&out, "config:flag:new-checkout"), vec![edge_category::DEFINES_CONFIG]);
        assert_eq!(
            crate::secrets_flags::marker(&[&out], "yaml").as_deref(),
            Some("[secrets] refs=0 providers=0 flags=0 flag_defs=2 src=yaml")
        );
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

    fn env_id(repo: RepoId, key: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo, node_kind::CONFIG_KEY, &format!("config:env:{key}"))
    }

    /// LE.4b: every read site's offset starts inside its key's read
    /// expression, and the first one starts the expression. Overlapping
    /// needles (`os.environ[` / `environ[`, `std::env::var(` / `env::var(`,
    /// `$_ENV[` / `ENV[`, `System.getenv(` / `getenv(`) give one site each,
    /// all inside the same expression, so they land on the same line.
    #[test]
    fn read_sites_point_at_the_read_expression() {
        let repo = RepoId(1);
        let cases: [(&str, &str); 7] = [
            ("PY_KEY", "os.environ['PY_KEY']"),
            ("JS_KEY", "process.env.JS_KEY"),
            ("RS_KEY", "std::env::var(\"RS_KEY\")"),
            ("GO_KEY", "os.Getenv(\"GO_KEY\")"),
            ("RB_KEY", "ENV.fetch('RB_KEY')"),
            ("JAVA_KEY", "System.getenv(\"JAVA_KEY\")"),
            ("PHP_KEY", "$_ENV['PHP_KEY']"),
        ];
        let src: String = cases.iter().map(|(_, e)| format!("v = {e};\n")).collect();
        let out = extract_config_reads(&src, module_id(repo), repo);
        for (key, expr) in cases {
            let start = src.find(expr).unwrap();
            let offs: Vec<usize> = out
                .sites
                .iter()
                .filter(|(t, _)| *t == env_id(repo, key))
                .map(|(_, o)| *o)
                .collect();
            assert_eq!(offs.iter().min(), Some(&start), "{key}: {offs:?}");
            assert!(
                offs.iter().all(|o| (start..start + expr.len()).contains(o)),
                "{key}: {offs:?} outside {expr} at {start}"
            );
        }
        // Every site names a node this call emitted, and the module edges are
        // unchanged: one READS_CONFIG per key.
        let ids: Vec<NodeId> = out.nodes.iter().map(|n| n.id).collect();
        assert!(out.sites.iter().all(|(t, _)| ids.contains(t)));
        assert_eq!(out.edges.len(), 7);
    }

    #[test]
    fn read_sites_one_per_read_char_safe_and_valid_only() {
        let repo = RepoId(1);
        // A multi-byte prefix, a key read twice, and a name the env validator
        // rejects (no node, so no site).
        let src = concat!(
            "// caf\u{e9} \u{2264}\n",
            "const a = process.env.TWICE;\n",
            "function f() {\n",
            "  return process.env.TWICE + process.env['9BAD'];\n",
            "}\n",
        );
        let out = extract_config_reads(src, module_id(repo), repo);
        let twice: Vec<usize> = out
            .sites
            .iter()
            .filter(|(t, _)| *t == env_id(repo, "TWICE"))
            .map(|(_, o)| *o)
            .collect();
        assert_eq!(twice.len(), 2, "{:?}", out.sites);
        assert!(
            twice
                .iter()
                .all(|&o| src.is_char_boundary(o) && src[o..].starts_with("process.env.TWICE"))
        );
        assert_eq!(out.sites.len(), 2, "the invalid name gives no site: {:?}", out.sites);
        assert_eq!(out.edges.len(), 1);
    }

    #[test]
    fn define_side_carries_no_sites() {
        let repo = RepoId(1);
        assert!(extract_dotenv_defs("A=1\n", module_id(repo), repo).sites.is_empty());
        assert!(extract_dockerfile_defs("ENV B=2\n", module_id(repo), repo).sites.is_empty());
    }
}
