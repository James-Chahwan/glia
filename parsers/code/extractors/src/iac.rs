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
    out
}

/// Extract from a YAML manifest. Detects k8s `kind:` declarations and
/// docker-compose `services:` entries. Emits one INFRA_RESOURCE per object,
/// linking containers' `image:` refs to their host workload.
pub fn extract_yaml(source: &str, module_id: NodeId, repo: RepoId) -> IacNodes {
    let mut out = IacNodes {
        nodes: Vec::new(),
        edges: Vec::new(),
        nav: CodeNav::default(),
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
}
