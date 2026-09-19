//! LD.5 — multi-seed blast radius: one walk and one PPR over a seed list,
//! per-row seed attribution, seed-to-seed links, unresolved names reported.
//!
//! The fixture is a two-service stack written to a tempdir: `web` POSTs
//! `/orders` (`checkout::placeOrder`), `api` serves it with a flask handler
//! (`app::create_order`) that calls `save` (which calls `audit`) and
//! `publish` (a kafka send). Built as `generate_many([web, api])`.
//!
//! Before LD.5 each layer took ONE seed and the wrapper unioned per-seed
//! answers: scores from separate PPR runs, and every seed dropped from the
//! union, so a seed another seed reaches vanished. The single-seed answer
//! must not move: [`BEFORE_SINGLE_SEED`] is the pre-LD.5
//! `blast_radius_by_qname(m, "app::save", "both", 4, None, false, None)` over
//! this same build, captured at the wave HEAD.

use glia_engine::{BlastAnswer, BlastOptions, BlastRadius, blast_radius, generate_many};
use glia_graph::{MergedGraph, Reach};

const APP_PY: &str = "from flask import Flask\nfrom kafka import KafkaProducer\n\napp = Flask(__name__)\nproducer = KafkaProducer()\n\n\ndef audit(order):\n    return order\n\n\ndef publish(order):\n    producer.send('orders', order)\n\n\ndef save(order):\n    return audit(order)\n\n\n@app.route('/orders', methods=['POST'])\ndef create_order():\n    order = {}\n    save(order)\n    publish(order)\n    return order\n";
const CHECKOUT_TS: &str = "export async function placeOrder(body: unknown) {\n  return fetch('/orders', { method: 'POST', body: JSON.stringify(body) });\n}\n";

/// The pre-LD.5 single-seed answer from `app::save` (both ways, depth 4),
/// captured at the wave HEAD (02f07e3) over this build, `id` included: the
/// repos are non-git tempdirs, so their identity is `dir:web` / `dir:api`
/// and the ids do not depend on where the tempdir sits.
const BEFORE_SINGLE_SEED: &str = r#"[{"id":15471702766527969293,"qname":"app::create_order","name":"create_order","kind":"FUNCTION","reason":"CALLS","depth":1,"score":0.1525352412397918,"live":true,"file":"app.py","line":21},{"id":726504284466770552,"qname":"app::audit","name":"audit","kind":"FUNCTION","reason":"CALLS","depth":1,"score":0.1382072412671569,"live":true,"file":"app.py","line":8},{"id":4788532873013058279,"qname":"app::publish","name":"publish","kind":"FUNCTION","reason":"CALLS","depth":2,"score":0.033935409987684226,"live":true,"file":"app.py","line":12},{"id":14906479061705902739,"qname":"POST /orders","name":"POST /orders","kind":"ROUTE","reason":"HANDLED_BY","depth":2,"score":0.022056468955862857,"live":true,"file":"app.py","line":21},{"id":10038803914390831396,"qname":"queue_producer:orders","name":"orders","kind":"QUEUE_PRODUCER","reason":"USES","depth":3,"score":0.010304603690456157,"live":true,"file":"app.py","line":13},{"id":14569927089078431525,"qname":"endpoint:POST:/orders","name":"POST /orders","kind":"ENDPOINT","reason":"HTTP_CALLS","depth":3,"score":0.006874044981633089,"live":false,"file":"checkout.ts","line":2},{"id":16380544205867808889,"qname":"checkout::placeOrder","name":"placeOrder","kind":"FUNCTION","reason":"CALLS","depth":4,"score":0.0017931708150544432,"live":false,"file":"checkout.ts","line":1}]"#;

/// `(tempdir, merged)`: the tempdir must outlive every use of the graph's
/// file paths.
fn fixture() -> (tempfile::TempDir, MergedGraph) {
    let dir = tempfile::tempdir().expect("tempdir");
    let (api, web) = (dir.path().join("api"), dir.path().join("web"));
    std::fs::create_dir_all(&api).expect("api dir");
    std::fs::create_dir_all(&web).expect("web dir");
    std::fs::write(api.join("app.py"), APP_PY).expect("write app.py");
    std::fs::write(web.join("checkout.ts"), CHECKOUT_TS).expect("write checkout.ts");
    let repos = vec![web.display().to_string(), api.display().to_string()];
    let merged = generate_many(&repos).expect("the stack builds").merged;
    (dir, merged)
}

fn opts(direction: Reach, depth: usize) -> BlastOptions {
    let mut o = BlastOptions::default();
    o.direction = direction;
    o.depth = depth;
    o
}

