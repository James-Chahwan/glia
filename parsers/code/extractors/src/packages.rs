//! Package dependency extraction (v0.4.x — task #10).
//!
//! Emits `PACKAGE_DEP` nodes per declared dependency in a manifest file with
//! `DEPENDS_ON` edges from the manifest's module. The identity resolver's
//! `package_name` rule then joins one package's nodes across repos into a
//! SAME_AS star (DD.5, DD.16) to surface "service A and service B both depend
//! on `react@^18`" cross-service signal.
//!
//! Single qname shape: `package:<ecosystem>:<name>`. Ecosystems:
//!   - `npm`        — package.json `dependencies` / `devDependencies` /
//!     `peerDependencies`
//!   - `pypi`       — pyproject.toml `[project] dependencies`,
//!     `[project.optional-dependencies]`, `[dependency-groups]` (PEP 735),
//!     `[tool.poetry.dependencies]` and every Poetry group, plain or dotted
//!     (`[tool.poetry.dependencies.<name>]`, GD.37), requirements.txt
//!   - `cargo`      — Cargo.toml `[dependencies]` / `[dev-dependencies]` /
//!     `[build-dependencies]`, their dotted forms (`[dependencies.<name>]`) and
//!     target tables (`[target.<cfg or triple>.dependencies]`, GD.37; recorded
//!     like any dependency, no condition cell)
//!   - `gomod`      — go.mod `require <name> <version>`
//!   - `rubygems`   — Gemfile `gem '<name>', ...`
//!   - `composer`   — composer.json `require` / `require-dev`
//!   - `maven`      — FD.31: pom.xml `<dependencies><dependency>` (`groupId:artifactId`),
//!     Gradle `build.gradle` / `build.gradle.kts` configuration calls
//!     (`implementation("g:a:v")`, the `group:` / `name:` map form)
//!   - `pub`        — FD.31: pubspec.yaml `dependencies:` / `dev_dependencies:`
//!     (an `sdk:` entry such as `flutter: sdk: flutter` is the SDK, no package)
//!   - `hex`        — FD.31: mix.exs `{:name, ..}` tuples of the `deps` function
//!   - `nuget`      — FD.31: `*.csproj` / `*.fsproj` / `*.vbproj`
//!     `<PackageReference Include="N">`
//!
//! Each extractor is a pure function over the file's source. Caller dispatches
//! by file basename via `extract_for_path` (the manifest route) or, for a file
//! that keeps another route, [`extract_deps_beside`]: `pubspec.yaml` stays a
//! YAML file and `mix.exs` an Elixir one, and their dependency read is appended
//! to that file's MODULE. pom.xml, the Gradle build files and the .NET project
//! files have no other route, so [`is_manifest_path`] names them and the walk
//! reads them for this read alone.
//!
//! Out of scope:
//!   - Gradle version catalogs (`implementation(libs.okhttp)`): the build file
//!     names no artifact; counted `catalog_refs=` in the `[packages]` marker
//!   - Maven `<dependencyManagement>` and plugin `<dependencies>` (versions
//!     and build tooling, not the project's own dependencies)
//!   - Lockfiles (`package-lock.json`, `Cargo.lock`, `go.sum`, etc.) — manifest
//!     declarations cover declared intent
//!   - Version-pinning analysis (we extract the name, not the constraint)
//!
//! GH.9: a `package.json` that is a host's plugin manifest
//! (`code_domain::hosts::HostManifest`: `engines.vscode`) also declares that
//! host's commands and settings (`host_manifest`), printing
//! `[host-manifest] host=<h> commands=<c> config=<g> path=<p>`.

use glia_code_domain::evidence::Evidence;
use glia_code_domain::{CodeNav, GRAPH_TYPE, edge_category, hosts, node_kind};
use glia_core::{Confidence, Edge, Node, NodeId, RepoId};

use crate::anchor;

pub struct PackageNodes {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub nav: CodeNav,
    /// FD.31 fired_on, one per manifest of the four ecosystems it reads
    /// (`maven`, `pub`, `hex`, `nuget`), even one that declares nothing:
    /// `[packages] eco=<e> deps=<n> path=<p>` (a Gradle file adds
    /// `catalog_refs=<c>` before `path=`). FD.77 adds a `Cargo.toml`
    /// (`eco=cargo`) and a `pyproject.toml` (`eco=pypi`), each with
    /// `array_tables=<t>` before `path=`. `None` for every other manifest.
    pub marker: Option<String>,
}

/// Detect manifest type from path basename and extract dependencies. Returns
/// an empty result when the path isn't a recognised manifest.
pub fn extract_for_path(source: &str, path: &str, module_id: NodeId, repo: RepoId) -> PackageNodes {
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
        _ => return extract_deps_beside(source, path, module_id, repo),
    };
    let mut out = build_nodes(names_with_eco, module_id, repo);
    // GH.9: a host's plugin manifest (VS Code: `engines.vscode`) declares
    // the host's commands and settings.
    if base == "package.json" {
        out.marker = host_manifest(source, path, module_id, repo, &mut out);
    }
    // FD.77: the two TOML manifests say what they read, and how many
    // array-of-tables blocks (Cargo targets, Poetry sources) they skipped.
    let toml_eco = match base {
        "Cargo.toml" => Some("cargo"),
        "pyproject.toml" => Some("pypi"),
        _ => None,
    };
    if let Some(eco) = toml_eco {
        out.marker = Some(format!(
            "[packages] eco={eco} deps={} array_tables={} path={path}",
            out.nodes.len(),
            count_toml_array_tables(source)
        ));
    }
    out
}

/// FD.31: the dependencies of a pom.xml, a `build.gradle` / `build.gradle.kts`,
/// a `pubspec.yaml`, a `mix.exs` or a `.csproj` / `.fsproj` / `.vbproj`, as
/// `package:<maven|pub|hex|nuget>:<name>` PACKAGE_DEPs each with a DEPENDS_ON
/// from `module_id`, and the file's `[packages]` marker. Empty (no marker) for
/// any other path. The router calls it beside the file's own route for a
/// `pubspec.yaml` (YAML) and a `mix.exs` (Elixir), and through
/// [`extract_for_path`] for the rest.
pub fn extract_deps_beside(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
) -> PackageNodes {
    let base = path.rsplit('/').next().unwrap_or(path);
    let (eco, names, catalog_refs) = match base {
        "pom.xml" => ("maven", scan_pom_xml(source), None),
        "build.gradle" | "build.gradle.kts" => {
            let (names, catalog) = scan_gradle(source);
            ("maven", names, Some(catalog))
        }
        "pubspec.yaml" => ("pub", scan_pubspec(source), None),
        "mix.exs" => ("hex", scan_mix_exs(source), None),
        _ if is_dotnet_project_name(base) => ("nuget", scan_package_references(source), None),
        _ => return build_nodes(Vec::new(), module_id, repo),
    };
    let mut out = build_nodes(
        names.into_iter().map(|n| (eco, n)).collect(),
        module_id,
        repo,
    );
    let catalog = catalog_refs.map_or(String::new(), |c| format!(" catalog_refs={c}"));
    out.marker = Some(format!(
        "[packages] eco={eco} deps={}{catalog} path={path}",
        out.nodes.len()
    ));
    out
}

/// True when the path is a recognised manifest filename: the walk reads it
/// and the router sends it down the manifest branch, so no language parser
/// sees it. FD.31 adds pom.xml, `build.gradle`, `build.gradle.kts` and the
/// .NET project files, which no parser reads; `pubspec.yaml` and `mix.exs`
/// keep their YAML / Elixir route and are not named here. DH.10: a
/// `*.gemspec` too, read for the executables it ships
/// (`cli::extract_manifest_binaries`) and for no dependency
/// ([`extract_for_path`] has no arm for it), so the walk reads it, the router
/// sends it down the manifest branch and no language parser sees it.
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
            | "pom.xml"
            | "build.gradle"
            | "build.gradle.kts"
    ) || is_gemspec_name(base)
        || is_dotnet_project_name(base)
}

/// FD.31: a .NET project file name, `<name>.csproj` / `.fsproj` / `.vbproj`
/// (extension ASCII case ignored; a bare `.csproj` is not one).
fn is_dotnet_project_name(base: &str) -> bool {
    base.rsplit_once('.').is_some_and(|(stem, ext)| {
        !stem.is_empty()
            && ["csproj", "fsproj", "vbproj"]
                .iter()
                .any(|e| ext.eq_ignore_ascii_case(e))
    })
}

