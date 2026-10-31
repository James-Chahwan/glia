//! IaC (Infrastructure-as-Code) extraction (v0.4.x — task #9).
//!
//! Emits `INFRA_RESOURCE` nodes for declared infra entities and
//! `INFRA_REFERENCES` edges for intra-graph links (Deployment → Image,
//! docker-compose service → image). `IacResolver` then pairs same-qname
//! resources across repos to surface "image built in repo A consumed by
//! k8s manifest in repo B" style cross-service joins.
//!
//! Single qname shape: `infra:<kind>:<name>`. Kinds:
//!   - `image`         — Dockerfile build target (the nearest enclosing
//!     directory that is not a generic one such as `src` / `docker`, GH.13)
//!     or compose/k8s image-ref (image name without tag)
//!   - `deployment`    — k8s Deployment / ReplicaSet (treated identically)
//!   - `statefulset`   — k8s StatefulSet
//!   - `daemonset`     — k8s DaemonSet
//!   - `job`           — k8s Job
//!   - `cronjob`       — k8s CronJob (also emits `CRON_JOB` from cron.rs)
//!   - `service`       — k8s Service or docker-compose service
//!   - `ingress`       — k8s Ingress
//!   - `configmap`     — k8s ConfigMap
//!   - `secret`        — k8s Secret
//!
//! Container starts (GH.15): every Dockerfile stage's ENTRYPOINT / CMD
//! (exec or shell form, inherited through `FROM <stage>`), WORKDIR, COPY map
//! and build-output RUN lines (`go build -o`, `cargo build`, `dotnet publish
//! -o`, gradle `installDist` / `bootJar` / `jar`, maven `package`) are one
//! CODE Json start cell per stage on the image node (`{"start":{"host":
//! "docker",..}}`, a `HEALTHCHECK CMD` its own cell with trigger
//! `healthcheck`), and a binary a start runs that the file neither builds
//! nor copies in is a CLI_INVOCATION `cli_invoke:<bin>` from the Dockerfile
//! MODULE. Binding a start to code is the host-start rules' job (GH.18);
//! this reader records what the file says.
//!
//! Orchestrator starts (GH.16): a compose service's `build:` is an
//! INFRA_REFERENCES service -> the image the Dockerfile reader names for the
//! declared context's Dockerfile, and its `command:` / `entrypoint:` /
//! `working_dir:` are a CODE start cell on the service (`host` `compose`); a
//! Kubernetes workload's pod-template containers and init containers with a
//! `command:` / `args:` / `workingDir:` are start cells on the workload
//! (`host` `k8s`, a CronJob's trigger `schedule`); skaffold
//! `build.artifacts[]` (and a compose service naming both `image:` and
//! `build:`) put a `build` cell on the declared image. These are read through
//! a small block-YAML reader (flow and block lists, anchors, `<<:` merges);
//! a Helm `{{ }}` value is refused and counted. The overrides are applied by
//! the host-start rules (GH.18), never here.
//!
//! Process managers (GH.17): a `Procfile` / `Procfile.<x>`, a systemd
//! `*.service` unit (gated on its `[Service]` section) and a supervisord
//! conf (`supervisord.conf` / `supervisor.conf`, or a `*.conf` under
//! `supervisor/` / `supervisord.d/`, gated on a `[program:<name>]` section)
//! each declare INFRA_RESOURCEs `infra:process:<type>@<dir>` (a Procfile's
//! types repeat per sub-project, so its directory is in the name),
//! `infra:unit:<stem>` and `infra:program:<name>`, each with a CODE start
//! cell (`host` procfile / systemd / supervisor, trigger process_type / boot /
//! supervised, the effective argv, the workdir, the declaring file's dir and
//! the decl). Binding a start to code is GH.18's.
//!
//! Out of scope (deferred to v0.5+):
//!   - Kustomize overlays (template merging is non-trivial)
//!   - Helm chart templating (a templated value is refused, GH.16)
//!   - Terraform / Pulumi / CDK / serverless.yml (Terraform has a tree-sitter
//!     parser already in parsers/code/terraform/ — IaC node emission belongs
//!     there in a follow-up, not here)
//!   - Selector → Deployment matching (Service.spec.selector → matching
//!     Deployment.metadata.labels — needs label-set comparison)
//!   - Ingress → Service routing (host-path-rules)

use glia_code_domain::{CodeNav, GRAPH_TYPE, edge_category, infra, node_kind};
use glia_core::{Confidence, Edge, Node, NodeId, RepoId};

pub struct IacNodes {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub nav: CodeNav,
    /// Per-file marker lines (GH.15's `[docker-start]`), printed by the
    /// engine's route fold so they keep walk order and replay with the
    /// non-code memo.
    pub markers: Vec<String>,
}

/// Extract from a Dockerfile. Emits one `infra:image:<name>` node, named by
/// `dockerfile_image_name`, and one `INFRA_REFERENCES` edge from it to each
/// external base image a stage builds `FROM` (every stage, the final one
/// included, in file order). A `FROM` naming an earlier stage, `scratch`, or a
/// `$VAR` no global `ARG` default fills mints nothing; `--platform=..` and other
/// flags are skipped (GH.13).
///
/// fired_on: `[iac-image] dockerfiles=1 stages=<s> flags_skipped=<f>
/// named_by=(compose=0 skaffold=0 dir=<r> walked_up=<w>) file=<p>` per
/// Dockerfile parsed fresh. `compose` / `skaffold` count a name taken from a
/// declared build context, which this per-file extractor cannot see (the
/// declaration lives in another file): they stay 0 here.
pub fn extract_dockerfile(source: &str, path: &str, module_id: NodeId, repo: RepoId) -> IacNodes {
    let mut out = IacNodes {
        nodes: Vec::new(),
        edges: Vec::new(),
        nav: CodeNav::default(),
        markers: Vec::new(),
    };
    let stages = dockerfile_stages(source);
    let naming = dockerfile_image_name(path);
    let image_name = naming.name.as_str();
    emit_resource(&mut out, "image", image_name, module_id, repo);
    let from_id = NodeId::from_parts(
        GRAPH_TYPE,
        repo,
        node_kind::INFRA_RESOURCE,
        &infra::qname("image", image_name),
    );
    // Link this image -> each base it builds from. Useful for "what depends
    // on python:3.11-slim" queries.
    let mut linked: Vec<String> = Vec::new();
    for stage in &stages.stages {
        let StageBase::External(base) = &stage.base else {
            continue;
        };
        let base_name = image_basename(base);
        if base_name.is_empty() || base_name == image_name || linked.contains(&base_name) {
            continue;
        }
        emit_resource(&mut out, "image", &base_name, module_id, repo);
        let to_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo,
            node_kind::INFRA_RESOURCE,
            &infra::qname("image", &base_name),
        );
        out.edges.push(Edge {
            from: from_id,
            to: to_id,
            category: edge_category::INFRA_REFERENCES,
            confidence: Confidence::Medium,
            cells: Vec::new(),
        });
        linked.push(base_name);
    }
    let (dir, walked_up) = match naming.source {
        ImageNameSource::Dir => (1, 0),
        ImageNameSource::WalkedUp => (0, 1),
        ImageNameSource::FileName | ImageNameSource::Degenerate => (0, 0),
    };
    eprintln!(
        "[iac-image] dockerfiles=1 stages={} flags_skipped={} named_by=(compose=0 skaffold=0 dir={dir} walked_up={walked_up}) file={path}",
        stages.stages.len(),
        stages.flags_skipped,
    );
    emit_starts(&mut out, source, path, &stages, from_id, module_id, repo);
    out
}

/// GH.15: the image's start records as CODE cells on `image_id`, a
/// CLI_INVOCATION `cli_invoke:<bin>` from the Dockerfile MODULE for each
/// binary a container start runs that the file neither builds nor copies
/// in, and the `[docker-start]` marker.
fn emit_starts(
    out: &mut IacNodes,
    source: &str,
    path: &str,
    stages: &DockerStages,
    image_id: NodeId,
    module_id: NodeId,
    repo: RepoId,
) {
    let starts = dockerfile_starts(source, stages);
    if starts.stages == 0 {
        return;
    }
    let context = dockerfile_context(path);
    let cells: Vec<glia_core::Cell> = starts
        .records
        .iter()
        .map(|r| start_cell(r, &context, path))
        .collect();
    if let Some(node) = out.nodes.iter_mut().find(|n| n.id == image_id) {
        node.cells.extend(cells);
    }
    let (mut exec, mut shell, mut healthchecks, mut binaries) = (0usize, 0usize, 0usize, 0usize);
    let mut refused: std::collections::BTreeMap<&'static str, usize> =
        std::collections::BTreeMap::new();
    let mut by_bin: std::collections::BTreeMap<String, Vec<Vec<String>>> =
        std::collections::BTreeMap::new();
    for rec in &starts.records {
        if rec.trigger == StartTrigger::Healthcheck {
            healthchecks += 1;
            continue;
        }
        if rec.exec {
            exec += 1;
        } else {
            shell += 1;
        }
        let target = glia_code_domain::hosts::normalise_argv(&rec.argv);
        if let Some(reason) = target.refusal() {
            *refused.entry(reason).or_default() += 1;
            continue;
        }
        if let Some((bin, args)) = binary_invocation(rec, &target) {
            binaries += 1;
            let argv = by_bin.entry(bin).or_default();
            if !args.is_empty() && !argv.contains(&args) {
                argv.push(args);
            }
        }
    }
    for (bin, argv) in &by_bin {
        let qname = format!("cli_invoke:{bin}");
        let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::CLI_INVOCATION, &qname);
        out.nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Medium,
            cells: vec![crate::cli::argv_cell(bin, argv)],
        });
        out.nav
            .record(id, bin, &qname, node_kind::CLI_INVOCATION, Some(module_id));
        out.edges.push(Edge {
            from: module_id,
            to: id,
            category: edge_category::DEFINES,
            confidence: Confidence::Medium,
            cells: Vec::new(),
        });
    }
    let tools: Vec<String> = BUILD_TOOLS
        .iter()
        .zip(starts.builds)
        .filter(|(_, n)| *n > 0)
        .map(|(t, n)| format!("{t}={n}"))
        .collect();
    let mut line = format!(
        "[docker-start] file={path} stages={} starts={} (exec={exec} shell={shell}) healthchecks={healthchecks} copies={} builds={} ({}) binaries={binaries}",
        starts.stages,
        exec + shell,
        starts.copies,
        starts.builds.iter().sum::<usize>(),
        tools.join(" "),
    );
    if starts.unmapped > 0 {
        line.push_str(&format!(" unmapped={}", starts.unmapped));
    }
    if !refused.is_empty() {
        let reasons: Vec<String> = refused.iter().map(|(r, n)| format!("{r}:{n}")).collect();
        line.push_str(&format!(" refused={}", reasons.join(",")));
    }
    out.markers.push(line);
}

/// Extract from a YAML manifest at the repo-relative `path`. Detects k8s
/// `kind:` declarations and docker-compose `services:` entries. Emits one
/// INFRA_RESOURCE per object, linking containers' `image:` refs to their host
/// workload.
///
/// GH.16 adds the orchestrator starts: a compose service's `build:` (a
/// context string, or a map of `context` / `dockerfile` / `target`) is an
/// INFRA_REFERENCES service -> `infra:image:<name>`, the name the Dockerfile
/// reader gives that context's Dockerfile, and its `command:` /
/// `entrypoint:` (a string is split with shell-word rules, as compose does;
/// a list is taken as written) and `working_dir:` with the build are one
/// CODE start cell on the service (`{"start":{"host":"compose",..}}`); each
/// container and init container of a Deployment / StatefulSet / DaemonSet /
/// Job / CronJob / ReplicaSet pod template with a `command:` / `args:` /
/// `workingDir:` is one start cell on the workload (`host` `k8s`, trigger
/// `container_start`, or `schedule` with the CronJob's schedule); a skaffold
/// `build.artifacts[]` entry (and a compose service naming both `image:` and
/// `build:`) puts a `{"build":{..}}` cell on the declared image naming its
/// context and Dockerfile. Overrides are not applied here: the cells say
/// what the file says (`null` = unset, `[]` = cleared). A Helm `{{ }}` value
/// is refused and counted, never read.
///
/// fired_on (per file, in [`IacNodes::markers`]):
/// `[compose-start] file=<p> services=<s> builds=<b> commands=<c>
/// entrypoints=<e> refused=<r>`, `[k8s-start] file=<p> containers=<c>
/// commands=<m> args=<a> refused=<r>` and `[skaffold-build] file=<p>
/// artifacts=<a> dockerfiles=<d> refused=<r>`.
pub fn extract_yaml(source: &str, path: &str, module_id: NodeId, repo: RepoId) -> IacNodes {
    let mut out = IacNodes {
        nodes: Vec::new(),
        edges: Vec::new(),
        nav: CodeNav::default(),
        markers: Vec::new(),
    };
    extract_k8s_documents(source, module_id, repo, &mut out);
    extract_compose_services(source, module_id, repo, &mut out);
    extract_orchestrator_starts(source, path, module_id, repo, &mut out);
    out
}

// ----------------------------------------------------------------------------
// Helpers
// ----------------------------------------------------------------------------

fn emit_resource(out: &mut IacNodes, kind: &str, name: &str, module_id: NodeId, repo: RepoId) {
    let qname = infra::qname(kind, name);
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::INFRA_RESOURCE, &qname);
    // Dedupe within this extraction call.
    if out.nodes.iter().any(|n| n.id == id) {
        return;
    }
    out.nodes.push(Node {
        id,
        repo,
        confidence: Confidence::Medium,
        cells: vec![],
    });
    out.nav
        .record(id, name, &qname, node_kind::INFRA_RESOURCE, Some(module_id));
    // Module → resource edge for "what does this file declare" queries.
    out.edges.push(Edge {
        from: module_id,
        to: id,
        category: edge_category::DEFINES,
        confidence: Confidence::Medium,
        cells: Vec::new(),
    });
}

/// Image basename: `ghcr.io/foo/bar:1.2` → `bar`. Drops registry, namespace,
/// and tag.
fn image_basename(image: &str) -> String {
    let no_tag = image.split(':').next().unwrap_or(image);
    no_tag.rsplit('/').next().unwrap_or(no_tag).to_string()
}

/// Directory names that say where a Dockerfile sits, not what it builds:
/// `src/cartservice/src/Dockerfile` builds `cartservice`, not `src`. Compared
/// ignoring ASCII case.
const GENERIC_IMAGE_DIRS: &[&str] = &[
    "src",
    "app",
    "docker",
    "build",
    "deploy",
    ".docker",
    "dockerfiles",
];

/// What a Dockerfile's image is named after.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImageNameSource {
    /// Its own parent directory.
    Dir,
    /// An ancestor directory, past one or more [`GENERIC_IMAGE_DIRS`].
    WalkedUp,
    /// `Dockerfile.<name>` / `<name>.dockerfile`, every directory generic.
    FileName,
    /// Nothing names it: `image` (a Dockerfile at the root, or under
    /// generic directories only).
    Degenerate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ImageName {
    name: String,
    source: ImageNameSource,
}

/// Dockerfile image name: the nearest enclosing directory that is not one of
/// [`GENERIC_IMAGE_DIRS`], so one image per Dockerfile is named by the
/// directory around it. With no such directory, the file name minus its
/// `Dockerfile.` prefix / `.dockerfile` suffix, else `image`.
fn dockerfile_image_name(path: &str) -> ImageName {
    let norm = path.replace('\\', "/");
    let segments: Vec<&str> = norm
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect();
    let file = segments.last().copied().unwrap_or("Dockerfile");
    let dirs = &segments[..segments.len().saturating_sub(1)];
    for (depth, dir) in dirs.iter().rev().enumerate() {
        if GENERIC_IMAGE_DIRS
            .iter()
            .any(|g| g.eq_ignore_ascii_case(dir))
        {
            continue;
        }
        let source = if depth == 0 {
            ImageNameSource::Dir
        } else {
            ImageNameSource::WalkedUp
        };
        return ImageName {
            name: (*dir).to_string(),
            source,
        };
    }
    let lower = file.to_ascii_lowercase();
    let from_file = lower
        .strip_suffix(".dockerfile")
        .or_else(|| lower.strip_prefix("dockerfile."))
        .filter(|s| !s.is_empty());
    match from_file {
        Some(stripped) => ImageName {
            name: stripped.to_string(),
            source: ImageNameSource::FileName,
        },
        None => ImageName {
            name: "image".to_string(),
            source: ImageNameSource::Degenerate,
        },
    }
}

/// What one `FROM` builds on.
#[derive(Debug, Clone, PartialEq, Eq)]
enum StageBase {
    /// An image reference, global `ARG` defaults substituted.
    External(String),
    /// An earlier stage of this file, by index.
    Stage(usize),
    /// `FROM scratch`: no base.
    Scratch,
    /// A `$VAR` no global `ARG` default fills: unreadable from the file.
    Unresolved,
}

/// One `FROM` instruction: a build stage.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DockerStage {
    /// `AS <name>`, as written.
    name: Option<String>,
    base: StageBase,
    /// 0-based row of the `FROM` keyword.
    line: usize,
}

/// Every stage of a Dockerfile, in file order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct DockerStages {
    stages: Vec<DockerStage>,
    /// `--flag[=value]` tokens skipped on `FROM` lines.
    flags_skipped: usize,
}

/// Read every `FROM`: flags skipped, `AS <stage>` recorded, a base naming an
/// earlier stage (case-insensitive, as Docker compares them) kept as that
/// stage, `$NAME` / `${NAME}` / `${NAME:-default}` filled from the `ARG`s
/// declared before the first `FROM` (the only ones a `FROM` sees).
fn dockerfile_stages(source: &str) -> DockerStages {
    let mut out = DockerStages::default();
    let mut global_args: Vec<(String, Option<String>)> = Vec::new();
    for (line, text) in dockerfile_instructions(source) {
        let mut tokens = text.split_whitespace();
        let Some(keyword) = tokens.next() else {
            continue;
        };
        if keyword.eq_ignore_ascii_case("ARG") {
            if out.stages.is_empty() {
                for decl in tokens {
                    let (name, value) = match decl.split_once('=') {
                        Some((n, v)) => (n, Some(unquote(v).to_string())),
                        None => (decl, None),
                    };
                    global_args.retain(|(n, _)| n != name);
                    global_args.push((name.to_string(), value));
                }
            }
            continue;
        }
        if !keyword.eq_ignore_ascii_case("FROM") {
            continue;
        }
        let mut image: Option<&str> = None;
        let mut name: Option<String> = None;
        let mut expect_name = false;
        for tok in tokens {
            if image.is_none() {
                if tok.starts_with("--") {
                    out.flags_skipped += 1;
                    continue;
                }
                image = Some(tok);
                continue;
            }
            if expect_name {
                name = Some(tok.to_string());
                break;
            }
            if tok.eq_ignore_ascii_case("AS") {
                expect_name = true;
                continue;
            }
            break;
        }
        let Some(image) = image else {
            continue;
        };
        let base = match substitute_args(image, &global_args) {
            None => StageBase::Unresolved,
            Some(img) if img.eq_ignore_ascii_case("scratch") => StageBase::Scratch,
            Some(img) => match out.stages.iter().position(|s| {
                s.name
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(&img))
            }) {
                Some(idx) => StageBase::Stage(idx),
                None => StageBase::External(img),
            },
        };
        out.stages.push(DockerStage { name, base, line });
    }
    out
}

fn unquote(v: &str) -> &str {
    v.trim_matches(|c| c == '"' || c == '\'')
}

/// `text` with `$NAME`, `${NAME}` and `${NAME:-default}` replaced from
/// `args`; `None` when a reference has no value to read.
fn substitute_args(text: &str, args: &[(String, Option<String>)]) -> Option<String> {
    let lookup = |name: &str| -> Option<String> {
        args.iter()
            .rev()
            .find(|(n, _)| n == name)
            .and_then(|(_, v)| v.clone())
            .filter(|v| !v.is_empty())
    };
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        if let Some(body) = after.strip_prefix('{') {
            let close = body.find('}')?;
            let inner = &body[..close];
            let value = match inner.split_once(":-") {
                Some((name, default)) => {
                    lookup(name).or_else(|| (!default.is_empty()).then(|| default.to_string()))
                }
                None => lookup(inner),
            };
            out.push_str(&value?);
            rest = &body[close + 1..];
        } else {
            let end = after
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(after.len());
            if end == 0 {
                return None;
            }
            out.push_str(&lookup(&after[..end])?);
            rest = &after[end..];
        }
    }
    out.push_str(rest);
    (!out.is_empty()).then_some(out)
}

/// The Dockerfile's logical instructions as `(0-based first row, text)`:
/// comment lines dropped, escape-continued lines joined (the `# escape=`
/// parser directive honoured), and the bodies of BuildKit heredocs
/// (`RUN <<EOF` .. `EOF`) skipped, so a line inside one is never read as an
/// instruction.
fn dockerfile_instructions(source: &str) -> Vec<(usize, String)> {
    let escape = dockerfile_escape(source);
    let mut out: Vec<(usize, String)> = Vec::new();
    let mut buf = String::new();
    let mut start = 0usize;
    let mut heredocs: Vec<(String, bool)> = Vec::new();
    for (row, raw) in source.lines().enumerate() {
        if let Some((term, strip_tabs)) = heredocs.first() {
            let candidate = if *strip_tabs {
                raw.trim_start_matches('\t')
            } else {
                raw
            };
            if candidate.trim_end() == term {
                heredocs.remove(0);
            }
            continue;
        }
        let trimmed = raw.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        if buf.is_empty() {
            if trimmed.is_empty() {
                continue;
            }
            start = row;
        } else if trimmed.is_empty() {
            continue;
        }
        match trimmed.strip_suffix(escape) {
            Some(cont) => {
                buf.push_str(cont);
                buf.push(' ');
            }
            None => {
                buf.push_str(trimmed);
                heredocs = heredoc_terminators(&buf);
                out.push((start, std::mem::take(&mut buf)));
            }
        }
    }
    if !buf.trim().is_empty() {
        out.push((start, buf));
    }
    out
}

/// The `# escape=<c>` parser directive (`\` or `` ` ``), read from the
/// directive block at the top of the file; `\` without one.
fn dockerfile_escape(source: &str) -> char {
    for line in source.lines() {
        let t = line.trim();
        let Some(body) = t.strip_prefix('#') else {
            break;
        };
        let Some((key, value)) = body.split_once('=') else {
            break;
        };
        let key = key.trim();
        if key.eq_ignore_ascii_case("escape") {
            if value.trim() == "`" {
                return '`';
            }
            break;
        }
        if key.is_empty() || key.contains(char::is_whitespace) {
            break;
        }
    }
    '\\'
}

