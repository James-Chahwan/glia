//! `glia vocab sync` (FB.13b) — the CLI surface of the identifier vocabulary
//! snapshot step, `glia_snapshots::vocab_sync` (FB.13a): read every tracked
//! text file of one commit's tree (default `HEAD`) and write
//! `<repo>/.glia/vocab-snapshot/` (files.jsonl, vocab.jsonl, then
//! meta.json): every identifier-like token with the files it occurs in.
//!
//! A query-time input: find's absence and gaps read it back
//! (`glia_code_domain::snapshots::read_vocab`); the build never reads or
//! refreshes it. Transport only: the capture, the snapshot format and the
//! `[vocab] sync rev=<12> ...` marker live in the snapshots crate; this
//! surface adds `[vocab] surface=cli rev=<12> files=<f> tokens=<t>` after a
//! written snapshot. Exits 0 on a written snapshot, 1 when the sync fails
//! (nothing written).

use std::path::Path;

use clap::Subcommand;
use glia_code_domain::snapshots::vocab_dir;
use glia_snapshots::{VocabOptions, VocabSummary, vocab_sync};

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    #[command(subcommand)]
    action: VocabCmd,
}

#[derive(Subcommand, Debug)]
enum VocabCmd {
    /// Read one commit's tracked text files and write
    /// `<repo>/.glia/vocab-snapshot/`, replacing any earlier snapshot.
    Sync {
        /// Repo whose tree to read.
        repo: String,
        /// The commit to read (default HEAD).
        #[arg(long, value_name = "REV")]
        rev: Option<String>,
        /// Print the summary as JSON.
        #[arg(long)]
        json: bool,
    },
}

pub(crate) fn run(args: Args) -> i32 {
    match args.action {
        VocabCmd::Sync { repo, rev, json } => sync(&repo, rev, json),
    }
}

/// The surface's own marker line, after the library's `[vocab] sync`.
fn surface_marker(s: &VocabSummary) -> String {
    let rev: String = s.rev.chars().take(12).collect();
    format!(
        "[vocab] surface=cli rev={rev} files={} tokens={}",
        s.files, s.tokens
    )
}

fn sync(repo: &str, rev: Option<String>, json: bool) -> i32 {
    let root = Path::new(repo);
    let opts = match rev {
        Some(r) => VocabOptions::at(r),
        None => VocabOptions::default(),
    };
    let summary = match vocab_sync(root, &opts) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };
    eprintln!("{}", surface_marker(&summary));
    if json {
        match serde_json::to_string_pretty(&summary) {
            Ok(text) => println!("{text}"),
            Err(e) => {
                eprintln!("error: {e}");
                return 1;
            }
        }
    } else {
        let rev12: String = summary.rev.chars().take(12).collect();
        println!(
            "synced {} tokens from {} files at {rev12} -> {}/",
            summary.tokens,
            summary.files,
            vocab_dir(root).display()
        );
    }
    0
}