fn row<'a>(r: &'a BlastRadius, qname: &str) -> &'a BlastAnswer {
    r.results
        .iter()
        .find(|a| a.qname == qname)
        .unwrap_or_else(|| panic!("{qname} is in the radius: {:?}", qnames(r)))
}

fn qnames(r: &BlastRadius) -> Vec<&str> {
    r.results.iter().map(|a| a.qname.as_str()).collect()
}

fn assert_score_sorted(r: &BlastRadius) {
    for w in r.results.windows(2) {
        assert!(
            w[0].score > w[1].score || (w[0].score == w[1].score && w[0].id < w[1].id),
            "one ranking, score desc then id asc: {:?}",
            r.results.iter().map(|a| (&a.qname, a.score)).collect::<Vec<_>>()
        );
    }
}

#[test]
fn two_seeds_backward_are_one_walk_attributed_to_the_first_seed() {
    let (_dir, m) = fixture();
    let r = blast_radius(&m, &["app::save", "app::publish"], &opts(Reach::Backward, 6));
    assert!(r.unresolved.is_empty() && r.absence.is_none(), "{:?}", r.unresolved);
    let seeds: Vec<(&str, &str, &str)> =
        r.seeds.iter().map(|s| (s.query.as_str(), s.qname.as_str(), s.kind)).collect();
    assert_eq!(
        seeds,
        [("app::save", "app::save", "FUNCTION"), ("app::publish", "app::publish", "FUNCTION")]
    );
    assert_eq!((r.seeds[0].file.as_deref(), r.seeds[0].line), (Some("app.py"), Some(16)));
    assert!(r.seeds.iter().all(|s| s.linked_seeds.is_empty()), "neither calls the other");

    // Both seeds' only caller is create_order; save is listed first and the
    // multi-source walk expands seeds in input order, so save reached it.
    let create = row(&r, "app::create_order");
    assert_eq!((create.seed.as_str(), create.depth, create.reason), ("app::save", 1, "CALLS"));
    let route = r.results.iter().find(|a| a.kind == "ROUTE").expect("the POST /orders ROUTE");
    let endpoint =
        r.results.iter().find(|a| a.kind == "ENDPOINT").expect("the POST /orders ENDPOINT");
    assert_eq!((route.name.as_str(), route.depth), ("POST /orders", 2));
    assert_eq!((endpoint.name.as_str(), endpoint.depth), ("POST /orders", 3));
    assert_eq!(row(&r, "checkout::placeOrder").depth, 4);
    assert_eq!(r.results.len(), 4, "{:?}", qnames(&r));
    assert!(r.results.iter().all(|a| a.seed == "app::save"), "one chain, rooted at save");
    assert!(!qnames(&r).contains(&"app::save") && !qnames(&r).contains(&"app::publish"));
    assert_score_sorted(&r);

    // Listed the other way round, publish's wave reaches the caller first.
    let swapped = blast_radius(&m, &["app::publish", "app::save"], &opts(Reach::Backward, 6));
    assert!(swapped.results.iter().all(|a| a.seed == "app::publish"));
    let rows = |r: &BlastRadius| {
        r.results.iter().map(|a| (a.qname.clone(), a.depth, a.score.to_bits())).collect::<Vec<_>>()
    };
    assert_eq!(rows(&swapped), rows(&r), "seed order moves attribution, never rows or scores");
}

#[test]
fn a_seed_reached_by_another_seed_is_a_linked_seed_not_a_lost_row() {
    let (_dir, m) = fixture();
    let r = blast_radius(&m, &["app::create_order", "app::save"], &opts(Reach::Forward, 4));
    assert_eq!(r.seeds[0].linked_seeds, ["app::save"], "create_order calls save");
    assert!(r.seeds[1].linked_seeds.is_empty(), "save does not call create_order");
    assert!(!qnames(&r).contains(&"app::save"), "a seed is never a row");
    let audit = row(&r, "app::audit");
    assert_eq!((audit.seed.as_str(), audit.depth), ("app::save", 1), "one hop from save, not two from create_order");
    let publish = row(&r, "app::publish");
    assert_eq!((publish.seed.as_str(), publish.depth), ("app::create_order", 1));
    assert_eq!(row(&r, "queue_producer:orders").seed, "app::create_order", "via publish");
    assert_score_sorted(&r);

    // Backward, the link runs the other way: save's caller is a seed.
    let back = blast_radius(&m, &["app::save", "app::create_order"], &opts(Reach::Backward, 4));
    assert_eq!(back.seeds[0].linked_seeds, ["app::create_order"]);
    assert!(back.seeds[1].linked_seeds.is_empty());
    // Both ways, each is the other's neighbour.
    let both = blast_radius(&m, &["app::save", "app::create_order"], &opts(Reach::Both, 4));
    assert_eq!(both.seeds[0].linked_seeds, ["app::create_order"]);
    assert_eq!(both.seeds[1].linked_seeds, ["app::save"]);
}