/// The heredoc terminators an instruction opens, in order, each with whether
/// its body strips leading tabs (`<<-`). A word must start with a letter or
/// `_`, so a shell shift (`$((1<<2))`) opens nothing.
fn heredoc_terminators(instr: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let mut rest = instr;
    while let Some(at) = rest.find("<<") {
        let mut after = &rest[at + 2..];
        let strip_tabs = after.starts_with('-');
        if strip_tabs {
            after = &after[1..];
        }
        let quote = after.chars().next().filter(|c| *c == '"' || *c == '\'');
        if let Some(q) = quote {
            after = &after[q.len_utf8()..];
        }
        let end = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(after.len());
        let word = &after[..end];
        if word
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        {
            out.push((word.to_string(), strip_tabs));
        }
        rest = &after[end..];
    }
    out
}

// ----------------------------------------------------------------------------
// Container starts (GH.15): ENTRYPOINT / CMD, WORKDIR, the COPY map and the
// build-output RUN lines, one start record per stage.
// ----------------------------------------------------------------------------

/// The directories an exec-form `argv[0]` without a `/` is looked up in
/// (the default `PATH` of the common base images).
const PATH_DIRS: &[&str] = &[
    "/usr/local/sbin",
    "/usr/local/bin",
    "/usr/sbin",
    "/usr/bin",
    "/sbin",
    "/bin",
];

/// Extensions that make a started file a script the repo holds, not a
/// binary on `PATH`: such a start mints no CLI_INVOCATION.
const SCRIPT_EXTS: &[&str] = &[
    "sh", "bash", "py", "js", "mjs", "cjs", "ts", "rb", "pl", "php", "lua",
];

/// One ENTRYPOINT / CMD / HEALTHCHECK command, as Docker runs it: the exec
/// (JSON) form as written, the shell form as the stage's `SHELL` (default
/// `["/bin/sh", "-c"]`) plus the string.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ArgvInstr {
    argv: Vec<String>,
    exec: bool,
    /// 0-based row of the instruction.
    row: usize,
}

/// The build tools a RUN line is read for, in marker order.
const BUILD_TOOLS: [&str; 5] = ["go", "cargo", "dotnet", "gradle", "maven"];

/// One build-output RUN line: what it writes where. Field order is the
/// serialised order, so a start cell is byte-stable.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
struct BuildLine {
    /// One of [`BUILD_TOOLS`].
    tool: &'static str,
    /// The container path written (a file for `go build -o` / `cargo build
    /// --bin`, else the output directory). After `COPY --from=<stage>` it is
    /// the path the copy put it at.
    out: String,
    /// `go build`'s package argument (`.` when none).
    #[serde(skip_serializing_if = "Option::is_none")]
    pkg: Option<String>,
    /// `cargo build --bin <n>` (or `-p <package>`, the default bin's name).
    #[serde(skip_serializing_if = "Option::is_none")]
    bin: Option<String>,
    /// `dotnet publish`'s project argument.
    #[serde(skip_serializing_if = "Option::is_none")]
    proj: Option<String>,
    /// The gradle task (`:app:installDist`) or maven goal (`package`).
    #[serde(skip_serializing_if = "Option::is_none")]
    task: Option<String>,
    /// The RUN line's working directory in the container.
    workdir: String,
    /// That directory as a build-context path, through the stage's COPY map
    /// (`null` when nothing copied it there).
    src: Option<String>,
    /// The stage the RUN line is in.
    stage: String,
    /// Whether `out` is the binary itself (not a directory holding it).
    #[serde(skip)]
    file: bool,
}

/// One stage's state at its end: what a start reads.
#[derive(Debug, Clone, Default)]
struct StageState {
    /// `AS <name>`, else the stage's 0-based index.
    name: String,
    entrypoint: Option<ArgvInstr>,
    cmd: Option<ArgvInstr>,
    /// Set in this stage (not inherited through `FROM <stage>`): an own
    /// ENTRYPOINT resets an inherited CMD, as Docker does.
    entrypoint_here: bool,
    cmd_here: bool,
    healthcheck: Option<ArgvInstr>,
    /// `None` when unknown (an external base's WORKDIR, a `$ARG` one).
    workdir: Option<String>,
    /// `ENV` values (`Some`) and `ARG` names (`None`: a build may override
    /// one, so it is never read), in declaration order.
    vars: Vec<(String, Option<String>)>,
    /// The `SHELL` a shell-form command runs under.
    shell: Vec<String>,
    /// Container path -> build-context path, in instruction order.
    copies: Vec<(String, String)>,
    builds: Vec<BuildLine>,
}

fn default_shell() -> Vec<String> {
    vec!["/bin/sh".to_string(), "-c".to_string()]
}

/// What a start record is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartTrigger {
    ContainerStart,
    Healthcheck,
}

impl StartTrigger {
    fn as_str(self) -> &'static str {
        match self {
            StartTrigger::ContainerStart => "container_start",
            StartTrigger::Healthcheck => "healthcheck",
        }
    }
}

/// One start a stage declares.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StartRecord {
    trigger: StartTrigger,
    stage: String,
    is_final: bool,
    argv: Vec<String>,
    entrypoint: Vec<String>,
    cmd: Vec<String>,
    exec: bool,
    workdir: Option<String>,
    copies: Vec<(String, String)>,
    builds: Vec<BuildLine>,
    /// 0-based row of the instruction that leads the argv.
    row: usize,
}

/// A Dockerfile's starts and the marker's counts.
#[derive(Debug, Default)]
struct DockerStarts {
    stages: usize,
    records: Vec<StartRecord>,
    /// COPY / ADD instructions that mapped a build-context path.
    copies: usize,
    /// Build lines read, by [`BUILD_TOOLS`] index.
    builds: [usize; 5],
    /// COPY / ADD sources or destinations no file text names (an `ARG`, a
    /// glob, an unknown WORKDIR): counted, never guessed.
    unmapped: usize,
}

/// The start cell's JSON shape. Field order is the serialised order.
#[derive(serde::Serialize)]
struct StartCell<'a> {
    start: StartBody<'a>,
}

#[derive(serde::Serialize)]
struct StartBody<'a> {
    host: &'static str,
    stage: &'a str,
    #[serde(rename = "final")]
    is_final: bool,
    trigger: &'static str,
    argv: &'a [String],
    entrypoint: &'a [String],
    cmd: &'a [String],
    workdir: Option<&'a str>,
    context: &'a str,
    copies: &'a [(String, String)],
    builds: &'a [BuildLine],
    decl: String,
}

/// The CODE Json cell of one start: `{"start":{"host":"docker","stage":..,
/// "final":..,"trigger":"container_start"|"healthcheck","argv":[..],
/// "entrypoint":[..],"cmd":[..],"workdir":..,"context":"<repo rel>",
/// "copies":[[<container>,<context>],..],"builds":[..],"decl":"<file>:<row>"}}`.
fn start_cell(rec: &StartRecord, context: &str, path: &str) -> glia_core::Cell {
    let body = StartCell {
        start: StartBody {
            host: "docker",
            stage: &rec.stage,
            is_final: rec.is_final,
            trigger: rec.trigger.as_str(),
            argv: &rec.argv,
            entrypoint: &rec.entrypoint,
            cmd: &rec.cmd,
            workdir: rec.workdir.as_deref(),
            context,
            copies: &rec.copies,
            builds: &rec.builds,
            decl: format!("{path}:{}", rec.row + 1),
        },
    };
    glia_core::Cell {
        kind: glia_code_domain::cell_type::CODE,
        payload: glia_core::CellPayload::Json(serde_json::to_string(&body).unwrap_or_default()),
    }
}

/// The Dockerfile's directory, repo-relative (`.` at the root): the build
/// context a plain `docker build <dir>` uses.
fn dockerfile_context(path: &str) -> String {
    let norm = path.replace('\\', "/");
    match norm.rsplit_once('/') {
        Some((dir, _)) => normalise_rel(dir).unwrap_or_else(|| ".".to_string()),
        None => ".".to_string(),
    }
}

/// Read every stage's ENTRYPOINT / CMD / HEALTHCHECK / WORKDIR / ENV / ARG /
/// SHELL / COPY / ADD / RUN, inheriting through `FROM <stage>`, and return
/// one container-start record per stage whose effective argv is non-empty
/// (ENTRYPOINT ++ CMD; a shell-form ENTRYPOINT ignores CMD) and one
/// healthcheck record per stage holding a `HEALTHCHECK CMD`. ONBUILD and
/// heredoc bodies are skipped.
fn dockerfile_starts(source: &str, stages: &DockerStages) -> DockerStarts {
    let mut out = DockerStarts::default();
    let mut finals: Vec<StageState> = Vec::new();
    let mut cur: Option<StageState> = None;
    for (row, text) in dockerfile_instructions(source) {
        let trimmed = text.trim();
        let (keyword, rest) = match trimmed.split_once(char::is_whitespace) {
            Some((k, r)) => (k, r.trim()),
            None => (trimmed, ""),
        };
        let keyword = keyword.to_ascii_uppercase();
        if keyword == "FROM" {
            let Some(stage) = stages.stages.iter().find(|s| s.line == row) else {
                continue;
            };
            if let Some(done) = cur.take() {
                finals.push(done);
            }
            let index = finals.len();
            let name = stage.name.clone().unwrap_or_else(|| index.to_string());
            let mut next = match &stage.base {
                StageBase::Stage(parent) => finals.get(*parent).cloned().unwrap_or_default(),
                StageBase::Scratch => StageState {
                    workdir: Some("/".to_string()),
                    ..StageState::default()
                },
                StageBase::External(_) | StageBase::Unresolved => StageState::default(),
            };
            if next.shell.is_empty() {
                next.shell = default_shell();
            }
            next.name = name;
            next.entrypoint_here = false;
            next.cmd_here = false;
            cur = Some(next);
            continue;
        }
        let Some(state) = cur.as_mut() else {
            continue;
        };
        match keyword.as_str() {
            "ENTRYPOINT" => {
                state.entrypoint = argv_instr(rest, &state.shell, row);
                state.entrypoint_here = true;
            }
            "CMD" => {
                state.cmd = argv_instr(rest, &state.shell, row);
                state.cmd_here = true;
            }
            "HEALTHCHECK" => {
                state.healthcheck = read_healthcheck(rest, &state.shell, row);
            }
            "SHELL" => {
                if let Ok(shell) = serde_json::from_str::<Vec<String>>(rest)
                    && !shell.is_empty()
                {
                    state.shell = shell;
                }
            }
            "WORKDIR" => {
                let next = match substitute_stage(unquote(rest), &state.vars) {
                    Some(dir) if dir.starts_with('/') => Some(normalise_abs(&dir)),
                    Some(dir) => state.workdir.as_deref().map(|base| join_abs(base, &dir)),
                    None => None,
                };
                state.workdir = next;
            }
            "ENV" => read_env(rest, &mut state.vars),
            "ARG" => {
                for decl in rest.split_whitespace() {
                    let name = decl.split_once('=').map_or(decl, |(n, _)| n);
                    state.vars.push((name.to_string(), None));
                }
            }
            "COPY" | "ADD" => {
                read_copy(rest, keyword == "ADD", state, &finals, &mut out);
            }
            "RUN" => read_run(rest, state, &mut out),
            _ => {}
        }
    }
    if let Some(done) = cur.take() {
        finals.push(done);
    }
    out.stages = finals.len();
    let last = finals.len().saturating_sub(1);
    for (i, st) in finals.iter().enumerate() {
        let is_final = i == last;
        let ep = st.entrypoint.as_ref().filter(|e| !e.argv.is_empty());
        let cmd = if st.entrypoint_here && !st.cmd_here {
            None
        } else {
            st.cmd.as_ref().filter(|c| !c.argv.is_empty())
        };
        let lead = ep.or(cmd);
        if let Some(lead) = lead {
            let entrypoint = ep.map(|e| e.argv.clone()).unwrap_or_default();
            let cmd_argv = match ep {
                Some(e) if !e.exec => Vec::new(),
                _ => cmd.map(|c| c.argv.clone()).unwrap_or_default(),
            };
            let mut argv = entrypoint.clone();
            argv.extend(cmd_argv.iter().cloned());
            out.records.push(StartRecord {
                trigger: StartTrigger::ContainerStart,
                stage: st.name.clone(),
                is_final,
                argv,
                entrypoint,
                cmd: cmd_argv,
                exec: lead.exec,
                workdir: st.workdir.clone(),
                copies: st.copies.clone(),
                builds: st.builds.clone(),
                row: lead.row,
            });
        }
        if let Some(hc) = st.healthcheck.as_ref().filter(|h| !h.argv.is_empty()) {
            out.records.push(StartRecord {
                trigger: StartTrigger::Healthcheck,
                stage: st.name.clone(),
                is_final,
                argv: hc.argv.clone(),
                entrypoint: Vec::new(),
                cmd: hc.argv.clone(),
                exec: hc.exec,
                workdir: st.workdir.clone(),
                copies: st.copies.clone(),
                builds: st.builds.clone(),
                row: hc.row,
            });
        }
    }
    out
}

/// An ENTRYPOINT / CMD body: a JSON string array is the exec form (as
/// written; Docker substitutes nothing in it), anything else the shell form,
/// `shell ++ [body]`. `None` for an empty body.
fn argv_instr(body: &str, shell: &[String], row: usize) -> Option<ArgvInstr> {
    let body = body.trim();
    if body.is_empty() {
        return None;
    }
    if body.starts_with('[')
        && let Ok(argv) = serde_json::from_str::<Vec<String>>(body)
    {
        return Some(ArgvInstr {
            argv,
            exec: true,
            row,
        });
    }
    let mut argv = shell.to_vec();
    argv.push(body.to_string());
    Some(ArgvInstr {
        argv,
        exec: false,
        row,
    })
}

/// `HEALTHCHECK [--opt=v ..] CMD <command>` (exec or shell form); `NONE`
/// (or anything else) is no check.
fn read_healthcheck(body: &str, shell: &[String], row: usize) -> Option<ArgvInstr> {
    let mut rest = body.trim();
    while rest.starts_with("--") {
        rest = rest
            .split_once(char::is_whitespace)
            .map_or("", |(_, r)| r)
            .trim_start();
    }
    let (word, cmd) = rest.split_once(char::is_whitespace)?;
    if !word.eq_ignore_ascii_case("CMD") {
        return None;
    }
    argv_instr(cmd, shell, row)
}

/// `ENV K=V K2="v 2"` or the legacy `ENV K value with spaces`; a value is
/// substituted from the earlier ENVs (an `ARG` reference leaves it unknown).
fn read_env(body: &str, vars: &mut Vec<(String, Option<String>)>) {
    let words = shell_words(body);
    let Some(first) = words.first() else {
        return;
    };
    if first.contains('=') {
        for w in &words {
            if let Some((k, v)) = w.split_once('=') {
                let value = substitute_stage(v, vars);
                vars.push((k.to_string(), value));
            }
        }
    } else {
        let value = words[1..].join(" ");
        let value = substitute_stage(&value, vars);
        vars.push((first.clone(), value));
    }
}

/// `text` with the stage's `ENV` values filled in; `None` when it names an
/// `ARG` (a build may override it) or anything undeclared.
fn substitute_stage(text: &str, vars: &[(String, Option<String>)]) -> Option<String> {
    if !text.contains('$') {
        return Some(text.to_string());
    }
    for name in referenced_vars(text) {
        match vars.iter().rev().find(|(n, _)| *n == name) {
            Some((_, Some(_))) => {}
            _ => return None,
        }
    }
    substitute_args(text, vars)
}

/// The `$NAME` / `${NAME..}` names `text` references.
fn referenced_vars(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find('$') {
        let after = &rest[at + 1..];
        let body = after.strip_prefix('{').unwrap_or(after);
        let end = body
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(body.len());
        out.push(body[..end].to_string());
        rest = &body[end..];
    }
    out
}

/// `/a/./b/../c//` -> `/a/c`; the root stays `/`.
fn normalise_abs(p: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    format!("/{}", parts.join("/"))
}

/// `rel` against the absolute `base` (`rel` absolute wins).
fn join_abs(base: &str, rel: &str) -> String {
    if rel.starts_with('/') {
        normalise_abs(rel)
    } else {
        normalise_abs(&format!("{base}/{rel}"))
    }
}

/// A build-context path, normalised (`./src/` -> `src`, `.` / empty -> `.`);
/// `None` when it climbs out of the context.
fn normalise_rel(p: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            s => parts.push(s),
        }
    }
    Some(if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    })
}

/// `ctx` + the container-path remainder `rest` (`""` or `/x/y`).
fn join_rel(ctx: &str, rest: &str) -> String {
    let rest = rest.trim_start_matches('/');
    if rest.is_empty() {
        ctx.to_string()
    } else if ctx == "." {
        rest.to_string()
    } else {
        format!("{ctx}/{rest}")
    }
}

/// The remainder of `path` under `dir` (`""` when equal), or `None`.
fn under<'a>(path: &'a str, dir: &str) -> Option<&'a str> {
    if dir == "/" {
        return Some(path.trim_start_matches('/'));
    }
    let rest = path.strip_prefix(dir)?;
    (rest.is_empty() || rest.starts_with('/')).then(|| rest.trim_start_matches('/'))
}

/// The container path `src` lands at under `dst`: `dst/<basename>` when the
/// destination is a directory (a trailing `/`, or several sources) and the
/// source is a file, else `dst` itself (a directory's contents go INTO
/// `dst`, as Docker copies them).
fn placement(dst: &str, dst_is_dir: bool, src: &str, src_is_file: bool) -> String {
    let dst_abs = normalise_abs(dst);
    if dst_is_dir && src_is_file {
        let base = src.trim_end_matches('/').rsplit('/').next().unwrap_or(src);
        join_abs(&dst_abs, base)
    } else {
        dst_abs
    }
}

/// A context source that names a file by its shape: a basename with an
/// extension (`entry.sh`, `app.py`). An extensionless one may be a
/// directory, whose contents a copy places at the destination itself.
fn looks_like_file(src: &str) -> bool {
    if src.ends_with('/') {
        return false;
    }
    let base = src.rsplit('/').next().unwrap_or(src);
    base.rfind('.').is_some_and(|dot| dot > 0)
}

/// The build-context path a container path was copied from, through the
/// longest copied prefix (the later copy on a tie).
fn to_context(copies: &[(String, String)], path: &str) -> Option<String> {
    let mut best: Option<(usize, String)> = None;
    for (c, ctx) in copies {
        if let Some(rest) = under(path, c)
            && best.as_ref().is_none_or(|(len, _)| c.len() >= *len)
        {
            best = Some((c.len(), join_rel(ctx, rest)));
        }
    }
    best.map(|(_, ctx)| ctx)
}

/// `COPY` / `ADD [--from=<stage>] [--opt ..] <src>.. <dst>` (or the JSON
/// form). A context source joins the stage's COPY map; a `--from=<stage>`
/// source carries that stage's build outputs (re-rooted at the destination)
/// or, when it is no build output, its COPY map entries. URLs, heredocs and
/// `--from=<external image>` map nothing; a `$ARG` / glob source or an
/// unknown destination is counted `unmapped`.
fn read_copy(
    body: &str,
    is_add: bool,
    state: &mut StageState,
    finals: &[StageState],
    out: &mut DockerStarts,
) {
    let mut rest = body.trim();
    let mut from: Option<String> = None;
    while rest.starts_with("--") {
        let (flag, after) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        if let Some(v) = flag.strip_prefix("--from=") {
            from = Some(unquote(v).to_string());
        }
        rest = after.trim_start();
    }
    let parts: Vec<String> = if rest.starts_with('[') {
        match serde_json::from_str::<Vec<String>>(rest) {
            Ok(p) => p,
            Err(_) => return,
        }
    } else {
        rest.split_whitespace().map(str::to_string).collect()
    };
    let Some((dst, srcs)) = parts.split_last() else {
        return;
    };
    if srcs.is_empty() || srcs.iter().any(|s| s.starts_with("<<")) {
        return;
    }
    let source_stage = match &from {
        None => None,
        Some(f) => {
            let by_name = finals.iter().position(|s| s.name.eq_ignore_ascii_case(f));
            let by_index = f.parse::<usize>().ok().filter(|i| *i < finals.len());
            match by_name.or(by_index) {
                Some(i) => finals.get(i),
                // An external image's files: nothing of this repo.
                None => return,
            }
        }
    };
    let dst_abs = match substitute_stage(dst, &state.vars) {
        Some(d) if d.starts_with('/') => Some(normalise_abs(&d)),
        Some(d) => state.workdir.as_deref().map(|w| join_abs(w, &d)),
        None => None,
    };
    let Some(dst_abs) = dst_abs else {
        out.unmapped += srcs.len();
        return;
    };
    // A directory destination: a trailing `/`, several sources, or a
    // directory the stage already made (`.` / the WORKDIR itself).
    let dst_is_dir = dst.ends_with('/')
        || srcs.len() > 1
        || dst == "."
        || dst.ends_with("/.")
        || state.workdir.as_deref() == Some(dst_abs.as_str());
    let mut mapped = false;
    for src in srcs {
        if is_add && (src.contains("://") || src.starts_with("git@")) {
            continue;
        }
        let Some(src) = substitute_stage(src, &state.vars) else {
            out.unmapped += 1;
            continue;
        };
        if src.contains(['*', '?', '[']) {
            out.unmapped += 1;
            continue;
        }
        match source_stage {
            None => {
                let Some(ctx) = normalise_rel(&src) else {
                    out.unmapped += 1;
                    continue;
                };
                let file = ctx != "." && looks_like_file(&src);
                let at = placement(&dst_abs, dst_is_dir, &ctx, file);
                state.copies.retain(|(c, _)| *c != at);
                state.copies.push((at, ctx));
                mapped = true;
            }
            Some(stage) => {
                let p = normalise_abs(&format!("/{src}"));
                let built: Vec<&BuildLine> = stage
                    .builds
                    .iter()
                    .filter(|b| under(&b.out, &p).is_some() || under(&p, &b.out).is_some())
                    .collect();
                let file = built.iter().any(|b| b.file && b.out == p)
                    || (src != "." && looks_like_file(&src));
                let at = placement(&dst_abs, dst_is_dir, &p, file);
                for b in &built {
                    let mut moved = (*b).clone();
                    moved.out = match under(&b.out, &p) {
                        Some(r) if !r.is_empty() => join_abs(&at, r),
                        _ => at.clone(),
                    };
                    if !state.builds.contains(&moved) {
                        state.builds.push(moved);
                    }
                }
                if built.iter().any(|b| under(&p, &b.out).is_some()) {
                    continue;
                }
                for (c, ctx) in &stage.copies {
                    let entry = if let Some(r) = under(c, &p) {
                        let container = if r.is_empty() {
                            at.clone()
                        } else {
                            join_abs(&at, r)
                        };
                        (container, ctx.clone())
                    } else if let Some(r) = under(&p, c) {
                        (at.clone(), join_rel(ctx, r))
                    } else {
                        continue;
                    };
                    state.copies.retain(|(k, _)| *k != entry.0);
                    state.copies.push(entry);
                }
            }
        }
    }
    if mapped {
        out.copies += 1;
    }
}

