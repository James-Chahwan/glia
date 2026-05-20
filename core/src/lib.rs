//! repo-graph-core — domain-agnostic knowledge graph primitives.
//!
//! Strict Node shape: `{id, repo, confidence, cells}`. Navigation lives in
//! domain-owned indices stored in the container, not in Node fields — this
//! keeps the core agnostic to code vs chemistry vs video vs policy.
//!
//! Cell payloads are one of `Text` / `Json` / `Bytes`. Cell/Edge/NodeKind
//! tags are `u32` registry-backed; the registries live in the container
//! header (not in this crate). `GraphType` is a self-describing string.
//!
//! See memory: `reference_format_spec.md`, `project_040_vision.md`.

use core::hash::Hasher;
use twox_hash::XxHash64;

// ============================================================================
// IDs
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug, PartialEq, Eq, Hash))]
pub struct NodeId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug, PartialEq, Eq, Hash))]
pub struct RepoId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug, PartialEq, Eq, Hash))]
pub struct ShardId(pub u64);

/// Self-describing graph-type tag — one per container file.
/// Code = `"code"`, chemistry = `"chemistry"`, etc. Core interprets no values.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct GraphType(pub String);

impl GraphType {
    pub fn code() -> Self { Self("code".into()) }
    pub fn as_str(&self) -> &str { &self.0 }
}

impl NodeId {
    /// `NodeId = xxhash(graph_type, repo, kind, qualified_name)`.
    /// Separators prevent field-boundary collisions.
    pub fn from_parts(graph_type: &str, repo: RepoId, kind: NodeKindId, qname: &str) -> Self {
        let mut h = XxHash64::with_seed(0);
        h.write(graph_type.as_bytes());
        h.write_u8(0xFF);
        h.write_u64(repo.0);
        h.write_u8(0xFF);
        h.write_u32(kind.0);
        h.write_u8(0xFF);
        h.write(qname.as_bytes());
        Self(h.finish())
    }
}

impl RepoId {
    pub fn from_canonical(url_or_path: &str) -> Self {
        let mut h = XxHash64::with_seed(0);
        h.write(url_or_path.as_bytes());
        Self(h.finish())
    }
}

impl ShardId {
    pub fn from_parts(repo: RepoId, shard_name: &str) -> Self {
        let mut h = XxHash64::with_seed(0);
        h.write_u64(repo.0);
        h.write_u8(0xFF);
        h.write(shard_name.as_bytes());
        Self(h.finish())
    }
}

// ============================================================================
// Kinds & registry-backed tags
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug, PartialEq, Eq, Hash))]
pub enum Confidence {
    Strong,
    Medium,
    Weak,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug, PartialEq, Eq, Hash))]
pub enum FlowKind {
    Http,
    Page,
    Cli,
    Grpc,
    Queue,
}

/// Registry-backed cell type tag. Interpretation lives in the per-domain
/// cell registry stored in the container header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug, PartialEq, Eq, Hash))]
pub struct CellTypeId(pub u32);

/// Registry-backed edge category tag. Interpretation lives in the per-domain
/// edge registry stored in the container header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug, PartialEq, Eq, Hash))]
pub struct EdgeCategoryId(pub u32);

/// Registry-backed node-kind tag. Code: Module/Class/Method/Route/...
/// Chemistry: Atom/Bond/Molecule/...
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug, PartialEq, Eq, Hash))]
pub struct NodeKindId(pub u32);

// ============================================================================
// Cells
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub enum CellPayload {
    /// Most cells: code, intent, doc, conv.
    Text(String),
    /// Structured cells: position, attn, decisions.
    Json(String),
    /// Binary cells: cached embeddings.
    Bytes(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct Cell {
    pub kind: CellTypeId,
    pub payload: CellPayload,
}

// ============================================================================
// Core graph types
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct Node {
    pub id: NodeId,
    pub repo: RepoId,
    pub confidence: Confidence,
    pub cells: Vec<Cell>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug, PartialEq, Eq, Hash))]
