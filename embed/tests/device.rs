//! The embedding devices (FB.14): selection, the batch pins, and the GPU
//! parity gate. The device-policy rule: a vector test compares with a
//! tolerance (max abs diff < 1e-5, top-10 neighbours equal), so it passes on
//! whichever device the host has; the GPU tests are `#[ignore]` (they need
//! the model download and, for WebGPU, an adapter):
//! `cargo test -p glia-embed --features gpu-webgpu --test device -- --ignored`.

use glia_embed::device::{self, Device};
use glia_embed::{CHUNK, Embedder};

#[test]
fn batches_are_pinned() {
    // WebGPU aborted the process at batch >= 64 and halved its throughput
    // above 8 on the spike's GTX 1660 Super.
    assert_eq!(device::batch(Device::WebGpu), 8);
    assert_eq!(device::batch(Device::Cuda), 32);
    assert_eq!(device::batch(Device::Cpu), CHUNK);
}

#[test]
fn built_devices_follow_the_features() {
    let built = device::built_devices();
    assert_eq!(built.contains(&Device::Cpu), glia_embed::ORT_BUILT);
    assert_eq!(built.contains(&Device::WebGpu), device::WEBGPU_BUILT);
    assert_eq!(built.contains(&Device::Cuda), device::CUDA_BUILT);
    if !glia_embed::ORT_BUILT {
        assert!(built.is_empty());
    }
}

#[cfg(not(all(
    feature = "fastembed",
    not(all(target_os = "macos", target_arch = "x86_64"))
)))]
#[test]
fn bge_small_needs_onnx_runtime() {
    let Err(e) = glia_embed::bge_small(None) else {
        panic!("bge-small without ONNX Runtime");
    };
    if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        assert_eq!(e, glia_embed::INTEL_MAC_REFUSAL);
    } else {
        assert!(e.contains("without the embedding model"), "{e}");
    }
    assert!(glia_embed::named("bge-small").is_err());
    assert!(glia_embed::named("test-hash").is_ok());
}

#[test]
fn the_cpu_adapter_has_a_name() {
    let a = device::cpu_adapter();
    assert!(!a.name.is_empty());
    assert_eq!(a.driver, "none");
}