/// A RUN body's build-output lines (exec form, or each simple command of the
/// shell form, `cd` followed), appended to the stage's builds.
fn read_run(body: &str, state: &mut StageState, out: &mut DockerStarts) {
    let mut rest = body.trim();
    while rest.starts_with("--") {
        rest = rest
            .split_once(char::is_whitespace)
            .map_or("", |(_, r)| r)
            .trim_start();
    }
    if rest.starts_with("<<") {
        return;
    }
    let commands: Vec<Vec<String>> = if rest.starts_with('[') {
        match serde_json::from_str::<Vec<String>>(rest) {
            Ok(argv) => vec![argv],
            Err(_) => shell_commands(rest),
        }
    } else {
        shell_commands(rest)
    };
    let mut cwd = state.workdir.clone();
    for words in commands {
        let mut words: Vec<String> = words
            .iter()
            .map(|w| substitute_stage(w, &state.vars).unwrap_or_else(|| w.clone()))
            .collect();
        let mut target_dir: Option<String> = None;
        loop {
            match words.first() {
                Some(w) if w == "env" => {
                    words.remove(0);
                }
                Some(w) if shell_assignment(w).is_some() => {
                    if let Some(("CARGO_TARGET_DIR", v)) = shell_assignment(w) {
                        target_dir = Some(v.to_string());
                    }
                    words.remove(0);
                }
                _ => break,
            }
        }
        if words.first().is_some_and(|w| w == "cd") {
            cwd = match (words.get(1), cwd.as_deref()) {
                (Some(d), _) if d.contains('$') => None,
                (Some(d), _) if d.starts_with('/') => Some(normalise_abs(d)),
                (Some(d), Some(base)) => Some(join_abs(base, d)),
                _ => None,
            };
            continue;
        }
        let Some(dir) = cwd.clone() else {
            continue;
        };
        let line = read_go_build(&words, &dir)
            .or_else(|| read_cargo_build(&words, &dir, target_dir.as_deref()))
            .or_else(|| read_dotnet_publish(&words, &dir))
            .or_else(|| read_gradle(&words, &dir))
            .or_else(|| read_maven(&words, &dir));
        let Some(mut line) = line else {
            continue;
        };
        if line.out.contains('$') {
            continue;
        }
        line.src = to_context(&state.copies, &line.workdir);
        line.stage = state.name.clone();
        if let Some(i) = BUILD_TOOLS.iter().position(|t| *t == line.tool) {
            out.builds[i] += 1;
        }
        state.builds.push(line);
    }
}

/// `NAME=value` with an identifier name.
fn shell_assignment(w: &str) -> Option<(&str, &str)> {
    let (k, v) = w.split_once('=')?;
    let mut chars = k.chars();
    let first = chars.next()?;
    ((first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_'))
    .then_some((k, v))
}

fn new_build(tool: &'static str, out: String, workdir: &str, file: bool) -> BuildLine {
    BuildLine {
        tool,
        out,
        pkg: None,
        bin: None,
        proj: None,
        task: None,
        workdir: workdir.to_string(),
        src: None,
        stage: String::new(),
        file,
    }
}

/// `go build [flags] -o <out> [<pkg>]` (`-C <dir>` followed). Without `-o`
/// the output name is the module's, which the file does not say: unread.
fn read_go_build(words: &[String], cwd: &str) -> Option<BuildLine> {
    if words.first()? != "go" || words.get(1)? != "build" {
        return None;
    }
    const VALUE_FLAGS: &[&str] = &[
        "-ldflags",
        "-gcflags",
        "-asmflags",
        "-gccgoflags",
        "-tags",
        "-mod",
        "-modfile",
        "-overlay",
        "-p",
        "-pkgdir",
        "-buildmode",
        "-compiler",
        "-installsuffix",
        "-toolexec",
        "-pgo",
        "-coverpkg",
        "-covermode",
    ];
    let mut dir = cwd.to_string();
    let mut out: Option<String> = None;
    let mut pkg: Option<String> = None;
    let mut i = 2;
    while i < words.len() {
        let w = &words[i];
        if w == "-o" {
            out = words.get(i + 1).cloned();
            i += 2;
            continue;
        }
        if let Some(v) = w.strip_prefix("-o=") {
            out = Some(v.to_string());
        } else if w == "-C" {
            if let Some(d) = words.get(i + 1) {
                dir = join_abs(&dir, d);
            }
            i += 2;
            continue;
        } else if VALUE_FLAGS.contains(&w.as_str()) {
            i += 2;
            continue;
        } else if !w.starts_with('-') && pkg.is_none() {
            pkg = Some(w.clone());
        }
        i += 1;
    }
    let out = out?;
    let pkg = pkg.unwrap_or_else(|| ".".to_string());
    let mut path = join_abs(&dir, &out);
    if out.ends_with('/') {
        let leaf = pkg
            .trim_end_matches('/')
            .rsplit('/')
            .find(|s| !s.is_empty() && *s != ".")
            .unwrap_or("")
            .to_string();
        if leaf.is_empty() {
            return None;
        }
        path = join_abs(&path, &leaf);
    }
    let mut line = new_build("go", path, &dir, true);
    line.pkg = Some(pkg);
    Some(line)
}

/// `cargo build [--release | --profile <p>] [--target <t>] [--bin <n> | -p
/// <package>]`: `target/[<t>/]<profile>/<n>`, or the profile directory when
/// no bin is named.
fn read_cargo_build(words: &[String], cwd: &str, target_env: Option<&str>) -> Option<BuildLine> {
    if words.first()? != "cargo" || words.get(1)? != "build" {
        return None;
    }
    let mut profile = "debug".to_string();
    let mut triple: Option<String> = None;
    let mut bin: Option<String> = None;
    let mut package: Option<String> = None;
    let mut target_dir = target_env.map(str::to_string);
    let mut i = 2;
    let take = |i: usize, flag: &str| -> Option<String> {
        let w = &words[i];
        if w == flag {
            words.get(i + 1).cloned()
        } else {
            w.strip_prefix(&format!("{flag}=")).map(str::to_string)
        }
    };
    while i < words.len() {
        let w = &words[i];
        let step = if w.contains('=') { 1 } else { 2 };
        if w == "--release" || w == "-r" {
            profile = "release".to_string();
        } else if let Some(p) = take(i, "--profile") {
            profile = if p == "dev" { "debug".to_string() } else { p };
            i += step;
            continue;
        } else if let Some(t) = take(i, "--target") {
            triple = Some(t);
            i += step;
            continue;
        } else if let Some(b) = take(i, "--bin") {
            bin = Some(b);
            i += step;
            continue;
        } else if let Some(p) = take(i, "--package").or_else(|| take(i, "-p")) {
            package = Some(p);
            i += step;
            continue;
        } else if let Some(d) = take(i, "--target-dir") {
            target_dir = Some(d);
            i += step;
            continue;
        } else if [
            "--manifest-path",
            "--features",
            "-F",
            "-j",
            "--jobs",
            "--config",
            "-Z",
        ]
        .contains(&w.as_str())
        {
            i += 2;
            continue;
        }
        i += 1;
    }
    let root = join_abs(cwd, target_dir.as_deref().unwrap_or("target"));
    let mut dir = root;
    if let Some(t) = &triple {
        dir = join_abs(&dir, t);
    }
    dir = join_abs(&dir, &profile);
    let name = bin.or(package);
    let out = match &name {
        Some(n) => join_abs(&dir, n),
        None => dir,
    };
    let mut line = new_build("cargo", out, cwd, name.is_some());
    line.bin = name;
    Some(line)
}

/// `dotnet publish [<proj>] -o|--output <dir>`.
fn read_dotnet_publish(words: &[String], cwd: &str) -> Option<BuildLine> {
    if words.first()? != "dotnet" || words.get(1)? != "publish" {
        return None;
    }
    const VALUE_FLAGS: &[&str] = &[
        "-c",
        "--configuration",
        "-r",
        "--runtime",
        "-f",
        "--framework",
        "-a",
        "--arch",
        "--os",
        "-v",
        "--verbosity",
        "--version-suffix",
        "--source",
        "-s",
    ];
    let mut out: Option<String> = None;
    let mut proj: Option<String> = None;
    let mut i = 2;
    while i < words.len() {
        let w = &words[i];
        if w == "-o" || w == "--output" {
            out = words.get(i + 1).cloned();
            i += 2;
            continue;
        }
        if let Some(v) = w
            .strip_prefix("--output=")
            .or_else(|| w.strip_prefix("-o="))
        {
            out = Some(v.to_string());
        } else if VALUE_FLAGS.contains(&w.as_str()) {
            i += 2;
            continue;
        } else if !w.starts_with('-') && proj.is_none() && w != "true" && w != "false" {
            proj = Some(w.clone());
        }
        i += 1;
    }
    let out = join_abs(cwd, &out?);
    let mut line = new_build("dotnet", out, cwd, false);
    line.proj = proj;
    Some(line)
}

/// `./gradlew | gradle [opts] [<path>:]installDist | bootJar | jar` (`sh
/// ./gradlew ..` too): `<project>/build/install` or `<project>/build/libs`.
fn read_gradle(words: &[String], cwd: &str) -> Option<BuildLine> {
    let words = strip_shell_runner(words, &["gradlew", "gradle"]);
    let head = words.first()?;
    let base = head.rsplit('/').next().unwrap_or(head);
    if base != "gradlew" && base != "gradle" {
        return None;
    }
    let mut project = cwd.to_string();
    let mut task: Option<&String> = None;
    let mut i = 1;
    while i < words.len() {
        let w = &words[i];
        if w == "-p" || w == "--project-dir" {
            if let Some(d) = words.get(i + 1) {
                project = join_abs(&project, d);
            }
            i += 2;
            continue;
        }
        if !w.starts_with('-') && task.is_none() {
            let leaf = w.rsplit(':').next().unwrap_or(w);
            if ["installDist", "bootJar", "jar"].contains(&leaf) {
                task = Some(w);
            }
        }
        i += 1;
    }
    let task = task?;
    let mut segs: Vec<&str> = task.split(':').filter(|s| !s.is_empty()).collect();
    let leaf = segs.pop()?;
    let module = segs.join("/");
    let dir = if module.is_empty() {
        project
    } else {
        join_abs(&project, &module)
    };
    let sub = if leaf == "installDist" {
        "build/install"
    } else {
        "build/libs"
    };
    let mut line = new_build("gradle", join_abs(&dir, sub), cwd, false);
    line.task = Some(task.clone());
    Some(line)
}

/// `mvn | ./mvnw [..] package | install` (`-f <pom>`, a single `-pl
/// <module>` followed): `<project>/target`.
fn read_maven(words: &[String], cwd: &str) -> Option<BuildLine> {
    let words = strip_shell_runner(words, &["mvnw", "mvn"]);
    let head = words.first()?;
    let base = head.rsplit('/').next().unwrap_or(head);
    if base != "mvn" && base != "mvnw" {
        return None;
    }
    let mut project = cwd.to_string();
    let mut module: Option<String> = None;
    let mut goal: Option<&String> = None;
    let mut i = 1;
    while i < words.len() {
        let w = &words[i];
        if w == "-f" || w == "--file" {
            if let Some(pom) = words.get(i + 1) {
                let p = join_abs(&project, pom);
                project = if p.ends_with(".xml") {
                    p.rsplit_once('/')
                        .map(|(d, _)| {
                            if d.is_empty() {
                                "/".to_string()
                            } else {
                                d.to_string()
                            }
                        })
                        .unwrap_or(p)
                } else {
                    p
                };
            }
            i += 2;
            continue;
        }
        if w == "-pl" || w == "--projects" {
            module = words.get(i + 1).filter(|m| !m.contains(',')).cloned();
            i += 2;
            continue;
        }
        if ["-P", "-s", "--settings", "-T", "--threads", "-gs"].contains(&w.as_str()) {
            i += 2;
            continue;
        }
        if (w == "package" || w == "install") && goal.is_none() {
            goal = Some(w);
        }
        i += 1;
    }
    let goal = goal?;
    let dir = match &module {
        Some(m) => join_abs(&project, m),
        None => project,
    };
    let mut line = new_build("maven", join_abs(&dir, "target"), cwd, false);
    line.task = Some(goal.clone());
    Some(line)
}

/// `sh ./gradlew ..` / `bash mvnw ..` -> `./gradlew ..`.
fn strip_shell_runner<'a>(words: &'a [String], tools: &[&str]) -> &'a [String] {
    match (words.first(), words.get(1)) {
        (Some(sh), Some(tool))
            if (sh == "sh" || sh == "bash")
                && tools.contains(&tool.rsplit('/').next().unwrap_or(tool)) =>
        {
            &words[1..]
        }
        _ => words,
    }
}

/// A RUN shell string's simple commands, split on `&&` / `||` / `;` / `|` /
/// `&` / parentheses, each a list of words under shell quoting (single
/// quotes literal, double quotes and backslashes escaping). A line scanner,
/// not a shell: nothing is expanded.
fn shell_commands(s: &str) -> Vec<Vec<String>> {
    let mut commands: Vec<Vec<String>> = Vec::new();
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = s.chars().peekable();
    let flush_word = |word: &mut String, in_word: &mut bool, words: &mut Vec<String>| {
        if *in_word {
            words.push(std::mem::take(word));
            *in_word = false;
        }
    };
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                for q in chars.by_ref() {
                    if q == '\'' {
                        break;
                    }
                    word.push(q);
                }
            }
            '"' => {
                in_word = true;
                while let Some(q) = chars.next() {
                    match q {
                        '"' => break,
                        '\\' => {
                            if let Some(n) = chars.next() {
                                if !matches!(n, '"' | '\\' | '$' | '`') {
                                    word.push('\\');
                                }
                                word.push(n);
                            }
                        }
                        _ => word.push(q),
                    }
                }
            }
            '\\' => {
                in_word = true;
                if let Some(n) = chars.next() {
                    word.push(n);
                }
            }
            ';' | '&' | '|' | '(' | ')' | '\n' => {
                flush_word(&mut word, &mut in_word, &mut words);
                if !words.is_empty() {
                    commands.push(std::mem::take(&mut words));
                }
            }
            c if c.is_whitespace() => flush_word(&mut word, &mut in_word, &mut words),
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    flush_word(&mut word, &mut in_word, &mut words);
    if !words.is_empty() {
        commands.push(words);
    }
    commands
}

/// Whitespace-separated words under shell quoting (one command's worth).
fn shell_words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match quote {
            Some(q) if c == q => quote = None,
            Some('"') if c == '\\' => {
                if let Some(n) = chars.next() {
                    word.push(n);
                }
            }
            Some(_) => word.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                in_word = true;
            }
            None if c == '\\' => {
                in_word = true;
                if let Some(n) = chars.next() {
                    word.push(n);
                }
            }
            None if c.is_whitespace() => {
                if in_word {
                    out.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            None => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        out.push(word);
    }
    out
}

/// The container paths an exec-form binary target can be: itself when
/// absolute, joined with the WORKDIR when relative with a `/`, each
/// [`PATH_DIRS`] entry for a bare name.
fn binary_candidates(target: &str, workdir: Option<&str>) -> Vec<String> {
    if target.starts_with('/') {
        vec![normalise_abs(target)]
    } else if target.contains('/') {
        workdir.map(|w| join_abs(w, target)).into_iter().collect()
    } else {
        PATH_DIRS.iter().map(|d| join_abs(d, target)).collect()
    }
}

/// `argv[0]` as a CLI_INVOCATION binary name, the cli extractor's rule: the
/// last path segment without `.exe`; `None` when empty, over-long or
/// dynamic.
fn invoked_binary_name(target: &str) -> Option<String> {
    let base = target.rsplit(['/', '\\']).next().unwrap_or(target);
    let base = base.strip_suffix(".exe").unwrap_or(base);
    if base.is_empty() || base.len() > 64 || base.contains(['$', '{']) {
        return None;
    }
    Some(base.to_string())
}

/// The CLI_INVOCATION a container start's binary target warrants: a
/// [`TargetForm::Binary`](glia_code_domain::hosts::TargetForm::Binary)
/// start whose binary no build line of the stage wrote and no COPY put
/// there from the context (a repo file the host-start rules map), and whose
/// name is no script file. Only a program found on `PATH` counts: a bare
/// name or an absolute path in a [`PATH_DIRS`] directory. `(bin, args)`.
fn binary_invocation(
    rec: &StartRecord,
    target: &glia_code_domain::hosts::StartTarget,
) -> Option<(String, Vec<String>)> {
    use glia_code_domain::hosts::TargetForm;
    if target.refused.is_some() || target.form != TargetForm::Binary {
        return None;
    }
    // Only a `PATH` lookup names an installed program: a bare name, or an
    // absolute path in a `PATH` directory. A relative or other absolute path
    // is a file the image itself holds (copied or built in).
    let on_path = !target.target.contains('/')
        || target
            .target
            .rsplit_once('/')
            .is_some_and(|(dir, _)| PATH_DIRS.contains(&normalise_abs(dir).as_str()));
    if !on_path {
        return None;
    }
    let candidates = binary_candidates(&target.target, rec.workdir.as_deref());
    let built = candidates
        .iter()
        .any(|c| rec.builds.iter().any(|b| under(c, &b.out).is_some()));
    let copied = candidates
        .iter()
        .any(|c| to_context(&rec.copies, c).is_some());
    if built || copied {
        return None;
    }
    let bin = invoked_binary_name(&target.target)?;
    let ext = bin.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    if ext.is_some_and(|e| SCRIPT_EXTS.contains(&e.as_str())) {
        return None;
    }
    Some((bin, target.args.clone()))
}

// ----------------------------------------------------------------------------
// k8s manifest extraction. Documents may be multi-doc YAML separated by `---`.
// ----------------------------------------------------------------------------

const K8S_KINDS: &[&str] = &[
    "Deployment",
    "StatefulSet",
    "DaemonSet",
    "Job",
    "CronJob",
    "Service",
    "Ingress",
    "ConfigMap",
    "Secret",
    "ReplicaSet",
];

fn extract_k8s_documents(source: &str, module_id: NodeId, repo: RepoId, out: &mut IacNodes) {
    for doc in source.split("\n---") {
        let doc = doc.trim_start_matches('\n');
        if doc.is_empty() {
            continue;
        }
        let Some(kind) = read_k8s_kind(doc) else {
            continue;
        };
        let Some(name) = read_k8s_name(doc) else {
            continue;
        };
        let kind_lower = kind.to_ascii_lowercase();
        emit_resource(out, &kind_lower, &name, module_id, repo);

        // Link container images. Walk every `image:` line in the doc; for
        // each, emit an `infra:image:<basename>` node + REFERENCES edge from
        // this resource.
        let from_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo,
            node_kind::INFRA_RESOURCE,
            &infra::qname(&kind_lower, &name),
        );
        for image in image_refs_in_doc(doc) {
            let basename = image_basename(&image);
            if basename.is_empty() {
                continue;
            }
            emit_resource(out, "image", &basename, module_id, repo);
            let to_id = NodeId::from_parts(
                GRAPH_TYPE,
                repo,
                node_kind::INFRA_RESOURCE,
                &infra::qname("image", &basename),
            );
            out.edges.push(Edge {
                from: from_id,
                to: to_id,
                category: edge_category::INFRA_REFERENCES,
                confidence: Confidence::Medium,
                cells: Vec::new(),
            });
        }
    }
}

