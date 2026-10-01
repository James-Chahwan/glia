//! **overlay_loop** (CK.1): pyo3 surface for `glia_engine::overlay_loop`
//! (CE.3b-CE.3d), the overlay loop as three module functions, each a native
//! answer (LD.2). They run the steps of `glia overlay propose|try|accept`
//! (CE.3e) and return its `--json` objects as dicts, so the repo-graph MCP can
//! drive gaps -> overlay -> `overlay_delta` without the CLI.
//!
//! - `overlay_propose(repo_paths, categories=None, top_k=20, snippet_lines=3)`
//!   is the model step's work list, the engine's `Proposal`: `{rows, counts,
//!   snippets, ambiguous_root, guide}`, a row the `gaps` row (`id` first) plus
//!   `snippet` (`{file, start_line, lines}` or `None`). Writes nothing.
//! - `overlay_try(repo_paths, candidate, leave_one_out=True)` builds base,
//!   base + candidate and, per stanza, the candidate without it: the engine's
//!   `TryReport`, `{stanzas, base, with, delta, verdict, closed, builds}`.
//!   Writes only the primary repo's parse cache
//!   (`<repo>/.glia/graph/parse_cache.bin`) when it is one repo, as the CLI
//!   does, and under `GLIA_NO_PERSIST=1` too: the house convention for a
//!   cache-only build (engine `ParseCache::save`).
//! - `overlay_accept(repo_path, candidate=None, only=None, remove=None,
//!   dry_run=False)` is the only writer of `<repo>/.glia/overlay.toml`
//!   (atomically; not on a dry run or when nothing changed): the engine's
//!   `AcceptSummary`, `{added, removed, duplicates, file, dry_run, written,
//!   diff}`.
//!
//! No layout is persisted by any of them. `repo_paths`: the first is the
//! primary repo, whose `.glia/overlay.toml` the loop reads; the rest merge in
//! (the CLI's `--with`). `candidate` is the candidate's TOML TEXT, not a path:
//! the model step produces text (`pathlib.Path(f).read_text()` covers a
//! file). A refusal raises `ValueError` with the engine's message. The
//! markers (fired_on, the engine's) end `surface=py`, and the candidate
//! marker, printed here before a try or an accept as the CLI prints it with
//! the file path, says `file=-`:
//! `[overlay] propose repo=<primary> rows=<n> snippets=<s> ambiguous_root=<a> surface=py`,
//! `[overlay] candidate file=- stanzas=<n> (...) gap_links=<g> errors=<e>`,
//! `[overlay] try repo=<primary> stanzas=<n> builds=<b> verdicts keep=<k> review=<r> drop=<d> gaps <G0>→<G1> closed=<c> surface=py`,
//! `[overlay] accept repo=<repo> added=<a> (...) removed=<r> duplicates=<u> file=.glia/overlay.toml dry_run=<bool> surface=py`.
//!
//! Transport only: the builds, verdicts, validation and markers live in the
//! engine. The helpers the pyfunctions delegate to are pyo3-free, so
//! `cargo test -p glia-py` covers them (see the crate doc).

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use glia_engine::overlay_loop::{
    AcceptOptions, AcceptSummary, Proposal, ProposeOptions, TryOptions, TryReport, accept, propose,
    report_candidate, try_candidate,
};

use crate::convert::to_py;
use crate::registry::ModuleFns;

/// The `surface=` of every marker this module's calls print.
const SURFACE: &str = "py";

/// The work list behind [`overlay_propose`], minus pyo3. `Err` when there is
/// no path, a category is unknown, or the build fails.
fn propose_of(
    repo_paths: &[String],
    categories: Vec<String>,
    top_k: usize,
    snippet_lines: usize,
) -> Result<Proposal, String> {
    let mut opts = ProposeOptions::default();
    opts.categories = categories;
    opts.top_k = top_k;
    opts.snippet_lines = snippet_lines;
    opts.surface = SURFACE;
    propose(repo_paths, &opts)
}

/// The trial behind [`overlay_try`], minus pyo3: the `[overlay] candidate
/// file=-` marker, then the engine's try. `Err` when there is no path (before
/// any marker), the candidate is refused, it does not merge into the file, or
/// a build fails.
fn try_of(repo_paths: &[String], candidate: &str, leave_one_out: bool) -> Result<TryReport, String> {
    if repo_paths.is_empty() {
        return Err("overlay try: no repo paths".to_string());
    }
    report_candidate(None, candidate).map_err(|e| format!("overlay try: candidate refused: {e}"))?;
    let mut opts = TryOptions::default();
    opts.leave_one_out = leave_one_out;
    opts.surface = SURFACE;
    try_candidate(repo_paths, candidate, &opts)
}