#[cfg(all(
    feature = "fastembed",
    not(all(target_os = "macos", target_arch = "x86_64"))
))]
mod model {
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, MutexGuard};

    use glia_embed::worker::{WorkerEmbedder, WorkerProgram};
    use glia_embed::{Device, Embedder, cpu_bge_small};

    static SERIAL: Mutex<()> = Mutex::new(());

    fn serial() -> MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The spike's 256 texts: consecutive 4000-byte chunks of engine/src's
    /// `.rs` files, the files in path order (the spike read them in directory
    /// order), every full chunk of a file before the next file.
    pub(super) fn spike_texts() -> Vec<String> {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../engine/src");
        let mut files: Vec<PathBuf> = std::fs::read_dir(&src)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "rs"))
            .collect();
        files.sort();
        let mut out = Vec::new();
        for p in files {
            let bytes = std::fs::read(&p).unwrap();
            for c in bytes.chunks(4000) {
                if c.len() < 4000 {
                    break;
                }
                out.push(String::from_utf8_lossy(c).into_owned());
                if out.len() == 256 {
                    return out;
                }
            }
        }
        out
    }

    /// (max abs diff, min top-10 overlap over the first 10 queries).
    pub(super) fn compare(a: &[Vec<f32>], b: &[Vec<f32>]) -> (f32, usize) {
        assert_eq!(a.len(), b.len());
        let diff = a
            .iter()
            .zip(b)
            .flat_map(|(x, y)| x.iter().zip(y).map(|(p, q)| (p - q).abs()))
            .fold(0.0f32, f32::max);
        let top10 = |m: &[Vec<f32>], q: usize| -> Vec<usize> {
            let mut s: Vec<(f32, usize)> = m
                .iter()
                .enumerate()
                .map(|(i, v)| (glia_embed::codec::cosine(&m[q], v), i))
                .collect();
            s.sort_by(|x, y| y.0.total_cmp(&x.0).then(x.1.cmp(&y.1)));
            let mut ids: Vec<usize> = s.iter().take(10).map(|p| p.1).collect();
            ids.sort_unstable();
            ids
        };
        let overlap = (0..10.min(a.len()))
            .map(|q| {
                let (ta, tb) = (top10(a, q), top10(b, q));
                ta.iter().filter(|i| tb.contains(i)).count()
            })
            .min()
            .unwrap_or(10);
        (diff, overlap)
    }

    fn worker() -> WorkerProgram {
        WorkerProgram::external(env!("CARGO_BIN_EXE_glia-embed-worker"))
    }

    /// The acceptance gate: WebGPU through the worker agrees with the CPU
    /// session (max abs diff < 1e-5, top-10 neighbours 10/10). Strict, so a
    /// host whose adapter does not start fails here instead of comparing the
    /// CPU with itself.
    #[cfg(feature = "gpu-webgpu")]
    #[test]
    #[ignore = "needs a WebGPU adapter and the bge-small download"]
    fn webgpu_matches_cpu() {
        let _one = serial();
        let texts = spike_texts();
        assert_eq!(texts.len(), 256);
        let cpu = cpu_bge_small().unwrap().embed(&texts).unwrap();
        let gpu = WorkerEmbedder::new(
            "bge-small",
            Device::WebGpu,
            true,
            Some(worker()),
            cpu_bge_small,
        )
        .unwrap();
        let tagged = gpu.embed_tagged(&texts).unwrap();
        assert_eq!(gpu.fallback(), None);
        assert!(tagged.iter().all(|(_, d)| *d == Some(Device::WebGpu)));
        let gpu: Vec<Vec<f32>> = tagged.into_iter().map(|(v, _)| v).collect();
        let (diff, overlap) = compare(&cpu, &gpu);
        eprintln!(
            "[embed-parity] webgpu vs cpu texts=256 max_abs_diff={diff:.3e} top10_min={overlap}/10"
        );
        assert!(diff < 1e-5, "max abs diff {diff}");
        assert_eq!(overlap, 10);
    }

    /// No Vulkan ICD for the worker: the WebGPU session cannot commit, the
    /// run falls back to the CPU with `no_adapter:..` and every vector is the
    /// CPU's.
    #[cfg(feature = "gpu-webgpu")]
    #[test]
    #[ignore = "needs the bge-small download"]
    fn no_vulkan_icd_falls_back_to_the_cpu() {
        let _one = serial();
        let texts: Vec<String> = spike_texts().into_iter().take(12).collect();
        let program = worker().with_env("VK_DRIVER_FILES", "/nonexistent.json");
        let e = WorkerEmbedder::new(
            "bge-small",
            Device::WebGpu,
            false,
            Some(program),
            cpu_bge_small,
        )
        .unwrap();
        let got = e.embed(&texts).unwrap();
        let reason = e.fallback().unwrap_or_default();
        assert!(reason.starts_with("no_adapter:"), "{reason}");
        assert_eq!(e.backend(), Device::Cpu);
        let cpu = cpu_bge_small().unwrap().embed(&texts).unwrap();
        assert_eq!(compare(&cpu, &got).0, 0.0);
    }

    /// The default selection, whatever the host: every text embedded, on the
    /// GPU or (fallback) the CPU, within tolerance of the CPU session.
    #[test]
    #[ignore = "needs the bge-small download"]
    fn the_default_device_agrees_with_the_cpu() {
        let _one = serial();
        let texts: Vec<String> = spike_texts().into_iter().take(24).collect();
        let e = WorkerEmbedder::new(
            "bge-small",
            Device::WebGpu,
            false,
            Some(worker()),
            cpu_bge_small,
        )
        .unwrap();
        let got = e.embed(&texts).unwrap();
        let cpu = cpu_bge_small().unwrap().embed(&texts).unwrap();
        let (diff, _) = compare(&cpu, &got);
        assert!(
            diff < 1e-5,
            "max abs diff {diff} (fallback {:?})",
            e.fallback()
        );
    }
}

#[test]
fn compare_is_exact_on_equal_inputs() {
    #[cfg(all(
        feature = "fastembed",
        not(all(target_os = "macos", target_arch = "x86_64"))
    ))]
    {
        let v: Vec<Vec<f32>> = (0..12)
            .map(|i| glia_embed::HashEmbedder::embed_one(&format!("text {i} item{}", i * 3)))
            .collect();
        assert_eq!(model::compare(&v, &v), (0.0, 10));
    }
    let e = glia_embed::HashEmbedder;
    assert_eq!(e.device(), None);
}
