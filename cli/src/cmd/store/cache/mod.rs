//! `glia cache push|pull|gc` (CE.2c): the shared parse cache's transport, a
//! CLI-only layer over the engine's keys, export and verified import
//! (`glia_engine::shared_cache`, CE.2a / CE.2b). The build stays offline: a
//! pull writes `<repo>/.glia/graph/parse_cache.bin` and the next build reuses
//! it through its ordinary checks; nothing here runs inside a build, and
//! neither the engine nor the glia-py wheel moves or authenticates bytes.
//!
//! - `push <repo> <store>` exports the entries of the repo's sidecar a build
//!   of the checkout would reuse and uploads each object the store lacks.
//! - `pull <repo> <store>` fetches the objects of the files the checkout's
//!   sidecar lacks (`--jobs` threads, each result in its row's slot so the
//!   outcome never depends on scheduling), refuses any that fail
//!   `object::decode`, then hands the rest to the engine's import, which
//!   checks each against the checkout and re-parses a sample (`--verify`,
//!   default 0 for a keyed store, 32 for `--unsigned`). A poisoned sample
//!   exits 1 and nothing is written.
//! - `gc <store>` prunes a directory store (`gc.rs`).
//! - `--layout` on push and pull (CE.2d, `layout.rs`) moves the whole
//!   `.glia/graph/` layout of a clean checkout at a known git tree as one
//!   object in parts: push uploads it unless the store holds it, pull installs
//!   it through the engine's verified install, then runs the per-file pull.
//!
//! Trust: objects carry a blake3 keyed MAC (`object.rs`) under the key in
//! `--key-file` or `GLIA_CACHE_KEY`; without one a push or pull needs
//! `--unsigned` (the store is trusted as-is and pulls are re-parse-sampled).
//! The key is never printed.
//!
//! Stores (the STORE argument): an `https://` URL is an HTTP object store
//! (`http.rs`, CE.2e: GET / HEAD / PUT, `Authorization: Bearer` from
//! `GLIA_CACHE_TOKEN`); a plain `http://` URL is one only when its host is
//! loopback, and any other is refused before a request is made, as is any
//! other `<scheme>://`; anything else is a directory (`store.rs`). `gc`
//! prunes a directory only: a remote store is pruned on its server.
//!
//! Exit codes: 0 ok; 1 a store or verification failure (a failed pull writes
//! nothing); 2 a usage error (bad store, no or bad key, not a repo).
//!
//! fired_on markers (stderr), after the engine's `[cache] export` / `[cache]
//! import` lines:
//! `[cache] push store=<label> repo=<label> entries=<n> uploaded=<u> present=<p> stale=<s> signed=<yes|no>`
//! `[cache] pull store=<label> repo=<label> files=<n> local_hits=<h> fetched=<f> missing=<m> rejected=<r> verified=<v> signed=<yes|no>`
//! `[cache] gc store=<dir> stamps_kept=<k> stamps_removed=<r> objects_removed=<o> bytes=<before>-><after>`
//! `[cache] layout <push|pull> repo=<label> key=<12 hex|-> tree=<12 hex|-> result=<pushed|present|stale|dirty|hit|miss|rejected>`
//! (before the per-file line, with `--layout`)
//! `[cache] store=<scheme>://<host> transport=<https|http-loopback> requests=<n> (get=<g> head=<h> put=<p>) status_4xx=<a> status_5xx=<b>`
//! (last, once per push or pull through an HTTP store, succeeded or not)

mod gc;
mod http;
mod layout;
mod object;
mod store;

use std::path::{Path, PathBuf};

use clap::Subcommand;
use glia_engine::shared_cache::{
    CacheKey, ImportOptions, Verify, export_entries, import_entries, wanted,
};
use serde_json::json;

use gc::{GcOptions, gc};
use http::{HttpStore, StoreUrl, token_from_env};
use object::{KEY_ENV, Reject, Signing, decode, encode, object_rel};
use store::{DirStore, ObjectStore};

