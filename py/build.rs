//! The extension's rpath (FB.14). A wheel's extension links only ORT's CPU
//! provider (feature `embed`), but a workspace build unifies glia-embed's
//! features with the CLI's (`gpu-webgpu`), and then the cdylib links
//! `libwebgpu_dawn` too, which ort-sys's `copy-dylibs` puts beside it in
//! target/<profile>/: `$ORIGIN` (Linux) / `@loader_path` (macOS) let such a
//! dev-tree extension load. Windows loads a DLL from the module's directory
//! already. Harmless when nothing is linked dynamically.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("linux") => println!("cargo:rustc-link-arg-cdylib=-Wl,-rpath,$ORIGIN"),
        Ok("macos") => println!("cargo:rustc-link-arg-cdylib=-Wl,-rpath,@loader_path"),
        _ => {}
    }
}
