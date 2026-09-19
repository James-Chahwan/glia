//! LD.3b — `glia find`, driving the real binary over the packet's two-repo
//! acceptance build (a Python service in `api/`, a TypeScript client in
//! `web/`, merged with `--with`).
//!
//! The `[find] query=` stderr line is the LD.3b fired_on marker; asserting it
//! here makes it a tested contract, and relaying it lets
//! `cargo test -p glia-cli --test find_cli -- --nocapture 2>&1 | grep '^\[find\] query='`
//! show it.

use std::path::PathBuf;
use std::process::{Command, Output};

use repo_graph_engine::find::{FindOptions, find_nodes};

const USERS_PY: &str = "def get_user(uid):\n    return load(uid)\n\n\ndef load(uid):\n    return {\"id\": uid}\n\n\nclass UserService:\n    def get_user(self, uid):\n        return get_user(uid)\n\n\ndef user_service_helper():\n    return UserService()\n";
const USERS_TS: &str = "export function getUser(id: string) {\n  return fetch(`/users/${id}`);\n}\n\nexport function getUsers() {\n  return fetch('/users');\n}\n";

/// A per-test directory under the system temp dir, removed on drop (the cli
/// crate has no `tempfile` dev-dependency).
struct Fixture(PathBuf);

impl Fixture {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!("glia-find-cli-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("api")).unwrap();
        std::fs::create_dir_all(root.join("web")).unwrap();
        std::fs::write(root.join("api/users.py"), USERS_PY).unwrap();
        std::fs::write(root.join("web/users.ts"), USERS_TS).unwrap();
        Fixture(root)
    }

    fn repo(&self, sub: &str) -> String {
        self.0.join(sub).to_string_lossy().into_owned()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn glia(args: &[&str]) -> Output {
    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .args(args)
        .env("GLIA_NO_PERSIST", "1")
        .output()
        .expect("glia runs");
    for line in String::from_utf8_lossy(&out.stderr).lines() {
        if line.starts_with("[find] ") {
            eprintln!("{line}");
        }
    }
    out
}

fn ok(args: &[&str]) -> Output {
    let out = glia(args);
    assert!(
        out.status.success(),
        "glia {args:?} exited {:?}\nstderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

/// The `results` rows of the LD.8a `{results, absence}` envelope; a found
/// answer's `absence` is `null`.
fn rows(out: &Output) -> Vec<serde_json::Value> {
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stdout is JSON");
    assert!(v["absence"].is_null(), "a found answer carries no absence: {v}");
    v["results"].as_array().expect("a `results` array").clone()
}

#[test]
fn json_equals_the_engine_order_across_processes() {
    let fx = Fixture::new("order");
    let (api, web) = (fx.repo("api"), fx.repo("web"));
    let out = ok(&["find", &api, "user", "--with", &web, "--json"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(
            "[find] query='user' matched=11 tiers=name_prefix:4,name_word:6,qname_substring:1 top_k=20"
        ),
        "{stderr}"
    );
    let cli = rows(&out);

    let merged = repo_graph_engine::generate_many(&[api.clone(), web.clone()])
        .expect("generate_many")
        .merged;
    let engine = find_nodes(&merged, "user", &FindOptions::default()).results;
    let engine = serde_json::to_value(&engine).expect("serialises");
    assert_eq!(serde_json::Value::Array(cli.clone()), engine);

    // The record serialises its tier as `match`.
    assert_eq!(cli[0]["match"], "name_prefix", "{cli:#?}");
    assert_eq!(cli.last().unwrap()["qname"], "users::load");
    assert_eq!(cli.last().unwrap()["match"], "qname_substring");

    // A second process, same answer.
    let again = ok(&["find", &api, "user", "--with", &web, "--json"]);
    assert_eq!(rows(&again), cli);
}

#[test]
fn kind_filter_keeps_only_functions_in_rank_order() {
    let fx = Fixture::new("kind");
    let (api, web) = (fx.repo("api"), fx.repo("web"));
    let all = rows(&ok(&["find", &api, "user", "--with", &web, "--json"]));
    for k in ["METHOD", "CLASS", "MODULE"] {
        assert!(all.iter().any(|r| r["kind"] == k), "fixture lacks a {k} row: {all:#?}");
    }
    let funcs = rows(&ok(&["find", &api, "user", "--with", &web, "--kind", "FUNCTION", "--json"]));
    assert!(!funcs.is_empty());
    assert!(funcs.iter().all(|r| r["kind"] == "FUNCTION"), "{funcs:#?}");
    let expected: Vec<serde_json::Value> =
        all.iter().filter(|r| r["kind"] == "FUNCTION").cloned().collect();
    assert_eq!(funcs, expected);
}

#[test]
fn unknown_kind_exits_2_and_lists_the_valid_names() {
    let fx = Fixture::new("nope");
    let out = glia(&["find", &fx.repo("api"), "user", "--kind", "NOPE"]);
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unknown --kind 'NOPE'"), "{stderr}");
    assert!(stderr.contains("FUNCTION") && stderr.contains("METHOD"), "{stderr}");
    assert!(out.stdout.is_empty());
}

#[test]
fn table_names_the_tier_and_location() {
    let fx = Fixture::new("table");
    let out = ok(&["find", &fx.repo("api"), "get_user", "--top-k", "1"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    // LD.6: the `live` column, rendered as `blast-radius` renders it; nothing
    // reaches `get_user` from an entrypoint in this fixture.
    assert!(stdout.contains("| match | live | kind | qname | location |"), "{stdout}");
    assert!(
        stdout.contains("| exact_name | ⊘ | FUNCTION | `users::get_user` | users.py:1 |"),
        "{stdout}"
    );
}
