//! `glia docs sync|push` — the doc-source sync step (Confluence, a MediaWiki,
//! a Notion database or page tree over the network, or a local wiki
//! checkout); the snapshot it writes feeds the offline, byte-identical build.

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
    /// A MediaWiki through its Action API (`--api`, one of `--namespace` /
    /// `--category`, optional `--container` / `--max-pages` / `--token`).
    #[value(name = "mediawiki")]
    MediaWiki,
    /// A Notion database or page tree through the Notion API (`--database`
    /// or `--page`, a token; optional `--max-pages` / `--api`).
    Notion,
}

impl SyncSource {
    fn name(self) -> &'static str {
        match self {
            SyncSource::Confluence => "confluence",
            SyncSource::Dir => "dir",
            SyncSource::MediaWiki => "mediawiki",
            SyncSource::Notion => "notion",
        }
    }
}

#[derive(Subcommand, Debug)]
enum DocsCmd {
    /// Pull external doc pages into `<repo>/.glia/docs-snapshot`: every page
    /// in a Confluence space (the default), with `--source dir` every
    /// Markdown page of a local wiki checkout, with `--source mediawiki`
    /// one namespace or category of a MediaWiki, read through its Action API
    /// (never by following links), or with `--source notion` every page of a
    /// Notion database (`--database`) or a page and its sub-pages (`--page`),
    /// their blocks, nested ones included, converted to Markdown. Then
    /// `glia build <repo>` ingests it (DOC_SPACE + DOC_SECTION + doc→code
    /// DOCUMENTS links). Confluence credentials resolve flag → env → `./.env`
    /// (CONFLUENCE_SITE / CONFLUENCE_EMAIL / CONFLUENCE_TOKEN); a MediaWiki
    /// bearer token flag → env → `./.env` (MEDIAWIKI_TOKEN); a Notion
    /// integration token the same way (NOTION_TOKEN).
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
        /// `--source dir` / `mediawiki`: the container the pages are filed
        /// under (`docspace::wiki::<container>`). Defaults to the directory's
        /// name, or the api host.
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
        /// `--source mediawiki`: the wiki's api.php URL
        /// (`https://wiki.example/w/api.php`); https, or plain http to
        /// loopback only. It is the only URL requested: no page URL is
        /// fetched, no link or redirect followed. `--source notion`: the API
        /// origin, `scheme://host[:port]` with no path (default
        /// `https://api.notion.com`), for a proxy or a loopback test server.
        #[arg(long, value_name = "URL")]
        api: Option<String>,
        /// `--source mediawiki`: pull every page of this namespace id (0 is
        /// the main namespace; redirects left out).
        #[arg(long, value_name = "N", conflicts_with = "category")]
        namespace: Option<u32>,
        /// `--source mediawiki`: pull the pages in this category (`Runbooks`
        /// or `Category:Runbooks`; subcategories are not descended into).
        #[arg(long, value_name = "NAME")]
        category: Option<String>,
        /// `--source mediawiki` / `notion`: stop after this many listed pages
        /// (default 5000).
        #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
        max_pages: Option<u32>,
        /// `--source notion`: the database id (32 hex digits, hyphens
        /// optional, from the database's URL). Every data source of the
        /// database is queried; its pages are filed under
        /// `docspace::notion::<id without hyphens>`. The database must be
        /// shared with the integration.
        #[arg(long, value_name = "ID")]
        database: Option<String>,
        /// `--source notion`, instead of `--database`: a root page id (from
        /// its URL). The page and its sub-pages (`child_page` blocks, up to 5
        /// levels down, at most `--max-pages`) are read, breadth-first, and
        /// filed under `docspace::notion::<root id without hyphens>`. The page
        /// must be shared with the integration.
        #[arg(long, value_name = "ID", conflicts_with = "database")]
        page: Option<String>,
        #[arg(long)]
        site: Option<String>,
        #[arg(long)]
        email: Option<String>,
        /// Confluence API token; with `--source mediawiki` a bearer token
        /// (an OAuth 2 owner-only consumer's access token); with `--source
        /// notion` an internal integration's token. Cookie / password login is
        /// not supported.
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
            api,
            namespace,
            category,
            max_pages,
            database,
            page,
            site,
            email,
            token,
        } => {
            use SyncSource::{Confluence, Dir, MediaWiki, Notion};
            let filter = glia_doc_sources::TitleFilter::new(&include, &exclude);
            // Every source-specific flag, in the order it is reported, with
            // the sources that take it.
            let given: [(&str, bool, &[SyncSource]); 13] = [
                ("--space", space.is_some(), &[Confluence]),
                ("--site", site.is_some(), &[Confluence]),
                ("--email", email.is_some(), &[Confluence]),
                ("--token", token.is_some(), &[Confluence, MediaWiki, Notion]),
                ("--path", path.is_some(), &[Dir]),
                ("--container", container.is_some(), &[Dir, MediaWiki]),
                ("--url-base", url_base.is_some(), &[Dir]),
                ("--api", api.is_some(), &[MediaWiki, Notion]),
                ("--namespace", namespace.is_some(), &[MediaWiki]),
                ("--category", category.is_some(), &[MediaWiki]),
                ("--max-pages", max_pages.is_some(), &[MediaWiki, Notion]),
                ("--database", database.is_some(), &[Notion]),
                ("--page", page.is_some(), &[Notion]),
            ];
            if let Some((flag, _, takes)) =
                given.iter().find(|(_, set, takes)| *set && !takes.contains(&source))
            {
                let takes: Vec<String> =
                    takes.iter().map(|s| format!("--source {}", s.name())).collect();
                eprintln!("error: {flag} applies only to {}", takes.join(" or "));
                return 2;
            }
            match source {
                SyncSource::Confluence => {
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
                    let Some(path) = path else {
                        eprintln!("error: --path <DIR> is required with --source dir");
                        return 2;
                    };
                    sync_dir(&repo, &path, container, url_base.as_deref(), &filter)
                }
                SyncSource::MediaWiki => {
                    use glia_doc_sources::mediawiki::{self, Selection};
                    let Some(api) = api else {
                        eprintln!(
                            "error: --api <URL> is required with --source mediawiki (the wiki's api.php, e.g. https://wiki.example/w/api.php)"
                        );
                        return 2;
                    };
                    let selection = match (namespace, category) {
                        (Some(n), _) => Selection::Namespace(n),
                        (None, Some(c)) => Selection::Category(c),
                        (None, None) => {
                            eprintln!(
                                "error: one of --namespace <N> or --category <NAME> is required with --source mediawiki"
                            );
                            return 2;
                        }
                    };
                    let cfg = match mediawiki::Config::resolve(
                        &api,
                        token,
                        container,
                        glia_engine::RELEASE,
                    ) {
                        Ok(c) => c,
                        Err(e) => {
                            eprintln!("error: {e}");
                            return 2;
                        }
                    };
                    let max_pages = max_pages.map_or(mediawiki::DEFAULT_MAX_PAGES, |n| n as usize);
                    sync_mediawiki(&repo, &cfg, &selection, max_pages, &filter)
                }
                SyncSource::Notion => {
                    use glia_doc_sources::notion;
                    let target = match (database, page) {
                        (Some(id), _) => NotionTarget::Database(id),
                        (None, Some(id)) => NotionTarget::Page(id),
                        (None, None) => {
                            eprintln!(
                                "error: one of --database <ID> or --page <ID> is required with --source notion (a database id, or a root page id, from its URL)"
                            );
                            return 2;
                        }
                    };
                    let container = match &target {
                        NotionTarget::Database(id) => notion::database_container(id),
                        NotionTarget::Page(id) => notion::page_container(id),
                    };
                    let container = match container {
                        Ok(c) => c,
                        Err(e) => {
                            eprintln!("error: {e}");
                            return 2;
                        }
                    };
                    let cfg = match notion::Config::resolve(token, api) {
                        Ok(c) => c,
                        Err(e) => {
                            eprintln!("error: {e}");
                            return 2;
                        }
                    };
                    let max_pages = max_pages.map_or(notion::DEFAULT_MAX_PAGES, |n| n as usize);
                    sync_notion(&repo, &cfg, &target, &container, max_pages, &filter)
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

/// `--source mediawiki` (CE.4d): pull one namespace or category through the
/// Action API, filter, redact, and merge it into the snapshot as one `wiki`
/// container. Prints the fired_on marker
/// `[docs] sync source=mediawiki api=<scheme://host> selection=<ns:N|category:C>
/// fetched=<f> kept=<k> skipped=<s> requests=<r> retries=<t>` (no token, no
/// path), then CE.4c's `[docs] wikitext` line.
fn sync_mediawiki(
    repo: &str,
    cfg: &glia_doc_sources::mediawiki::Config,
    selection: &glia_doc_sources::mediawiki::Selection,
    max_pages: usize,
    filter: &glia_doc_sources::TitleFilter,
) -> i32 {
    let label = selection.label();
    let origin = match cfg.origin() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let (pages, stats) = match glia_doc_sources::mediawiki::pull(cfg, selection, max_pages) {
        Ok(pulled) => pulled,
        Err(e) => {
            eprintln!("error: pulling {label} from {origin}: {e}");
            return 1;
        }
    };
    let pages: Vec<_> = pages.into_iter().filter(|p| filter.keep(&p.title)).collect();
    eprintln!(
        "[docs] sync source=mediawiki api={origin} selection={label} fetched={} kept={} skipped={} requests={} retries={}",
        stats.fetched,
        pages.len(),
        stats.skipped(),
        stats.requests,
        stats.retries
    );
    if let Some(marker) = stats.wikitext_marker() {
        eprintln!("{marker}");
    }
    if stats.truncated {
        eprintln!(
            "[docs] warning: stopped at --max-pages {max_pages}; the rest of {label} was not pulled"
        );
    }
    if pages.is_empty() {
        if stats.pages > 0 {
            eprintln!(
                "error: every one of {} page(s) was filtered out; refusing to overwrite the snapshot with an empty manifest",
                stats.pages
            );
        } else {
            eprintln!(
                "error: no wikitext pages in {label} ({} listed, {} skipped); nothing to sync",
                stats.fetched,
                stats.skipped()
            );
        }
        return 1;
    }
    let source = glia_doc_sources::SnapshotSource {
        kind: glia_code_domain::DocSourceKind::Wiki,
        container: cfg.container.clone(),
    };
    match write_pages(repo, &source, &pages) {
        Ok(w) => {
            println!(
                "synced {} page(s) from {origin} {label} (container {}) → {}",
                w.written,
                cfg.container,
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

/// What `--source notion` pulls: a database (`--database`) or a root page
/// and its sub-pages (`--page`), by the id the user gave.
enum NotionTarget {
    Database(String),
    Page(String),
}

/// `--source notion` (CE.4e, CE.4f): pull one database's pages, or one page
/// tree, through the Notion API, filter, redact, and merge them into the
/// snapshot as one `notion` container. Prints the fired_on marker
/// `[docs] sync source=notion database=<container> data_sources=<d>
/// fetched=<f> kept=<k> blocks=<b> unsupported=<u> requests=<r> retries=<t>
/// depth_capped=<c>` (no token; `root=<container>` in place of `database=`
/// for a page tree), then `[docs] notion unsupported <type>=<n> ...` when a
/// block type was skipped.
fn sync_notion(
    repo: &str,
    cfg: &glia_doc_sources::notion::Config,
    target: &NotionTarget,
    container: &str,
    max_pages: usize,
    filter: &glia_doc_sources::TitleFilter,
) -> i32 {
    use glia_doc_sources::notion;
    let (what, pulled) = match target {
        NotionTarget::Database(id) => ("database", notion::pull_database(cfg, id, max_pages)),
        NotionTarget::Page(id) => ("page tree", notion::pull_page_tree(cfg, id, max_pages)),
    };
    let (pages, stats) = match pulled {
        Ok(pulled) => pulled,
        Err(e) => {
            eprintln!("error: pulling Notion {what} {container}: {e}");
            return 1;
        }
    };
    let pages: Vec<_> = pages.into_iter().filter(|p| filter.keep(&p.title)).collect();
    let marker = match target {
        NotionTarget::Database(_) => stats.sync_marker(container, pages.len()),
        NotionTarget::Page(_) => stats.tree_marker(container, pages.len()),
    };
    eprintln!("{marker}");
    if let Some(marker) = stats.unsupported_marker() {
        eprintln!("{marker}");
    }
    if stats.truncated {
        eprintln!(
            "[docs] warning: stopped at --max-pages {max_pages}; the rest of {what} {container} was not pulled"
        );
    }
    if stats.depth_capped > 0 {
        eprintln!(
            "[docs] warning: {} nested block(s) or sub-page(s) lie past the depth cap ({} block levels, {} page levels) and were not read",
            stats.depth_capped,
            notion::MAX_BLOCK_DEPTH,
            notion::MAX_PAGE_DEPTH
        );
    }
    if stats.unreadable > 0 {
        eprintln!(
            "[docs] warning: {} synced block original(s) or sub-page(s) are not shared with the integration and were skipped",
            stats.unreadable
        );
    }
    if pages.is_empty() {
        if stats.pages > 0 {
            eprintln!(
                "error: every one of {} page(s) was filtered out; refusing to overwrite the snapshot with an empty manifest",
                stats.pages
            );
        } else {
            eprintln!(
                "error: no pages in Notion {what} {container} ({} listed, {} archived or trashed); nothing to sync",
                stats.fetched, stats.skipped_archived
            );
        }
        return 1;
    }
    let source = glia_doc_sources::SnapshotSource {
        kind: glia_code_domain::DocSourceKind::Notion,
        container: container.to_string(),
    };
    match write_pages(repo, &source, &pages) {
        Ok(w) => {
            println!(
                "synced {} page(s) from Notion {what} {container} → {}",
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