fn read_k8s_kind(doc: &str) -> Option<String> {
    for line in doc.lines() {
        let t = line.trim_start();
        if t.starts_with('-') || t.is_empty() {
            continue;
        }
        if let Some(rest) = t.strip_prefix("kind:") {
            let v = rest.trim().trim_matches(|c| c == '"' || c == '\'');
            if K8S_KINDS.contains(&v) {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// Read `metadata.name` — the first `name:` after `metadata:` at lower indent.
/// Cheap heuristic: the second `name:` line (first is usually under metadata)
/// is fragile; instead match the indent of `metadata:` and look for `name:`
/// indented under it.
fn read_k8s_name(doc: &str) -> Option<String> {
    let mut in_metadata = false;
    let mut metadata_indent: usize = 0;
    for line in doc.lines() {
        let indent = line.len() - line.trim_start().len();
        let t = line.trim();
        if t.starts_with("metadata:") {
            in_metadata = true;
            metadata_indent = indent;
            continue;
        }
        if in_metadata {
            if t.is_empty() {
                continue;
            }
            // Left the metadata block.
            if indent <= metadata_indent {
                in_metadata = false;
                continue;
            }
            if let Some(rest) = t.strip_prefix("name:") {
                let v = rest.trim().trim_matches(|c| c == '"' || c == '\'');
                if !v.is_empty() {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

fn image_refs_in_doc(doc: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in doc.lines() {
        let t = line.trim_start();
        let t = t.strip_prefix("- ").unwrap_or(t);
        if let Some(rest) = t.strip_prefix("image:") {
            let v = rest.trim().trim_matches(|c| c == '"' || c == '\'');
            // Skip inline-list / object references.
            if v.starts_with('[') || v.starts_with('{') || v.is_empty() {
                continue;
            }
            out.push(v.to_string());
        }
    }
    out
}

// ----------------------------------------------------------------------------
// docker-compose extraction. Each `services:<name>:` block becomes an
// `infra:service:<name>` node; if the block has `image:`, also emit
// `infra:image:<basename>` and REFERENCES edge.
// ----------------------------------------------------------------------------

fn extract_compose_services(source: &str, module_id: NodeId, repo: RepoId, out: &mut IacNodes) {
    // Find `services:` block at top level (indent 0).
    let mut in_services = false;
    let mut services_indent: usize = 0;
    let mut svc_indent: Option<usize> = None;
    let mut current_svc: Option<String> = None;

    for line in source.lines() {
        let indent = line.len() - line.trim_start().len();
        let t = line.trim_end();
        let trimmed = t.trim_start();

        if !in_services {
            if trimmed == "services:" {
                in_services = true;
                services_indent = indent;
            }
            continue;
        }

        // Done with services block?
        if !trimmed.is_empty() && indent <= services_indent {
            in_services = false;
            continue;
        }

        // A service header is a `name:` at the first level of nesting under
        // `services:`. Capture it and its indent so we know what counts as
        // "inside this service".
        if let Some(name_line) = trimmed.strip_suffix(':') {
            // Distinguish service header (`api:`) from subkeys (`environment:`).
            let is_top_level_under_services = match svc_indent {
                None => indent > services_indent,
                Some(idx) => indent == idx,
            };
            if is_top_level_under_services && is_compose_service_name(name_line) {
                let name = name_line.to_string();
                emit_resource(out, "service", &name, module_id, repo);
                current_svc = Some(name);
                svc_indent = Some(indent);
                continue;
            }
        }

        // Image ref inside the current service block.
        if let Some(rest) = trimmed.strip_prefix("image:")
            && let Some(svc) = &current_svc
        {
            let v = rest.trim().trim_matches(|c| c == '"' || c == '\'');
            if !v.is_empty() && !v.starts_with('$') {
                let basename = image_basename(v);
                if !basename.is_empty() {
                    emit_resource(out, "image", &basename, module_id, repo);
                    let from_id = NodeId::from_parts(
                        GRAPH_TYPE,
                        repo,
                        node_kind::INFRA_RESOURCE,
                        &infra::qname("service", svc),
                    );
                    let to_id = NodeId::from_parts(
                        GRAPH_TYPE,
                        repo,
                        node_kind::INFRA_RESOURCE,
                        &infra::qname("image", &basename),
                    );
                    out.edges.push(Edge {
                        from: from_id,
                        to: to_id,
                        category: edge_category::INFRA_REFERENCES,
                        confidence: Confidence::Medium,
                        cells: Vec::new(),
                    });
                }
            }
        }
    }
}

/// docker-compose service names are identifiers (alphanumeric + `-` + `_`).
/// Filters out common subkeys like `environment` / `image` / `volumes`.
fn is_compose_service_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 128 {
        return false;
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return false;
    }
    !matches!(
        name,
        "version"
            | "services"
            | "networks"
            | "volumes"
            | "configs"
            | "secrets"
            | "image"
            | "build"
            | "command"
            | "ports"
            | "environment"
            | "depends_on"
            | "restart"
            | "labels"
            | "deploy"
            | "healthcheck"
            | "logging"
    )
}

// ----------------------------------------------------------------------------
// GH.16: orchestrator starts. Compose services' `build:` / `command:` /
// `entrypoint:` / `working_dir:`, Kubernetes pod-template containers'
// `command:` / `args:` / `workingDir:` and skaffold `build.artifacts[]`, read
// through a small block-YAML reader (below), so flow and block lists,
// anchors and `<<:` merges read alike.
// ----------------------------------------------------------------------------

/// The Kubernetes kinds whose pod template starts containers.
const K8S_WORKLOADS: &[&str] = &[
    "Deployment",
    "StatefulSet",
    "DaemonSet",
    "Job",
    "CronJob",
    "ReplicaSet",
];

/// Per-file counts behind `[compose-start]`, `[k8s-start]` and
/// `[skaffold-build]`.
#[derive(Debug, Default)]
struct OrchestratorTally {
    services: usize,
    builds: usize,
    commands: usize,
    entrypoints: usize,
    compose_refused: usize,
    containers: usize,
    k8s_commands: usize,
    k8s_args: usize,
    k8s_refused: usize,
    /// Workload documents seen (with or without containers).
    workloads: usize,
    artifacts: usize,
    dockerfiles: usize,
    skaffold_refused: usize,
    skaffold_docs: usize,
}

/// An argv a file writes: the words, and whether it was one string (compose
/// splits it with shell-word rules, no shell runs) or a list.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ArgvField {
    argv: Vec<String>,
    string: bool,
}

/// One field's reading: absent, read, or refused (a Helm `{{ }}` template, a
/// value of the wrong shape, an alias to nothing).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Field<T> {
    Absent,
    Read(T),
    Refused,
}

impl<T> Field<T> {
    fn read(&self) -> Option<&T> {
        match self {
            Field::Read(t) => Some(t),
            _ => None,
        }
    }
    fn present(&self) -> bool {
        !matches!(self, Field::Absent)
    }
}

/// A declared docker build: the context and Dockerfile, repo-relative.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DeclaredBuild {
    context: String,
    /// `None` for an inline Dockerfile (`dockerfile_inline`) or a builder
    /// that reads none (skaffold jib / ko / buildpacks / bazel / custom).
    dockerfile: Option<String>,
    target: Option<String>,
}

impl DeclaredBuild {
    /// The image name the Dockerfile reader gives this build's Dockerfile
    /// (GH.13's `dockerfile_image_name`), so a declaring service meets it.
    fn built_image(&self) -> Option<String> {
        self.dockerfile
            .as_deref()
            .map(|d| dockerfile_image_name(d).name)
    }
}

#[derive(serde::Serialize)]
struct ComposeStartCell<'a> {
    start: ComposeStart<'a>,
}

#[derive(serde::Serialize)]
struct ComposeStart<'a> {
    host: &'static str,
    service: &'a str,
    trigger: &'static str,
    command: Option<&'a [String]>,
    /// `string` (split with shell-word rules) or `list`; `null` when unset.
    command_form: Option<&'static str>,
    entrypoint: Option<&'a [String]>,
    entrypoint_form: Option<&'static str>,
    workdir: Option<&'a str>,
    image: Option<&'a str>,
    built_image: Option<String>,
    context: Option<&'a str>,
    dockerfile: Option<&'a str>,
    target_stage: Option<&'a str>,
    refused: Vec<&'static str>,
    decl: String,
}

#[derive(serde::Serialize)]
struct K8sStartCell<'a> {
    start: K8sStart<'a>,
}

#[derive(serde::Serialize)]
struct K8sStart<'a> {
    host: &'static str,
    workload: &'a str,
    container: Option<&'a str>,
    init: bool,
    trigger: &'static str,
    schedule: Option<&'a str>,
    image: Option<&'a str>,
    command: Option<&'a [String]>,
    args: Option<&'a [String]>,
    workdir: Option<&'a str>,
    refused: Vec<&'static str>,
    decl: String,
}

#[derive(serde::Serialize)]
struct BuildCell<'a> {
    build: BuildBody<'a>,
}

#[derive(serde::Serialize)]
struct BuildBody<'a> {
    host: &'static str,
    image: &'a str,
    builder: &'a str,
    context: &'a str,
    dockerfile: Option<&'a str>,
    target_stage: Option<&'a str>,
    built_image: Option<String>,
    decl: String,
}

fn json_cell<T: serde::Serialize>(body: &T) -> glia_core::Cell {
    glia_core::Cell {
        kind: glia_code_domain::cell_type::CODE,
        payload: glia_core::CellPayload::Json(serde_json::to_string(body).unwrap_or_default()),
    }
}

/// Push `cell` onto the node `id` this extraction minted.
fn attach_cell(out: &mut IacNodes, id: NodeId, cell: glia_core::Cell) {
    if let Some(node) = out.nodes.iter_mut().find(|n| n.id == id) {
        node.cells.push(cell);
    }
}

fn infra_id(repo: RepoId, kind: &str, name: &str) -> NodeId {
    NodeId::from_parts(
        GRAPH_TYPE,
        repo,
        node_kind::INFRA_RESOURCE,
        &infra::qname(kind, name),
    )
}

/// One INFRA_REFERENCES edge `from -> to`, once.
fn push_reference(out: &mut IacNodes, from: NodeId, to: NodeId) {
    if out
        .edges
        .iter()
        .any(|e| e.from == from && e.to == to && e.category == edge_category::INFRA_REFERENCES)
    {
        return;
    }
    out.edges.push(Edge {
        from,
        to,
        category: edge_category::INFRA_REFERENCES,
        confidence: Confidence::Medium,
        cells: Vec::new(),
    });
}

/// A Helm / Go template the file holds instead of a value.
fn is_template(s: &str) -> bool {
    s.contains("{{")
}

/// A compose `command:` / `entrypoint:`: a string (split with shell-word
/// rules: compose runs no shell for it) or a list of scalars.
fn read_compose_argv(v: Option<&Yv>) -> Field<ArgvField> {
    match v {
        None | Some(Yv::Null) => Field::Absent,
        Some(Yv::Str(s)) if is_template(s) => Field::Refused,
        Some(Yv::Str(s)) => Field::Read(ArgvField {
            argv: shell_words(s),
            string: true,
        }),
        Some(Yv::List(_)) => match scalar_list(v) {
            Some(argv) => Field::Read(ArgvField {
                argv,
                string: false,
            }),
            None => Field::Refused,
        },
        Some(_) => Field::Refused,
    }
}

/// A list of plain / quoted scalars, none a template.
fn scalar_list(v: Option<&Yv>) -> Option<Vec<String>> {
    let Some(Yv::List(items)) = v else {
        return None;
    };
    items
        .iter()
        .map(|(_, item)| match item {
            Yv::Str(s) if !is_template(s) => Some(s.clone()),
            _ => None,
        })
        .collect()
}

/// A Kubernetes `command:` / `args:`: a list only (the API refuses a string).
fn read_k8s_list(v: Option<&Yv>) -> Field<Vec<String>> {
    match v {
        None | Some(Yv::Null) => Field::Absent,
        Some(Yv::List(_)) => scalar_list(v).map_or(Field::Refused, Field::Read),
        Some(_) => Field::Refused,
    }
}

/// A scalar setting (`working_dir:`, `image:`), refused when templated.
fn read_scalar(v: Option<&Yv>) -> Field<String> {
    match v {
        None | Some(Yv::Null) => Field::Absent,
        Some(Yv::Str(s)) if is_template(s) => Field::Refused,
        Some(Yv::Str(s)) => Field::Read(s.clone()),
        Some(_) => Field::Refused,
    }
}

/// A declared build context, `rel` to the declaring file's directory `dir`,
/// as a repo-relative path. `None` for a remote context (a URL, `git@..`),
/// an absolute host path, an interpolated or templated one, or one that
/// climbs out of the repo.
fn declared_path(dir: &str, rel: &str) -> Option<String> {
    let rel = rel.trim();
    if rel.is_empty()
        || rel.starts_with('/')
        || rel.contains("://")
        || rel.starts_with("git@")
        || rel.contains('$')
        || is_template(rel)
    {
        return None;
    }
    let joined = if dir == "." {
        rel.to_string()
    } else {
        format!("{dir}/{rel}")
    };
    normalise_rel(&joined)
}

/// `dockerfile` (relative to the context, `Dockerfile` when unset) under the
/// repo-relative `context`.
fn dockerfile_under(context: &str, dockerfile: Option<&str>) -> Option<String> {
    let file = dockerfile.unwrap_or("Dockerfile");
    declared_path(context, file)
}

/// A compose `build:` (a context string, or a map of `context` /
/// `dockerfile` / `dockerfile_inline` / `target`).
fn read_compose_build(v: Option<&Yv>, dir: &str) -> Field<DeclaredBuild> {
    let (context, dockerfile, inline, target) = match v {
        None | Some(Yv::Null) => return Field::Absent,
        Some(Yv::Str(s)) => (s.as_str(), None, false, None),
        Some(m @ Yv::Map(_)) => {
            let context = match m.get("context") {
                None | Some(Yv::Null) => ".",
                Some(Yv::Str(s)) => s.as_str(),
                Some(_) => return Field::Refused,
            };
            let dockerfile = match m.get("dockerfile") {
                None | Some(Yv::Null) => None,
                Some(Yv::Str(s)) => Some(s.as_str()),
                Some(_) => return Field::Refused,
            };
            let target = match read_scalar(m.get("target")) {
                Field::Absent => None,
                Field::Read(t) => Some(t),
                Field::Refused => return Field::Refused,
            };
            (
                context,
                dockerfile,
                m.get("dockerfile_inline").is_some(),
                target,
            )
        }
        Some(_) => return Field::Refused,
    };
    let Some(context) = declared_path(dir, context) else {
        return Field::Refused;
    };
    let dockerfile = if inline {
        None
    } else {
        match dockerfile_under(&context, dockerfile) {
            Some(d) => Some(d),
            None => return Field::Refused,
        }
    };
    Field::Read(DeclaredBuild {
        context,
        dockerfile,
        target,
    })
}

/// The `build` CODE cell on a declared image node: which context / Dockerfile
/// builds the image a file names, for the cross-file naming join.
fn build_cell(
    host: &'static str,
    image: &str,
    builder: &str,
    build: &DeclaredBuild,
    path: &str,
    row: usize,
) -> glia_core::Cell {
    json_cell(&BuildCell {
        build: BuildBody {
            host,
            image,
            builder,
            context: &build.context,
            dockerfile: build.dockerfile.as_deref(),
            target_stage: build.target.as_deref(),
            built_image: build.built_image(),
            decl: format!("{path}:{}", row + 1),
        },
    })
}

/// GH.16: read the orchestrator starts of one YAML file (compose, k8s
/// workloads, skaffold) into start cells, the compose service -> built image
/// edges and the per-file markers. Files that hold none of the three skip the
/// YAML read.
fn extract_orchestrator_starts(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
    out: &mut IacNodes,
) {
    if !(source.contains("services:")
        || source.contains("containers:")
        || source.contains("skaffold/"))
    {
        return;
    }
    let dir = dockerfile_context(path);
    let mut tally = OrchestratorTally::default();
    for doc in yaml_docs(source) {
        let root = parse_yaml_doc(doc);
        let api = root.get("apiVersion").and_then(Yv::str);
        if api.is_some_and(|a| a.starts_with("skaffold/")) {
            skaffold_artifacts(&root, path, &dir, module_id, repo, out, &mut tally);
        } else if let Some(kind) = root.get("kind").and_then(Yv::str)
            && K8S_WORKLOADS.contains(&kind)
        {
            k8s_containers(&root, kind, path, module_id, repo, out, &mut tally);
        } else if api.is_none()
            && let Some(Yv::Map(_)) = root.get("services")
        {
            compose_services(&root, path, &dir, module_id, repo, out, &mut tally);
        }
    }
    if tally.services > 0 || tally.compose_refused > 0 {
        out.markers.push(format!(
            "[compose-start] file={path} services={} builds={} commands={} entrypoints={} refused={}",
            tally.services, tally.builds, tally.commands, tally.entrypoints, tally.compose_refused
        ));
    }
    if tally.workloads > 0 {
        out.markers.push(format!(
            "[k8s-start] file={path} containers={} commands={} args={} refused={}",
            tally.containers, tally.k8s_commands, tally.k8s_args, tally.k8s_refused
        ));
    }
    if tally.skaffold_docs > 0 {
        out.markers.push(format!(
            "[skaffold-build] file={path} artifacts={} dockerfiles={} refused={}",
            tally.artifacts, tally.dockerfiles, tally.skaffold_refused
        ));
    }
}

/// Every service of a compose file's `services:` map.
fn compose_services(
    root: &Yv,
    path: &str,
    dir: &str,
    module_id: NodeId,
    repo: RepoId,
    out: &mut IacNodes,
    tally: &mut OrchestratorTally,
) {
    let Some(Yv::Map(services)) = root.get("services") else {
        return;
    };
    for (name, header_row, svc) in services {
        if !is_compose_service_name(name) || !matches!(svc, Yv::Map(_)) {
            continue;
        }
        tally.services += 1;
        let build = read_compose_build(svc.get("build"), dir);
        let command = read_compose_argv(svc.get("command"));
        let entrypoint = read_compose_argv(svc.get("entrypoint"));
        let workdir = read_scalar(svc.get("working_dir"));
        let image = read_scalar(svc.get("image"));
        let mut refused: Vec<&'static str> = Vec::new();
        for (field, is_refused) in [
            ("build", matches!(build, Field::Refused)),
            ("command", matches!(command, Field::Refused)),
            ("entrypoint", matches!(entrypoint, Field::Refused)),
            ("working_dir", matches!(workdir, Field::Refused)),
            ("image", matches!(image, Field::Refused)),
        ] {
            if is_refused {
                refused.push(field);
            }
        }
        tally.compose_refused += refused.len();
        tally.builds += usize::from(build.read().is_some());
        tally.commands += usize::from(command.read().is_some());
        tally.entrypoints += usize::from(entrypoint.read().is_some());
        if !(build.present() || command.present() || entrypoint.present() || workdir.present()) {
            continue;
        }
        emit_resource(out, "service", name, module_id, repo);
        let svc_id = infra_id(repo, "service", name);
        // An interpolated `image: ${REGISTRY}/x` names no image the file
        // shows (the line reader above skips it too).
        let declared = image
            .read()
            .filter(|i| !i.contains('$'))
            .map(|i| image_basename(i));
        if let Some(b) = build.read() {
            if let Some(built) = b.built_image()
                && !built.is_empty()
            {
                emit_resource(out, "image", &built, module_id, repo);
                push_reference(out, svc_id, infra_id(repo, "image", &built));
            }
            if let Some(decl_image) = declared.as_deref().filter(|d| !d.is_empty()) {
                let build_row = svc.entry("build").map_or(*header_row, |(r, _)| r);
                emit_resource(out, "image", decl_image, module_id, repo);
                let cell = build_cell("compose", decl_image, "docker", b, path, build_row);
                attach_cell(out, infra_id(repo, "image", decl_image), cell);
            }
        }
        let decl_row = ["command", "entrypoint", "build", "working_dir"]
            .iter()
            .find_map(|k| svc.entry(k).map(|(r, _)| r))
            .unwrap_or(*header_row);
        let form =
            |f: &Field<ArgvField>| f.read().map(|a| if a.string { "string" } else { "list" });
        let cell = json_cell(&ComposeStartCell {
            start: ComposeStart {
                host: "compose",
                service: name,
                trigger: "container_start",
                command: command.read().map(|a| a.argv.as_slice()),
                command_form: form(&command),
                entrypoint: entrypoint.read().map(|a| a.argv.as_slice()),
                entrypoint_form: form(&entrypoint),
                workdir: workdir.read().map(String::as_str),
                image: declared.as_deref(),
                built_image: build.read().and_then(DeclaredBuild::built_image),
                context: build.read().map(|b| b.context.as_str()),
                dockerfile: build.read().and_then(|b| b.dockerfile.as_deref()),
                target_stage: build.read().and_then(|b| b.target.as_deref()),
                refused,
                decl: format!("{path}:{}", decl_row + 1),
            },
        });
        attach_cell(out, svc_id, cell);
    }
}

/// Every container (and init container) of one workload's pod template.
fn k8s_containers(
    root: &Yv,
    kind: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
    out: &mut IacNodes,
    tally: &mut OrchestratorTally,
) {
    tally.workloads += 1;
    let kind_lower = kind.to_ascii_lowercase();
    let name = match read_scalar(root.path(&["metadata", "name"])) {
        Field::Read(n) => n,
        Field::Refused => {
            tally.k8s_refused += 1;
            return;
        }
        Field::Absent => return,
    };
    let cron = kind == "CronJob";
    let (pod, schedule) = if cron {
        (
            root.path(&["spec", "jobTemplate", "spec", "template", "spec"]),
            read_scalar(root.path(&["spec", "schedule"])),
        )
    } else {
        (root.path(&["spec", "template", "spec"]), Field::Absent)
    };
    if matches!(schedule, Field::Refused) {
        tally.k8s_refused += 1;
    }
    let Some(pod) = pod else {
        return;
    };
    let workload_id = infra_id(repo, &kind_lower, &name);
    for (init, key) in [(false, "containers"), (true, "initContainers")] {
        let items = match pod.get(key) {
            Some(Yv::List(items)) => items,
            None | Some(Yv::Null) => continue,
            Some(_) => {
                tally.k8s_refused += 1;
                continue;
            }
        };
        for (item_row, c) in items {
            if !matches!(c, Yv::Map(_)) {
                tally.k8s_refused += 1;
                continue;
            }
            tally.containers += 1;
            let cname = c.get("name").and_then(Yv::str).filter(|n| !is_template(n));
            let image = read_scalar(c.get("image"));
            let command = read_k8s_list(c.get("command"));
            let args = read_k8s_list(c.get("args"));
            let workdir = read_scalar(c.get("workingDir"));
            let mut refused: Vec<&'static str> = Vec::new();
            for (field, is_refused) in [
                ("command", matches!(command, Field::Refused)),
                ("args", matches!(args, Field::Refused)),
                ("workingDir", matches!(workdir, Field::Refused)),
                ("image", matches!(image, Field::Refused)),
            ] {
                if is_refused {
                    refused.push(field);
                }
            }
            tally.k8s_refused += refused.len();
            tally.k8s_commands += usize::from(command.read().is_some());
            tally.k8s_args += usize::from(args.read().is_some());
            if !(command.present() || args.present() || workdir.present()) {
                continue;
            }
            emit_resource(out, &kind_lower, &name, module_id, repo);
            let decl_row = ["command", "args", "workingDir"]
                .iter()
                .find_map(|k| c.entry(k).map(|(r, _)| r))
                .unwrap_or(*item_row);
            let image_base = image.read().map(|i| image_basename(i));
            let cell = json_cell(&K8sStartCell {
                start: K8sStart {
                    host: "k8s",
                    workload: &kind_lower,
                    container: cname,
                    init,
                    trigger: if cron { "schedule" } else { "container_start" },
                    schedule: schedule.read().map(String::as_str),
                    image: image_base.as_deref(),
                    command: command.read().map(Vec::as_slice),
                    args: args.read().map(Vec::as_slice),
                    workdir: workdir.read().map(String::as_str),
                    refused,
                    decl: format!("{path}:{}", decl_row + 1),
                },
            });
            attach_cell(out, workload_id, cell);
        }
    }
}

/// The skaffold builders, in the order a config is read for one; `docker` is
/// the default when an artifact names none.
const SKAFFOLD_BUILDERS: &[&str] = &[
    "docker",
    "kaniko",
    "jib",
    "ko",
    "buildpacks",
    "bazel",
    "custom",
];

