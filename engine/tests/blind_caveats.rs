//! FH.9 - answers say when they are blind: an answer that reads a mechanism
//! `health::boundary_health` rates BLIND or DEGRADED on this graph carries a
//! FACT `blind` row naming the mechanism and the measure.
//!
//! The two trees are FH.2a's (`engine/tests/health.rs`): a gin server serving
//! `GET /api/r<i>` and a TS client. In the OK tree the client fetches every
//! route, so HTTP pairs them all; in the BLIND tree it fetches `/gw/api/v<i>`
//! through a gateway prefix the server does not serve, so no ENDPOINT pairs
//! (HTTP BLIND `clients_unpaired`). Each tree is committed through the LE.1b
//! git fixture, then `load0`'s body is edited, a helper moved and a dead file
//! deleted, so the rev answers (contract-breaks, review, diff-impact) have a
//! change to read.
//!
//! - `blind_tree_every_answer_says_so`: contract-breaks, review (and its
//!   markdown line), serves, trace, flows, diff-impact, blast-radius and gaps
//!   each carry exactly one `http` row, BLIND, with the health row's measure.
//! - `ok_tree_no_answer_says_so`: the same answers carry no row, and their
//!   JSON has no `blind` key.
//! - `mechanism_names`: the category / kind -> mechanism table and the
//!   one-line table form.
//! - `immich_contract_breaks` (`#[ignore]`): `GLIA_FH9_IMMICH` names a git
//!   work tree of immich; its contract-breaks against HEAD carries exactly
//!   the client mechanisms health rates BLIND / DEGRADED there, and the run
//!   prints health's rows and the boundary_health time.

mod git_fixture;

use git_fixture::GitRepo;
use glia_engine::absence::BlindCaveat;
use glia_engine::contract_breaks::{ContractBreakArgs, contract_breaks_vs_rev};
use glia_engine::diff_impact::diff_impact_vs_rev;
use glia_engine::gaps::{GapsOptions, gaps_report};
use glia_engine::health::{
    Verdict, blast_mechanisms, blind_caveats, blind_line, boundary_health, mechanism_of_category,
    mechanism_of_kind,
};
use glia_engine::review::{MarkdownOptions, ReviewArgs, render_markdown, review_vs_rev};
use glia_engine::serves::serves;
use glia_engine::trace::{TraceOptions, cross_stack_trace, entry_flows};
use glia_engine::{BlastOptions, GenerateResult, blast_radius, generate_one};
use std::path::PathBuf;

const SERVER_GO_MOD: &str =
    "module example.com/server\n\ngo 1.22\n\nrequire github.com/gin-gonic/gin v1.9.1\n";

/// A gin server serving `GET /api/r<i>` for `i` in `0..n`.
fn gin_main(n: usize) -> String {
    let mut main = String::from(
        "package main\n\nimport \"github.com/gin-gonic/gin\"\n\nfunc handle(c *gin.Context) {\n\tc.JSON(200, gin.H{})\n}\n\nfunc main() {\n\tr := gin.Default()\n",
    );
    for i in 0..n {
        main.push_str(&format!("\tr.GET(\"/api/r{i}\", handle)\n"));
    }
    main.push_str("\tr.Run()\n}\n");
    main
}

/// The client module: `load<i>` fetches `<prefix>/api/<seg><i>`; `edited`
/// changes `load0`'s body (an extra statement) without moving its fetch.
fn api_ts(n: usize, prefix: &str, seg: &str, edited: bool) -> String {
    let mut api = String::new();
    if !prefix.is_empty() {
        api.push_str("import { gw } from './gateway';\n\n");
    }
    for i in 0..n {
        let extra = if edited && i == 0 {
            "  const tag = 'edited';\n  console.log(tag);\n"
        } else {
            ""
        };
        api.push_str(&format!(
            "export async function load{i}() {{\n{extra}  return fetch('{prefix}/api/{seg}{i}');\n}}\n\n"
        ));
    }
    api
}

