//! LA.5 — `glia arch` folds the native shells a cross-platform app framework
//! generates (Flutter / React Native `android/`, `ios/`, `macos/`, `linux/`,
//! `windows/`) into the app that owns them, driving the real binary.
//!
//! The fixture is the exact shape `glia arch quokka-stack` got wrong: a
//! Flutter-generated `android/app` Gradle root is an A8.5 PROJECT anchor, so
//! longest-prefix keying listed it as a service with one Kotlin file. The
//! precision half is `tools/plugin` — Gradle under an npm dir, but not in a
//! platform dir — which stays its own service.

use std::path::PathBuf;
use std::process::{Command, Output};

fn fixture(name: &str) -> String {
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    here.parent()
        .expect("cli/ has a parent")
        .join("tests/fixtures")
        .join(name)
        .to_str()
        .expect("fixture path is UTF-8")
        .to_string()
}

fn glia_arch_json(name: &str) -> Output {
    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .args(["arch", &fixture(name), "--json"])
        .env("GLIA_NO_PERSIST", "1")
        .output()
        .expect("glia runs");
    assert!(
        out.status.success(),
        "glia arch exited {:?}\nstderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    // Relay the child's markers so `-- --nocapture | grep '\[arch\] '` sees them.
    for line in String::from_utf8_lossy(&out.stderr).lines() {
        if line.starts_with("[arch] ") {
            eprintln!("{line}");
        }
    }
    out
}

#[test]
fn arch_folds_platform_host_roots_into_their_app() {
    let out = glia_arch_json("arch_platform_hosts");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(
            "[arch] platform hosts folded: 5 (mobile/android->mobile, \
             mobile/android/app->mobile, mobile/linux->mobile, rn/android->rn, \
             rn/android/app->rn)"
        ),
        "fold marker missing or wrong:\n{stderr}"
    );
    assert!(
        stderr.contains("[arch] 4 services, 0 links (keying=project_roots "),
        "summary marker missing or wrong:\n{stderr}"
    );

    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stdout is JSON");
    assert_eq!(v["keying"], "project_roots");
    let services = v["services"].as_array().expect("services array");
    let ids: Vec<&str> = services.iter().filter_map(|s| s["id"].as_str()).collect();
    // No `mobile/android/app`, `mobile/linux` or `rn/android/app` row; the
    // Gradle plugin under the npm `tools/` dir is NOT a platform shell.
    assert_eq!(ids, ["mobile", "rn", "server", "tools/plugin"]);

    let svc = |id: &str| {
        services
            .iter()
            .find(|s| s["id"] == id)
            .unwrap_or_else(|| panic!("service {id} present"))
    };
    let langs = |id: &str| -> Vec<String> {
        svc(id)["languages"]
            .as_array()
            .expect("languages array")
            .iter()
            .filter_map(|l| l.as_str().map(String::from))
            .collect()
    };
    let mobile = langs("mobile");
    assert!(mobile.iter().any(|l| l == "dart"), "mobile languages: {mobile:?}");
    // The shells' files now count under the app: Dart + the android Kotlin
    // activity + the linux runner.
    assert_eq!(svc("mobile")["files"], 3, "mobile: {}", svc("mobile"));
    // RN app: index.js + the android Java activity.
    assert_eq!(svc("rn")["files"], 2, "rn: {}", svc("rn"));
    assert_eq!(svc("tools/plugin")["files"], 1, "tools/plugin: {}", svc("tools/plugin"));
}