/// The write behind [`overlay_accept`], minus pyo3: with a candidate, the
/// `[overlay] candidate file=-` marker first (a refused candidate writes
/// nothing, in the CLI's words), then the engine's accept, which refuses
/// `nothing to accept`, `only` without a candidate, an unknown stanza or gap
/// id, and a result the loader would not load as validated.
fn accept_of(
    repo_path: &str,
    candidate: Option<&str>,
    only: Vec<String>,
    remove: Vec<String>,
    dry_run: bool,
) -> Result<AcceptSummary, String> {
    if let Some(text) = candidate {
        report_candidate(None, text)
            .map_err(|e| format!("overlay accept: candidate refused, nothing written: {e}"))?;
    }
    let mut opts = AcceptOptions::default();
    opts.only = only;
    opts.remove = remove;
    opts.dry_run = dry_run;
    opts.surface = SURFACE;
    accept(repo_path, candidate, &opts)
}

/// **overlay_propose** (CK.1, CE.3d): the gaps an overlay stanza could
/// close, as one dict `{rows, counts, snippets, ambiguous_root, guide}` — the
/// model step's work list (`glia overlay propose --json`).
///
/// `rows`: per chosen category in report order, at most `top_k` each, each
/// the `gaps` row `{id, category, qname, kind, file, line, detail, suggest,
/// tier}` (+ `draft` on a `suspected_edge` row) plus `snippet`: `{file,
/// start_line, lines}`, the `snippet_lines` source lines either side of the
/// row's 1-based `line`, read from the one root that holds its file (`None`
/// when none or two do; `ambiguous_root` counts the latter). `id`
/// (`gap:<16 hex>`) is what a candidate's `# gap:` comment and
/// `overlay_accept(remove=...)` name. `counts`: per chosen category, the
/// total before `top_k`. `guide`: where docs/overlay.md documents each
/// `suggest` value.
///
/// `repo_paths`: the first is the primary repo, the rest merge in.
/// `categories`: the gaps categories to list (default every one an overlay
/// can repair: all but `wrapped_sink`); an unknown one raises `ValueError`,
/// as does an empty `repo_paths`. Builds the tree with the overlay as it is;
/// writes nothing. Prints `[overlay] propose repo=<primary> rows=<n>
/// snippets=<s> ambiguous_root=<a> surface=py`.
#[pyfunction]
#[pyo3(signature = (repo_paths, categories=None, top_k=20, snippet_lines=3))]
fn overlay_propose(
    py: Python<'_>,
    repo_paths: Vec<String>,
    categories: Option<Vec<String>>,
    top_k: usize,
    snippet_lines: usize,
) -> PyResult<Py<PyAny>> {
    let proposal = propose_of(
        &repo_paths,
        categories.unwrap_or_default(),
        top_k,
        snippet_lines,
    )
    .map_err(PyValueError::new_err)?;
    to_py(py, serde_json::to_string(&proposal))
}

/// **overlay_try** (CK.1, CE.3c): what would `.glia/overlay.toml` plus the
/// `candidate` stanzas change, and which stanza changed what, as one dict
/// `{stanzas, base, with, delta, verdict, closed, builds}` (`glia overlay
/// try --json`).
///
/// `candidate` is the candidate's TOML text (not a path): overlay sections
/// and entrypoints only, each stanza under `# gap: <id>` comments naming the
/// gaps it targets. The tree is built with the overlay as it is (`base`),
/// with every stanza merged in (`with`) and, with `leave_one_out`, once per
/// stanza without it. `base` / `with`: `{nodes_by_kind, edges_by_category,
/// gaps_by_category}`; `delta`: `{nodes, edges, gaps}`, with minus base,
/// zero entries left out; `verdict`: `keep` / `review` / `drop`; `closed`:
/// the linked gap ids in base and not in with; `builds`: the distinct
/// overlay texts built (at most N + 2). `stanzas` (empty without
/// `leave_one_out`): per stanza in candidate order `{stanza, gaps, closes,
/// marginal, verdict}`, `stanza` its handle (`wrapper#1`, `edge#2`,
/// `constants.GATEWAY`, `entrypoints#1`).
///
/// `repo_paths`: the first is the primary repo, whose overlay file the
/// candidate would join; the rest merge in. Raises `ValueError` for an empty
/// `repo_paths`, a refused candidate (the loader's reasons at their candidate
/// lines), one that does not merge into the file, or a failed build. Writes
/// only the primary repo's parse cache (`<repo>/.glia/graph/parse_cache.bin`,
/// one repo only, also under `GLIA_NO_PERSIST=1`); never the overlay file or
/// a layout. Prints `[overlay] candidate file=- ...` then `[overlay] try
/// repo=<primary> ... surface=py`.
#[pyfunction]
#[pyo3(signature = (repo_paths, candidate, leave_one_out=true))]
fn overlay_try(
    py: Python<'_>,
    repo_paths: Vec<String>,
    candidate: &str,
    leave_one_out: bool,
) -> PyResult<Py<PyAny>> {
    let report = try_of(&repo_paths, candidate, leave_one_out).map_err(PyValueError::new_err)?;
    to_py(py, serde_json::to_string(&report))
}