pub struct Edge {
    pub from: NodeId,
    pub to: NodeId,
    pub category: EdgeCategoryId,
    pub confidence: Confidence,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct Flow {
    pub kind: FlowKind,
    pub entry: NodeId,
    pub steps: Vec<NodeId>,
    pub confidence: Confidence,
}

// ============================================================================
// Traits — same surface on Owned and Archived forms
// ============================================================================

pub trait NodeLike {
    fn id(&self) -> NodeId;
    fn repo(&self) -> RepoId;
    fn confidence(&self) -> Confidence;
    fn cell_count(&self) -> usize;
}

impl NodeLike for Node {
    fn id(&self) -> NodeId { self.id }
    fn repo(&self) -> RepoId { self.repo }
    fn confidence(&self) -> Confidence { self.confidence }
    fn cell_count(&self) -> usize { self.cells.len() }
}

impl NodeLike for ArchivedNode {
    fn id(&self) -> NodeId { NodeId(self.id.0.to_native()) }
    fn repo(&self) -> RepoId { RepoId(self.repo.0.to_native()) }
    fn confidence(&self) -> Confidence { (&self.confidence).into() }
    fn cell_count(&self) -> usize { self.cells.len() }
}

#[allow(clippy::wrong_self_convention)]
pub trait EdgeLike {
    fn from_id(&self) -> NodeId;
    fn to_id(&self) -> NodeId;
    fn category(&self) -> EdgeCategoryId;
    fn confidence(&self) -> Confidence;
}

impl EdgeLike for Edge {
    fn from_id(&self) -> NodeId { self.from }
    fn to_id(&self) -> NodeId { self.to }
    fn category(&self) -> EdgeCategoryId { self.category }
    fn confidence(&self) -> Confidence { self.confidence }
}

impl EdgeLike for ArchivedEdge {
    fn from_id(&self) -> NodeId { NodeId(self.from.0.to_native()) }
    fn to_id(&self) -> NodeId { NodeId(self.to.0.to_native()) }
    fn category(&self) -> EdgeCategoryId { EdgeCategoryId(self.category.0.to_native()) }
    fn confidence(&self) -> Confidence { (&self.confidence).into() }
}

// Bridge the archived unit-variant enum back to its owned form — needed to
// make the traits uniform across Owned/Archived.
impl From<&ArchivedConfidence> for Confidence {
    fn from(v: &ArchivedConfidence) -> Self {
        match v {
            ArchivedConfidence::Strong => Confidence::Strong,
            ArchivedConfidence::Medium => Confidence::Medium,
            ArchivedConfidence::Weak => Confidence::Weak,
        }
    }
}

// ============================================================================
// Project-name derivation (G18)
// ============================================================================

/// Resolve a human-friendly project name for `repo` by inspecting the
/// canonical manifest files in priority order:
///
/// 1. `Cargo.toml`        — `[package].name`
/// 2. `package.json`      — `.name`
/// 3. `pyproject.toml`    — `[project].name`
/// 4. `composer.json`     — `.name`
/// 5. `go.mod`            — last `/`-segment of the `module` line
/// 6. `repo.file_name()`  — fall-back to the directory name
///
/// Returns `None` only if every step fails (typically: `repo` is `/` or empty,
/// has no manifests, and has no directory-name component).
///
/// The implementation deliberately avoids pulling in a full TOML / JSON
/// dependency in the core crate — it does single-key string extraction
/// scoped to the relevant top-level section.
pub fn project_name(repo: &std::path::Path) -> Option<String> {
    if let Some(name) = read_toml_section_key(&repo.join("Cargo.toml"), "package", "name") {
        return Some(name);
    }
    if let Some(name) = read_json_top_string(&repo.join("package.json"), "name") {
        return Some(name);
    }
    if let Some(name) = read_toml_section_key(&repo.join("pyproject.toml"), "project", "name") {
        return Some(name);
    }
    if let Some(name) = read_json_top_string(&repo.join("composer.json"), "name") {
        return Some(name);
    }
    if let Some(name) = read_go_mod_module(&repo.join("go.mod")) {
        return Some(name);
    }
    repo.file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
}

/// Pull a `key = "value"` line out of a TOML file scoped to `[section]`.
/// Stops at the next section header or EOF. Tolerates whitespace and inline
/// comments; ignores quoted strings spanning lines (the affected manifests
/// never need that).
fn read_toml_section_key(path: &std::path::Path, section: &str, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let header = format!("[{section}]");
    let mut in_section = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_section = line == header;
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some(rest) = line.strip_prefix(key) {
            let rest = rest.trim_start();
            if let Some(after_eq) = rest.strip_prefix('=') {
                let v = after_eq.trim();
                if let Some(unquoted) = strip_toml_string(v) {
                    return Some(unquoted);
                }
            }
        }
    }
    None
}