/// The committed tree, then `load0` edited in the work tree.
fn tree(blind: bool) -> GitRepo {
    let repo = GitRepo::init();
    let (n, prefix, seg) = if blind {
        (12, "/gw", "v")
    } else {
        (10, "", "r")
    };
    repo.write("server/go.mod", SERVER_GO_MOD);
    repo.write("server/main.go", &gin_main(n));
    repo.write(
        "web/package.json",
        "{\"name\":\"web\",\"version\":\"1.0.0\"}\n",
    );
    if blind {
        repo.write(
            "web/src/gateway.ts",
            "export function gw(path: string) {\n  return fetch('/gw' + path);\n}\n",
        );
    }
    repo.write("web/src/api.ts", &api_ts(n, prefix, seg, false));
    repo.write(
        "web/src/util.ts",
        "export function trim(s: string) {\n  return s.trim();\n}\n",
    );
    repo.write(
        "web/src/legacy.ts",
        "export function legacy() {\n  return 1;\n}\n",
    );
    repo.commit("base");
    // The change: `load0` edited, a helper moved, a dead file deleted.
    repo.write("web/src/api.ts", &api_ts(n, prefix, seg, true));
    repo.git_mv("web/src/util.ts", "web/src/strings.ts");
    repo.remove("web/src/legacy.ts");
    repo
}

/// The work tree's build.
fn build(repo: &GitRepo) -> GenerateResult {
    generate_one(repo.root().to_str().expect("utf-8 temp path")).expect("build")
}

fn roots_of(r: &GenerateResult) -> Vec<(u64, PathBuf)> {
    r.repo_roots
        .iter()
        .map(|(id, p)| (*id, PathBuf::from(p)))
        .collect()
}

