//! Runtime trait + llama.cpp FFI implementation.
//!
//! The trait isolates the unsafe FFI from the rest of the binary so the lens
//! math, JSONL, PNG, and ASCII rendering can be tested against `FakeRuntime`
//! without the ~10-min llama-cpp-sys-2 compile.
//!
//! ## llama.cpp hook strategy
//!
//! llama.cpp exposes `cb_eval` on `llama_context_params` — a per-op callback
//! that fires during graph evaluation with the tensor pointer. See
//! `common/debug.cpp::common_debug_cb_eval` in llama.cpp for the canonical
//! pattern. We use this WITHOUT patching llama.cpp: the cb_eval callback
//! filters by tensor name, capturing:
//!
//!   - `l_out-{N}` for each N — the residual stream after transformer block N
//!     (verified against `src/models/qwen2.cpp` cb() calls).
//!   - On the first eval of `result_norm`, capture `src[1]->data` as the
//!     `output_norm.weight` static tensor (only the weight, not the activation).
//!     We use the tensor name on `src[1]` to confirm it's the norm weight.
//!   - On the first eval of `result_output`, capture the unembed weight
//!     similarly (it's whichever src has name `"output.weight"` or matches
//!     the tied-embedding fallback `"token_embd.weight"`).
//!
//! All captures happen on the FIRST eval; weights don't change between runs.

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result, anyhow};

use crate::lens::{
    AutoregressiveCaptures, ForwardPassCaptures, LayerCapture, PositionCapture, UnembedHead,
};

/// Minimum surface every backend must provide.
pub trait LensRuntime {
    /// Load model weights + tokenizer. Returns a handle for subsequent calls.
    fn load(weights: &Path, tokenizer: &Path) -> Result<Self>
    where
        Self: Sized;

    /// Tokenize input text into model token ids.
    fn tokenize(&self, text: &str) -> Result<Vec<u32>>;

    /// Detokenize one id into a printable string. Used by the JSONL emitter
    /// to surface `token_str` in TopKEntry.
    fn detokenize(&self, id: u32) -> String;

    /// Number of transformer blocks.
    fn n_layers(&self) -> usize;

    /// Vocabulary size.
    fn n_vocab(&self) -> usize;

    /// Embedding dimension.
    fn n_embd(&self) -> usize;

    /// Unembed head (norm + output weight). Extracted once at load time.
    fn unembed_head(&self) -> &UnembedHead;

    /// Run a forward pass over `tokens`, capturing the residual stream at
    /// every transformer block at every requested `output_position`.
    /// `run_label` is copied into the resulting `ForwardPassCaptures.run`
    /// for downstream tagging. Positions out of `tokens.len()` are skipped
    /// silently (caller chose the range).
    fn forward_capture(
        &mut self,
        tokens: &[u32],
        output_positions: &[u32],
        run_label: &str,
    ) -> Result<ForwardPassCaptures>;

    /// Cycle 0.4 lens — autoregressive generation with per-step residual
    /// capture. Greedy sampling (argmax over final-layer logits) for v1.
    ///
    /// At each generation step:
    ///   1. read final-layer logits at the last position
    ///   2. argmax → next token id
    ///   3. append to running sequence; feed back through model (single-token
    ///      decode for incremental KV-cache reuse where possible)
    ///   4. cb_eval captures l_out-N rows for the new token at every layer
    ///   5. record `PositionCapture { position: step_idx, layers: [...] }`
    ///   6. if next token == EOS or step_idx == max_new, stop
    ///
    /// Returns the generated token sequence + per-step per-layer residuals.
    /// This is what the slice-1.5 prompt-position lens couldn't see — the
    /// cycle 0.3 directive steers DURING DECODING, so differentiation
    /// between baseline / iter5 conditions is at generated tokens 1..N,
    /// not at the prompt-encoding stage.
    fn forward_generate(
        &mut self,
        prompt_tokens: &[u32],
        max_new: u32,
        eos_token_id: Option<u32>,
        run_label: &str,
    ) -> Result<AutoregressiveCaptures>;

    /// Cycle 0.6 spitball B3 — autoregressive generation WITH residual-stream
    /// injection at the decision band (L25-27 per cycle 0.4 lens evidence).
    ///
    /// At each generation step, for each layer L in `spec.inject_layers`, add
    /// `spec.target_embed * spec.alpha` to the residual stream at every
    /// position p in `spec.inject_positions`, then continue the forward pass.
    /// The modified residual flows through subsequent layers + the unembed
    /// head; the resulting logits → argmax produces the steered token.
    ///
    /// FakeRuntime: writes the injection into its synthetic residual tensors
    /// (testable end-to-end without llama.cpp).
    ///
    /// LlamaCppRuntime: writes via ggml_backend_tensor_set in cb_eval. On
    /// `l_out-N` for N in inject_layers, reads the tensor, modifies the rows
    /// at inject_positions in scratch buffer, calls ggml_backend_tensor_set
    /// to push the modified data back, returns true to continue evaluation.
    /// Subsequent ops in the compute graph (L26 reading L25's output, etc.)
    /// then see the injected residual. cb_eval return value semantics: true
    /// = continue, false = abort the forward pass. Modification is via
    /// tensor_set, NOT the return value.
    ///
    /// Returns the same AutoregressiveCaptures shape as forward_generate so
    /// the lens-math + JSONL pipeline can ingest it identically.
    fn forward_generate_with_inject(
        &mut self,
        prompt_tokens: &[u32],
        max_new: u32,
        eos_token_id: Option<u32>,
        run_label: &str,
        spec: &InjectSpec,
    ) -> Result<AutoregressiveCaptures>;

    /// D2 — toggle attention-output capture. When true, subsequent forward
    /// calls populate per-layer attention norms accessible via
    /// `attention_norms()`. Saves the per-row L2 norms only (one f32 per
    /// token position per layer) to keep memory bounded.
    fn set_capture_attention(&mut self, on: bool);

    /// D2 — pull per-layer attention norms captured during the most recent
    /// forward pass. Returns (HashMap<layer, Vec<f32>>, tensor_names_seen).
    /// Empty when capture_attention was off OR no attention tensors fired.
    fn attention_norms(&self) -> AttentionCaptures;

    /// C3 — attach an AttentionBiasSpec to subsequent forward passes. When
    /// set, the cb_eval trampoline biases the `attn_out-N` / `kqv_out-N`
    /// tensor at decision layers by adding `alpha * target_embed`. Pass None
    /// to detach. Independent of `forward_generate_with_inject`'s
    /// `InjectSpec` — both can be active in the same forward pass.
    ///
    /// Default no-op (FakeRuntime ignores). LlamaCppRuntime stores it in
    /// CbState so the trampoline can read it without re-locking.
    fn set_attn_bias(&mut self, _spec: Option<AttentionBiasSpec>) {}
}

/// D2 — attention output captured per layer per token-position. Same
/// row order as the residual `per_layer` so callers can correlate by index.
#[derive(Debug, Clone, Default)]
pub struct AttentionCaptures {
    /// Layer index → per-row L2 norm of attention output tensor (one f32
    /// per token position, appended across ubatch firings).
    pub norms_per_layer: std::collections::HashMap<u32, Vec<f32>>,
    /// Diagnostic: which attention-stage tensor names actually fired during
    /// the forward pass. Tells the caller whether `kqv_out-N` /
    /// `kq_soft_max_ext-N` / etc was the active name in this model arch.
    pub tensor_names_seen: std::collections::BTreeSet<String>,
}