/// **overlay_accept** (CK.1, CE.3d): write into `<repo_path>/.glia/overlay.toml`
/// — the only writer of that file — and return one dict `{added, removed,
/// duplicates, file, dry_run, written, diff}` (`glia overlay accept --json`).
///
/// `candidate` (TOML text, not a path): its stanzas named by `only` (their
/// handles: `wrapper#1`, `constants.GATEWAY`; default every stanza) are
/// merged in, keeping the file's comments and layout; a constant or pattern
/// the file already holds counts in `duplicates`. `remove`: gap ids of
/// `orphaned_rule` / `redundant_rule` rows (from `overlay_propose` or `gaps`)
/// whose stanzas are deleted by identity, never by line. The result must
/// load as validated, and is written atomically unless `dry_run` or
/// unchanged (`written`). `added`: stanzas written per section; `removed`:
/// entries deleted; `diff`: the unified diff before -> after.
///
/// Raises `ValueError` (nothing written) with the engine's message for:
/// neither a candidate nor `remove` (`nothing to accept`), `only` without a
/// candidate, a refused candidate (`candidate refused, nothing written`), an
/// unknown stanza handle or gap id, a result the loader would drop, a
/// constant already pinned to another value, or a failed read, build or
/// write. Prints `[overlay] candidate file=- ...` (with a candidate) then
/// `[overlay] accept repo=<repo> ... dry_run=<bool> surface=py`.
#[pyfunction]
#[pyo3(signature = (repo_path, candidate=None, only=None, remove=None, dry_run=false))]
fn overlay_accept(
    py: Python<'_>,
    repo_path: &str,
    candidate: Option<&str>,
    only: Option<Vec<String>>,
    remove: Option<Vec<String>>,
    dry_run: bool,
) -> PyResult<Py<PyAny>> {
    let summary = accept_of(
        repo_path,
        candidate,
        only.unwrap_or_default(),
        remove.unwrap_or_default(),
        dry_run,
    )
    .map_err(PyValueError::new_err)?;
    to_py(py, serde_json::to_string(&summary))
}

fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(overlay_propose, m)?)?;
    m.add_function(wrap_pyfunction!(overlay_try, m)?)?;
    m.add_function(wrap_pyfunction!(overlay_accept, m)?)?;
    Ok(())
}

