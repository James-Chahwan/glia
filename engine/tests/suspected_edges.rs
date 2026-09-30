//! CD.3b — the `suspected_edge` gaps category: an orphan the graph already
//! knows how to pair (a learned (kind, category, kind) triple) is matched to
//! the targets whose channel tokens it shares, and each surviving pair is a
//! HEURISTIC row with a paste-ready `[[edge]]` stanza in its `draft`.
//!
//! The fixture is one root holding an Angular client (`web/`) and an Express
//! API (`api/`). `list()` and `count()` pair through the HTTP resolver, so
//! (ENDPOINT, HTTP_CALLS, ROUTE) is seen twice; `markAllRead()` posts to
//! `${environment.apiUrl}/notifications/read-all` through a class-field base,
//! which the resolver cannot pair: the orphan `endpoint:POST:${…}/read-all @web`.
//! The read-all routes sit behind a `/:tenant` path parameter: a parameter is
//! never a mount, so the resolver's mount-segment fold (CB.23), which pairs
//! `/{}/read-all` with the one route ending `/<literal mounts>/read-all`,
//! leaves the orphan to this heuristic.

use std::path::Path;

use glia_code_domain::glia_config::parse_str;
use glia_code_domain::snapshots::{HistoryCommit, HistoryFile, HistoryMeta, write_history};
use glia_engine::gaps::{
    GapRow, GapsOptions, GapsReport, HEURISTIC, SUSPECTED_EDGE, UNPAIRED_ENDPOINT, UNPAIRED_ROUTE,
    gaps_report, overlay_delta,
};
use glia_engine::{GenerateResult, generate_one};

const SERVICE_TS: &str = "import { Injectable } from '@angular/core';
import { HttpClient } from '@angular/common/http';
import { environment } from '../environments/environment';

@Injectable({ providedIn: 'root' })
export class NotificationsApi {
  private readonly base = `${environment.apiUrl}/notifications`;
  constructor(private http: HttpClient) {}

  markAllRead() {
    return this.http.post(`${this.base}/read-all`, {});
  }

  list() {
    return this.http.get('/notifications');
  }

  count() {
    return this.http.get('/notifications/count');
  }
}
";

/// [`SERVICE_TS`] without `list()` and `count()`: no HTTP pair, so no
/// learned triple.
const SERVICE_UNPAIRED_TS: &str = "import { Injectable } from '@angular/core';
import { HttpClient } from '@angular/common/http';
import { environment } from '../environments/environment';

@Injectable({ providedIn: 'root' })
export class NotificationsApi {
  private readonly base = `${environment.apiUrl}/notifications`;
  constructor(private http: HttpClient) {}

  markAllRead() {
    return this.http.post(`${this.base}/read-all`, {});
  }
}
";

const ENVIRONMENT_TS: &str =
    "export const environment = { production: false, apiUrl: 'http://localhost:8080/api' };\n";
const WEB_PACKAGE: &str = "{\"name\":\"web\",\"dependencies\":{\"@angular/core\":\"17.0.0\",\"@angular/common\":\"17.0.0\"}}\n";
const ROUTES_TS: &str = "import express from 'express';
const router = express.Router();

function markAll(req, res) { res.json({}); }
function getOne(req, res) { res.json({}); }
function listAll(req, res) { res.json([]); }
function countAll(req, res) { res.json(0); }

router.post('/:tenant/notifications/read-all', markAll);
router.get('/:tenant/notifications/read-all', getOne);
router.get('/notifications', listAll);
router.get('/notifications/count', countAll);

export default router;
";
const API_PACKAGE: &str = "{\"name\":\"api\",\"dependencies\":{\"express\":\"4.18.0\"}}\n";

const ORPHAN: &str = "endpoint:POST:${…}/read-all @web";
const TARGET: &str = "POST /:tenant/notifications/read-all @api";
const WRONG_METHOD: &str = "GET /:tenant/notifications/read-all @api";

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, body).expect("write");
}

/// The fixture tree, with `service` as the Angular service.
fn fixture(service: &str) -> tempfile::TempDir {
    let d = tempfile::tempdir().expect("tempdir");
    let root = d.path();
    write(root, "web/src/notifications.service.ts", service);
    write(root, "web/environments/environment.ts", ENVIRONMENT_TS);
    write(root, "web/package.json", WEB_PACKAGE);
    write(root, "api/src/routes.ts", ROUTES_TS);
    write(root, "api/package.json", API_PACKAGE);
    d
}

fn build(root: &Path) -> GenerateResult {
    generate_one(&root.to_string_lossy()).expect("build")
}

fn report(r: &GenerateResult) -> GapsReport {
    let roots: Vec<(u64, std::path::PathBuf)> = r
        .repo_roots
        .iter()
        .map(|(id, p)| (*id, std::path::PathBuf::from(p)))
        .collect();
    gaps_report(&r.merged, &roots, &GapsOptions::default()).expect("known categories")
}

