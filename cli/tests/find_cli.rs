//! LD.3b — `glia find`, driving the real binary over the packet's two-repo
//! acceptance build (a Python service in `api/`, a TypeScript client in
//! `web/`, merged with `--with`).
//!
//! The `[find] query=` stderr line is the LD.3b fired_on marker; asserting it
//! here makes it a tested contract, and relaying it lets
//! `cargo test -p glia-cli --test find_cli -- --nocapture 2>&1 | grep '^\[find\] query='`
//! show it. DD.9's `[identity] collapse surface=find` line is relayed and
//! asserted the same way.

use std::path::PathBuf;
use std::process::{Command, Output};

use glia_engine::find::{FindOptions, find_nodes};

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
        if line.starts_with("[find] ")
            || line.starts_with("[identity] collapse ")
            || line.starts_with("[find-files] ")
        {
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

    let merged = glia_engine::generate_many(&[api.clone(), web.clone()])
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

/// A DD.5 identity fixture repo (committed under bench/substrate-gap).
fn identity_fixture(name: &str, repo: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../bench/substrate-gap/fixtures")
        .join(name)
        .join(repo)
        .to_string_lossy()
        .into_owned()
}

/// DD.9: the package both repos declare is one row naming the other repo;
/// `--no-collapse` is the two raw rows. Before DD.9 the default printed the
/// two rows.
#[test]
fn identity_class_is_one_row_unless_no_collapse() {
    let web = identity_fixture("identity-npm-package", "web");
    let admin = identity_fixture("identity-npm-package", "admin");
    let q = "package:npm:react";
    let out = ok(&["find", &web, q, "--with", &admin, "--json"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("[identity] collapse surface=find classes=2 walk_classes=0 folded=1 rows=1"),
        "{stderr}"
    );
    assert!(stderr.contains("[find] query='package:npm:react' matched=2"), "{stderr}");
    let one = rows(&out);
    assert_eq!(one.len(), 1, "{one:#?}");
    let id = &one[0]["identity"];
    assert_eq!(id["tier"], "fact", "{id}");
    assert_eq!(id["rules"], serde_json::json!(["package_name"]), "{id}");
    assert_eq!(id["diverged"], false, "{id}");
    let also = id["also_in"].as_array().expect("also_in");
    assert_eq!(also.len(), 1, "{id}");
    assert_eq!(also[0]["repo"], "admin", "{id}");
    assert_eq!(also[0]["qname"], q, "{id}");
    assert!(also[0]["file"].is_null() && also[0]["line"].is_null(), "{id}");

    let raw = ok(&["find", &web, q, "--with", &admin, "--no-collapse", "--json"]);
    let stderr = String::from_utf8_lossy(&raw.stderr);
    assert!(!stderr.contains("[identity] collapse "), "--no-collapse builds no view: {stderr}");
    let two = rows(&raw);
    assert_eq!(two.len(), 2, "{two:#?}");
    assert!(two.iter().all(|r| r.get("identity").is_none()), "{two:#?}");
    assert_eq!(two[0]["id"], one[0]["id"], "the collapsed row is the best-ranked member");

    // The table names the other repo in the location cell.
    let table = ok(&["find", &web, q, "--with", &admin]);
    let stdout = String::from_utf8_lossy(&table.stdout);
    assert!(
        stdout.contains("| exact_qname | ⊘ | PACKAGE_DEP | `package:npm:react` | — (also in: admin) |"),
        "{stdout}"
    );
}

/// A located class (DD.5's contract ops in one repo): the table lists each
/// other member as `<repo> <file>:<line>`.
#[test]
fn identity_table_lists_located_members() {
    let repo = identity_fixture("identity-contract-ops", ".");
    let out = ok(&["find", &repo, "GET:/orders"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let row = stdout
        .lines()
        .find(|l| l.starts_with("| qname_suffix |"))
        .unwrap_or_else(|| panic!("{stdout}"));
    assert_eq!(stdout.lines().filter(|l| l.starts_with("| qname_suffix |")).count(), 1, "{stdout}");
    assert!(row.contains(" (also in: "), "{row}");
    assert!(row.contains("features/orders/feature.yaml:"), "the feature op: {row}");
    assert!(!row.contains("diverged"), "{row}");
}

// ---- EB.8: `glia find --files` ----

const ORDERS_PY: &str = "def place_order(uid):\n    return uid\n";
const TASKS_MD: &str = "# Tasks\n\n- [ ] T012 user lookup in users.py\n";

/// The `files` fixture: `users.py`, `orders.py` and a spec-kit task naming
/// `users.py`; with `history`, a history snapshot whose one subject
/// (`order cache warmup`) touched `orders.py`.
fn files_fixture(fx: &Fixture, history: bool) -> String {
    let root = fx.0.join("files");
    std::fs::create_dir_all(root.join("specs/001-users")).unwrap();
    std::fs::write(root.join("users.py"), USERS_PY).unwrap();
    std::fs::write(root.join("orders.py"), ORDERS_PY).unwrap();
    std::fs::write(root.join("specs/001-users/tasks.md"), TASKS_MD).unwrap();
    // The working tree's definitions are read through git (`intent::defs::Tree::Work`).
    for args in [["init", "-q"].as_slice(), &["add", "-A"], &["commit", "-q", "-m", "init"]] {
        let st = Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid", "-c", "commit.gpgsign=false"])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("git");
        assert!(st.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&st.stderr));
    }
    if history {
        use glia_code_domain::snapshots::{
            HistoryCommit, HistoryFile, HistoryMessage, HistoryMeta, write_history_with_messages,
        };
        let c = format!("01{}", "0".repeat(38));
        let commits = [HistoryCommit {
            c: c.clone(),
            t: 1_767_225_601,
            files: vec![HistoryFile { p: "orders.py".into(), a: Some(1), d: Some(0), from: None }],
        }];
        let messages = [HistoryMessage {
            c: c.clone(),
            at: 1_767_225_601,
            s: "order cache warmup".into(),
            b: String::new(),
            br: None,
        }];
        let meta = HistoryMeta::new(c, 2000, None, String::new());
        write_history_with_messages(&root, meta, &commits, &[], Some(&messages)).expect("snapshot");
    }
    root.to_string_lossy().into_owned()
}

#[test]
fn files_ranks_the_id_first_then_the_text() {
    let fx = Fixture::new("files");
    let repo = files_fixture(&fx, true);
    let out = ok(&["find", &repo, "T012 order cache", "--files"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("[find-files] repo=files surface=cli fusion=LX[IDR,T1s] ids=1 idr_files=1 text=present text_files=1 rows=2"),
        "{stderr}"
    );
    assert!(stderr.contains("[intent] text repo=files surface=cli "), "{stderr}");
    assert!(stderr.contains("[idr] repo=files surface=cli "), "{stderr}");
    assert!(stdout.starts_with("# files for `T012 order cache`\n"), "{stdout}");
    assert!(stdout.contains("| id | namespace | tier | route | defined at | state |"), "{stdout}");
    assert!(stdout.contains("| `T012` |"), "{stdout}");
    let at = |s: &str| stdout.find(s).unwrap_or_else(|| panic!("{s} in {stdout}"));
    assert!(at("| 1 | `users.py` | 1 |") < at("| 2 | `orders.py` | 0 |"), "{stdout}");

    let json = ok(&["find", &repo, "T012 order cache", "--files", "--json"]).stdout;
    let text = String::from_utf8_lossy(&json);
    // the envelope, then each row's keys in the engine's field order
    assert!(text.starts_with("{\"results\":[{\"rank\":1,\"file\":\"users.py\",\"idr\":1.0,\"text\":0.0,\"degree\":"), "{text}");
    assert!(text.contains(",\"ids\":[\"T012\"]},{\"rank\":2,"), "{text}");
    let v: serde_json::Value = serde_json::from_slice(&json).expect("json");
    let rows = v["results"].as_array().expect("results");
    assert!(v["absence"].is_null(), "{v}");
    assert_eq!((rows[0]["file"].as_str(), rows[1]["file"].as_str()), (Some("users.py"), Some("orders.py")));
    assert_eq!(rows[0]["ids"], serde_json::json!(["T012"]));
}

#[test]
fn files_absence_names_the_missing_inputs() {
    let fx = Fixture::new("files-none");
    let repo = files_fixture(&fx, false);
    let out = glia(&["find", &repo, "warm the cache", "--files", "--json"]);
    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(v["results"], serde_json::json!([]));
    let note = v["absence"]["note"].as_str().expect("note");
    assert!(note.contains("no id in the query resolved to a defining row"), "{note}");
    assert!(note.contains("no history messages (run glia history sync)"), "{note}");
    assert_eq!(v["absence"]["reason"], "no_match");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("text=absent text_files=0 rows=0"), "{stderr}");
}

#[test]
fn files_refuses_with_and_node_flags() {
    let fx = Fixture::new("files-with");
    let (api, web) = (fx.repo("api"), fx.repo("web"));
    let out = glia(&["find", &api, "user", "--with", &web, "--files"]);
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("error: find --files ranks one repo's files; got 2 repos"), "{stderr}");
    for flag in [["--kind", "FUNCTION"].as_slice(), &["--no-collapse"], &["--scope", "x"]] {
        let mut args = vec!["find", api.as_str(), "user", "--files"];
        args.extend_from_slice(flag);
        let out = glia(&args);
        assert_eq!(out.status.code(), Some(2), "{flag:?}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("cannot be used with"), "{flag:?}");
    }
}
