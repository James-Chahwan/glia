//! FB.5a: the embed run over `.glia/vectors.jsonl`, with the deterministic
//! `test-hash` model.

use std::path::{Path, PathBuf};

use glia_code_domain::external_inputs::{VectorRow, b64_encode, read_rows, write_rows};
use glia_core::RepoId;
use glia_embed::{Device, Embedder, HashEmbedder, MODEL_TEST_HASH, codec};
use glia_engine::semantic::{
    EmbedOptions, EmbedRecipe, EmbedReport, VECTORS_FILE, VectorIndex, embed_repo, embed_text,
    text_hash,
};
use glia_engine::{GenerateResult, generate_one};

fn app_py(doc: &str, chart: &str) -> String {
    format!(
        "import requests\n\n\n\
         def fetch_invoice(invoice_id):\n    \"\"\"{doc}\"\"\"\n    return requests.get(invoice_id)\n\n\n\
         def {chart}(points):\n    \"\"\"Draw the chart of the points.\"\"\"\n    return list(points)\n\n\n\
         class Ledger:\n    \"\"\"Holds the entries.\"\"\"\n\n    def total(self):\n        return 0\n"
    )
}

struct Repo {
    td: tempfile::TempDir,
}

impl Repo {
    fn new(file: &str, src: &str) -> Self {
        let td = tempfile::tempdir().expect("tempdir");
        std::fs::write(td.path().join(file), src).expect("write source");
        Repo { td }
    }

    fn root(&self) -> &Path {
        self.td.path()
    }

    fn build(&self) -> GenerateResult {
        generate_one(self.root().to_str().expect("utf-8")).expect("generate_one")
    }

    fn vectors(&self) -> PathBuf {
        self.root().join(VECTORS_FILE)
    }

    fn body(&self) -> String {
        std::fs::read_to_string(self.vectors()).expect("vectors.jsonl")
    }

    fn embed(&self, opts: &EmbedOptions) -> Result<EmbedReport, String> {
        let r = self.build();
        embed_repo(self.root(), &r.merged, repo_of(&r), &HashEmbedder, opts)
    }

    fn run(&self) -> EmbedReport {
        self.embed(&EmbedOptions::default()).expect("embed_repo")
    }
}

fn repo_of(r: &GenerateResult) -> RepoId {
    let keys: Vec<u64> = r.repo_roots.keys().copied().collect();
    assert_eq!(keys.len(), 1, "one repo");
    RepoId(keys[0])
}

fn rows(path: &Path) -> Vec<VectorRow> {
    let (rows, errors) = read_rows::<VectorRow>(path);
    assert!(errors.is_empty(), "{errors:?}");
    rows
}

#[test]
fn first_run_embeds_every_node_and_a_second_embeds_none() {
    let repo = Repo::new("app.py", &app_py("Fetch one invoice.", "render_chart"));
    let first = repo.run();
    assert!(
        first.nodes >= 5,
        "module, 2 functions, class, method: {first:?}"
    );
    assert_eq!(
        (
            first.embedded,
            first.cached,
            first.reused,
            first.stale_dropped
        ),
        (first.nodes, 0, 0, 0),
        "{first:?}"
    );
    assert_eq!(first.rows_written, first.nodes);
    assert_eq!(first.model, MODEL_TEST_HASH);
    assert_eq!(first.recipe, EmbedRecipe::T1);
    let body = repo.body();

    // Every row is fresh to the index and carries its kind, hint and hash.
    let r = repo.build();
    let written = rows(&repo.vectors());
    for row in &written {
        assert_eq!(row.model.as_deref(), Some(MODEL_TEST_HASH));
        assert!(row.kind.is_some() && row.text_hash.is_some(), "{row:?}");
    }
    assert!(
        written
            .iter()
            .any(|row| row.qname == "app::fetch_invoice" && row.hint.is_some()),
        "a function row carries its v1 hint"
    );
    let roots = r
        .repo_roots
        .iter()
        .map(|(k, v)| (*k, PathBuf::from(v)))
        .collect();
    let idx = VectorIndex::load(&r.merged, &roots, MODEL_TEST_HASH, EmbedRecipe::T1);
    assert_eq!(idx.stats().missing, 0, "{:?}", idx.stats());
    assert_eq!(idx.stats().fresh, first.nodes);

    let second = repo.run();
    assert_eq!(
        (second.embedded, second.cached, second.stale_dropped),
        (0, first.nodes, 0),
        "{second:?}"
    );
    assert_eq!(repo.body(), body, "a re-run is byte-identical");
}

