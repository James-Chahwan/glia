//! **embed** (FB.5c): `glia_py.embed`, the pyo3 surface of the `glia embed`
//! snapshot step. It brings one repo's `<repo>/.glia/vectors.jsonl` up to
//! date with the graph (`glia_engine::semantic::embed_repo`, FB.5a); no build
//! reads what it writes, and `generate()` never embeds.
//!
//! The model: `test-hash` (glia-embed's deterministic test model) always;
//! `bge-small` only in a wheel built with this crate's `embed` feature
//! (`glia-embed/fastembed`, which links the ONNX Runtime), which
//! `py/pyproject.toml` turns on for the published wheels. A wheel without it
//! refuses `bge-small` with glia-embed's `named()` message. One bge-small
//! session is held per process (`bge_small`): every
//! `FastEmbedder::new()` is a fresh ORT session and model load.
//!
//! Transport only: the run, the sidecar format and the per-run `[embed]
//! repo=..` line are the engine's. This surface adds one `[embed] repo=<label>
//! model=<m> recipe=<r> ... surface=py` line per call. The helper the
//! pyfunction delegates to is pyo3-free, so `cargo test -p glia-py` covers it.

use std::path::{Path, PathBuf};

use glia_core::RepoId;
use glia_embed::Embedder;
use glia_engine::persist::{LoadOutcome, default_layout_dir, load_or_rebuild};
use glia_engine::semantic::{
    DEFAULT_RECIPE, EmbedOptions, EmbedRecipe, EmbedReport, RECIPES, embed_repo,
};
use glia_engine::{GenerateResult, generate_one};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use crate::convert::to_py;
use crate::registry::ModuleFns;

/// The recipe a `recipe=` string names; None is the engine's default.
fn recipe_of(recipe: Option<&str>) -> Result<EmbedRecipe, String> {
    match recipe {
        None => Ok(DEFAULT_RECIPE),
        Some(s) => EmbedRecipe::parse(s).ok_or_else(|| {
            let known: Vec<&str> = RECIPES.iter().map(|r| r.as_str()).collect();
            format!(
                "unknown embed recipe `{s}`: expected one of {}",
                known.join(", ")
            )
        }),
    }
}

/// The graph of the repo at `root`. A write run takes the repo's layout
/// through `load_or_rebuild` (rebuilt and written back when stale, unless
/// `GLIA_NO_PERSIST=1`); a `check` run writes nothing, so it reads the layout
/// only when it is fresh and otherwise builds in memory (`generate_one`).
fn graph_of(root: &Path, check: bool) -> Result<GenerateResult, String> {
    let dir = default_layout_dir(root);
    if check {
        return match load_or_rebuild(&dir, Some(root), false) {
            Ok((r, LoadOutcome::Fresh)) => Ok(r),
            _ => generate_one(&root.to_string_lossy()),
        };
    }
    load_or_rebuild(&dir, Some(root), true)
        .map(|(r, _)| r)
        .map_err(|e| e.to_string())
}

/// The repo of `built` whose root is `root`: the only one, or the one whose
/// recorded root canonicalises to `root`'s.
fn repo_of(built: &GenerateResult, root: &Path) -> Result<RepoId, String> {
    if built.repo_roots.len() == 1
        && let Some(id) = built.repo_roots.keys().next()
    {
        return Ok(RepoId(*id));
    }
    let want = root.canonicalize().map_err(|e| e.to_string())?;
    built
        .repo_roots
        .iter()
        .find(|(_, r)| {
            PathBuf::from(r)
                .canonicalize()
                .is_ok_and(|have| have == want)
        })
        .map(|(id, _)| RepoId(*id))
        .ok_or_else(|| format!("the graph of {} holds no repo rooted there", root.display()))
}

/// The process's one bge-small session, loaded on first use.
#[cfg(feature = "embed")]
fn bge_small() -> Result<&'static glia_embed::FastEmbedder, String> {
    use std::sync::{Mutex, OnceLock};
    static MODEL: OnceLock<glia_embed::FastEmbedder> = OnceLock::new();
    static LOADING: Mutex<()> = Mutex::new(());
    if let Some(m) = MODEL.get() {
        return Ok(m);
    }
    let _guard = LOADING.lock().map_err(|e| e.to_string())?;
    if let Some(m) = MODEL.get() {
        return Ok(m);
    }
    let loaded = glia_embed::FastEmbedder::new()?;
    Ok(MODEL.get_or_init(|| loaded))
}

