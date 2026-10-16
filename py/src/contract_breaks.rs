//! **contract_breaks** (CC.8b): the contract-break check against a git rev,
//! over the engine's `contract_breaks::contract_breaks_vs_rev` (CC.8a) — the
//! pyo3 twin of `glia contract-breaks`.
//!
//! The module function `contract_breaks_vs_rev(repo_path, base="HEAD",
//! avro="backward", breaking_only=False, with_repos=None,
//! models_min_tier="fact")` builds the repo's working tree and the rev
//! itself (LE.1b's graph delta), so it takes a path; `with_repos` (CC.8c)
//! lists client repos built beside the provider on both sides
//! (`contract_breaks_vs_rev_with`), so a client in another repo that lost
//! its provider is an orphan too, and its typed calls are judged as model
//! exchanges (DT.1p). It returns the engine's `ContractBreaks` as a native
//! dict (LD.2): `{base, schemas, models, orphaned_clients, breaking,
//! advisory, absence}`. An `avro` mode outside the engine's `AVRO_MODES`, a
//! `models_min_tier` outside its `MODEL_MIN_TIERS` (both checked before any
//! build) or an engine `Err` (not a git work tree, an unknown rev, a failed
//! build) raises `ValueError`.
//!
//! Transport only: the pairing, the evolution rules, the orphaned clients,
//! the inferred-model judge, the location of every row and the
//! `[contract-breaks] base=..` / `[model-breaks] base=..` markers live in the
//! engine. The helpers the pyo3 entry point delegates to are pyo3-free,
//! so `cargo test -p glia-py` covers them (see the crate doc).

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use glia_engine::contract_breaks::{
    AVRO_MODES, ContractBreakArgs, ContractBreaks, MODEL_MIN_TIERS, contract_breaks_vs_rev_with,
};

use crate::convert::to_py;
use crate::registry::ModuleFns;

/// The engine's arguments from the keyword arguments; `Err` names a mode
/// outside [`AVRO_MODES`] or a tier outside [`MODEL_MIN_TIERS`] in the
/// engine's own words.
fn engine_args(
    avro: &str,
    breaking_only: bool,
    models_min_tier: &str,
) -> Result<ContractBreakArgs, String> {
    let Some(mode) = AVRO_MODES.iter().copied().find(|m| *m == avro) else {
        return Err(format!(
            "unknown avro mode `{avro}`: expected one of {}",
            AVRO_MODES.join(", ")
        ));
    };
    let Some(tier) = MODEL_MIN_TIERS.iter().copied().find(|t| *t == models_min_tier) else {
        return Err(format!(
            "unknown models min tier `{models_min_tier}`: expected one of {}",
            MODEL_MIN_TIERS.join(", ")
        ));
    };
    let mut args = ContractBreakArgs::default();
    args.avro_mode = mode;
    args.breaking_only = breaking_only;
    args.models_min_tier = tier;
    Ok(args)
}

/// The body of [`contract_breaks_vs_rev_py`], minus pyo3. The mode and the
/// tier are checked before anything is built; no `with_repos` is none.
fn rev_answer(
    repo_path: &str,
    base: &str,
    avro: &str,
    breaking_only: bool,
    with_repos: Option<&[String]>,
    models_min_tier: &str,
) -> Result<ContractBreaks, String> {
    let args = engine_args(avro, breaking_only, models_min_tier)?;
    contract_breaks_vs_rev_with(repo_path, base, with_repos.unwrap_or_default(), &args)
}

