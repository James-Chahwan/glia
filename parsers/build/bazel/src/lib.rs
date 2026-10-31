//! glia-parser-bazel: the Bazel build-target parser (0.5.4, owner FA.17).
//!
//! It reads the rows a `bazel query` snapshot recorded and extracts targets
//! and their dependencies for the build-target domain (glia-build-domain). It
//! never runs bazel or any process: glia-snapshots' `targets sync` does, and
//! writes the snapshot this crate reads. It sits under the hashed `parsers`
//! root, so a change here moves PARSER_STAMP.
//!
//! Reserved for 0.5.7 (track T9); empty in 0.5.4 to 0.5.6.
