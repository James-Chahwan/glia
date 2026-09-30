//! `glia docs sync|push` — the doc-source sync step (Confluence over the
//! network, or a local wiki checkout); the snapshot it writes feeds the
//! offline, byte-identical build.

use std::path::Path;

use clap::{Subcommand, ValueEnum};

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    #[command(subcommand)]
    action: DocsCmd,
}

/// Where `glia docs sync` reads pages from.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum SyncSource {
    /// A Confluence space over REST (`--space`, credentials).
    Confluence,
    /// A local directory of Markdown pages: a GitHub / GitLab wiki checkout
    /// (`--path`, optional `--container` / `--url-base`).
    Dir,
}

#[derive(Subcommand, Debug)]
enum DocsCmd {
    /// Pull external doc pages into `<repo>/.glia/docs-snapshot`: every page
    /// in a Confluence space (the default), or with `--source dir` every
    /// Markdown page of a local wiki checkout. Then `glia build <repo>`
    /// ingests it (DOC_SPACE + DOC_SECTION + doc→code DOCUMENTS links).
    /// Confluence credentials resolve flag → env → `./.env`
    /// (CONFLUENCE_SITE / CONFLUENCE_EMAIL / CONFLUENCE_TOKEN).
    Sync {
        /// Repo whose snapshot to write.
        repo: String,
        /// Where the pages come from.
        #[arg(long, value_enum, default_value_t = SyncSource::Confluence)]
        source: SyncSource,
        /// Confluence space key (e.g. `MFS`). Required with `--source confluence`.
        #[arg(long)]
        space: Option<String>,
        /// `--source dir`: the wiki checkout (e.g. a `git clone` of
        /// `<repo>.wiki.git`). Its `*.md` / `*.markdown` pages are read, never
        /// its dot-entries, `_Sidebar.md`-style files or symlinks.
        #[arg(long, value_name = "DIR")]
        path: Option<String>,
        /// `--source dir`: the container the pages are filed under
        /// (`docspace::wiki::<container>`). Defaults to the directory's name.
        #[arg(long, value_name = "NAME")]
        container: Option<String>,
        /// `--source dir`: the wiki's web root; a page's url becomes
        /// `<url-base>/<path under the wiki, spaces as ->`. Without it the url
        /// is the page's `file://` path.
        #[arg(long, value_name = "URL")]
        url_base: Option<String>,
        /// Keep only pages whose title matches. Repeatable; `*` wildcard;
        /// case-insensitive substring when the pattern has no `*`. Applied
        /// locally after the read (Confluence has no title-glob parameter),
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
        DocsCmd::Sync {
            repo,
            source,
            space,
            path,
            container,
            url_base,
            include,
            exclude,
            site,
            email,
            token,
        } => {
            let filter = glia_doc_sources::TitleFilter::new(&include, &exclude);
            match source {
                SyncSource::Confluence => {
                    let dir_only = [
                        ("--path", path.is_some()),
                        ("--container", container.is_some()),
                        ("--url-base", url_base.is_some()),
                    ];
                    if let Some((flag, _)) = dir_only.iter().find(|(_, set)| *set) {
                        eprintln!("error: {flag} applies only to --source dir");
                        return 2;
                    }
                    let Some(space) = space else {
                        eprintln!(
                            "error: --space <SPACE> is required with --source confluence (the default); \
                             a local wiki checkout syncs with --source dir --path <DIR>"
                        );
                        return 2;
                    };
                    let cfg = match Config::resolve(site, email, token) {
                        Ok(c) => c,
                        Err(e) => {
                            eprintln!("error: {e}");
                            return 2;
                        }
                    };
                    sync_confluence(&repo, &space, &cfg, &filter, include.len(), exclude.len())
                }
                SyncSource::Dir => {
                    let confluence_only = [
                        ("--space", space.is_some()),
                        ("--site", site.is_some()),
                        ("--email", email.is_some()),
                        ("--token", token.is_some()),
                    ];
                    if let Some((flag, _)) = confluence_only.iter().find(|(_, set)| *set) {
                        eprintln!("error: {flag} applies only to --source confluence");
                        return 2;
                    }
                    let Some(path) = path else {
                        eprintln!("error: --path <DIR> is required with --source dir");
                        return 2;
                    };
                    sync_dir(&repo, &path, container, url_base.as_deref(), &filter)
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

/// `--source confluence`: pull `space`, filter, redact, merge into the snapshot.
fn sync_confluence(
    repo: &str,
    space: &str,
    cfg: &glia_doc_sources::confluence_rest::Config,
    filter: &glia_doc_sources::TitleFilter,
    include: usize,
    exclude: usize,
) -> i32 {
    let pages = match glia_doc_sources::confluence_rest::pull_space(cfg, space) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: pulling space {space}: {e}");
            return 1;
        }
    };
    let fetched = pages.len();
    let pages: Vec<_> = pages.into_iter().filter(|p| filter.keep(&p.title)).collect();
    eprintln!(
        "[docs] sync space={space} fetched={fetched} kept={} include={include} exclude={exclude}",
        pages.len()
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
    let source = glia_doc_sources::SnapshotSource {
        kind: glia_code_domain::DocSourceKind::Confluence,
        container: space.to_string(),
    };
    match write_pages(repo, &source, &pages) {
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

/// `--source dir` (CE.4b): read a local wiki checkout's Markdown pages, filter,
/// redact, and merge them into the snapshot as one `wiki` container.
fn sync_dir(
    repo: &str,
    path: &str,
    container: Option<String>,
    url_base: Option<&str>,
    filter: &glia_doc_sources::TitleFilter,
) -> i32 {
    use glia_doc_sources::wikidir;
    let dir = Path::new(path);
    let container = match container {
        Some(c) => match wikidir::check_container(&c) {
            Ok(()) => c,
            Err(e) => {
                eprintln!("error: --container: {e}");
                return 2;
            }
        },
        None => match wikidir::default_container(dir) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("error: --path: {e}");
                return 2;
            }
        },
    };
    let (pages, stats) = match wikidir::read_wiki_dir(dir, &container, url_base) {
        Ok(read) => read,
        Err(e) => {
            eprintln!("error: reading wiki dir {path}: {e}");
            return 1;
        }
    };
    let pages: Vec<_> = pages.into_iter().filter(|p| filter.keep(&p.title)).collect();
    eprintln!(
        "[docs] sync source=dir path={path} container={container} files={} pages={} kept={} skipped={}",
        stats.files,
        stats.pages,
        pages.len(),
        stats.skipped()
    );
    if pages.is_empty() {
        if stats.pages > 0 {
            eprintln!(
                "error: every one of {} page(s) was filtered out; refusing to overwrite the snapshot with an empty manifest",
                stats.pages
            );
        } else {
            eprintln!(
                "error: no Markdown pages under {path} ({} entries examined, {} skipped); nothing to sync",
                stats.files,
                stats.skipped()
            );
        }
        return 1;
    }
    let source = glia_doc_sources::SnapshotSource {
        kind: glia_code_domain::DocSourceKind::Wiki,
        container: container.clone(),
    };
    match write_pages(repo, &source, &pages) {
        Ok(w) => {
            println!(
                "synced {} page(s) from wiki dir {path} (container {container}) → {}",
                w.written,
                w.path.display()
            );
            println!("run `glia build {repo}` to ingest.");
            0
        }
        Err(e) => {
            eprintln!("error: writing snapshot: {e}");
            1
        }
    }
}

/// Every page through `record_from_page` (the A13.7 redaction), then one
/// `write_snapshot` merge for `source`.
fn write_pages(
    repo: &str,
    source: &glia_doc_sources::SnapshotSource,
    pages: &[glia_doc_sources::Page],
) -> Result<glia_doc_sources::SnapshotWrite, String> {
    let mut redacted = 0usize;
    let records: Vec<_> = pages
        .iter()
        .map(|p| {
            let (record, n) = glia_doc_sources::record_from_page(p);
            redacted += n;
            record
        })
        .collect();
    glia_doc_sources::write_snapshot(Path::new(repo), source, &records, redacted)
}
