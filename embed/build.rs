//! The worker binary's rpath (FB.14). `glia-embed-worker` built with
//! `gpu-webgpu` links ORT's WebGPU provider, which needs `libwebgpu_dawn`
//! beside the executable (ort-sys's `copy-dylibs` links it into
//! target/<profile>/; the wheels ship it next to the worker in the scripts
//! directory): `$ORIGIN` on Linux and `@loader_path` on macOS. Only the bin
//! gets it, never the library a host links.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("linux") => {
            println!("cargo:rustc-link-arg-bin=glia-embed-worker=-Wl,-rpath,$ORIGIN")
        }
        Ok("macos") => {
            println!("cargo:rustc-link-arg-bin=glia-embed-worker=-Wl,-rpath,@loader_path")
        }
        _ => {}
    }
}
