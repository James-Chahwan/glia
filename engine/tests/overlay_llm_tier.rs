//! HC.1: a pair edge that rests on a model-written overlay stanza tiers
//! heuristic. The `http` resolver names such a pairing's rule with the suffix
//! `/overlay_llm` (an `origin = "llm"` `[[wrapper]]` minted the client
//! ENDPOINT, an overlay `[constants]` pin folded its path, or an llm
//! `[[route_prefix]]` mounted the gateway path it reached), and `why::tier_of`
//! reads that suffix as heuristic with the note
//! `rests on a model-written overlay stanza (<base rule>)`.
//!
//! Each case is a two-repo build (web client, flask api) in a temp dir:
//! the stanza unmarked (the loader's default, `llm`), the same stanza
//! `origin = "human"` (stays derived, unchanged), and no overlay (no rule
//! carries the suffix). `[constants]` has no per-key origin (every pin is
//! `llm`), so its control is the no-overlay build only.
//!
//! Probe (HEAD a0c1bf1a, before the fix): an unmarked
//! `[[wrapper]] call="request" kind="http" method_arg=0 path_arg=1` over
//! `fetch(String(path))` and a flask `GET /users`:
//! `glia why web --with api endpoint:GET:/users "GET /users"` printed
//! `| HTTP_CALLS | weak | derived | resolver:http | exact | src/client.ts:6 |  |`.

use std::path::{Path, PathBuf};

use glia_activation::profile::Tier;
use glia_code_domain::edge_category;
use glia_code_domain::evidence::Evidence;
use glia_core::Edge;
use glia_engine::{BuildOptions, GenerateResult, generate_many_opts, why};

/// The flask api: `GET /users` and `GET /api/users`.
const API_PY: &str = "from flask import Flask\n\napp = Flask(__name__)\n\n\n@app.route(\"/users\", methods=[\"GET\"])\ndef list_users():\n    return []\n\n\n@app.route(\"/api/users\", methods=[\"GET\"])\ndef api_users():\n    return []\n";

/// A `request(method, path)` wrapper whose path goes through `String(..)`,
/// so no inferred wrapper reads it: only the overlay stanza mints its site.
const OPAQUE_CLIENT_TS: &str = "export function request(method: string, path: string) {\n  return fetch(String(path), { method });\n}\n\nexport async function loadUsers() {\n  return request('GET', '/users');\n}\n";

/// The same wrapper with its path straight into `fetch`: an inferred wrapper
/// (FH.4b, `pass:inferred_wrapper`), no overlay needed.
const INFERRED_CLIENT_TS: &str = "export function request(method: string, path: string) {\n  return fetch(path, { method });\n}\n\nexport async function loadUsers() {\n  return request('GET', '/users');\n}\n";

/// An env-read base the source cannot bind.
const GATEWAY_CLIENT_TS: &str = "import axios from 'axios';\n\nconst GATEWAY = process.env.GATEWAY_URL;\n\nexport async function listUsers() {\n  return axios.get(`${GATEWAY}/users`);\n}\n";

/// A gateway path no route serves as written.
const MOUNTED_CLIENT_TS: &str = "import axios from 'axios';\n\nexport async function listUsers() {\n  return axios.get('/svc/users');\n}\n";

fn wrapper_stanza(origin: &str) -> String {
    format!(
        "version = 1\n\n[[wrapper]]\ncall = \"request\"\nkind = \"http\"\nmethod_arg = 0\npath_arg = 1\n{origin}"
    )
}

fn mount_stanza(origin: &str) -> String {
    format!("version = 1\n\n[[route_prefix]]\nscope = \".\"\nprefix = \"/svc\"\n{origin}")
}

const PIN_STANZA: &str = "version = 1\n\n[constants]\nGATEWAY = \"/api\"\n";

const HUMAN: &str = "origin = \"human\"\n";

/// A fresh, empty temp dir for one test.
fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("glia_hc1_{}_{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("mkdir");
    d
}

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).expect("mkdir");
    }
    std::fs::write(path, body).expect("write");
}

/// `web/src/client.ts = client` with `web_overlay`, the flask api with
/// `api_overlay`, built as two repos.
fn build(
    tag: &str,
    client: &str,
    web_overlay: Option<&str>,
    api_overlay: Option<&str>,
) -> GenerateResult {
    let root = tmp(tag);
    write(&root, "web/src/client.ts", client);
    write(&root, "api/app.py", API_PY);
    if let Some(o) = web_overlay {
        write(&root, "web/.glia/overlay.toml", o);
    }
    if let Some(o) = api_overlay {
        write(&root, "api/.glia/overlay.toml", o);
    }
    let dirs = [root.join("web"), root.join("api")].map(|d| d.to_string_lossy().into_owned());
    let built = generate_many_opts(&dirs, false, &BuildOptions::default());
    std::fs::remove_dir_all(&root).ok();
    built.expect("build")
}

fn qname(r: &GenerateResult, id: glia_core::NodeId) -> String {
    r.merged
        .graphs
        .iter()
        .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
        .unwrap_or_default()
}