/// Strip surrounding `"..."` or `'...'` from a TOML scalar, honouring no
/// escape semantics beyond the literal quote character.
fn strip_toml_string(v: &str) -> Option<String> {
    let bytes = v.as_bytes();
    if bytes.len() < 2 {
        return None;
    }
    let quote = bytes[0];
    if quote != b'"' && quote != b'\'' {
        return None;
    }
    let end = v[1..].find(quote as char)?;
    Some(v[1..=end].to_string())
}

/// Pull a top-level `"key": "value"` string out of a JSON file. Naively
/// scans for the first occurrence of `"<key>"` followed by a colon then a
/// quoted string. Returns `None` if not found.
fn read_json_top_string(path: &std::path::Path, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let needle = format!("\"{key}\"");
    let mut from = 0;
    while let Some(rel) = text[from..].find(&needle) {
        let pos = from + rel;
        from = pos + needle.len();
        let after = text[pos + needle.len()..].trim_start();
        let rest = match after.strip_prefix(':') {
            Some(r) => r.trim_start(),
            None => continue,
        };
        let rb = rest.as_bytes();
        if rb.is_empty() || rb[0] != b'"' {
            continue;
        }
        let mut i = 1;
        let mut value = String::new();
        while i < rb.len() {
            if rb[i] == b'\\' && i + 1 < rb.len() {
                value.push(rb[i + 1] as char);
                i += 2;
                continue;
            }
            if rb[i] == b'"' {
                return Some(value);
            }
            value.push(rb[i] as char);
            i += 1;
        }
    }
    None
}

/// Pull the trailing path-segment of the `module ...` declaration from a
/// `go.mod` file. The neuropil convention is to use the last `/`-segment as
/// the human-friendly project name.
fn read_go_mod_module(path: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    for raw in text.lines() {
        let line = raw.trim();
        if let Some(rest) = line.strip_prefix("module ") {
            let module = rest.trim();
            let last = module.rsplit('/').next().unwrap_or(module);
            if !last.is_empty() {
                return Some(last.to_string());
            }
        }
    }
    None
}