/// **contract_breaks_vs_rev** (CC.8b): did the working tree's change against
/// git rev `base` (a branch, tag, sha or `"HEAD~N"`; default `"HEAD"`) of the
/// repo at `repo_path` break a client? Every contract (an OpenAPI / AsyncAPI
/// op, a proto / Avro / JSON Schema message type) is paired old -> new and
/// judged by its format's evolution rules; `avro` picks Avro's direction
/// (`"backward"`: the new schema reads what the old one wrote, `"forward"`:
/// the old reads the new, `"full"`: both).
///
/// Returns a dict `{base, schemas, models, orphaned_clients, breaking,
/// advisory, absence}`:
/// `schemas` rows `{kind, key, format, before, after, status, change, tier,
/// note, changes}` (`status` `breaking` | `unknown` | `compatible`; `before`
/// / `after` the declaration at the rev / in the working tree, each `{repo_id,
/// qname, format, file, line}`; `changes` one `{section, field, change,
/// producer, consumer, rule, breaking}` per declared difference, `producer`
/// the rev's side); `orphaned_clients` rows `{client_qname, file, line,
/// category, target_qname, reason, tier}` for every client whose call lost
/// its provider; `models` (DT.1p) the inferred client <-> server model rows
/// `{pairing, direction, client, server, exchanges, status, change, changed,
/// tier, blocking, note, changes}` (`pairing` `http_model` |
/// `graphql_selection`; `exchanges` `{key, tier, client_site, server_site}`;
/// `changes` `{field, change, side, rule, breaking, before, after, client,
/// server}`, each site `{file, line, at_base}`); `breaking` the breaking
/// schema rows plus the orphaned clients plus the BLOCKING model rows (a CI
/// gate fails when it is above 0); `advisory` the breaking model rows below
/// `models_min_tier`; `absence` a dict saying why when nothing is listed,
/// else `None`. `breaking_only=True` lists only the breaking schema and
/// model rows; `breaking`, `advisory` and the orphans are the same either
/// way. `models_min_tier` (`"fact"`, the default, or `"derived"`; check's
/// vocabulary) is the weakest pairing tier at which a breaking model row
/// blocks: every pairing glia 0.5.2 infers is derived, so by default a model
/// row is advisory; a heuristic pairing never blocks.
/// `with_repos` (a list of paths, default `None`) adds client repos: both
/// sides are then built as multi-repo merges, the provider at the rev and at
/// its working tree, each beside the same clients at their own trees, so a
/// client in another repo that lost its provider is in `orphaned_clients`,
/// and its typed calls are judged in `models`. Every client is built twice,
/// once per side.
/// Lines are 1-based. Builds both sides itself and saves the parse-cache
/// sidecar (`<repo>/.glia/graph/parse_cache.bin`, self-gitignored) of the
/// repo and of every client as `generate(incremental=True)` does, never a
/// `.gmap` layout. Raises ValueError on a bad `avro` mode or
/// `models_min_tier` (`unknown models min tier `x`: expected one of fact,
/// derived`, before any build), a client that is not a directory, or a git
/// or build failure.
#[pyfunction]
#[pyo3(name = "contract_breaks_vs_rev", signature = (repo_path, base="HEAD", avro="backward", breaking_only=false, with_repos=None, models_min_tier="fact"))]
fn contract_breaks_vs_rev_py(
    py: Python<'_>,
    repo_path: &str,
    base: &str,
    avro: &str,
    breaking_only: bool,
    with_repos: Option<Vec<String>>,
    models_min_tier: &str,
) -> PyResult<Py<PyAny>> {
    let answer = rev_answer(
        repo_path,
        base,
        avro,
        breaking_only,
        with_repos.as_deref(),
        models_min_tier,
    )
    .map_err(PyValueError::new_err)?;
    to_py(py, serde_json::to_string(&answer))
}

fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(contract_breaks_vs_rev_py, m)?)?;
    Ok(())
}

