//! The CPU determinism gate for bge-small (FB.1, next-leap-0.5.4.md §B).
//! Needs the model download, so every test is `#[ignore]`:
//! `cargo test -p glia-embed --features fastembed -- --ignored`.
#![cfg(feature = "fastembed")]

use std::sync::{Mutex, MutexGuard};

use glia_embed::{DIMS_BGE_SMALL, Embedder, FastEmbedder, MODEL_BGE_SMALL, codec};

fn texts() -> Vec<String> {
    let words = [
        "resolve",
        "http",
        "route",
        "handler",
        "UserService.getUser",
        "queue consumer",
        "graph",
        "blast radius",
        "fn parse_one(path: &Path) -> Result<Items>",
        "SELECT * FROM orders",
        "the quick brown fox",
        "",
    ];
    (0..300)
        .map(|i| {
            let a = words[i % words.len()];
            let b = words[(i * 7 + 3) % words.len()];
            let c = words[(i * 13 + 5) % words.len()];
            format!("{a} {b} {i} {c}")
        })
        .collect()
}

/// One session at a time across this file's tests (they would otherwise run
/// on parallel test threads: two sessions at once thrash, and two first
/// loads race the download).
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|p| p.into_inner())
}

fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|x| x.to_bits()).collect()
}

#[test]
#[ignore = "downloads the bge-small model"]
fn cpu_vectors_are_bit_identical_across_batching() {
    let _one = serial();
    let texts = texts();
    let first = FastEmbedder::new().unwrap();
    assert_eq!(first.model(), MODEL_BGE_SMALL);
    assert_eq!(first.dims(), DIMS_BGE_SMALL);
    let batch = first.embed(&texts).unwrap();
    assert_eq!(batch.len(), texts.len());
    for v in &batch {
        assert_eq!(v.len(), DIMS_BGE_SMALL as usize);
        assert!((codec::norm(v) - 1.0).abs() < 1e-5);
    }
    // Three shuffled batches through the same session.
    let mut order: Vec<usize> = (0..texts.len()).collect();
    order.sort_by_key(|&i| (i * 131 + 17) % texts.len());
    let mut shuffled = vec![Vec::new(); texts.len()];
    for part in order.chunks(100) {
        let chunk: Vec<String> = part.iter().map(|&i| texts[i].clone()).collect();
        for (&i, v) in part.iter().zip(first.embed(&chunk).unwrap()) {
            shuffled[i] = v;
        }
    }
    drop(first);
    // A second session, in sequence (never concurrent): each text alone.
    let second = FastEmbedder::new().unwrap();
    for (i, t) in texts.iter().enumerate() {
        let alone = second.embed(std::slice::from_ref(t)).unwrap().remove(0);
        assert_eq!(bits(&alone), bits(&batch[i]), "alone vs batch: {t:?}");
        assert_eq!(
            bits(&shuffled[i]),
            bits(&batch[i]),
            "shuffled vs batch: {t:?}"
        );
    }
}

#[test]
#[ignore = "downloads the bge-small model"]
fn cache_inside_a_repo_is_refused() {
    let _one = serial();
    let e = FastEmbedder::new().unwrap();
    let cache = e.cache_dir().to_path_buf();
    assert!(e.refuse_repo_dir(&cache).is_err());
    let elsewhere = std::env::temp_dir();
    if !cache.starts_with(&elsewhere) {
        assert!(e.refuse_repo_dir(&elsewhere).is_ok());
    }
}