/// Every `build.artifacts[]` of a skaffold config: the declared image node
/// gains a `build` cell naming its context and Dockerfile.
fn skaffold_artifacts(
    root: &Yv,
    path: &str,
    dir: &str,
    module_id: NodeId,
    repo: RepoId,
    out: &mut IacNodes,
    tally: &mut OrchestratorTally,
) {
    tally.skaffold_docs += 1;
    let Some(Yv::List(items)) = root.path(&["build", "artifacts"]) else {
        return;
    };
    for (row, a) in items {
        let Field::Read(image) = read_scalar(a.get("image")) else {
            tally.skaffold_refused += 1;
            continue;
        };
        let context = match a.get("context") {
            None | Some(Yv::Null) => Some(dir.to_string()),
            Some(Yv::Str(c)) => declared_path(dir, c),
            Some(_) => None,
        };
        let Some(context) = context else {
            tally.skaffold_refused += 1;
            continue;
        };
        let builder = SKAFFOLD_BUILDERS
            .iter()
            .copied()
            .find(|b| a.get(b).is_some())
            .unwrap_or("docker");
        let dockerfile = if matches!(builder, "docker" | "kaniko") {
            let named = a.path(&[builder, "dockerfile"]).and_then(Yv::str);
            match dockerfile_under(&context, named) {
                Some(d) => Some(d),
                None => {
                    tally.skaffold_refused += 1;
                    continue;
                }
            }
        } else {
            None
        };
        let target = a
            .path(&[builder, "target"])
            .and_then(Yv::str)
            .map(str::to_string);
        let base = image_basename(&image);
        if base.is_empty() {
            tally.skaffold_refused += 1;
            continue;
        }
        tally.artifacts += 1;
        tally.dockerfiles += usize::from(dockerfile.is_some());
        let build = DeclaredBuild {
            context,
            dockerfile,
            target,
        };
        emit_resource(out, "image", &base, module_id, repo);
        let cell = build_cell("skaffold", &base, builder, &build, path, *row);
        attach_cell(out, infra_id(repo, "image", &base), cell);
    }
}

// ----------------------------------------------------------------------------
// A small block-YAML reader (GH.16). Enough of YAML 1.2 for orchestrator
// files: block maps and sequences (a sequence may sit at its key's indent),
// flow sequences and maps over several lines, plain / single / double-quoted
// scalars, `|` / `>` block scalars, comments, `---` documents, anchors,
// aliases and `<<:` merge keys. Tags are dropped. A line it cannot read (a
// Helm `{{- if .. }}` directive) is skipped; a value it cannot read is
// `Yv::Unread`, which a reader counts as refused.
// ----------------------------------------------------------------------------

/// A YAML value. Rows are 0-based source rows.
#[derive(Debug, Clone, PartialEq)]
enum Yv {
    Null,
    Str(String),
    List(Vec<(usize, Yv)>),
    Map(Vec<(String, usize, Yv)>),
    Unread,
}

impl Yv {
    fn get(&self, key: &str) -> Option<&Yv> {
        self.entry(key).map(|(_, v)| v)
    }

    /// The entry's row and value.
    fn entry(&self, key: &str) -> Option<(usize, &Yv)> {
        match self {
            Yv::Map(entries) => entries
                .iter()
                .find(|(k, _, _)| k == key)
                .map(|(_, r, v)| (*r, v)),
            _ => None,
        }
    }

    fn path(&self, keys: &[&str]) -> Option<&Yv> {
        keys.iter().try_fold(self, |v, k| v.get(k))
    }

    fn str(&self) -> Option<&str> {
        match self {
            Yv::Str(s) => Some(s),
            _ => None,
        }
    }
}

/// One non-blank, non-comment line: `text` with its comment stripped, `raw`
/// as written (a block scalar's content), both without the indent.
#[derive(Debug, Clone)]
struct YLine {
    row: usize,
    indent: usize,
    text: String,
    raw: String,
}

/// The documents of a YAML stream, split at `---` / `...` lines.
fn yaml_docs(source: &str) -> Vec<Vec<YLine>> {
    let mut docs: Vec<Vec<YLine>> = vec![Vec::new()];
    for (row, line) in source.lines().enumerate() {
        let t = line.trim_end();
        if t == "---" || t.starts_with("--- ") || t == "..." {
            docs.push(Vec::new());
            continue;
        }
        let body = t.trim_start();
        if body.is_empty() || (body.starts_with('%') && t.len() == body.len()) {
            continue;
        }
        let text = strip_yaml_comment(body);
        if text.is_empty() {
            continue;
        }
        if let Some(doc) = docs.last_mut() {
            doc.push(YLine {
                row,
                indent: t.len() - body.len(),
                text: text.to_string(),
                raw: body.to_string(),
            });
        }
    }
    docs.retain(|d| !d.is_empty());
    docs
}

/// `s` up to a `#` comment (a `#` at the start or after whitespace, outside
/// quotes). A quote opens only at the start of a token, so `don't` stays
/// plain.
fn strip_yaml_comment(s: &str) -> &str {
    let b = s.as_bytes();
    let mut quote: Option<u8> = None;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match quote {
            Some(b'\'') => {
                if c == b'\'' {
                    if b.get(i + 1) == Some(&b'\'') {
                        i += 1;
                    } else {
                        quote = None;
                    }
                }
            }
            Some(_) => {
                if c == b'\\' {
                    i += 1;
                } else if c == b'"' {
                    quote = None;
                }
            }
            None => {
                let at_token = i == 0 || matches!(b[i - 1], b' ' | b'\t' | b'[' | b'{' | b',');
                if c == b'#' && (i == 0 || b[i - 1].is_ascii_whitespace()) {
                    return s[..i].trim_end();
                }
                if (c == b'"' || c == b'\'') && at_token {
                    quote = Some(c);
                }
            }
        }
        i += 1;
    }
    s
}

fn is_dash(t: &str) -> bool {
    t == "-" || t.starts_with("- ") || t.starts_with("-\t")
}

/// A block-map line's key and value text: `key: value`, `"key": value`.
/// `None` for a line that is no map entry.
fn split_key(text: &str) -> Option<(String, &str)> {
    if text.starts_with('{') || text.starts_with('[') {
        return None;
    }
    let colon_at = |rest: &str, from: usize| -> Option<usize> {
        let b = rest.as_bytes();
        (from..b.len())
            .find(|&i| b[i] == b':' && (i + 1 == b.len() || b[i + 1] == b' ' || b[i + 1] == b'\t'))
    };
    if let Some(q) = text.chars().next().filter(|c| *c == '"' || *c == '\'') {
        let close = text[1..].find(q)? + 1;
        let after = text[close + 1..].trim_start();
        let value = after.strip_prefix(':')?;
        if !(value.is_empty() || value.starts_with([' ', '\t'])) {
            return None;
        }
        return Some((text[1..close].to_string(), value.trim()));
    }
    let at = colon_at(text, 0)?;
    let key = text[..at].trim_end();
    if key.is_empty() {
        return None;
    }
    Some((key.to_string(), text[at + 1..].trim()))
}

/// The deepest block nesting read; deeper content is `Unread`.
const MAX_YAML_DEPTH: usize = 64;

struct YamlParser {
    lines: Vec<YLine>,
    i: usize,
    depth: usize,
    /// Anchors in definition order; an alias takes the latest of its name.
    anchors: Vec<(String, Yv)>,
}

/// One document's value (`Null` for an empty one).
fn parse_yaml_doc(lines: Vec<YLine>) -> Yv {
    let Some(indent) = lines.first().map(|l| l.indent) else {
        return Yv::Null;
    };
    let mut p = YamlParser {
        lines,
        i: 0,
        depth: 0,
        anchors: Vec::new(),
    };
    p.node(indent)
}

impl YamlParser {
    fn line(&self) -> Option<&YLine> {
        self.lines.get(self.i)
    }

    /// Skip lines nested deeper than `indent` that no reader consumed.
    fn skip_deeper(&mut self, indent: usize) {
        while self.line().is_some_and(|l| l.indent > indent) {
            self.i += 1;
        }
    }

    /// The node starting at the current line, whose indent is `indent`.
    /// Always consumes at least one line.
    fn node(&mut self, indent: usize) -> Yv {
        let Some(line) = self.line() else {
            return Yv::Null;
        };
        if self.depth >= MAX_YAML_DEPTH {
            self.i += 1;
            self.skip_deeper(indent);
            return Yv::Unread;
        }
        let text = line.text.clone();
        self.depth += 1;
        let v = if is_dash(&text) {
            self.seq(indent)
        } else if split_key(&text).is_some() {
            self.map(indent)
        } else {
            self.inline(&text, indent)
        };
        self.depth -= 1;
        v
    }

    /// The value nested under the line just consumed at `indent`.
    fn child(&mut self, indent: usize, same_indent_seq: bool) -> Yv {
        match self.line() {
            Some(l) if l.indent > indent => {
                let ind = l.indent;
                self.node(ind)
            }
            Some(l) if same_indent_seq && l.indent == indent && is_dash(&l.text) => {
                self.seq(indent)
            }
            _ => Yv::Null,
        }
    }

    fn seq(&mut self, indent: usize) -> Yv {
        let mut items = Vec::new();
        while let Some(line) = self.line() {
            if line.indent != indent || !is_dash(&line.text) {
                break;
            }
            let row = line.row;
            let rest = line.text[1..].trim_start().to_string();
            if rest.is_empty() {
                self.i += 1;
                items.push((row, self.child(indent, false)));
            } else {
                let off = line.text.len() - rest.len();
                let raw_rest = line.raw.get(1..).unwrap_or("").trim_start().to_string();
                let l = &mut self.lines[self.i];
                l.indent = indent + off;
                l.text = rest;
                l.raw = raw_rest;
                items.push((row, self.node(indent + off)));
            }
            self.skip_deeper(indent);
        }
        Yv::List(items)
    }

    fn map(&mut self, indent: usize) -> Yv {
        let mut entries: Vec<(String, usize, Yv)> = Vec::new();
        let mut merges: Vec<Yv> = Vec::new();
        while let Some(line) = self.line() {
            if line.indent != indent || is_dash(&line.text) {
                break;
            }
            let row = line.row;
            let text = line.text.clone();
            let Some((key, value)) = split_key(&text) else {
                self.i += 1;
                self.skip_deeper(indent);
                continue;
            };
            let v = self.value(value, indent);
            if key == "<<" {
                merges.push(v);
            } else if !entries.iter().any(|(k, _, _)| *k == key) {
                entries.push((key, row, v));
            }
            self.skip_deeper(indent);
        }
        for m in merges {
            let sources = match m {
                Yv::List(items) => items.into_iter().map(|(_, v)| v).collect(),
                other => vec![other],
            };
            for src in sources {
                let Yv::Map(src) = src else {
                    continue;
                };
                for (k, r, v) in src {
                    if !entries.iter().any(|(e, _, _)| *e == k) {
                        entries.push((k, r, v));
                    }
                }
            }
        }
        Yv::Map(entries)
    }

    /// A map entry's value whose text on the key's line is `value`.
    fn value(&mut self, value: &str, indent: usize) -> Yv {
        let (anchor, rest) = take_anchor(value);
        let rest = strip_tag(rest);
        let v = if rest.is_empty() {
            self.i += 1;
            self.child(indent, true)
        } else {
            self.inline(rest, indent)
        };
        if let Some(a) = anchor {
            self.anchors.push((a.to_string(), v.clone()));
        }
        v
    }

    /// A value written on the current line (`text`), with any continuation
    /// lines nested deeper than `indent`.
    fn inline(&mut self, text: &str, indent: usize) -> Yv {
        let row = self.line().map_or(0, |l| l.row);
        self.i += 1;
        let (anchor, text) = take_anchor(text);
        let text = strip_tag(text);
        let v = if let Some(name) = text.strip_prefix('*') {
            let name = name.trim();
            self.anchors
                .iter()
                .rev()
                .find(|(a, _)| a == name)
                .map_or(Yv::Unread, |(_, v)| v.clone())
        } else if is_block_scalar_header(text) {
            let mut parts: Vec<String> = Vec::new();
            while let Some(l) = self.line() {
                if l.indent <= indent {
                    break;
                }
                parts.push(l.raw.clone());
                self.i += 1;
            }
            let sep = if text.starts_with('>') { " " } else { "\n" };
            Yv::Str(parts.join(sep))
        } else if text.starts_with('[') || text.starts_with('{') {
            let mut s = text.to_string();
            while !flow_closed(&s) {
                match self.line() {
                    Some(l) if l.indent > indent => {
                        s.push(' ');
                        s.push_str(&l.text);
                        self.i += 1;
                    }
                    _ => break,
                }
            }
            parse_flow(&s, row).unwrap_or(Yv::Unread)
        } else if text.starts_with('"') || text.starts_with('\'') {
            let mut s = text.to_string();
            while parse_quoted(&s).is_none() {
                match self.line() {
                    Some(l) if l.indent > indent => {
                        s.push(' ');
                        s.push_str(&l.raw);
                        self.i += 1;
                    }
                    _ => break,
                }
            }
            match parse_quoted(&s) {
                Some((v, rest)) if rest.trim().is_empty() => Yv::Str(v),
                _ => Yv::Unread,
            }
        } else {
            let mut s = text.to_string();
            while let Some(l) = self.line() {
                if l.indent <= indent {
                    break;
                }
                s.push(' ');
                s.push_str(&l.text);
                self.i += 1;
            }
            plain_scalar(&s)
        };
        if let Some(a) = anchor {
            self.anchors.push((a.to_string(), v.clone()));
        }
        v
    }
}

/// `&name rest` -> (`name`, `rest`).
fn take_anchor(value: &str) -> (Option<&str>, &str) {
    match value.strip_prefix('&') {
        Some(rest) => {
            let end = rest.find([' ', '\t']).unwrap_or(rest.len());
            (Some(&rest[..end]), rest[end..].trim_start())
        }
        None => (None, value),
    }
}

/// `!!str x` / `!Ref x` -> `x`.
fn strip_tag(value: &str) -> &str {
    if value.starts_with('!') {
        let end = value.find([' ', '\t']).unwrap_or(value.len());
        value[end..].trim_start()
    } else {
        value
    }
}

fn is_block_scalar_header(text: &str) -> bool {
    let mut chars = text.chars();
    matches!(chars.next(), Some('|' | '>'))
        && chars.all(|c| c == '-' || c == '+' || c.is_ascii_digit())
}

fn plain_scalar(s: &str) -> Yv {
    let s = s.trim();
    if s.is_empty() || s == "~" || s == "null" || s == "Null" || s == "NULL" {
        Yv::Null
    } else {
        Yv::Str(s.to_string())
    }
}

/// Whether every `[` / `{` outside quotes is closed.
fn flow_closed(s: &str) -> bool {
    let mut depth: i64 = 0;
    let mut quote: Option<char> = None;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match quote {
            Some('"') if c == '\\' => {
                chars.next();
            }
            Some(q) if c == q => {
                if q == '\'' && chars.peek() == Some(&'\'') {
                    chars.next();
                } else {
                    quote = None;
                }
            }
            Some(_) => {}
            None => match c {
                '"' | '\'' => quote = Some(c),
                '[' | '{' => depth += 1,
                ']' | '}' => depth -= 1,
                _ => {}
            },
        }
    }
    depth <= 0 && quote.is_none()
}

/// A quoted scalar at the start of `s`: its value and the text after it.
fn parse_quoted(s: &str) -> Option<(String, &str)> {
    let q = s.chars().next()?;
    let mut out = String::new();
    let mut iter = s.char_indices().skip(1).peekable();
    while let Some((i, c)) = iter.next() {
        if c == q {
            if q == '\'' && iter.peek().map(|(_, n)| *n) == Some('\'') {
                iter.next();
                out.push('\'');
                continue;
            }
            return Some((out, &s[i + c.len_utf8()..]));
        }
        if q == '"' && c == '\\' {
            let (_, n) = iter.next()?;
            out.push(match n {
                'n' => '\n',
                't' => '\t',
                '0' => '\0',
                other => other,
            });
            continue;
        }
        out.push(c);
    }
    None
}

/// A flow collection (`[a, "b", {c: d}]`), every item at `row`; `None` when
/// it does not parse whole.
fn parse_flow(s: &str, row: usize) -> Option<Yv> {
    let mut f = Flow {
        s,
        pos: 0,
        row,
        depth: 0,
    };
    let v = f.value(&[])?;
    f.ws();
    (f.pos == s.len()).then_some(v)
}

struct Flow<'a> {
    s: &'a str,
    pos: usize,
    row: usize,
    /// Open collections; past [`MAX_YAML_DEPTH`] the value does not parse.
    depth: usize,
}

impl Flow<'_> {
    fn ws(&mut self) {
        let rest = &self.s[self.pos..];
        self.pos += rest.len() - rest.trim_start().len();
    }

    fn peek(&self) -> Option<char> {
        self.s[self.pos..].chars().next()
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.pos += c.len_utf8();
            true
        } else {
            false
        }
    }

    /// A value ending before any of `stops` (a plain scalar's terminators
    /// beside `,` `]` `}`).
    fn value(&mut self, stops: &[char]) -> Option<Yv> {
        self.ws();
        match self.peek()? {
            '[' | '{' if self.depth >= MAX_YAML_DEPTH => None,
            '[' => {
                self.depth += 1;
                let v = self.list();
                self.depth -= 1;
                v
            }
            '{' => {
                self.depth += 1;
                let v = self.map();
                self.depth -= 1;
                v
            }
            '"' | '\'' => {
                let (v, rest) = parse_quoted(&self.s[self.pos..])?;
                self.pos = self.s.len() - rest.len();
                Some(Yv::Str(v))
            }
            _ => {
                let rest = &self.s[self.pos..];
                let end = rest
                    .find(|c: char| c == ',' || c == ']' || c == '}' || stops.contains(&c))
                    .unwrap_or(rest.len());
                self.pos += end;
                Some(plain_scalar(&rest[..end]))
            }
        }
    }

    fn list(&mut self) -> Option<Yv> {
        self.eat('[');
        let mut items = Vec::new();
        loop {
            self.ws();
            if self.eat(']') {
                return Some(Yv::List(items));
            }
            items.push((self.row, self.value(&[])?));
            self.ws();
            if self.eat(',') {
                continue;
            }
            self.ws();
            return self.eat(']').then_some(Yv::List(items));
        }
    }

    fn map(&mut self) -> Option<Yv> {
        self.eat('{');
        let mut entries = Vec::new();
        loop {
            self.ws();
            if self.eat('}') {
                return Some(Yv::Map(entries));
            }
            let key = match self.value(&[':'])? {
                Yv::Str(k) => k,
                _ => return None,
            };
            self.ws();
            let v = if self.eat(':') {
                self.value(&[])?
            } else {
                Yv::Null
            };
            entries.push((key, self.row, v));
            self.ws();
            if self.eat(',') {
                continue;
            }
            self.ws();
            return self.eat('}').then_some(Yv::Map(entries));
        }
    }
}

// ----------------------------------------------------------------------------
// GH.17: process managers (Procfile, systemd units, supervisord programs)
// ----------------------------------------------------------------------------

/// GH.17: the process-manager file kinds the walk admits and the route
/// branches read. Each is a synthetic MODULE DEFINES-ing one INFRA_RESOURCE
/// per process it declares, with a CODE start cell naming what it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessManager {
    /// `Procfile` / `Procfile.<x>` (Heroku, Foreman, honcho, Overmind).
    Procfile,
    /// A systemd `*.service` unit.
    Systemd,
    /// A supervisord config: `supervisord.conf` / `supervisor.conf`, or a
    /// `*.conf` under a `supervisor/` or `supervisord.d/` directory.
    Supervisor,
}

impl ProcessManager {
    /// The kind `path` names, by its path alone (the walk's admission and the
    /// route's choice of branch). The route branch then gates on the text:
    /// a Procfile needs a `<type>: <command>` line, a unit a `[Service]`
    /// section, a supervisor conf a `[program:<name>]` section.
    pub fn of_path(path: &str) -> Option<ProcessManager> {
        if is_procfile_path(path) {
            Some(ProcessManager::Procfile)
        } else if is_systemd_unit_path(path) {
            Some(ProcessManager::Systemd)
        } else if is_supervisor_conf_path(path) {
            Some(ProcessManager::Supervisor)
        } else {
            None
        }
    }

    /// The route branch's lang key, and the start cell's `host`.
    pub fn key(self) -> &'static str {
        match self {
            ProcessManager::Procfile => "procfile",
            ProcessManager::Systemd => "systemd",
            ProcessManager::Supervisor => "supervisor",
        }
    }
}

/// `Procfile`, or `Procfile.<variant>` (`Procfile.dev`): the name is
/// case-sensitive, as Heroku and Foreman read it.
pub fn is_procfile_path(path: &str) -> bool {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    base == "Procfile"
        || base
            .strip_prefix("Procfile.")
            .is_some_and(|v| !v.is_empty() && !v.contains('.'))
}

/// A `*.service` file. Whether it is a systemd unit is the route branch's
/// `[Service]` gate (an unrelated `.service` file mints nothing).
pub fn is_systemd_unit_path(path: &str) -> bool {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    base.len() > ".service".len() && base.ends_with(".service")
}

/// `supervisord.conf` / `supervisor.conf`, or any `*.conf` with an ancestor
/// directory named `supervisor` or `supervisord.d` (`deploy/supervisor/
/// conf.d/app.conf`). A HOCON conf (`application.conf`, `resources/*.conf`)
/// is never one.
pub fn is_supervisor_conf_path(path: &str) -> bool {
    let norm = path.replace('\\', "/");
    let mut parts: Vec<&str> = norm.split('/').collect();
    let Some(base) = parts.pop() else {
        return false;
    };
    if crate::cron::is_hocon_conf_path(&norm) {
        return false;
    }
    if base == "supervisord.conf" || base == "supervisor.conf" {
        return true;
    }
    base.len() > ".conf".len()
        && base.ends_with(".conf")
        && parts
            .iter()
            .any(|d| *d == "supervisor" || *d == "supervisord.d")
}

/// GH.17: what one process-manager file held, summed per repo by the engine
/// into the `[proc-start]` marker.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ProcCounts {
    /// Procfiles that declared a process type.
    pub procfile: usize,
    /// Process types those Procfiles declared.
    pub types: usize,
    /// `.service` files with a `[Service]` section.
    pub systemd: usize,
    /// Units minted (one per such file).
    pub units: usize,
    /// Supervisor confs that declared a program.
    pub supervisor: usize,
    /// `[program:<name>]` sections with a `command`.
    pub programs: usize,
    /// Files the path admitted and the text gate refused (a `.service` with
    /// no `[Service]`, a conf with no program, a Procfile with no type).
    pub gated_out: usize,
    /// Starts by the target form `normalise_argv` read
    /// (`TargetForm::as_str`).
    pub forms: std::collections::BTreeMap<&'static str, usize>,
    /// Starts `normalise_argv` refused, by reason (`StartTarget::refusal`).
    pub refused: std::collections::BTreeMap<&'static str, usize>,
}