const EXIT_OK: i32 = 0;
/// A store or verification failure.
const EXIT_FAILED: i32 = 1;
/// A usage error: bad store, no or bad key, not a repo.
const EXIT_USAGE: i32 = 2;
/// Most `[cache] rejected` lines one pull prints for its transport rejections.
const MAX_REJECTION_LINES: usize = 20;
/// The re-parse sample of an `--unsigned` pull when `--verify` is not given.
const UNSIGNED_SAMPLE: usize = 32;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    #[command(subcommand)]
    action: CacheCmd,
}

#[derive(Subcommand, Debug)]
enum CacheCmd {
    /// Upload the parse cache of a built checkout to a directory or HTTPS
    /// object store: every entry of `<repo>/.glia/graph/parse_cache.bin` a
    /// build of the checkout as it is now would reuse, one object per content
    /// address, skipping objects the store already holds. With `--layout`,
    /// first the whole layout.
    Push(PushArgs),
    /// Fetch the parses the checkout's cache lacks from a directory or HTTPS
    /// object store, check each object (size, key, MAC), import them through
    /// the engine's checks and re-parse sample, and write
    /// `<repo>/.glia/graph/parse_cache.bin`. Exits
    /// 1 and writes nothing when a sampled payload differs from a local parse.
    /// With `--layout`, first install the whole layout for the checkout.
    Pull(PullArgs),
    /// Prune a directory store: keep the newest `--keep-stamps` build stamps
    /// (by last upload), then delete the oldest objects over `--max-bytes`,
    /// and remove staging files a crashed push left over an hour ago.
    Gc(GcArgs),
}

#[derive(clap::Args, Debug)]
struct KeyArgs {
    /// Trust the store as-is: push objects without a MAC, and pull without
    /// checking one (a pull then re-parses `--verify` entries, default 32).
    /// A key, when one is set, wins.
    #[arg(long)]
    unsigned: bool,
    /// The signing key: a file holding 64 hex characters (32 bytes). Default:
    /// the GLIA_CACHE_KEY environment variable.
    #[arg(long, value_name = "FILE")]
    key_file: Option<PathBuf>,
}

#[derive(clap::Args, Debug)]
struct PushArgs {
    /// Path to the built repo (its `.glia/graph/parse_cache.bin` is read).
    repo: String,
    /// The store: a directory (created when missing), or an `https://` URL
    /// (PUT; `Authorization: Bearer $GLIA_CACHE_TOKEN` when set). Plain
    /// `http://` only to a loopback host.
    store: String,
    #[command(flatten)]
    key: KeyArgs,
    /// Also upload the whole `.glia/graph/` layout (manifest and shards) of a
    /// clean checkout whose layout is fresh, keyed by the build stamp, repo
    /// identity, HEAD tree, `.glia` inputs and target, unless the store holds
    /// it. A dirty checkout or a stale layout is reported and skipped.
    #[arg(long)]
    layout: bool,
    /// Print the summary as JSON.
    #[arg(long)]
    json: bool,
}

#[derive(clap::Args, Debug)]
struct PullArgs {
    /// Path to the checkout to fill (its `.glia/graph/parse_cache.bin` is
    /// written).
    repo: String,
    /// The store: an existing directory, or an `https://` URL (GET;
    /// `Authorization: Bearer $GLIA_CACHE_TOKEN` when set). Plain `http://`
    /// only to a loopback host.
    store: String,
    #[command(flatten)]
    key: KeyArgs,
    /// How many accepted entries to re-parse and compare byte for byte
    /// before anything is written: a count, or `all`. Default 0 with a key
    /// (the MAC authenticates), 32 with `--unsigned`.
    #[arg(long, value_name = "N|all", value_parser = parse_sample)]
    verify: Option<Sample>,
    /// Parallel object fetches.
    #[arg(long, default_value_t = 8, value_parser = clap::value_parser!(u16).range(1..=256))]
    jobs: u16,
    /// First install the whole `.glia/graph/` layout the store holds for this
    /// clean checkout's key, checked (size, key, MAC) and then verified by the
    /// engine (key re-derived, names, a fresh load) before it replaces the
    /// current layout; then pull the per-file cache as usual.
    #[arg(long)]
    layout: bool,
    /// Print the summary as JSON.
    #[arg(long)]
    json: bool,
}

