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
        }
    }
    fn reset_for_forward(&mut self) {
        self.per_layer.clear();
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

    if l_out_layer.is_none() && !is_result_norm && !is_result_output {
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
    }
    true
}

#[cfg(feature = "real")]
fn parse_l_out_layer(name: &str) -> Option<u32> {
    let suffix = name.strip_prefix("l_out-")?;
    suffix.parse::<u32>().ok()
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
