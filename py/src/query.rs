//! **query** (DB.6): `PyGraph.query` answers a Datalog program and
//! `PyGraph.query_gql` a GQL read query over the graph, each as the engine's
//! `glia_engine::rules::QueryAnswer` in one native dict (LD.2), from this
//! module's own `#[pymethods] impl PyGraph` block (no `ModuleFns`). A parse,
//! analysis, budget or evaluation error raises `ValueError` whose text is the
//! engine's rendered diagnostics. docs/rules.md is the language reference.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use glia_engine::rules::{QueryAnswer, QueryOptions, RuleError, query, query_gql};
use glia_graph::MergedGraph;

use crate::convert::to_py;
use crate::graph::PyGraph;

/// The surface name the engine's `[query]` marker and budget hint read: a budget
/// stop's text then names `max_rows=<n>`, this module's parameter.
const SURFACE: &str = "py";

/// The source name a Datalog program's diagnostics carry (`<query>:<line>:<col>:
/// error: ...`); the engine turns it into `<gql>` for a GQL query.
const SOURCE: &str = "<query>";

/// The options both methods pass: `max_rows` is the engine's row budget (`None`
/// keeps glia-rules' default).
fn options(
    outputs: Option<Vec<String>>,
    limit: Option<usize>,
    explain: bool,
    max_rows: Option<usize>,
) -> QueryOptions {
    let mut o = QueryOptions::default();
    o.outputs = outputs.unwrap_or_default();
    o.limit = limit;
    o.explain = explain;
    o.row_budget = max_rows;
    o.source_name = SOURCE.to_string();
    o.surface = SURFACE;
    o
}

/// An engine answer as JSON text, its absence counting the build's
/// `unparsed_files`, or the error's rendered diagnostics.
fn answer_json(
    answer: Result<QueryAnswer, RuleError>,
    unparsed_files: usize,
) -> Result<String, String> {
    let mut a = answer.map_err(|e| e.rendered)?;
    if let Some(absence) = a.absence.as_mut() {
        absence.unparsed_files = unparsed_files;
    }
    serde_json::to_string(&a).map_err(|e| e.to_string())
}

/// The whole body of [`PyGraph::query`], minus pyo3 — kept pyo3-free so
/// `cargo test -p glia-py` covers it (see the crate doc).
fn query_json(
    merged: &MergedGraph,
    program: &str,
    opts: &QueryOptions,
    unparsed_files: usize,
) -> Result<String, String> {
    answer_json(query(merged, program, opts), unparsed_files)
}

/// The whole body of [`PyGraph::query_gql`], minus pyo3.
fn query_gql_json(
    merged: &MergedGraph,
    text: &str,
    opts: &QueryOptions,
    unparsed_files: usize,
) -> Result<String, String> {
    answer_json(query_gql(merged, text, opts), unparsed_files)
}

#[pymethods]
impl PyGraph {
    /// **query** (DB.6): answer the Datalog `program` over this graph.
    /// docs/rules.md is the language: rules over `node/2`, `edge/3`,
    /// `cell/3`, the code attributes (`qname/2`, `file/2`, ...), the builtins
    /// (`reaches`, `in_scope`, `under`, the string tests) and the prelude's
    /// structural predicates (`calls`, `depends`, `carry`, `effect`,
    /// `unreached`, ...).
    ///
    /// `outputs` names the predicates answered (`"name"` or `"name/arity"`;
    /// a prelude predicate may be asked for), in that order; `None` answers
    /// the program's sinks, every predicate it defines that no rule of
    /// another predicate reads. `limit` cuts each output's rows after the
    /// sort. `explain=True` gives every shown row its `witness`. `max_rows`
    /// is the row budget (`None`: the evaluator's default); a program that
    /// derives more rows stops with a ValueError saying so, never a silently
    /// truncated answer.
    ///
    /// Returns a dict `{language, outputs, results, absence}`: `language`
    /// `"datalog"`; `outputs` one `{predicate, arity, columns, total, shown}`
    /// per output (`columns` the first rule's head variables, `total` its
    /// rows, `shown` those returned after `limit`); `results` the rows, output
    /// by output, each `{predicate, values, tier, file, line}` plus `caveats`
    /// and `witness` when set. A value is `{"node": {qname, kind, file,
    /// line}}`, `{"str": ...}` or `{"int": ...}`; rows sort by their rendered
    /// values (qnames, never node ids, so the order is checkout-stable).
    /// `file` / `line` locate the row's first node value, `line` 1-based.
    /// `tier` is `fact` (read at a site), `derived` (paired by a resolver or
    /// pass) or `heuristic` (an overlay, a name guess, git history): the
    /// weakest input of the row's strongest derivation. A row derived through
    /// a negation carries `caveats`, the coverage notes of every edge
    /// category the negated part reads, since a blind extraction looks like a
    /// missing edge; negation never lowers a tier. `witness` (with
    /// `explain=True`) is the row's minimal derivation's leaf edges,
    /// `{from_qname, to_qname, category, file, line, emitter, tier, note}`,
    /// at most 100. `absence` is None unless no output has a row; then it is
    /// the FACT-tier absence dict (`reason` `no_match`, the edge categories
    /// the outputs read as its `mechanisms`).
    ///
    /// Raises ValueError with the rendered diagnostics,
    /// `<query>:<line>:<col>: error: ...` one per line: a parse error, an
    /// analysis error (an unknown node kind / edge category / cell type /
    /// predicate, an arity mismatch, an unsafe variable, an unstratifiable
    /// negation, a rule defining a prelude predicate, an output no rule
    /// defines), the row budget (naming `max_rows=<n>`) or an evaluation
    /// error. Prints `[query] lang=datalog outputs=<o> rows=<r> fact=<f>
    /// derived=<d> heuristic=<h> caveated=<c> surface=py` on stderr.
    #[pyo3(signature = (program, outputs=None, limit=None, explain=false, max_rows=None))]
    fn query(
        &self,
        py: Python<'_>,
        program: &str,
        outputs: Option<Vec<String>>,
        limit: Option<usize>,
        explain: bool,
        max_rows: Option<usize>,
    ) -> PyResult<Py<PyAny>> {
        let opts = options(outputs, limit, explain, max_rows);
        let json = query_json(&self.merged, program, &opts, self.parse_errors.len())
            .map_err(PyValueError::new_err)?;
        to_py(py, Ok(json))
    }

