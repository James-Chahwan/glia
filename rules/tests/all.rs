//! The merged integration-test target of glia-rules (FH.18): every tests/*.rs that
//! build.rs's `KEEP` list does not keep as its own `[[test]]` is one module here, so `cargo
//! test` links one binary for them. One file's tests: `cargo test -p glia-rules --test
//! rules_tests <file>::`.

include!(concat!(env!("OUT_DIR"), "/test_mods.rs"));
