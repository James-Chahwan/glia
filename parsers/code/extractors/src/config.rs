//! Config / env-var extraction (v0.4.x — task #8).
//!
//! Emits `CONFIG_KEY` nodes per unique env-var name with two edge flavours:
//!
//!   - `READS_CONFIG`  — code module → key  (`os.environ['DB_URL']` etc.);
//!     the engine then re-homes it to the innermost FUNCTION / METHOD holding
//!     the read (LE.4b, `anchor::rehome_to_owner` over [`ConfigNodes::sites`]),
//!     so only a read at module scope keeps the module edge. A feature-flag
//!     check takes the same path (CC.7a); a secret reference stays on the
//!     module.
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
//!   - `config:setting:<Section:Key>` — a hierarchical application setting
//!     (CL.7b), the key exactly as the code reads it: .NET's `:`-joined
//!     configuration path. `appsettings*.json` defines it, `IConfiguration`
//!     reads it, and an env define `Section__Key` (.NET's environment
//!     override, `__` standing for `:`) defines it too.
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
//!     - C#:     `Environment.GetEnvironmentVariable("X")` (CL.7a; the
//!       `(..., EnvironmentVariableTarget.X)` form reads its first literal)
//!     - .NET settings (CL.7b, `config:setting:`, `.cs` files only):
//!       `configuration["A:B"]`,
//!       `.GetValue<T>("A:B")`, `.GetSection("A")` / `.GetRequiredSection`,
//!       `.GetConnectionString("Db")` (= `ConnectionStrings:Db`); a
//!       `GetSection("A")` chain prefixes the key it is read through. See
//!       [`scan_dotnet_config_reads`].
//!
//!   **Defines (separate file types via pipeline bypass):**
//!     - Dockerfile `ENV KEY=value` / `ENV KEY value`
//!     - `.env` files (KEY=value lines, # comments)
//!     - k8s YAML `env: - name: KEY`
//!     - docker-compose `environment: - KEY=value`
//!     - every one of the four above, for a name `A__B[__C..]`, ALSO defines
//!       `config:setting:A:B[:C..]` (CL.7b, [`dotnet_env_override`])
//!     - .NET `appsettings.json` / `appsettings.<Env>.json` leaves
//!       (CL.7b, [`extract_settings_json_defs`])
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
//!   - Rails `config/database.yml`
//!   - CI variable definitions (GHA `env:`, GitLab CI variables) — reads
//!     covered via shell `$VAR` if needed in v0.5+.

use glia_code_domain::evidence::{self, Evidence};
use glia_code_domain::{CodeNav, GRAPH_TYPE, cell_type, edge_category, node_kind};
use glia_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};

use crate::code_guard::LazyGuard;

pub struct ConfigNodes {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub nav: CodeNav,
    /// LE.4b: every env read [`extract_config_reads`] kept, as
    /// `(CONFIG_KEY id, byte offset of the read expression)`, in scan order
    /// (one per match, so a key read twice has two sites). The engine turns
    /// the offsets into lines and re-homes each `module -> key` READS_CONFIG
    /// edge to the function holding its reads. CC.7a: the flag checks
    /// `secrets_flags::extract_feature_flags` kept fill it the same way (the
    /// offset is the SDK call's needle). Empty on the define side and for
    /// secrets.
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
    /// `config:setting:<Section:Key>` (CL.7b) — see [`is_valid_setting_key`].
    Setting,
}

impl Flavor {
    pub(crate) fn accepts(self, def: &ConfigDef) -> bool {
        match self {
            Flavor::Env => is_valid_env_name(&def.name),
            Flavor::Secret => !def.source.is_empty() && is_valid_secret_ref(&def.name),
            Flavor::Flag => is_valid_flag_key(&def.name),
            Flavor::Setting => is_valid_setting_key(&def.name),
        }
    }

    pub(crate) fn qname(self, def: &ConfigDef) -> String {
        format!("config:{}:{}", self.segment(), self.display_name(def))
    }

