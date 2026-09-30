//! `glia docs sync|push` — Confluence sync (the network step; the snapshot
//! it writes feeds the offline, byte-identical build).

use std::path::Path;

use clap::Subcommand;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    #[command(subcommand)]
    action: DocsCmd,
}

#[derive(Subcommand, Debug)]
enum DocsCmd {
    /// Pull every page in a Confluence space into `<repo>/.glia/docs-snapshot`.
    /// Then `glia build <repo>` ingests it (DOC_SPACE + DOC_SECTION + doc→code
    /// DOCUMENTS links). Credentials resolve flag → env → `./.env`
    /// (CONFLUENCE_SITE / CONFLUENCE_EMAIL / CONFLUENCE_TOKEN).
    Sync {
        /// Repo whose snapshot to write.
        repo: String,
        /// Confluence space key (e.g. `MFS`).
        #[arg(long)]
        space: String,
        /// Keep only pages whose title matches. Repeatable; `*` wildcard;
        /// case-insensitive substring when the pattern has no `*`. Applied
        /// locally after the fetch (Confluence has no title-glob parameter),
        /// so it scopes what is ingested, not what is downloaded.
        #[arg(long, value_name = "PATTERN")]
        include: Vec<String>,
        /// Drop pages whose title matches. Repeatable; same syntax as
        /// `--include`, and wins over it.
        #[arg(long, value_name = "PATTERN")]
        exclude: Vec<String>,
        #[arg(long)]
        site: Option<String>,
        #[arg(long)]
        email: Option<String>,
        #[arg(long)]
        token: Option<String>,
    },
    /// Push a storage-format (XHTML) page body to Confluence — create a new
    /// page, or update an existing one with `--page-id`. With `--markdown` the
    /// file is converted from markdown first.
    Push {
        /// Confluence space key.
        #[arg(long)]
        space: String,
        /// Page title.
        #[arg(long)]
        title: String,
        /// File containing the Confluence storage-format (XHTML) body.
        #[arg(long)]
        file: String,
        /// Treat --file as markdown and convert it to Confluence storage format
        /// first (headings, fenced code, lists, inline code, links).
        #[arg(long)]
        markdown: bool,
        /// Update this page id instead of creating a new page.
        #[arg(long)]
        page_id: Option<String>,
        #[arg(long)]
        site: Option<String>,
        #[arg(long)]
        email: Option<String>,
        #[arg(long)]
        token: Option<String>,
    },
}

pub(crate) fn run(args: Args) -> i32 {
    cmd_docs(args.action)
}

fn cmd_docs(action: DocsCmd) -> i32 {
    use glia_doc_sources::confluence_rest::{self, Config};
    match action {
        DocsCmd::Sync { repo, space, include, exclude, site, email, token } => {
            let cfg = match Config::resolve(site, email, token) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("error: {e}");
                    return 2;
                }
            };
            let pages = match confluence_rest::pull_space(&cfg, &space) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("error: pulling space {space}: {e}");
                    return 1;
                }
            };
            let filter = glia_doc_sources::TitleFilter::new(&include, &exclude);
            let fetched = pages.len();
            let pages: Vec<_> = pages.into_iter().filter(|p| filter.keep(&p.title)).collect();
            eprintln!(
                "[docs] sync space={space} fetched={fetched} kept={} include={} exclude={}",
                pages.len(),
                include.len(),
                exclude.len()
            );
            if pages.is_empty() && fetched > 0 {
                eprintln!(
                    "error: every one of {fetched} fetched page(s) was filtered out; refusing to overwrite the snapshot with an empty manifest"
                );
                return 1;
            }
            // CE.4a: every page is redacted on its way into the snapshot, and the
            // write merges this space into the manifest (other spaces kept). An
            // empty fetch is refused by write_snapshot rather than deleting the
            // space's records.
            let mut redacted = 0usize;
            let records: Vec<_> = pages
                .iter()
                .map(|p| {
                    let (record, n) = glia_doc_sources::record_from_page(p);
                    redacted += n;
                    record
                })
                .collect();
            let source = glia_doc_sources::SnapshotSource {
                kind: glia_code_domain::DocSourceKind::Confluence,
                container: space.clone(),
            };
            match glia_doc_sources::write_snapshot(Path::new(&repo), &source, &records, redacted) {
                Ok(w) => {
                    println!("synced {} page(s) from space {space} → {}", w.written, w.path.display());
                    println!("run `glia build {repo}` to ingest.");
                    0
                }
                Err(e) => {
                    eprintln!("error: writing snapshot: {e}");
                    1
                }
            }
        }
        DocsCmd::Push { space, title, file, markdown, page_id, site, email, token } => {
            let cfg = match Config::resolve(site, email, token) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("error: {e}");
                    return 2;
                }
            };
            let body = match std::fs::read_to_string(&file) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("error: reading {file}: {e}");
                    return 1;
                }
            };
            // Converted before any network call, so the marker below proves the
            // conversion ran even when the push itself fails on credentials.
            let storage = if markdown {
                let (s, st) =
                    glia_doc_sources::markdown::markdown_to_storage_with_stats(&body);
                eprintln!(
                    "[docs] push markdown→storage: {} md bytes → {} storage bytes (h={} code={} link={} inline={})",
                    body.len(),
                    s.len(),
                    st.headings,
                    st.code_blocks,
                    st.links,
                    st.inline_code
                );
                s
            } else {
                body
            };
            let result = match &page_id {
                Some(id) => confluence_rest::update_page(&cfg, id, &space, &title, &storage),
                None => confluence_rest::create_page(&cfg, &space, &title, &storage),
            };
            match result {
                Ok(p) => {
                    let verb = if page_id.is_some() { "updated" } else { "created" };
                    println!("{verb} page {} (v{}) — {}", p.id, p.version, p.url);
                    0
                }
                Err(e) => {
                    eprintln!("error: pushing page: {e}");
                    1
                }
            }
        }
    }
}
