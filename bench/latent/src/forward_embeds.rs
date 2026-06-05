//! The soft-prompt-prefix injection hook as a first-class trait.
//!
//! Every decoder-only transformer in candle's `quantized_*` family follows the
//! same sealed template: `from_gguf(...)` + `forward(input_ids, index_pos)`,
//! with `tok_embeddings` held private. To inject glia graph vectors into the
//! hidden state we fork the module, expose the embedding layer, and skip the
//! token-lookup. Since the fallback substrate (Llama 3.1 8B) will go through
//! the same surgery, we ratify the shape here so the probe harness is
//! substrate-agnostic.
//!
//! Surface is deliberately minimum-viable. Do NOT add `forward_embeds_with_cache`,
//! `forward_embeds_batched`, or `forward_embeds_streaming` until a concrete
//! caller needs them — traits calcify once there are two implementers.

use candle_core::{Result, Tensor};

/// A quantized decoder-only transformer that exposes its own `tok_embeddings`
/// layer and accepts pre-embedded input.
pub trait ForwardEmbeds {
    /// Token IDs `[B, T]` → embeddings `[B, T, D]`. Same tensor that the
    /// model's `forward` feeds into its first transformer layer.
    fn embed(&self, tokens: &Tensor) -> Result<Tensor>;

    /// Run the transformer stack on pre-embedded input `[B, T, D]`, skipping
    /// the token-embedding lookup. Returns last-token logits `[B, vocab]`.
    ///
    /// `index_pos` is the KV-cache position offset — 0 on the first call,
    /// `prev_seq_len` on continuations.
    fn forward_embeds(&mut self, embeds: &Tensor, index_pos: usize) -> Result<Tensor>;
}