#[test]
fn an_unresolved_query_is_reported_and_the_rest_still_answer() {
    let (_dir, m) = fixture();
    let r = blast_radius(&m, &["app::save", "nope", "nope"], &BlastOptions::default());
    assert_eq!(r.unresolved, ["nope"], "each unresolved query once");
    assert!(!r.results.is_empty());
    assert!(r.absence.is_none(), "an answer with rows carries no absence");
    assert_eq!(r.seeds.len(), 1);

    // Two queries naming one node are one seed, the first query kept.
    let one = blast_radius(&m, &["app::save", "save"], &BlastOptions::default());
    assert_eq!(one.seeds.len(), 1);
    assert_eq!(one.seeds[0].query, "app::save");
}

#[test]
fn every_query_unresolved_is_an_unknown_symbol_absence() {
    let (_dir, m) = fixture();
    let r = blast_radius(&m, &["nope"], &BlastOptions::default());
    assert!(r.results.is_empty() && r.seeds.is_empty());
    assert_eq!(r.unresolved, ["nope"]);
    let a = r.absence.expect("an empty answer says why");
    assert_eq!((a.reason, a.query.as_str(), a.tier), ("unknown_symbol", "nope", "FACT"));

    // A near miss is suggested.
    let r = blast_radius(&m, &["create_ordr"], &BlastOptions::default());
    let a = r.absence.expect("absence");
    assert_eq!(a.reason, "unknown_symbol");
    assert!(a.suggestions.iter().any(|s| s == "app::create_order"), "{:?}", a.suggestions);

    // No query at all is an empty answer too, not a panic.
    let none = blast_radius(&m, &[], &BlastOptions::default());
    assert_eq!(none.absence.map(|a| a.reason), Some("no_match"));
}

#[test]
fn an_emptied_answer_says_what_emptied_it() {
    let (_dir, m) = fixture();
    // placeOrder has no caller: backward from it reaches nothing.
    let r = blast_radius(&m, &["checkout::placeOrder"], &opts(Reach::Backward, 4));
    let a = r.absence.expect("no carry edge reaches placeOrder");
    assert_eq!(a.reason, "no_edges");
    assert_eq!(a.mechanisms, ["CALLS"], "a FUNCTION seed depends on CALLS");
    assert!(a.note.contains("no carry edge reaches `checkout::placeOrder`"), "{}", a.note);

    // A MODULE seed is told structural edges do not carry.
    let module = blast_radius(&m, &["checkout"], &opts(Reach::Backward, 4));
    assert_eq!(module.seeds[0].kind, "MODULE");
    let a = module.absence.expect("a module has no carry edge in");
    assert_eq!(a.reason, "no_edges");
    assert!(a.note.contains("structural IMPORTS/CONTAINS/DEFINES are not carry edges"), "{}", a.note);

    // top_k 0 and live_only both empty a reached radius: no_match.
    let mut zero = BlastOptions::default();
    zero.top_k = Some(0);
    let r = blast_radius(&m, &["app::save"], &zero);
    assert_eq!(r.absence.map(|a| a.reason), Some("no_match"));
    let r = blast_radius(&m, &["checkout::placeOrder"], &opts(Reach::Forward, 1));
    assert!(
        !r.results.is_empty() && r.results.iter().all(|a| !a.live),
        "precondition: placeOrder reaches only dead rows in one hop"
    );
    let mut live = opts(Reach::Forward, 1);
    live.live_only = true;
    let a = blast_radius(&m, &["checkout::placeOrder"], &live).absence.expect("live_only dropped every row");
    assert_eq!(a.reason, "no_match");
    assert!(a.note.contains("live_only"), "{}", a.note);
}

#[test]
fn one_seed_is_the_answer_it_replaces() {
    let (_dir, m) = fixture();
    let r = blast_radius(&m, &["app::save"], &BlastOptions::default());
    let mut got = serde_json::to_value(&r.results).expect("rows serialise");
    for row in got.as_array_mut().expect("rows") {
        let seed = row.as_object_mut().expect("a row object").remove("seed");
        assert_eq!(seed, Some(serde_json::json!("app::save")), "one seed owns every row");
    }
    let before: serde_json::Value = serde_json::from_str(BEFORE_SINGLE_SEED).expect("before-file");
    assert_eq!(got, before, "rows, order and scores of the pre-LD.5 single-seed answer");
}
