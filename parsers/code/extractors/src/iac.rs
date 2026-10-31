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
//! Out of scope (deferred to v0.5+):
//!   - Kustomize overlays (template merging is non-trivial)
//!   - Helm chart templating
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

/// Extract from a YAML manifest. Detects k8s `kind:` declarations and
/// docker-compose `services:` entries. Emits one INFRA_RESOURCE per object,
/// linking containers' `image:` refs to their host workload.
pub fn extract_yaml(source: &str, module_id: NodeId, repo: RepoId) -> IacNodes {
    let mut out = IacNodes {
        nodes: Vec::new(),
        edges: Vec::new(),
        nav: CodeNav::default(),
        markers: Vec::new(),
    };
    extract_k8s_documents(source, module_id, repo, &mut out);
    extract_compose_services(source, module_id, repo, &mut out);
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
        let out = extract_yaml(src, module_id(repo), repo);
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
        let out = extract_yaml(src, module_id(repo), repo);
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
        let out = extract_yaml(src, module_id(repo), repo);
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
        let out = extract_yaml(src, module_id(repo), repo);
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
        let out = extract_yaml(src, module_id(repo), repo);
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
}