#[derive(clap::Args, Debug)]
struct GcArgs {
    /// The directory store to prune (a remote store is pruned on its server).
    store: String,
    /// How many build stamps to keep, newest first by last upload.
    #[arg(long, default_value_t = 2)]
    keep_stamps: usize,
    /// A byte budget for the kept objects (a count, or with a K / M / G
    /// suffix, powers of 1024): the oldest objects go first.
    #[arg(long, value_name = "BYTES", value_parser = parse_bytes)]
    max_bytes: Option<u64>,
    /// Print the summary as JSON.
    #[arg(long)]
    json: bool,
}

/// `--verify`: a sample size or every entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sample {
    Count(usize),
    All,
}

impl Sample {
    fn engine(self) -> Verify {
        match self {
            Sample::Count(n) => Verify::Count(n),
            Sample::All => Verify::All,
        }
    }

    fn json(self) -> serde_json::Value {
        match self {
            Sample::Count(n) => json!(n),
            Sample::All => json!("all"),
        }
    }
}

fn parse_sample(s: &str) -> Result<Sample, String> {
    if s.eq_ignore_ascii_case("all") {
        return Ok(Sample::All);
    }
    s.parse::<usize>()
        .map(Sample::Count)
        .map_err(|_| format!("want a count or `all`, got {s:?}"))
}

fn parse_bytes(s: &str) -> Result<u64, String> {
    let t = s.trim();
    let (digits, mult) = match t.char_indices().last() {
        Some((i, c)) if c.eq_ignore_ascii_case(&'k') => (&t[..i], 1u64 << 10),
        Some((i, c)) if c.eq_ignore_ascii_case(&'m') => (&t[..i], 1u64 << 20),
        Some((i, c)) if c.eq_ignore_ascii_case(&'g') => (&t[..i], 1u64 << 30),
        _ => (t, 1),
    };
    digits
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(mult))
        .ok_or_else(|| format!("want a byte count such as 500000000 or 512M, got {s:?}"))
}

pub(crate) fn run(args: Args) -> i32 {
    match args.action {
        CacheCmd::Push(a) => push(a),
        CacheCmd::Pull(a) => pull(a),
        CacheCmd::Gc(a) => run_gc(a),
    }
}

/// A usage error: its message, exit 2.
fn usage(msg: &str) -> i32 {
    eprintln!("error: {msg}");
    EXIT_USAGE
}

/// A store or verification failure: its message, exit 1.
fn failed(msg: &str) -> i32 {
    eprintln!("error: {msg}");
    EXIT_FAILED
}

