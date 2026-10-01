//! CJ.3 — `glia effects` and `glia serves` label a sink outside the build.
//!
//! The tree is `engine/tests/http_external.rs`'s (the `http-external-host`
//! bench fixture's shape): a web client calling
//! `https://nominatim.openstreetmap.org/search` from the call's own literal
//! beside an Express server that serves `GET /search` too. CG.4a marks that
//! call site external, CG.4b stamps the ENDPOINT ORIGIN `external` and leaves
//! it unpaired; these two answers say so instead of reading like an unpaired
//! in-repo call.
//!
//! The `[effects-external] sinks=<n> hosts=<k>` stderr line is CJ.3's fired_on
//! marker (beside `[serves] ... match=exact,external`); asserting both here
//! makes them a tested contract, and relaying them lets
//! `cargo test -p glia-cli --test external_sinks_cli -- --nocapture 2>&1 | grep -E '^\[(effects-external|serves)\]'`
//! show them.

use std::path::PathBuf;
use std::process::{Command, Output};

const APP_JS: &str = "const express = require('express');

const app = express();

app.get('/search', (req, res) => res.json([]));
app.get('/products', (req, res) => res.json([]));
app.get('/orders', (req, res) => res.json([]));

app.listen(3000);
";
const SERVER_PACKAGE: &str =
    "{\"name\":\"server\",\"version\":\"1.0.0\",\"dependencies\":{\"express\":\"^4.18.0\"}}\n";
const WEB_PACKAGE: &str = "{\"name\":\"web\",\"version\":\"1.0.0\"}\n";
const ENVIRONMENT_TS: &str = "export const environment = {
  apiUrl: 'https://api.shop.io',
};
";
const GEO_TS: &str = "import { environment } from './environments/environment';

export async function forwardGeocode(q: string) {
  return fetch(`https://nominatim.openstreetmap.org/search?q=${q}&format=json`);
}

export async function listProducts() {
  return fetch(`${environment.apiUrl}/products`);
}

export async function listOrders() {
  return fetch('https://api.shop.io/orders');
}
";

/// A per-test directory under the system temp dir, removed on drop (the cli
/// crate has no `tempfile` dev-dependency).
struct Fixture(PathBuf);

impl Fixture {
    fn new(tag: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("glia-cj3-external-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (rel, text) in [
            ("server/app.js", APP_JS),
            ("server/package.json", SERVER_PACKAGE),
            ("web/package.json", WEB_PACKAGE),
            ("web/src/environments/environment.ts", ENVIRONMENT_TS),
            ("web/src/geo.ts", GEO_TS),
        ] {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().expect("a parent dir")).expect("mkdir");
            std::fs::write(p, text).expect("write fixture file");
        }
        Fixture(root)
    }

    fn path(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run `glia <args>` with persistence off; relay and return the marker lines
/// starting with `prefix`.
fn glia(args: &[&str], prefix: &str) -> (Output, Vec<String>) {
    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .args(args)
        .env("GLIA_NO_PERSIST", "1")
        .output()
        .expect("glia runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let markers: Vec<String> = stderr
        .lines()
        .filter(|l| l.starts_with(prefix))
        .map(str::to_string)
        .collect();
    for m in &markers {
        eprintln!("{m}");
    }
    (out, markers)
}

#[test]
fn effects_names_the_external_host() {
    let fx = Fixture::new("effects");
    let seed = "web::src::geo::forwardGeocode";
    let (out, markers) = glia(&["effects", &fx.path(), seed], "[effects-external]");
    assert!(out.status.success(), "{out:?}");
    assert_eq!(markers, ["[effects-external] sinks=1 hosts=1"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let row = stdout
        .lines()
        .find(|l| l.contains("| `endpoint:GET:/search @web` |"))
        .unwrap_or_else(|| panic!("the /search sink row: {stdout}"));
    assert!(row.starts_with("| http_call |"), "{row}");
    assert!(
        row.ends_with("| external: `nominatim.openstreetmap.org` |"),
        "the downstream cell names the host: {row}"
    );

    let (out, _) = glia(
        &["effects", &fx.path(), seed, "--json"],
        "[effects-external]",
    );
    assert!(out.status.success(), "{out:?}");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON answer");
    let row = &v["effects"][0];
    assert_eq!(row["qname"], "endpoint:GET:/search @web", "{v}");
    assert_eq!(
        row["external_hosts"],
        serde_json::json!(["nominatim.openstreetmap.org"])
    );
    assert_eq!(row["downstream"], serde_json::json!([]));

    // An in-repo sink: no marker line, its route downstream.
    let (out, markers) = glia(
        &["effects", &fx.path(), "web::src::geo::listOrders"],
        "[effects-external]",
    );
    assert!(out.status.success(), "{out:?}");
    assert!(markers.is_empty(), "{markers:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("| `GET /orders @server` (HTTP_CALLS) |"),
        "{stdout}"
    );
    assert!(!stdout.contains("external:"), "{stdout}");
}

#[test]
fn serves_lists_the_external_row_after_the_route() {
    let fx = Fixture::new("serves");
    let (out, markers) = glia(&["serves", &fx.path(), "GET /search"], "[serves]");
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        markers,
        ["[serves] mechanism=http channel='GET /search' servers=2 match=exact,external"]
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.contains("_(nothing serves it)_"), "{stdout}");
    let route = stdout
        .find("| exact (")
        .unwrap_or_else(|| panic!("the route row: {stdout}"));
    let external = stdout
        .find("| external (")
        .unwrap_or_else(|| panic!("the external row: {stdout}"));
    assert!(route < external, "the route first: {stdout}");
    let row = stdout[external..].lines().next().unwrap_or_default();
    assert!(
        row.contains("| ENDPOINT | `endpoint:GET:/search @web` |"),
        "{row}"
    );
    assert!(
        row.ends_with("| external: `nominatim.openstreetmap.org` |"),
        "{row}"
    );

    let (out, _) = glia(&["serves", &fx.path(), "GET /search", "--json"], "[serves]");
    assert!(out.status.success(), "{out:?}");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON answer");
    assert!(v["absence"].is_null(), "{v}");
    let results = v["results"].as_array().expect("results");
    assert_eq!(results.len(), 2, "{v}");
    assert_eq!(results[0]["qname"], "GET /search @server");
    assert_eq!(results[0]["external_hosts"], serde_json::json!([]));
    assert_eq!(results[1]["match"], "external");
    assert_eq!(results[1]["kind"], "ENDPOINT");
    assert_eq!(results[1]["handlers"], serde_json::json!([]));
    assert_eq!(
        results[1]["external_hosts"],
        serde_json::json!(["nominatim.openstreetmap.org"])
    );

    // A URL naming another host: the route alone.
    let (out, markers) = glia(
        &["serves", &fx.path(), "GET https://other.example.org/search"],
        "[serves]",
    );
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        markers,
        [
            "[serves] mechanism=http channel='GET https://other.example.org/search' servers=1 match=exact"
        ]
    );
}