impl ProcCounts {
    /// Add one file's counts.
    pub fn add(&mut self, o: &ProcCounts) {
        self.procfile += o.procfile;
        self.types += o.types;
        self.systemd += o.systemd;
        self.units += o.units;
        self.supervisor += o.supervisor;
        self.programs += o.programs;
        self.gated_out += o.gated_out;
        for (k, n) in &o.forms {
            *self.forms.entry(k).or_default() += n;
        }
        for (k, n) in &o.refused {
            *self.refused.entry(k).or_default() += n;
        }
    }

    /// The per-repo fired_on line, `None` when the repo held no
    /// process-manager file: `[proc-start] procfile=<f> types=<t>
    /// systemd=<s> units=<u> supervisor=<v> programs=<p> gated_out=<g>
    /// repo=<label>`, then ` forms=(<form>=<n> ..)` when a start was read and
    /// ` refused=<reason>:<n>,..` when one was refused.
    pub fn marker(&self, repo_label: &str) -> Option<String> {
        if self.procfile + self.systemd + self.supervisor + self.gated_out == 0 {
            return None;
        }
        let mut line = format!(
            "[proc-start] procfile={} types={} systemd={} units={} supervisor={} programs={} gated_out={} repo={repo_label}",
            self.procfile,
            self.types,
            self.systemd,
            self.units,
            self.supervisor,
            self.programs,
            self.gated_out,
        );
        if !self.forms.is_empty() {
            let forms: Vec<String> = self.forms.iter().map(|(f, n)| format!("{f}={n}")).collect();
            line.push_str(&format!(" forms=({})", forms.join(" ")));
        }
        if !self.refused.is_empty() {
            let reasons: Vec<String> = self
                .refused
                .iter()
                .map(|(r, n)| format!("{r}:{n}"))
                .collect();
            line.push_str(&format!(" refused={}", reasons.join(",")));
        }
        Some(line)
    }

    fn read_argv(&mut self, argv: &[String]) {
        let target = glia_code_domain::hosts::normalise_argv(argv);
        match target.refusal() {
            Some(reason) => *self.refused.entry(reason).or_default() += 1,
            None => *self.forms.entry(target.form.as_str()).or_default() += 1,
        }
    }
}

/// One process-manager file's nodes and counts.
pub struct ProcNodes {
    pub iac: IacNodes,
    pub counts: ProcCounts,
}

/// The start cell's JSON shape. Field order is the serialised order.
#[derive(serde::Serialize)]
struct ProcStartCell<'a> {
    start: ProcStart<'a>,
}

#[derive(serde::Serialize)]
struct ProcStart<'a> {
    host: &'static str,
    /// The Procfile type, the unit stem or the program name.
    name: &'a str,
    trigger: &'static str,
    /// What the file wrote (the Procfile command, the `ExecStart=` value
    /// without its prefixes, supervisor's `command=`).
    command: &'a str,
    /// The effective argv: `["/bin/sh", "-c", command]` for a Procfile (a
    /// shell runs it), the command split with shell-word rules for a systemd
    /// unit and a supervisor program (neither runs a shell).
    argv: &'a [String],
    /// `WorkingDirectory=` / `directory=` as written, `null` when unset.
    workdir: Option<&'a str>,
    /// The declaring file's directory, repo-relative (`.` at the root): a
    /// Procfile's processes run there.
    dir: &'a str,
    decl: String,
}

fn proc_nodes() -> ProcNodes {
    ProcNodes {
        iac: IacNodes {
            nodes: Vec::new(),
            edges: Vec::new(),
            nav: CodeNav::default(),
            markers: Vec::new(),
        },
        counts: ProcCounts::default(),
    }
}

/// One start: the resource `infra:<kind>:<name>` (DEFINES from the file's
/// MODULE) and its start cell.
fn push_proc_start(
    out: &mut ProcNodes,
    kind: &str,
    resource: &str,
    start: ProcStart<'_>,
    module_id: NodeId,
    repo: RepoId,
) {
    emit_resource(&mut out.iac, kind, resource, module_id, repo);
    out.counts.read_argv(start.argv);
    let id = infra_id(repo, kind, resource);
    attach_cell(&mut out.iac, id, json_cell(&ProcStartCell { start }));
}

/// GH.17: a Procfile. Each `<type>: <command>` line (Heroku's
/// `^[A-Za-z0-9_-]+:\s*(.+)$`; `#` comments) is one INFRA_RESOURCE
/// `infra:process:<type>@<dir>` (the Procfile's repo-relative directory, `.`
/// at the root: `web` / `worker` repeat per sub-project) with a start cell
/// `{"start":{"host":"procfile","name":<type>,"trigger":"process_type",
/// "command","argv":["/bin/sh","-c",<command>],"workdir":null,"dir","decl"}}`.
/// A Procfile with no type line is gated out.
pub fn extract_procfile(source: &str, path: &str, module_id: NodeId, repo: RepoId) -> ProcNodes {
    let mut out = proc_nodes();
    let dir = dockerfile_context(path);
    for (row, line) in source.lines().enumerate() {
        let line = line.trim_end_matches('\r');
        if line.trim_start().starts_with('#') {
            continue;
        }
        let Some((ty, cmd)) = line.split_once(':') else {
            continue;
        };
        if ty.is_empty()
            || !ty
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            continue;
        }
        let cmd = cmd.trim();
        if cmd.is_empty() {
            continue;
        }
        let argv = vec!["/bin/sh".to_string(), "-c".to_string(), cmd.to_string()];
        let resource = format!("{ty}@{dir}");
        let start = ProcStart {
            host: "procfile",
            name: ty,
            trigger: "process_type",
            command: cmd,
            argv: &argv,
            workdir: None,
            dir: &dir,
            decl: format!("{path}:{}", row + 1),
        };
        push_proc_start(&mut out, "process", &resource, start, module_id, repo);
        out.counts.types += 1;
    }
    if out.counts.types > 0 {
        out.counts.procfile = 1;
    } else {
        out.counts.gated_out = 1;
    }
    out
}

/// systemd's command-line prefixes (`-` ignore failure, `@` argv[0] given
/// second, `:` no substitution, `+` / `!` / `!!` privileges), stripped; with
/// `@` the second word is the process's argv[0], not an argument, and is
/// dropped from the effective argv.
fn strip_exec_prefixes(value: &str) -> (&str, bool) {
    let rest = value.trim_start_matches(['-', '@', ':', '+', '!']);
    let prefix = &value[..value.len() - rest.len()];
    (rest.trim_start(), prefix.contains('@'))
}

/// GH.17: a systemd unit. Gated on a `[Service]` section; the unit is one
/// INFRA_RESOURCE `infra:unit:<file stem>` (`worker` of `worker.service`,
/// `app@` of a template `app@.service`). Each `ExecStart=` of `[Service]`
/// that an empty `ExecStart=` does not reset (a oneshot unit may list
/// several) is a start cell `{"start":{"host":"systemd","name":<stem>,
/// "trigger":"boot","command","argv","workdir","dir","decl"}}`, its
/// prefixes stripped and its value split with shell-word rules (systemd runs
/// no shell); `WorkingDirectory=` (its `-` dropped) is the workdir.
/// `ExecStartPre=` / `ExecReload=` and the other `Exec*` lines are no start.
/// Lines continue with a trailing `\`; `#` / `;` lines are comments.
pub fn extract_systemd_unit(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
) -> ProcNodes {
    let mut out = proc_nodes();
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let stem = base.strip_suffix(".service").unwrap_or(base);
    let mut in_service = false;
    let mut has_service = false;
    let mut workdir: Option<String> = None;
    let mut execs: Vec<(usize, String)> = Vec::new();
    for (row, line) in logical_lines(source) {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') || t.starts_with(';') {
            continue;
        }
        if t.starts_with('[') && t.ends_with(']') {
            in_service = t == "[Service]";
            has_service |= in_service;
            continue;
        }
        if !in_service {
            continue;
        }
        let Some((key, value)) = t.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "ExecStart" if value.is_empty() => execs.clear(),
            "ExecStart" => execs.push((row, value.to_string())),
            "WorkingDirectory" => {
                let w = value.strip_prefix('-').unwrap_or(value);
                workdir = (!w.is_empty()).then(|| w.to_string());
            }
            _ => {}
        }
    }
    if !has_service || stem.is_empty() {
        out.counts.gated_out = 1;
        return out;
    }
    out.counts.systemd = 1;
    out.counts.units = 1;
    emit_resource(&mut out.iac, "unit", stem, module_id, repo);
    let dir = dockerfile_context(path);
    for (row, value) in &execs {
        let (command, argv0_given) = strip_exec_prefixes(value);
        let mut argv = shell_words(command);
        if argv0_given && argv.len() > 1 {
            argv.remove(1);
        }
        if argv.is_empty() {
            continue;
        }
        let start = ProcStart {
            host: "systemd",
            name: stem,
            trigger: "boot",
            command,
            argv: &argv,
            workdir: workdir.as_deref(),
            dir: &dir,
            decl: format!("{path}:{}", row + 1),
        };
        push_proc_start(&mut out, "unit", stem, start, module_id, repo);
    }
    out
}

/// systemd's logical lines: a line ending `\` continues on the next (joined
/// with a space), each keyed by its first physical row (0-based).
fn logical_lines(source: &str) -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = Vec::new();
    let mut open: Option<(usize, String)> = None;
    for (row, raw) in source.lines().enumerate() {
        let raw = raw.trim_end_matches('\r');
        let (text, continues) = match raw.strip_suffix('\\') {
            Some(t) => (t, true),
            None => (raw, false),
        };
        let cur = match open.take() {
            Some((r, mut acc)) => {
                let t = text.trim_start();
                // A comment line inside a continuation is skipped, as systemd does.
                if !(t.starts_with('#') || t.starts_with(';')) {
                    acc.truncate(acc.trim_end().len());
                    acc.push(' ');
                    acc.push_str(t);
                }
                (r, acc)
            }
            None => (row, text.to_string()),
        };
        if continues {
            open = Some(cur);
        } else {
            out.push(cur);
        }
    }
    if let Some(cur) = open {
        out.push(cur);
    }
    out
}

/// supervisor's inline comments: a `;` or `#` preceded by whitespace ends the
/// value (configparser's `inline_comment_prefixes`, which supervisor sets).
fn strip_ini_inline_comment(v: &str) -> &str {
    let bytes = v.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if (*b == b';' || *b == b'#') && i > 0 && bytes[i - 1].is_ascii_whitespace() {
            return v[..i].trim_end();
        }
    }
    v.trim_end()
}

/// GH.17: a supervisord config. Each `[program:<name>]` section holding a
/// `command` is one INFRA_RESOURCE `infra:program:<name>` with a start cell
/// `{"start":{"host":"supervisor","name":<name>,"trigger":"supervised",
/// "command","argv","workdir","dir","decl"}}`; the command is split with
/// shell-word rules (supervisor runs it with no shell), `directory=` is the
/// workdir. Read as configparser does: `=` or `:` delimits, an indented line
/// continues the value above, `;` / `#` start a comment (inline when
/// whitespace precedes them). A conf with no such program is gated out.
pub fn extract_supervisor(source: &str, path: &str, module_id: NodeId, repo: RepoId) -> ProcNodes {
    struct Program {
        name: String,
        command: Option<(usize, String)>,
        directory: Option<String>,
    }
    let mut out = proc_nodes();
    let mut programs: Vec<Program> = Vec::new();
    let mut in_program = false;
    // The key the last value line belonged to, for a continuation line.
    let mut last_key: Option<String> = None;
    for (row, raw) in source.lines().enumerate() {
        let raw = raw.trim_end_matches('\r');
        let t = raw.trim();
        if t.is_empty() || t.starts_with(';') || t.starts_with('#') {
            continue;
        }
        if raw.starts_with([' ', '\t']) {
            // A continuation of the previous value.
            if let (true, Some(key), Some(p)) = (in_program, &last_key, programs.last_mut()) {
                let more = strip_ini_inline_comment(t);
                match key.as_str() {
                    "command" => {
                        if let Some((_, c)) = p.command.as_mut() {
                            c.push(' ');
                            c.push_str(more);
                        }
                    }
                    "directory" => {
                        if let Some(d) = p.directory.as_mut() {
                            d.push(' ');
                            d.push_str(more);
                        }
                    }
                    _ => {}
                }
            }
            continue;
        }
        last_key = None;
        if t.starts_with('[') && t.ends_with(']') {
            let header = &t[1..t.len() - 1];
            in_program = false;
            if let Some(name) = header.trim().strip_prefix("program:") {
                let name = name.trim();
                if !name.is_empty() {
                    in_program = true;
                    programs.push(Program {
                        name: name.to_string(),
                        command: None,
                        directory: None,
                    });
                }
            }
            continue;
        }
        if !in_program {
            continue;
        }
        let delim = t.find(['=', ':']);
        let Some(at) = delim else {
            continue;
        };
        let key = t[..at].trim().to_ascii_lowercase();
        let value = strip_ini_inline_comment(t[at + 1..].trim_start()).to_string();
        let Some(p) = programs.last_mut() else {
            continue;
        };
        match key.as_str() {
            "command" => p.command = Some((row, value)),
            "directory" => p.directory = Some(value),
            _ => {}
        }
        last_key = Some(key);
    }
    let dir = dockerfile_context(path);
    for p in &programs {
        let Some((row, command)) = &p.command else {
            continue;
        };
        let argv = shell_words(command);
        if argv.is_empty() {
            continue;
        }
        let start = ProcStart {
            host: "supervisor",
            name: &p.name,
            trigger: "supervised",
            command,
            argv: &argv,
            workdir: p.directory.as_deref().filter(|d| !d.is_empty()),
            dir: &dir,
            decl: format!("{path}:{}", row + 1),
        };
        push_proc_start(&mut out, "program", &p.name, start, module_id, repo);
        out.counts.programs += 1;
    }
    if out.counts.programs > 0 {
        out.counts.supervisor = 1;
    } else {
        out.counts.gated_out = 1;
    }
    out
}