fn yes_no(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

/// What a STORE argument names.
#[derive(Debug, PartialEq, Eq)]
enum StoreArg {
    Dir(PathBuf),
    Url(StoreUrl),
}

/// Classify a STORE argument without touching the network: `https://` (and
/// `http://` to a loopback host) is an HTTP store, plain `http://` to any
/// other host and any other `<scheme>://` are usage errors (the message names
/// the host or scheme, never the whole URL), anything else is a directory.
fn store_arg(s: &str) -> Result<StoreArg, String> {
    let url = s.split_once("://").filter(|(scheme, _)| {
        !scheme.is_empty()
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    });
    match url {
        None => Ok(StoreArg::Dir(PathBuf::from(s))),
        Some((scheme, rest))
            if scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("http") =>
        {
            StoreUrl::parse(scheme, rest).map(StoreArg::Url)
        }
        Some((scheme, _)) => Err(format!(
            "unsupported store {scheme}://: a store is a directory or an https:// URL"
        )),
    }
}

/// An opened store.
enum Store {
    Dir(DirStore),
    Http(HttpStore),
}

impl Store {
    fn object_store(&self) -> &dyn ObjectStore {
        match self {
            Store::Dir(d) => d,
            Store::Http(h) => h,
        }
    }

    /// Print the HTTP store's fired_on marker (a directory store has none).
    fn report(&self) {
        if let Store::Http(h) = self {
            eprintln!("{}", h.marker());
        }
    }
}

/// Open a checked STORE: a directory (created when `create`, else it must
/// exist), or the HTTP store with `GLIA_CACHE_TOKEN`. `Err` is the exit code
/// of the error it printed.
fn open(arg: StoreArg, create: bool) -> Result<Store, i32> {
    match arg {
        StoreArg::Dir(dir) => {
            if create {
                std::fs::create_dir_all(&dir)
                    .map_err(|e| failed(&format!("store {}: {e}", dir.display())))?;
            } else if !dir.is_dir() {
                return Err(usage(&format!(
                    "store {} is not a directory",
                    dir.display()
                )));
            }
            Ok(Store::Dir(DirStore::new(&dir)))
        }
        StoreArg::Url(url) => {
            let token = token_from_env().map_err(|e| usage(&e))?;
            Ok(Store::Http(HttpStore::new(url, token)))
        }
    }
}

/// The signing key of a push or pull: `--key-file`, else `GLIA_CACHE_KEY`,
/// else none when `--unsigned` (the usage error otherwise). A key given with
/// `--unsigned` wins, with one note. The key text is never printed.
fn signing(k: &KeyArgs) -> Result<Signing, String> {
    let keyed = match &k.key_file {
        Some(path) => Some(read_key_file(path)?),
        None => Signing::from_env()?,
    };
    match (keyed, k.unsigned) {
        (Some(s), true) => {
            eprintln!(
                "note: a cache key is set (--key-file or {KEY_ENV}), so objects are signed and checked; --unsigned is ignored"
            );
            Ok(s)
        }
        (Some(s), false) => Ok(s),
        (None, true) => Ok(Signing::unsigned()),
        (None, false) => Err(format!(
            "no cache key: set {KEY_ENV} or pass --key-file (or --unsigned to trust the store as-is)"
        )),
    }
}

fn read_key_file(path: &Path) -> Result<Signing, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("key file {}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mode = meta.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                eprintln!(
                    "warning: key file {} is readable by other users (mode {mode:o}); chmod 600 it",
                    path.display()
                );
            }
        }
    }
    Signing::from_hex(&text).map_err(|e| format!("key file {}: {e}", path.display()))
}

/// The checks every push / pull makes before any work: the store, the key,
/// the repo. Any failure is a usage error.
fn prepare(repo: &str, store: &str, key: &KeyArgs) -> Result<(StoreArg, Signing), String> {
    let arg = store_arg(store)?;
    let signing = signing(key)?;
    if !Path::new(repo).is_dir() {
        return Err(format!("not a directory: {repo}"));
    }
    Ok((arg, signing))
}

fn push(a: PushArgs) -> i32 {
    let (arg, signing) = match prepare(&a.repo, &a.store, &a.key) {
        Ok(p) => p,
        Err(e) => return usage(&e),
    };
    let store = match open(arg, true) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let code = push_to(&a, store.object_store(), &signing);
    store.report();
    code
}