/// DH.10: a RubyGems spec file name, `<name>.gemspec` (a bare `.gemspec` is
/// not one).
pub(crate) fn is_gemspec_name(base: &str) -> bool {
    base.len() > ".gemspec".len() && base.ends_with(".gemspec")
}

fn build_nodes(deps: Vec<(&'static str, String)>, module_id: NodeId, repo: RepoId) -> PackageNodes {
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
            cells: Vec::new(),
        });
    }
    PackageNodes {
        nodes,
        edges,
        nav,
        marker: None,
    }
}

// ----------------------------------------------------------------------------
// GH.9: a host's plugin manifest (VS Code's package.json).
// ----------------------------------------------------------------------------

/// GH.9: when `source` (a `package.json`) names a host under `engines`
/// ([`hosts::manifest_host_for`]), each `contributes.commands[].command` is
/// the host's CLI_COMMAND `host_cmd:<host>:<id>` (the qname the extension's
/// `registerCommand` mints, so the two are one node) DEFINES-d from the
/// manifest's MODULE, and each key of `contributes.configuration`'s
/// `properties` (one object or an array of them) the CONFIG_KEY
/// `config:<host>:<key>` DEFINES_CONFIG from it, each located at its text.
/// Returns the `[host-manifest]` marker, `None` for a file that is no host's
/// manifest (or no JSON object). Only the keys are nodes, never a title,
/// default or description.
fn host_manifest(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
    out: &mut PackageNodes,
) -> Option<String> {
    let doc: serde_json::Value = serde_json::from_str(source).ok()?;
    let engines = doc.get("engines").and_then(serde_json::Value::as_object)?;
    let host = hosts::manifest_host_for("package.json", |e| engines.contains_key(e))?;
    let contributes = doc.get("contributes");
    let from = source.find("\"contributes\"").unwrap_or(0);
    let one_or_many = |v: Option<&serde_json::Value>| -> Vec<serde_json::Value> {
        match v {
            Some(serde_json::Value::Array(items)) => items.clone(),
            Some(o @ serde_json::Value::Object(_)) => vec![o.clone()],
            _ => Vec::new(),
        }
    };
    let commands: Vec<String> = one_or_many(contributes.and_then(|c| c.get("commands")))
        .iter()
        .filter_map(|c| c.get("command").and_then(serde_json::Value::as_str))
        .filter(|c| !c.is_empty())
        .map(str::to_string)
        .collect();
    let mut keys: Vec<String> = Vec::new();
    for section in one_or_many(contributes.and_then(|c| c.get("configuration"))) {
        if let Some(props) = section
            .get("properties")
            .and_then(serde_json::Value::as_object)
        {
            keys.extend(props.keys().filter(|k| !k.is_empty()).cloned());
        }
    }
    let (mut n_commands, mut n_config) = (0usize, 0usize);
    let mut refs = Vec::new();
    for id in &commands {
        let row = json_string_row(source, from, id, JsonRole::Value);
        let site = hosts::HostSite::new(
            repo,
            module_id,
            host.name,
            row,
            vec![anchor::position_cell(path, row)],
        );
        let mut sink = hosts::HostSink::new(&mut out.nodes, &mut out.nav, &mut refs);
        let cmd = hosts::push_registration(&site, hosts::Pattern::Command, id, &[], &mut sink);
        if !out
            .edges
            .iter()
            .any(|e| e.from == module_id && e.to == cmd && e.category == edge_category::DEFINES)
        {
            let ev = Evidence::emitter(MANIFEST_EMITTER)
                .rule("host_manifest_command")
                .line(row);
            out.edges.push(
                Edge::new(module_id, cmd, edge_category::DEFINES, Confidence::Strong)
                    .with_cell(ev.to_cell()),
            );
            n_commands += 1;
        }
    }
    for key in &keys {
        let row = json_string_row(source, from, key, JsonRole::Key);
        let site = hosts::HostSite::new(
            repo,
            module_id,
            host.name,
            row,
            vec![anchor::position_cell(path, row)],
        )
        .with_emitter(MANIFEST_EMITTER);
        let found = hosts::ConfigMatch::new(host, hosts::ConfigFlavour::Manifest, key.clone());
        let mut sink = hosts::HostSink::new(&mut out.nodes, &mut out.nav, &mut refs);
        hosts::push_config_key(
            &site,
            &found,
            hosts::ConfigSide::Define,
            module_id,
            &mut sink,
            &mut out.edges,
        );
        n_config += 1;
    }
    Some(format!(
        "[host-manifest] host={} commands={n_commands} config={n_config} path={path}",
        host.name
    ))
}

/// GH.9: the EVIDENCE emitter of a manifest's host edges, the manifest
/// route's own (`extractor:<route key>`).
const MANIFEST_EMITTER: &str = "extractor:manifest";

/// Whether a JSON string is looked up as an object key (`"k":`) or as a
/// value (`: "v"`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum JsonRole {
    Key,
    Value,
}