/// `(surface, rows, JSON)` of every listed answer over `repo`.
fn answers(repo: &GitRepo) -> Vec<(&'static str, Vec<BlindCaveat>, String)> {
    let json = |v: &dyn erased::Json| v.json();
    let built = build(repo);
    let merged = &built.merged;
    let mut out: Vec<(&'static str, Vec<BlindCaveat>, String)> = Vec::new();

    let cb = contract_breaks_vs_rev(repo.path(), "HEAD", &ContractBreakArgs::default())
        .expect("contract-breaks");
    out.push(("contract_breaks", cb.blind.clone(), json(&cb)));

    let rv = review_vs_rev(repo.path(), "HEAD", &ReviewArgs::default()).expect("review");
    let md = render_markdown(&rv, &MarkdownOptions::default());
    let md_line = blind_line(&rv.blind);
    match &md_line {
        Some(line) => assert!(md.contains(&format!("\n{line}\n")), "{md}"),
        None => assert!(!md.contains("\nblind: "), "{md}"),
    }
    out.push(("review", rv.blind.clone(), json(&rv)));

    let sv = serves(merged, "GET /api/r0", "auto").expect("serves");
    out.push(("serves", sv.blind.clone(), json(&sv)));

    let tr = cross_stack_trace(merged, "load0", &TraceOptions::default());
    assert!(!tr.hops.is_empty(), "load0 reaches its ENDPOINT: {tr:#?}");
    out.push(("trace", tr.blind.clone(), json(&tr)));

    let flows = entry_flows(merged, &built.repo_labels, 6);
    assert!(!flows.is_empty(), "the routes are entry flows");
    let route_flow = flows
        .iter()
        .find(|f| f.entry.kind == "ROUTE")
        .expect("a ROUTE flow");
    out.push(("flows", route_flow.blind.clone(), json(route_flow)));

    let di =
        diff_impact_vs_rev(repo.path(), "HEAD", &BlastOptions::default()).expect("diff-impact");
    assert!(!di.impact.results.is_empty(), "load0's radius: {di:#?}");
    out.push(("diff_impact", di.blind.clone(), json(&di)));

    // FH.8b (FH.9's handoff): the standalone answer carries its own rows,
    // the ones health gives for the mechanisms it read.
    let br = blast_radius(merged, &["load0"], &BlastOptions::default());
    assert!(!br.results.is_empty(), "load0's radius");
    let rows = blind_caveats(merged, "blast_radius", &blast_mechanisms(&br));
    assert_eq!(br.blind, rows, "BlastRadius.blind is health's reading");
    out.push(("blast_radius", br.blind.clone(), json(&br)));

    let gaps = gaps_report(merged, &roots_of(&built), &GapsOptions::default()).expect("gaps");
    out.push(("gaps", gaps.blind.clone(), json(&gaps)));
    out
}

/// A serde JSON form of any answer, so the table above can hold them all.
mod erased {
    pub trait Json {
        fn json(&self) -> String;
    }
    impl<T: serde::Serialize> Json for T {
        fn json(&self) -> String {
            serde_json::to_string(self).expect("json")
        }
    }
}

#[test]
fn blind_tree_every_answer_says_so() {
    let repo = tree(true);
    let built = build(&repo);
    let health = boundary_health(&built.merged);
    let http = health
        .mechanisms
        .iter()
        .find(|m| m.mechanism == "http")
        .expect("http row");
    assert_eq!(
        (http.verdict, http.reason),
        (Verdict::Blind, "clients_unpaired"),
        "{http:#?}"
    );
    for (surface, rows, json) in answers(&repo) {
        assert_eq!(rows.len(), 1, "{surface}: {rows:#?}");
        let row = &rows[0];
        assert_eq!(
            (row.tier, row.mechanism, row.verdict, row.reason),
            ("fact", "http", Verdict::Blind, "clients_unpaired"),
            "{surface}"
        );
        assert_eq!(row.measure, http.measure, "{surface}");
        if !json.is_empty() {
            assert!(
                json.contains(&format!(
                    "\"blind\":[{{\"tier\":\"fact\",\"mechanism\":\"http\",\"verdict\":\"BLIND\",\"reason\":\"clients_unpaired\",\"measure\":\"{}\"}}]",
                    http.measure
                )),
                "{surface}: {json}"
            );
        }
        assert_eq!(
            blind_line(&rows),
            Some(format!("blind: http ({})", http.measure)),
            "{surface}"
        );
    }
}

#[test]
fn ok_tree_no_answer_says_so() {
    let repo = tree(false);
    let built = build(&repo);
    let health = boundary_health(&built.merged);
    assert!(
        health.mechanisms.iter().all(|m| m.verdict == Verdict::Ok),
        "{health:#?}"
    );
    for (surface, rows, json) in answers(&repo) {
        assert!(rows.is_empty(), "{surface}: {rows:#?}");
        assert!(!json.contains("\"blind\""), "{surface}: {json}");
        assert_eq!(blind_line(&rows), None, "{surface}");
    }
}

#[test]
fn mechanism_names() {
    assert_eq!(mechanism_of_category("HTTP_CALLS"), Some("http"));
    assert_eq!(mechanism_of_category("QUEUE_FLOWS"), Some("queue"));
    assert_eq!(mechanism_of_category("WS_CONNECTS"), Some("ws"));
    assert_eq!(mechanism_of_category("CALLS"), None);
    assert_eq!(mechanism_of_kind("ENDPOINT"), Some("http"));
    assert_eq!(mechanism_of_kind("ROUTE"), Some("http"));
    assert_eq!(mechanism_of_kind("GRPC_SERVICE"), Some("grpc"));
    assert_eq!(mechanism_of_kind("QUEUE_CONSUMER"), Some("queue"));
    assert_eq!(mechanism_of_kind("FUNCTION"), None);
    assert_eq!(blind_line(&[]), None);
}

#[test]
#[ignore = "needs GLIA_FH9_IMMICH: a git work tree of immich"]
fn immich_contract_breaks() {
    let Ok(root) = std::env::var("GLIA_FH9_IMMICH") else {
        panic!("set GLIA_FH9_IMMICH to a git work tree of immich");
    };
    let cb = contract_breaks_vs_rev(&root, "HEAD", &ContractBreakArgs::default())
        .expect("contract-breaks");
    let built = generate_one(&root).expect("build");
    let started = std::time::Instant::now();
    let health = boundary_health(&built.merged);
    eprintln!(
        "immich boundary_health ms={:.1}",
        started.elapsed().as_secs_f64() * 1000.0
    );
    for m in &health.mechanisms {
        eprintln!(
            "immich health {} {} {} ({})",
            m.mechanism,
            m.verdict.as_str(),
            m.reason,
            m.measure
        );
    }
    eprintln!("immich contract-breaks blind: {:?}", blind_line(&cb.blind));
    // The answer's rows are exactly health's non-OK client mechanisms.
    let expected: Vec<&str> = health
        .mechanisms
        .iter()
        .filter(|m| m.verdict != Verdict::Ok && m.mechanism != "queue")
        .map(|m| m.mechanism)
        .collect();
    let got: Vec<&str> = cb.blind.iter().map(|b| b.mechanism).collect();
    assert_eq!(got, expected);
}