/// Injection parameters for `forward_generate_with_inject`. Lives in
/// runtime so both FakeRuntime + LlamaCppRuntime can share the shape.
#[derive(Debug, Clone)]
pub struct InjectSpec {
    /// Target embedding to add to the residual stream. Length must equal
    /// `runtime.n_embd()`. Typically the model's own token-embedding row for
    /// the steering token (e.g. the first token of the target qname).
    pub target_embed: Vec<f32>,
    /// Layer indices to inject at (0-indexed). Cycle 0.4 evidence: 25..=27
    /// on Qwen 2.5 Coder 7B.
    pub inject_layers: Vec<u32>,
    /// Generated-token positions to inject at (0-indexed). Cycle 0.4
    /// identified positions 23-25 as the marshmallow decision window.
    pub inject_positions: Vec<u32>,
    /// Mix strength. 0.0 = no-op; 1.0 = full target embed replaces a unit of
    /// existing residual. Start at 0.3 per the plan.
    pub alpha: f32,
}

/// C3 — direct attention-output biasing parameters. When attached to a
/// forward_generate_with_inject call alongside an InjectSpec (or alone), the
/// cb_eval trampoline modifies the `attn_out-N` / `kqv_out-N` tensor at
/// decision layers by adding `alpha * target_embed` to the row corresponding
/// to the current generation position. Functionally this is "direct attention
/// biasing": regardless of which keys the attention attended to, an extra
/// fixed contribution in the target direction enters the residual stream.
///
/// Separate from InjectSpec because B3 hooks `l_out-N` (post-layer residual)
/// while C3 hooks `attn_out-N` (intra-layer attention output, BEFORE the FFN
/// in the same layer). Both can be active in the same forward pass.
#[derive(Debug, Clone)]
pub struct AttentionBiasSpec {
    /// Target direction to add. Length = `runtime.n_embd()`. Same shape as
    /// `InjectSpec::target_embed` — typically the token embedding for the
    /// first token of the qname the directive named as PRIMARY target.
    pub target_embed: Vec<f32>,
    /// Layer indices at whose `attn_out-N` to bias (0-indexed). Cycle 0.4:
    /// 25..=27 on Qwen 2.5 Coder 7B.
    pub bias_layers: Vec<u32>,
    /// Generated-token positions to bias at (0-indexed). Cycle 0.4: 23..=25
    /// is the marshmallow decision window. Subset is fine.
    pub bias_positions: Vec<u32>,
    /// Mix strength. Smaller than InjectSpec.alpha because attn_out is added
    /// straight into the residual; 0.1 is a reasonable starting point.
    pub alpha: f32,
}

// ============================================================================
// FakeRuntime — synthetic data, no llama.cpp.
// ============================================================================

/// Deterministic synthetic runtime for testing the lens pipeline end-to-end
/// without a real model. Constants chosen so the math exercises the code
/// paths but the numbers stay readable in test failure messages.
pub struct FakeRuntime {
    n_layers: usize,
    n_vocab: usize,
    n_embd: usize,
    head: UnembedHead,
    token_strs: HashMap<u32, String>,
}

impl FakeRuntime {
    pub fn new(n_layers: usize, n_vocab: usize, n_embd: usize) -> Self {
        let head = UnembedHead {
            output_norm_weight: vec![1.0; n_embd],
            // Identity-ish embedding: token v's row is one-hot at e=v%n_embd.
            output_weight: {
                let mut w = vec![0.0f32; n_vocab * n_embd];
                for v in 0..n_vocab {
                    w[v * n_embd + (v % n_embd)] = 1.0;
                }
                w
            },
            n_embd,
            n_vocab,
            eps: 1e-6,
        };
        let token_strs = (0..n_vocab as u32)
            .map(|i| (i, format!("tok{}", i)))
            .collect();
        Self {
            n_layers,
            n_vocab,
            n_embd,
            head,
            token_strs,
        }
    }
}

impl LensRuntime for FakeRuntime {
    fn load(_weights: &Path, _tokenizer: &Path) -> Result<Self> {
        Ok(Self::new(8, 32, 16))
    }
    fn tokenize(&self, text: &str) -> Result<Vec<u32>> {
        Ok(text.bytes().take(64).map(|b| (b as u32) % (self.n_vocab as u32)).collect())
    }
    fn detokenize(&self, id: u32) -> String {
        self.token_strs.get(&id).cloned().unwrap_or_else(|| format!("?{id}"))
    }
    fn n_layers(&self) -> usize { self.n_layers }
    fn n_vocab(&self) -> usize { self.n_vocab }
    fn n_embd(&self) -> usize { self.n_embd }
    fn unembed_head(&self) -> &UnembedHead { &self.head }
    fn forward_capture(
        &mut self,
        tokens: &[u32],
        output_positions: &[u32],
        run_label: &str,
    ) -> Result<ForwardPassCaptures> {
        // Synthetic residuals: each layer biases progressively toward a
        // single token id derived from the input + position. Lets tests
        // assert that top-1 sharpens with depth AND that different
        // positions land at different tokens.
        let base = tokens.last().copied().unwrap_or(0);
        let positions = output_positions
            .iter()
            .map(|&pos| {
                let target = (base + pos) % (self.n_vocab as u32);
                let target_dim = (target as usize) % self.n_embd;
                let layers = (0..self.n_layers)
                    .map(|l| {
                        let mut r = vec![0.0f32; self.n_embd];
                        let bias = 1.0 + (l as f32) * 2.0;
                        r[target_dim] = bias;
                        LayerCapture {
                            layer: l as u32,
                            residual: r,
                        }
                    })
                    .collect();
                PositionCapture {
                    position: pos,
                    layers,
                }
            })
            .collect();
        Ok(ForwardPassCaptures {
            run: run_label.to_string(),
            positions,
        })
    }

    fn forward_generate(
        &mut self,
        prompt_tokens: &[u32],
        max_new: u32,
        eos_token_id: Option<u32>,
        run_label: &str,
    ) -> Result<AutoregressiveCaptures> {
        // Deterministic synthetic generation:
        //   token_at_step(i) = (last_prompt_token + i + 1) % n_vocab
        // EOS fires when token equals eos_token_id (if Some). Tests can wire
        // an early EOS via small n_vocab.
        let mut generated_tokens: Vec<u32> = Vec::new();
        let mut steps: Vec<PositionCapture> = Vec::new();
        let mut stopped_on_eos = false;
        let mut last = prompt_tokens.last().copied().unwrap_or(0);
        for step in 0..max_new {
            let next = (last + 1) % (self.n_vocab as u32);
            if eos_token_id == Some(next) {
                stopped_on_eos = true;
                break;
            }
            generated_tokens.push(next);
            // Per-step residual: bias toward the GENERATED token, sharpening
            // with depth — mirrors the real lens shape so downstream code
            // exercising AutoregressiveCaptures works the same in both.
            let target_dim = (next as usize) % self.n_embd;
            let layers = (0..self.n_layers)
                .map(|l| {
                    let mut r = vec![0.0f32; self.n_embd];
                    r[target_dim] = 1.0 + (l as f32) * 2.0;
                    LayerCapture {
                        layer: l as u32,
                        residual: r,
                    }
                })
                .collect();
            steps.push(PositionCapture {
                position: step,
                layers,
            });
            last = next;
        }
        Ok(AutoregressiveCaptures {
            run: run_label.to_string(),
            generated_tokens,
            steps,
            stopped_on_eos,
        })
    }

    fn set_capture_attention(&mut self, _on: bool) {
        // FakeRuntime has no attention; no-op.
    }

    fn attention_norms(&self) -> AttentionCaptures {
        AttentionCaptures::default()
    }