#[test]
fn an_edited_docstring_embeds_one_node() {
    let repo = Repo::new("app.py", &app_py("Fetch one invoice.", "render_chart"));
    let first = repo.run();
    std::fs::write(
        repo.root().join("app.py"),
        app_py("Fetch an invoice by id.", "render_chart"),
    )
    .expect("edit");
    let second = repo.run();
    assert_eq!(
        (second.embedded, second.stale_dropped, second.cached),
        (1, 1, first.nodes - 1),
        "{second:?}"
    );
    assert_eq!(second.rows_written, first.nodes);
}

#[test]
fn a_renamed_function_embeds_one_and_drops_its_old_row() {
    let repo = Repo::new("app.py", &app_py("Fetch one invoice.", "render_chart"));
    let first = repo.run();
    std::fs::write(
        repo.root().join("app.py"),
        app_py("Fetch one invoice.", "draw_chart"),
    )
    .expect("rename");
    let second = repo.run();
    assert_eq!(
        (second.embedded, second.stale_dropped),
        (1, 1),
        "{second:?}"
    );
    let qnames: Vec<String> = rows(&repo.vectors()).into_iter().map(|r| r.qname).collect();
    assert!(
        qnames.contains(&"app::draw_chart".to_string()),
        "{qnames:?}"
    );
    assert!(
        !qnames.contains(&"app::render_chart".to_string()),
        "{qnames:?}"
    );
    assert_eq!(second.rows_written, first.nodes);
}

#[test]
fn a_moved_file_reembeds_its_nodes_under_their_new_qnames() {
    let repo = Repo::new("app.py", &app_py("Fetch one invoice.", "render_chart"));
    let first = repo.run();
    std::fs::rename(repo.root().join("app.py"), repo.root().join("billing.py")).expect("move");
    let second = repo.run();
    assert_eq!(
        (second.embedded, second.stale_dropped, second.cached),
        (first.nodes, first.nodes, 0),
        "every node's qname moved, so its text did: {second:?}"
    );
    assert!(
        rows(&repo.vectors())
            .iter()
            .all(|r| r.qname.starts_with("billing")),
        "old rows dropped"
    );
}

#[test]
fn a_row_under_another_key_with_the_same_text_is_reused() {
    let repo = Repo::new("app.py", &app_py("Fetch one invoice.", "render_chart"));
    let first = repo.run();
    let mut moved = rows(&repo.vectors());
    let at = moved
        .iter()
        .position(|r| r.qname == "app::fetch_invoice")
        .expect("row");
    moved[at].qname = "app::old_name".to_string();
    moved[at].hint = None;
    let b64 = moved[at].b64.clone();
    write_rows(&repo.vectors(), &moved).expect("rewrite");

    let second = repo.run();
    assert_eq!(
        (
            second.reused,
            second.embedded,
            second.stale_dropped,
            second.cached
        ),
        (1, 0, 1, first.nodes - 1),
        "{second:?}"
    );
    let back = rows(&repo.vectors());
    let row = back
        .iter()
        .find(|r| r.qname == "app::fetch_invoice")
        .expect("re-keyed row");
    assert_eq!(row.b64, b64, "the cached bytes, unchanged");
    assert!(!back.iter().any(|r| r.qname == "app::old_name"));
}