fn push_to(a: &PushArgs, store: &dyn ObjectStore, signing: &Signing) -> i32 {
    let layout = if a.layout {
        match layout::push(&a.repo, store, signing) {
            Ok(o) => Some(o),
            Err(e) => return failed(&format!("layout push to {}: {e}", store.label())),
        }
    } else {
        None
    };
    let export = match export_entries(&a.repo) {
        Ok(x) => x,
        Err(e) => return failed(&e),
    };
    let (mut uploaded, mut present) = (0usize, 0usize);
    let mut objects = Vec::with_capacity(export.entries.len());
    for e in &export.entries {
        let rel = object_rel(export.stamp, &e.key);
        let status = match store.has(&rel) {
            Ok(true) => {
                present += 1;
                "present"
            }
            Ok(false) => {
                if let Err(err) = store.put(&rel, &encode(&e.key, &e.payload, signing)) {
                    return failed(&format!("push to {}: {err}", store.label()));
                }
                uploaded += 1;
                "uploaded"
            }
            Err(err) => return failed(&format!("push to {}: {err}", store.label())),
        };
        objects.push(json!({"path": e.path, "key": e.key, "status": status}));
    }
    let signed = signing.is_signed();
    eprintln!(
        "[cache] push store={} repo={} entries={} uploaded={uploaded} present={present} stale={} signed={}",
        store.label(),
        export.repo_label,
        export.entries.len(),
        export.stale,
        yes_no(signed)
    );
    if a.json {
        let mut out = json!({
            "store": store.label(),
            "repo": export.repo_label,
            "stamp": export.stamp,
            "entries": export.entries.len(),
            "uploaded": uploaded,
            "present": present,
            "stale": export.stale,
            "signed": signed,
            "objects": objects,
        });
        if let Some(l) = &layout {
            out["layout"] = l.json();
        }
        println!("{out}");
    } else {
        if let Some(l) = &layout {
            println!("{}", l.line(&export.repo_label, &store.label()));
        }
        println!(
            "push {} -> {}: {} entries, {uploaded} uploaded, {present} already present, {} stale ({})",
            export.repo_label,
            store.label(),
            export.entries.len(),
            export.stale,
            if signed { "signed" } else { "unsigned" }
        );
    }
    EXIT_OK
}

/// What fetching one wanted row's object came to.
enum Fetched {
    Missing,
    Rejected(Reject),
    Payload(Vec<u8>),
}

/// Fetch and decode `rels[i]` for `keys[i]`, on up to `jobs` threads over
/// contiguous chunks. Slot `i` holds row `i`'s result, so the outcome is the
/// same at any `jobs`.
fn fetch_all(
    store: &dyn ObjectStore,
    rels: &[String],
    keys: &[CacheKey],
    signing: &Signing,
    jobs: usize,
) -> Vec<Result<Fetched, String>> {
    let mut slots: Vec<Option<Result<Fetched, String>>> = rels.iter().map(|_| None).collect();
    if rels.is_empty() {
        return Vec::new();
    }
    let chunk = rels.len().div_ceil(jobs.clamp(1, rels.len()));
    let fetch_one = |rel: &str, key: &CacheKey| -> Result<Fetched, String> {
        Ok(match store.get(rel)? {
            None => Fetched::Missing,
            Some(bytes) => match decode(&bytes, key, signing) {
                Ok(payload) => Fetched::Payload(payload),
                Err(r) => Fetched::Rejected(r),
            },
        })
    };
    std::thread::scope(|scope| {
        for ((slot_chunk, rel_chunk), key_chunk) in slots
            .chunks_mut(chunk)
            .zip(rels.chunks(chunk))
            .zip(keys.chunks(chunk))
        {
            let fetch_one = &fetch_one;
            scope.spawn(move || {
                for ((slot, rel), key) in slot_chunk.iter_mut().zip(rel_chunk).zip(key_chunk) {
                    *slot = Some(fetch_one(rel, key));
                }
            });
        }
    });
    slots
        .into_iter()
        .map(|s| s.unwrap_or_else(|| Err("fetch did not run".to_string())))
        .collect()
}

fn pull(a: PullArgs) -> i32 {
    let (arg, signing) = match prepare(&a.repo, &a.store, &a.key) {
        Ok(p) => p,
        Err(e) => return usage(&e),
    };
    let store = match open(arg, false) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let code = pull_from(&a, store.object_store(), &signing);
    store.report();
    code
}