    fn forward_generate_with_inject(
        &mut self,
        prompt_tokens: &[u32],
        max_new: u32,
        eos_token_id: Option<u32>,
        run_label: &str,
        spec: &InjectSpec,
    ) -> Result<AutoregressiveCaptures> {
        // Synthetic analog of cb_eval write: run the standard fake pass, then
        // add spec.target_embed * alpha to the residual stream at every
        // (layer, position) in spec.inject_{layers,positions}.
        if spec.target_embed.len() != self.n_embd {
            anyhow::bail!(
                "inject target_embed len {} != n_embd {}",
                spec.target_embed.len(),
                self.n_embd
            );
        }
        let mut caps = self.forward_generate(prompt_tokens, max_new, eos_token_id, run_label)?;
        for step in caps.steps.iter_mut() {
            if !spec.inject_positions.contains(&step.position) {
                continue;
            }
            for lc in step.layers.iter_mut() {
                if !spec.inject_layers.contains(&lc.layer) {
                    continue;
                }
                for (r, t) in lc.residual.iter_mut().zip(spec.target_embed.iter()) {
                    *r += spec.alpha * *t;
                }
            }
        }
        Ok(caps)
    }
}

// ============================================================================
// LlamaCppRuntime — real backend via llama-cpp-sys-2.
// ============================================================================

/// State that the cb_eval C callback writes into. Lives behind a Mutex inside
/// `LlamaCppRuntime` and is reset between forward passes.
struct CbState {
    /// Layer index → residual data, concatenated across all ubatch firings.
    /// llama.cpp processes the prompt in ubatches (default 512); the cb_eval
    /// for `l_out-N` fires once per ubatch with that ubatch's rows. We APPEND
    /// each firing to preserve the full sequence in batch-index order.
    per_layer: HashMap<u32, Vec<f32>>,
    /// Static weights captured on first sighting.
    output_norm_weight: Option<Vec<f32>>,
    output_weight: Option<Vec<f32>>,
    /// Hparams discovered from captures.
    n_embd: usize,
    n_vocab: usize,
    /// Scratch host buffer to avoid per-callback allocation.
    scratch: Vec<u8>,
    /// B3 injection state. When Some + inject_active_position is Some, the
    /// trampoline modifies the l_out-N tensor at the matched layer/position
    /// before appending to per_layer + before subsequent ops read from it.
    inject_spec: Option<InjectSpec>,
    /// The generated-token index currently being processed. Set by
    /// `forward_generate_with_inject` before each single-token decode call.
    /// None during prompt prefill or non-inject forward passes.
    inject_active_position: Option<u32>,
    /// D2 (attention-bloat-ratio metric, cycle 0.6 spitball): when enabled,
    /// capture per-row L2 norms of the attention output tensor at each
    /// layer. cb_eval recognizes Qwen2 attention-stage tensor names
    /// (kqv_out-N, attn_out-N, kq_soft_max_ext-N) and stores per-row norms
    /// (one f32 per ubatch row) keyed by layer. Saves memory vs storing
    /// full tensors. When `capture_attention` is false the path is
    /// short-circuited.
    capture_attention: bool,
    /// Per-layer attention output L2 norms. Same row-order as `per_layer`
    /// so callers can correlate by index.
    attn_norms_per_layer: HashMap<u32, Vec<f32>>,
    /// Set of attention-stage tensor names seen during the current forward
    /// (diagnostic; first-pass tells the caller which tensor names are
    /// actually populated by the running Qwen2 graph). Cleared on reset.
    attn_tensor_names_seen: std::collections::BTreeSet<String>,
    /// C3 direct attention bias state. When Some + inject_active_position is
    /// Some, the trampoline modifies the `attn_out-N` / `kqv_out-N` tensor at
    /// the matched layer/position by adding `alpha * target_embed`. Operates
    /// independently of `inject_spec` (B3); both can be active simultaneously.
    attn_bias_spec: Option<AttentionBiasSpec>,
}

impl CbState {
    fn new() -> Self {
        Self {
            per_layer: HashMap::new(),
            output_norm_weight: None,
            output_weight: None,
            n_embd: 0,
            n_vocab: 0,
            scratch: Vec::with_capacity(64 * 1024),
            inject_spec: None,
            inject_active_position: None,
            capture_attention: false,
            attn_norms_per_layer: HashMap::new(),
            attn_tensor_names_seen: std::collections::BTreeSet::new(),
            attn_bias_spec: None,
        }
    }
    fn reset_for_forward(&mut self) {
        self.per_layer.clear();
        self.attn_norms_per_layer.clear();
        self.attn_tensor_names_seen.clear();
        self.inject_active_position = None;
    }
}

#[cfg(feature = "real")]
pub struct LlamaCppRuntime {
    // SAFETY: All llama_* pointers are valid until Drop runs. They are
    // accessed only on the thread that owns this struct.
    model: *mut llama_cpp_sys_2::llama_model,
    ctx: *mut llama_cpp_sys_2::llama_context,
    vocab: *const llama_cpp_sys_2::llama_vocab,
    tokenizer: tokenizers::Tokenizer,
    head: UnembedHead,
    n_layers: usize,
    cb_state: Box<Mutex<CbState>>,
    // !Send + !Sync marker. Negative trait bounds aren't stable Rust, so we
    // use `*const ()` (not Send/Sync) via PhantomData. llama_context is not
    // thread-safe; sending across threads would race the kv-cache.
    _not_send_sync: std::marker::PhantomData<*const ()>,
}

