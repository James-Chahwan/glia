//! IaC (Infrastructure-as-Code) extraction (v0.4.x — task #9).
//!
//! Emits `INFRA_RESOURCE` nodes for declared infra entities and
//! `INFRA_REFERENCES` edges for intra-graph links (Deployment → Image,
//! docker-compose service → image). `IacResolver` then pairs same-qname
//! resources across repos to surface "image built in repo A consumed by
//! k8s manifest in repo B" style cross-service joins.
//!
//! Single qname shape: `infra:<kind>:<name>`. Kinds:
//!   - `image`         — Dockerfile build target (basename of Dockerfile dir)
//!                       or compose/k8s image-ref (image name without tag)
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

use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, edge_category, node_kind};
use repo_graph_core::{Confidence, Edge, Node, NodeId, RepoId};

pub struct IacNodes {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub nav: CodeNav,
}

/// Extract from a Dockerfile. Emits one `infra:image:<basename>` node where
/// `<basename>` is the parent directory basename (the conventional image name
/// before any tag is applied at build time). Cell records the FROM base.
pub fn extract_dockerfile(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
) -> IacNodes {
    let mut out = IacNodes {
        nodes: Vec::new(),
        edges: Vec::new(),
        nav: CodeNav::default(),
    };
    let base_image = dockerfile_from_base(source);
    let image_name = dockerfile_image_name(path);
    if !image_name.is_empty() {
        emit_resource(&mut out, "image", &image_name, module_id, repo);
        // If FROM references another image, link this image → its base. Useful
        // for "what depends on python:3.11-slim" queries.
        if let Some(base) = &base_image {
            let base_name = image_basename(base);
            if !base_name.is_empty() && base_name != image_name {
                let from_id = NodeId::from_parts(
                    GRAPH_TYPE,
                    repo,
                    node_kind::INFRA_RESOURCE,
                    &format!("infra:image:{image_name}"),
                );
                emit_resource(&mut out, "image", &base_name, module_id, repo);
                let to_id = NodeId::from_parts(
                    GRAPH_TYPE,
                    repo,
                    node_kind::INFRA_RESOURCE,
                    &format!("infra:image:{base_name}"),
                );
                out.edges.push(Edge {
                    from: from_id,
                    to: to_id,
                    category: edge_category::INFRA_REFERENCES,
                    confidence: Confidence::Medium,
                });
            }
        }
    }
    out
}

/// Extract from a YAML manifest. Detects k8s `kind:` declarations and
/// docker-compose `services:` entries. Emits one INFRA_RESOURCE per object,
/// linking containers' `image:` refs to their host workload.
pub fn extract_yaml(
    source: &str,
    module_id: NodeId,
    repo: RepoId,
) -> IacNodes {
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

fn emit_resource(
    out: &mut IacNodes,
    kind: &str,
    name: &str,
    module_id: NodeId,
    repo: RepoId,
) {
    let qname = format!("infra:{kind}:{name}");
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
    out.nav.record(id, name, &qname, node_kind::INFRA_RESOURCE, Some(module_id));
    // Module → resource edge for "what does this file declare" queries.
    out.edges.push(Edge {
        from: module_id,
        to: id,
        category: edge_category::DEFINES,
        confidence: Confidence::Medium,
    });
}

/// Image basename: `ghcr.io/foo/bar:1.2` → `bar`. Drops registry, namespace,
/// and tag.
fn image_basename(image: &str) -> String {
    let no_tag = image.split(':').next().unwrap_or(image);
    no_tag.rsplit('/').next().unwrap_or(no_tag).to_string()
}

/// Dockerfile image name: derive from the parent directory name. Falls back
/// to the file name minus `.dockerfile` extension. Convention is to assume
/// repos use one image per Dockerfile and the surrounding directory names it.
fn dockerfile_image_name(path: &str) -> String {
    let norm = path.replace('\\', "/");
    let segments: Vec<&str> = norm.split('/').collect();
    if segments.len() >= 2 {
        let parent = segments[segments.len() - 2];
        if !parent.is_empty() && parent != "." {
            return parent.to_string();
        }
    }
    // Fall back: strip .dockerfile suffix or use Dockerfile basename.
    let base = segments.last().copied().unwrap_or("dockerfile");
    let lower = base.to_ascii_lowercase();
    if let Some(stripped) = lower.strip_suffix(".dockerfile") {
        return stripped.to_string();
    }
    if let Some(stripped) = lower.strip_prefix("dockerfile.") {
        return stripped.to_string();
    }
    "image".to_string()
}

fn dockerfile_from_base(source: &str) -> Option<String> {
    for line in source.lines() {
        let trimmed = line.trim_start();
        // Strip inline comments.
        let trimmed = trimmed.splitn(2, '#').next().unwrap_or(trimmed).trim();
        let upper = trimmed.get(..5).unwrap_or("").to_ascii_uppercase();
        if upper == "FROM " {
            // `FROM image[:tag] [AS alias]` — first token after FROM.
            let rest = trimmed[5..].trim();
            let first = rest.split_whitespace().next().unwrap_or(rest);
            return Some(first.to_string());
        }
    }
    None
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

fn extract_k8s_documents(
    source: &str,
    module_id: NodeId,
    repo: RepoId,
    out: &mut IacNodes,
) {
    for doc in source.split("\n---") {
        let doc = doc.trim_start_matches('\n');
        if doc.is_empty() {
            continue;
        }
        let Some(kind) = read_k8s_kind(doc) else { continue };
        let Some(name) = read_k8s_name(doc) else { continue };
        let kind_lower = kind.to_ascii_lowercase();
        emit_resource(out, &kind_lower, &name, module_id, repo);

        // Link container images. Walk every `image:` line in the doc; for
        // each, emit an `infra:image:<basename>` node + REFERENCES edge from
        // this resource.
        let from_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo,
            node_kind::INFRA_RESOURCE,
            &format!("infra:{kind_lower}:{name}"),
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
                &format!("infra:image:{basename}"),
            );
            out.edges.push(Edge {
                from: from_id,
                to: to_id,
                category: edge_category::INFRA_REFERENCES,
                confidence: Confidence::Medium,
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
            if K8S_KINDS.iter().any(|k| *k == v) {
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

fn extract_compose_services(
    source: &str,
    module_id: NodeId,
    repo: RepoId,
    out: &mut IacNodes,
) {
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
        if let Some(rest) = trimmed.strip_prefix("image:") {
            if let Some(svc) = &current_svc {
                let v = rest.trim().trim_matches(|c| c == '"' || c == '\'');
                if !v.is_empty() && !v.starts_with('$') {
                    let basename = image_basename(v);
                    if !basename.is_empty() {
                        emit_resource(out, "image", &basename, module_id, repo);
                        let from_id = NodeId::from_parts(
                            GRAPH_TYPE,
                            repo,
                            node_kind::INFRA_RESOURCE,
                            &format!("infra:service:{svc}"),
                        );
                        let to_id = NodeId::from_parts(
                            GRAPH_TYPE,
                            repo,
                            node_kind::INFRA_RESOURCE,
                            &format!("infra:image:{basename}"),
                        );
                        out.edges.push(Edge {
                            from: from_id,
                            to: to_id,
                            category: edge_category::INFRA_REFERENCES,
                            confidence: Confidence::Medium,
                        });
                    }
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
        assert!(out.edges.iter().any(|e| e.category == edge_category::INFRA_REFERENCES));
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
            e.from == dep_id
                && e.to == img_id
                && e.category == edge_category::INFRA_REFERENCES
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
            e.from == svc_id
                && e.to == img_id
                && e.category == edge_category::INFRA_REFERENCES
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
