//! v0.4.13 latent-injection probe.
//!
//! Loads a quantized Qwen2 GGUF via a forked `quantized_qwen2` module and exposes
//! `forward_embeds` to skip the token-embedding lookup. Glia graph-node summaries
//! get pooled through the target model's own `tok_embeddings`, concatenated ahead
//! of the prompt, and fed into the transformer stack.
//!
//! Status: probe. Graduates to `latent/` at workspace root once the hook is verified.

pub mod forward_embeds;
pub mod qwen2_hacked;

pub use forward_embeds::ForwardEmbeds;