#[cfg(feature = "real")]
unsafe extern "C" fn cb_eval_trampoline(
    t: *mut llama_cpp_sys_2::ggml_tensor,
    ask: bool,
    user_data: *mut std::os::raw::c_void,
) -> bool {
    if user_data.is_null() {
        return true;
    }
    let state_ptr = user_data as *const Mutex<CbState>;
    let state_mtx = unsafe { &*state_ptr };

    if ask {
        return true;
    }
    if t.is_null() {
        return true;
    }

    let name_ptr = unsafe { (*t).name.as_ptr() };
    let name = unsafe { CStr::from_ptr(name_ptr) }
        .to_str()
        .unwrap_or("");

    let l_out_layer = parse_l_out_layer(name);
    let is_result_norm = name == "result_norm";
    let is_result_output = name == "result_output";
    // D2 — attention-stage tensor names per Qwen2's graph. The exact name
    // depends on the model arch's compute graph; we accept any of these and
    // record which fired in attn_tensor_names_seen for diagnostic.
    let attn_layer = parse_attn_layer(name);

    if l_out_layer.is_none() && !is_result_norm && !is_result_output && attn_layer.is_none() {
        return true;
    }

    let mut state = match state_mtx.lock() {
        Ok(g) => g,
        Err(_) => return true,
    };

    if let Some(layer) = l_out_layer {
        // l_out is an f32 activation tensor; extract via backend_tensor_get.
        // APPEND to any prior firing for this layer (llama.cpp fires once per
        // ubatch; concatenation in callback order = full sequence).
        let nbytes = unsafe { llama_cpp_sys_2::ggml_nbytes(t) };
        let n_elems = nbytes / std::mem::size_of::<f32>();
        state.scratch.resize(nbytes, 0);
        unsafe {
            llama_cpp_sys_2::ggml_backend_tensor_get(
                t,
                state.scratch.as_mut_ptr() as *mut std::os::raw::c_void,
                0,
                nbytes,
            );
        }

        // B3: if inject_spec is active AND this layer + position match, modify
        // the tensor data in scratch and push it back via ggml_backend_tensor_set
        // BEFORE appending to per_layer (so per_layer reflects what subsequent
        // layers actually see). Modification: add target_embed * alpha
        // element-wise to the row(s) at inject_positions in this ubatch.
        let do_inject = state
            .inject_spec
            .as_ref()
            .map(|s| s.inject_layers.contains(&layer))
            .unwrap_or(false)
            && state.inject_active_position.is_some();
        if do_inject {
            // Borrow-checker dance: pull values out before mutating scratch.
            let spec = state.inject_spec.clone().unwrap();
            let active_pos = state.inject_active_position.unwrap();
            if spec.inject_positions.contains(&active_pos)
                && spec.target_embed.len() <= n_elems
            {
                let scratch_ptr = state.scratch.as_mut_ptr() as *mut f32;
                let n_embd = spec.target_embed.len();
                let n_rows = n_elems / n_embd;
                if n_rows > 0 {
                    // The ubatch's last row is the current decode token. Modify it.
                    let row_offset = (n_rows - 1) * n_embd;
                    unsafe {
                        let row = std::slice::from_raw_parts_mut(scratch_ptr.add(row_offset), n_embd);
                        for (r, e) in row.iter_mut().zip(spec.target_embed.iter()) {
                            *r += spec.alpha * *e;
                        }
                    }
                    // Push modified data back to the backend tensor.
                    unsafe {
                        llama_cpp_sys_2::ggml_backend_tensor_set(
                            t,
                            state.scratch.as_ptr() as *const std::os::raw::c_void,
                            0,
                            nbytes,
                        );
                    }
                }
            }
        }

        let data: &[f32] = unsafe {
            std::slice::from_raw_parts(state.scratch.as_ptr() as *const f32, n_elems)
        };
        state.per_layer.entry(layer).or_default().extend_from_slice(data);
    } else if is_result_norm {
        // src[1] of norm op is the norm weight (named "output_norm.weight").
        // Capture once, type-aware.
        if state.output_norm_weight.is_none() {
            if let Some(w) = capture_src_by_name(t, "output_norm.weight") {
                state.output_norm_weight = Some(w);
            }
        }
    } else if is_result_output {
        // src of matmul includes "output.weight" (or tied "token_embd.weight").
        // Capture once, type-aware.
        if state.output_weight.is_none() {
            if let Some(w) = capture_src_by_name(t, "output.weight") {
                state.output_weight = Some(w);
            } else if let Some(w) = capture_src_by_name(t, "token_embd.weight") {
                state.output_weight = Some(w);
            }
        }
    } else if let Some(layer) = attn_layer {
        // C3 — direct attention-output bias. When attn_bias_spec is set,
        // intercept `attn_out-N` / `kqv_out-N` at the matched layer and add
        // `alpha * target_embed` to the row corresponding to the current
        // generation position. Independent of capture_attention.
        let do_bias = state
            .attn_bias_spec
            .as_ref()
            .map(|s| s.bias_layers.contains(&layer))
            .unwrap_or(false)
            && state.inject_active_position.is_some()
            // Only bias on attn_out-style tensors (post-projection, in residual
            // space). kq_soft_max_ext / soft_max are attention weights and
            // their tensor shape isn't compatible with target_embed addition.
            && (name.starts_with("attn_out-")
                || name.starts_with("kqv_out-")
                || name.starts_with("kqv_merged_cont-"));
        if do_bias {
            let dtype = unsafe { (*t).type_ };
            if dtype == llama_cpp_sys_2::GGML_TYPE_F32 {
                let spec = state.attn_bias_spec.clone().unwrap();
                let active_pos = state.inject_active_position.unwrap();
                if spec.bias_positions.contains(&active_pos)
                    && !spec.target_embed.is_empty()
                {
                    let nbytes = unsafe { llama_cpp_sys_2::ggml_nbytes(t) };
                    let n_elems = nbytes / std::mem::size_of::<f32>();
                    state.scratch.resize(nbytes, 0);
                    unsafe {
                        llama_cpp_sys_2::ggml_backend_tensor_get(
                            t,
                            state.scratch.as_mut_ptr() as *mut std::os::raw::c_void,
                            0,
                            nbytes,
                        );
                    }
                    let n_embd = spec.target_embed.len();
                    if n_embd <= n_elems && n_elems % n_embd == 0 {
                        let n_rows = n_elems / n_embd;
                        let row_offset = (n_rows - 1) * n_embd;
                        let scratch_ptr = state.scratch.as_mut_ptr() as *mut f32;
                        unsafe {
                            let row = std::slice::from_raw_parts_mut(
                                scratch_ptr.add(row_offset),
                                n_embd,
                            );
                            for (r, e) in row.iter_mut().zip(spec.target_embed.iter()) {
                                *r += spec.alpha * *e;
                            }
                            llama_cpp_sys_2::ggml_backend_tensor_set(
                                t,
                                state.scratch.as_ptr() as *const std::os::raw::c_void,
                                0,
                                nbytes,
                            );
                        }
                    }
                }
            }
        }
        // D2 — attention-output capture. Only when capture_attention is on
        // (saves ~10-20ms per forward in the default path).
        if state.capture_attention {
            state.attn_tensor_names_seen.insert(name.to_string());
            let nbytes = unsafe { llama_cpp_sys_2::ggml_nbytes(t) };
            let n_elems = nbytes / std::mem::size_of::<f32>();
            // Qwen2's `kqv_out-N` is f32 (post-projection attention output);
            // `kq_soft_max_ext-N` is f32 (softmax weights, shape varies).
            // For unsupported dtypes skip silently.
            let dtype = unsafe { (*t).type_ };
            if dtype != llama_cpp_sys_2::GGML_TYPE_F32 {
                return true;
            }
            state.scratch.resize(nbytes, 0);
            unsafe {
                llama_cpp_sys_2::ggml_backend_tensor_get(
                    t,
                    state.scratch.as_mut_ptr() as *mut std::os::raw::c_void,
                    0,
                    nbytes,
                );
            }
            let data: &[f32] = unsafe {
                std::slice::from_raw_parts(state.scratch.as_ptr() as *const f32, n_elems)
            };
            // Row-wise L2 norm. For attention output tensors (kqv_out, attn_out)
            // the shape is [n_embd, n_tokens]; columns are tokens, so reshape
            // by stride n_embd. For softmax tensors [n_head, n_dst, n_src],
            // we collapse to a single norm per "row" of the leading dim.
            // Heuristic: use ne[1] (second dim) as row count, fall back to
            // 1 if not available.
            let ne0 = unsafe { (*t).ne[0] } as usize;
            let ne1 = unsafe { (*t).ne[1] } as usize;
            let row_len = ne0.max(1);
            let n_rows = if ne1 > 0 { ne1 } else { 1 };
            let buf = state.attn_norms_per_layer.entry(layer).or_default();
            for r in 0..n_rows {
                let start = r * row_len;
                let end = (start + row_len).min(n_elems);
                if start >= n_elems { break; }
                let s: f32 = data[start..end].iter().map(|x| x * x).sum();
                buf.push(s.sqrt());
            }
        }
    }
    true
}

#[cfg(feature = "real")]
fn parse_l_out_layer(name: &str) -> Option<u32> {
    let suffix = name.strip_prefix("l_out-")?;
    suffix.parse::<u32>().ok()
}

#[cfg(feature = "real")]
fn parse_attn_layer(name: &str) -> Option<u32> {
    // Try the Qwen2 attention-stage names. The exact name depends on
    // llama.cpp's compute graph; we accept any and record which fired in
    // attn_tensor_names_seen for downstream diagnostic.
    for prefix in &[
        "kqv_out-", "attn_out-", "kqv_merged_cont-",
        "kq_soft_max_ext-", "soft_max-",
    ] {
        if let Some(suffix) = name.strip_prefix(*prefix) {
            // Strip a possible "-pos-N" suffix if present.
            let layer_str = suffix.split('-').next().unwrap_or(suffix);
            if let Ok(n) = layer_str.parse::<u32>() {
                return Some(n);
            }
        }
    }
    None
}

