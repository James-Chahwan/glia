//! **contracts** (A12): producer / consumer message-type agreement per
//! queue topic, and (LE.10d) the field-level diff of every contract pairing.

use pyo3::prelude::*;

use glia_graph::MergedGraph;

use crate::convert::to_py;
use crate::graph::PyGraph;

/// The whole body of [`PyGraph::contracts`], minus pyo3 — kept pyo3-free so
/// `cargo test -p glia-py` can cover the binding (see the crate doc).
fn contracts_json(merged: &MergedGraph) -> Result<String, serde_json::Error> {
    // `graphs` is one entry per (repo, language), so count distinct repos.
    let repos: std::collections::BTreeSet<u64> = merged.graphs.iter().map(|g| g.repo.0).collect();
    eprintln!("[contracts] surface=pyo3 repos={}", repos.len());
    serde_json::to_string(&glia_engine::message_contracts(merged))
}

/// The whole body of [`PyGraph::contract_fields`], minus pyo3 (see
/// [`contracts_json`]). JSON text rather than a `serde_json::Value`: `to_py`
/// decodes text so each row dict keeps the engine struct's field order (a
/// `Value` map would sort it; see `convert`). The engine prints its
/// `[contract-fields] pairs=…` line first (only when it found rows), then this
/// surface's marker, always.
fn contract_fields_json(merged: &MergedGraph) -> Result<String, serde_json::Error> {
    let rows = glia_engine::contract_fields::contract_fields(merged);
    eprintln!("[contract-fields] surface=pyo3 rows={}", rows.len());
    serde_json::to_string(&rows)
}

#[pymethods]
impl PyGraph {
    /// **contracts** (A12): for every queue topic, the producer's and
    /// consumer's declared message type and whether they agree. Each row
    /// `{topic, topic_is_tag, pattern, producer, consumer, status, confidence,
    /// note}` where each side is `null` or `{node_id, repo_id, qname, topic,
    /// module, file, line, message_type, message_type_raw, form, window,
    /// types_seen, conflicting}` (`line` is 1-based). `status` ∈ {match,
    /// mismatch, unknown}. `repo_id` is the raw `RepoId` (an xxhash of the
    /// repo identity key: git remote / git dir / dir name, LB.1), a Python
    /// `int`; Python has no label map for it yet. Report-only: no edge is
    /// emitted. Returns a list of dicts.
    fn contracts(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        to_py(py, contracts_json(&self.merged))
    }

    /// **contract_fields** (LE.10c / LE.10d): the declared fields of both
    /// sides of every contract pairing the graph holds, diffed by that
    /// format's own compatibility rules. Each row `{pairing, key, producer,
    /// consumer, status, tier, note, changes}`: `pairing` ∈ {schema_copy,
    /// topic, channel, route}; each side `{repo_id, qname, format, file,
    /// line}` (`line` 1-based, `repo_id` an exact `int`); `status` ∈
    /// {identical, compatible, breaking, unknown}; `tier` is `derived`; `note`
    /// says why a row is `unknown` (or `truncated`), else `None`; each change
    /// `{section, field, change, producer, consumer, rule, breaking}`. An Avro
    /// pair whose two directions judge differently is two rows. Report-only:
    /// nothing is added to the graph. Returns a list of dicts.
    fn contract_fields(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        to_py(py, contract_fields_json(&self.merged))
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
        let built = glia_engine::generate_one(root.to_str().expect("utf-8 temp path"));
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

    /// LE.10d: `contract_fields()` is transport only — pin the wiring. The
    /// committed proto-field-drift fixture (two repos' copies of one message)
    /// reaches Python as a JSON array holding its one schema_copy row, keys in
    /// the engine struct's order, the drifted field a breaking change. The
    /// verdicts are covered by engine/tests/contract_fields.rs.
    #[test]
    fn contract_fields_value_is_a_list_of_rows() {
        let empty = contract_fields_json(&MergedGraph::new(Vec::new())).expect("serialises");
        assert_eq!(empty, "[]");

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../bench/substrate-gap/fixtures/proto-field-drift");
        let dirs: Vec<String> = ["producer", "consumer"]
            .iter()
            .map(|d| {
                root.join(d)
                    .to_str()
                    .expect("utf-8 fixture path")
                    .to_string()
            })
            .collect();
        let merged = glia_engine::generate_many(&dirs)
            .expect("build")
            .merged;

        let json = contract_fields_json(&merged).expect("serialises");
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let rows = v.as_array().expect("a JSON array, not an object");
        assert_eq!(rows.len(), 1, "one SHARES_SCHEMA pair: {json}");
        let row = rows[0].as_object().expect("a row is an object");
        let key_at = |k: &str| json.find(&format!("\"{k}\":")).unwrap_or(usize::MAX);
        let order = [
            "pairing", "key", "producer", "consumer", "status", "tier", "note", "changes",
        ];
        assert!(
            order.iter().all(|k| row.contains_key(*k))
                && order.map(key_at).windows(2).all(|w| w[0] < w[1]),
            "row keys in engine field order: {json}"
        );
        assert_eq!(row["pairing"], "schema_copy", "{json}");
        assert_eq!(row["key"], "shop.v1.OrderCreated", "{json}");
        assert_eq!(row["status"], "breaking", "{json}");
        assert_eq!(row["producer"]["line"], 6, "1-based: {json}");
        assert!(row["producer"]["repo_id"].is_u64(), "an exact int: {json}");
        let changes = row["changes"].as_array().expect("changes is a list");
        let drift = changes
            .iter()
            .find(|c| c["field"] == "total_cents")
            .unwrap_or_else(|| panic!("total_cents drift missing: {json}"));
        assert_eq!(
            (
                &drift["producer"],
                &drift["consumer"],
                &drift["rule"],
                &drift["breaking"]
            ),
            (
                &serde_json::json!("int64"),
                &serde_json::json!("int32"),
                &serde_json::json!("proto_wire_type"),
                &serde_json::json!(true)
            ),
            "{json}"
        );
    }
}