/// The HTTP_CALLS cross edge `from -> to` (by qname).
fn http_edge<'r>(r: &'r GenerateResult, from: &str, to: &str) -> &'r Edge {
    let hits: Vec<&Edge> = r
        .merged
        .cross_edges
        .iter()
        .filter(|e| e.category == edge_category::HTTP_CALLS)
        .filter(|e| qname(r, e.from) == from && qname(r, e.to) == to)
        .collect();
    assert_eq!(hits.len(), 1, "one {from} -> {to}: {:?}", all_http(r));
    hits[0]
}

/// `(from, to, rule)` of every HTTP_CALLS cross edge, for messages.
fn all_http(r: &GenerateResult) -> Vec<(String, String, Option<String>)> {
    r.merged
        .cross_edges
        .iter()
        .filter(|e| e.category == edge_category::HTTP_CALLS)
        .map(|e| {
            (
                qname(r, e.from),
                qname(r, e.to),
                Evidence::of(e).and_then(|ev| ev.rule),
            )
        })
        .collect()
}

fn rule(e: &Edge) -> Option<String> {
    Evidence::of(e).and_then(|ev| ev.rule)
}

/// The edge rests on the model: rule `<base>/overlay_llm`, why's tier
/// heuristic with the note naming the base rule, on both the edge reading and
/// `why_edge`'s answer.
fn assert_model_rested(r: &GenerateResult, from: &str, to: &str, base: &str) {
    let e = http_edge(r, from, to);
    assert_eq!(rule(e), Some(format!("{base}/overlay_llm")));
    let note = format!("rests on a model-written overlay stanza ({base})");
    assert_eq!(why::edge_tier(e), (Tier::Heuristic, Some(note.clone())));
    let ans = why::why_edge(&r.merged, from, to, Some("HTTP_CALLS")).expect("why");
    assert!(ans.found, "{from} -> {to}");
    let row = &ans.edges[0];
    assert_eq!(row.tier, "heuristic");
    assert_eq!(row.note.as_deref(), Some(note.as_str()));
    assert_eq!(
        row.rule.as_deref(),
        Some(format!("{base}/overlay_llm").as_str())
    );
}

/// The edge is the resolver's plain pairing: rule `base`, derived, no note.
fn assert_derived(r: &GenerateResult, from: &str, to: &str, base: &str) {
    let e = http_edge(r, from, to);
    assert_eq!(rule(e).as_deref(), Some(base));
    assert_eq!(why::edge_tier(e), (Tier::Derived, None));
    let ans = why::why_edge(&r.merged, from, to, Some("HTTP_CALLS")).expect("why");
    assert_eq!(ans.edges[0].tier, "derived");
}

/// No HTTP_CALLS edge carries the suffix.
fn assert_no_suffix(r: &GenerateResult) {
    let all = all_http(r);
    assert!(
        all.iter()
            .all(|(_, _, rule)| !rule.as_deref().unwrap_or("").ends_with("/overlay_llm")),
        "{all:?}"
    );
}

#[test]
fn llm_wrapper_pairing_is_heuristic() {
    let r = build(
        "wrapper_llm",
        OPAQUE_CLIENT_TS,
        Some(&wrapper_stanza("")),
        None,
    );
    assert_model_rested(&r, "endpoint:GET:/users", "GET /users", "exact");
}

#[test]
fn human_wrapper_pairing_stays_derived() {
    let r = build(
        "wrapper_human",
        OPAQUE_CLIENT_TS,
        Some(&wrapper_stanza(HUMAN)),
        None,
    );
    assert_derived(&r, "endpoint:GET:/users", "GET /users", "exact");
    assert_no_suffix(&r);
}

#[test]
fn inferred_wrapper_without_overlay_is_unchanged() {
    let r = build("wrapper_none", INFERRED_CLIENT_TS, None, None);
    assert_derived(&r, "endpoint:GET:/users", "GET /users", "exact");
    assert_no_suffix(&r);
}

#[test]
fn pinned_constant_pairing_is_heuristic() {
    let r = build("pin_llm", GATEWAY_CLIENT_TS, Some(PIN_STANZA), None);
    assert_model_rested(&r, "endpoint:GET:/api/users", "GET /api/users", "exact");
}

#[test]
fn unpinned_base_fold_is_unchanged() {
    let r = build("pin_none", GATEWAY_CLIENT_TS, None, None);
    assert!(!all_http(&r).is_empty(), "the env base still base-folds");
    assert_no_suffix(&r);
    for e in r
        .merged
        .cross_edges
        .iter()
        .filter(|e| e.category == edge_category::HTTP_CALLS)
    {
        assert_eq!(why::edge_tier(e).0, Tier::Derived);
    }
}

#[test]
fn llm_route_prefix_pairing_is_heuristic() {
    let r = build(
        "mount_llm",
        MOUNTED_CLIENT_TS,
        None,
        Some(&mount_stanza("")),
    );
    assert_model_rested(&r, "endpoint:GET:/svc/users", "GET /users", "exact");
}

#[test]
fn human_route_prefix_pairing_stays_derived() {
    let r = build(
        "mount_human",
        MOUNTED_CLIENT_TS,
        None,
        Some(&mount_stanza(HUMAN)),
    );
    assert_derived(&r, "endpoint:GET:/svc/users", "GET /users", "exact");
    assert_no_suffix(&r);
}

#[test]
fn no_mount_carries_no_suffix() {
    let r = build("mount_none", MOUNTED_CLIENT_TS, None, None);
    assert_no_suffix(&r);
}
