//! HT.2 — the hidden `glia ids ls|next`: the id blocks and the linked domains'
//! ids, and the next free id of a (domain, table) as a Rust const line.
//!
//! fired_on, grep-able:
//! `cargo test -p glia-cli --test cli_tests ids_cli:: -- --nocapture 2>&1 | grep -o '\[ids\] .*'`

use std::process::Command;

fn glia(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .args(args)
        .output()
        .expect("run glia");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    for line in stderr.lines().filter(|l| l.starts_with("[ids] ")) {
        eprintln!("{line}");
    }
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr,
    )
}

#[test]
fn ls_json_lists_the_blocks_and_the_code_ids() {
    let (code, stdout, stderr) = glia(&["ids", "ls", "--json"]);
    assert_eq!(code, 0, "stderr: {stderr}");
    let marker = stderr
        .lines()
        .find(|l| l.starts_with("[ids] ls "))
        .unwrap_or_else(|| panic!("no [ids] ls marker: {stderr}"));
    assert_eq!(marker, "[ids] ls domains=2 blocks=5 surface=cli");
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("ls --json is JSON");

    let blocks: Vec<(&str, &str, &str)> = json["blocks"]
        .as_array()
        .expect("blocks")
        .iter()
        .map(|b| {
            (
                b["block"].as_str().unwrap_or_default(),
                b["name"].as_str().unwrap_or_default(),
                b["status"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        blocks,
        vec![
            ("0x0000", "code", "registered"),
            ("0x0001", "core", "shared"),
            ("0x0002", "build", "registered"),
            ("0x0003", "goal", "reserved"),
            ("0x8000-0xFFFE", "private", "private"),
        ]
    );

    let domains = json["domains"].as_array().expect("domains");
    let code_domain = domains
        .iter()
        .find(|d| d["graph_type"] == "code")
        .unwrap_or_else(|| panic!("no code domain in {json}"));
    assert_eq!(code_domain["block"], "code");
    let tables: Vec<(&str, u64, u64)> = code_domain["tables"]
        .as_array()
        .expect("tables")
        .iter()
        .map(|t| {
            (
                t["table"].as_str().unwrap_or_default(),
                t["count"].as_u64().unwrap_or_default(),
                t["next"].as_u64().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        tables,
        vec![
            ("node_kind", 50, 51),
            ("edge_category", 38, 39),
            ("cell_type", 27, 28),
        ]
    );
    for t in code_domain["tables"].as_array().expect("tables") {
        let ids = t["ids"].as_array().expect("ids");
        assert_eq!(ids.len() as u64, t["count"].as_u64().unwrap_or_default());
        assert!(ids.iter().all(|i| i["block"] == "code"), "{t}");
        let numbers: Vec<u64> = ids.iter().filter_map(|i| i["id"].as_u64()).collect();
        let mut sorted = numbers.clone();
        sorted.sort_unstable();
        assert_eq!(numbers, sorted, "ids sorted by id");
    }
    // HT.7: the build-target domain is linked too (through
    // glia_engine::targets), its ids in block 2.
    let graph_types: Vec<&str> = domains
        .iter()
        .filter_map(|d| d["graph_type"].as_str())
        .collect();
    assert_eq!(graph_types, ["code", "build-targets"]);
    let build = &domains[1];
    assert_eq!(build["block"], "build");
    for t in build["tables"].as_array().expect("tables") {
        assert_eq!(t["count"], 4, "{t}");
        assert_eq!(t["next"], 0x0002_0005, "{t}");
        assert!(
            t["ids"]
                .as_array()
                .expect("ids")
                .iter()
                .all(|i| i["block"] == "build"),
            "{t}"
        );
    }
    let modules = code_domain["tables"][0]["ids"][0].clone();
    assert_eq!(modules["id"], 1);
    assert_eq!(modules["hex"], "0x00000001");
    assert_eq!(modules["name"], "MODULE");
}

#[test]
fn ls_text_names_each_table_and_next() {
    let (code, stdout, stderr) = glia(&["ids", "ls", "--domain", "code"]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("| 0x0002 | build | registered | build-targets | 0.5.7 |"));
    assert!(stdout.contains("## code (block code)"), "{stdout}");
    assert!(
        stdout.contains("### node_kind: 50 ids, next 51"),
        "{stdout}"
    );
    assert!(
        stdout.contains("### edge_category: 38 ids, next 39"),
        "{stdout}"
    );
    assert!(
        stdout.contains("### cell_type: 27 ids, next 28"),
        "{stdout}"
    );
    assert!(
        stdout.contains("| 1 | 0x00000001 | code | MODULE |"),
        "{stdout}"
    );
}

#[test]
fn next_prints_a_const_line() {
    let (code, stdout, stderr) = glia(&["ids", "next", "--domain", "code", "--table", "cell_type"]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(
        stdout.trim_end(),
        "pub const NAME: CellTypeId = CellTypeId(28); // 0x0000001c, block code"
    );
    assert!(
        stderr
            .lines()
            .any(|l| l == "[ids] next domain=code table=cell_type id=0x0000001c block=code"),
        "{stderr}"
    );

    let (code, stdout, _) = glia(&["ids", "next", "--domain", "code", "--table", "node_kind"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("NodeKindId(51);"), "{stdout}");

    let (code, stdout, stderr) = glia(&[
        "ids",
        "next",
        "--domain",
        "code",
        "--table",
        "edge_category",
        "--private",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(
        stdout.trim_end(),
        "pub const NAME: EdgeCategoryId = EdgeCategoryId(glia_core::ids::id_in(0x8000, 1)); \
         // 0x80000001, block private"
    );
    assert!(stderr.contains("block=private"), "{stderr}");
}

#[test]
fn next_in_the_build_block() {
    let (code, stdout, stderr) = glia(&[
        "ids",
        "next",
        "--domain",
        "build-targets",
        "--table",
        "node_kind",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(
        stdout.trim_end(),
        "pub const NAME: NodeKindId = NodeKindId(glia_core::ids::id_in(BUILD, 5)); \
         // 0x00020005, block build"
    );
    assert!(
        stderr
            .lines()
            .any(|l| l
                == "[ids] next domain=build-targets table=node_kind id=0x00020005 block=build"),
        "{stderr}"
    );
}

#[test]
fn unknown_domain_or_table_exits_2() {
    let (code, stdout, stderr) = glia(&["ids", "next", "--domain", "nope", "--table", "cell_type"]);
    assert_eq!(code, 2, "stderr: {stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unknown domain `nope`"), "{stderr}");
    assert!(!stderr.contains("[ids] "), "no marker on failure: {stderr}");

    let (code, _, stderr) = glia(&["ids", "next", "--domain", "code", "--table", "kinds"]);
    assert_eq!(code, 2, "stderr: {stderr}");
    assert!(stderr.contains("unknown table `kinds`"), "{stderr}");

    let (code, _, stderr) = glia(&["ids", "ls", "--domain", "nope"]);
    assert_eq!(code, 2, "stderr: {stderr}");
}
