//! `PassContext` — shared state threaded through the pipeline.
//!
//! Holds the (immutable) `RepoGraph` plus a string-keyed bag of intermediate
//! artifacts that passes produce and consume. Artifacts are stored as
//! `serde_json::Value` so they round-trip identically to the JSON-on-disk
//! shape the existing standalone bins use — making it cheap to swap a pass
//! for a CLI shell-out during incremental migration.

use std::collections::HashMap;

use anyhow::{Context, Result, anyhow};
use repo_graph_graph::RepoGraph;
use serde_json::Value;

pub struct PassContext {
    pub graph: RepoGraph,
    pub issue: String,
    pub test_patch: Option<String>,
    artifacts: HashMap<String, Value>,
}

impl PassContext {
    pub fn new(graph: RepoGraph, issue: String) -> Self {
        Self {
            graph,
            issue,
            test_patch: None,
            artifacts: HashMap::new(),
        }
    }

    pub fn with_test_patch(mut self, test_patch: String) -> Self {
        self.test_patch = Some(test_patch);
        self
    }

    /// Serialise `value` and store it under `key`. Overwrites any prior entry.
    pub fn put<T: serde::Serialize>(&mut self, key: &str, value: &T) -> Result<()> {
        let v = serde_json::to_value(value)
            .with_context(|| format!("serialise artifact {key}"))?;
        self.artifacts.insert(key.to_string(), v);
        Ok(())
    }

    /// Deserialise the artifact at `key` into `T`. Errors if missing or shape
    /// doesn't match.
    pub fn get<T: serde::de::DeserializeOwned>(&self, key: &str) -> Result<T> {
        let v = self
            .artifacts
            .get(key)
            .ok_or_else(|| anyhow!("artifact {key} not found in context"))?;
        serde_json::from_value(v.clone())
            .with_context(|| format!("deserialise artifact {key}"))
    }

    /// Borrow the raw JSON value at `key` without cloning. Returns `None` if
    /// the artifact hasn't been produced yet.
    pub fn get_raw(&self, key: &str) -> Option<&Value> {
        self.artifacts.get(key)
    }

    pub fn has(&self, key: &str) -> bool {
        self.artifacts.contains_key(key)
    }

    pub fn artifact_keys(&self) -> Vec<&str> {
        let mut keys: Vec<&str> = self.artifacts.keys().map(|s| s.as_str()).collect();
        keys.sort();
        keys
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use repo_graph_core::RepoId;
    use repo_graph_graph::build_python;
    use serde::{Deserialize, Serialize};

    fn ctx() -> PassContext {
        let graph = build_python(RepoId::from_canonical("test"), vec![]).unwrap();
        PassContext::new(graph, String::new())
    }

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct Sample {
        a: u32,
        b: String,
    }

    #[test]
    fn put_then_get_round_trips() {
        let mut c = ctx();
        let s = Sample { a: 7, b: "x".into() };
        c.put("k", &s).unwrap();
        let got: Sample = c.get("k").unwrap();
        assert_eq!(got, s);
    }

    #[test]
    fn get_missing_errors() {
        let c = ctx();
        let err = c.get::<Sample>("nope").unwrap_err();
        assert!(err.to_string().contains("nope"));
    }

    #[test]
    fn has_reflects_put() {
        let mut c = ctx();
        assert!(!c.has("k"));
        c.put("k", &42u32).unwrap();
        assert!(c.has("k"));
    }

    #[test]
    fn artifact_keys_sorted() {
        let mut c = ctx();
        c.put("zeta", &1u32).unwrap();
        c.put("alpha", &2u32).unwrap();
        c.put("mike", &3u32).unwrap();
        assert_eq!(c.artifact_keys(), vec!["alpha", "mike", "zeta"]);
    }
}