#[test]
fn another_models_file_is_refused_unless_replace() {
    let repo = Repo::new("app.py", &app_py("Fetch one invoice.", "render_chart"));
    let v = HashEmbedder::embed_one("x");
    let other = VectorRow {
        qname: "app::fetch_invoice".to_string(),
        kind: Some("FUNCTION".to_string()),
        hint: None,
        model: Some("Xenova/bge-small-en-v1.5".to_string()),
        dims: Some(v.len() as u32),
        text_hash: None,
        device: None,
        b64: b64_encode(&codec::to_le_bytes(&v)),
    };
    write_rows(&repo.vectors(), &[other]).expect("seed");
    let before = repo.body();
    let err = repo.embed(&EmbedOptions::default()).unwrap_err();
    assert!(
        err.contains("Xenova/bge-small-en-v1.5") && err.contains("--replace"),
        "{err}"
    );
    assert_eq!(repo.body(), before, "nothing written");

    let replaced = repo
        .embed(&EmbedOptions::default().with_replace(true))
        .expect("replace");
    assert_eq!(replaced.stale_dropped, 1, "{replaced:?}");
    assert_eq!(replaced.embedded, replaced.nodes);
    assert!(
        rows(&repo.vectors())
            .iter()
            .all(|r| r.model.as_deref() == Some(MODEL_TEST_HASH))
    );
}

#[test]
fn a_foreign_row_of_this_model_is_kept_byte_identical() {
    let repo = Repo::new("app.py", &app_py("Fetch one invoice.", "render_chart"));
    let v = HashEmbedder::embed_one("an external note");
    let foreign = VectorRow {
        qname: "app::external_note".to_string(),
        kind: None,
        hint: None,
        model: Some(MODEL_TEST_HASH.to_string()),
        dims: Some(v.len() as u32),
        text_hash: None,
        device: None,
        b64: b64_encode(&codec::to_le_bytes(&v)),
    };
    write_rows(&repo.vectors(), std::slice::from_ref(&foreign)).expect("seed");
    let line = repo.body();
    let first = repo.run();
    assert_eq!(first.foreign_kept, 1, "{first:?}");
    assert_eq!(first.rows_written, first.nodes + 1);
    assert!(repo.body().contains(&line), "the foreign line, unchanged");
    let second = repo.run();
    assert_eq!((second.foreign_kept, second.embedded), (1, 0), "{second:?}");
}

#[test]
fn check_only_counts_and_writes_nothing() {
    let repo = Repo::new("app.py", &app_py("Fetch one invoice.", "render_chart"));
    let report = repo
        .embed(&EmbedOptions::default().with_check_only(true))
        .expect("check");
    assert_eq!(report.embedded, report.nodes, "{report:?}");
    assert_eq!(report.rows_written, 0);
    assert!(!repo.vectors().exists(), "no sidecar written");
    assert!(!repo.root().join(".glia/cells.lock").exists());
}

#[test]
fn limit_bounds_a_run_and_the_next_finishes() {
    let repo = Repo::new("app.py", &app_py("Fetch one invoice.", "render_chart"));
    let first = repo
        .embed(&EmbedOptions::default().with_limit(Some(2)))
        .expect("limited");
    assert_eq!(first.embedded, 2, "{first:?}");
    assert_eq!(first.deferred, first.nodes - 2);
    assert_eq!(first.rows_written, 2);
    let second = repo.run();
    assert_eq!(
        (second.embedded, second.cached),
        (first.nodes - 2, 2),
        "{second:?}"
    );
}

#[test]
fn the_rows_hash_the_recipe_text() {
    let repo = Repo::new("app.py", &app_py("Fetch one invoice.", "render_chart"));
    let report = repo
        .embed(&EmbedOptions::default().with_recipe(EmbedRecipe::T2))
        .expect("t2");
    assert_eq!(report.recipe, EmbedRecipe::T2);
    let r = repo.build();
    let id = r
        .merged
        .graphs
        .iter()
        .find_map(|g| {
            g.nodes
                .iter()
                .find(|n| {
                    g.nav.qname_by_id.get(&n.id).map(String::as_str) == Some("app::fetch_invoice")
                })
                .map(|n| n.id)
        })
        .expect("node");
    let text = embed_text(&r.merged, id, EmbedRecipe::T2).expect("text");
    let row = rows(&repo.vectors())
        .into_iter()
        .find(|r| r.qname == "app::fetch_invoice")
        .expect("row");
    assert_eq!(
        row.text_hash.as_deref(),
        Some(text_hash(EmbedRecipe::T2, &text).as_str())
    );
    assert_eq!(
        row.b64,
        b64_encode(&codec::to_le_bytes(&HashEmbedder::embed_one(&text)))
    );
    // The other recipe sees every row as stale and re-embeds them all.
    let back = repo.run();
    assert_eq!(
        (back.embedded, back.stale_dropped),
        (back.nodes, back.nodes),
        "{back:?}"
    );
}

