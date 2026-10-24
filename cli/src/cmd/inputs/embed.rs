//! `glia embed` (FB.5b) — the CLI surface of the embedding snapshot step,
//! `glia_engine::semantic::embed_repo` (FB.5a): bring `<repo>/.glia/vectors.jsonl`
//! up to date with the graph, re-embedding only the nodes whose embed text
//! changed.
//!
//! The run, in order:
//! 1. The model: `--model`, else `semantic::surface_model()` (bge-small unless
//!    `GLIA_EMBED_MODEL=test-hash`). For bge-small the model cache directory is
//!    checked against the repo BEFORE the model loads (or downloads),
//!    `glia_embed::refuse_repo_cache`, so a `FASTEMBED_CACHE_DIR` inside the
//!    tree exits 2 having fetched nothing.
//! 2. The graph: the repo's default layout through `persist::load_or_rebuild`
//!    (vectors are not a build input, so a fresh layout stays fresh after an
//!    embed); a layout of more than one repo exits 2. The build has returned,
//!    so its rayon pool is idle, before any model session exists.
//! 3. `--check` counts the work and writes nothing: it never loads a model
//!    (a name-only embedder carries the model's name and dims), and exits 1
//!    when any node needs a vector. Otherwise `embed_repo` runs; bge-small
//!    runs on the device `GLIA_EMBED_DEVICE` selects (`glia_embed::bge_small`,
//!    FB.14: the GPU by default, its session in a `glia embed-worker` child of
//!    this binary, the CPU in-process on any GPU failure), and the model loads
//!    on the first chunk in whichever process embeds.
//!
//! Never part of a build: find, flows and pack read the sidecar. Exits 0 on a
//! written (or, under `--check`, already fresh) sidecar, 1 when `--check`
//! found work, 2 on any error (an explicit `GLIA_EMBED_DEVICE` whose device
//! does not start among them). fired_on, on stderr: bge-small's `[embed]
//! device=<adapter> backend=<cpu|webgpu|cuda> driver=<v> fallback=<reason|none>
//! batch=<n>` line, the engine's `[embed] repo=<label> model=<m> recipe=<t>
//! nodes=.. embedded=<e> ..` line, then `[embed] cli repo=<label>
//! exit=<code>` on every exit.

use std::path::Path;

use clap::builder::PossibleValuesParser;
use glia_core::RepoId;
use glia_embed::Embedder;
use glia_engine::arch::repo_label_for;
use glia_engine::persist::{default_layout_dir, load_or_rebuild};
use glia_engine::semantic::{
    DEFAULT_RECIPE, EmbedOptions, EmbedRecipe, EmbedReport, VECTORS_FILE, embed_repo, surface_model,
};

const EXIT_OK: i32 = 0;
const EXIT_WORK: i32 = 1;
const EXIT_ERROR: i32 = 2;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Repo whose nodes to embed.
    repo: String,
    /// The embedding model (default bge-small, or GLIA_EMBED_MODEL).
    #[arg(long, value_name = "MODEL", value_parser = PossibleValuesParser::new(glia_embed::MODEL_NAMES))]
    model: Option<String>,
    /// The text recipe: t1 (name, qname, leading doc, imports) or t2 (t1 then
    /// the code). Default t1.
    #[arg(long, value_name = "RECIPE", value_parser = PossibleValuesParser::new(["t1", "t2"]))]
    recipe: Option<String>,
    /// Rewrite a sidecar that holds another model's vectors, dropping them.
    #[arg(long)]
    replace: bool,
    /// Embed at most this many nodes this run; the rest wait for the next.
    #[arg(long, value_name = "N")]
    limit: Option<usize>,
    /// Count the nodes with no fresh vector and write nothing; exits 1 when
    /// there are any.
    #[arg(long)]
    check: bool,
    /// Print the run's report as JSON.
    #[arg(long)]
    json: bool,
}

/// A name-only embedder for `--check`: `embed_repo` under `check_only` reads
/// the model's name and dims and never calls the model, so no session loads.
struct CheckOnly {
    model: &'static str,
    dims: u32,
}

impl Embedder for CheckOnly {
    fn model(&self) -> &str {
        self.model
    }

    fn dims(&self) -> u32 {
        self.dims
    }

    fn embed(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        Err("--check never embeds".to_string())
    }
}

pub(crate) fn run(a: Args) -> i32 {
    let mut label = repo_label_for(&a.repo);
    let code = match embed(&a, &mut label) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            EXIT_ERROR
        }
    };
    eprintln!("[embed] cli repo={label} exit={code}");
    code
}