/// Walk every src of `t`, return the first one whose name matches `target`,
/// dequantizing if necessary via `ggml_get_type_traits().to_float`.
#[cfg(feature = "real")]
fn capture_src_by_name(
    t: *mut llama_cpp_sys_2::ggml_tensor,
    target: &str,
) -> Option<Vec<f32>> {
    if t.is_null() {
        return None;
    }
    // GGML_MAX_SRC = 10; the constant isn't exported via bindgen, baked in.
    for idx in 0..10usize {
        let src_ptr = unsafe { (*t).src[idx] };
        if src_ptr.is_null() {
            continue;
        }
        let src_name_ptr = unsafe { (*src_ptr).name.as_ptr() };
        let src_name = unsafe { CStr::from_ptr(src_name_ptr) }
            .to_str()
            .unwrap_or("");
        if src_name == target {
            return tensor_to_f32_owned(src_ptr);
        }
    }
    None
}

/// Read a tensor as Vec<f32>, dequantizing if needed. Element count comes
/// from `ne[0]*ne[1]*ne[2]*ne[3]`, NOT from `ggml_nbytes / 4` (which is
/// wrong for quantized types where each element is <4 bytes).
#[cfg(feature = "real")]
fn tensor_to_f32_owned(t: *const llama_cpp_sys_2::ggml_tensor) -> Option<Vec<f32>> {
    if t.is_null() {
        return None;
    }
    let (n_elems, type_) = unsafe {
        let ne = (*t).ne;
        let n: i64 = ne[0] * ne[1] * ne[2] * ne[3];
        (n as usize, (*t).type_)
    };
    if n_elems == 0 {
        return None;
    }

    // Stage 1: get raw bytes into a scratch buffer.
    let nbytes = unsafe { llama_cpp_sys_2::ggml_nbytes(t as *mut _) };
    let mut raw: Vec<u8> = vec![0; nbytes];
    let is_host = unsafe { llama_cpp_sys_2::ggml_backend_buffer_is_host((*t).buffer) };
    if is_host {
        let src = unsafe { (*t).data as *const u8 };
        if src.is_null() {
            unsafe {
                llama_cpp_sys_2::ggml_backend_tensor_get(
                    t as *mut _,
                    raw.as_mut_ptr() as *mut std::os::raw::c_void,
                    0,
                    nbytes,
                );
            }
        } else {
            unsafe {
                std::ptr::copy_nonoverlapping(src, raw.as_mut_ptr(), nbytes);
            }
        }
    } else {
        unsafe {
            llama_cpp_sys_2::ggml_backend_tensor_get(
                t as *mut _,
                raw.as_mut_ptr() as *mut std::os::raw::c_void,
                0,
                nbytes,
            );
        }
    }

    // Stage 2: convert to f32 via type_traits.to_float.
    let mut out = vec![0.0f32; n_elems];
    let f32_type = llama_cpp_sys_2::GGML_TYPE_F32;
    if type_ == f32_type {
        // Direct reinterpret.
        if raw.len() < n_elems * std::mem::size_of::<f32>() {
            return None;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(
                raw.as_ptr() as *const f32,
                out.as_mut_ptr(),
                n_elems,
            );
        }
        return Some(out);
    }

    let traits_ptr = unsafe { llama_cpp_sys_2::ggml_get_type_traits(type_) };
    if traits_ptr.is_null() {
        return None;
    }
    let to_float = unsafe { (*traits_ptr).to_float };
    let Some(to_float_fn) = to_float else {
        return None;
    };
    unsafe {
        to_float_fn(
            raw.as_ptr() as *const std::os::raw::c_void,
            out.as_mut_ptr(),
            n_elems as i64,
        );
    }
    Some(out)
}

#[cfg(feature = "real")]
impl LensRuntime for LlamaCppRuntime {
    fn load(weights: &Path, tokenizer: &Path) -> Result<Self> {
        unsafe {
            llama_cpp_sys_2::llama_backend_init();
            let model_params = llama_cpp_sys_2::llama_model_default_params();
            let path_c = CString::new(weights.to_string_lossy().as_bytes())?;
            let model = llama_cpp_sys_2::llama_model_load_from_file(
                path_c.as_ptr(),
                model_params,
            );
            if model.is_null() {
                anyhow::bail!("llama_model_load_from_file returned NULL for {weights:?}");
            }
            let vocab = llama_cpp_sys_2::llama_model_get_vocab(model);
            let n_vocab = llama_cpp_sys_2::llama_vocab_n_tokens(vocab) as usize;
            let n_embd = llama_cpp_sys_2::llama_model_n_embd(model) as usize;
            let n_layers = llama_cpp_sys_2::llama_model_n_layer(model) as usize;

            let cb_state = Box::new(Mutex::new(CbState::new()));
            let cb_state_ptr: *const Mutex<CbState> = &*cb_state;

            let mut ctx_params = llama_cpp_sys_2::llama_context_default_params();
            // Bigger ctx — astropy/django prefixes + summary injections push
            // 16k+ tokens. Qwen 2.5 is trained for 32k ctx so this is safe.
            ctx_params.n_ctx = 32768;
            ctx_params.n_batch = 32768;
            ctx_params.cb_eval = Some(cb_eval_trampoline);
            ctx_params.cb_eval_user_data = cb_state_ptr as *mut std::os::raw::c_void;
            // We don't need the KV cache to grow — single forward, no decode.
            let ctx = llama_cpp_sys_2::llama_init_from_model(model, ctx_params);
            if ctx.is_null() {
                llama_cpp_sys_2::llama_model_free(model);
                anyhow::bail!("llama_init_from_model returned NULL");
            }

            let tokenizer_obj = tokenizers::Tokenizer::from_file(tokenizer)
                .map_err(|e| anyhow!("tokenizer load failed: {e}"))?;

            let head = UnembedHead {
                output_norm_weight: vec![],
                output_weight: vec![],
                n_embd,
                n_vocab,
                eps: 1e-6,
            };

            Ok(Self {
                model,
                ctx,
                vocab,
                tokenizer: tokenizer_obj,
                head,
                n_layers,
                cb_state,
                _not_send_sync: std::marker::PhantomData,
            })
        }
    }

    fn tokenize(&self, text: &str) -> Result<Vec<u32>> {
        let enc = self
            .tokenizer
            .encode(text, false)
            .map_err(|e| anyhow!("tokenize: {e}"))?;
        Ok(enc.get_ids().to_vec())
    }

    fn detokenize(&self, id: u32) -> String {
        self.tokenizer.decode(&[id], false).unwrap_or_else(|_| format!("?{id}"))
    }

    fn n_layers(&self) -> usize { self.n_layers }
    fn n_vocab(&self) -> usize { self.head.n_vocab }
    fn n_embd(&self) -> usize { self.head.n_embd }
    fn unembed_head(&self) -> &UnembedHead { &self.head }

    fn set_capture_attention(&mut self, on: bool) {
        let mut s = self.cb_state.lock().unwrap();
        s.capture_attention = on;
    }

    fn attention_norms(&self) -> AttentionCaptures {
        let s = self.cb_state.lock().unwrap();
        AttentionCaptures {
            norms_per_layer: s.attn_norms_per_layer.clone(),
            tensor_names_seen: s.attn_tensor_names_seen.clone(),
        }
    }