/// GH.17: the one entry the engine's route branch calls.
pub fn extract_process_manager(
    kind: ProcessManager,
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
) -> ProcNodes {
    match kind {
        ProcessManager::Procfile => extract_procfile(source, path, module_id, repo),
        ProcessManager::Systemd => extract_systemd_unit(source, path, module_id, repo),
        ProcessManager::Supervisor => extract_supervisor(source, path, module_id, repo),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module_id(repo: RepoId) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MODULE, "test")
    }

    fn qnames(out: &IacNodes) -> Vec<String> {
        out.nav.qname_by_id.values().cloned().collect()
    }

    #[test]
    fn dockerfile_emits_image_from_parent_dir() {
        let repo = RepoId(1);
        let src = "FROM python:3.11-slim\nWORKDIR /app\nCOPY . .\nCMD [\"./run.sh\"]\n";
        let out = extract_dockerfile(src, "services/api/Dockerfile", module_id(repo), repo);
        let q = qnames(&out);
        assert!(q.contains(&"infra:image:api".to_string()));
        // FROM base also captured.
        assert!(q.contains(&"infra:image:python".to_string()));
        // REFERENCES edge from api → python.
        assert!(
            out.edges
                .iter()
                .any(|e| e.category == edge_category::INFRA_REFERENCES)
        );
    }

    #[test]
    fn dockerfile_handles_named_dockerfile_suffix() {
        let repo = RepoId(1);
        let src = "FROM node:20\n";
        let out = extract_dockerfile(src, "Dockerfile.worker", module_id(repo), repo);
        let q = qnames(&out);
        // No parent dir context — fall back to suffix-stripped basename.
        // Path has no `/` so segments.len() == 1 → fall through to suffix logic.
        assert!(q.iter().any(|s| s.starts_with("infra:image:")));
    }

    fn image_id(repo: RepoId, name: &str) -> NodeId {
        NodeId::from_parts(
            GRAPH_TYPE,
            repo,
            node_kind::INFRA_RESOURCE,
            &infra::qname("image", name),
        )
    }

    /// `(from, to)` image names of every INFRA_REFERENCES edge, in emit order.
    fn base_refs(out: &IacNodes) -> Vec<(String, String)> {
        out.edges
            .iter()
            .filter(|e| e.category == edge_category::INFRA_REFERENCES)
            .map(|e| {
                (
                    out.nav.qname_by_id[&e.from].clone(),
                    out.nav.qname_by_id[&e.to].clone(),
                )
            })
            .collect()
    }

    #[test]
    fn dockerfile_from_flags_skipped_and_every_stage_read() {
        let repo = RepoId(1);
        let src = "# syntax=docker/dockerfile:1\n\
FROM --platform=$BUILDPLATFORM golang:1.22-alpine@sha256:abc AS builder\n\
RUN go build -o /bin/server ./cmd/server\n\
FROM gcr.io/distroless/static\n\
COPY --from=builder /bin/server /bin/server\n";
        let out = extract_dockerfile(src, "src/frontend/Dockerfile", module_id(repo), repo);
        let q = qnames(&out);
        assert!(
            !q.iter().any(|s| s.contains("--platform")),
            "a FROM flag is no image: {q:?}"
        );
        assert!(!q.contains(&"infra:image:builder".to_string()), "{q:?}");
        assert_eq!(
            base_refs(&out),
            vec![
                (
                    "infra:image:frontend".to_string(),
                    "infra:image:golang".to_string()
                ),
                (
                    "infra:image:frontend".to_string(),
                    "infra:image:static".to_string()
                ),
            ],
            "the builder's base and the final stage's base, in file order"
        );
    }

    #[test]
    fn dockerfile_stage_parent_is_not_an_image() {
        let repo = RepoId(1);
        let src = "FROM python:3.12-alpine AS base\n\
FROM base AS builder\n\
RUN pip install -r requirements.txt\n\
FROM Base\n";
        let out = extract_dockerfile(src, "src/emailservice/Dockerfile", module_id(repo), repo);
        let q = qnames(&out);
        assert!(!q.contains(&"infra:image:base".to_string()), "{q:?}");
        assert!(!q.contains(&"infra:image:Base".to_string()), "{q:?}");
        assert_eq!(
            base_refs(&out),
            vec![(
                "infra:image:emailservice".to_string(),
                "infra:image:python".to_string()
            )]
        );
        let lines = dockerfile_stages(src);
        assert_eq!(lines.stages.len(), 3);
        assert_eq!(lines.stages[1].name.as_deref(), Some("builder"));
        assert_eq!(lines.stages[2].base, StageBase::Stage(0));
    }

    #[test]
    fn dockerfile_image_name_walks_up_past_generic_dirs() {
        assert_eq!(
            dockerfile_image_name("src/cartservice/src/Dockerfile").name,
            "cartservice"
        );
        assert_eq!(
            dockerfile_image_name("src/cartservice/src/Dockerfile.debug").name,
            "cartservice"
        );
        assert_eq!(
            dockerfile_image_name("services/api/docker/Dockerfile").name,
            "api"
        );
        assert_eq!(dockerfile_image_name("services/api/Dockerfile").name, "api");
        // Every directory generic: the file name's suffix, else the degenerate name.
        assert_eq!(
            dockerfile_image_name("docker/Dockerfile.worker").name,
            "worker"
        );
        assert_eq!(dockerfile_image_name("build/api.dockerfile").name, "api");
        assert_eq!(dockerfile_image_name("app/Dockerfile").name, "image");
        assert_eq!(dockerfile_image_name("Dockerfile").name, "image");
        assert_eq!(
            dockerfile_image_name("src/cartservice/src/Dockerfile").source,
            ImageNameSource::WalkedUp
        );
        assert_eq!(
            dockerfile_image_name("services/api/Dockerfile").source,
            ImageNameSource::Dir
        );
        assert_eq!(
            dockerfile_image_name("docker/Dockerfile.worker").source,
            ImageNameSource::FileName
        );

        let repo = RepoId(1);
        let out = extract_dockerfile(
            "FROM mcr.microsoft.com/dotnet/sdk:10.0 AS builder\nFROM mcr.microsoft.com/dotnet/runtime-deps:10.0\n",
            "src/cartservice/src/Dockerfile",
            module_id(repo),
            repo,
        );
        let q = qnames(&out);
        assert!(q.contains(&"infra:image:cartservice".to_string()), "{q:?}");
        assert!(!q.contains(&"infra:image:src".to_string()), "{q:?}");
        assert!(out.edges.iter().any(|e| e.from == module_id(repo)
            && e.to == image_id(repo, "cartservice")
            && e.category == edge_category::DEFINES));
    }

    #[test]
    fn dockerfile_from_args_scratch_heredoc_and_continuations() {
        let repo = RepoId(1);
        let src = "ARG BASE=node:20-slim\n\
ARG UNSET\n\
FROM ${BASE} AS deps\n\
RUN <<EOF\n\
FROM not-an-instruction\n\
EOF\n\
FROM $UNSET AS mystery\n\
FROM \\\n\
  --platform=linux/amd64 \\\n\
  # a comment inside the continuation\n\
  ghcr.io/acme/runtime:1 AS final\n\
FROM scratch\n\
COPY --from=final / /\n";
        let out = extract_dockerfile(src, "services/web/Dockerfile", module_id(repo), repo);
        let q = qnames(&out);
        for absent in [
            "not-an-instruction",
            "scratch",
            "$UNSET",
            "UNSET",
            "mystery",
            "deps",
        ] {
            assert!(
                !q.contains(&format!("infra:image:{absent}")),
                "{absent} must mint nothing: {q:?}"
            );
        }
        assert_eq!(
            base_refs(&out),
            vec![
                (
                    "infra:image:web".to_string(),
                    "infra:image:node".to_string()
                ),
                (
                    "infra:image:web".to_string(),
                    "infra:image:runtime".to_string()
                ),
            ]
        );
        let stages = dockerfile_stages(src);
        assert_eq!(stages.stages.len(), 4);
        assert_eq!(stages.flags_skipped, 1);
        assert_eq!(stages.stages[1].base, StageBase::Unresolved);
        assert_eq!(stages.stages[3].base, StageBase::Scratch);
        assert_eq!(stages.stages[2].line, 7);
    }

    #[test]
    fn dockerfile_escape_directive_backtick() {
        let repo = RepoId(1);
        let src = "# escape=`\nFROM `\n  mcr.microsoft.com/windows/servercore:ltsc2022\n";
        let out = extract_dockerfile(src, "win/Dockerfile", module_id(repo), repo);
        assert_eq!(
            base_refs(&out),
            vec![(
                "infra:image:win".to_string(),
                "infra:image:servercore".to_string()
            )]
        );
    }

    #[test]
    fn k8s_deployment_with_image_link() {
        let repo = RepoId(1);
        let src = r#"
apiVersion: apps/v1
kind: Deployment
metadata:
  name: api
spec:
  template:
    spec:
      containers:
      - name: api
        image: ghcr.io/example/api:v1.2
"#;
        let out = extract_yaml(src, "deploy.yaml", module_id(repo), repo);
        let q = qnames(&out);
        assert!(q.contains(&"infra:deployment:api".to_string()));
        assert!(q.contains(&"infra:image:api".to_string()));

        let dep_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo,
            node_kind::INFRA_RESOURCE,
            "infra:deployment:api",
        );
        let img_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo,
            node_kind::INFRA_RESOURCE,
            "infra:image:api",
        );
        assert!(out.edges.iter().any(|e| {
            e.from == dep_id && e.to == img_id && e.category == edge_category::INFRA_REFERENCES
        }));
    }

    #[test]
    fn k8s_multidoc_extracts_each() {
        let repo = RepoId(1);
        let src = r#"
apiVersion: v1
kind: Service
metadata:
  name: api-svc
spec:
  selector:
    app: api
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: api
spec:
  template:
    spec:
      containers:
      - image: example/api:latest
---
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: api-ing
"#;
        let out = extract_yaml(src, "deploy.yaml", module_id(repo), repo);
        let q = qnames(&out);
        assert!(q.contains(&"infra:service:api-svc".to_string()));
        assert!(q.contains(&"infra:deployment:api".to_string()));
        assert!(q.contains(&"infra:ingress:api-ing".to_string()));
        assert!(q.contains(&"infra:image:api".to_string()));
    }

    #[test]
    fn k8s_skips_unknown_kinds() {
        let repo = RepoId(1);
        let src = r#"
apiVersion: example.com/v1
kind: WeirdCustomResource
metadata:
  name: should-skip
"#;
        let out = extract_yaml(src, "deploy.yaml", module_id(repo), repo);
        assert!(out.nodes.is_empty(), "non-allowlisted kinds must not emit");
    }

    #[test]
    fn compose_services_with_image_links() {
        let repo = RepoId(1);
        let src = r#"
version: '3.8'
services:
  api:
    image: example/api:1.0
    ports:
      - "8080:8080"
  worker:
    image: example/worker:latest
    depends_on:
      - api
  redis:
    image: redis:7
"#;
        let out = extract_yaml(src, "deploy.yaml", module_id(repo), repo);
        let q = qnames(&out);
        assert!(q.contains(&"infra:service:api".to_string()));
        assert!(q.contains(&"infra:service:worker".to_string()));
        assert!(q.contains(&"infra:service:redis".to_string()));
        assert!(q.contains(&"infra:image:api".to_string()));
        assert!(q.contains(&"infra:image:worker".to_string()));
        assert!(q.contains(&"infra:image:redis".to_string()));

        // REFERENCES edge: service → image.
        let svc_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo,
            node_kind::INFRA_RESOURCE,
            "infra:service:api",
        );
        let img_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo,
            node_kind::INFRA_RESOURCE,
            "infra:image:api",
        );
        assert!(out.edges.iter().any(|e| {
            e.from == svc_id && e.to == img_id && e.category == edge_category::INFRA_REFERENCES
        }));
    }

    #[test]
    fn compose_skips_subkeys_as_service_names() {
        let repo = RepoId(1);
        let src = r#"
services:
  api:
    image: example/api:1.0
    environment:
      - FOO=bar
    ports:
      - "8080:8080"
"#;
        let out = extract_yaml(src, "deploy.yaml", module_id(repo), repo);
        let q = qnames(&out);
        // Must include `api` but not `environment` or `ports`.
        assert!(q.contains(&"infra:service:api".to_string()));
        assert!(!q.contains(&"infra:service:environment".to_string()));
        assert!(!q.contains(&"infra:service:ports".to_string()));
    }

    // ---- GH.15: container starts ----------------------------------------

    /// The image node's start cells, as JSON text.
    fn start_cells(out: &IacNodes, image: &str) -> Vec<String> {
        let id = image_id(RepoId(1), image);
        let Some(node) = out.nodes.iter().find(|n| n.id == id) else {
            return Vec::new();
        };
        node.cells
            .iter()
            .filter(|c| c.kind == glia_code_domain::cell_type::CODE)
            .filter_map(|c| match &c.payload {
                glia_core::CellPayload::Json(j) => Some(j.clone()),
                _ => None,
            })
            .collect()
    }

    fn invocations(out: &IacNodes) -> Vec<(String, Vec<Vec<String>>)> {
        let mut rows: Vec<(String, Vec<Vec<String>>)> = out
            .nodes
            .iter()
            .filter(|n| out.nav.kind_by_id.get(&n.id) == Some(&node_kind::CLI_INVOCATION))
            .map(|n| {
                let q = out.nav.qname_by_id.get(&n.id).cloned().unwrap_or_default();
                let argv = n
                    .cells
                    .iter()
                    .flat_map(crate::cli::parse_argv_cell)
                    .collect();
                (q, argv)
            })
            .collect();
        rows.sort();
        rows
    }

    fn docker(path: &str, src: &str) -> IacNodes {
        let repo = RepoId(1);
        extract_dockerfile(src, path, module_id(repo), repo)
    }

    const GO_MULTI: &str = "FROM golang:1.22 AS build\nWORKDIR /src\nCOPY . .\nRUN CGO_ENABLED=0 go build -o /bin/server ./cmd/server\n\nFROM gcr.io/distroless/static\nCOPY --from=build /bin/server /bin/server\nENTRYPOINT [\"/bin/server\"]\n";

    #[test]
    fn docker_start_go_multi_stage_marker_and_cell() {
        let out = docker("svc-go/Dockerfile", GO_MULTI);
        assert_eq!(
            out.markers,
            [
                "[docker-start] file=svc-go/Dockerfile stages=2 starts=1 (exec=1 shell=0) healthchecks=0 copies=1 builds=1 (go=1) binaries=0"
            ]
        );
        assert_eq!(
            start_cells(&out, "svc-go"),
            [
                r#"{"start":{"host":"docker","stage":"1","final":true,"trigger":"container_start","argv":["/bin/server"],"entrypoint":["/bin/server"],"cmd":[],"workdir":null,"context":"svc-go","copies":[],"builds":[{"tool":"go","out":"/bin/server","pkg":"./cmd/server","workdir":"/src","src":".","stage":"build"}],"decl":"svc-go/Dockerfile:8"}}"#
            ]
        );
        assert!(invocations(&out).is_empty());
    }

    #[test]
    fn docker_start_shell_form_continuation_and_healthcheck() {
        let src = "FROM python:3.12-slim\nWORKDIR /app\nCOPY . .\nHEALTHCHECK --interval=30s CMD curl -f http://localhost:8000/ || exit 1\nCMD gunicorn \\\n    --bind 0.0.0.0:8000 app:app\n";
        let out = docker("svc-py/Dockerfile", src);
        assert_eq!(
            out.markers,
            [
                "[docker-start] file=svc-py/Dockerfile stages=1 starts=1 (exec=0 shell=1) healthchecks=1 copies=1 builds=0 () binaries=0"
            ]
        );
        let cells = start_cells(&out, "svc-py");
        assert_eq!(cells.len(), 2, "{cells:?}");
        assert!(
            cells[0].contains(r#""trigger":"container_start","argv":["/bin/sh","-c","gunicorn  --bind 0.0.0.0:8000 app:app"],"entrypoint":[],"cmd":["/bin/sh","-c","gunicorn  --bind 0.0.0.0:8000 app:app"],"workdir":"/app","context":"svc-py","copies":[["/app","."]]"#),
            "{}",
            cells[0]
        );
        assert!(
            cells[0].ends_with(r#""decl":"svc-py/Dockerfile:5"}}"#),
            "{}",
            cells[0]
        );
        assert!(
            cells[1].contains(r#""trigger":"healthcheck","argv":["/bin/sh","-c","curl -f http://localhost:8000/ || exit 1"]"#),
            "{}",
            cells[1]
        );
        // A health probe is not the container's process: no `cli_invoke:curl`.
        assert!(invocations(&out).is_empty());
    }

    #[test]
    fn docker_start_binary_on_path_mints_an_invocation_through_tini() {
        let src = "FROM python:3.12-slim\nWORKDIR /app\nCOPY . .\nRUN pip install .\nENTRYPOINT [\"/sbin/tini\", \"--\"]\nCMD [\"mytool\", \"serve\", \"--port\", \"8080\"]\nHEALTHCHECK CMD [\"curl\", \"-f\", \"http://localhost/\"]\n";
        let out = docker("tool/Dockerfile", src);
        assert_eq!(
            invocations(&out),
            [(
                "cli_invoke:mytool".to_string(),
                vec![vec![
                    "serve".to_string(),
                    "--port".to_string(),
                    "8080".to_string()
                ]]
            )]
        );
        assert_eq!(
            out.markers,
            [
                "[docker-start] file=tool/Dockerfile stages=1 starts=1 (exec=1 shell=0) healthchecks=1 copies=1 builds=0 () binaries=1"
            ]
        );
        let module = module_id(RepoId(1));
        let inv = NodeId::from_parts(
            GRAPH_TYPE,
            RepoId(1),
            node_kind::CLI_INVOCATION,
            "cli_invoke:mytool",
        );
        assert!(
            out.edges
                .iter()
                .any(|e| e.from == module && e.to == inv && e.category == edge_category::DEFINES)
        );
        assert!(start_cells(&out, "tool")[0].contains(
            r#""argv":["/sbin/tini","--","mytool","serve","--port","8080"],"entrypoint":["/sbin/tini","--"],"cmd":["mytool","serve","--port","8080"]"#
        ));
    }

    #[test]
    fn docker_start_rust_bin_copied_to_path_is_built_not_invoked() {
        let src = "FROM rust:1.80 AS build\nWORKDIR /app\nCOPY . .\nRUN cargo build --release --bin api\n\nFROM debian:bookworm-slim\nCOPY --from=build /app/target/release/api /usr/local/bin/\nCMD [\"api\"]\n";
        let out = docker("svc-rust/Dockerfile", src);
        assert_eq!(
            out.markers,
            [
                "[docker-start] file=svc-rust/Dockerfile stages=2 starts=1 (exec=1 shell=0) healthchecks=0 copies=1 builds=1 (cargo=1) binaries=0"
            ]
        );
        let cells = start_cells(&out, "svc-rust");
        assert!(
            cells[0].contains(r#""builds":[{"tool":"cargo","out":"/usr/local/bin/api","bin":"api","workdir":"/app","src":".","stage":"build"}]"#),
            "{}",
            cells[0]
        );
        assert!(invocations(&out).is_empty());
    }

    #[test]
    fn docker_start_dotnet_gradle_maven_builds() {
        let dotnet = "FROM mcr.microsoft.com/dotnet/sdk:8.0 AS build\nWORKDIR /src\nCOPY . .\nRUN dotnet publish Cart.csproj -c Release -o /app\n\nFROM mcr.microsoft.com/dotnet/runtime:8.0\nWORKDIR /app\nCOPY --from=build /app .\nENTRYPOINT [\"dotnet\", \"Cart.dll\"]\n";
        let out = docker("cartsvc/Dockerfile", dotnet);
        assert!(
            out.markers[0].contains("builds=1 (dotnet=1) binaries=0"),
            "{:?}",
            out.markers
        );
        assert!(start_cells(&out, "cartsvc")[0].contains(
            r#""workdir":"/app","context":"cartsvc","copies":[],"builds":[{"tool":"dotnet","out":"/app","proj":"Cart.csproj","workdir":"/src","src":".","stage":"build"}]"#
        ));

        let gradle = "FROM eclipse-temurin:21 AS builder\nWORKDIR /app\nCOPY . .\nRUN ./gradlew installDist --no-daemon\n\nFROM eclipse-temurin:21-jre\nWORKDIR /app\nCOPY --from=builder /app .\nENTRYPOINT [\"/app/build/install/shop/bin/Ad\"]\n";
        let out = docker("adservice/Dockerfile", gradle);
        assert!(
            out.markers[0].contains("copies=1 builds=1 (gradle=1) binaries=0"),
            "{:?}",
            out.markers
        );
        assert!(start_cells(&out, "adservice")[0].contains(
            r#""copies":[["/app","."]],"builds":[{"tool":"gradle","out":"/app/build/install","task":"installDist","workdir":"/app","src":".","stage":"builder"}]"#
        ));
        assert!(invocations(&out).is_empty());

        let maven = "FROM maven:3 AS build\nWORKDIR /w\nCOPY . .\nRUN mvn -B -f server/pom.xml package -DskipTests\nFROM eclipse-temurin:21-jre\nCOPY --from=build /w/server/target/app.jar /app.jar\nENTRYPOINT [\"java\", \"-jar\", \"/app.jar\"]\n";
        let out = docker("svc/Dockerfile", maven);
        assert!(
            out.markers[0].contains("builds=1 (maven=1) binaries=0"),
            "{:?}",
            out.markers
        );
        assert!(start_cells(&out, "svc")[0].contains(
            r#""builds":[{"tool":"maven","out":"/app.jar","task":"package","workdir":"/w","src":".","stage":"build"}]"#
        ), "{:?}", start_cells(&out, "svc"));
    }

    #[test]
    fn docker_start_inherits_through_stages_and_entrypoint_resets_cmd() {
        let src = "FROM node:20 AS base\nWORKDIR /srv\nCMD [\"node\", \"a.js\"]\nFROM base AS dev\nFROM base AS prod\nENTRYPOINT [\"node\", \"b.js\"]\nONBUILD CMD [\"never\"]\n";
        let out = docker("web/Dockerfile", src);
        let cells = start_cells(&out, "web");
        assert_eq!(cells.len(), 3, "{cells:?}");
        assert!(cells[0].contains(
            r#""stage":"base","final":false,"trigger":"container_start","argv":["node","a.js"]"#
        ));
        // `dev` inherits base's CMD and WORKDIR, declared at base's row.
        assert!(cells[1].contains(r#""stage":"dev","final":false,"trigger":"container_start","argv":["node","a.js"],"entrypoint":[],"cmd":["node","a.js"],"workdir":"/srv""#));
        assert!(cells[1].ends_with(r#""decl":"web/Dockerfile:3"}}"#));
        // `prod` sets ENTRYPOINT: the inherited CMD is reset, as Docker does.
        assert!(cells[2].contains(r#""stage":"prod","final":true,"trigger":"container_start","argv":["node","b.js"],"entrypoint":["node","b.js"],"cmd":[]"#));
        assert!(out.markers[0].contains("stages=3 starts=3 (exec=3 shell=0)"));
    }

    #[test]
    fn docker_start_shell_entrypoint_ignores_cmd_and_refuses_dynamic() {
        let src = "FROM alpine\nENTRYPOINT exec $APP_BIN\nCMD [\"--flag\"]\n";
        let out = docker("x/Dockerfile", src);
        let cells = start_cells(&out, "x");
        assert!(cells[0].contains(r#""argv":["/bin/sh","-c","exec $APP_BIN"],"entrypoint":["/bin/sh","-c","exec $APP_BIN"],"cmd":[]"#), "{cells:?}");
        assert!(
            out.markers[0].ends_with("binaries=0 refused=dynamic:1"),
            "{:?}",
            out.markers
        );
        assert!(invocations(&out).is_empty());
    }

    #[test]
    fn docker_start_copy_map_env_arg_globs_and_script_entry() {
        // mealie's shape: an ENV-named home, a script copied in and started.
        let src = "FROM python:3.12\nENV APP_HOME=\"/app\"\nARG SRC=.\nWORKDIR $APP_HOME\nCOPY ./docker/entry.sh $APP_HOME/run.sh\nCOPY ${SRC} /data\nCOPY *.txt ./\nCOPY pkg/ lib/\nADD https://example.com/x.tgz /tmp/\nCOPY --from=nginx:1 /etc/nginx /etc/nginx\nENTRYPOINT [\"/app/run.sh\"]\n";
        let out = docker("svc/Dockerfile", src);
        let cells = start_cells(&out, "svc");
        assert!(
            cells[0].contains(r#""workdir":"/app","context":"svc","copies":[["/app/run.sh","docker/entry.sh"],["/app/lib","pkg"]]"#),
            "{cells:?}"
        );
        assert!(
            out.markers[0].ends_with("copies=2 builds=0 () binaries=0 unmapped=2"),
            "{:?}",
            out.markers
        );
        // A copied script is a repo file the host-start rules map, never a
        // `cli_invoke:run.sh`.
        assert!(invocations(&out).is_empty());
    }

    #[test]
    fn docker_start_none_for_build_only_file_and_healthcheck_none() {
        let src = "FROM golang:1.22\nWORKDIR /src\nHEALTHCHECK NONE\nRUN go build ./...\n";
        let out = docker("lib/Dockerfile", src);
        assert!(start_cells(&out, "lib").is_empty());
        assert_eq!(
            out.markers,
            [
                "[docker-start] file=lib/Dockerfile stages=1 starts=0 (exec=0 shell=0) healthchecks=0 copies=0 builds=0 () binaries=0"
            ]
        );
    }

    #[test]
    fn docker_start_run_cd_and_shell_words() {
        assert_eq!(
            shell_commands("cd cmd && CGO_ENABLED=0 go build -o \"/out/my app\" . ; echo 'a b'"),
            vec![
                vec!["cd".to_string(), "cmd".to_string()],
                vec![
                    "CGO_ENABLED=0".to_string(),
                    "go".to_string(),
                    "build".to_string(),
                    "-o".to_string(),
                    "/out/my app".to_string(),
                    ".".to_string()
                ],
                vec!["echo".to_string(), "a b".to_string()],
            ]
        );
        let src = "FROM golang AS b\nWORKDIR /src\nCOPY . .\nRUN cd cmd/srv && go build -o ../../bin/srv .\nFROM scratch\nCOPY --from=b /src/bin/srv /srv\nENTRYPOINT [\"/srv\"]\n";
        let out = docker("g/Dockerfile", src);
        let cells = start_cells(&out, "g");
        assert!(cells[0].contains(r#""workdir":"/","context":"g","copies":[],"builds":[{"tool":"go","out":"/srv","pkg":".","workdir":"/src/cmd/srv","src":"cmd/srv","stage":"b"}]"#), "{cells:?}");
    }

    #[test]
    fn docker_start_relative_or_app_path_binary_mints_nothing() {
        let src = "FROM alpine\nCOPY --from=builder /out/server ./\nCMD [\"./server\"]\nFROM alpine\nCMD [\"/opt/app/bin/run\"]\nFROM alpine\nCMD [\"/usr/bin/redis-server\", \"--port\", \"7000\"]\n";
        let out = docker("r/Dockerfile", src);
        assert_eq!(
            invocations(&out),
            [(
                "cli_invoke:redis-server".to_string(),
                vec![vec!["--port".to_string(), "7000".to_string()]]
            )]
        );
        assert!(
            out.markers[0].ends_with("copies=0 builds=0 () binaries=1"),
            "{:?}",
            out.markers
        );
    }

    #[test]
    fn docker_start_go_output_dir_takes_the_package_name() {
        let src = "FROM golang AS b\nWORKDIR /w\nRUN go build -o /out/ ./cmd/api\nFROM alpine\nCOPY --from=b /out/api /usr/bin/api\nCMD [\"api\"]\n";
        let out = docker("goapi/Dockerfile", src);
        assert!(
            start_cells(&out, "goapi")[0].contains(r#""builds":[{"tool":"go","out":"/usr/bin/api","pkg":"./cmd/api","workdir":"/w","src":null,"stage":"b"}]"#),
            "{:?}",
            start_cells(&out, "goapi")
        );
        assert!(invocations(&out).is_empty());
    }

    #[test]
    fn docker_start_copy_into_workdir_keeps_the_file_name() {
        let src = "FROM python:3.12\nWORKDIR /loadgen\nCOPY locustfile.py .\nCOPY conf.yaml /etc/app.yaml\nCOPY tools /opt/tools\nENTRYPOINT [\"locust\"]\n";
        let out = docker("lg/Dockerfile", src);
        assert!(
            start_cells(&out, "lg")[0].contains(r#""copies":[["/loadgen/locustfile.py","locustfile.py"],["/etc/app.yaml","conf.yaml"],["/opt/tools","tools"]]"#),
            "{:?}",
            start_cells(&out, "lg")
        );
    }

    // ---- GH.16: orchestrator starts ----

    fn yaml(path: &str, src: &str) -> IacNodes {
        let repo = RepoId(16);
        extract_yaml(src, path, module_id(repo), repo)
    }

    /// The CODE Json cells on `infra:<kind>:<name>`.
    fn cells_on(out: &IacNodes, kind: &str, name: &str) -> Vec<String> {
        let id = infra_id(RepoId(16), kind, name);
        out.nodes
            .iter()
            .filter(|n| n.id == id)
            .flat_map(|n| n.cells.iter())
            .filter_map(|c| match &c.payload {
                glia_core::CellPayload::Json(j) => Some(j.clone()),
                _ => None,
            })
            .collect()
    }

    fn refs(out: &IacNodes) -> Vec<(String, String)> {
        let by_id = |id: NodeId| out.nav.qname_by_id.get(&id).cloned().unwrap_or_default();
        out.edges
            .iter()
            .filter(|e| e.category == edge_category::INFRA_REFERENCES)
            .map(|e| (by_id(e.from), by_id(e.to)))
            .collect()
    }

    const PROBE_COMPOSE: &str = "services:
  api:
    build: ./svc-py
    command: gunicorn app:app
    ports:
      - \"8000:8000\"
  worker:
    build:
      context: ./svc-py
    command: [\"python\", \"worker.py\"]
  gosrv:
    build: ./svc-go
    entrypoint: /bin/server
  db:
    image: postgres:16
";

    #[test]
    fn compose_build_command_entrypoint_probe_stack() {
        let out = yaml("docker-compose.yml", PROBE_COMPOSE);
        assert_eq!(
            out.markers,
            [
                "[compose-start] file=docker-compose.yml services=4 builds=3 commands=2 entrypoints=1 refused=0"
            ]
        );
        let r = refs(&out);
        for (svc, img) in [("api", "svc-py"), ("worker", "svc-py"), ("gosrv", "svc-go")] {
            assert!(
                r.contains(&(format!("infra:service:{svc}"), format!("infra:image:{img}"))),
                "{r:?}"
            );
        }
        assert!(r.contains(&("infra:service:db".into(), "infra:image:postgres".into())));
        assert_eq!(
            cells_on(&out, "service", "api"),
            [
                r#"{"start":{"host":"compose","service":"api","trigger":"container_start","command":["gunicorn","app:app"],"command_form":"string","entrypoint":null,"entrypoint_form":null,"workdir":null,"image":null,"built_image":"svc-py","context":"svc-py","dockerfile":"svc-py/Dockerfile","target_stage":null,"refused":[],"decl":"docker-compose.yml:4"}}"#
            ]
        );
        let worker = cells_on(&out, "service", "worker");
        assert!(
            worker[0].contains(r#""command":["python","worker.py"],"command_form":"list""#),
            "{worker:?}"
        );
        let gosrv = cells_on(&out, "service", "gosrv");
        assert!(
            gosrv[0].contains(r#""command":null,"command_form":null,"entrypoint":["/bin/server"],"entrypoint_form":"string""#),
            "{gosrv:?}"
        );
        // An image-only service starts what its image says: no start cell.
        assert!(cells_on(&out, "service", "db").is_empty());
    }

    #[test]
    fn compose_build_map_target_dockerfile_workdir_and_subdir_file() {
        let src = "version: \"3.8\"
x-base: &base
  build:
    context: ..
    dockerfile: ./docker/dockerfiles/rails.Dockerfile
    target: runtime
  working_dir: /app
services:
  rails:
    <<: *base
    image: registry.example.com/acme/rails:latest
    entrypoint: docker/entrypoints/rails.sh
    command: ['bundle', 'exec', 'rails', 's', '-p', '3000']
  sidekiq:
    <<: *base
    command:
      - bundle
      - exec
      - sidekiq
    entrypoint: []
";
        let out = yaml("deploy/docker-compose.yaml", src);
        assert_eq!(
            out.markers,
            [
                "[compose-start] file=deploy/docker-compose.yaml services=2 builds=2 commands=2 entrypoints=2 refused=0"
            ]
        );
        let rails = cells_on(&out, "service", "rails");
        assert!(
            rails[0].contains(r#""entrypoint":["docker/entrypoints/rails.sh"]"#)
                && rails[0].contains(r#""command":["bundle","exec","rails","s","-p","3000"],"command_form":"list""#)
                && rails[0].contains(r#""workdir":"/app","image":"rails","built_image":"rails","context":".","dockerfile":"docker/dockerfiles/rails.Dockerfile","target_stage":"runtime""#),
            "{rails:?}"
        );
        // `entrypoint: []` clears the image's ENTRYPOINT: read, empty.
        let sidekiq = cells_on(&out, "service", "sidekiq");
        assert!(
            sidekiq[0].contains(r#""command":["bundle","exec","sidekiq"],"command_form":"list","entrypoint":[],"entrypoint_form":"list""#),
            "{sidekiq:?}"
        );
        // The declared image gains a build cell for the naming join.
        assert_eq!(
            cells_on(&out, "image", "rails"),
            [
                r#"{"build":{"host":"compose","image":"rails","builder":"docker","context":".","dockerfile":"docker/dockerfiles/rails.Dockerfile","target_stage":"runtime","built_image":"rails","decl":"deploy/docker-compose.yaml:3"}}"#
            ]
        );
        let r = refs(&out);
        assert!(
            r.contains(&("infra:service:sidekiq".into(), "infra:image:rails".into())),
            "{r:?}"
        );
    }

    #[test]
    fn compose_refuses_templates_remote_contexts_and_climbs() {
        let src = "services:
  a:
    build: https://github.com/acme/app.git#main
  b:
    build: ../../outside
    command: \"{{ .Values.cmd }}\"
  c:
    build: ${CTX}
  d:
    build: ./d
    command: [python, -m, app.main]
";
        let out = yaml("compose.yaml", src);
        assert_eq!(
            out.markers,
            [
                "[compose-start] file=compose.yaml services=4 builds=1 commands=1 entrypoints=0 refused=4"
            ]
        );
        let b = cells_on(&out, "service", "b");
        assert!(
            b[0].contains(r#""command":null"#) && b[0].contains(r#""refused":["build","command"]"#),
            "{b:?}"
        );
        assert_eq!(
            refs(&out),
            [("infra:service:d".to_string(), "infra:image:d".to_string())]
        );
    }

    const PROBE_K8S: &str = "apiVersion: apps/v1
kind: Deployment
metadata:
  name: py-worker
spec:
  template:
    spec:
      containers:
        - name: worker
          image: svc-py:latest
          command: [\"python\"]
          args: [\"worker.py\"]
";

    #[test]
    fn k8s_deployment_command_args_probe_stack() {
        let out = yaml("k8s/deploy.yaml", PROBE_K8S);
        assert_eq!(
            out.markers,
            ["[k8s-start] file=k8s/deploy.yaml containers=1 commands=1 args=1 refused=0"]
        );
        assert_eq!(
            cells_on(&out, "deployment", "py-worker"),
            [
                r#"{"start":{"host":"k8s","workload":"deployment","container":"worker","init":false,"trigger":"container_start","schedule":null,"image":"svc-py","command":["python"],"args":["worker.py"],"workdir":null,"refused":[],"decl":"k8s/deploy.yaml:11"}}"#
            ]
        );
        assert!(refs(&out).contains(&(
            "infra:deployment:py-worker".into(),
            "infra:image:svc-py".into()
        )));
    }

    #[test]
    fn k8s_block_lists_two_containers_init_and_cronjob() {
        let src = "apiVersion: apps/v1
kind: StatefulSet
metadata:
  name: store
spec:
  template:
    spec:
      initContainers:
      - name: migrate
        image: ghcr.io/acme/store:1.2
        command:
        - python
        - manage.py
        - migrate
      containers:
      - name: app
        image: ghcr.io/acme/store:1.2
        workingDir: /srv
        args:
          - --port
          - \"8080\"
      - name: sidecar
        image: envoyproxy/envoy:v1
---
apiVersion: batch/v1
kind: CronJob
metadata:
  name: nightly
spec:
  schedule: \"0 3 * * *\"
  jobTemplate:
    spec:
      template:
        spec:
          containers:
            - name: nightly
              image: svc-py:latest
              command: [\"python\", \"jobs/nightly.py\"]
          restartPolicy: OnFailure
---
apiVersion: v1
kind: Service
metadata:
  name: store
";
        let out = yaml("k8s/store.yaml", src);
        assert_eq!(
            out.markers,
            ["[k8s-start] file=k8s/store.yaml containers=4 commands=2 args=1 refused=0"]
        );
        let store = cells_on(&out, "statefulset", "store");
        assert_eq!(store.len(), 2, "{store:?}");
        assert!(
            store[0].contains(r#""container":"app","init":false"#)
                && store[0].contains(r#""command":null,"args":["--port","8080"],"workdir":"/srv""#),
            "{store:?}"
        );
        assert!(
            store[1].contains(r#""container":"migrate","init":true"#)
                && store[1].contains(r#""command":["python","manage.py","migrate"]"#)
                && store[1].ends_with(r#""decl":"k8s/store.yaml:11"}}"#),
            "{store:?}"
        );
        let nightly = cells_on(&out, "cronjob", "nightly");
        assert!(
            nightly[0].contains(r#""trigger":"schedule","schedule":"0 3 * * *","image":"svc-py","command":["python","jobs/nightly.py"],"args":null"#),
            "{nightly:?}"
        );
        assert!(cells_on(&out, "service", "store").is_empty());
    }

    #[test]
    fn k8s_helm_template_values_are_refused() {
        let src = "apiVersion: apps/v1
kind: Deployment
metadata:
  name: {{ include \"chart.fullname\" . }}
spec:
  template:
    spec:
      containers:
        - name: app
          command: [\"python\"]
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: web
spec:
  template:
    spec:
      containers:
        - name: web
          image: \"{{ .Values.image.repository }}:{{ .Values.image.tag }}\"
          {{- with .Values.command }}
          command:
            {{- toYaml . | nindent 12 }}
          {{- end }}
          args: [\"{{ .Values.arg }}\"]
";
        let out = yaml("chart/templates/deploy.yaml", src);
        assert_eq!(
            out.markers,
            [
                "[k8s-start] file=chart/templates/deploy.yaml containers=1 commands=0 args=0 refused=4"
            ]
        );
        let web = cells_on(&out, "deployment", "web");
        assert!(
            web[0].contains(r#""image":null,"command":null,"args":null"#),
            "{web:?}"
        );
        // The templated metadata.name, then command (a `toYaml` block), args and
        // image of `web`.
        assert!(
            web[0].contains(r#""refused":["command","args","image"]"#),
            "{web:?}"
        );
    }

    #[test]
    fn skaffold_artifacts_name_their_contexts() {
        let src = "apiVersion: skaffold/v3
kind: Config
metadata:
  name: app
build:
  artifacts:
  - image: frontend
    context: src/frontend
  - image: cartservice
    context: src/cartservice/src
    docker:
      dockerfile: Dockerfile
  - image: adservice
    context: src/adservice
    jib: {}
";
        let out = yaml("skaffold.yaml", src);
        assert_eq!(
            out.markers,
            ["[skaffold-build] file=skaffold.yaml artifacts=3 dockerfiles=2 refused=0"]
        );
        assert_eq!(
            cells_on(&out, "image", "cartservice"),
            [
                r#"{"build":{"host":"skaffold","image":"cartservice","builder":"docker","context":"src/cartservice/src","dockerfile":"src/cartservice/src/Dockerfile","target_stage":null,"built_image":"cartservice","decl":"skaffold.yaml:9"}}"#
            ]
        );
        assert!(
            cells_on(&out, "image", "adservice")[0]
                .contains(r#""builder":"jib","context":"src/adservice","dockerfile":null"#)
        );
        // A skaffold config is neither a compose file nor a k8s workload.
        assert!(
            !out.markers
                .iter()
                .any(|m| m.starts_with("[compose-start]") || m.starts_with("[k8s-start]"))
        );
    }

    #[test]
    fn plain_yaml_files_read_nothing() {
        let out = yaml(
            ".github/workflows/ci.yml",
            "on: push\njobs:\n  test:\n    runs-on: ubuntu-latest\n    services:\n      redis:\n        image: redis\n",
        );
        assert!(out.markers.is_empty(), "{:?}", out.markers);
    }

    #[test]
    fn yaml_reader_shapes() {
        let doc = |s: &str| parse_yaml_doc(yaml_docs(s).remove(0));
        let v = doc(
            "a: 1 # c\nb:\n- x\n- 'it''s'\nc: {d: [1, \"two\", ], e: f}\nd: >\n  one\n  two\ne: \"#notcomment\"\n",
        );
        assert_eq!(v.get("a"), Some(&Yv::Str("1".into())));
        assert_eq!(
            v.get("b"),
            Some(&Yv::List(vec![
                (2, Yv::Str("x".into())),
                (3, Yv::Str("it's".into()))
            ]))
        );
        assert_eq!(
            v.path(&["c", "d"]),
            Some(&Yv::List(vec![
                (4, Yv::Str("1".into())),
                (4, Yv::Str("two".into()))
            ]))
        );
        assert_eq!(v.path(&["c", "e"]).and_then(Yv::str), Some("f"));
        assert_eq!(v.get("d").and_then(Yv::str), Some("one two"));
        assert_eq!(v.get("e").and_then(Yv::str), Some("#notcomment"));
        let v = doc("k: [a,\n  b]\nm: *nope\n");
        assert_eq!(
            v.get("k"),
            Some(&Yv::List(vec![
                (0, Yv::Str("a".into())),
                (0, Yv::Str("b".into()))
            ]))
        );
        assert_eq!(v.get("m"), Some(&Yv::Unread));
        // Pathological nesting stops at MAX_YAML_DEPTH instead of the stack.
        let deep = format!("{}{}", "[".repeat(10_000), "]".repeat(10_000));
        assert_eq!(parse_flow(&deep, 0), None);
        let block: String = (0..1_000)
            .map(|i| format!("{}k:\n", " ".repeat(i)))
            .collect();
        assert!(matches!(doc(&block), Yv::Map(_)));
    }

    // ---- GH.17: process managers ------------------------------------------

    /// The `infra:<kind>:<name>` node's start cells, as JSON text.
    fn proc_cells(out: &ProcNodes, kind: &str, name: &str) -> Vec<String> {
        let id = infra_id(RepoId(1), kind, name);
        let Some(node) = out.iac.nodes.iter().find(|n| n.id == id) else {
            return Vec::new();
        };
        node.cells
            .iter()
            .filter_map(|c| match &c.payload {
                glia_core::CellPayload::Json(j) => Some(j.clone()),
                _ => None,
            })
            .collect()
    }

    fn proc_qnames(out: &ProcNodes) -> Vec<String> {
        let mut q: Vec<String> = out.iac.nav.qname_by_id.values().cloned().collect();
        q.sort();
        q
    }

    #[test]
    fn process_manager_paths() {
        assert_eq!(
            ProcessManager::of_path("Procfile"),
            Some(ProcessManager::Procfile)
        );
        assert_eq!(
            ProcessManager::of_path("svc/Procfile.dev"),
            Some(ProcessManager::Procfile)
        );
        assert_eq!(ProcessManager::of_path("procfile"), None);
        assert_eq!(ProcessManager::of_path("Procfile.dev.bak"), None);
        assert_eq!(
            ProcessManager::of_path("deploy/worker.service"),
            Some(ProcessManager::Systemd)
        );
        assert_eq!(ProcessManager::of_path("src/app/user.service.ts"), None);
        assert_eq!(ProcessManager::of_path(".service"), None);
        assert_eq!(
            ProcessManager::of_path("deployments/aio/supervisor.conf"),
            Some(ProcessManager::Supervisor)
        );
        assert_eq!(
            ProcessManager::of_path("supervisord.conf"),
            Some(ProcessManager::Supervisor)
        );
        assert_eq!(
            ProcessManager::of_path("ops/supervisor/conf.d/api.conf"),
            Some(ProcessManager::Supervisor)
        );
        assert_eq!(
            ProcessManager::of_path("etc/supervisord.d/api.conf"),
            Some(ProcessManager::Supervisor)
        );
        assert_eq!(ProcessManager::of_path("deploy/nginx/app.conf"), None);
        // A HOCON conf never is one.
        assert_eq!(
            ProcessManager::of_path("src/main/resources/supervisor/app.conf"),
            None
        );
    }

    #[test]
    fn procfile_types_are_processes_named_by_dir() {
        let repo = RepoId(1);
        let src = "# processes\nrelease: python manage.py migrate\nweb: gunicorn app:app\nworker: celery -A tasks worker\n\nnot a line\n";
        let out = extract_procfile(src, "svc-py/Procfile", module_id(repo), repo);
        assert_eq!(
            proc_qnames(&out),
            [
                "infra:process:release@svc-py",
                "infra:process:web@svc-py",
                "infra:process:worker@svc-py"
            ]
        );
        assert_eq!(
            proc_cells(&out, "process", "web@svc-py"),
            [
                r#"{"start":{"host":"procfile","name":"web","trigger":"process_type","command":"gunicorn app:app","argv":["/bin/sh","-c","gunicorn app:app"],"workdir":null,"dir":"svc-py","decl":"svc-py/Procfile:3"}}"#
            ]
        );
        assert_eq!((out.counts.procfile, out.counts.types), (1, 3));
        assert_eq!(out.counts.gated_out, 0);
        // The shell string is read through to the target.
        assert_eq!(out.counts.forms.get("module_attr"), Some(&1));
        assert_eq!(out.counts.forms.get("celery_app"), Some(&1));
        // Every resource is DEFINES-d from the MODULE.
        assert_eq!(
            out.iac
                .edges
                .iter()
                .filter(|e| e.category == edge_category::DEFINES && e.from == module_id(repo))
                .count(),
            3
        );
        // A root Procfile's dir is `.`.
        let root = extract_procfile("web: node server.js\n", "Procfile", module_id(repo), repo);
        assert_eq!(proc_qnames(&root), ["infra:process:web@."]);
        assert_eq!(root.counts.forms.get("script"), Some(&1));
        // No type line: gated out, nothing minted.
        let none = extract_procfile("# empty\n", "Procfile", module_id(repo), repo);
        assert!(none.iac.nodes.is_empty());
        assert_eq!((none.counts.procfile, none.counts.gated_out), (0, 1));
    }

    #[test]
    fn systemd_unit_exec_start_is_the_start() {
        let repo = RepoId(1);
        let src = "[Unit]\nDescription=worker\n\n[Service]\nType=simple\nWorkingDirectory=-/opt/app\nExecStartPre=/usr/bin/mkdir -p /run/app\nExecStart=-/usr/bin/python3 \\\n  /opt/app/worker.py --queue default\nExecReload=/bin/kill -HUP $MAINPID\n\n[Install]\nWantedBy=multi-user.target\n";
        let out = extract_systemd_unit(src, "deploy/worker.service", module_id(repo), repo);
        assert_eq!(proc_qnames(&out), ["infra:unit:worker"]);
        assert_eq!(
            proc_cells(&out, "unit", "worker"),
            [
                r#"{"start":{"host":"systemd","name":"worker","trigger":"boot","command":"/usr/bin/python3 /opt/app/worker.py --queue default","argv":["/usr/bin/python3","/opt/app/worker.py","--queue","default"],"workdir":"/opt/app","dir":"deploy","decl":"deploy/worker.service:8"}}"#
            ]
        );
        assert_eq!((out.counts.systemd, out.counts.units), (1, 1));
        assert_eq!(out.counts.forms.get("script"), Some(&1));
        assert_eq!(
            out.counts.forms.values().sum::<usize>(),
            1,
            "ExecStartPre / ExecReload are no start"
        );
    }

    #[test]
    fn systemd_prefixes_resets_and_shell_strings() {
        let repo = RepoId(1);
        // `@`: the second word is argv[0]; `+`: privileges. An empty
        // ExecStart= resets the list.
        let src = "[Service]\nExecStart=/bin/false\nExecStart=\nExecStart=@+/usr/bin/node node-app /srv/server.js\nExecStart=/bin/bash -lc 'cd /srv && exec bundle exec puma -C config/puma.rb'\n";
        let out = extract_systemd_unit(src, "deployment/app.1.service", module_id(repo), repo);
        let cells = proc_cells(&out, "unit", "app.1");
        assert_eq!(cells.len(), 2, "{cells:?}");
        assert!(
            cells[0].contains(r#""argv":["/usr/bin/node","/srv/server.js"]"#),
            "{}",
            cells[0]
        );
        assert!(cells[0].contains(r#""decl":"deployment/app.1.service:4""#));
        assert!(
            cells[1].contains(
                r#""argv":["/bin/bash","-lc","cd /srv && exec bundle exec puma -C config/puma.rb"]"#
            ),
            "{}",
            cells[1]
        );
        assert_eq!(out.counts.forms.get("script"), Some(&1));
        assert_eq!(out.counts.forms.get("framework"), Some(&1));
    }

    #[test]
    fn service_file_without_service_section_is_gated_out() {
        let repo = RepoId(1);
        let out = extract_systemd_unit(
            "export class UserService {}\n",
            "src/user.service",
            module_id(repo),
            repo,
        );
        assert!(out.iac.nodes.is_empty());
        assert_eq!(
            (out.counts.systemd, out.counts.units, out.counts.gated_out),
            (0, 0, 1)
        );
        // A [Service] section with no ExecStart is still a unit, with no start.
        let bare = extract_systemd_unit(
            "[Service]\nType=oneshot\n",
            "x.service",
            module_id(repo),
            repo,
        );
        assert_eq!(proc_qnames(&bare), ["infra:unit:x"]);
        assert!(proc_cells(&bare, "unit", "x").is_empty());
    }

    #[test]
    fn supervisor_programs_are_starts() {
        let repo = RepoId(1);
        let src = "[supervisord]\nnodaemon=true\n\n[program:api]\ncommand=sh -c \"./bin/docker-entrypoint-api.sh\"   ; the API\ndirectory=/app/api\nautostart=true\n\n[program:live]\ncommand = node\n    /app/live/apps/live\n\n[program:empty]\nautostart=false\n\n[group:all]\nprograms=api,live\n";
        let out = extract_supervisor(
            src,
            "deployments/aio/community/supervisor.conf",
            module_id(repo),
            repo,
        );
        assert_eq!(
            proc_qnames(&out),
            ["infra:program:api", "infra:program:live"]
        );
        assert_eq!(
            proc_cells(&out, "program", "api"),
            [
                r#"{"start":{"host":"supervisor","name":"api","trigger":"supervised","command":"sh -c \"./bin/docker-entrypoint-api.sh\"","argv":["sh","-c","./bin/docker-entrypoint-api.sh"],"workdir":"/app/api","dir":"deployments/aio/community","decl":"deployments/aio/community/supervisor.conf:5"}}"#
            ]
        );
        assert!(
            proc_cells(&out, "program", "live")[0]
                .contains(r#""argv":["node","/app/live/apps/live"]"#)
        );
        assert_eq!((out.counts.supervisor, out.counts.programs), (1, 2));
        // `./bin/x.sh` run directly is GH.14's Binary; `node <file>` a Script.
        assert_eq!(out.counts.forms.get("binary"), Some(&1));
        assert_eq!(out.counts.forms.get("script"), Some(&1));
        // No program: gated out.
        let none = extract_supervisor(
            "[supervisord]\nnodaemon=true\n",
            "supervisord.conf",
            module_id(repo),
            repo,
        );
        assert!(none.iac.nodes.is_empty());
        assert_eq!(none.counts.gated_out, 1);
    }

    #[test]
    fn proc_counts_marker() {
        let mut c = ProcCounts::default();
        assert_eq!(c.marker("."), None);
        let repo = RepoId(1);
        c.add(
            &extract_procfile(
                "web: gunicorn app:app\nworker: celery -A tasks worker\n",
                "svc-py/Procfile",
                module_id(repo),
                repo,
            )
            .counts,
        );
        c.add(
            &extract_systemd_unit(
                "[Service]\nExecStart=/usr/bin/python3 /opt/app/worker.py\n",
                "deploy/worker.service",
                module_id(repo),
                repo,
            )
            .counts,
        );
        c.add(
            &extract_systemd_unit(
                "[Service]\nExecStart=$APP\n",
                "a.service",
                module_id(repo),
                repo,
            )
            .counts,
        );
        assert_eq!(
            c.marker(".").as_deref(),
            Some(
                "[proc-start] procfile=1 types=2 systemd=2 units=2 supervisor=0 programs=0 gated_out=0 repo=. forms=(celery_app=1 module_attr=1 script=1) refused=dynamic:1"
            )
        );
    }
}