fn rows<'a>(rep: &'a GapsReport, category: &str) -> Vec<&'a GapRow> {
    rep.rows.iter().filter(|r| r.category == category).collect()
}

/// The shape this packet exists for is still minted at HEAD: the class-field
/// `${…}` base leaves one unpaired endpoint, and both read-all routes unpaired.
fn assert_the_orphan_is_there(rep: &GapsReport) {
    let unpaired: Vec<&str> = rows(rep, UNPAIRED_ENDPOINT)
        .iter()
        .map(|r| r.qname.as_str())
        .collect();
    assert_eq!(
        unpaired,
        [ORPHAN],
        "the fixture no longer mints the orphan: {rep:#?}"
    );
    let routes: Vec<&str> = rows(rep, UNPAIRED_ROUTE)
        .iter()
        .map(|r| r.qname.as_str())
        .collect();
    assert_eq!(routes, [TARGET, WRONG_METHOD], "{rep:#?}");
}

/// (1) Exactly one suspected row: the orphan, located, pointing at the POST
/// route; HEURISTIC, suggest `edge`; its draft is a `# gap: <id>` comment and
/// one binding `[[edge]]` stanza; its id survives a rebuild.
#[test]
fn proposes_the_read_all_pair() {
    let d = fixture(SERVICE_TS);
    let rep = report(&build(d.path()));
    let again = report(&build(d.path()));
    assert_the_orphan_is_there(&rep);

    let suspected = rows(&rep, SUSPECTED_EDGE);
    assert_eq!(suspected.len(), 1, "{rep:#?}");
    assert_eq!(rep.count(SUSPECTED_EDGE), 1);
    let s = suspected[0];
    assert_eq!(s.qname, ORPHAN);
    assert_eq!(s.kind, "ENDPOINT");
    assert_eq!(
        (s.file.as_deref(), s.line),
        (Some("web/src/notifications.service.ts"), Some(11)),
        "the post call, 1-based"
    );
    assert_eq!((s.tier, s.suggest), (HEURISTIC, "edge"));
    assert_eq!(
        s.detail,
        format!(
            "HTTP_CALLS -> `{TARGET}` (api/src/routes.ts:9) score=0.70 channel=1.00 aa=0 ra=0 \
             cochange=no target_unpaired=yes triple=ENDPOINT-HTTP_CALLS->ROUTE seen 2x"
        )
    );
    assert!(s.id.starts_with("gap:") && s.id.len() == 20, "{}", s.id);

    let draft = s.draft.as_deref().expect("a suspected row carries a draft");
    assert_eq!(
        draft.lines().next(),
        Some(format!("# gap: {}", s.id).as_str()),
        "{draft}"
    );
    assert_eq!(
        draft,
        format!(
            "# gap: {}\n[[edge]]\nfrom = \"{ORPHAN}\"\nto = \"{TARGET}\"\n\
             category = \"HTTP_CALLS\"\nnote = \"glia suspected_edge score=0.70\"\n",
            s.id
        )
    );
    let cfg = parse_str(&format!("version = 1\n\n{draft}"));
    assert!(cfg.errors.is_empty(), "{:?}", cfg.errors);
    assert_eq!(cfg.config.edge.len(), 1);
    let e = cfg.config.edge[0].get_ref();
    assert_eq!(
        (e.from.as_str(), e.to.as_str(), e.category.as_str()),
        (ORPHAN, TARGET, "HTTP_CALLS")
    );

    // Every other row keeps no draft.
    assert!(
        rep.rows
            .iter()
            .filter(|r| r.category != SUSPECTED_EDGE)
            .all(|r| r.draft.is_none()),
        "{rep:#?}"
    );
    let ids = |rep: &GapsReport| -> Vec<String> {
        rows(rep, SUSPECTED_EDGE)
            .iter()
            .map(|r| r.id.clone())
            .collect()
    };
    assert_eq!(ids(&rep), ids(&again), "the id is stable across builds");
}

/// (2) `GET /:tenant/notifications/read-all` shares every token with the orphan but
/// not its method: never proposed.
#[test]
fn method_mismatch_is_not_proposed() {
    let d = fixture(SERVICE_TS);
    let rep = report(&build(d.path()));
    assert_the_orphan_is_there(&rep);
    assert!(
        rows(&rep, SUSPECTED_EDGE)
            .iter()
            .all(|r| !r.detail.contains(WRONG_METHOD)
                && !r
                    .draft
                    .as_deref()
                    .unwrap_or_default()
                    .contains(WRONG_METHOD)),
        "{rep:#?}"
    );
}