/// Run `f` with the embedder `model` names, after checking (bge-small) that
/// its model cache is not inside `root`.
fn with_embedder<T>(
    model: &str,
    root: &Path,
    f: impl FnOnce(&dyn Embedder) -> Result<T, String>,
) -> Result<T, String> {
    #[cfg(feature = "embed")]
    if model == "bge-small" {
        let e = bge_small()?;
        e.refuse_repo_dir(root)?;
        return f(e);
    }
    let _ = root;
    let e = glia_embed::named(model)?;
    f(e.as_ref())
}

/// The run behind [`embed_py`], minus pyo3: check the model name and the
/// recipe, take the graph ([`graph_of`]), embed, print the surface line.
/// `Err` is plain text: an unknown model or recipe (before any build), a
/// model this build lacks, a path that is not a directory, a build or layout
/// error, or the run's own (a sidecar of another model without `replace`).
fn embed_report(
    repo_path: &str,
    model: &str,
    recipe: Option<&str>,
    replace: bool,
    limit: Option<usize>,
    check: bool,
) -> Result<EmbedReport, String> {
    if !glia_embed::MODEL_NAMES.contains(&model) {
        // named() refuses it, with the one message listing the known names.
        return Err(match glia_embed::named(model) {
            Err(e) => e,
            Ok(_) => format!("unknown embedding model `{model}`"),
        });
    }
    let recipe = recipe_of(recipe)?;
    let root = Path::new(repo_path);
    if !root.is_dir() {
        return Err(format!("{repo_path} is not a directory"));
    }
    let built = graph_of(root, check)?;
    let repo = repo_of(&built, root)?;
    let opts = EmbedOptions::default()
        .with_recipe(recipe)
        .with_replace(replace)
        .with_limit(limit)
        .with_check_only(check);
    let report = with_embedder(model, root, |e| {
        embed_repo(root, &built.merged, repo, e, &opts)
    })?;
    eprintln!(
        "[embed] repo={} model={} recipe={} embedded={} reused={} cached={} deferred={} \
         rows={} check={} ms={} surface=py",
        report.repo,
        report.model,
        report.recipe,
        report.embedded,
        report.reused,
        report.cached,
        report.deferred,
        report.rows_written,
        check,
        report.ms
    );
    Ok(report)
}

/// **embed** (FB.5c): embed the repo at `repo_path` into
/// `<repo>/.glia/vectors.jsonl`, the sidecar the semantic tiers read. Only
/// nodes without a fresh vector are embedded: a second call on an unchanged
/// repo embeds nothing. The repo's layout (`.glia/graph/`) is loaded, or
/// rebuilt and written back when stale (`GLIA_NO_PERSIST=1` keeps it in
/// memory). A snapshot step: `generate()` never calls it.
///
/// `model`: `"bge-small"` (fastembed, downloaded once into
/// `FASTEMBED_CACHE_DIR`, else `$XDG_CACHE_HOME/glia/fastembed`, else
/// `~/.cache/glia/fastembed`; never inside the repo) or `"test-hash"` (the
/// deterministic test model). `recipe`: the node text embedded, `"t1"` or
/// `"t2"`; None is the engine's default. `replace`: rewrite a sidecar holding
/// another model's rows. `limit`: embed at most this many nodes this call.
/// `check`: count the work and write nothing at all (the model is never
/// called; a stale layout is built in memory, not written).
///
/// Returns `{repo, model, recipe, dims, nodes, cached, reused, embedded,
/// ambiguous, deferred, stale_dropped, foreign_kept, rows_written, ms}`.
/// Raises `ValueError` for an unknown model or recipe, a model this wheel
/// was built without, a path that is not a directory, a build error, or a
/// sidecar of another model without `replace`. The GIL is released while it
/// runs. Prints `[embed] repo=<label> model=<m> ... surface=py` on stderr.
#[pyfunction]
#[pyo3(
    name = "embed",
    signature = (repo_path, model="bge-small", recipe=None, replace=false, limit=None, check=false)
)]
fn embed_py(
    py: Python<'_>,
    repo_path: &str,
    model: &str,
    recipe: Option<String>,
    replace: bool,
    limit: Option<usize>,
    check: bool,
) -> PyResult<Py<PyAny>> {
    let report = py
        .detach(|| embed_report(repo_path, model, recipe.as_deref(), replace, limit, check))
        .map_err(PyValueError::new_err)?;
    to_py(py, serde_json::to_string(&report))
}

fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(embed_py, m)?)?;
    Ok(())
}

inventory::submit! { ModuleFns { name: "embed", add: register } }

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use glia_engine::semantic::VECTORS_FILE;

    use super::*;

    /// A scratch repo under the system temp dir, removed on drop (`py` has no
    /// `tempfile` dev-dependency).
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn repo(name: &str) -> (Scratch, String) {
        let root = std::env::temp_dir().join(format!("glia-fb5c-py-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let top = root.join("repo");
        std::fs::create_dir_all(&top).expect("scratch dir");
        std::fs::write(
            top.join("app.py"),
            "def fetch_invoice(invoice_id):\n    \"\"\"Fetch one invoice.\"\"\"\n    return invoice_id\n\n\n\
             class Ledger:\n    \"\"\"Holds the entries.\"\"\"\n\n    def total(self):\n        return 0\n",
        )
        .expect("write source");
        let path = top.to_string_lossy().into_owned();
        (Scratch(root), path)
    }

    /// A first run embeds every node and writes the sidecar; a second run on
    /// the unchanged repo embeds nothing and keeps every row.
    #[test]
    fn embeds_then_keeps_fresh_rows() {
        let (_s, top) = repo("twice");
        let first = embed_report(&top, "test-hash", None, false, None, false).expect("first run");
        assert!(first.embedded > 0, "{first:?}");
        assert_eq!(first.model, glia_embed::MODEL_TEST_HASH);
        assert_eq!(first.rows_written, first.embedded);
        let body = std::fs::read_to_string(Path::new(&top).join(VECTORS_FILE)).expect("sidecar");
        let second = embed_report(&top, "test-hash", None, false, None, false).expect("second");
        assert_eq!(second.embedded, 0, "{second:?}");
        assert_eq!(second.cached, first.embedded);
        assert_eq!(
            std::fs::read_to_string(Path::new(&top).join(VECTORS_FILE)).expect("sidecar"),
            body,
            "an unchanged repo rewrites the sidecar byte for byte"
        );
    }

    /// `check` counts the work and writes nothing: no sidecar and no layout.
    #[test]
    fn check_writes_nothing() {
        let (_s, top) = repo("check");
        let r = embed_report(&top, "test-hash", Some("t2"), false, None, true).expect("check");
        assert!(r.embedded > 0, "{r:?}");
        assert_eq!(r.rows_written, 0);
        assert_eq!(r.recipe, EmbedRecipe::T2);
        assert!(
            !Path::new(&top).join(".glia").exists(),
            "check=True wrote under .glia/"
        );
    }

    /// `limit` defers the rest to a later call.
    #[test]
    fn limit_defers() {
        let (_s, top) = repo("limit");
        let r = embed_report(&top, "test-hash", None, false, Some(1), false).expect("limited");
        assert_eq!(r.embedded, 1, "{r:?}");
        assert!(r.deferred > 0, "{r:?}");
    }

    /// An unknown model or recipe is refused before any build writes.
    #[test]
    fn refuses_unknown_names_before_building() {
        let (_s, top) = repo("names");
        let e = embed_report(&top, "nope", None, false, None, false).expect_err("model");
        assert!(e.contains("unknown embedding model `nope`"), "{e}");
        assert!(e.contains("bge-small, test-hash"), "{e}");
        let e = embed_report(&top, "test-hash", Some("t9"), false, None, false).expect_err("r");
        assert_eq!(e, "unknown embed recipe `t9`: expected one of t1, t2");
        assert!(!Path::new(&top).join(".glia").exists());
        let e = embed_report(
            "/nonexistent/glia-fb5c",
            "test-hash",
            None,
            false,
            None,
            false,
        )
        .expect_err("dir");
        assert!(e.ends_with("is not a directory"), "{e}");
    }

    /// Without the `embed` feature, bge-small is glia-embed's refusal.
    #[cfg(not(feature = "embed"))]
    #[test]
    fn bge_small_needs_the_feature() {
        let (_s, top) = repo("bge");
        let e = embed_report(&top, "bge-small", None, false, None, false).expect_err("bge");
        assert!(e.contains("without the embedding model"), "{e}");
    }
}