    fn set_attn_bias(&mut self, spec: Option<AttentionBiasSpec>) {
        let mut s = self.cb_state.lock().unwrap();
        s.attn_bias_spec = spec;
    }

    fn forward_capture(
        &mut self,
        tokens: &[u32],
        output_positions: &[u32],
        run_label: &str,
    ) -> Result<ForwardPassCaptures> {
        if tokens.is_empty() {
            anyhow::bail!("forward_capture: empty tokens");
        }
        if output_positions.is_empty() {
            anyhow::bail!("forward_capture: empty output_positions");
        }

        {
            let mut s = self.cb_state.lock().unwrap();
            s.reset_for_forward();
        }

        // Clear KV cache so a second forward_capture pass on the same ctx
        // doesn't fail with "inconsistent sequence positions".
        unsafe {
            let mem = llama_cpp_sys_2::llama_get_memory(self.ctx);
            if !mem.is_null() {
                llama_cpp_sys_2::llama_memory_clear(mem, true);
            }
        }

        let n_tokens = tokens.len();

        // Translate requested output positions (offset from end of prompt:
        // 0 = last token, predicts first generated token) into batch indices.
        // Sort ascending by batch_idx so the captured l_out rows come back
        // in a predictable order — llama.cpp's optimisation only computes
        // l_out for positions whose batch.logits[i] is set, and the captured
        // tensor has rows in batch-index order.
        let mut wanted: Vec<(u32 /*requested_pos*/, usize /*batch_idx*/)> = output_positions
            .iter()
            .filter_map(|&p| {
                let p_us = p as usize;
                if p_us >= n_tokens {
                    None
                } else {
                    Some((p, n_tokens - 1 - p_us))
                }
            })
            .collect();
        wanted.sort_by_key(|(_, idx)| *idx);
        wanted.dedup_by_key(|(_, idx)| *idx);
        if wanted.is_empty() {
            anyhow::bail!(
                "no in-range output_positions (got {:?}, prompt length {n_tokens})",
                output_positions,
            );
        }
        let logits_indices: std::collections::HashSet<usize> =
            wanted.iter().map(|(_, idx)| *idx).collect();

        unsafe {
            let mut batch = llama_cpp_sys_2::llama_batch_init(n_tokens as i32, 0, 1);
            for (i, &tok) in tokens.iter().enumerate() {
                *batch.token.add(i) = tok as i32;
                *batch.pos.add(i) = i as i32;
                *batch.n_seq_id.add(i) = 1;
                *(*batch.seq_id.add(i)).add(0) = 0;
                *batch.logits.add(i) = if logits_indices.contains(&i) { 1 } else { 0 };
            }
            batch.n_tokens = n_tokens as i32;

            let rc = llama_cpp_sys_2::llama_decode(self.ctx, batch);
            llama_cpp_sys_2::llama_batch_free(batch);
            if rc != 0 {
                anyhow::bail!("llama_decode returned {rc}");
            }
        }

        // Build positions × layers output, slicing the captured buffer.
        // Captured-row count can DIFFER per layer: early layers usually get
        // the full ubatch rows (n_tokens), late layers may be optimised by
        // llama.cpp's graph scheduler to only n_outputs rows (those with
        // logits=1). We compute the row index per layer using a stable rule:
        // the LAST K rows of any captured buffer correspond to the last K
        // batch positions (the most-recent indices), so for batch index i
        // in a buffer of R rows: row = i - (n_tokens - R).
        let mut positions = Vec::with_capacity(wanted.len());
        {
            let s = self.cb_state.lock().unwrap();
            let n_embd = self.head.n_embd;

            for (_i, (requested_pos, batch_idx)) in wanted.iter().enumerate() {
                let mut layers = Vec::with_capacity(self.n_layers);
                for l in 0..self.n_layers as u32 {
                    let Some(buf) = s.per_layer.get(&l) else {
                        return Err(anyhow!(
                            "missing l_out-{l} in cb_eval captures (got {} layer keys)",
                            s.per_layer.len(),
                        ));
                    };
                    let captured_rows = buf.len() / n_embd;
                    if captured_rows == 0 {
                        anyhow::bail!("layer {l} captured 0 rows");
                    }
                    // Compute row offset within this layer's buffer.
                    let offset_from_end = n_tokens - 1 - batch_idx; // distance from last position
                    if offset_from_end >= captured_rows {
                        anyhow::bail!(
                            "batch_idx {batch_idx} not in capture for l_out-{l} (captured_rows={captured_rows}, n_tokens={n_tokens})",
                        );
                    }
                    let row = captured_rows - 1 - offset_from_end;
                    let row_start = row * n_embd;
                    let row_end = row_start + n_embd;
                    if row_end > buf.len() {
                        anyhow::bail!(
                            "row {row} out of range for l_out-{l} (len {})",
                            buf.len(),
                        );
                    }
                    let residual = buf[row_start..row_end].to_vec();
                    layers.push(LayerCapture { layer: l, residual });
                }
                positions.push(PositionCapture {
                    position: *requested_pos,
                    layers,
                });
            }

            // Promote captured weights into self.head if they arrived.
            if self.head.output_norm_weight.is_empty() {
                if let Some(w) = &s.output_norm_weight {
                    self.head.output_norm_weight = w.clone();
                }
            }
            if self.head.output_weight.is_empty() {
                if let Some(w) = &s.output_weight {
                    self.head.output_weight = w.clone();
                }
            }
        }

        Ok(ForwardPassCaptures {
            run: run_label.to_string(),
            positions,
        })
    }