// ============================================================================
// Errors
// ============================================================================

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("id collision: {0}")]
    IdCollision(String),
    #[error("missing parent for node {0:?}")]
    MissingParent(NodeId),
    #[error("invalid utf-8: {0}")]
    InvalidUtf8(#[from] core::str::Utf8Error),
    #[error("registry has no entry for id {0}")]
    RegistryUnknown(u32),
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_id_deterministic() {
        let repo = RepoId::from_canonical("github.com/x/y");
        let a = NodeId::from_parts("code", repo, NodeKindId(1), "foo.bar.baz");
        let b = NodeId::from_parts("code", repo, NodeKindId(1), "foo.bar.baz");
        assert_eq!(a, b);
    }

    #[test]
    fn node_id_separators_prevent_field_collision() {
        let repo = RepoId::from_canonical("r");
        let a = NodeId::from_parts("co", repo, NodeKindId(1), "de");
        let b = NodeId::from_parts("c", repo, NodeKindId(1), "ode");
        assert_ne!(a, b);
    }

    #[test]
    fn rkyv_roundtrip_nodes() {
        let nodes = vec![Node {
            id: NodeId::from_parts("code", RepoId(1), NodeKindId(1), "mod.a"),
            repo: RepoId(1),
            confidence: Confidence::Strong,
            cells: vec![Cell {
                kind: CellTypeId(0),
                payload: CellPayload::Text("hello".into()),
            }],
        }];
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&nodes).unwrap();
        let archived =
            rkyv::access::<rkyv::Archived<Vec<Node>>, rkyv::rancor::Error>(&bytes).unwrap();
        let back: Vec<Node> =
            rkyv::deserialize::<Vec<Node>, rkyv::rancor::Error>(archived).unwrap();
        assert_eq!(nodes, back);
    }

    #[test]
    fn node_like_trait_works_on_both_forms() {
        let n = Node {
            id: NodeId(42),
            repo: RepoId(7),
            confidence: Confidence::Medium,
            cells: vec![],
        };
        // Owned
        assert_eq!(n.id(), NodeId(42));
        assert_eq!(n.repo(), RepoId(7));
        assert_eq!(n.confidence(), Confidence::Medium);
        assert_eq!(n.cell_count(), 0);

        // Archived
        let nodes = vec![n.clone()];
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&nodes).unwrap();
        let archived =
            rkyv::access::<rkyv::Archived<Vec<Node>>, rkyv::rancor::Error>(&bytes).unwrap();
        let arch_n = &archived[0];
        assert_eq!(arch_n.id(), NodeId(42));
        assert_eq!(arch_n.repo(), RepoId(7));
        assert_eq!(arch_n.confidence(), Confidence::Medium);
        assert_eq!(arch_n.cell_count(), 0);
    }

    #[test]
    fn edge_like_trait_works_on_both_forms() {
        let e = Edge {
            from: NodeId(1),
            to: NodeId(2),
            category: EdgeCategoryId(5),
            confidence: Confidence::Weak,
        };
        assert_eq!(e.from_id(), NodeId(1));

        let edges = vec![e];
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&edges).unwrap();
        let archived =
            rkyv::access::<rkyv::Archived<Vec<Edge>>, rkyv::rancor::Error>(&bytes).unwrap();
        let arch_e = &archived[0];
        assert_eq!(arch_e.from_id(), NodeId(1));
        assert_eq!(arch_e.to_id(), NodeId(2));
        assert_eq!(arch_e.category(), EdgeCategoryId(5));
        assert_eq!(arch_e.confidence(), Confidence::Weak);
    }

    #[test]
    fn project_name_cargo_toml() {
        let tmp = std::env::temp_dir().join("glia_pn_cargo");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(
            tmp.join("Cargo.toml"),
            "[package]\nname = \"my-lib\"\nversion = \"0.1\"\n",
        )
        .unwrap();
        assert_eq!(project_name(&tmp), Some("my-lib".to_string()));
    }

    #[test]
    fn project_name_cargo_skips_other_sections() {
        let tmp = std::env::temp_dir().join("glia_pn_cargo_other");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(
            tmp.join("Cargo.toml"),
            "[workspace]\nname = \"wrong\"\n[package]\nname = \"right\"\n",
        )
        .unwrap();
        assert_eq!(project_name(&tmp), Some("right".to_string()));
    }

    #[test]
    fn project_name_package_json() {
        let tmp = std::env::temp_dir().join("glia_pn_pkg");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(
            tmp.join("package.json"),
            r#"{ "version": "1.0", "name": "my-app", "deps": {} }"#,
        )
        .unwrap();
        assert_eq!(project_name(&tmp), Some("my-app".to_string()));
    }

    #[test]
    fn project_name_pyproject() {
        let tmp = std::env::temp_dir().join("glia_pn_py");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(
            tmp.join("pyproject.toml"),
            "[build-system]\nrequires = [\"setuptools\"]\n[project]\nname = \"my_py\"\n",
        )
        .unwrap();
        assert_eq!(project_name(&tmp), Some("my_py".to_string()));
    }

    #[test]
    fn project_name_go_mod_last_segment() {
        let tmp = std::env::temp_dir().join("glia_pn_go");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(
            tmp.join("go.mod"),
            "module github.com/me/coolservice\n\ngo 1.22\n",
        )
        .unwrap();
        assert_eq!(project_name(&tmp), Some("coolservice".to_string()));
    }

    #[test]
    fn project_name_falls_back_to_dirname() {
        let tmp = std::env::temp_dir().join("glia_pn_fallback");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        assert_eq!(project_name(&tmp), Some("glia_pn_fallback".to_string()));
    }

    #[test]
    fn project_name_priority_cargo_over_package_json() {
        let tmp = std::env::temp_dir().join("glia_pn_priority");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("Cargo.toml"), "[package]\nname = \"rust-one\"\n").unwrap();
        std::fs::write(tmp.join("package.json"), r#"{"name":"js-one"}"#).unwrap();
        assert_eq!(project_name(&tmp), Some("rust-one".to_string()));
    }

    #[test]
    fn hash_collision_smoke_1000() {
        use std::collections::HashSet;
        let repo = RepoId::from_canonical("repo");
        let mut seen = HashSet::new();
        for i in 0..1000 {
            let id = NodeId::from_parts("code", repo, NodeKindId(1), &format!("entity_{i}"));
            assert!(seen.insert(id), "collision at i={i}");
        }
    }
}
