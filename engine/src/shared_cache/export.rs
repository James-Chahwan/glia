//! What a checkout's parse cache can offer a shared store (CE.2a): the
//! content-addressed rows of its walk ([`cache_rows`]) and the payloads of the
//! sidecar entries still valid for them ([`export_entries`]). The CLI's
//! transport (CE.2c) moves the `(key, payload)` pairs; nothing here does I/O
//! beyond the walk and the sidecar read.

use std::path::Path;

use glia_code_domain::walk_gating::repo_identity;

use super::key::{CacheKey, file_key};
use crate::cache::{self, CACHE_VERSION, ParseCache};
use crate::route::{cache_plan, cached_under_other_form};

/// One language-parser file of a repo's walk and its content address.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[non_exhaustive]
pub struct CacheRow {
    /// Repo-relative path, as walked.
    pub path: String,
    /// The language tag the router parses it as.
    pub lang: &'static str,
    /// The MODULE qname the build's LB.9b plan gives it now.
    pub module_qname: String,
    /// Its content address ([`file_key`]).
    pub key: CacheKey,
    /// The local parse cache's xxhash64 of its content
    /// (`glia_engine::cache::content_hash`), the hash a cache entry records.
    pub content_hash: u64,
}

/// Every [`CacheRow`] of one repo, in path order.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[non_exhaustive]
pub struct CacheRows {
    /// The build stamp every key hashes (`glia_engine::BUILD_STAMP`).
    pub stamp: &'static str,
    /// The repo's label (its directory name, `arch::repo_label_for`).
    pub repo_label: String,
    pub rows: Vec<CacheRow>,
}

/// One sidecar entry a store may take: its key and its payload, the entry's
/// own bytes (`bincode` of `{ content_hash, lang, parse }`, the parse in
/// canonical order, LC.11).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[non_exhaustive]
pub struct ExportedEntry {
    pub path: String,
    pub key: CacheKey,
    pub payload: Vec<u8>,
}

/// A repo's exportable sidecar entries ([`export_entries`]), in path order.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[non_exhaustive]
pub struct Export {
    /// The build stamp every key hashes.
    pub stamp: &'static str,
    /// The repo's label (its directory name).
    pub repo_label: String,
    pub entries: Vec<ExportedEntry>,
    /// Rows with no valid entry: never cached, cached from other content or
    /// under the other MODULE form, or every row when the sidecar was written
    /// under another repo identity or go.mod module set.
    pub stale: usize,
}

/// A repo's rows plus the build context their keys hash.
struct Planned {
    rows: CacheRows,
    repo_key: String,
    go_ctx: String,
}

/// Walk `repo_path` exactly as a build does and key every language-parser
/// file: the identity of `repo_identity`, the go.mod set of `go_modules_for`
/// and the MODULE form of the router's own plan (`route::cache_plan`).
fn plan(repo_path: &str) -> Result<Planned, String> {
    let root = Path::new(repo_path);
    if !root.is_dir() {
        return Err(format!("not a directory: {repo_path}"));
    }
    let ident = repo_identity(root);
    let (files, _regions, _md, roots) = crate::walk::walk_source_files(root);
    let go_ctx = crate::build::go_modules_for(root, &roots, repo_path).context_key();
    let mut rows: Vec<CacheRow> = cache_plan(&files)
        .into_iter()
        .map(|r| CacheRow {
            key: file_key(
                CACHE_VERSION,
                &ident.key,
                r.lang,
                r.path,
                &r.module_qname,
                &go_ctx,
                r.source.as_bytes(),
            ),
            content_hash: cache::content_hash(r.source),
            path: r.path.to_string(),
            lang: r.lang,
            module_qname: r.module_qname,
        })
        .collect();
    rows.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Planned {
        rows: CacheRows {
            stamp: CACHE_VERSION,
            repo_label: crate::arch::repo_label_for(repo_path),
            rows,
        },
        repo_key: ident.key,
        go_ctx,
    })
}

/// The content-addressed rows of `repo_path`'s checkout as it is on disk now:
/// one per file a build hands a language parser, in path order. Walks the repo
/// and plans its MODULE forms; parses nothing and reads no cache.
pub fn cache_rows(repo_path: &str) -> Result<CacheRows, String> {
    plan(repo_path).map(|p| p.rows)
}

/// The entries of `repo_path`'s parse-cache sidecar
/// (`<repo>/.glia/graph/parse_cache.bin`) that a build of the checkout as it
/// is now would reuse, each under its row's key: cached from the file's
/// current content as its language, under the MODULE form the plan picks now.
/// A sidecar written under another build stamp loads empty, and one written
/// under another repo identity or go.mod module set exports nothing (a build
/// would discard it): every row is then `stale`.
///
/// fired_on marker, once per call:
///   `[cache] export repo=<label> entries=<n> stale=<s>`
pub fn export_entries(repo_path: &str) -> Result<Export, String> {
    let Planned {
        rows,
        repo_key,
        go_ctx,
    } = plan(repo_path)?;
    let CacheRows {
        stamp,
        repo_label,
        rows,
    } = rows;
    let cache = ParseCache::load(repo_path);
    let mut entries = Vec::new();
    let mut stale = 0usize;
    if cache.context() == (repo_key.as_str(), go_ctx.as_str()) {
        for row in rows {
            let fresh = cache
                .peek(&row.path, row.content_hash, row.lang)
                .is_some_and(|fp| !cached_under_other_form(fp, &row.path, &row.module_qname));
            let payload = if fresh {
                cache.entry_payload(&row.path, row.content_hash, row.lang)?
            } else {
                None
            };
            match payload {
                Some(payload) => entries.push(ExportedEntry {
                    path: row.path,
                    key: row.key,
                    payload,
                }),
                None => stale += 1,
            }
        }
    } else {
        stale = rows.len();
    }
    eprintln!(
        "[cache] export repo={repo_label} entries={} stale={stale}",
        entries.len()
    );
    Ok(Export {
        stamp,
        repo_label,
        entries,
        stale,
    })
}