    fn segment(self) -> &'static str {
        match self {
            Flavor::Env => "env",
            Flavor::Secret => "secret",
            Flavor::Flag => "flag",
            Flavor::Setting => "setting",
        }
    }

    /// The node's simple name: the qname past `config:<flavor>:`. A secret
    /// keeps its provider (`k8s/db-secret`, `aws_sm/prod/db-creds`) — the
    /// same ref under two providers is two secrets, and a bare Secret name
    /// must not read as an env-var or a same-file symbol of that name.
    fn display_name(self, def: &ConfigDef) -> String {
        match self {
            Flavor::Secret => format!("{}/{}", def.source, def.name),
            Flavor::Env | Flavor::Flag | Flavor::Setting => def.name.clone(),
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
    /// `compose` (an env define, and the setting it overrides, CL.7b),
    /// `appsettings` (a settings-file leaf), empty on the env / setting read
    /// side. For a secret or flag it is the
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

    /// A .NET setting `config:setting:<Section:Key>` (CL.7b).
    fn setting(name: impl Into<String>, value: Option<String>, source: &'static str) -> Self {
        ConfigDef { name: name.into(), value, source, flavor: Flavor::Setting }
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
///
/// CJ.1c: `path` picks the literal / comment guard ([`LazyGuard`]; `""` = no
/// guard). In a Rust or Python file a read whose needle starts inside a string
/// literal or comment (a scanner's own test source, a doc line) is dropped
/// before any node, edge or site is built; a Python f-string's replacement
/// fields are code, so `f"{os.getenv('X')}"` stays a read. Every other
/// language is unchanged. fired_on: `[code-guard] config lang=.. dropped=..`.
pub fn extract_config_reads(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
) -> ConfigNodes {
    let env = |hits: Vec<(String, usize)>| hits.into_iter().map(|(n, o)| (n, o, Flavor::Env));
    let mut reads: Vec<(String, usize, Flavor)> = Vec::new();
    reads.extend(env(scan_python_env(source)));
    reads.extend(env(scan_js_process_env(source)));
    reads.extend(env(scan_rust_env(source)));
    reads.extend(env(scan_go_env(source)));
    reads.extend(env(scan_ruby_env(source)));
    reads.extend(env(scan_java_system_getenv(source)));
    reads.extend(env(scan_php_env(source)));
    reads.extend(env(scan_dotnet_env(source)));
    // CL.7b: .NET IConfiguration reads, on the `config:setting:` track — C#
    // files only, so a TypeScript `interface IConfiguration` beside a
    // `this.config["apiUrl"]` mints nothing.
    if is_csharp_path(path) {
        reads.extend(
            scan_dotnet_config_reads(source).into_iter().map(|(n, o)| (n, o, Flavor::Setting)),
        );
    }
    // Each offset is the read needle's start (capture_first_string_arg / the
    // process.env scan), so the guard tests the read expression itself.
    let mut guard = LazyGuard::new(path, source);
    reads.retain(|(_, offset, _)| guard.admits(*offset));
    guard.report("config");
    let mut sites = Vec::new();
    let mut defs = Vec::with_capacity(reads.len());
    for (name, offset, flavor) in reads {
        let def = match flavor {
            Flavor::Setting => ConfigDef::setting(name, None, ""),
            _ => ConfigDef::bare(name, ""),
        };
        // The same validator and qname `build_sided` applies, so a site
        // always names a node this call emits.
        if def.flavor.accepts(&def) {
            let qname = def.flavor.qname(&def);
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

/// Most keys one settings file defines (CL.7b); the rest are counted as
/// `capped=` in the `[config] settings` line and dropped.
const SETTINGS_KEY_CAP: usize = 2_000;

/// True for a .NET settings file (CL.7b): a basename matching
/// `appsettings*.json`, case-insensitive — `appsettings.json`, the
/// environment files `appsettings.Development.json`, and the names a host
/// loads by hand (`AddJsonFile("configs/appsettings-prod.json")`,
/// `appsettingsAdmin.json`). The part between is `[A-Za-z0-9._-]*`. The walk
/// admits it beside the contract / JSON Schema sniffs and the engine routes
/// it to [`extract_settings_json_defs`] before either.
pub fn is_dotnet_settings_path(path: &str) -> bool {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path).to_ascii_lowercase();
    base.strip_prefix("appsettings")
        .and_then(|r| r.strip_suffix(".json"))
        .is_some_and(|mid| {
            mid.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        })
}

/// CL.7b: the `config:setting:<Section:Key>` keys a .NET settings file
/// defines — every scalar leaf, its object path joined with `:` and an array
/// element indexed `:0`, `:1` (the .NET binder's spelling), in the parsed
/// map's iteration order (deterministic per file). A leaf's value rides an
/// ENV cell through the A13.7 redaction ([`env_cell`], with the setting rules
/// of [`is_secret_setting`]); a `null` or empty-string leaf is valueless. The
/// value never reaches a name or qname.
///
/// The text is read the way .NET's JSON configuration provider reads it: a
/// leading BOM, `//` / `/* */` comments and trailing commas are allowed
/// ([`strip_jsonc`]). Anything else serde_json rejects, or a root that is not
/// an object, mints nothing (`parse_error=1`). At most [`SETTINGS_KEY_CAP`]
/// keys per file.
///
/// fired_on, once per settings file:
/// `[config] settings file=<path> keys=<k> valued=<v> redacted=<r> capped=<c> parse_error=<0|1>`.
pub fn extract_settings_json_defs(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
) -> ConfigNodes {
    let text = strip_jsonc(source.strip_prefix('\u{feff}').unwrap_or(source));
    let mut leaves: Vec<(String, Option<String>)> = Vec::new();
    let parse_error = match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(root @ serde_json::Value::Object(_)) => {
            flatten_settings(&root, "", &mut leaves);
            false
        }
        _ => true,
    };
    let capped = leaves.len().saturating_sub(SETTINGS_KEY_CAP);
    leaves.truncate(SETTINGS_KEY_CAP);
    let defs = leaves
        .into_iter()
        .map(|(name, value)| ConfigDef::setting(name, value, "appsettings"))
        .collect();
    let out = build_nodes(defs, Side::Define, module_id, repo);
    let (mut valued, mut redacted) = (0usize, 0usize);
    for n in &out.nodes {
        if let Some(c) = n.cells.iter().find(|c| c.kind == cell_type::ENV) {
            valued += 1;
            if matches!(&c.payload, CellPayload::Json(s) if s.contains("\"redacted\":true")) {
                redacted += 1;
            }
        }
    }
    eprintln!(
        "[config] settings file={path} keys={} valued={valued} redacted={redacted} capped={capped} parse_error={}",
        out.nodes.len(),
        u8::from(parse_error)
    );
    out
}

/// Depth-first walk of a parsed settings document, pushing every scalar leaf
/// as `(Section:Key path, value)`. serde_json caps nesting at 128 levels, so
/// the recursion is bounded.
fn flatten_settings(v: &serde_json::Value, prefix: &str, out: &mut Vec<(String, Option<String>)>) {
    use serde_json::Value;
    let join = |seg: &str| {
        if prefix.is_empty() { seg.to_string() } else { format!("{prefix}:{seg}") }
    };
    match v {
        Value::Object(map) => {
            for (k, child) in map {
                flatten_settings(child, &join(k), out);
            }
        }
        Value::Array(items) => {
            for (i, child) in items.iter().enumerate() {
                flatten_settings(child, &join(&i.to_string()), out);
            }
        }
        Value::Null => out.push((prefix.to_string(), None)),
        Value::String(s) => out.push((prefix.to_string(), (!s.is_empty()).then(|| s.clone()))),
        Value::Bool(b) => out.push((prefix.to_string(), Some(b.to_string()))),
        Value::Number(n) => out.push((prefix.to_string(), Some(n.to_string()))),
    }
}

/// `source` with what .NET's JSON configuration reader skips removed:
/// `//` line and `/* */` block comments outside strings, and a comma whose
/// next non-blank character closes an object or array. String contents are
/// copied untouched (escapes included).
fn strip_jsonc(source: &str) -> String {
    let mut no_comments = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    let (mut in_str, mut escaped) = (false, false);
    while let Some(c) = chars.next() {
        if in_str {
            no_comments.push(c);
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => in_str = false,
                _ => {}
            }
            continue;
        }
        match (c, chars.peek()) {
            ('/', Some('/')) => {
                for n in chars.by_ref() {
                    if n == '\n' {
                        no_comments.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                chars.next();
                let mut prev = '\0';
                for n in chars.by_ref() {
                    if prev == '*' && n == '/' {
                        break;
                    }
                    prev = n;
                }
                no_comments.push(' ');
            }
            _ => {
                in_str = c == '"';
                no_comments.push(c);
            }
        }
    }
    // Trailing commas, over the comment-free text.
    let mut out = String::with_capacity(no_comments.len());
    let (mut in_str, mut escaped) = (false, false);
    for (i, c) in no_comments.char_indices() {
        if in_str {
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => in_str = false,
                _ => {}
            }
        } else if c == '"' {
            in_str = true;
        } else if c == ','
            && no_comments[i + 1..]
                .trim_start()
                .starts_with(['}', ']'])
        {
            continue;
        }
        out.push(c);
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
///
/// CL.7b: every env define whose name is a .NET environment override
/// (`Stripe__SecretKey`, [`dotnet_env_override`]) is followed, in place, by
/// the `config:setting:Stripe:SecretKey` define it implies, so item order and
/// so NodeId / edge order stay deterministic. The override edge carries its
/// own EVIDENCE (the route key's emitter, rule `dotnet_env_override`), which
/// the engine's `stamp_missing` then leaves alone. fired_on, once per call
/// that emitted one: `[config] env overrides -> settings=<n> source=<s>`.
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
    // report through `[secrets]`, settings files through `[config] settings`
    // and env overrides through `[config] env overrides`.
    let (mut env_defined, mut valued, mut redacted) = (0usize, 0usize, 0usize);
    let mut overrides = 0usize;
    let mut override_emitter: Option<&'static str> = None;

    let mut expanded: Vec<(ConfigDef, Side, bool)> = Vec::with_capacity(items.len());
    for (def, side) in items {
        let ov = if side == Side::Define { dotnet_env_override(&def) } else { None };
        expanded.push((def, side, false));
        if let Some(ov) = ov {
            expanded.push((ov, Side::Define, true));
        }
    }

    for (def, side, is_override) in expanded {
        if !def.flavor.accepts(&def) {
            continue;
        }
        let counts_env = def.flavor == Flavor::Env && side == Side::Define;
        // The env / setting read side states no value, so it never attaches a
        // cell; a secret or flag records its provider and never a value.
        let cell = match (def.flavor, side) {
            (Flavor::Env | Flavor::Setting, Side::Read) => None,
            (Flavor::Env | Flavor::Setting, Side::Define) => env_cell(&def),
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
            let mut edge = Edge {
                from: module_id,
                to: nodes[idx].id,
                category: match side {
                    Side::Read => edge_category::READS_CONFIG,
                    Side::Define => edge_category::DEFINES_CONFIG,
                },
                confidence: Confidence::Medium,
                cells: Vec::new(),
            };
            if is_override {
                let emitter = define_emitter(def.source);
                evidence::attach(&mut edge, Evidence::emitter(emitter).rule("dotnet_env_override"));
                overrides += 1;
                override_emitter.get_or_insert(emitter);
            }
            edges.push(edge);
        }
    }

    if env_defined > 0 {
        eprintln!("[config] defined={env_defined} valued={valued} redacted={redacted}");
    }
    if let Some(emitter) = override_emitter {
        let source = emitter.strip_prefix("extractor:").unwrap_or(emitter);
        eprintln!("[config] env overrides -> settings={overrides} source={source}");
    }

    ConfigNodes { nodes, edges, nav, sites: Vec::new() }
}

/// CL.7b: .NET's environment configuration provider reads an env var
/// `Stripe__SecretKey` as the setting `Stripe:SecretKey` (`__` is the
/// hierarchy separator, since `:` is not legal in every shell's env names).
/// So a valid env define whose name splits on `__` into two or more non-empty
/// segments also defines that setting, from the same module, with the same
/// value (its ENV cell built by [`env_cell`] over the JOINED name). `PORT`,
/// `__X`, `X__` and `A____B` override nothing; a read states no override, so
/// only the define side calls this.
fn dotnet_env_override(def: &ConfigDef) -> Option<ConfigDef> {
    if def.flavor != Flavor::Env || !Flavor::Env.accepts(def) {
        return None;
    }
    let segs: Vec<&str> = def.name.split("__").collect();
    if segs.len() < 2 || segs.iter().any(|s| s.is_empty()) {
        return None;
    }
    Some(ConfigDef::setting(segs.join(":"), def.value.clone(), def.source))
}

/// The EVIDENCE emitter of an env-override setting edge: exactly the one the
/// engine's `stamp_missing` gives its sibling env edge, `extractor:<route
/// key>` — `dockerfile`, `dotenv`, and `yaml` for the two YAML scanners
/// (`k8s`, `compose`).
fn define_emitter(source: &str) -> &'static str {
    match source {
        "dockerfile" => "extractor:dockerfile",
        "dotenv" => "extractor:dotenv",
        _ => "extractor:yaml",
    }
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

/// Value fragments that mark an embedded credential (an ADO.NET / Npgsql /
/// Azure connection string) whatever the key is called.
const SECRET_VALUE_NEEDLES: [&str; 5] =
    ["PASSWORD=", "PWD=", "ACCOUNTKEY=", "SHAREDACCESSKEY=", "SECRET="];

/// CL.7b: A13.7's rule for a `config:setting:` key, which is stricter than the
/// env rule because .NET names its secrets differently. Redacted when the
/// whole `Section:Key` name hits [`is_secret_name`] (a superset of testing its
/// last segment, so an override never shows a value its env sibling redacts),
/// when it sits under `ConnectionStrings`, when its last segment ends in
/// `Key` (`Jwt:Key`, `Stripe:PublishableKey`) or names a connection string,
/// or when the value itself carries a credential fragment.
fn is_secret_setting(name: &str, value: &str) -> bool {
    let first = name.split(':').next().unwrap_or(name);
    let last = name.rsplit(':').next().unwrap_or(name).to_ascii_lowercase();
    let upper_value = value.to_ascii_uppercase();
    is_secret_name(name)
        || first.eq_ignore_ascii_case("ConnectionStrings")
        || last.ends_with("key")
        || last.contains("connectionstring")
        || SECRET_VALUE_NEEDLES.iter().any(|n| upper_value.contains(n))
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
    let secret = match def.flavor {
        Flavor::Setting => is_secret_setting(&def.name, raw),
        _ => is_secret_name(&def.name),
    };
    let (stored, redacted) = if secret {
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

/// A .NET setting key (CL.7b): 1..=128 bytes of `:`-separated segments, each
/// non-empty and made of `[A-Za-z0-9_.-]` (`Logging:LogLevel:Microsoft.AspNetCore`,
/// `Endpoints:0:Url`). A JSON key with a space, `$` or quote is no setting a
/// code literal names, and a `"Stripe:" + x` fragment fails the empty-segment
/// rule.
pub(crate) fn is_valid_setting_key(s: &str) -> bool {
    (1..=128).contains(&s.len())
        && s.split(':').all(|seg| {
            !seg.is_empty()
                && seg.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
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

/// CL.7a: .NET `Environment.GetEnvironmentVariable("X")`. The two-argument
/// form `("X", EnvironmentVariableTarget.Process)` reads its first literal; a
/// non-literal argument (`GetEnvironmentVariable(name)`) reads nothing.
fn scan_dotnet_env(source: &str) -> Vec<(String, usize)> {
    capture_first_string_arg(source, "Environment.GetEnvironmentVariable(")
}

/// A `.cs` file, the one .NET language the engine routes (CL.7b).
fn is_csharp_path(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("cs"))
}

/// CL.7b: .NET `IConfiguration` reads, as `(Section:Key, offset)`, scanned
/// in a C# file ([`is_csharp_path`]). Only a source that names the API (`IConfiguration*`, the
/// `Microsoft.Extensions.Configuration` namespace) or reads through a
/// `.Configuration` property (`builder.Configuration["X"]`: implicit usings
/// let a minimal-API `Program.cs` do that with neither) is scanned.
///
/// - an indexer `<recv>["A:B"]` whose receiver's last segment, lower-cased
///   and stripped of `_` / `@`, ends with `configuration` or `config`
///   (`configuration[`, `_config[`, `builder.Configuration[`) — a user
///   dictionary `cache["k"]` is not a read;
/// - `.GetValue<T>("A:B")`, `.GetSection("A")`, `.GetRequiredSection("A")`
///   (a section read is a key read of the section path);
/// - `.GetConnectionString("Db")` -> `ConnectionStrings:Db`, where .NET
///   keeps it.
///
/// A receiver that is itself a literal `GetSection("A")` / `GetRequiredSection`
/// call prefixes the key (`config.GetSection("Stripe")["SecretKey"]` reads
/// `Stripe:SecretKey`); any other call chain, or a method receiver named like
/// a section (`stripeSection.GetValue<..>("Key")`, a relative key), reads
/// nothing. A non-literal or interpolated key reads nothing. The offset is
/// the receiver's last segment for an indexer and the method name for a call.
fn scan_dotnet_config_reads(source: &str) -> Vec<(String, usize)> {
    let named = [
        "IConfiguration",
        "Microsoft.Extensions.Configuration",
        ".Configuration[",
        ".Configuration.Get",
    ];
    if !named.iter().any(|n| source.contains(n)) {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(rel) = source[from..].find("[") {
        let at = from + rel;
        from = at + 1;
        let Some(key) = csharp_literal_arg(&source[at + 1..], b"]") else { continue };
        if let Some((prefix, start)) = dotnet_receiver(source, at, true, 0) {
            out.push((join_setting(prefix, &key), start));
        }
    }
    for (method, generic) in [
        ("GetValue", true),
        ("GetSection", false),
        ("GetRequiredSection", false),
        ("GetConnectionString", false),
    ] {
        let needle = format!(".{method}");
        let mut from = 0;
        while let Some(rel) = source[from..].find(&needle) {
            let dot = from + rel;
            from = dot + needle.len();
            let Some(args) = dotnet_call_args(&source[from..], generic) else { continue };
            let Some(key) = csharp_literal_arg(&source[from + args..], b"),") else { continue };
            let key = match method {
                "GetConnectionString" => format!("ConnectionStrings:{key}"),
                _ => key,
            };
            if let Some((prefix, _)) = dotnet_receiver(source, dot, false, 0) {
                out.push((join_setting(prefix, &key), dot + 1));
            }
        }
    }
    out
}

/// `key` under a `GetSection` chain's path, if any.
fn join_setting(prefix: Option<String>, key: &str) -> String {
    match prefix {
        Some(p) => format!("{p}:{key}"),
        None => key.to_string(),
    }
}

/// Past a method name: an optional generic argument list (required when
/// `generic`, matched by `<` / `>` depth on one line), blanks, then `(`.
/// Returns the byte length up to and including the `(`; `None` when the name
/// continues (`.GetValues(`) or no call follows.
fn dotnet_call_args(after: &str, generic: bool) -> Option<usize> {
    let b = after.as_bytes();
    let mut i = 0;
    if generic {
        if b.first() != Some(&b'<') {
            return None;
        }
        let mut depth = 0usize;
        while i < b.len() {
            match b[i] {
                b'<' => depth += 1,
                b'>' => {
                    depth -= 1;
                    if depth == 0 {
                        i += 1;
                        break;
                    }
                }
                b'\n' | b';' | b'{' | b'"' => return None,
                _ => {}
            }
            i += 1;
        }
        if depth != 0 {
            return None;
        }
    }
    while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
        i += 1;
    }
    (b.get(i) == Some(&b'(')).then_some(i + 1)
}

/// A C# string literal (`"A:B"` or verbatim `@"A:B"`) at the start of `after`
/// (blanks allowed before it), whose next non-blank byte is one of `closers`
/// — so `config["Stripe:" + x]` and an interpolated `$"..."` read nothing.
fn csharp_literal_arg(after: &str, closers: &[u8]) -> Option<String> {
    let b = after.as_bytes();
    let mut i = 0;
    while i < b.len() && b[i].is_ascii_whitespace() {
        i += 1;
    }
    if b.get(i) == Some(&b'@') {
        i += 1;
    }
    if b.get(i) != Some(&b'"') {
        return None;
    }
    let start = i + 1;
    let end = start + after[start..].find('"')?;
    let mut j = end + 1;
    while j < b.len() && b[j].is_ascii_whitespace() {
        j += 1;
    }
    b.get(j).is_some_and(|c| closers.contains(c)).then(|| after[start..end].to_string())
}

/// What a .NET config read at byte `pos` (its `[`, or the `.` before its
/// method) is read through: `Some((None, start))` for the configuration root,
/// `Some((Some(path), start))` under a literal `GetSection(..)` chain, `None`
/// when unknown. `start` is the receiver's first byte (the read's site).
/// `depth` bounds the chain walk.
fn dotnet_receiver(
    source: &str,
    pos: usize,
    indexer: bool,
    depth: usize,
) -> Option<(Option<String>, usize)> {
    let b = source.as_bytes();
    let mut end = pos;
    if !indexer {
        // A fluent chain may break the line before `.GetValue<..>(..)`.
        while end > 0 && b[end - 1].is_ascii_whitespace() {
            end -= 1;
        }
    }
    // `config?["X"]`, `config!.GetValue<..>(..)`.
    if end > 0 && matches!(b[end - 1], b'?' | b'!') {
        end -= 1;
    }
    if end == 0 {
        return None;
    }
    if b[end - 1] == b')' {
        if depth >= 8 {
            return None;
        }
        let (section, dot) = section_call_ending_at(source, end - 1)?;
        let (outer, start) = dotnet_receiver(source, dot, false, depth + 1)?;
        return Some((Some(join_setting(outer, &section)), start));
    }
    let mut start = end;
    let ident_byte = |c: u8| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'@');
    while start > 0 && ident_byte(b[start - 1]) {
        start -= 1;
    }
    if start == end {
        return None;
    }
    let ident: String = source[start..end]
        .chars()
        .filter(|c| !matches!(c, '_' | '@'))
        .collect::<String>()
        .to_ascii_lowercase();
    let configish = ident.ends_with("configuration") || ident.ends_with("config");
    let root = if indexer { configish } else { configish || !ident.contains("section") };
    root.then_some((None, start))
}

/// The literal section of a `.GetSection("A")` / `.GetRequiredSection("A")`
/// call whose `)` is at byte `close`, and the byte of that call's `.`.
fn section_call_ending_at(source: &str, close: usize) -> Option<(String, usize)> {
    let b = source.as_bytes();
    let mut j = close;
    while j > 0 && b[j - 1].is_ascii_whitespace() {
        j -= 1;
    }
    if j == 0 || b[j - 1] != b'"' {
        return None;
    }
    let lit_end = j - 1;
    let lit_start = source[..lit_end].rfind('"')?;
    let mut k = lit_start;
    if k > 0 && b[k - 1] == b'@' {
        k -= 1;
    }
    while k > 0 && b[k - 1].is_ascii_whitespace() {
        k -= 1;
    }
    if k == 0 || b[k - 1] != b'(' {
        return None;
    }
    k -= 1;
    let head = source[..k].trim_end();
    for name in ["GetRequiredSection", "GetSection"] {
        if let Some(rest) = head.strip_suffix(name)
            && rest.ends_with('.')
        {
            return Some((source[lit_start + 1..lit_end].to_string(), rest.len() - 1));
        }
    }
    None
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
        let out = extract_config_reads(src, "", module_id(repo), repo);
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
        let out = extract_config_reads(src, "", module_id(repo), repo);
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
        let out = extract_config_reads(src, "", module_id(repo), repo);
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
        let out = extract_config_reads(src, "", module_id(repo), repo);
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
        let out = extract_config_reads(src, "", module_id(repo), repo);
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
        let out = extract_config_reads(src, "", module_id(repo), repo);
        let keys = config_keys(&out);
        assert!(keys.contains(&"config:env:DATABASE_URL".to_string()));
        assert!(keys.contains(&"config:env:API_KEY".to_string()));
    }

    #[test]
    fn dotnet_environment_read() {
        let repo = RepoId(1);
        let src = r#"
public class Db
{
    public NpgsqlConnection Open() => new NpgsqlConnection(Environment.GetEnvironmentVariable("DATABASE_URL"));
    public string Region() => Environment.GetEnvironmentVariable("AWS_REGION", EnvironmentVariableTarget.Process);
    public string Dynamic(string name) => Environment.GetEnvironmentVariable(name);
}
"#;
        let out = extract_config_reads(src, "Db.cs", module_id(repo), repo);
        let mut keys = config_keys(&out);
        keys.sort();
        assert_eq!(keys, vec!["config:env:AWS_REGION", "config:env:DATABASE_URL"]);
        let reads: Vec<_> = out
            .edges
            .iter()
            .filter(|e| e.category == edge_category::READS_CONFIG)
            .collect();
        assert_eq!(reads.len(), 2);
        assert!(reads.iter().all(|e| e.from == module_id(repo)));
        // One site per literal read, each at its needle; the non-literal
        // `GetEnvironmentVariable(name)` contributes neither node nor site.
        assert_eq!(out.sites.len(), 2);
        let db =
            NodeId::from_parts(GRAPH_TYPE, repo, node_kind::CONFIG_KEY, "config:env:DATABASE_URL");
        let (_, off) = out.sites.iter().find(|(id, _)| *id == db).expect("DATABASE_URL site");
        assert!(src[*off..].starts_with("Environment.GetEnvironmentVariable(\"DATABASE_URL\")"));
    }

    #[test]
    fn php_env_idioms() {
        let repo = RepoId(1);
        let src = r#"
<?php
$db = getenv('DATABASE_URL');
$key = $_ENV['API_KEY'];
"#;
        let out = extract_config_reads(src, "", module_id(repo), repo);
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
        let out = extract_config_reads(src, "", module_id(repo), repo);
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
        let out = extract_config_reads(src, "", module_id(repo), repo);
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
        let out = extract_config_reads(&src, "", module_id(repo), repo);
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
        let out = extract_config_reads(src, "", module_id(repo), repo);
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

    /// CJ.1c: in a Rust or Python file a read needle starting inside a
    /// string literal or comment is no read; one in code (a Python f-string's
    /// replacement field included) keeps its key and its site.
    #[test]
    fn literal_env_reads_mint_nothing_in_rust_and_python() {
        let repo = RepoId(1);
        let sorted_keys = |out: &ConfigNodes| {
            let mut k = config_keys(out);
            k.sort();
            k
        };
        let rs = "pub fn mode() -> String {\n    std::env::var(\"APP_MODE\").unwrap_or_default()\n}\n\
                  /// `os.getenv(\"DOC_KEY\")`\n\
                  fn t() { let s = \"os.getenv('JWT_SECRET')\"; }\n";
        let out = extract_config_reads(rs, "src/x.rs", module_id(repo), repo);
        assert_eq!(sorted_keys(&out), vec!["config:env:APP_MODE".to_string()]);
        let at = rs.find("std::env::var(").unwrap();
        assert_eq!(out.sites.iter().map(|(_, o)| *o).min(), Some(at));
        assert!(out.sites.iter().all(|(k, _)| *k == env_id(repo, "APP_MODE")));
        // The same text read as TypeScript keeps every HEAD read.
        let ts = extract_config_reads(rs, "src/x.ts", module_id(repo), repo);
        assert_eq!(
            sorted_keys(&ts),
            ["config:env:APP_MODE", "config:env:DOC_KEY", "config:env:JWT_SECRET"]
        );
        // Python: a plain read and one inside an f-string, sites at the needle.
        let py = "import os\nDB_URL = os.getenv(\"DB_URL\")\n\
                  URL = f\"redis://{os.getenv('REDIS_HOST')}:6379/0\"\n\
                  # os.getenv('COMMENTED')\nS = \"os.environ['IN_STRING']\"\n";
        let out = extract_config_reads(py, "app/settings.py", module_id(repo), repo);
        assert_eq!(sorted_keys(&out), ["config:env:DB_URL", "config:env:REDIS_HOST"]);
        for (key, needle) in [("DB_URL", "os.getenv(\"DB_URL"), ("REDIS_HOST", "os.getenv('REDIS")] {
            let at = py.find(needle).unwrap();
            let offs: Vec<usize> = out
                .sites
                .iter()
                .filter(|(k, _)| *k == env_id(repo, key))
                .map(|(_, o)| *o)
                .collect();
            assert_eq!(offs.iter().min(), Some(&at), "{key}: {offs:?}");
        }
    }

    #[test]
    fn define_side_carries_no_sites() {
        let repo = RepoId(1);
        assert!(extract_dotenv_defs("A=1\n", module_id(repo), repo).sites.is_empty());
        assert!(extract_dockerfile_defs("ENV B=2\n", module_id(repo), repo).sites.is_empty());
    }

    // ------------------------------------------------------------------
    // CL.7b — .NET settings: `config:setting:<Section:Key>`.
    // ------------------------------------------------------------------

    fn setting_id(repo: RepoId, key: &str) -> NodeId {
        NodeId::from_parts(
            GRAPH_TYPE,
            repo,
            node_kind::CONFIG_KEY,
            &format!("config:setting:{key}"),
        )
    }

    /// The ENV-cell payload of the node with full `qname`.
    fn payload_of(out: &ConfigNodes, qname: &str) -> Option<String> {
        let (id, _) = out.nav.qname_by_id.iter().find(|(_, q)| *q == qname)?;
        let node = out.nodes.iter().find(|n| n.id == *id)?;
        node.cells.iter().find(|c| c.kind == cell_type::ENV).map(|c| match &c.payload {
            CellPayload::Text(s) | CellPayload::Json(s) => s.clone(),
            CellPayload::Bytes(_) => String::new(),
        })
    }

    /// Every ENV / EVIDENCE payload in `out`, for leak checks.
    fn all_payloads(out: &ConfigNodes) -> String {
        out.nodes
            .iter()
            .flat_map(|n| n.cells.iter())
            .chain(out.edges.iter().flat_map(|e| e.cells.iter()))
            .map(|c| match &c.payload {
                CellPayload::Text(s) | CellPayload::Json(s) => s.clone(),
                CellPayload::Bytes(_) => String::new(),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn setting_key_validator() {
        for ok in ["Stripe:SecretKey", "Logging:LogLevel:Microsoft.AspNetCore", "Endpoints:0:Url", "Db", "a-b_c.d"]
        {
            assert!(is_valid_setting_key(ok), "{ok}");
        }
        let long = "a".repeat(129);
        for bad in ["", "Stripe:", ":Key", "A::B", "has space", "$schema", "a\"b", "x{0}", long.as_str()] {
            assert!(!is_valid_setting_key(bad), "{bad}");
        }
        assert!(is_valid_setting_key(&"a".repeat(128)));
    }

    #[test]
    fn dotnet_indexer_and_getvalue_reads() {
        let repo = RepoId(1);
        let src = r#"using Microsoft.Extensions.Configuration;

public class Payments
{
    private readonly IConfiguration _config;
    private readonly Dictionary<string, string> cache = new();

    public Payments(IConfiguration configuration)
    {
        var key = configuration["Stripe:SecretKey"];
        var retries = _config.GetValue<int>("Retry:Max");
        var limits = configuration
            .GetValue<Dictionary<string, int>>("Limits:PerMinute", null);
        var section = configuration.GetSection("Logging");
        var req = _config.GetRequiredSection("Features");
        var db = configuration.GetConnectionString("Db");
        var nested = configuration.GetSection("Smtp")["Host"];
        var deep = configuration.GetSection("A").GetSection("B").GetValue<string>("C");
        var verbatim = configuration[@"Cdn:BaseUrl"];
        var c = cache["k:v"];
        var rel = stripeSection.GetValue<string>("Relative");
        var dynamicKey = configuration[name];
        var concat = configuration["Prefix:" + name];
        var interp = configuration[$"Tenant:{id}"];
        var values = configuration.GetValues("NotAMethod");
    }
}
"#;
        let out = extract_config_reads(src, "Payments.cs", module_id(repo), repo);
        let mut keys = config_keys(&out);
        keys.sort();
        assert_eq!(
            keys,
            [
                "config:setting:A",
                "config:setting:A:B",
                "config:setting:A:B:C",
                "config:setting:Cdn:BaseUrl",
                "config:setting:ConnectionStrings:Db",
                "config:setting:Features",
                "config:setting:Limits:PerMinute",
                "config:setting:Logging",
                "config:setting:Retry:Max",
                "config:setting:Smtp",
                "config:setting:Smtp:Host",
                "config:setting:Stripe:SecretKey",
            ]
        );
        // Read side: READS_CONFIG from the module, no value cell, one site
        // per read at its receiver / method name.
        assert!(out.edges.iter().all(|e| e.category == edge_category::READS_CONFIG
            && e.from == module_id(repo)));
        assert!(out.nodes.iter().all(|n| n.cells.is_empty()));
        assert_eq!(out.sites.len(), keys.len());
        let site = |key: &str| {
            out.sites.iter().find(|(id, _)| *id == setting_id(repo, key)).map(|(_, o)| *o).unwrap()
        };
        assert!(src[site("Stripe:SecretKey")..].starts_with("configuration[\"Stripe:SecretKey\"]"));
        assert!(src[site("Retry:Max")..].starts_with("GetValue<int>(\"Retry:Max\")"));
        assert!(src[site("Limits:PerMinute")..].starts_with("GetValue<Dictionary"));
        assert!(src[site("Smtp:Host")..].starts_with("configuration.GetSection(\"Smtp\")"));
        // Nothing reads without an IConfiguration mention.
        let plain = "var x = config[\"A:B\"];\nvar y = settings.GetValue<int>(\"C:D\");\n";
        assert!(extract_config_reads(plain, "X.cs", module_id(repo), repo).nodes.is_empty());
        // A minimal-API Program.cs reads through `builder.Configuration`.
        let program = "var builder = WebApplication.CreateBuilder(args);\n\
                       var cs = builder.Configuration[\"Redis:Host\"];\n";
        let out = extract_config_reads(program, "Program.cs", module_id(repo), repo);
        assert_eq!(config_keys(&out), ["config:setting:Redis:Host"]);
    }

    /// The setting scan runs on C# files only: the same text read as
    /// TypeScript (an `interface IConfiguration` and a `config[..]` lookup) or
    /// as Rust (the scanner's own test text) mints nothing.
    #[test]
    fn dotnet_config_reads_only_in_csharp_files() {
        let repo = RepoId(1);
        let src = "interface IConfiguration { apiUrl: string }\nconst u = this.config[\"apiUrl\"];\n";
        for path in ["src/app/config.ts", "src/x.rs", "Program.csx", "cs", ""] {
            let out = extract_config_reads(src, path, module_id(repo), repo);
            assert!(out.nodes.is_empty(), "{path}: {:?}", config_keys(&out));
        }
        let cs = extract_config_reads(src, "src/Config.CS", module_id(repo), repo);
        assert_eq!(config_keys(&cs), ["config:setting:apiUrl"]);
    }

    #[test]
    fn appsettings_flattens_objects_and_arrays() {
        let repo = RepoId(1);
        let src = "\u{feff}{\n  // Kestrel\n  \"Logging\": { \"LogLevel\": { \"Default\": \"Information\", \"Microsoft.AspNetCore\": \"Warning\" } },\n  \
                   \"AllowedHosts\": \"*\",\n  \"Retry\": { \"Max\": 3, \"Enabled\": true, \"Backoff\": null, \"Empty\": \"\" },\n  \
                   /* endpoints */ \"Endpoints\": [ { \"Url\": \"http://+:80\" }, { \"Url\": \"https://+:443\", } ],\n  \
                   \"Tags\": [\"a\", \"b\"],\n  \"Nothing\": {},\n  \"$schema\": \"https://example.test/schema\",\n  \
                   \"Note\": \"// not a comment, /* nor this */\",\n}\n";
        let out = extract_settings_json_defs(src, "appsettings.json", module_id(repo), repo);
        let mut keys = config_keys(&out);
        keys.sort();
        assert_eq!(
            keys,
            [
                "config:setting:AllowedHosts",
                "config:setting:Endpoints:0:Url",
                "config:setting:Endpoints:1:Url",
                "config:setting:Logging:LogLevel:Default",
                "config:setting:Logging:LogLevel:Microsoft.AspNetCore",
                "config:setting:Note",
                "config:setting:Retry:Backoff",
                "config:setting:Retry:Empty",
                "config:setting:Retry:Enabled",
                "config:setting:Retry:Max",
                "config:setting:Tags:0",
                "config:setting:Tags:1",
            ]
        );
        assert_eq!(
            payload_of(&out, "config:setting:Retry:Max").as_deref(),
            Some(r#"{"value":"3","source":"appsettings","redacted":false}"#)
        );
        assert!(payload_of(&out, "config:setting:Retry:Enabled").unwrap().contains(r#""value":"true""#));
        assert!(payload_of(&out, "config:setting:Endpoints:1:Url").unwrap().contains(r#""value":"https://+:443""#));
        assert!(payload_of(&out, "config:setting:Note").unwrap().contains("// not a comment, /* nor this */"));
        // null and "" declare the key with no value.
        assert_eq!(payload_of(&out, "config:setting:Retry:Backoff"), None);
        assert_eq!(payload_of(&out, "config:setting:Retry:Empty"), None);
        // Every key is a DEFINES_CONFIG target of the file's module, named by
        // its full path.
        assert_eq!(out.edges.len(), keys.len());
        assert!(out.edges.iter().all(|e| e.category == edge_category::DEFINES_CONFIG
            && e.from == module_id(repo)));
        let id = setting_id(repo, "Retry:Max");
        assert_eq!(out.nav.name_by_id[&id], "Retry:Max");
        assert!(out.sites.is_empty());
    }

    #[test]
    fn appsettings_secret_values_are_redacted() {
        let repo = RepoId(1);
        let src = r#"{
  "Stripe": { "SecretKey": "sk_test_FIXTUREPLACEHOLDER", "PublishableKey": "pk_test_abc" },
  "Jwt": { "Key": "jwt-signing-material", "Issuer": "payments" },
  "Smtp": { "Password": "hunter2" },
  "ConnectionStrings": { "Db": "Host=db;Database=app" },
  "Redis": { "ConnectionString": "cache:6379,ssl=false" },
  "Storage": { "Main": "DefaultEndpointsProtocol=https;AccountName=x;AccountKey=c2VjcmV0" },
  "Upstream": { "Url": "https://svc:p4ss@users.internal/api" }
}"#;
        let out = extract_settings_json_defs(src, "appsettings.Production.json", module_id(repo), repo);
        for key in [
            "Stripe:SecretKey",
            "Stripe:PublishableKey",
            "Jwt:Key",
            "Smtp:Password",
            "ConnectionStrings:Db",
            "Redis:ConnectionString",
            "Storage:Main",
        ] {
            assert_eq!(
                payload_of(&out, &format!("config:setting:{key}")).as_deref(),
                Some(r#"{"source":"appsettings","redacted":true}"#),
                "{key}"
            );
        }
        assert!(payload_of(&out, "config:setting:Jwt:Issuer").unwrap().contains(r#""value":"payments""#));
        // A URL keeps its host and loses its userinfo (A13.7).
        let up = payload_of(&out, "config:setting:Upstream:Url").unwrap();
        assert!(up.contains(r#""value":"https://***@users.internal/api""#), "{up}");
        let all = all_payloads(&out);
        for leak in ["sk_test_", "pk_test_", "jwt-signing", "hunter2", "Host=db", "cache:6379", "c2VjcmV0", "p4ss"] {
            assert!(!all.contains(leak), "{leak} leaked: {all}");
        }
        // The value never becomes an identity either.
        assert!(out.nav.name_by_id.values().all(|n| !n.contains("sk_test_")));
    }

    #[test]
    fn appsettings_parse_error_mints_nothing() {
        let repo = RepoId(1);
        for bad in ["{ \"Stripe\": { \"SecretKey\": ", "[ { \"A\": 1 } ]", "\"just a string\"", ""] {
            let out = extract_settings_json_defs(bad, "appsettings.json", module_id(repo), repo);
            assert!(out.nodes.is_empty() && out.edges.is_empty(), "{bad:?}");
        }
    }

    #[test]
    fn appsettings_key_cap() {
        let repo = RepoId(1);
        let body: Vec<String> = (0..SETTINGS_KEY_CAP + 5).map(|i| format!("\"K{i}\": {i}")).collect();
        let src = format!("{{ {} }}", body.join(", "));
        let out = extract_settings_json_defs(&src, "appsettings.json", module_id(repo), repo);
        assert_eq!(out.nodes.len(), SETTINGS_KEY_CAP);
    }

    #[test]
    fn dotnet_settings_paths() {
        for ok in [
            "appsettings.json",
            "src/Api/appsettings.json",
            "src/Api/appsettings.Development.json",
            "AppSettings.Production.JSON",
            "svc\\appsettings.Local.json",
            "src/Apps/cms-api/configs/appsettings-prod.json",
            "configs/appsettingsAdminOne-prod.json",
        ] {
            assert!(is_dotnet_settings_path(ok), "{ok}");
        }
        for bad in
            ["myappsettings.json", "appsettings.json.bak", "launchSettings.json", "appsettings.yaml", "appsettings x.json"]
        {
            assert!(!is_dotnet_settings_path(bad), "{bad}");
        }
    }

    #[test]
    fn dotnet_env_override_defines_the_setting() {
        let repo = RepoId(1);
        let src = "ENV Stripe__SecretKey=sk_live_x\nENV Retry__Max=3\nENV PORT=8080\n";
        let out = extract_dockerfile_defs(src, module_id(repo), repo);
        let mut keys = config_keys(&out);
        keys.sort();
        assert_eq!(
            keys,
            [
                "config:env:PORT",
                "config:env:Retry__Max",
                "config:env:Stripe__SecretKey",
                "config:setting:Retry:Max",
                "config:setting:Stripe:SecretKey",
            ]
        );
        for key in ["Stripe:SecretKey", "Retry:Max"] {
            let edges: Vec<&Edge> =
                out.edges.iter().filter(|e| e.to == setting_id(repo, key)).collect();
            assert_eq!(edges.len(), 1, "{key}");
            assert_eq!(edges[0].category, edge_category::DEFINES_CONFIG);
            assert_eq!(edges[0].from, module_id(repo));
            let ev = Evidence::of(edges[0]).expect("override edge carries EVIDENCE");
            assert_eq!(ev.emitter, "extractor:dockerfile");
            assert_eq!(ev.rule.as_deref(), Some("dotnet_env_override"));
        }
        // The env edges are untouched: no evidence yet (route.rs stamps them).
        let env_edge = out.edges.iter().find(|e| e.to == env_id(repo, "Stripe__SecretKey")).unwrap();
        assert!(Evidence::of(env_edge).is_none());
        assert_eq!(
            payload_of(&out, "config:setting:Stripe:SecretKey").as_deref(),
            Some(r#"{"source":"dockerfile","redacted":true}"#)
        );
        assert_eq!(
            payload_of(&out, "config:setting:Retry:Max").as_deref(),
            Some(r#"{"value":"3","source":"dockerfile","redacted":false}"#)
        );
        assert!(!all_payloads(&out).contains("sk_live_x"));
        // Item order: each override follows its env define.
        let order: Vec<&String> = out.nodes.iter().map(|n| &out.nav.qname_by_id[&n.id]).collect();
        assert_eq!(
            order,
            [
                "config:env:Stripe__SecretKey",
                "config:setting:Stripe:SecretKey",
                "config:env:Retry__Max",
                "config:setting:Retry:Max",
                "config:env:PORT",
            ]
        );

        // .env: a three-segment override, emitter extractor:dotenv.
        let out = extract_dotenv_defs("Logging__LogLevel__Default=Warning\n", module_id(repo), repo);
        let edge = out
            .edges
            .iter()
            .find(|e| e.to == setting_id(repo, "Logging:LogLevel:Default"))
            .expect("dotenv override");
        assert_eq!(Evidence::of(edge).unwrap().emitter, "extractor:dotenv");
        assert!(
            payload_of(&out, "config:setting:Logging:LogLevel:Default")
                .unwrap()
                .contains(r#""value":"Warning""#)
        );

        // k8s env list: emitter extractor:yaml, and the setting's cell is the
        // one an appsettings leaf of that key gets (ConnectionStrings:* is
        // redacted), while the env node keeps its env-track cell.
        let k8s = concat!(
            "spec:\n",
            "  containers:\n",
            "    - name: api\n",
            "      env:\n",
            "        - name: ConnectionStrings__Db\n",
            "          value: Host=db\n",
        );
        let out = extract_yaml_env_defs(k8s, module_id(repo), repo);
        let edge = out
            .edges
            .iter()
            .find(|e| e.to == setting_id(repo, "ConnectionStrings:Db"))
            .expect("k8s override");
        assert_eq!(Evidence::of(edge).unwrap().emitter, "extractor:yaml");
        let setting = payload_of(&out, "config:setting:ConnectionStrings:Db").unwrap();
        assert_eq!(setting, r#"{"source":"k8s","redacted":true}"#);
        assert!(!setting.contains("Host=db"));
        assert_eq!(
            env_payload(&out, "ConnectionStrings__Db").as_deref(),
            Some(r#"{"value":"Host=db","source":"k8s","redacted":false}"#)
        );

        // compose environment map: emitter extractor:yaml too.
        let compose = "services:\n  api:\n    environment:\n      Retry__Max: 5\n";
        let out = extract_yaml_env_defs(compose, module_id(repo), repo);
        let edge = out.edges.iter().find(|e| e.to == setting_id(repo, "Retry:Max")).unwrap();
        assert_eq!(Evidence::of(edge).unwrap().emitter, "extractor:yaml");
    }

    #[test]
    fn dotnet_env_override_needs_two_nonempty_segments() {
        let env = |name: &str| ConfigDef::bare(name, "dockerfile");
        for none in ["__X", "X__", "A____B", "PORT", "A_B"] {
            assert!(dotnet_env_override(&env(none)).is_none(), "{none}");
        }
        let ov = dotnet_env_override(&env("A__B__C")).expect("A__B__C overrides");
        assert_eq!((ov.name.as_str(), ov.flavor, ov.source), ("A:B:C", Flavor::Setting, "dockerfile"));
        // Only an env define overrides: a secret or flag never does.
        assert!(dotnet_env_override(&ConfigDef::flag("A__B", "flipt")).is_none());
        // A read states no override.
        let repo = RepoId(1);
        let out = extract_config_reads("os.getenv('Stripe__SecretKey')\n", "", module_id(repo), repo);
        assert_eq!(config_keys(&out), ["config:env:Stripe__SecretKey"]);
    }
}
