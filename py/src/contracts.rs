//! **contracts** (A12): producer / consumer message-type agreement per
//! queue topic.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use repo_graph_graph::MergedGraph;

use crate::graph::PyGraph;

/// The whole body of [`PyGraph::contracts`], minus pyo3 — kept pyo3-free so
/// `cargo test -p repo-graph-py` can cover the binding (see the crate doc).
fn contracts_json(merged: &MergedGraph) -> Result<String, serde_json::Error> {
    // `graphs` is one entry per (repo, language), so count distinct repos.
    let repos: std::collections::BTreeSet<u64> = merged.graphs.iter().map(|g| g.repo.0).collect();
    eprintln!("[contracts] surface=pyo3 repos={}", repos.len());
    serde_json::to_string(&repo_graph_engine::message_contracts(merged))
}

#[pymethods]
impl PyGraph {
    /// **contracts** (A12): for every queue topic, the producer's and
    /// consumer's declared message type and whether they agree. Each row
    /// `{topic, topic_is_tag, pattern, producer, consumer, status, confidence,
    /// note}` where each side is `null` or `{node_id, repo_id, qname, topic,
    /// module, file, line, message_type, message_type_raw, form, window,
    /// types_seen, conflicting}`. `status` ∈ {match, mismatch, unknown}.
    /// `repo_id` is the raw `RepoId` (an xxhash of the repo identity key: git
    /// remote / git dir / dir name, LB.1); Python has no label map for it yet. Report-only: no edge is emitted. Returns a
    /// JSON array.
    fn contracts(&self) -> PyResult<String> {
        contracts_json(&self.merged).map_err(|e| PyValueError::new_err(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A12.3: `contracts()` is transport only — pin the wiring (a real build's
    /// rows reach Python as a JSON array of the documented row shape). The
    /// verdict logic is covered by the engine's `message_contracts` tests.
    #[test]
    fn contracts_returns_the_documented_json_array() {
        let empty = contracts_json(&MergedGraph::new(Vec::new())).expect("serialises");
        assert_eq!(empty, "[]");

        let root = std::env::temp_dir().join(format!("glia-a12-3-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("temp dir");
        let publisher = "package svc\n\nimport \"github.com/nats-io/nats.go\"\n\n\
            func Publish(nc *nats.Conn) error {\n\treturn nc.Publish(\"orders\", nil)\n}\n";
        std::fs::write(root.join("publisher.go"), publisher).expect("write fixture");
        let built = repo_graph_engine::generate_one(root.to_str().expect("utf-8 temp path"));
        let _ = std::fs::remove_dir_all(&root);
        let merged = built.expect("build").merged;

        let json = contracts_json(&merged).expect("serialises");
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let rows = v.as_array().expect("a JSON array, not an object");
        let first = rows
            .first()
            .and_then(|r| r.as_object())
            .expect("one row per topic");
        for key in [
            "topic",
            "topic_is_tag",
            "producer",
            "consumer",
            "status",
            "note",
        ] {
            assert!(first.contains_key(key), "missing `{key}` in {json}");
        }
        assert_eq!(first["topic"], "orders", "{json}");
        assert!(
            first["consumer"].is_null(),
            "a producer-only topic is one-sided: {json}"
        );
    }
}
