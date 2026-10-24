//! The deterministic test model, the codec and `named` (FB.1).

use glia_embed::{
    DIMS_TEST_HASH, Embedder, HashEmbedder, MODEL_NAMES, MODEL_TEST_HASH, codec, named,
};

fn one(text: &str) -> Vec<f32> {
    HashEmbedder.embed(&[text.to_string()]).unwrap().remove(0)
}

#[test]
fn same_text_gives_identical_bytes() {
    let a = codec::to_le_bytes(&one("resolve the HTTP route handler"));
    let b = codec::to_le_bytes(&one("resolve the HTTP route handler"));
    assert_eq!(a, b);
    assert_eq!(a.len(), DIMS_TEST_HASH as usize * 4);
}

#[test]
fn identifier_and_words_are_close() {
    let c = codec::cosine(&one("UserService.getUser"), &one("user service get user"));
    assert!(c > 0.9, "cosine {c}");
}

#[test]
fn unrelated_texts_are_far() {
    let c = codec::cosine(
        &one("parse configuration file"),
        &one("render html template widget"),
    );
    assert!(c < 0.5, "cosine {c}");
}

#[test]
fn empty_text_is_e0() {
    let v = one("");
    assert_eq!(v[0], 1.0);
    assert!(v[1..].iter().all(|&x| x == 0.0));
    assert_eq!(one(" .- "), v);
}

#[test]
fn every_vector_is_unit_and_64_long() {
    let texts: Vec<String> = [
        "",
        "a",
        "fetchOrders",
        "the quick brown fox jumps over the lazy dog",
        "http2Server listens on port 8080",
        "graph::resolvers::http::HttpStackResolver",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let e = HashEmbedder;
    assert_eq!(e.dims(), 64);
    assert_eq!(e.model(), MODEL_TEST_HASH);
    let out = e.embed(&texts).unwrap();
    assert_eq!(out.len(), texts.len());
    for (t, v) in texts.iter().zip(&out) {
        assert_eq!(v.len(), 64);
        assert!((codec::norm(v) - 1.0).abs() < 1e-6, "{t:?}");
        assert_eq!(v, &one(t), "batch == alone for {t:?}");
    }
}

#[test]
fn codec_round_trips_an_embedding() {
    let v = one("blast radius over the identity quotient");
    let back = codec::from_le_bytes(&codec::to_le_bytes(&v), DIMS_TEST_HASH).unwrap();
    assert_eq!(back, v);
    assert!(codec::from_le_bytes(&codec::to_le_bytes(&v), 384).is_err());
}

#[test]
fn named_selects_and_refuses() {
    let e = named("test-hash").unwrap();
    assert_eq!(e.model(), MODEL_TEST_HASH);
    let err = named("nope").err().unwrap();
    for n in MODEL_NAMES {
        assert!(err.contains(n), "{err}");
    }
}

#[cfg(not(feature = "fastembed"))]
#[test]
fn bge_small_needs_the_feature() {
    let err = named("bge-small").err().unwrap();
    assert!(err.contains("--model test-hash"), "{err}");
}
