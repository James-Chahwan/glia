//! The `glia` binary's rpath (FB.14). With the default `embed` feature the
//! binary links ORT's WebGPU provider, which needs `libwebgpu_dawn` beside it
//! at run time (ort-sys's `copy-dylibs` links it into target/<profile>/; the
//! wheels bundle it next to the executable): `$ORIGIN` on Linux and
//! `@loader_path` on macOS let `./target/debug/glia` and an installed `glia`
//! find it without LD_LIBRARY_PATH. Windows loads a DLL from the exe's
//! directory already. Harmless when nothing is linked dynamically.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("linux") => println!("cargo:rustc-link-arg-bins=-Wl,-rpath,$ORIGIN"),
        Ok("macos") => println!("cargo:rustc-link-arg-bins=-Wl,-rpath,@loader_path"),
        _ => {}
    }
}