inventory::submit! { ModuleFns { name: "overlay_loop", add: register } }

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use glia_engine::overlay_loop::{DEFAULT_SNIPPET_LINES, DEFAULT_TOP_K};

    use super::*;

    const CHAT_REPO_GO: &str = include_str!(
        "../../bench/substrate-gap/fixtures/go-overlay-data-wrapper/chat_preview_repository.go"
    );
    const COLLECTION_GO: &str =
        include_str!("../../bench/substrate-gap/fixtures/go-overlay-data-wrapper/collection.go");
    const FIXTURE_OVERLAY: &str = include_str!(
        "../../bench/substrate-gap/fixtures/go-overlay-data-wrapper/.glia/overlay.toml"
    );
    /// An `[[edge]]` between two qnames that do not exist: binds nothing.
    const USELESS_EDGE: &str =
        "[[edge]]\nfrom = \"nowhere::Caller\"\nto = \"nowhere::Callee\"\ncategory = \"CALLS\"\n";

    /// A scratch dir under the system temp dir, removed on drop (`py` has no
    /// `tempfile` dev-dependency).
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The fixture's two Go files under `<scratch>/repo`, `NewCollection`
    /// taking a computed name (`strings.ToLower(name)`, as
    /// cli/tests/overlay_loop_cli.rs lays it out: with the literal the CA.4
    /// wrapper inference already mints the entity, and the wrapper stanza
    /// would add nothing); no `.glia/overlay.toml`.
    fn go_repo(tag: &str) -> (Scratch, PathBuf) {
        let root = std::env::temp_dir().join(format!("glia-ck1-py-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let scratch = Scratch(root.clone());
        let collection = COLLECTION_GO
            .replace(
                "import (\n\t\"go.mongodb.org/mongo-driver/mongo\"\n)",
                "import (\n\t\"strings\"\n\n\t\"go.mongodb.org/mongo-driver/mongo\"\n)",
            )
            .replace(".Collection(name)", ".Collection(strings.ToLower(name))");
        assert!(
            collection.contains("\"strings\"") && collection.contains("strings.ToLower(name)"),
            "the fixture's NewCollection moved"
        );
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        std::fs::write(repo.join("collection.go"), collection).expect("write");
        std::fs::write(repo.join("chat_preview_repository.go"), CHAT_REPO_GO).expect("write");
        (scratch, repo)
    }

    /// The fixture overlay's `[[wrapper]]` table (`wrapper#1`), then the
    /// useless edge (`edge#1`).
    fn cand() -> String {
        let at = FIXTURE_OVERLAY
            .find("[[wrapper]]")
            .expect("the fixture overlay has a [[wrapper]]");
        format!("{}\n{USELESS_EDGE}", &FIXTURE_OVERLAY[at..])
    }

    /// [`cand`] with the wrapper's `flavor` misspelt: refused by the loader.
    fn bad() -> String {
        let bad = cand().replace("flavor = \"nosql\"", "flavour = \"nosql\"");
        assert!(bad.contains("flavour"), "the fixture wrapper's flavor moved");
        bad
    }

    /// Each `needle` occurs in `json`, in order: the struct's field order.
    fn assert_in_order(json: &str, needles: &[&str]) {
        let mut at = 0;
        for k in needles {
            let found = json[at..].find(k).map(|i| at + i);
            assert!(found.is_some(), "{k} after byte {at}: {json}");
            at = found.unwrap_or(at) + k.len();
        }
    }

    /// `serde_json` text and its parsed value (`py` has no `serde`
    /// dependency to name the `Serialize` bound with).
    macro_rules! to_json {
        ($v:expr) => {{
            let text = serde_json::to_string($v).expect("json");
            let value: serde_json::Value = serde_json::from_str(&text).expect("json");
            (text, value)
        }};
    }

    /// The signature literals are the engine's defaults.
    #[test]
    fn defaults_match_the_engine() {
        assert_eq!(DEFAULT_TOP_K, 20, "update `top_k=20` in overlay_propose's signature");
        assert_eq!(
            DEFAULT_SNIPPET_LINES, 3,
            "update `snippet_lines=3` in overlay_propose's signature"
        );
        assert!(
            TryOptions::default().leave_one_out,
            "update `leave_one_out=true` in overlay_try's signature"
        );
        assert!(
            !AcceptOptions::default().dry_run,
            "update `dry_run=false` in overlay_accept's signature"
        );
    }

    /// CK.1: the three helpers are transport — pin the wiring over the
    /// round trip cli/tests/overlay_loop_cli.rs drives through the binary:
    /// propose (three dead symbols), try (the wrapper keeps, the edge drops),
    /// accept (dry run, then both stanzas), the edge reported as an orphaned
    /// rule and removed by its gap id, and each refusal.
    #[test]
    fn loop_helpers_return_the_documented_objects() {
        let (_scratch, repo_path) = go_repo("loop");
        let repo = repo_path.to_str().expect("utf-8 temp path").to_string();
        let paths = [repo.clone()];
        let overlay: PathBuf = Path::new(&repo).join(".glia/overlay.toml");
        let (cand, bad) = (cand(), bad());

        // propose
        let p = propose_of(&paths, Vec::new(), 20, 3).expect("propose");
        let (text, v) = to_json!(&p);
        assert!(text.starts_with("{\"rows\":[{\"id\":\"gap:"), "{text}");
        assert_in_order(
            &text,
            &[
                "{\"rows\":",
                ",\"counts\":",
                ",\"snippets\":",
                ",\"ambiguous_root\":",
                ",\"guide\":",
            ],
        );
        let rows: Vec<(&str, &str)> = v["rows"]
            .as_array()
            .expect("rows")
            .iter()
            .map(|r| {
                (
                    r["category"].as_str().unwrap_or(""),
                    r["qname"].as_str().unwrap_or(""),
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                (
                    "dead_symbol",
                    "chat_preview_repository::NewChatPreviewRepository"
                ),
                ("dead_symbol", "collection::NewCollection"),
                ("dead_symbol", "collection::NewNamedCollection"),
            ],
            "{text}"
        );
        assert_eq!(v["snippets"], 3, "{text}");
        let unknown = propose_of(&paths, vec!["nope".to_string()], 20, 3);
        assert!(
            unknown
                .as_ref()
                .is_err_and(|e| e.contains("unknown gaps category")),
            "{unknown:?}"
        );

        // try
        let t = try_of(&paths, &cand, true).expect("try");
        let verdicts: Vec<(&str, &str)> = t
            .stanzas
            .iter()
            .map(|s| (s.stanza.as_str(), s.verdict))
            .collect();
        assert_eq!(verdicts, [("wrapper#1", "keep"), ("edge#1", "drop")], "{t:?}");
        assert_eq!(t.builds, 4, "{t:?}");
        assert_eq!(t.verdict, "keep", "{t:?}");
        assert!(t.closed.is_empty(), "{t:?}");
        let two = try_of(&paths, &cand, false).expect("try, no leave-one-out");
        assert_eq!(two.builds, 2, "{two:?}");
        assert!(two.stanzas.is_empty(), "{two:?}");
        let refused = try_of(&paths, &bad, true);
        assert!(
            refused
                .as_ref()
                .is_err_and(|e| e.contains("unknown field `flavour`")),
            "{refused:?}"
        );
        let none = try_of(&[], &cand, true);
        assert!(
            none.as_ref().is_err_and(|e| e.contains("no repo paths")),
            "{none:?}"
        );
        assert!(!overlay.exists(), "a try never writes the overlay file");

        // accept: a dry run, then both stanzas
        let dry = accept_of(
            &repo,
            Some(&cand),
            vec!["wrapper#1".to_string()],
            Vec::new(),
            true,
        )
        .expect("dry run");
        assert!(dry.dry_run && !dry.written, "{dry:?}");
        assert!(dry.diff.contains("+[[wrapper]]"), "{dry:?}");
        assert!(!overlay.exists(), "a dry run writes nothing");
        let a = accept_of(&repo, Some(&cand), Vec::new(), Vec::new(), false).expect("accept");
        let (text, v) = to_json!(&a);
        assert_eq!(v["added"], serde_json::json!({"edge": 1, "wrapper": 1}), "{text}");
        assert!(a.written, "{text}");
        assert_in_order(
            &text,
            &[
                "{\"added\":",
                ",\"removed\":",
                ",\"duplicates\":",
                ",\"file\":",
                ",\"dry_run\":",
                ",\"written\":",
                ",\"diff\":",
            ],
        );

        // the useless edge is an orphaned rule: remove it by its gap id
        let o = propose_of(&paths, vec!["orphaned_rule".to_string()], 20, 3).expect("propose");
        assert_eq!(o.rows.len(), 1, "{o:?}");
        let (_, row) = to_json!(&o.rows[0]);
        assert_eq!(row["qname"], "nowhere::Caller", "{row}");
        assert_eq!(row["suggest"], "remove", "{row}");
        assert_eq!(row["file"], ".glia/overlay.toml", "{row}");
        let id = row["id"].as_str().expect("id").to_string();
        let r = accept_of(&repo, None, Vec::new(), vec![id], false).expect("remove");
        assert_eq!(r.removed, 1, "{r:?}");
        assert!(r.added.is_empty() && r.written, "{r:?}");
        let kept = std::fs::read_to_string(&overlay).expect("overlay file");
        assert!(kept.starts_with("version = 1\n"), "{kept}");
        assert!(
            kept.contains("[[wrapper]]\ncall = \"NewCollection\""),
            "{kept}"
        );
        assert!(!kept.contains("[[edge]]"), "{kept}");

        // refusals write nothing
        let nothing = accept_of(&repo, None, Vec::new(), Vec::new(), false);
        assert!(
            nothing
                .as_ref()
                .is_err_and(|e| e.contains("nothing to accept")),
            "{nothing:?}"
        );
        let before = std::fs::read(&overlay).expect("overlay file");
        let refused = accept_of(&repo, Some(&bad), Vec::new(), Vec::new(), false);
        assert!(
            refused
                .as_ref()
                .is_err_and(|e| e.contains("nothing written")),
            "{refused:?}"
        );
        assert_eq!(
            std::fs::read(&overlay).expect("overlay file"),
            before,
            "a refused accept leaves the file byte-identical"
        );
    }
}
