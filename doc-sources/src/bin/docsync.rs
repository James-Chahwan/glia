//! `docsync` — turn **already-fetched** Confluence pages (local storage-format
//! files) into a doc snapshot, offline. This is the network-free sibling of
//! `glia docs sync`: same conversion + snapshot format (`snapshot` module), but
//! the bodies come from disk instead of REST. Useful for fixtures/eval and for
//! re-ingesting a captured space without hitting the network.
//!
//! Usage:
//!   docsync <repo_root> <pages.jsonl>
//!
//! Each line of `pages.jsonl` describes one page:
//!   {"space":"DEV","title":"Ordering Service","url":"https://…/pages/123",
//!    "version":"3","body_file":"ordering.xhtml"}
//! `body_file` is resolved relative to the `pages.jsonl` file.

use glia_doc_sources::{Page, record_from_page, write_snapshot};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
struct PageSpec {
    space: String,
    title: String,
    url: String,
    version: String,
    /// Path (relative to pages.jsonl) to the page's storage-format XHTML body.
    body_file: String,
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: docsync <repo_root> <pages.jsonl>");
        return std::process::ExitCode::FAILURE;
    }
    let repo_root = PathBuf::from(&args[1]);
    let pages_path = PathBuf::from(&args[2]);
    let pages_dir = pages_path.parent().unwrap_or(Path::new("."));

    let pages_src = match std::fs::read_to_string(&pages_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {}: {e}", pages_path.display());
            return std::process::ExitCode::FAILURE;
        }
    };

    let mut records = Vec::new();
    for (i, line) in pages_src.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let spec: PageSpec = match serde_json::from_str(line) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("error: pages.jsonl line {}: {e}", i + 1);
                return std::process::ExitCode::FAILURE;
            }
        };
        let body_path = pages_dir.join(&spec.body_file);
        let storage = match std::fs::read_to_string(&body_path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error: cannot read body {}: {e}", body_path.display());
                return std::process::ExitCode::FAILURE;
            }
        };
        records.push(record_from_page(&Page {
            space: spec.space,
            title: spec.title,
            url: spec.url,
            version: spec.version,
            storage,
        }));
    }

    match write_snapshot(&repo_root, &records) {
        Ok(manifest) => {
            eprintln!("docsync: wrote {} record(s) → {}", records.len(), manifest.display());
            std::process::ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
