//! JSONL output schema.
//!
//! Mirrors neuropil's `RecordedFlow` shape at
//! `/home/ivy/Code/neuropil/crates/neuropil-app/src/flow_replay.rs:36-46` so
//! the slice-2 panel can ingest with the existing `FlowReplayer` parsing
//! pattern. A new `LensStep` variant gets added to the consumer-side
//! `RecordedEvent` enum in slice 2; in slice 1 the JSONL stands alone.
//!
//! Wire format: line-delimited JSON, one `RecordedLens` per line.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

/// One line of the JSONL. `t_ms` is monotonic from binary start (matches
/// neuropil's recording convention) — not used by the lens itself, kept for
/// schema compatibility with `RecordedFlow`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedLens {
    pub t_ms: u64,
    pub event: LensEvent,
}

/// Tagged enum with the same `#[serde(tag, content)]` discriminator neuropil
/// uses, so slice 2 only needs to add `LensStep` as a new variant of
/// `RecordedEvent` without changing parsing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data")]
pub enum LensEvent {
    LensStep(LensStep),
}

/// One snapshot of the model's top-K prediction at one (layer, run, position).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LensStep {
    /// 0-indexed transformer block index. `0..n_layers`.
    pub layer: u32,
    /// "baseline" or "with_injection".
    pub run: String,
    /// Token index in the model's output stream. 0 = first generated token.
    pub position: u32,
    /// Top-K predictions sorted by descending logit.
    pub top_k: Vec<TopKEntry>,
    /// Optional: token id of the expected answer if known (extracted from
    /// SWE-bench gold patch). Set to None when no ground truth applies.
    pub gold_token_id: Option<u32>,
    /// Rank of `gold_token_id` in the full vocab logits at this layer.
    /// 0 = top-1, 1 = top-2, etc. None if `gold_token_id` is None.
    pub gold_rank: Option<u32>,
    /// Probability assigned to `gold_token_id` at this layer (post-softmax).
    pub gold_prob: Option<f32>,
    /// KL divergence (baseline || with_injection) at this (layer, position).
    /// Populated only on `with_injection` records.
    pub kl_vs_baseline: Option<f32>,
    /// SWE-bench instance id when this record is part of a batch aggregation
    /// (slice 1.6 `lens-batch`). Skipped from JSONL when None so single-
    /// instance lens runs stay schema-clean.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub instance_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopKEntry {
    pub token_id: u32,
    pub token_str: String,
    pub logit: f32,
    pub prob: f32,
}

/// Append-only writer. Caller calls `record` once per `(layer, run, position)`.
pub struct LensJsonlWriter {
    out: BufWriter<File>,
    start: std::time::Instant,
}

impl LensJsonlWriter {
    pub fn create(path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = File::create(path)?;
        Ok(Self {
            out: BufWriter::new(file),
            start: std::time::Instant::now(),
        })
    }

    pub fn record(&mut self, step: LensStep) -> anyhow::Result<()> {
        let t_ms = self.start.elapsed().as_millis() as u64;
        let line = serde_json::to_string(&RecordedLens {
            t_ms,
            event: LensEvent::LensStep(step),
        })?;
        self.out.write_all(line.as_bytes())?;
        self.out.write_all(b"\n")?;
        Ok(())
    }

    /// Write an opaque JSON value as a JSONL line. Used by lens-inject /
    /// lens-ablate to embed run-summary records alongside LensSteps.
    pub fn record_raw(&mut self, value: &serde_json::Value) -> anyhow::Result<()> {
        let line = serde_json::to_string(value)?;
        self.out.write_all(line.as_bytes())?;
        self.out.write_all(b"\n")?;
        Ok(())
    }

    pub fn finish(mut self) -> anyhow::Result<()> {
        self.out.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_through_serde() {
        let step = LensStep {
            layer: 12,
            run: "baseline".into(),
            position: 0,
            top_k: vec![
                TopKEntry {
                    token_id: 13049,
                    token_str: "getattr".into(),
                    logit: 12.34,
                    prob: 0.41,
                },
                TopKEntry {
                    token_id: 1521,
                    token_str: "self".into(),
                    logit: 11.82,
                    prob: 0.24,
                },
            ],
            gold_token_id: Some(13049),
            gold_rank: Some(0),
            gold_prob: Some(0.41),
            kl_vs_baseline: None,
            instance_id: None,
        };
        let rec = RecordedLens {
            t_ms: 42,
            event: LensEvent::LensStep(step.clone()),
        };
        let json = serde_json::to_string(&rec).unwrap();
        let back: RecordedLens = serde_json::from_str(&json).unwrap();
        assert_eq!(back.t_ms, 42);
        match back.event {
            LensEvent::LensStep(s) => {
                assert_eq!(s.layer, 12);
                assert_eq!(s.top_k.len(), 2);
                assert_eq!(s.top_k[0].token_str, "getattr");
                assert_eq!(s.gold_rank, Some(0));
            }
        }
    }

    #[test]
    fn discriminator_matches_neuropil_recorded_flow_shape() {
        // The wire format must use `kind`/`data` discriminator so neuropil's
        // FlowReplayer can union-parse a future LensStep variant alongside
        // Edge/Node without changing parsing semantics.
        let step = LensStep {
            layer: 0,
            run: "with_injection".into(),
            position: 0,
            top_k: vec![],
            gold_token_id: None,
            gold_rank: None,
            gold_prob: None,
            kl_vs_baseline: Some(0.123),
            instance_id: None,
        };
        let rec = RecordedLens {
            t_ms: 0,
            event: LensEvent::LensStep(step),
        };
        let json = serde_json::to_value(&rec).unwrap();
        let event = json.get("event").unwrap();
        assert_eq!(
            event.get("kind").and_then(|v| v.as_str()),
            Some("LensStep")
        );
        assert!(event.get("data").is_some());
    }
}