fn embed(a: &Args, label: &mut String) -> Result<i32, String> {
    let root = Path::new(&a.repo);
    if !root.is_dir() {
        return Err(format!("{} is not a directory", a.repo));
    }
    let model = a.model.as_deref().unwrap_or_else(|| surface_model());
    let recipe = match a.recipe.as_deref() {
        Some(r) => EmbedRecipe::parse(r).ok_or_else(|| format!("unknown recipe `{r}`"))?,
        None => DEFAULT_RECIPE,
    };
    if model == "bge-small" {
        glia_embed::refuse_repo_cache(root)?;
    }

    let (result, _outcome) = load_or_rebuild(&default_layout_dir(root), Some(root), true)
        .map_err(|e| format!("the graph of {}: {e}", a.repo))?;
    let keys: Vec<u64> = result.repo_roots.keys().copied().collect();
    let [repo] = keys[..] else {
        return Err(format!(
            "the layout of {} holds {} repos; glia embed embeds one repo",
            a.repo,
            keys.len()
        ));
    };
    if let Some(l) = result.repo_labels.get(&repo) {
        label.clone_from(l);
    }

    let opts = EmbedOptions::default()
        .with_recipe(recipe)
        .with_replace(a.replace)
        .with_limit(a.limit)
        .with_check_only(a.check);
    let report = if a.check {
        let check = check_embedder(model)?;
        embed_repo(root, &result.merged, RepoId(repo), &check, &opts)?
    } else {
        let embedder = load_embedder(model)?;
        embed_repo(root, &result.merged, RepoId(repo), embedder.as_ref(), &opts)?
    };
    label.clone_from(&report.repo);
    render(&report, a)
}

fn check_embedder(model: &str) -> Result<CheckOnly, String> {
    match model {
        "bge-small" => Ok(CheckOnly {
            model: glia_embed::MODEL_BGE_SMALL,
            dims: glia_embed::DIMS_BGE_SMALL,
        }),
        "test-hash" => Ok(CheckOnly {
            model: glia_embed::MODEL_TEST_HASH,
            dims: glia_embed::DIMS_TEST_HASH,
        }),
        other => Err(format!(
            "unknown embedding model `{other}`: expected one of {}",
            glia_embed::MODEL_NAMES.join(", ")
        )),
    }
}

/// The embedder `model` names. bge-small's GPU worker is this binary as
/// `glia embed-worker` (cmd/embed_worker.rs) when it carries a GPU provider
/// (a source build's default `gpu-webgpu`); else the `glia-embed-worker`
/// installed beside it (the glia-cli wheel builds `glia` without the provider,
/// so it starts on any glibc, and ships the worker with the Dawn library);
/// else this binary again, which answers `not_built` (the CPU). A build
/// without the `embed` feature refuses bge-small with glia-embed's message.
fn load_embedder(model: &str) -> Result<Box<dyn Embedder>, String> {
    use glia_embed::worker::WorkerProgram;
    if model == "bge-small" {
        let this = || WorkerProgram::this_binary(&[crate::cmd::embed_worker::SUBCOMMAND]);
        let gpu_built = glia_embed::device::WEBGPU_BUILT || glia_embed::device::CUDA_BUILT;
        let worker = match WorkerProgram::beside_current_exe() {
            Some(external) if !gpu_built => external,
            _ => this()?,
        };
        return glia_embed::bge_small(Some(worker));
    }
    glia_embed::named(model)
}

fn render(r: &EmbedReport, a: &Args) -> Result<i32, String> {
    let need = r.embedded + r.reused;
    if a.json {
        println!(
            "{}",
            serde_json::to_string_pretty(r).map_err(|e| e.to_string())?
        );
    } else if a.check {
        println!(
            "{need} nodes need embedding (model {}, recipe {})",
            r.model,
            r.recipe.as_str()
        );
    } else {
        println!(
            "embedded {}, cached {}, reused {}, dropped {} -> {VECTORS_FILE} ({} rows, model {}, recipe {})",
            r.embedded,
            r.cached,
            r.reused,
            r.stale_dropped,
            r.rows_written,
            r.model,
            r.recipe.as_str()
        );
    }
    if !a.json && (r.ambiguous > 0 || r.deferred > 0) {
        println!(
            "skipped {} nodes sharing a (qname, kind), deferred {} past --limit",
            r.ambiguous, r.deferred
        );
    }
    Ok(if a.check && need > 0 {
        EXIT_WORK
    } else {
        EXIT_OK
    })
}
