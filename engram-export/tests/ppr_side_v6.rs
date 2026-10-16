//! DE.5: `--ppr-candidates <K>` writes `<out>.ppr.jsonl` beside the gmap -
//! a header naming the gmap's content digest, then each fact's top-K
//! approximate personalised PageRank neighbours over the exported edges - and
//! changes nothing else the export writes. Without the flag no side file is
//! left beside the gmap.
//!
//! Every test runs the built bin on a scratch dir outside git, with every
//! `--out` outside the exported repo, and `--no-persist` so no parse cache
//! is written into the fixture.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::{Command, Output};

use engram_core::{Content, Gmap};
use glia_engram_export::{meta_path, ppr_side_path, sidecar_path};

/// A call chain `a -> b -> c` beside a lone `d`, and a doc section naming
/// `a`, so the export has Calls, Contains and Documents edges and both
/// Symbol and Proposition facts.
const CHAIN_PY: &str = "def a():
    return b()


def b():
    return c()


def c():
    return 1


def d():
    return 0
";

const README_MD: &str = "# Chain

## Entry

Call `a` to run the chain.
";

const MARKER: &str = "[engram-export] ppr side: ";

struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Scratch {
        let root = std::env::temp_dir().join(format!("glia-de5-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("fx/svc")).unwrap();
        std::fs::create_dir_all(root.join("fx/docs")).unwrap();
        std::fs::create_dir_all(root.join("out")).unwrap();
        std::fs::write(root.join("fx/svc/chain.py"), CHAIN_PY).unwrap();
        std::fs::write(root.join("fx/docs/README.md"), README_MD).unwrap();
        Scratch(root)
    }

    fn out(&self, name: &str) -> PathBuf {
        self.0.join("out").join(format!("{name}.engram-gmap"))
    }

    fn side(&self, name: &str) -> PathBuf {
        ppr_side_path(&self.out(name))
    }

    fn run(&self, name: &str, extra: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_glia-export-engram"))
            .arg(self.0.join("fx"))
            .arg("--out")
            .arg(self.out(name))
            .arg("--no-persist")
            .args(extra)
            .env_remove("GLIA_NO_PERSIST")
            .output()
            .unwrap()
    }

    /// Export to `out/<name>`, asserting exit 0; returns stderr.
    fn export(&self, name: &str, extra: &[&str]) -> String {
        let out = self.run(name, extra);
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert_eq!(out.status.code(), Some(0), "export {name} failed:\n{stderr}");
        stderr
    }

    fn read(&self, path: PathBuf) -> Vec<u8> {
        std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    fn gmap(&self, name: &str) -> Gmap {
        bincode::deserialize(&self.read(self.out(name))).unwrap()
    }

    /// The side file's header and rows, each line parsed.
    fn side_lines(&self, name: &str) -> (serde_json::Value, Vec<serde_json::Value>) {
        let text = String::from_utf8(self.read(self.side(name))).unwrap();
        assert!(text.ends_with('\n'), "JSONL ends with a newline");
        let mut lines = text.lines().map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap());
        let header = lines.next().expect("a header line");
        (header, lines.collect())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn marker_lines(stderr: &str) -> Vec<&str> {
    stderr.lines().filter(|l| l.starts_with(MARKER)).collect()
}

/// `(key, [(candidate, score)])` per row.
fn rows(lines: &[serde_json::Value]) -> Vec<(String, Vec<(String, f64)>)> {
    lines
        .iter()
        .map(|r| {
            let cands = r["candidates"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| (c[0].as_str().unwrap().to_string(), c[1].as_f64().unwrap()))
                .collect();
            (r["key"].as_str().unwrap().to_string(), cands)
        })
        .collect()
}

#[test]
fn side_file_is_deterministic_and_bound_to_its_gmap() {
    let s = Scratch::new("bound");
    let first = s.export("a", &["--ppr-candidates", "5"]);
    let side_a = s.read(s.side("a"));
    // The same out again, and a second out: the same bytes each time.
    s.export("a", &["--ppr-candidates", "5"]);
    s.export("b", &["--ppr-candidates", "5"]);
    assert_eq!(s.read(s.side("a")), side_a, "a rerun writes the same side bytes");
    assert_eq!(s.read(s.side("b")), side_a, "a second out writes the same side bytes");

    let meta: serde_json::Value = serde_json::from_slice(&s.read(meta_path(&s.out("a")))).unwrap();
    let (header, lines) = s.side_lines("a");
    assert_eq!(
        header,
        serde_json::json!({
            "alpha": 0.5,
            "epsilon": 0.001,
            "gmap_digest": meta["gmap_digest"],
            "k": 5,
            "ppr_side": 1,
            "walk": "undirected",
            "weights": "gmap edge weight, 1.0 when none",
        })
    );
    let digest = engram_core::content_digest(&s.read(s.out("a")));
    assert_eq!(header["gmap_digest"], format!("{digest:016x}"));

    // One row per fact, in gmap order; every candidate is a gmap key.
    let gmap = s.gmap("a");
    let facts: Vec<&str> = gmap
        .nodes
        .iter()
        .filter(|n| matches!(n.content, Content::Symbol { .. } | Content::Proposition { .. }))
        .map(|n| n.key.as_str())
        .collect();
    assert_eq!(facts.len(), gmap.nodes.len(), "a glia export holds only Symbols and Propositions");
    let rows = rows(&lines);
    let row_keys: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
    assert_eq!(row_keys, facts);
    let keys: BTreeSet<&str> = gmap.nodes.iter().map(|n| n.key.as_str()).collect();
    assert!(rows.iter().any(|r| !r.1.is_empty()), "some fact has neighbours");
    for (key, cands) in &rows {
        assert!(cands.len() <= 5, "{key}: {cands:?}");
        for (c, score) in cands {
            assert!(keys.contains(c.as_str()), "{key}: candidate {c} is no gmap key");
            assert_ne!(c, key, "a fact is not its own candidate");
            assert!(*score > 0.0 && *score < 1.0, "{key}: {c} {score}");
            assert_eq!(*score, (score * 1e6).round() / 1e6, "rounded to 6 decimals");
        }
        assert!(
            cands.windows(2).all(|w| w[0].1 > w[1].1 || (w[0].1 == w[1].1 && w[0].0 < w[1].0)),
            "{key}: ranked by score, then key: {cands:?}"
        );
    }

    // The marker: one line, the fact count, K, and the side file's path.
    let lines = marker_lines(&first);
    assert_eq!(lines.len(), 1, "{first}");
    let want = format!("{MARKER}{} facts, k=5, ", facts.len());
    assert!(lines[0].starts_with(&want), "{} vs {want}", lines[0]);
    assert!(lines[0].ends_with(&format!("-> {}", s.side("a").display())), "{}", lines[0]);
}

#[test]
fn one_call_hop_ranks_above_two() {
    let s = Scratch::new("hops");
    s.export("g", &["--ppr-candidates", "20"]);
    let (_, lines) = s.side_lines("g");
    let rows = rows(&lines);
    let key = |suffix: &str| {
        rows.iter()
            .map(|r| r.0.clone())
            .find(|k| k.ends_with(suffix))
            .unwrap_or_else(|| panic!("no fact ending {suffix}"))
    };
    let (a, b, c) = (key("chain::a"), key("chain::b"), key("chain::c"));
    let from_a = &rows.iter().find(|r| r.0 == a).unwrap().1;
    let rank = |k: &str| from_a.iter().position(|x| x.0 == k);
    assert!(rank(&b).is_some(), "a's callee is a candidate: {from_a:?}");
    assert!(rank(&b) < rank(&c).or(Some(usize::MAX)), "b (one Calls hop) above c (two): {from_a:?}");
    let score = |k: &str| from_a.iter().find(|x| x.0 == k).map_or(0.0, |x| x.1);
    assert!(score(&b) > score(&c), "{from_a:?}");
}

#[test]
fn the_flag_changes_no_other_output_and_its_absence_leaves_no_side_file() {
    let s = Scratch::new("absent");
    let plain = s.export("plain", &[]);
    assert!(marker_lines(&plain).is_empty(), "no marker without the flag:\n{plain}");
    assert!(!s.side("plain").exists(), "no side file without the flag");
    s.export("flag", &["--ppr-candidates", "3"]);
    assert!(s.side("flag").exists());
    let files = |name: &str| [s.out(name), sidecar_path(&s.out(name)), meta_path(&s.out(name))];
    for (plain, flag) in files("plain").into_iter().zip(files("flag")) {
        assert_eq!(s.read(plain), s.read(flag.clone()), "the flag moved {}", flag.display());
    }

    // A rerun without the flag removes the stale side file: it named a gmap
    // digest that this run may replace.
    s.export("flag", &[]);
    assert!(!s.side("flag").exists(), "a stale side file is removed");
    assert_eq!(s.read(s.out("plain")), s.read(s.out("flag")));
}

#[test]
fn k_outside_one_to_two_hundred_is_refused() {
    let s = Scratch::new("range");
    for k in ["0", "201", "-1", "many"] {
        let out = s.run("r", &["--ppr-candidates", k]);
        assert_eq!(out.status.code(), Some(2), "k={k}: {}", String::from_utf8_lossy(&out.stderr));
        assert!(!s.out("r").exists(), "k={k}: a refused run writes nothing");
    }
    s.export("r", &["--ppr-candidates", "200"]);
    let (header, _) = s.side_lines("r");
    assert_eq!(header["k"], 200);
}