    fn forward_generate(
        &mut self,
        prompt_tokens: &[u32],
        max_new: u32,
        eos_token_id: Option<u32>,
        run_label: &str,
    ) -> Result<AutoregressiveCaptures> {
        if prompt_tokens.is_empty() {
            anyhow::bail!("forward_generate: empty prompt_tokens");
        }
        if max_new == 0 {
            anyhow::bail!("forward_generate: max_new must be > 0");
        }

        // Reset cb_state (per_layer buffers cleared); clear KV cache so we
        // start fresh for this generation run.
        {
            let mut s = self.cb_state.lock().unwrap();
            s.reset_for_forward();
        }
        unsafe {
            let mem = llama_cpp_sys_2::llama_get_memory(self.ctx);
            if !mem.is_null() {
                llama_cpp_sys_2::llama_memory_clear(mem, true);
            }
        }

        let n_prompt = prompt_tokens.len();
        let n_embd = self.head.n_embd;
        let n_vocab = self.head.n_vocab as i32;

        // Prompt decode. Only the LAST token gets logits=1 (we sample from
        // it). KV cache is filled for positions 0..n_prompt.
        unsafe {
            let mut batch = llama_cpp_sys_2::llama_batch_init(n_prompt as i32, 0, 1);
            for (i, &tok) in prompt_tokens.iter().enumerate() {
                *batch.token.add(i) = tok as i32;
                *batch.pos.add(i) = i as i32;
                *batch.n_seq_id.add(i) = 1;
                *(*batch.seq_id.add(i)).add(0) = 0;
                *batch.logits.add(i) = (i == n_prompt - 1) as i8;
            }
            batch.n_tokens = n_prompt as i32;
            let rc = llama_cpp_sys_2::llama_decode(self.ctx, batch);
            llama_cpp_sys_2::llama_batch_free(batch);
            if rc != 0 {
                anyhow::bail!("llama_decode (prompt) returned {rc}");
            }
        }

        // Resolve EOS. Caller-supplied wins; fall back to the model's eos
        // token id from the vocab.
        let eos = eos_token_id.unwrap_or_else(|| unsafe {
            let e = llama_cpp_sys_2::llama_vocab_eos(self.vocab);
            if e < 0 { u32::MAX } else { e as u32 }
        });

        // Generation loop. Greedy argmax v1.
        let mut generated_tokens: Vec<u32> = Vec::with_capacity(max_new as usize);
        let mut stopped_on_eos = false;
        let mut cur_pos = n_prompt as i32;
        for _step in 0..max_new {
            // Read final-layer logits for the most recent position.
            // -1 means "the most recent token with logits=1 enabled."
            let logits_ptr = unsafe { llama_cpp_sys_2::llama_get_logits_ith(self.ctx, -1) };
            if logits_ptr.is_null() {
                anyhow::bail!("llama_get_logits_ith returned NULL at gen pos {cur_pos}");
            }
            let logits = unsafe {
                std::slice::from_raw_parts(logits_ptr, n_vocab as usize)
            };
            // Argmax (greedy).
            let next = logits
                .iter()
                .enumerate()
                .fold((0u32, f32::NEG_INFINITY), |acc, (i, &l)| {
                    if l > acc.1 { (i as u32, l) } else { acc }
                })
                .0;

            // EOS check (caller-supplied OR model's vocab EOS OR ggml's eog).
            let is_eog = unsafe {
                llama_cpp_sys_2::llama_vocab_is_eog(self.vocab, next as i32)
            };
            if next == eos || is_eog {
                stopped_on_eos = true;
                break;
            }
            generated_tokens.push(next);

            // Single-token decode. KV cache extends to cur_pos.
            unsafe {
                let mut batch = llama_cpp_sys_2::llama_batch_init(1, 0, 1);
                *batch.token.add(0) = next as i32;
                *batch.pos.add(0) = cur_pos;
                *batch.n_seq_id.add(0) = 1;
                *(*batch.seq_id.add(0)).add(0) = 0;
                *batch.logits.add(0) = 1;
                batch.n_tokens = 1;
                let rc = llama_cpp_sys_2::llama_decode(self.ctx, batch);
                llama_cpp_sys_2::llama_batch_free(batch);
                if rc != 0 {
                    anyhow::bail!("llama_decode (gen pos {cur_pos}) returned {rc}");
                }
            }
            cur_pos += 1;
        }

        // Slice per_layer buffers: the LAST N rows of each layer's buffer
        // correspond to the N generated tokens, in order. Earlier rows are
        // the prompt's residuals (we ignore those here — the prompt-position
        // path is `forward_capture`).
        let n_gen = generated_tokens.len();
        let mut steps: Vec<PositionCapture> = Vec::with_capacity(n_gen);
        {
            let s = self.cb_state.lock().unwrap();
            for step_idx in 0..n_gen {
                let mut layers = Vec::with_capacity(self.n_layers);
                for l in 0..self.n_layers as u32 {
                    let Some(buf) = s.per_layer.get(&l) else {
                        return Err(anyhow!(
                            "missing l_out-{l} in cb_eval captures during generate (got {} layer keys)",
                            s.per_layer.len(),
                        ));
                    };
                    let captured_rows = buf.len() / n_embd;
                    if captured_rows < n_gen {
                        anyhow::bail!(
                            "layer {l} has {captured_rows} captured rows, expected at least {n_gen} (one per generated token)",
                        );
                    }
                    let row = captured_rows - n_gen + step_idx;
                    let row_start = row * n_embd;
                    let row_end = row_start + n_embd;
                    let residual = buf[row_start..row_end].to_vec();
                    layers.push(LayerCapture { layer: l, residual });
                }
                steps.push(PositionCapture {
                    position: step_idx as u32,
                    layers,
                });
            }

            // Promote captured weights if they arrived (same as forward_capture).
            if self.head.output_norm_weight.is_empty() {
                if let Some(w) = &s.output_norm_weight {
                    self.head.output_norm_weight = w.clone();
                }
            }
            if self.head.output_weight.is_empty() {
                if let Some(w) = &s.output_weight {
                    self.head.output_weight = w.clone();
                }
            }
        }

        Ok(AutoregressiveCaptures {
            run: run_label.to_string(),
            generated_tokens,
            steps,
            stopped_on_eos,
        })
    }

    fn forward_generate_with_inject(
        &mut self,
        prompt_tokens: &[u32],
        max_new: u32,
        eos_token_id: Option<u32>,
        run_label: &str,
        spec: &InjectSpec,
    ) -> Result<AutoregressiveCaptures> {
        // B3 (cycle 0.6 spitball): autoregressive decode with residual-stream
        // modification at the lens-identified decision band (L25-27 × gen
        // positions 23-25). cb_eval reads l_out-N, modifies the row in scratch,
        // writes back via ggml_backend_tensor_set, and subsequent ops see the
        // injected residual. Inject hot-path is gated on
        // cb_state.inject_active_position so prompt prefill and out-of-band
        // generation steps stay untouched.
        if prompt_tokens.is_empty() {
            anyhow::bail!("forward_generate_with_inject: empty prompt_tokens");
        }
        if max_new == 0 {
            anyhow::bail!("forward_generate_with_inject: max_new must be > 0");
        }
        if spec.target_embed.len() != self.head.n_embd {
            anyhow::bail!(
                "inject target_embed len {} != n_embd {}",
                spec.target_embed.len(),
                self.head.n_embd
            );
        }

        {
            let mut s = self.cb_state.lock().unwrap();
            s.reset_for_forward();
            s.inject_spec = Some(spec.clone());
            s.inject_active_position = None; // prompt phase
        }
        unsafe {
            let mem = llama_cpp_sys_2::llama_get_memory(self.ctx);
            if !mem.is_null() {
                llama_cpp_sys_2::llama_memory_clear(mem, true);
            }
        }

        let n_prompt = prompt_tokens.len();
        let n_embd = self.head.n_embd;
        let n_vocab = self.head.n_vocab as i32;

        // Prompt prefill — no inject during prompt.
        unsafe {
            let mut batch = llama_cpp_sys_2::llama_batch_init(n_prompt as i32, 0, 1);
            for (i, &tok) in prompt_tokens.iter().enumerate() {
                *batch.token.add(i) = tok as i32;
                *batch.pos.add(i) = i as i32;
                *batch.n_seq_id.add(i) = 1;
                *(*batch.seq_id.add(i)).add(0) = 0;
                *batch.logits.add(i) = (i == n_prompt - 1) as i8;
            }
            batch.n_tokens = n_prompt as i32;
            let rc = llama_cpp_sys_2::llama_decode(self.ctx, batch);
            llama_cpp_sys_2::llama_batch_free(batch);
            if rc != 0 {
                anyhow::bail!("llama_decode (prompt) returned {rc}");
            }
        }

        let eos = eos_token_id.unwrap_or_else(|| unsafe {
            let e = llama_cpp_sys_2::llama_vocab_eos(self.vocab);
            if e < 0 { u32::MAX } else { e as u32 }
        });

        let mut generated_tokens: Vec<u32> = Vec::with_capacity(max_new as usize);
        let mut stopped_on_eos = false;
        let mut cur_pos = n_prompt as i32;
        for step in 0..max_new {
            // Sample from logits (no inject during sampling — inject is on the
            // NEXT step's decode, modifying l_out-N as it computes the next
            // token's residual).
            let logits_ptr = unsafe { llama_cpp_sys_2::llama_get_logits_ith(self.ctx, -1) };
            if logits_ptr.is_null() {
                anyhow::bail!("llama_get_logits_ith returned NULL at gen pos {cur_pos}");
            }
            let logits = unsafe {
                std::slice::from_raw_parts(logits_ptr, n_vocab as usize)
            };
            let next = logits
                .iter()
                .enumerate()
                .fold((0u32, f32::NEG_INFINITY), |acc, (i, &l)| {
                    if l > acc.1 { (i as u32, l) } else { acc }
                })
                .0;

            let is_eog = unsafe {
                llama_cpp_sys_2::llama_vocab_is_eog(self.vocab, next as i32)
            };
            if next == eos || is_eog {
                stopped_on_eos = true;
                break;
            }
            generated_tokens.push(next);

            // Activate inject for THIS step's decode. cb_eval fires on the new
            // token's l_out-N tensors; the trampoline checks inject_positions
            // and inject_layers to decide whether to modify.
            {
                let mut s = self.cb_state.lock().unwrap();
                s.inject_active_position = Some(step);
            }

            unsafe {
                let mut batch = llama_cpp_sys_2::llama_batch_init(1, 0, 1);
                *batch.token.add(0) = next as i32;
                *batch.pos.add(0) = cur_pos;
                *batch.n_seq_id.add(0) = 1;
                *(*batch.seq_id.add(0)).add(0) = 0;
                *batch.logits.add(0) = 1;
                batch.n_tokens = 1;
                let rc = llama_cpp_sys_2::llama_decode(self.ctx, batch);
                llama_cpp_sys_2::llama_batch_free(batch);
                if rc != 0 {
                    anyhow::bail!("llama_decode (gen pos {cur_pos}) returned {rc}");
                }
            }
            cur_pos += 1;
        }

        // Clear inject state so subsequent forward calls don't carry over.
        {
            let mut s = self.cb_state.lock().unwrap();
            s.inject_spec = None;
            s.inject_active_position = None;
        }

        // Slice per_layer buffers as in forward_generate. The captures reflect
        // the MODIFIED residuals (the trampoline appends post-modification).
        let n_gen = generated_tokens.len();
        let mut steps: Vec<PositionCapture> = Vec::with_capacity(n_gen);
        {
            let s = self.cb_state.lock().unwrap();
            for step_idx in 0..n_gen {
                let mut layers = Vec::with_capacity(self.n_layers);
                for l in 0..self.n_layers as u32 {
                    let Some(buf) = s.per_layer.get(&l) else {
                        return Err(anyhow!(
                            "missing l_out-{l} in cb_eval captures during inject-generate",
                        ));
                    };
                    let captured_rows = buf.len() / n_embd;
                    if captured_rows < n_gen {
                        anyhow::bail!(
                            "layer {l}: {captured_rows} captured rows, expected ≥ {n_gen}",
                        );
                    }
                    let row = captured_rows - n_gen + step_idx;
                    let row_start = row * n_embd;
                    let residual = buf[row_start..row_start + n_embd].to_vec();
                    layers.push(LayerCapture { layer: l, residual });
                }
                steps.push(PositionCapture {
                    position: step_idx as u32,
                    layers,
                });
            }
        }

        Ok(AutoregressiveCaptures {
            run: run_label.to_string(),
            generated_tokens,
            steps,
            stopped_on_eos,
        })
    }
}

