//! The crash-isolated worker (FB.14) over the deterministic test model, so
//! no GPU, model download or network is needed: the protocol, the reuse, and
//! every fallback (`no_worker`, `not_built`, `no_adapter:..`,
//! `gpu_crash:..`), each ending with every text embedded.

use glia_embed::worker::{WORKER_TEST_ENV, WorkerEmbedder, WorkerProgram};
use glia_embed::{DIMS_TEST_HASH, Device, Embedder, HashEmbedder, MODEL_TEST_HASH};

fn hash_cpu() -> Result<Box<dyn Embedder>, String> {
    Ok(Box::new(HashEmbedder))
}

fn worker() -> WorkerProgram {
    WorkerProgram::external(env!("CARGO_BIN_EXE_glia-embed-worker"))
}

fn texts(n: usize) -> Vec<String> {
    (0..n)
        .map(|i| format!("fn handler_{i} UserService.getUser{} route", i % 7))
        .collect()
}

fn expected(texts: &[String]) -> Vec<Vec<f32>> {
    texts.iter().map(|t| HashEmbedder::embed_one(t)).collect()
}

fn embedder(program: Option<WorkerProgram>, strict: bool) -> WorkerEmbedder {
    WorkerEmbedder::new("test-hash", Device::WebGpu, strict, program, hash_cpu).unwrap()
}

#[test]
fn a_live_worker_embeds_every_text_on_its_device() {
    let e = embedder(Some(worker()), false);
    assert_eq!(e.model(), MODEL_TEST_HASH);
    assert_eq!(e.dims(), DIMS_TEST_HASH);
    let t = texts(21);
    let tagged = e.embed_tagged(&t).unwrap();
    assert_eq!(tagged.len(), t.len());
    let want = expected(&t);
    for ((v, d), w) in tagged.iter().zip(&want) {
        assert_eq!(v, w);
        assert_eq!(*d, Some(Device::WebGpu));
    }
    // The same child answers a second call (batches of 8 across calls).
    assert_eq!(e.embed(&t[..3]).unwrap(), want[..3].to_vec());
    assert_eq!(e.fallback(), None);
    assert_eq!(e.backend(), Device::WebGpu);
    assert_eq!(e.embed(&[]).unwrap(), Vec::<Vec<f32>>::new());
}

#[test]
fn a_worker_that_aborts_mid_batch_falls_back_with_every_text_embedded() {
    // Two answered sub-batches of 8, then an abort on the third request.
    let program = worker().with_env(WORKER_TEST_ENV, "abort_after=2");
    let e = embedder(Some(program), false);
    let t = texts(40);
    let tagged = e.embed_tagged(&t).unwrap();
    assert_eq!(tagged.len(), 40);
    let want = expected(&t);
    for (i, ((v, d), w)) in tagged.iter().zip(&want).enumerate() {
        assert_eq!(v, w, "text {i}");
        let on = if i < 16 { Some(Device::WebGpu) } else { None };
        assert_eq!(*d, on, "text {i}");
    }
    let reason = e.fallback().expect("a fallback");
    assert!(reason.starts_with("gpu_crash:"), "{reason}");
    #[cfg(unix)]
    assert_eq!(reason, "gpu_crash:signal=6");
    assert_eq!(e.backend(), Device::Cpu);
    // Later calls stay on the CPU fallback.
    assert_eq!(e.embed(&t[..5]).unwrap(), want[..5].to_vec());
}

#[test]
fn a_worker_without_an_adapter_falls_back() {
    let program = worker().with_env(WORKER_TEST_ENV, "no_adapter");
    let e = embedder(Some(program), false);
    let t = texts(9);
    assert_eq!(e.embed(&t).unwrap(), expected(&t));
    assert_eq!(e.fallback().as_deref(), Some("no_adapter:simulated"));
    assert_eq!(e.backend(), Device::Cpu);
}

#[test]
fn an_explicit_device_that_does_not_start_is_an_error() {
    let program = worker().with_env(WORKER_TEST_ENV, "no_adapter");
    let e = embedder(Some(program), true);
    let err = e.embed(&texts(2)).expect_err("strict");
    assert!(
        err.starts_with("GLIA_EMBED_DEVICE=webgpu: the webgpu device did not start"),
        "{err}"
    );
    assert!(err.contains("no_adapter:simulated"), "{err}");
    let err = embedder(None, true).embed(&texts(1)).expect_err("strict");
    assert!(err.contains("(no_worker)"), "{err}");
}

#[test]
fn no_worker_falls_back() {
    let e = embedder(None, false);
    let t = texts(3);
    assert_eq!(e.embed(&t).unwrap(), expected(&t));
    assert_eq!(e.fallback().as_deref(), Some("no_worker"));

    let missing = WorkerProgram::external("/nonexistent/glia-embed-worker");
    let e = embedder(Some(missing), false);
    assert_eq!(e.embed(&t).unwrap(), expected(&t));
    let reason = e.fallback().unwrap_or_default();
    assert!(reason.starts_with("no_worker:/nonexistent"), "{reason}");
}

#[test]
fn a_builtin_worker_lacking_the_device_is_not_built() {
    if Device::Cuda.built() {
        return;
    }
    // This test binary as a "builtin" worker: refused before any spawn.
    let program = WorkerProgram::this_binary(&["--never-run"]).unwrap();
    let e = WorkerEmbedder::new("test-hash", Device::Cuda, false, Some(program), hash_cpu).unwrap();
    let t = texts(2);
    assert_eq!(e.embed(&t).unwrap(), expected(&t));
    assert_eq!(e.fallback().as_deref(), Some("not_built"));
}

#[test]
fn locate_finds_the_worker_binary() {
    let bin = std::path::PathBuf::from(env!("CARGO_BIN_EXE_glia-embed-worker"));
    let dir = bin.parent().unwrap().to_path_buf();
    let found = WorkerProgram::locate(&[std::env::temp_dir().join("no-such-dir"), dir]).unwrap();
    assert_eq!(found.program, bin);
    assert!(!found.builtin);
    assert!(WorkerProgram::locate(&[]).is_none());
}

#[test]
fn unknown_models_are_refused() {
    let e = WorkerEmbedder::new("gpt", Device::WebGpu, false, None, hash_cpu);
    assert!(e.unwrap_err().contains("unknown embedding model `gpt`"));
}
