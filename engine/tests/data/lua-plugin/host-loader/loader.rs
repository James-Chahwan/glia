use std::path::Path;

// As employed in maki (maki-lua/src/loader.rs): the loader discovers plugin
// directories and reads each one's manifest. The literal below is the
// `manifest_literal` LOADS anchor; the include_str! is the embedded-module
// IMPORTS anchor.
fn load_all(dir: &Path) {
    let manifest = dir.join("plugin.toml");
    let _ = manifest;
}

const EMBEDDED: &str = include_str!("../tool-registration/init.lua");