fn pull_from(a: &PullArgs, store: &dyn ObjectStore, signing: &Signing) -> i32 {
    let signed = signing.is_signed();
    let sample = a.verify.unwrap_or(if signed {
        Sample::Count(0)
    } else {
        Sample::Count(UNSIGNED_SAMPLE)
    });
    let layout = if a.layout {
        match layout::pull(&a.repo, store, signing) {
            Ok(o) => Some(o),
            Err(e) => return failed(&format!("layout pull from {}: {e}", store.label())),
        }
    } else {
        None
    };
    let w = match wanted(&a.repo) {
        Ok(w) => w,
        Err(e) => return failed(&e),
    };
    let rels: Vec<String> = w.rows.iter().map(|r| object_rel(w.stamp, &r.key)).collect();
    let keys: Vec<CacheKey> = w.rows.iter().map(|r| r.key).collect();
    let results = fetch_all(store, &rels, &keys, signing, usize::from(a.jobs));

    // Fold in row order.
    let (mut missing, mut rejected) = (0usize, 0usize);
    let mut offered: Vec<(CacheKey, Vec<u8>)> = Vec::new();
    for ((result, rel), key) in results.into_iter().zip(&rels).zip(&keys) {
        match result {
            Err(e) => {
                return failed(&format!(
                    "pull from {}: {e}; nothing written",
                    store.label()
                ));
            }
            Ok(Fetched::Missing) => missing += 1,
            Ok(Fetched::Rejected(why)) => {
                rejected += 1;
                if rejected <= MAX_REJECTION_LINES {
                    eprintln!("[cache] rejected {rel}: {why}");
                }
            }
            Ok(Fetched::Payload(p)) => offered.push((*key, p)),
        }
    }
    if rejected > MAX_REJECTION_LINES {
        eprintln!(
            "[cache] rejected {} more (not shown)",
            rejected - MAX_REJECTION_LINES
        );
    }
    let fetched = offered.len();
    let opts = ImportOptions::default().with_verify(sample.engine());
    let summary = match import_entries(&a.repo, offered, &opts) {
        Ok(s) => s,
        Err(e) => return failed(&e),
    };
    let rejected = rejected + summary.rejected;
    let files = w.rows.len() + w.local_hits;
    eprintln!(
        "[cache] pull store={} repo={} files={files} local_hits={} fetched={fetched} missing={missing} rejected={rejected} verified={} signed={}",
        store.label(),
        w.repo_label,
        w.local_hits,
        summary.verified,
        yes_no(signed)
    );
    if a.json {
        let mut out = json!({
            "store": store.label(),
            "repo": w.repo_label,
            "stamp": w.stamp,
            "files": files,
            "local_hits": w.local_hits,
            "wanted": w.rows.len(),
            "fetched": fetched,
            "missing": missing,
            "rejected": rejected,
            "verify": sample.json(),
            "signed": signed,
            "import": summary,
        });
        if let Some(l) = &layout {
            out["layout"] = l.json();
        }
        println!("{out}");
    } else {
        if let Some(l) = &layout {
            println!("{}", l.line(&w.repo_label, &store.label()));
        }
        println!(
            "pull {} <- {}: {files} files, {} local, {fetched} fetched, {} imported, {missing} missing, {rejected} rejected, {} verified ({})",
            w.repo_label,
            store.label(),
            w.local_hits,
            summary.accepted,
            summary.verified,
            if signed { "signed" } else { "unsigned" }
        );
    }
    EXIT_OK
}

