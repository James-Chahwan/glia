//! Rust crate-path call and `use` resolution hook (`RustCrate`): turns
//! `crate::`, `self::`, `super::` and full-crate-path references into edges
//! the generic symbol-table walker cannot reach. Filled by LA.1a; extended by
//! LA.1b and LA.3.