/// (3) The draft pasted into `.glia/overlay.toml` binds: the orphan gains its
/// HTTP_CALLS edge, the suspected row is gone, and `overlay_delta` counts one
/// orphan fewer.
#[test]
fn stanza_closes_the_loop() {
    let d = fixture(SERVICE_TS);
    let before = report(&build(d.path()));
    let draft = rows(&before, SUSPECTED_EDGE)[0]
        .draft
        .clone()
        .expect("draft");
    write(
        d.path(),
        ".glia/overlay.toml",
        &format!("version = 1\n\n{draft}"),
    );

    let r = build(d.path());
    let after = report(&r);
    let paired = r.merged.all_edges().any(|e| {
        e.category == glia_code_domain::edge_category::HTTP_CALLS
            && r.merged.graphs.iter().any(|g| {
                g.nav.qname_by_id.get(&e.from).map(String::as_str) == Some(ORPHAN)
                    && r.merged
                        .graphs
                        .iter()
                        .any(|h| h.nav.qname_by_id.get(&e.to).map(String::as_str) == Some(TARGET))
            })
    });
    assert!(paired, "the stanza bound both qnames");
    assert_eq!(
        (
            before.count(UNPAIRED_ENDPOINT),
            after.count(UNPAIRED_ENDPOINT)
        ),
        (1, 0),
        "{after:#?}"
    );
    assert_eq!(after.count(SUSPECTED_EDGE), 0, "{after:#?}");

    let delta = overlay_delta(&[d.path().to_string_lossy().into_owned()], false).expect("delta");
    assert_eq!(delta.orphans_with + 1, delta.orphans_without, "{delta:?}");
    assert_eq!(
        (
            delta.without.gaps_by_category.get(SUSPECTED_EDGE),
            delta.with.gaps_by_category.get(SUSPECTED_EDGE)
        ),
        (Some(&1), Some(&0)),
        "{delta:?}"
    );
}

/// (4) With `list()` and `count()` gone nothing pairs, so no triple is
/// learned and nothing is proposed, though the orphan and its route remain.
#[test]
fn no_support_no_rows() {
    let d = fixture(SERVICE_UNPAIRED_TS);
    let rep = report(&build(d.path()));
    assert_eq!(rep.count(UNPAIRED_ENDPOINT), 1, "{rep:#?}");
    assert_eq!(rep.count(SUSPECTED_EDGE), 0, "{rep:#?}");
    assert!(rows(&rep, SUSPECTED_EDGE).is_empty());
    assert!(rep.counts.contains_key(SUSPECTED_EDGE), "computed, zero");
}

/// (5) Two reports of one tree serialise identically, drafts included.
#[test]
fn deterministic() {
    let d = fixture(SERVICE_TS);
    let a = serde_json::to_string(&report(&build(d.path()))).expect("json");
    let b = serde_json::to_string(&report(&build(d.path()))).expect("json");
    assert_eq!(a, b);
    assert!(a.contains("\"draft\":\"# gap: gap:"), "{a}");
    assert_eq!(
        a.matches("\"draft\"").count(),
        1,
        "only the suspected row: {a}"
    );
}

/// (6) Git history co-changing the client service and the route file boosts
/// the pair by the co-change weight (150 per mille): 0.70 -> 0.85.
#[test]
fn cochange_boosts_the_score() {
    let d = fixture(SERVICE_TS);
    let files = ["web/src/notifications.service.ts", "api/src/routes.ts"];
    let commits: Vec<HistoryCommit> = (1..=3i64)
        .rev()
        .map(|i| HistoryCommit {
            c: format!("{i:02}{}", "0".repeat(38)),
            t: 1_767_225_600 + i * 86_400,
            files: files
                .iter()
                .map(|p| HistoryFile {
                    p: (*p).to_string(),
                    a: Some(1),
                    d: Some(0),
                    from: None,
                })
                .collect(),
        })
        .collect();
    write_history(
        d.path(),
        HistoryMeta::new(commits[0].c.clone(), 2000, None, String::new()),
        &commits,
        &[],
    )
    .expect("write snapshot");
    let r = build(d.path());
    let cochanges = r
        .merged
        .all_edges()
        .filter(|e| e.category == glia_code_domain::edge_category::CO_CHANGES)
        .count();
    assert!(cochanges > 0, "the snapshot minted CO_CHANGES");
    let rep = report(&r);
    let s = rows(&rep, SUSPECTED_EDGE);
    assert_eq!(s.len(), 1, "{rep:#?}");
    assert!(
        s[0].detail.contains(" score=0.85 ") && s[0].detail.contains(" cochange=yes "),
        "{}",
        s[0].detail
    );
    assert!(
        s[0].draft
            .as_deref()
            .is_some_and(|d| d.contains("note = \"glia suspected_edge score=0.85\"")),
        "{:?}",
        s[0].draft
    );
}