    /// **query_gql** (DB.6): answer a GQL read query (`MATCH ... [WHERE ...]
    /// RETURN ... [ORDER BY ...] [LIMIT n]`) over this graph. It is a read
    /// subset of GQL / openCypher, compiled to a Datalog program and answered
    /// by the same evaluator (docs/rules.md, `## GQL read subset`): a label is
    /// a node kind, a relationship type an edge category, a property a code
    /// attribute, `-[:CALLS*]->` reachability. Rows are DISTINCT (set
    /// semantics, `RETURN DISTINCT` changes nothing); two relationship
    /// patterns may bind the same edge (homomorphism, unlike openCypher); a
    /// node without a property the query reads drops the row (no nulls).
    ///
    /// Returns the dict `query` returns, with `language` `"gql"` and one
    /// output, `result`, whose `columns` are the RETURN items' aliases, else
    /// their text (`a.qname`, `count(*)`). A property is a `{"str": ...}` or
    /// `{"int": ...}` value, a node variable a located node. A row of
    /// properties (`RETURN h.qname, s.qname`) carries the `file` / `line` of
    /// the first property's node, `h`, as `query`'s `x(h, s)` does; rows equal
    /// in every shown column stay one row; an aggregate's row is a group and
    /// is not located. A path variable
    /// (`p = (a)-[:CALLS*]->(b) RETURN p`) is the row's rendered path
    /// (`a -CALLS-> b`), its witness kept. ORDER BY sorts by rendered values
    /// (DESC flips a key; ties keep the rendered order), the query's LIMIT
    /// cuts them, then `limit` cuts again; `total` keeps the uncut count.
    /// `explain` and `max_rows` are `query`'s; tiers, caveats and the absence
    /// are as there.
    ///
    /// Raises ValueError with the rendered diagnostics,
    /// `<gql>:<line>:<col>: error: ...`: a GQL construct outside the subset, an
    /// unknown label / type / property, the row budget (naming
    /// `max_rows=<n>`) or an evaluation error. Prints `[query] lang=gql ...
    /// surface=py` on stderr.
    #[pyo3(signature = (query, limit=None, explain=false, max_rows=None))]
    fn query_gql(
        &self,
        py: Python<'_>,
        query: &str,
        limit: Option<usize>,
        explain: bool,
        max_rows: Option<usize>,
    ) -> PyResult<Py<PyAny>> {
        let opts = options(None, limit, explain, max_rows);
        let json = query_gql_json(&self.merged, query, &opts, self.parse_errors.len())
            .map_err(PyValueError::new_err)?;
        to_py(py, Ok(json))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// DB.6: `query` / `query_gql` are transport only — pin the documented
    /// dict on an empty graph, the options reaching the engine on a real
    /// build (outputs, limit, explain, max_rows) and the rendered errors. The
    /// evaluation is covered by `engine/tests/rules_query.rs`.
    #[test]
    fn query_returns_the_engine_answer() {
        let empty = MergedGraph::new(Vec::new());
        let json = query_json(
            &empty,
            "x(a) :- node(a, \"MODULE\").",
            &options(None, None, false, None),
            2,
        )
        .expect("answers");
        assert_eq!(
            json,
            concat!(
                r#"{"language":"datalog","outputs":[{"predicate":"x","arity":1,"columns":["a"],"total":0,"shown":0}],"#,
                r#""results":[],"absence":{"tier":"FACT","reason":"no_match","query":"x(a) :- node(a, \"MODULE\").","#,
                r#""note":"no row of x/1 in this graph","mechanisms":[],"caveats":[],"suggestions":[],"#,
                r#""nodes_searched":0,"unparsed_files":2}}"#
            )
        );
        let err = query_json(
            &empty,
            "x(a) :- node(a, \"ROUTEE\").",
            &options(None, None, false, None),
            0,
        )
        .expect_err("an unknown kind is refused");
        assert!(err.contains("unknown node kind"), "{err}");
        assert!(err.contains("ROUTEE"), "{err}");
        assert!(err.starts_with("<query>:1:"), "{err}");
        let gql = query_gql_json(
            &empty,
            "MATCH (a:ROUTEE) RETURN a",
            &options(None, None, false, None),
            0,
        )
        .expect_err("an unknown label is refused");
        assert!(
            gql.starts_with("<gql>:1:") && gql.contains("ROUTEE"),
            "{gql}"
        );
    }

    const FILES: [(&str, &str); 7] = [
        ("web/pyproject.toml", "[project]\nname = \"web\"\n"),
        ("shared/pyproject.toml", "[project]\nname = \"shared\"\n"),
        ("services/api/pyproject.toml", "[project]\nname = \"api\"\n"),
        (
            "web/app.py",
            "from shared.util import helper\nfrom services.api.public.facade import get\n\n\n\
             def page():\n    return helper()\n\n\ndef other():\n    return get()\n",
        ),
        (
            "shared/util.py",
            "from services.api.internal.db import query\n\n\ndef helper():\n    return query()\n",
        ),
        (
            "services/api/public/facade.py",
            "from services.api.internal.db import write\n\n\ndef get():\n    return write()\n",
        ),
        (
            "services/api/internal/db.py",
            "def query():\n    return 1\n\n\ndef write():\n    return 2\n",
        ),
    ];

    /// The values of `rows[i]`, each a node's qname, a string or an int, as text.
    fn rendered(v: &serde_json::Value) -> Vec<Vec<String>> {
        let rows = v["results"].as_array().cloned().unwrap_or_default();
        rows.iter()
            .map(|r| {
                let vals = r["values"].as_array().cloned().unwrap_or_default();
                vals.iter()
                    .map(|x| match (&x["node"], &x["str"], &x["int"]) {
                        (n, _, _) if n.is_object() => n["qname"].as_str().unwrap_or("").to_string(),
                        (_, s, _) if s.is_string() => s.as_str().unwrap_or("").to_string(),
                        (_, _, i) => i.to_string(),
                    })
                    .collect()
            })
            .collect()
    }

    /// DB.6 on DB.2a's fx1 without its overlay edge (the fixture
    /// py/tests/surface/test_query.py builds): the same calls the Python test
    /// makes, so the dict it asserts is the one these bodies produce.
    #[test]
    fn query_options_reach_the_engine() {
        let root = std::env::temp_dir().join(format!("glia-db6-query-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (rel, src) in FILES {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().expect("a parent dir")).expect("mkdir");
            std::fs::write(p, src).expect("write fixture");
        }
        let built = glia_engine::generate_one(root.to_str().expect("utf-8 temp path"));
        let _ = std::fs::remove_dir_all(&root);
        let merged = built.expect("build").merged;
        let run = |text: &str, o: QueryOptions| -> serde_json::Value {
            let json = query_json(&merged, text, &o, 0).unwrap_or_else(|e| panic!("{e}"));
            serde_json::from_str(&json).expect("valid JSON")
        };
        let web_calls = "x(a, b) :- calls(a, b), in_scope(a, \"web\").";

        let a = run(web_calls, options(None, None, false, None));
        assert_eq!(
            a["outputs"],
            serde_json::json!([{"predicate": "x", "arity": 2, "columns": ["a", "b"], "total": 2, "shown": 2}])
        );
        assert_eq!(
            rendered(&a),
            [
                ["web::app::other", "services::api::public::facade::get"],
                ["web::app::page", "shared::util::helper"],
            ]
        );
        let r0 = &a["results"][0];
        assert_eq!(
            r0["values"][0]["node"],
            serde_json::json!({"qname": "web::app::other", "kind": "FUNCTION", "file": "web/app.py", "line": 9})
        );
        assert_eq!(
            (&r0["tier"], &r0["file"], &r0["line"]),
            (&"fact".into(), &"web/app.py".into(), &9.into())
        );
        assert!(
            r0.get("witness").is_none() && r0.get("caveats").is_none(),
            "{r0}"
        );
        assert!(a["absence"].is_null());

        // outputs= and limit= reach the engine.
        let two = "x(a) :- calls(a, _), in_scope(a, \"web\").\ny(a) :- calls(_, a), in_scope(a, \"shared\").";
        let only = run(two, options(Some(vec!["y".to_string()]), None, false, None));
        assert_eq!(only["outputs"][0]["predicate"], "y");
        assert_eq!(only["outputs"].as_array().map(Vec::len), Some(1));
        assert_eq!(rendered(&only), [["shared::util::helper"]]);
        let cut = run(web_calls, options(None, Some(1), false, None));
        assert_eq!(
            (&cut["outputs"][0]["total"], &cut["outputs"][0]["shown"]),
            (&2.into(), &1.into())
        );

        // explain= gives each row its one witness edge.
        let ex = run(web_calls, options(None, None, true, None));
        for r in ex["results"].as_array().into_iter().flatten() {
            assert_eq!(r["witness"].as_array().map(Vec::len), Some(1), "{r}");
        }
        let w = &ex["results"][0]["witness"][0];
        assert_eq!(
            (
                &w["from_qname"],
                &w["to_qname"],
                &w["category"],
                &w["file"],
                &w["tier"]
            ),
            (
                &"web::app::other".into(),
                &"services::api::public::facade::get".into(),
                &"CALLS".into(),
                &"web/app.py".into(),
                &"fact".into()
            ),
            "{w}"
        );

        // max_rows= is the row budget, and its stop names the py parameter.
        let stop = query_json(
            &merged,
            "x(a) :- node(a, _).",
            &options(None, None, false, Some(1)),
            0,
        )
        .expect_err("one row is over budget");
        assert!(stop.contains("max_rows=<n>"), "{stop}");
        let roomy = run(
            "x(a) :- node(a, _).",
            options(None, None, false, Some(1000)),
        );
        assert!(
            roomy["outputs"][0]["total"].as_u64().unwrap_or(0) > 1,
            "{roomy}"
        );
        let unknown = query_json(
            &merged,
            web_calls,
            &options(Some(vec!["nope".to_string()]), None, false, None),
            0,
        )
        .expect_err("an output no rule defines");
        assert!(unknown.contains("nope"), "{unknown}");

        // GQL: the compiled columns, ORDER BY / LIMIT, then limit=.
        let gql = "MATCH (a:FUNCTION)-[:CALLS]->(b) RETURN a.qname, count(b) AS n ORDER BY n DESC";
        let g = |text: &str, o: QueryOptions| -> serde_json::Value {
            let json = query_gql_json(&merged, text, &o, 0).unwrap_or_else(|e| panic!("{e}"));
            serde_json::from_str(&json).expect("valid JSON")
        };
        let q = g(gql, options(None, None, false, None));
        assert_eq!(q["language"], "gql");
        assert_eq!(q["outputs"][0]["predicate"], "result");
        assert_eq!(
            q["outputs"][0]["columns"],
            serde_json::json!(["a.qname", "n"])
        );
        assert_eq!(
            rendered(&q),
            [
                ["services::api::public::facade::get", "1"],
                ["shared::util::helper", "1"],
                ["web::app::other", "1"],
                ["web::app::page", "1"],
            ]
        );
        assert!(q["results"][0]["values"][0]["str"].is_string(), "{q}");
        assert!(q["results"][0]["values"][1]["int"].is_i64(), "{q}");
        // An aggregate's row is a group: not located.
        assert!(
            q["results"][0]["file"].is_null() && q["results"][0]["line"].is_null(),
            "{q}"
        );
        // DT.8b: a RETURN of properties is located at the first property's node.
        let props = g(
            "MATCH (a:FUNCTION)-[:CALLS]->(b) WHERE a.qname STARTS WITH 'web::' \
             RETURN a.qname, b.qname",
            options(None, None, false, None),
        );
        assert_eq!(
            rendered(&props),
            [
                ["web::app::other", "services::api::public::facade::get"],
                ["web::app::page", "shared::util::helper"],
            ]
        );
        let at: Vec<(&serde_json::Value, &serde_json::Value)> = props["results"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|r| (&r["file"], &r["line"]))
            .collect();
        assert_eq!(
            at,
            [
                (&"web/app.py".into(), &9.into()),
                (&"web/app.py".into(), &5.into())
            ]
        );
        let lim = g(
            &format!("{gql} LIMIT 3"),
            options(None, Some(2), false, None),
        );
        assert_eq!(
            (&lim["outputs"][0]["total"], &lim["outputs"][0]["shown"]),
            (&4.into(), &2.into())
        );
        let bad = query_gql_json(
            &merged,
            "MATCH (a:FUNCTION) RETURN",
            &options(None, None, false, None),
            0,
        )
        .expect_err("a RETURN with no item");
        assert!(bad.starts_with("<gql>:1:"), "{bad}");
    }
}
