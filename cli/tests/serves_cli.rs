//! LD.8b / DD.13 — `glia serves`, driving the real binary over two repos in a
//! scratch dir: `billing` and `shop`, each a Flask `app.py` whose `POST
//! /orders` route is handled by a byte-identical `create_order`, both with a
//! `.glia/scip-snapshot/` written the DD.7 way defining it under one SCIP
//! symbol, so the two handlers are one FACT `scip_symbol` identity class.
//!
//! By default the answer is two ROUTE servers (no rule makes routes one
//! class), each handler tagged with the other repo's copy (`handler_identity`
//! in `--json`, `(also in: ...)` in the table), and the engine prints one
//! `[identity] collapse surface=serves` line beside its `[serves]
//! mechanism=` line; asserting both here makes them a tested contract.
//! `--no-collapse` is the raw rows: no identity key, no `[identity]` line.
//! Grep the markers from a run:
//! `glia serves <billing> 'POST /orders' --with <shop> 2>&1 >/dev/null | grep -E '^\[(identity\] collapse surface=serves|serves\] mechanism=)'`.
//!
//! The CJ.3 external rows are covered by `external_sinks_cli.rs`.

use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::Value;

const APP: &str = "from flask import Flask\n\napp = Flask(__name__)\n\n\n\
@app.route(\"/orders\", methods=[\"POST\"])\ndef create_order():\n    return {\"ok\": True}\n";
const SYMBOL: &str = "scip-python python shop-svc 1.0 `app`/create_order().";

/// The two repos under the system temp dir, removed on drop (the cli crate
/// has no `tempfile` dev-dependency).
struct Repos(PathBuf);

impl Drop for Repos {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl Repos {
    fn new(tag: &str) -> Self {
        use glia_code_domain::snapshots::{
            ScipDefRow, ScipDocumentRecord, ScipMeta, ScipSymbolRecord, source_hash, write_scip,
        };
        let root = std::env::temp_dir().join(format!("glia-dd13-serves-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for repo in ["billing", "shop"] {
            let dir = root.join(repo);
            std::fs::create_dir_all(&dir).expect("repo dir");
            std::fs::write(dir.join("app.py"), APP).expect("write app.py");
            let documents = vec![ScipDocumentRecord {
                path: "app.py".to_string(),
                language: "python".to_string(),
                source_hash: source_hash(APP.as_bytes()),
                defs: vec![ScipDefRow { s: 0, line: 6, name: "create_order".to_string() }],
                refs: Vec::new(),
            }];
            let symbols = vec![ScipSymbolRecord { id: 0, symbol: SYMBOL.to_string(), implements: Vec::new() }];
            let meta = ScipMeta::new("scip-python".to_string(), "0.6.0".to_string(), String::new(), 0);
            write_scip(&dir, meta, &documents, &symbols).expect("write snapshot");
        }
        Repos(root)
    }

    /// `glia serves <billing> 'POST /orders' --with <shop> <args>`.
    fn serves(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_glia"))
            .arg("serves")
            .arg(self.0.join("billing"))
            .arg("POST /orders")
            .arg("--with")
            .arg(self.0.join("shop"))
            .args(args)
            .env("GLIA_NO_PERSIST", "1")
            .output()
            .expect("run glia")
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// The stderr lines starting with `prefix`.
fn lines(o: &Output, prefix: &str) -> Vec<String> {
    stderr(o).lines().filter(|l| l.starts_with(prefix)).map(str::to_string).collect()
}

#[test]
fn identity_classes_tag_handlers_unless_no_collapse() {
    let r = Repos::new("collapse");

    let out = r.serves(&["--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        lines(&out, "[identity] collapse"),
        ["[identity] collapse surface=serves classes=1 walk_classes=1 folded=0 rows=2"],
        "{}",
        stderr(&out)
    );
    assert_eq!(
        lines(&out, "[serves] mechanism="),
        ["[serves] mechanism=http channel='POST /orders' servers=2 match=exact"],
        "{}",
        stderr(&out)
    );
    let v: Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    let rows = v["results"].as_array().expect("results list");
    assert_eq!(rows.len(), 2, "two ROUTEs, kept: {v}");
    let mut also: Vec<&str> = Vec::new();
    for row in rows {
        assert_eq!(row["kind"], "ROUTE", "{row}");
        assert!(row.get("identity").is_none(), "no rule pairs routes: {row}");
        assert_eq!(row["handlers"][0]["qname"], "app::create_order", "{row}");
        let hi = &row["handler_identity"];
        assert_eq!(hi.as_array().map(Vec::len), Some(1), "{row}");
        assert_eq!(hi[0]["qname"], "app::create_order", "{row}");
        assert_eq!(hi[0]["identity"]["tier"], "fact", "{row}");
        assert_eq!(hi[0]["identity"]["rules"][0], "scip_symbol", "{row}");
        assert_eq!(hi[0]["identity"]["also_in"][0]["file"], "app.py", "{row}");
        also.push(hi[0]["identity"]["also_in"][0]["repo"].as_str().expect("a repo label"));
    }
    also.sort_unstable();
    assert_eq!(also, ["billing", "shop"], "each handler names the other repo's copy");

    let out = r.serves(&[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);
    let line = |repo: &str| format!("`app::create_order` app.py:{n} (also in: {repo} app.py:{n})", n = handler_line(rows));
    assert!(text.contains(&line("shop")) && text.contains(&line("billing")), "{text}");

    let out = r.serves(&["--json", "--no-collapse"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(lines(&out, "[identity] collapse").is_empty(), "{}", stderr(&out));
    assert!(!stdout(&out).contains("identity"), "{}", stdout(&out));
    let raw: Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(raw["results"].as_array().map(Vec::len), Some(2), "{raw}");
    let out = r.serves(&["--no-collapse"]);
    assert!(!stdout(&out).contains("(also in: "), "{}", stdout(&out));
}

/// The handler's 1-based line (both copies are one file, so one line).
fn handler_line(rows: &[Value]) -> i64 {
    rows[0]["handlers"][0]["line"].as_i64().expect("a located handler")
}