inventory::submit! { ModuleFns { name: "contract_breaks", add: register } }

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use super::*;

    /// A scratch dir under the system temp dir, removed on drop (`py` has no
    /// `tempfile` dev-dependency).
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// CC.8a's openapi fixture: one GET op whose 200 response declares `id`
    /// and `total`.
    const ORDERS_V1: &str = "openapi: 3.0.0\ninfo:\n  title: orders\n  version: \"1\"\npaths:\n  /orders/{id}:\n    get:\n      operationId: getOrder\n      responses:\n        \"200\":\n          description: ok\n          content:\n            application/json:\n              schema:\n                type: object\n                properties:\n                  id:\n                    type: string\n                  total:\n                    type: number\n";
    const TOTAL: &str = "                  total:\n                    type: number\n";

    fn git(top: &Path, gitconfig: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(["-c", "user.name=glia", "-c", "user.email=glia@example.invalid"])
            .args(["-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main"])
            .arg("-C")
            .arg(top)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", gitconfig)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap_or_else(|e| panic!("CC.8b contract_breaks_vs_rev test needs a `git` binary: {e}"));
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    /// The object the pyo3 entry point returns, as `to_py` hands it to Python.
    fn value(a: &ContractBreaks) -> serde_json::Value {
        serde_json::from_str(&serde_json::to_string(a).expect("serialises")).expect("valid JSON")
    }

    #[test]
    fn modes_map_and_a_bad_one_is_refused_before_any_build() {
        for mode in AVRO_MODES {
            let a = engine_args(mode, false, "fact").expect("an engine mode");
            assert_eq!(a.avro_mode, *mode);
        }
        for tier in MODEL_MIN_TIERS {
            let a = engine_args("backward", false, tier).expect("an engine tier");
            assert_eq!(a.models_min_tier, *tier);
        }
        let err = rev_answer("/nonexistent/dt1p", "HEAD", "backward", false, None, "heuristic")
            .expect_err("bad tier");
        assert_eq!(err, "unknown models min tier `heuristic`: expected one of fact, derived");
        let err = rev_answer("/nonexistent/cc8b", "HEAD", "x", false, None, "fact").expect_err("bad mode");
        assert_eq!(err, "unknown avro mode `x`: expected one of backward, forward, full");
        let err = rev_answer("/nonexistent/cc8b", "HEAD", "backward", false, None, "fact")
            .expect_err("not a directory");
        assert!(!err.contains("avro"), "the mode passed; the engine refused the path: {err}");
    }

    /// CC.8b: the helper behind `contract_breaks_vs_rev` returns the engine's
    /// answer in field order with `breaking == 1` for a removed response
    /// field, and `breaking_only` reaches the engine. The judging itself is
    /// covered by `engine/tests/contract_breaks.rs`.
    #[test]
    fn rev_helper_returns_the_documented_object() {
        let root = std::env::temp_dir().join(format!("glia-cc8b-py-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let _scratch = Scratch(root.clone());
        let top = root.join("repo");
        std::fs::create_dir_all(&top).expect("scratch dir");
        let gitconfig = root.join("gitconfig");
        std::fs::write(&gitconfig, "").expect("empty gitconfig");
        std::fs::write(top.join("openapi.yaml"), ORDERS_V1).expect("write v1");
        git(&top, &gitconfig, &["init", "-q"]);
        git(&top, &gitconfig, &["add", "-A"]);
        git(&top, &gitconfig, &["commit", "-q", "-m", "v1"]);
        std::fs::write(top.join("openapi.yaml"), ORDERS_V1.replace(TOTAL, "")).expect("write v2");

        let repo = top.to_str().expect("utf-8");
        let a = rev_answer(repo, "HEAD", "backward", false, None, "fact").expect("answer");
        let text = serde_json::to_string(&a).expect("serialises");
        let order: Vec<usize> = ["\"base\":", "\"schemas\":", "\"models\":", "\"orphaned_clients\":", "\"breaking\":", "\"advisory\":", "\"absence\":"]
            .iter()
            .map(|k| text.rfind(k).unwrap_or(usize::MAX))
            .collect();
        // A top-level key is its last occurrence: `"breaking":` also names a
        // field change's verdict inside `schemas`.
        assert!(order.windows(2).all(|w| w[0] < w[1]), "field order: {text}");
        let v = value(&a);
        assert_eq!(v["base"], "HEAD");
        assert_eq!(v["breaking"], 1, "{v}");
        assert_eq!((v["advisory"].as_u64(), v["models"].as_array().map(Vec::len)), (Some(0), Some(0)), "{v}");
        assert!(v["absence"].is_null(), "{v}");
        let c = &v["schemas"][0]["changes"][0];
        assert_eq!(
            (c["field"].as_str(), c["rule"].as_str()),
            (Some("total"), Some("response_field_removed")),
            "{v}"
        );

        std::fs::write(
            top.join("openapi.yaml"),
            format!("{ORDERS_V1}                  currency:\n                    type: string\n"),
        )
        .expect("write v3");
        let all = value(&rev_answer(repo, "HEAD", "backward", false, None, "fact").expect("compatible"));
        assert_eq!((all["breaking"].as_u64(), all["schemas"][0]["status"].as_str()), (Some(0), Some("compatible")));
        let only = value(&rev_answer(repo, "HEAD", "full", true, None, "fact").expect("breaking only"));
        assert_eq!(only["schemas"], serde_json::json!([]), "{only}");
        assert_eq!(only["absence"]["reason"], "no_match", "{only}");
    }

    /// CC.8c: `with_repos` reaches the engine. The provider (a git repo)
    /// drops the route a client in a separate, plain dir calls: with the
    /// client the call is an orphaned client, without it nothing is reported,
    /// and a client that is not a directory is refused before any build.
    #[test]
    fn with_repos_reports_a_client_in_another_repo() {
        let root = std::env::temp_dir().join(format!("glia-cc8c-py-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let _scratch = Scratch(root.clone());
        let top = root.join("provider");
        let web = root.join("client").join("web");
        std::fs::create_dir_all(top.join("services/api")).expect("provider dir");
        std::fs::create_dir_all(&web).expect("client dir");
        let gitconfig = root.join("gitconfig");
        std::fs::write(&gitconfig, "").expect("empty gitconfig");
        std::fs::write(top.join("services/api/pyproject.toml"), "[project]\nname = \"api\"\n")
            .expect("write pyproject");
        std::fs::write(
            top.join("services/api/app.py"),
            "from flask import Flask\n\napp = Flask(__name__)\n\n\n@app.route(\"/orders\")\ndef orders():\n    return []\n",
        )
        .expect("write the route");
        std::fs::write(
            web.join("client.py"),
            "import requests\n\n\ndef list_orders():\n    return requests.get(\"http://api/orders\")\n",
        )
        .expect("write the client");
        git(&top, &gitconfig, &["init", "-q"]);
        git(&top, &gitconfig, &["add", "-A"]);
        git(&top, &gitconfig, &["commit", "-q", "-m", "service"]);
        std::fs::remove_file(top.join("services/api/app.py")).expect("remove the route");

        let repo = top.to_str().expect("utf-8");
        let clients = [root.join("client").to_str().expect("utf-8").to_string()];
        let v = value(&rev_answer(repo, "HEAD", "backward", false, Some(&clients), "fact").expect("answer"));
        assert_eq!(v["breaking"], 1, "{v}");
        let o = &v["orphaned_clients"][0];
        assert_eq!(
            (o["client_qname"].as_str(), o["category"].as_str(), o["reason"].as_str(), o["file"].as_str()),
            (Some("endpoint:GET:/orders"), Some("HTTP_CALLS"), Some("target_removed"), Some("web/client.py")),
            "{v}"
        );
        let alone = value(&rev_answer(repo, "HEAD", "backward", false, None, "fact").expect("alone"));
        assert_eq!(alone["orphaned_clients"], serde_json::json!([]), "{alone}");
        let none = value(&rev_answer(repo, "HEAD", "backward", false, Some(&[]), "fact").expect("empty list"));
        assert_eq!(none["orphaned_clients"], serde_json::json!([]), "an empty list is no client: {none}");
        let missing = ["/nonexistent/cc8c-client".to_string()];
        let err = rev_answer(repo, "HEAD", "backward", false, Some(&missing), "fact").expect_err("no such client");
        assert_eq!(err, "not a directory: /nonexistent/cc8c-client");
    }
}