/// GH.9: the 0-based row of the first occurrence of `s` as a JSON string in
/// the `role` position at or after byte `from` (the `"contributes"` key), 0
/// when none is found.
fn json_string_row(source: &str, from: usize, s: &str, role: JsonRole) -> u32 {
    let Ok(quoted) = serde_json::to_string(s) else {
        return 0;
    };
    let mut at = from.min(source.len());
    while let Some(rel) = source.get(at..).and_then(|t| t.find(&quoted)) {
        let start = at + rel;
        let end = start + quoted.len();
        let fits = match role {
            JsonRole::Key => source[end..].trim_start().starts_with(':'),
            JsonRole::Value => source[..start].trim_end().ends_with(':'),
        };
        if fits {
            let rows = source[..start].bytes().filter(|b| *b == b'\n').count();
            return u32::try_from(rows).unwrap_or(u32::MAX);
        }
        at = end;
    }
    0
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
                while k < bytes.len()
                    && (bytes[k] == b' ' || bytes[k] == b'\t' || bytes[k] == b'\n')
                {
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
// pyproject.toml — PEP 621 `[project]` deps and `[project.optional-dependencies]`,
// PEP 735 `[dependency-groups]`, and Poetry's dependency tables (every group,
// plain or dotted). Both layouts captured via a section walker.
// ----------------------------------------------------------------------------

fn scan_pyproject(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    let sections = parse_toml_sections(source);
    for section in sections.iter().filter(|s| !s.array) {
        let segs = toml_key_segments(&section.name);
        let segs: Vec<&str> = segs.iter().map(String::as_str).collect();
        match segs.as_slice() {
            ["project"] => {
                // PEP-621: `dependencies = ["foo>=1.0", "bar~=2.0"]`.
                if let Some(list) = read_toml_array_field(&section.body, "dependencies") {
                    out.extend(list.iter().filter_map(|item| pep508_package_name(item)));
                }
            }
            // GD.37: PEP 621 makes `optional-dependencies` a table of arrays
            // (extra -> PEP 508 specs), and PEP 735's `[dependency-groups]` is
            // the same shape, its `{include-group = ..}` items naming no package.
            ["project", "optional-dependencies"] | ["dependency-groups"] => {
                for (_, items) in toml_array_values(&section.body) {
                    out.extend(items.iter().filter_map(|item| pep508_package_name(item)));
                }
            }
            // Poetry's dependency tables are key/value tables: the main one,
            // the legacy dev one and every `group.<g>` (GD.37: any group, not
            // only `dev`).
            ["tool", "poetry", "dependencies" | "dev-dependencies"]
            | ["tool", "poetry", "group", _, "dependencies"] => {
                for key in toml_table_keys(&section.body) {
                    if key != "python" {
                        out.push(key);
                    }
                }
            }
            // GD.37: a dotted Poetry table names one dependency.
            ["tool", "poetry", "dependencies" | "dev-dependencies", name]
            | ["tool", "poetry", "group", _, "dependencies", name]
                if *name != "python" && is_pep503_or_ident(name) =>
            {
                out.push((*name).to_string());
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
    /// FD.77: opened by an array-of-tables header (`[[test]]`,
    /// `[[tool.poetry.source]]`); its keys belong to one array element, never
    /// to a dependency table.
    array: bool,
}

/// Split a TOML document into sections by `[header]` and `[[header]]` lines.
/// Returns the section names + raw body text (between header and next
/// section). FD.77: a `[[..]]` header closes the open section like a `[..]`
/// one does and opens a section named by its inner text, marked `array`
/// (with no `]]` it opens an unnamed array section, so its keys still go
/// nowhere); before, it was skipped and every key of the block landed in the
/// body of the section before it. A `[..]` header with no `]` opens nothing.
fn parse_toml_sections(source: &str) -> Vec<TomlSection> {
    let mut out = Vec::new();
    let mut current: Option<TomlSection> = None;
    for line in source.lines() {
        let t = line.trim_start();
        if let Some(inner) = t.strip_prefix("[[") {
            out.extend(current.take());
            let name = toml_header_close(inner, "]]").map_or("", |end| inner[..end].trim());
            current = Some(TomlSection {
                name: name.to_string(),
                body: String::new(),
                array: true,
            });
        } else if let Some(inner) = t.strip_prefix('[') {
            out.extend(current.take());
            current = toml_header_close(inner, "]").map(|end| TomlSection {
                name: inner[..end].trim().to_string(),
                body: String::new(),
                array: false,
            });
        } else if let Some(section) = current.as_mut() {
            section.body.push_str(line);
            section.body.push('\n');
        }
    }
    out.extend(current);
    out.retain(|s| s.array || !s.name.is_empty());
    out
}

/// GD.37: the offset of a header's closing `close` (`]` or `]]`) in the text
/// after its opener, found outside quotes: a quoted key segment such as
/// `'cfg(x = "]")'` may hold a `]`. A basic (`"`) string honours `\` escapes,
/// a literal (`'`) one does not. `None` when the header never closes.
fn toml_header_close(inner: &str, close: &str) -> Option<usize> {
    let bytes = inner.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            q @ (b'"' | b'\'') => {
                i += 1;
                while i < bytes.len() && bytes[i] != q {
                    i += if q == b'"' && bytes[i] == b'\\' { 2 } else { 1 };
                }
            }
            _ if bytes[i..].starts_with(close.as_bytes()) => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}

/// GD.37: a dotted TOML key (a header's name) split into its segments,
/// whitespace around the dots dropped and quoted segments unquoted:
/// `target.'cfg(unix)'.dependencies` -> `["target", "cfg(unix)",
/// "dependencies"]`. Escapes inside a basic string are kept verbatim; no
/// segment this module matches holds one.
fn toml_key_segments(key: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seg = String::new();
    let mut chars = key.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' | '\'' => {
                while let Some(d) = chars.next() {
                    if d == c {
                        break;
                    }
                    if c == '"' && d == '\\' {
                        if let Some(e) = chars.next() {
                            seg.push(d);
                            seg.push(e);
                        }
                        continue;
                    }
                    seg.push(d);
                }
            }
            '.' => out.push(std::mem::take(&mut seg).trim().to_string()),
            _ => seg.push(c),
        }
    }
    out.push(seg.trim().to_string());
    out
}

/// GD.37: every `key = [..]` of a TOML table body (multi-line arrays
/// included), with the array's top-level strings. A string inside an
/// inline-table item (`{ include-group = "test" }`) or a nested array is
/// skipped, and so is a `#` comment inside the array.
fn toml_array_values(body: &str) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    let mut rest = body;
    while !rest.is_empty() {
        let line_end = rest.find('\n').map_or(rest.len(), |e| e + 1);
        let line = &rest[..line_end];
        let t = line.trim_start();
        let value = (!t.starts_with('#'))
            .then(|| t.split_once('='))
            .flatten()
            .map(|(key, value)| (key.trim().trim_matches(|c| c == '"' || c == '\''), value))
            .filter(|(key, value)| !key.is_empty() && value.trim_start().starts_with('['));
        let Some((key, value)) = value else {
            rest = &rest[line_end..];
            continue;
        };
        // `value` is a sub-slice of `rest`: its offset there is the `[`.
        let open = value.trim_start().as_ptr() as usize - rest.as_ptr() as usize;
        let array = &rest[open + 1..];
        let Some(close) = balanced_close(array, b'[', b']') else {
            break;
        };
        out.push((key.to_string(), toml_top_level_strings(&array[..close])));
        let after = &array[close + 1..];
        rest = after.find('\n').map_or("", |e| &after[e + 1..]);
    }
    out
}

/// GD.37: the strings at depth 0 of a TOML array's inner text: items inside
/// `{..}` / `[..]` and `#` comments are skipped.
fn toml_top_level_strings(inner: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = inner.as_bytes();
    let mut depth = 0i32;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'{' | b'[' => depth += 1,
            b'}' | b']' => depth -= 1,
            b'#' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            q @ (b'"' | b'\'') => {
                let start = i + 1;
                let mut j = start;
                while j < bytes.len() && bytes[j] != q {
                    j += if q == b'"' && bytes[j] == b'\\' { 2 } else { 1 };
                }
                if depth == 0 && j <= bytes.len() {
                    out.push(inner[start..j.min(bytes.len())].to_string());
                }
                i = j;
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// FD.77: the number of array-of-tables headers a TOML document holds, the
/// `array_tables=` field of its `[packages]` marker.
fn count_toml_array_tables(source: &str) -> usize {
    parse_toml_sections(source)
        .iter()
        .filter(|s| s.array)
        .count()
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
    if name.is_empty() { None } else { Some(name) }
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
// their dotted forms (`[dependencies.serde]`), and the same under a target
// table (`[target.'cfg(unix)'.dependencies]`, `[target.<triple>.dependencies]`,
// `[target.<spec>.dependencies.<name>]`). A target table's dependencies are
// platform-conditional; they are recorded like any other dependency, with no
// condition cell. `[workspace.dependencies]` stays unread: a member
// re-declares `x = { workspace = true }`, which is read as its own dependency.
// ----------------------------------------------------------------------------

/// GD.37: the Cargo dependency table kinds (with Cargo's legacy underscore
/// spellings, which it still accepts).
fn is_cargo_dep_kind(seg: &str) -> bool {
    matches!(
        seg,
        "dependencies"
            | "dev-dependencies"
            | "build-dependencies"
            | "dev_dependencies"
            | "build_dependencies"
    )
}

fn scan_cargo_toml(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for section in parse_toml_sections(source).into_iter().filter(|s| !s.array) {
        let segs = toml_key_segments(&section.name);
        let segs: Vec<&str> = segs.iter().map(String::as_str).collect();
        // GD.37: a `target.<spec>.` prefix (a quoted cfg or a bare triple)
        // reads like the table it prefixes.
        let segs = match segs.as_slice() {
            ["target", _, rest @ ..] => rest,
            all => all,
        };
        match segs {
            [kind] if is_cargo_dep_kind(kind) => out.extend(toml_table_keys(&section.body)),
            // GD.37: `[dependencies.serde]` is the dependency `serde`; its
            // keys (`version`, `features`) are its settings.
            [kind, name] if is_cargo_dep_kind(kind) && is_pep503_or_ident(name) => {
                out.push((*name).to_string());
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

// ----------------------------------------------------------------------------
// FD.31: pom.xml — `<project><dependencies><dependency>` (and a profile's),
// read by a tag-level scan with an element stack (no XML dependency). The
// dependency is `groupId:artifactId`; its version, scope and type are not
// read. `<dependencyManagement>` (version pins) and a plugin's or an
// extension's `<dependencies>` (build tooling) are no dependency of the
// project. A coordinate holding a `${property}` names no artifact.
// ----------------------------------------------------------------------------

fn scan_pom_xml(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack: Vec<&str> = Vec::new();
    // The `<dependency>` being read: its groupId and artifactId so far.
    let mut dep: Option<(Option<String>, Option<String>)> = None;
    let mut rest = source;
    while let Some(open) = rest.find('<') {
        rest = &rest[open..];
        if let Some(after) = rest.strip_prefix("<!--") {
            rest = after.split_once("-->").map_or("", |(_, r)| r);
            continue;
        }
        if let Some(after) = rest.strip_prefix("<![CDATA[") {
            rest = after.split_once("]]>").map_or("", |(_, r)| r);
            continue;
        }
        let Some(close) = rest.find('>') else {
            break;
        };
        let tag = &rest[1..close];
        rest = &rest[close + 1..];
        if tag.starts_with('?') || tag.starts_with('!') || tag.ends_with('/') {
            continue;
        }
        if let Some(name) = tag.strip_prefix('/') {
            let name = name.trim();
            if let Some(at) = stack.iter().rposition(|t| *t == name) {
                stack.truncate(at);
            }
            if name == "dependency"
                && let Some((Some(g), Some(a))) = dep.take()
            {
                out.push(format!("{g}:{a}"));
            }
            continue;
        }
        let name = tag.split_whitespace().next().unwrap_or("");
        if name == "dependency"
            && stack.last() == Some(&"dependencies")
            && !stack
                .iter()
                .any(|t| matches!(*t, "dependencyManagement" | "plugin" | "extension"))
        {
            dep = Some((None, None));
        } else if stack.last() == Some(&"dependency")
            && let Some((group, artifact)) = dep.as_mut()
        {
            let text = rest.split('<').next().unwrap_or("").trim();
            let ok = is_maven_part(text);
            match name {
                "groupId" => *group = ok.then(|| text.to_string()),
                "artifactId" => *artifact = ok.then(|| text.to_string()),
                _ => {}
            }
        }
        stack.push(name);
    }
    out
}

/// A Maven groupId / artifactId as written: non-empty, `[A-Za-z0-9._-]` only
/// (so a `${property}` or an interpolated Gradle string is none).
fn is_maven_part(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// `group:artifact[:version[:classifier]][@ext]` -> `group:artifact`, when
/// both parts are [`is_maven_part`]s (the version is never read).
fn maven_coordinate(s: &str) -> Option<String> {
    let s = s.split('@').next().unwrap_or(s);
    let mut parts = s.split(':');
    let (g, a) = (parts.next()?.trim(), parts.next()?.trim());
    (is_maven_part(g) && is_maven_part(a)).then(|| format!("{g}:{a}"))
}

// ----------------------------------------------------------------------------
// FD.31: Gradle `build.gradle` (Groovy) / `build.gradle.kts` (Kotlin) — the
// configuration calls inside every `dependencies { .. }` block (a
// `buildscript`'s and a `subprojects`' included; a `constraints { .. }` block
// pins versions and is skipped): `implementation("g:a:v")`,
// `api 'g:a:v'`, `testImplementation(platform("g:a:v"))`, the map form
// `implementation(group = "g", name = "a")` / `implementation group: 'g',
// name: 'a'`, and `kotlin("stdlib")` (`org.jetbrains.kotlin:kotlin-stdlib`).
// `project(":x")`, `files(..)` / `fileTree(..)` and a version-catalog accessor
// (`libs.okhttp`, counted) name no published artifact.
// ----------------------------------------------------------------------------

/// The configuration names Gradle and its Java / Android / Kotlin plugins
/// declare dependencies under; any `<variant>Implementation` / `..Api` /
/// `..CompileOnly` / `..RuntimeOnly` / `..AnnotationProcessor` (a source set's
/// or a build variant's) is one too.
const GRADLE_CONFIGS: &[&str] = &[
    "implementation",
    "api",
    "compileOnly",
    "compileOnlyApi",
    "runtimeOnly",
    "annotationProcessor",
    "developmentOnly",
    "kapt",
    "ksp",
    "classpath",
    "compile",
    "runtime",
    "testCompile",
    "testRuntime",
];

const GRADLE_CONFIG_SUFFIXES: &[&str] = &[
    "Implementation",
    "Api",
    "CompileOnly",
    "RuntimeOnly",
    "AnnotationProcessor",
];

fn is_gradle_config(ident: &str) -> bool {
    GRADLE_CONFIGS.contains(&ident)
        || GRADLE_CONFIG_SUFFIXES.iter().any(|suf| {
            ident.strip_suffix(suf).is_some_and(|prefix| {
                prefix
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_lowercase())
            })
        })
}

/// The `group:artifact` names of a Gradle build file and its count of
/// version-catalog references.
fn scan_gradle(source: &str) -> (Vec<String>, usize) {
    let src = strip_comments(source, CommentStyle::CLike);
    let mut out = Vec::new();
    let mut catalog = 0usize;
    for (start, ident) in idents(&src) {
        if ident != "dependencies" {
            continue;
        }
        let after = &src[start + ident.len()..];
        let open = after.len() - after.trim_start().len();
        if !after[open..].starts_with('{') {
            continue;
        }
        let body_start = start + ident.len() + open + 1;
        let Some(close) = balanced_close(&src[body_start..], b'{', b'}') else {
            continue;
        };
        scan_gradle_block(&src[body_start..body_start + close], &mut out, &mut catalog);
    }
    (out, catalog)
}

/// One `dependencies { .. }` body. A nested `dependencies` block is read by
/// [`scan_gradle`]'s own walk, so only this body's configuration calls are.
fn scan_gradle_block(body: &str, out: &mut Vec<String>, catalog: &mut usize) {
    let mut skip_to = 0usize;
    for (start, ident) in idents(body) {
        if start < skip_to {
            continue;
        }
        let after_ident = start + ident.len();
        let rest = &body[after_ident..];
        let ws = rest.len() - rest.trim_start_matches([' ', '\t']).len();
        let rest = &rest[ws..];
        if matches!(ident, "constraints" | "dependencies") && rest.starts_with('{') {
            if let Some(close) = balanced_close(&rest[1..], b'{', b'}') {
                skip_to = after_ident + ws + 1 + close;
            }
            continue;
        }
        if !is_gradle_config(ident) {
            continue;
        }
        let arg = if let Some(inner) = rest.strip_prefix('(') {
            let Some(close) = balanced_close(inner, b'(', b')') else {
                continue;
            };
            &inner[..close]
        } else if rest.starts_with(['"', '\'']) || rest.starts_with(|c: char| c.is_alphabetic()) {
            // Groovy's call without parentheses: the rest of the line, up
            // to a trailing configuration closure.
            let line = rest.split('\n').next().unwrap_or(rest);
            line.split('{').next().unwrap_or(line)
        } else {
            continue;
        };
        read_gradle_arg(arg.trim(), out, catalog);
    }
}

fn read_gradle_arg(arg: &str, out: &mut Vec<String>, catalog: &mut usize) {
    if arg.starts_with(['"', '\'']) {
        for s in quoted_strings(arg) {
            if let Some(c) = maven_coordinate(&s) {
                out.push(c);
            }
        }
        return;
    }
    let head: String = arg
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    let tail = arg[head.len()..].trim_start();
    match head.as_str() {
        "platform" | "enforcedPlatform" | "testFixtures" if tail.starts_with('(') => {
            if let Some(c) = quoted_strings(tail)
                .first()
                .and_then(|s| maven_coordinate(s))
            {
                out.push(c);
            } else if !tail.contains(['"', '\'']) && tail.contains('.') {
                *catalog += 1;
            }
        }
        "kotlin" if tail.starts_with('(') => {
            if let Some(m) = quoted_strings(tail).first()
                && is_maven_part(m)
            {
                out.push(format!("org.jetbrains.kotlin:kotlin-{m}"));
            }
        }
        "project" | "files" | "fileTree" | "gradleApi" | "localGroovy" | "gradleTestKit" => {}
        "group" => {
            let group = map_value(arg, "group");
            let name = map_value(arg, "name");
            if let (Some(g), Some(a)) = (group, name)
                && is_maven_part(&g)
                && is_maven_part(&a)
            {
                out.push(format!("{g}:{a}"));
            }
        }
        _ if !head.is_empty() && tail.starts_with('.') => *catalog += 1,
        _ => {}
    }
}

/// The quoted value of `key: '<v>'` (Groovy map) or `key = "<v>"` (Kotlin
/// named argument) in a Gradle call's argument text.
fn map_value(arg: &str, key: &str) -> Option<String> {
    idents(arg).into_iter().find_map(|(start, ident)| {
        if ident != key {
            return None;
        }
        let rest = arg[start + ident.len()..].trim_start();
        let rest = rest
            .strip_prefix(':')
            .or_else(|| rest.strip_prefix('='))?
            .trim_start();
        first_quoted(rest)
    })
}

/// Every complete `".."` / `'..'` string of `s`, in order (no escapes read).
fn quoted_strings(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(at) = rest.find(['"', '\'']) {
        let q = rest[at..].chars().next().unwrap_or('"');
        let body = &rest[at + 1..];
        let Some(end) = body.find(q) else {
            break;
        };
        out.push(body[..end].to_string());
        rest = &body[end + 1..];
    }
    out
}

/// The identifiers of `src` outside string literals, with their byte offsets:
/// a run of `[A-Za-z0-9_]` starting with a letter or `_`, not preceded by an
/// identifier character or a `.` (so `configurations.implementation` and
/// `libs.api` are no configuration call).
fn idents(src: &str) -> Vec<(usize, &str)> {
    let b = src.as_bytes();
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'"' || c == b'\'' {
            i += 1;
            while i < b.len() && b[i] != c {
                i += if b[i] == b'\\' { 2 } else { 1 };
            }
            i += 1;
            continue;
        }
        if (c.is_ascii_alphabetic() || c == b'_')
            && (i == 0 || !(word(b[i - 1]) || b[i - 1] == b'.'))
        {
            let start = i;
            while i < b.len() && word(b[i]) {
                i += 1;
            }
            out.push((start, &src[start..i]));
            continue;
        }
        i += 1;
    }
    out
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CommentStyle {
    /// `//` line and `/* .. */` block comments (Groovy, Kotlin).
    CLike,
    /// `#` line comments (YAML, Elixir).
    Hash,
}

/// `src` with its comments blanked to spaces (newlines kept, so offsets and
/// lines stay put), string literals left whole.
fn strip_comments(src: &str, style: CommentStyle) -> String {
    let b = src.as_bytes();
    let mut out = b.to_vec();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'"' || c == b'\'' {
            i += 1;
            while i < b.len() && b[i] != c && b[i] != b'\n' {
                i += if b[i] == b'\\' { 2 } else { 1 };
            }
            i += 1;
            continue;
        }
        let line_comment = match style {
            CommentStyle::CLike => c == b'/' && b.get(i + 1) == Some(&b'/'),
            CommentStyle::Hash => c == b'#',
        };
        if line_comment {
            while i < b.len() && b[i] != b'\n' {
                out[i] = b' ';
                i += 1;
            }
            continue;
        }
        if style == CommentStyle::CLike && c == b'/' && b.get(i + 1) == Some(&b'*') {
            while i < b.len() && !(b[i] == b'*' && b.get(i + 1) == Some(&b'/')) {
                if b[i] != b'\n' {
                    out[i] = b' ';
                }
                i += 1;
            }
            let end = (i + 2).min(b.len());
            out[i..end].fill(b' ');
            i += 2;
            continue;
        }
        i += 1;
    }
    // Only ASCII bytes were replaced (by ASCII spaces), so the text stays UTF-8.
    String::from_utf8(out).unwrap_or_else(|_| src.to_string())
}

// ----------------------------------------------------------------------------
// FD.31: pubspec.yaml — the keys of the top-level `dependencies:` and
// `dev_dependencies:` maps. An entry whose value holds `sdk:` (`flutter:
// sdk: flutter`, `flutter_test`) is the SDK, not a pub package;
// `dependency_overrides:` overrides versions and declares nothing.
// ----------------------------------------------------------------------------

fn scan_pubspec(source: &str) -> Vec<String> {
    let src = strip_comments(source, CommentStyle::Hash);
    let mut out = Vec::new();
    let mut in_section = false;
    let mut child_indent: Option<usize> = None;
    // The entry being read and whether its nested value named an `sdk:`.
    let mut entry: Option<(String, bool)> = None;
    let flush = |entry: &mut Option<(String, bool)>, out: &mut Vec<String>| {
        if let Some((name, false)) = entry.take() {
            out.push(name);
        }
    };
    for line in src.lines() {
        let t = line.trim();
        if t.is_empty() || t == "---" {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if indent == 0 {
            flush(&mut entry, &mut out);
            in_section = matches!(t, "dependencies:" | "dev_dependencies:");
            child_indent = None;
            continue;
        }
        if !in_section {
            continue;
        }
        let child = *child_indent.get_or_insert(indent);
        if indent == child {
            flush(&mut entry, &mut out);
            let Some((key, _)) = t.split_once(':') else {
                continue;
            };
            let key = key.trim().trim_matches(|c| c == '"' || c == '\'');
            if !key.is_empty()
                && key
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            {
                entry = Some((key.to_string(), false));
            }
        } else if indent > child
            && let Some((_, sdk)) = entry.as_mut()
            && t.starts_with("sdk:")
        {
            *sdk = true;
        }
    }
    flush(&mut entry, &mut out);
    out
}

// ----------------------------------------------------------------------------
// FD.31: mix.exs — the `{:name, ..}` tuples of the list the `deps` function
// returns (`defp deps do [..] end` / `defp deps, do: [..]`), else of an
// inline `deps: [..]` keyword in `project/0`. An option's own tuples and
// atoms (`only: [:dev, :test]`) are inside a dependency tuple, not the list.
// ----------------------------------------------------------------------------

fn scan_mix_exs(source: &str) -> Vec<String> {
    let src = strip_comments(source, CommentStyle::Hash);
    let ids = idents(&src);
    let fn_list = ids.windows(2).find_map(|w| {
        let [(_, kw), (at, name)] = w else {
            return None;
        };
        (matches!(*kw, "def" | "defp") && *name == "deps").then(|| at + name.len())
    });
    let list_start = fn_list
        .and_then(|from| src[from..].find('[').map(|o| from + o))
        .or_else(|| {
            ids.iter().find_map(|(at, name)| {
                let rest = src[at + name.len()..].strip_prefix(':')?;
                (*name == "deps" && rest.trim_start().starts_with('['))
                    .then(|| src.len() - rest.trim_start().len())
            })
        });
    let Some(open) = list_start else {
        return Vec::new();
    };
    let body_start = open + 1;
    let Some(close) = balanced_close(&src[body_start..], b'[', b']') else {
        return Vec::new();
    };
    let body = &src[body_start..body_start + close];
    let b = body.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'"' | b'\'' => {
                let q = b[i];
                i += 1;
                while i < b.len() && b[i] != q {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                i += 1;
            }
            b'{' => {
                let inner = &body[i + 1..];
                let atom = inner.trim_start().strip_prefix(':').map(|a| {
                    a.chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                        .collect::<String>()
                });
                if let Some(atom) = atom.filter(|a| !a.is_empty()) {
                    out.push(atom);
                }
                i += 1 + balanced_close(inner, b'{', b'}').unwrap_or(inner.len());
            }
            b'[' => {
                let inner = &body[i + 1..];
                i += 1 + balanced_close(inner, b'[', b']').unwrap_or(inner.len());
            }
            _ => i += 1,
        }
    }
    out
}

// ----------------------------------------------------------------------------
// FD.31: .NET project files — `<PackageReference Include="N" .. />` (the tag
// and attribute names ASCII case-insensitive, as MSBuild reads them; one
// Include may list several names split by `;`). An `Update=` item edits a
// reference declared elsewhere and an `$(Property)` name is no package.
// ----------------------------------------------------------------------------

fn scan_package_references(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    let lower = source.to_ascii_lowercase();
    let needle = "<packagereference";
    let mut from = 0;
    while let Some(rel) = lower[from..].find(needle) {
        let start = from + rel + needle.len();
        from = start;
        // The tag must end here (`<PackageReferences>` is another element).
        if !lower[start..].starts_with(|c: char| c.is_ascii_whitespace() || c == '/' || c == '>') {
            continue;
        }
        let end = lower[start..].find('>').map_or(lower.len(), |e| start + e);
        let tag_lower = &lower[start..end];
        let tag = &source[start..end];
        let Some(value) = xml_attr(tag_lower, tag, "include") else {
            continue;
        };
        for name in value.split(';').map(str::trim) {
            if !name.is_empty()
                && !name.contains("$(")
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
            {
                out.push(name.to_string());
            }
        }
    }
    out
}

/// The value of attribute `name` (given lowercase) in one tag's attribute
/// text, read from `tag` at the offsets its ASCII-lowercased copy matched.
fn xml_attr(tag_lower: &str, tag: &str, name: &str) -> Option<String> {
    let mut from = 0;
    while let Some(rel) = tag_lower[from..].find(name) {
        let at = from + rel;
        from = at + name.len();
        let before_ok = at == 0 || tag_lower.as_bytes()[at - 1].is_ascii_whitespace();
        let rest = tag[at + name.len()..].trim_start();
        if !before_ok {
            continue;
        }
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        return first_quoted(rest);
    }
    None
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
    fn a_vscode_manifest_declares_commands_and_settings() {
        // GH.9: `engines.vscode` makes package.json the host's manifest.
        let src = r#"{
  "name": "hello",
  "engines": { "vscode": "^1.74.0" },
  "contributes": {
    "commands": [
      { "command": "extension.helloWorld", "title": "Hello World" },
      { "command": "extension.helloWorld", "title": "Twice" }
    ],
    "configuration": [
      { "title": "A", "properties": { "hello.greeting": { "type": "string", "default": "Hi" } } },
      { "properties": { "hello.excited": { "type": "boolean" } } }
    ]
  },
  "devDependencies": { "@types/vscode": "^1.73.0" }
}"#;
        let repo = RepoId::from_canonical("test://host-manifest");
        let module = module_id(repo);
        let out = extract_for_path(src, "ext/package.json", module, repo);
        assert_eq!(
            out.marker.as_deref(),
            Some("[host-manifest] host=vscode commands=1 config=2 path=ext/package.json")
        );
        let cmd = NodeId::from_parts(
            GRAPH_TYPE,
            repo,
            node_kind::CLI_COMMAND,
            "host_cmd:vscode:extension.helloWorld",
        );
        let defines: Vec<&Edge> = out
            .edges
            .iter()
            .filter(|e| e.from == module && e.to == cmd && e.category == edge_category::DEFINES)
            .collect();
        assert_eq!(defines.len(), 1);
        let ev = Evidence::of(defines[0]).expect("evidence");
        assert_eq!(ev.rule.as_deref(), Some("host_manifest_command"));
        assert_eq!(ev.line, Some(5), "0-based row of the command id");
        for key in ["hello.greeting", "hello.excited"] {
            let id = NodeId::from_parts(
                GRAPH_TYPE,
                repo,
                node_kind::CONFIG_KEY,
                &format!("config:vscode:{key}"),
            );
            assert!(out.edges.iter().any(|e| e.from == module
                && e.to == id
                && e.category == edge_category::DEFINES_CONFIG));
        }
        // Only keys: no title, default or description is a node; the
        // dependency is still read.
        let qnames = pkg_qnames(&out);
        assert!(
            qnames
                .iter()
                .all(|q| !q.contains("Hello World") && !q.contains("Hi"))
        );
        assert!(qnames.contains(&"package:npm:@types/vscode".to_string()));
    }

    #[test]
    fn a_package_without_the_engine_is_no_manifest() {
        let repo = RepoId::from_canonical("test://host-manifest-none");
        let src = r#"{ "engines": { "node": ">=18" }, "contributes": { "commands": [ { "command": "x.y" } ] } }"#;
        let out = extract_for_path(src, "package.json", module_id(repo), repo);
        assert_eq!(out.marker, None);
        assert!(out.nodes.is_empty());
        let out = extract_for_path("not json {", "package.json", module_id(repo), repo);
        assert_eq!(out.marker, None);
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

    /// DH.10: a `*.gemspec` is a manifest path (walked, routed down the
    /// manifest branch), but declares no PACKAGE_DEP of its own.
    #[test]
    fn gemspec_is_a_manifest_path() {
        for path in [
            "mytool.gemspec",
            "server/mytool.gemspec",
            "a/b/my-tool.gemspec",
        ] {
            assert!(is_manifest_path(path), "{path}");
        }
        for path in [
            ".gemspec",
            "mytool.gemspec.bak",
            "gemspec",
            "mytool.gemspecs",
            "lib/gemspec.rb",
        ] {
            assert!(!is_manifest_path(path), "{path}");
        }
        let repo = RepoId(1);
        let src = "Gem::Specification.new do |s|\n  s.name = \"mytool\"\n  s.add_dependency \"thor\"\n  s.executables = [\"mytool\"]\nend\n";
        let out = extract_for_path(src, "server/mytool.gemspec", module_id(repo), repo);
        assert!(out.nodes.is_empty() && out.edges.is_empty());
    }

    fn names(out: &PackageNodes) -> Vec<String> {
        let mut v: Vec<String> = out.nav.qname_by_id.values().cloned().collect();
        v.sort();
        v
    }

    fn deps_of(src: &str, path: &str) -> PackageNodes {
        let repo = RepoId(1);
        extract_deps_beside(src, path, module_id(repo), repo)
    }

    /// FD.31: a project's own dependencies are `groupId:artifactId`; a
    /// managed version, a plugin's classpath, an exclusion and a
    /// `${property}` coordinate are none.
    #[test]
    fn pom_xml_dependencies() {
        let src = r#"<?xml version="1.0"?>
<project>
  <groupId>com.acme</groupId>
  <artifactId>shop</artifactId>
  <parent><groupId>org.parent</groupId><artifactId>parent</artifactId></parent>
  <!-- <dependency><groupId>x.commented</groupId><artifactId>out</artifactId></dependency> -->
  <dependencyManagement><dependencies>
    <dependency><groupId>org.managed</groupId><artifactId>bom</artifactId></dependency>
  </dependencies></dependencyManagement>
  <dependencies>
    <dependency>
      <groupId>org.slf4j</groupId>
      <artifactId>slf4j-api</artifactId>
      <version>${slf4j.version}</version>
      <exclusions><exclusion><groupId>x.excl</groupId><artifactId>gone</artifactId></exclusion></exclusions>
    </dependency>
    <dependency><groupId>${project.groupId}</groupId><artifactId>sibling</artifactId></dependency>
    <dependency><artifactId>no-group</artifactId></dependency>
  </dependencies>
  <profiles><profile><dependencies>
    <dependency><groupId>junit</groupId><artifactId>junit</artifactId><scope>test</scope></dependency>
  </dependencies></profile></profiles>
  <build><plugins><plugin>
    <artifactId>maven-surefire-plugin</artifactId>
    <dependencies><dependency><groupId>org.tool</groupId><artifactId>tool</artifactId></dependency></dependencies>
  </plugin></plugins></build>
</project>
"#;
        let out = deps_of(src, "svc/pom.xml");
        assert_eq!(
            names(&out),
            [
                "package:maven:junit:junit",
                "package:maven:org.slf4j:slf4j-api"
            ]
        );
        assert_eq!(
            out.marker.as_deref(),
            Some("[packages] eco=maven deps=2 path=svc/pom.xml")
        );
        // The manifest route reaches the same read.
        let repo = RepoId(1);
        let via = extract_for_path(src, "svc/pom.xml", module_id(repo), repo);
        assert_eq!(names(&via), names(&out));
        assert_eq!(via.edges.len(), 2);
        assert!(
            via.edges
                .iter()
                .all(|e| e.category == edge_category::DEPENDS_ON)
        );
    }

    /// FD.31: Kotlin DSL and Groovy configuration calls, platforms, the map
    /// form and `kotlin(..)`; a project, file or catalog reference and
    /// anything outside a `dependencies` block (or in `constraints`) is none.
    #[test]
    fn gradle_dependencies() {
        let kts = r#"plugins { id("org.jetbrains.kotlin.jvm") version "2.0.0" }
val implementation = "not:a-call"
dependencies {
    implementation("com.squareup.okhttp3:okhttp:4.12.0")
    api(platform("org.springframework.boot:spring-boot-dependencies:3.3.0"))
    testImplementation(kotlin("test"))
    debugImplementation(group = "com.example", name = "debug-kit", version = "1.0")
    implementation(project(":core"))
    implementation(libs.retrofit)
    implementation(files("libs/local.jar"))
    // implementation("commented:out:1")
    /* api("block:commented:1") */
    constraints { implementation("org.pinned:only-a-version:2.0") }
    compileOnly("org.projectlombok:lombok:${lombokVersion}")
}
"#;
        let out = deps_of(kts, "app/build.gradle.kts");
        assert_eq!(
            names(&out),
            [
                "package:maven:com.example:debug-kit",
                "package:maven:com.squareup.okhttp3:okhttp",
                "package:maven:org.jetbrains.kotlin:kotlin-test",
                "package:maven:org.projectlombok:lombok",
                "package:maven:org.springframework.boot:spring-boot-dependencies",
            ]
        );
        assert_eq!(
            out.marker.as_deref(),
            Some("[packages] eco=maven deps=5 catalog_refs=1 path=app/build.gradle.kts")
        );
        let groovy = r#"buildscript {
    dependencies { classpath 'com.android.tools.build:gradle:8.5.0' }
}
dependencies {
    implementation 'com.google.guava:guava:33.0-jre'
    testImplementation "junit:junit:4.13.2"
    runtimeOnly group: 'org.postgresql', name: 'postgresql', version: '42.7.3'
    implementation('io.reactivex:rxjava:1.3.8') { exclude group: 'x', module: 'y' }
    implementation libs.okhttp
}
"#;
        let out = deps_of(groovy, "build.gradle");
        assert_eq!(
            names(&out),
            [
                "package:maven:com.android.tools.build:gradle",
                "package:maven:com.google.guava:guava",
                "package:maven:io.reactivex:rxjava",
                "package:maven:junit:junit",
                "package:maven:org.postgresql:postgresql",
            ]
        );
        assert_eq!(
            out.marker.as_deref(),
            Some("[packages] eco=maven deps=5 catalog_refs=1 path=build.gradle")
        );
    }

    /// FD.31: pub packages of both maps; an `sdk:` entry is the SDK and
    /// `dependency_overrides` declares nothing.
    #[test]
    fn pubspec_dependencies() {
        let src = "name: app\nenvironment:\n  sdk: \">=3.0.0 <4.0.0\"\ndependencies:\n  http: ^1.2.0\n  flutter:\n    sdk: flutter\n  # commented: ^1.0.0\n  shared:\n    path: ../shared\n  riverpod:\n    hosted: https://pub.example.dev\n    version: ^2.0.0\ndev_dependencies:\n  flutter_test:\n    sdk: flutter\n  lints: ^4.0.0\ndependency_overrides:\n  meta: 1.9.0\nflutter:\n  uses-material-design: true\n";
        let out = deps_of(src, "app/pubspec.yaml");
        assert_eq!(
            names(&out),
            [
                "package:pub:http",
                "package:pub:lints",
                "package:pub:riverpod",
                "package:pub:shared",
            ]
        );
        assert_eq!(
            out.marker.as_deref(),
            Some("[packages] eco=pub deps=4 path=app/pubspec.yaml")
        );
    }

    /// FD.31: the deps function's tuples, in either `do` form, or an inline
    /// `deps:` list; an option's atoms are no dependency.
    #[test]
    fn mix_exs_dependencies() {
        let one_line = "defmodule Ex.MixProject do\n  use Mix.Project\n  def project, do: [app: :ex, deps: deps()]\n  defp deps, do: [{:jason, \"~> 1.4\"}]\nend\n";
        let out = deps_of(one_line, "ex/mix.exs");
        assert_eq!(names(&out), ["package:hex:jason"]);
        assert_eq!(
            out.marker.as_deref(),
            Some("[packages] eco=hex deps=1 path=ex/mix.exs")
        );
        let block = r#"defmodule Shop.MixProject do
  use Mix.Project
  def project do
    [app: :shop, version: "0.1.0", deps: deps()]
  end

  defp deps do
    [
      {:phoenix, "~> 1.7"},
      # {:commented, "~> 1.0"},
      {:ecto_sql, "~> 3.10", only: [:dev, :test], runtime: false},
      {:my_lib, git: "https://example.com/my_lib.git", tag: "v1"},
      {:shared, in_umbrella: true}
    ]
  end
end
"#;
        assert_eq!(
            names(&deps_of(block, "mix.exs")),
            [
                "package:hex:ecto_sql",
                "package:hex:my_lib",
                "package:hex:phoenix",
                "package:hex:shared",
            ]
        );
        let inline = "defmodule A.MixProject do\n  def project do\n    [app: :a, deps: [{:plug, \"~> 1.0\"}]]\n  end\nend\n";
        assert_eq!(names(&deps_of(inline, "mix.exs")), ["package:hex:plug"]);
        assert!(deps_of("defmodule B do\nend\n", "mix.exs").nodes.is_empty());
    }

    /// FD.31: `PackageReference Include` in any .NET project file; `Update`
    /// and a property name are none.
    #[test]
    fn dotnet_package_references() {
        let src = r#"<Project Sdk="Microsoft.NET.Sdk.Web">
  <ItemGroup>
    <PackageReference Include="Newtonsoft.Json" Version="13.0.3" />
    <packagereference include='Serilog.AspNetCore'>
      <Version>8.0.0</Version>
    </packagereference>
    <PackageReference Update="Updated.Elsewhere" Version="1.0" />
    <PackageReference Include="$(SharedPackage)" />
    <ProjectReference Include="..\Core\Core.csproj" />
  </ItemGroup>
</Project>
"#;
        let out = deps_of(src, "net/Api.csproj");
        assert_eq!(
            names(&out),
            [
                "package:nuget:Newtonsoft.Json",
                "package:nuget:Serilog.AspNetCore"
            ]
        );
        assert_eq!(
            out.marker.as_deref(),
            Some("[packages] eco=nuget deps=2 path=net/Api.csproj")
        );
        assert_eq!(names(&deps_of(src, "Lib.fsproj")).len(), 2);
    }

    /// FD.31: the walk reads the build files no parser reads through the
    /// manifest route; pubspec.yaml and mix.exs keep their own route.
    #[test]
    fn fd31_manifest_paths() {
        for path in [
            "pom.xml",
            "svc/pom.xml",
            "build.gradle",
            "app/build.gradle.kts",
            "net/Api.csproj",
            "Lib.FSPROJ",
            "x/Tool.vbproj",
        ] {
            assert!(is_manifest_path(path), "{path}");
        }
        for path in [
            "pubspec.yaml",
            "mix.exs",
            ".csproj",
            "settings.gradle",
            "pom.xml.bak",
            "Api.csproj.user",
        ] {
            assert!(!is_manifest_path(path), "{path}");
        }
        let out = deps_of("anything", "README.md");
        assert!(out.nodes.is_empty() && out.marker.is_none());
        // The five earlier manifests other than Cargo.toml / pyproject.toml
        // (FD.77) keep no marker.
        let repo = RepoId(1);
        let npm = extract_for_path(
            r#"{"dependencies":{"a":"1"}}"#,
            "package.json",
            module_id(repo),
            repo,
        );
        assert!(npm.marker.is_none());
    }

    fn manifest_deps(src: &str, path: &str) -> PackageNodes {
        let repo = RepoId(1);
        extract_for_path(src, path, module_id(repo), repo)
    }

    const ARRAY_TABLES_CARGO: &str = "[package]\nname = \"shop\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nserde = \"1\"\n\n[dev-dependencies]\ntempfile = \"3\"\n\n[[test]]\nname = \"smoke\"\npath = \"tests/smoke.rs\"\nharness = false\n\n[[bench]]\nname = \"speed\"\npath = \"benches/speed.rs\"\nharness = false\n\n[[bin]]\nname = \"shopd\"\npath = \"src/main.rs\"\nrequired-features = [\"cli\"]\n\n[[example]]\nname = \"demo\"\npath = \"examples/demo.rs\"\n\n[build-dependencies]\ncc = \"1\"\n";

    /// FD.77: the keys of a `[[test]]` / `[[bench]]` / `[[bin]]` /
    /// `[[example]]` table are no dependencies of the `[dev-dependencies]`
    /// table before them; a `[build-dependencies]` after them still is.
    #[test]
    fn cargo_array_tables_close_the_dependency_section() {
        let out = manifest_deps(ARRAY_TABLES_CARGO, "crate/Cargo.toml");
        assert_eq!(
            names(&out),
            [
                "package:cargo:cc",
                "package:cargo:serde",
                "package:cargo:tempfile"
            ]
        );
        assert_eq!(out.edges.len(), 3);
    }

    /// FD.77: activation/Cargo.toml's shape, `[dependencies]` then a
    /// `[[bench]]`, reads the one path dependency.
    #[test]
    fn cargo_bench_after_dependencies_reads_only_the_dependency() {
        let src = "[package]\nname = \"glia-activation\"\n\n[dependencies]\nglia-core = { path = \"../core\" }\n\n[[bench]]\nname = \"ppr\"\npath = \"benches/ppr.rs\"\nharness = false\n";
        let out = manifest_deps(src, "activation/Cargo.toml");
        assert_eq!(names(&out), ["package:cargo:glia-core"]);
    }

    /// FD.77: a `[[tool.poetry.source]]` after `[tool.poetry.dependencies]`
    /// leaks no `name` / `url` / `priority` dependency.
    #[test]
    fn poetry_source_array_table_is_no_dependency() {
        let src = "[tool.poetry]\nname = \"pyapp\"\nversion = \"0.1.0\"\n\n[tool.poetry.dependencies]\npython = \"^3.11\"\nrequests = \"^2.31\"\n\n[[tool.poetry.source]]\nname = \"internal\"\nurl = \"https://pypi.example.com/simple\"\npriority = \"supplemental\"\n";
        let out = manifest_deps(src, "pyapp/pyproject.toml");
        assert_eq!(names(&out), ["package:pypi:requests"]);
        assert_eq!(
            out.marker.as_deref(),
            Some("[packages] eco=pypi deps=1 array_tables=1 path=pyapp/pyproject.toml")
        );
    }

    /// FD.77: a PEP-621 `[project]` table followed by a `[[project.authors]]`
    /// style array keeps its dependencies.
    #[test]
    fn pep621_dependencies_survive_a_following_array_table() {
        let src = "[project]\nname = \"pkg\"\ndependencies = [\n  \"httpx>=0.27\",\n  \"click\",\n]\n\n[[project.authors]]\nname = \"Someone\"\nemail = \"someone@example.com\"\n\n[[project.maintainers]]\nname = \"Other\"\n";
        let out = manifest_deps(src, "pyproject.toml");
        assert_eq!(names(&out), ["package:pypi:click", "package:pypi:httpx"]);
        assert_eq!(
            out.marker.as_deref(),
            Some("[packages] eco=pypi deps=2 array_tables=2 path=pyproject.toml")
        );
    }

    /// FD.77: the Cargo.toml marker counts the deps and the array tables.
    #[test]
    fn cargo_marker_counts_array_tables() {
        let out = manifest_deps(ARRAY_TABLES_CARGO, "crate/Cargo.toml");
        assert_eq!(
            out.marker.as_deref(),
            Some("[packages] eco=cargo deps=3 array_tables=4 path=crate/Cargo.toml")
        );
    }

    /// FD.77: an array-table header opens a section of its own, marked as
    /// one; one with no `]]` opens an unnamed array section.
    #[test]
    fn toml_sections_mark_array_tables() {
        let src = "[dependencies]\na = \"1\"\n[[bin]]\nname = \"x\"\n[[broken\nb = 1\n[c]\nd = 1\n";
        let got: Vec<(String, bool)> = parse_toml_sections(src)
            .into_iter()
            .map(|s| (s.name, s.array))
            .collect();
        assert_eq!(
            got,
            [
                ("dependencies".to_string(), false),
                ("bin".to_string(), true),
                (String::new(), true),
                ("c".to_string(), false),
            ]
        );
    }

    /// GD.37: probe-f4p/toml1, the dotted, target and dotted-target tables.
    const DOTTED_CARGO: &str = "[package]\nname = \"probe\"\nversion = \"0.1.0\"\n\n[dependencies]\nanyhow = \"1\"\n\n[dependencies.serde]\nversion = \"1\"\nfeatures = [\"derive\"]\n\n[target.'cfg(unix)'.dependencies]\nlibc = \"0.2\"\n\n[target.'cfg(windows)'.dependencies.winapi]\nversion = \"0.3\"\n\n[dev-dependencies.tempfile]\nversion = \"3\"\n";

    /// GD.37: a dotted `[<kind>.<name>]` table is the dependency <name>, and
    /// a `[target.<spec>.<kind>]` table reads like `[<kind>]`; the keys of a
    /// dotted table (`version`, `features`) are no dependencies.
    #[test]
    fn cargo_dotted_and_target_dependency_tables() {
        let out = manifest_deps(DOTTED_CARGO, "Cargo.toml");
        assert_eq!(
            names(&out),
            [
                "package:cargo:anyhow",
                "package:cargo:libc",
                "package:cargo:serde",
                "package:cargo:tempfile",
                "package:cargo:winapi",
            ]
        );
        assert_eq!(
            out.marker.as_deref(),
            Some("[packages] eco=cargo deps=5 array_tables=0 path=Cargo.toml")
        );
    }

    /// GD.37: a target triple, a double-quoted cfg holding a `]` and a dot,
    /// build-dependencies in both forms, and a quoted dotted name.
    #[test]
    fn cargo_target_triples_and_quoted_cfgs() {
        let src = "[target.x86_64-pc-windows-msvc.dependencies]\nwindows-sys = \"0.52\"\n\n[target.\"cfg(any(target_os = \\\"a]b\\\", unix))\".build-dependencies]\npkg-config = \"0.3\"\n\n[target.'cfg(target_arch = \"wasm32\")'.dev-dependencies.wasm-bindgen-test]\nversion = \"0.3\"\n\n[build-dependencies.\"cc\"]\nversion = \"1\"\n\n[target.'cfg(unix)'.features]\nnot-a-dep = []\n\n[workspace.dependencies]\nshared = \"1\"\n";
        let out = manifest_deps(src, "Cargo.toml");
        assert_eq!(
            names(&out),
            [
                "package:cargo:cc",
                "package:cargo:pkg-config",
                "package:cargo:wasm-bindgen-test",
                "package:cargo:windows-sys",
            ]
        );
    }

    /// GD.37: a header's `]` is the first one outside quotes.
    #[test]
    fn toml_header_close_skips_quoted_brackets() {
        let src =
            "[target.'cfg(x = \"]\")'.dependencies]\na = \"1\"\n[[bin.'x]]y']]\nname = \"b\"\n";
        let got: Vec<(String, bool)> = parse_toml_sections(src)
            .into_iter()
            .map(|s| (s.name, s.array))
            .collect();
        assert_eq!(
            got,
            [
                ("target.'cfg(x = \"]\")'.dependencies".to_string(), false),
                ("bin.'x]]y'".to_string(), true),
            ]
        );
    }

    /// GD.37: probe-f4p/toml2, PEP 621 optional dependencies (a table of
    /// arrays), a Poetry group other than `dev` and a dotted Poetry
    /// dependency table.
    #[test]
    fn pyproject_optional_groups_and_dotted_poetry_tables() {
        let src = "[tool.poetry]\nname = \"probe\"\nversion = \"0.1.0\"\n\n[project]\nname = \"probe\"\ndependencies = [\"requests>=2\"]\n\n[project.optional-dependencies]\ndocs = [\"sphinx>=7\"]\n\n[tool.poetry.group.test.dependencies]\npytest-cov = \"^5\"\n\n[tool.poetry.dependencies.httpx]\nversion = \"^0.27\"\n";
        let out = manifest_deps(src, "pyproject.toml");
        assert_eq!(
            names(&out),
            [
                "package:pypi:httpx",
                "package:pypi:pytest-cov",
                "package:pypi:requests",
                "package:pypi:sphinx",
            ]
        );
        assert_eq!(
            out.marker.as_deref(),
            Some("[packages] eco=pypi deps=4 array_tables=0 path=pyproject.toml")
        );
    }

    /// GD.37: prefect's layout, `[project.optional-dependencies]` with
    /// multi-line arrays and extras, then PEP 735 `[dependency-groups]`
    /// whose `{include-group = ..}` items name no package.
    #[test]
    fn pyproject_dependency_groups_skip_include_group() {
        let src = "[project]\nname = \"app\"\ndependencies = [\n  \"anyio>=4\",\n]\n\n[project.optional-dependencies]\notel = [\n  \"opentelemetry-api>=1.27\", # traces\n  \"opentelemetry-sdk[grpc]>=1.27\",\n]\naws = [\"boto3\"]\n\n[dependency-groups]\ndev = [\n  \"ruff==0.6\",\n  { include-group = \"test\" },\n]\ntest = [\"pytest>=8\", 'pytest-xdist']\n\n[tool.poetry.group.lint.dependencies.mypy]\nversion = \"^1\"\n\n[tool.poetry.dev-dependencies.black]\nversion = \"^24\"\n";
        let out = manifest_deps(src, "pyproject.toml");
        assert_eq!(
            names(&out),
            [
                "package:pypi:anyio",
                "package:pypi:black",
                "package:pypi:boto3",
                "package:pypi:mypy",
                "package:pypi:opentelemetry-api",
                "package:pypi:opentelemetry-sdk",
                "package:pypi:pytest",
                "package:pypi:pytest-xdist",
                "package:pypi:ruff",
            ]
        );
    }

    /// GD.37: a dotted Poetry `python` table is the interpreter, no package.
    #[test]
    fn poetry_dotted_python_is_no_dependency() {
        let src = "[tool.poetry.dependencies.python]\nversion = \"^3.11\"\n";
        assert!(names(&manifest_deps(src, "pyproject.toml")).is_empty());
    }
}