#[cfg(feature = "real")]
impl Drop for LlamaCppRuntime {
    fn drop(&mut self) {
        unsafe {
            if !self.ctx.is_null() {
                llama_cpp_sys_2::llama_free(self.ctx);
            }
            if !self.model.is_null() {
                llama_cpp_sys_2::llama_model_free(self.model);
            }
            llama_cpp_sys_2::llama_backend_free();
        }
        let _ = &self.vocab;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_runtime_returns_n_layers_residuals() {
        let mut rt = FakeRuntime::new(4, 16, 8);
        let captures = rt
            .forward_capture(&[1, 2, 3], &[0], "baseline")
            .unwrap();
        assert_eq!(captures.positions.len(), 1);
        assert_eq!(captures.positions[0].layers.len(), 4);
        assert_eq!(captures.positions[0].layers[0].residual.len(), 8);
        assert_eq!(captures.run, "baseline");
    }

    #[test]
    fn fake_runtime_top1_sharpens_with_depth() {
        use crate::lens::{residual_to_logits, softmax};
        let mut rt = FakeRuntime::new(6, 32, 16);
        let captures = rt
            .forward_capture(&[1, 5, 3, 7], &[0], "baseline")
            .unwrap();
        let head = rt.unembed_head();
        let mut probs_at_layer = Vec::new();
        for c in &captures.positions[0].layers {
            let l = residual_to_logits(&c.residual, head);
            let p = softmax(&l);
            probs_at_layer.push(*p.iter().fold(&0.0, |a, b| if b > a { b } else { a }));
        }
        // Each successive layer should have higher top-1 probability.
        for w in probs_at_layer.windows(2) {
            assert!(w[1] >= w[0], "top-1 prob did not monotonically increase: {probs_at_layer:?}");
        }
    }

    #[test]
    fn fake_runtime_returns_multiple_positions() {
        let mut rt = FakeRuntime::new(3, 32, 8);
        let captures = rt
            .forward_capture(&[1, 5, 3, 7, 11], &[0, 1, 2, 3], "baseline")
            .unwrap();
        assert_eq!(captures.positions.len(), 4);
        for (i, pc) in captures.positions.iter().enumerate() {
            assert_eq!(pc.position, i as u32);
            assert_eq!(pc.layers.len(), 3);
        }
    }

    #[test]
    fn fake_runtime_forward_generate_runs_to_max_new_without_eos() {
        let mut rt = FakeRuntime::new(4, 32, 8);
        let cap = rt.forward_generate(&[1, 2, 3], 5, None, "gen-baseline").unwrap();
        assert_eq!(cap.generated_tokens.len(), 5);
        assert_eq!(cap.steps.len(), 5);
        assert!(!cap.stopped_on_eos);
        for (i, pc) in cap.steps.iter().enumerate() {
            assert_eq!(pc.position, i as u32);
            assert_eq!(pc.layers.len(), 4);
        }
        assert_eq!(cap.run, "gen-baseline");
    }

    #[test]
    fn fake_runtime_forward_generate_stops_on_eos() {
        let mut rt = FakeRuntime::new(2, 32, 8);
        // last_prompt=3 → next tokens 4, 5, 6, ...; set EOS=5 so we stop after token 4.
        let cap = rt.forward_generate(&[1, 2, 3], 100, Some(5), "gen-eos").unwrap();
        assert!(cap.stopped_on_eos);
        assert_eq!(cap.generated_tokens.len(), 1, "got {:?}", cap.generated_tokens);
        assert_eq!(cap.generated_tokens[0], 4);
        assert_eq!(cap.steps.len(), 1);
    }

    #[test]
    fn fake_runtime_forward_generate_residuals_track_generated_tokens() {
        use crate::lens::{residual_to_logits, softmax};
        // n_vocab == n_embd so no FakeHead row aliasing (token v's row is
        // one-hot at e=v%n_embd; vocab>embd would collide).
        let mut rt = FakeRuntime::new(3, 8, 8);
        let cap = rt.forward_generate(&[1, 2, 3], 4, None, "gen").unwrap();
        let head = rt.unembed_head();
        // Per-step LAST-layer top-1 should be the generated token (synthetic
        // bias makes it sharpen by depth, peak at the target token's dim).
        for (i, pc) in cap.steps.iter().enumerate() {
            let last = pc.layers.last().unwrap();
            let logits = residual_to_logits(&last.residual, head);
            let probs = softmax(&logits);
            let top_idx = probs
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(i, _)| i as u32)
                .unwrap();
            assert_eq!(
                top_idx, cap.generated_tokens[i],
                "step {i} top-1 = {top_idx}, generated_token = {}",
                cap.generated_tokens[i],
            );
        }
    }
}