fn run_gc(a: GcArgs) -> i32 {
    let dir = match store_arg(&a.store) {
        Ok(StoreArg::Dir(d)) => d,
        Ok(StoreArg::Url(_)) => {
            return usage("gc takes a directory store; gc a remote store on the server");
        }
        Err(e) => return usage(&e),
    };
    if !dir.is_dir() {
        return usage(&format!("store {} is not a directory", dir.display()));
    }
    let opts = GcOptions {
        keep_stamps: a.keep_stamps,
        max_bytes: a.max_bytes,
    };
    let s = match gc(&dir, &opts) {
        Ok(s) => s,
        Err(e) => return failed(&format!("gc {}: {e}", dir.display())),
    };
    let label = dir.display().to_string();
    eprintln!(
        "[cache] gc store={label} stamps_kept={} stamps_removed={} objects_removed={} bytes={}->{}",
        s.kept.len(),
        s.removed.len(),
        s.objects_removed,
        s.bytes_before,
        s.bytes_after
    );
    if a.json {
        let out = json!({
            "store": label,
            "keep_stamps": opts.keep_stamps,
            "max_bytes": opts.max_bytes,
            "kept": s.kept,
            "removed": s.removed,
            "objects_removed": s.objects_removed,
            "tmp_removed": s.tmp_removed,
            "bytes_before": s.bytes_before,
            "bytes_after": s.bytes_after,
        });
        println!("{out}");
    } else {
        println!(
            "gc {label}: kept {} stamps, removed {} stamps and {} objects ({} stale staging files); {} -> {} bytes",
            s.kept.len(),
            s.removed.len(),
            s.objects_removed,
            s.tmp_removed,
            s.bytes_before,
            s.bytes_after
        );
    }
    EXIT_OK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_args_classify_without_the_network() {
        assert_eq!(
            store_arg("/srv/cache"),
            Ok(StoreArg::Dir(PathBuf::from("/srv/cache")))
        );
        assert_eq!(
            store_arg("rel/dir"),
            Ok(StoreArg::Dir(PathBuf::from("rel/dir")))
        );
        for url in ["https://example.com/x", "HTTPS://h", "http://127.0.0.1:9/x"] {
            assert!(matches!(store_arg(url), Ok(StoreArg::Url(_))), "{url}");
        }
        let err = store_arg("http://h/x").expect_err("plain http");
        assert!(
            err.starts_with("refusing plain http:// to non-loopback host h:"),
            "{err}"
        );
        let err = store_arg("s3://key:secret@bucket/x").expect_err("s3");
        assert!(err.starts_with("unsupported store s3://"), "{err}");
        assert!(!err.contains("secret"), "{err}");
    }

    #[test]
    fn sample_and_byte_flags_parse() {
        assert_eq!(parse_sample("all"), Ok(Sample::All));
        assert_eq!(parse_sample("12"), Ok(Sample::Count(12)));
        assert!(parse_sample("-1").is_err());
        assert_eq!(parse_bytes("1000"), Ok(1000));
        assert_eq!(parse_bytes("2K"), Ok(2048));
        assert_eq!(parse_bytes("512m"), Ok(512 << 20));
        assert_eq!(parse_bytes("1G"), Ok(1 << 30));
        assert!(parse_bytes("G").is_err());
        assert!(parse_bytes("99999999999999G").is_err());
    }

    /// The fetch outcome is indexed by row, at any thread count.
    #[test]
    fn parallel_fetch_is_order_stable() {
        let dir = std::env::temp_dir().join(format!("glia-ce2c-fetch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = DirStore::new(&dir);
        let k = Signing::from_hex(&"42".repeat(32)).expect("key");
        let keys: Vec<CacheKey> = (0u8..23).map(|b| CacheKey::from_bytes([b; 32])).collect();
        let rels: Vec<String> = keys
            .iter()
            .map(|key| object_rel("0.0.0+ptest", key))
            .collect();
        for (i, (key, rel)) in keys.iter().zip(&rels).enumerate() {
            match i % 3 {
                0 => store
                    .put(rel, &encode(key, &[i as u8; 5], &k))
                    .expect("put"),
                1 => store
                    .put(rel, &encode(key, &[i as u8; 5], &Signing::unsigned()))
                    .expect("put"),
                _ => {}
            }
        }
        let shape = |jobs| -> Vec<String> {
            fetch_all(&store, &rels, &keys, &k, jobs)
                .into_iter()
                .map(|r| match r {
                    Ok(Fetched::Missing) => "missing".to_string(),
                    Ok(Fetched::Rejected(why)) => format!("rejected {why}"),
                    Ok(Fetched::Payload(p)) => format!("payload {p:?}"),
                    Err(e) => format!("error {e}"),
                })
                .collect()
        };
        let one = shape(1);
        assert_eq!(one[0], format!("payload {:?}", [0u8; 5]));
        assert_eq!(one[1], format!("rejected {}", Reject::Unsigned));
        assert_eq!(one[2], "missing");
        for jobs in [2, 4, 8, 64] {
            assert_eq!(shape(jobs), one, "jobs={jobs}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