#[test]
fn a_run_inside_a_rayon_worker_is_refused() {
    let repo = Repo::new("app.py", &app_py("Fetch one invoice.", "render_chart"));
    let r = repo.build();
    let repo_id = repo_of(&r);
    let mut out: Option<Result<EmbedReport, String>> = None;
    rayon::scope(|_| {
        out = Some(embed_repo(
            repo.root(),
            &r.merged,
            repo_id,
            &HashEmbedder,
            &EmbedOptions::default(),
        ));
    });
    let err = out.expect("ran").unwrap_err();
    assert!(err.contains("rayon worker"), "{err}");
    assert!(!repo.vectors().exists());
}

/// The test model reporting a GPU device, as a worker embedder on WebGPU
/// does (FB.14): its vectors are HashEmbedder's, tagged `webgpu`.
struct OnWebGpu;

impl Embedder for OnWebGpu {
    fn model(&self) -> &str {
        MODEL_TEST_HASH
    }

    fn dims(&self) -> u32 {
        HashEmbedder.dims()
    }

    fn device(&self) -> Option<Device> {
        Some(Device::WebGpu)
    }

    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        HashEmbedder.embed(texts)
    }
}

/// FB.14: every new row records the device its vector came from; the test
/// model records none (its rows stay byte-identical to FB.5a's); a row of
/// another device is fresh, and a reused vector keeps its row's device.
#[test]
fn rows_record_their_device_and_the_device_never_decides_freshness() {
    let repo = Repo::new("app.py", &app_py("Fetch one invoice.", "render_chart"));
    let r = repo.build();
    let first = embed_repo(
        repo.root(),
        &r.merged,
        repo_of(&r),
        &OnWebGpu,
        &EmbedOptions::default(),
    )
    .expect("embed_repo");
    assert_eq!(first.embedded, first.nodes);
    let written = rows(&repo.vectors());
    assert!(
        written
            .iter()
            .all(|row| row.device.as_deref() == Some("webgpu")),
        "{written:?}"
    );
    let body = repo.body();
    assert!(body.contains(r#""device":"webgpu""#), "{body}");

    // The device-less test model finds every webgpu row fresh.
    let second = repo.run();
    assert_eq!(
        (second.embedded, second.cached),
        (0, first.nodes),
        "{second:?}"
    );
    assert_eq!(
        repo.body(),
        body,
        "rows of another device kept as they were"
    );

    // A vector reused by its text hash keeps its device.
    let mut moved = rows(&repo.vectors());
    let at = moved
        .iter()
        .position(|row| row.qname == "app::fetch_invoice")
        .expect("row");
    moved[at].qname = "app::old_name".to_string();
    moved[at].hint = None;
    write_rows(&repo.vectors(), &moved).expect("rewrite");
    let third = repo.run();
    assert_eq!((third.reused, third.embedded), (1, 0), "{third:?}");
    let back = rows(&repo.vectors());
    let row = back
        .iter()
        .find(|row| row.qname == "app::fetch_invoice")
        .expect("re-keyed row");
    assert_eq!(row.device.as_deref(), Some("webgpu"));

    // A fresh repo embedded by the test model writes no device at all.
    let plain = Repo::new("app.py", &app_py("Fetch one invoice.", "render_chart"));
    plain.run();
    assert!(!plain.body().contains("\"device\""), "{}", plain.body());
}
