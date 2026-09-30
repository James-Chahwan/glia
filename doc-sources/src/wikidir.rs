//! A local wiki checkout as a doc source (CE.4b).
//!
//! GitHub and GitLab wikis are git repositories of Markdown pages that live
//! beside the code repo (`<repo>.wiki.git`), so the repo walk never sees them.
//! [`read_wiki_dir`] reads a clone of one into [`Page`]s of
//! [`DocSourceKind::Wiki`] that `glia docs sync --source dir` hands to
//! [`record_from_page`](crate::record_from_page) and
//! [`write_snapshot`](crate::write_snapshot). No network, no token.
//!
//! The walk is bounded and never leaves the directory: it goes at most
//! [`MAX_DEPTH`] directories deep, in sorted name order, never follows a
//! symlink (a hostile wiki cannot pull `/etc` into the snapshot), skips
//! dot-entries (`.git` included) and `_`-prefixed files (GitHub's `_Sidebar.md`
//! / `_Footer.md` navigation), takes `*.md` / `*.markdown` regular files of at
//! most [`MAX_PAGE_BYTES`], and stops after [`MAX_PAGES`] pages.

use std::path::Path;

use glia_code_domain::DocSourceKind;
use glia_code_domain::snapshots::data_hash;

use crate::snapshot::{Page, PageBody};

/// Directories below the wiki root the walk descends into; a deeper one is
/// skipped and counted in [`DirStats::skipped_deep`].
pub const MAX_DEPTH: usize = 8;
/// Largest page read, in bytes (2 MiB); a larger file is skipped.
pub const MAX_PAGE_BYTES: u64 = 2 * 1024 * 1024;
/// Pages read before the walk stops and sets [`DirStats::truncated`].
pub const MAX_PAGES: usize = 10_000;

/// What one [`read_wiki_dir`] saw. Every directory entry the walk examined,
/// other than a directory it descended into, is in `files` and in exactly one
/// of `pages` or a `skipped_*` count, so `files == pages + skipped()`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DirStats {
    /// Entries examined (a skipped directory, `.git` say, counts once and is
    /// never read).
    pub files: usize,
    /// Markdown pages read.
    pub pages: usize,
    /// Dot-entries: `.git`, `.gitignore`, any hidden file or directory.
    pub skipped_hidden: usize,
    /// `_`-prefixed files: GitHub's `_Sidebar.md` / `_Footer.md` navigation.
    pub skipped_underscore: usize,
    /// Symlinks, never followed.
    pub skipped_symlink: usize,
    /// Regular files that are not `*.md` / `*.markdown` (images, uploads), and
    /// entries that are neither a file nor a directory.
    pub skipped_not_markdown: usize,
    /// Pages larger than [`MAX_PAGE_BYTES`].
    pub skipped_large: usize,
    /// Pages whose bytes, or entries whose names, are not UTF-8.
    pub skipped_non_utf8: usize,
    /// Directories below [`MAX_DEPTH`].
    pub skipped_deep: usize,
    /// Entries whose metadata or contents could not be read.
    pub skipped_unreadable: usize,
    /// The walk stopped at [`MAX_PAGES`]; later entries were not examined.
    pub truncated: bool,
}

impl DirStats {
    /// Every skipped entry, over all reasons.
    pub fn skipped(&self) -> usize {
        self.skipped_hidden
            + self.skipped_underscore
            + self.skipped_symlink
            + self.skipped_not_markdown
            + self.skipped_large
            + self.skipped_non_utf8
            + self.skipped_deep
            + self.skipped_unreadable
    }
}

/// The walk's bounds: [`MAX_DEPTH`] / [`MAX_PAGE_BYTES`] / [`MAX_PAGES`] in
/// every caller; the unit tests shrink them.
struct Limits {
    depth: usize,
    page_bytes: u64,
    pages: usize,
}

const LIMITS: Limits = Limits {
    depth: MAX_DEPTH,
    page_bytes: MAX_PAGE_BYTES,
    pages: MAX_PAGES,
};

/// Refuse a container name that would not survive as one manifest path
/// segment and one qname segment (`wiki/<container>/<page>.md`,
/// `docspace::wiki::<container>`).
pub fn check_container(name: &str) -> Result<(), String> {
    if name.is_empty() || name == "." || name == ".." {
        return Err(format!("container name {name:?} is not a name"));
    }
    if name.contains(['/', '\\']) || name.contains("::") || name.chars().any(char::is_control) {
        return Err(format!(
            "container name {name:?} may not contain `/`, `\\`, `::` or control characters"
        ));
    }
    Ok(())
}

/// The container a wiki checkout gets when none is named: its directory's name
/// (`acme.wiki` for a `git clone …/acme.wiki.git`), read from the canonical
/// path so `.` names the directory it is.
pub fn default_container(dir: &Path) -> Result<String, String> {
    let canon = std::fs::canonicalize(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let name = canon.file_name().and_then(|n| n.to_str()).ok_or_else(|| {
        format!(
            "{} has no UTF-8 directory name; pass --container",
            canon.display()
        )
    })?;
    check_container(name)?;
    Ok(name.to_string())
}

/// Read every Markdown page of the wiki checkout at `dir` as a [`Page`] of
/// [`DocSourceKind::Wiki`] in `container`, in walk order (sorted names, depth
/// first), with what the walk saw.
///
/// Per page: `rel` is its path under `dir` without the extension, `/`-joined
/// (`guides/Setup`); the title is the file stem with `-` and `_` read as
/// spaces (`Order-Flow` -> `Order Flow`); `slug_hint` is `rel`, so two pages
/// with one title in different directories get their own manifest stems
/// (`guides-setup` beside `setup`); the version is the content hash
/// ([`data_hash`]) of its bytes, so an unchanged page keeps its record; the
/// url is `<url_base>/<rel, spaces as `-`>` when a base is given (a GitLab
/// wiki's `https://gitlab.com/o/r/-/wikis`), else `file://<canonical path>`.
///
/// Errors when `container` is not a valid name or `dir` is not a readable
/// directory; a single unreadable entry below it is skipped and counted.
pub fn read_wiki_dir(
    dir: &Path,
    container: &str,
    url_base: Option<&str>,
) -> Result<(Vec<Page>, DirStats), String> {
    read_with(dir, container, url_base, &LIMITS)
}

fn read_with(
    dir: &Path,
    container: &str,
    url_base: Option<&str>,
    limits: &Limits,
) -> Result<(Vec<Page>, DirStats), String> {
    check_container(container)?;
    let root = std::fs::canonicalize(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if !root.is_dir() {
        return Err(format!("{} is not a directory", dir.display()));
    }
    let entries = sorted_entries(&root).map_err(|e| format!("read {}: {e}", root.display()))?;
    let mut walk = Walk {
        container,
        url_base: url_base.map(|b| b.trim_end_matches('/')),
        limits,
        pages: Vec::new(),
        stats: DirStats::default(),
    };
    let mut rel = Vec::new();
    walk.dir(&root, entries, &mut rel, 0);
    if walk.stats.truncated {
        eprintln!(
            "[docs] warning: {} holds more than {} pages; synced the first {} in name order, the rest are left out",
            dir.display(),
            limits.pages,
            walk.pages.len()
        );
    }
    Ok((walk.pages, walk.stats))
}

/// A directory's entries, sorted by name (bytes), so the walk order and with
/// it the page order are the same on every filesystem.
fn sorted_entries(dir: &Path) -> std::io::Result<Vec<std::io::Result<std::fs::DirEntry>>> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect();
    entries.sort_by(|a, b| match (a, b) {
        (Ok(a), Ok(b)) => a.file_name().cmp(&b.file_name()),
        (Ok(_), Err(_)) => std::cmp::Ordering::Less,
        (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
        (Err(_), Err(_)) => std::cmp::Ordering::Equal,
    });
    Ok(entries)
}

struct Walk<'a> {
    container: &'a str,
    url_base: Option<&'a str>,
    limits: &'a Limits,
    pages: Vec<Page>,
    stats: DirStats,
}

impl Walk<'_> {
    /// Walk one directory's `entries`; `rel` holds its name segments below the
    /// root and `depth` their count. Stops early once `truncated` is set.
    fn dir(
        &mut self,
        path: &Path,
        entries: Vec<std::io::Result<std::fs::DirEntry>>,
        rel: &mut Vec<String>,
        depth: usize,
    ) {
        for entry in entries {
            if self.stats.truncated {
                return;
            }
            let Ok(entry) = entry else {
                self.skip(|s| &mut s.skipped_unreadable);
                continue;
            };
            let Ok(name) = entry.file_name().into_string() else {
                self.skip(|s| &mut s.skipped_non_utf8);
                continue;
            };
            if name.starts_with('.') {
                self.skip(|s| &mut s.skipped_hidden);
                continue;
            }
            let child = path.join(&name);
            // symlink_metadata: a symlink is reported as itself, never followed.
            let Ok(meta) = std::fs::symlink_metadata(&child) else {
                self.skip(|s| &mut s.skipped_unreadable);
                continue;
            };
            let kind = meta.file_type();
            if kind.is_symlink() {
                self.skip(|s| &mut s.skipped_symlink);
            } else if kind.is_dir() {
                self.subdir(&child, name, rel, depth);
            } else if kind.is_file() {
                self.file(&child, &name, meta.len(), rel);
            } else {
                self.skip(|s| &mut s.skipped_not_markdown);
            }
        }
    }

    fn subdir(&mut self, child: &Path, name: String, rel: &mut Vec<String>, depth: usize) {
        if depth + 1 > self.limits.depth {
            self.skip(|s| &mut s.skipped_deep);
            return;
        }
        let Ok(entries) = sorted_entries(child) else {
            self.skip(|s| &mut s.skipped_unreadable);
            return;
        };
        rel.push(name);
        self.dir(child, entries, rel, depth + 1);
        rel.pop();
    }

    fn file(&mut self, child: &Path, name: &str, len: u64, rel: &[String]) {
        if name.starts_with('_') {
            self.skip(|s| &mut s.skipped_underscore);
            return;
        }
        let Some(stem) = markdown_stem(name) else {
            self.skip(|s| &mut s.skipped_not_markdown);
            return;
        };
        if len > self.limits.page_bytes {
            self.skip(|s| &mut s.skipped_large);
            return;
        }
        if self.pages.len() >= self.limits.pages {
            // Not counted: the walk stops here, before examining it.
            self.stats.truncated = true;
            return;
        }
        let Ok(bytes) = std::fs::read(child) else {
            self.skip(|s| &mut s.skipped_unreadable);
            return;
        };
        let version = data_hash(&[&bytes]);
        let Ok(text) = String::from_utf8(bytes) else {
            self.skip(|s| &mut s.skipped_non_utf8);
            return;
        };
        let rel_path = rel
            .iter()
            .map(String::as_str)
            .chain([stem])
            .collect::<Vec<_>>()
            .join("/");
        let url = match self.url_base {
            Some(base) => format!("{base}/{}", rel_path.replace(' ', "-")),
            None => format!("file://{}", child.display()),
        };
        self.stats.files += 1;
        self.stats.pages += 1;
        self.pages.push(Page {
            kind: DocSourceKind::Wiki,
            container: self.container.to_string(),
            title: stem.replace(['-', '_'], " "),
            url,
            version,
            body: PageBody::Markdown(text),
            slug_hint: Some(rel_path),
        });
    }

    fn skip(&mut self, reason: impl FnOnce(&mut DirStats) -> &mut usize) {
        self.stats.files += 1;
        *reason(&mut self.stats) += 1;
    }
}

/// The stem of a `*.md` / `*.markdown` file name (extension matched without
/// case), or `None` for any other name. A bare `.md` has no stem and is a
/// dot-entry anyway.
fn markdown_stem(name: &str) -> Option<&str> {
    let (stem, ext) = name.rsplit_once('.')?;
    let is_md = ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown");
    (is_md && !stem.is_empty()).then_some(stem)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(test: &str) -> Scratch {
            let dir =
                std::env::temp_dir().join(format!("glia-ce4b-unit-{}-{test}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create scratch dir");
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn stops_at_the_page_cap() {
        let s = Scratch::new("cap");
        for name in ["a.md", "b.md", "c.md", "d.md"] {
            std::fs::write(s.0.join(name), "x\n").expect("write page");
        }
        let limits = Limits {
            depth: MAX_DEPTH,
            page_bytes: MAX_PAGE_BYTES,
            pages: 2,
        };
        let (pages, stats) = read_with(&s.0, "w", None, &limits).expect("read");
        let titles: Vec<&str> = pages.iter().map(|p| p.title.as_str()).collect();
        assert_eq!(titles, ["a", "b"]);
        assert!(stats.truncated);
        assert_eq!((stats.files, stats.pages, stats.skipped()), (2, 2, 0));
    }

    #[test]
    fn exactly_the_cap_is_not_truncated() {
        let s = Scratch::new("cap-exact");
        for name in ["a.md", "b.md", "notes.txt"] {
            std::fs::write(s.0.join(name), "x\n").expect("write file");
        }
        let limits = Limits {
            depth: MAX_DEPTH,
            page_bytes: MAX_PAGE_BYTES,
            pages: 2,
        };
        let (pages, stats) = read_with(&s.0, "w", None, &limits).expect("read");
        assert_eq!(pages.len(), 2);
        assert!(!stats.truncated);
        assert_eq!((stats.files, stats.skipped_not_markdown), (3, 1));
    }

    #[test]
    fn markdown_stems() {
        assert_eq!(markdown_stem("Home.md"), Some("Home"));
        assert_eq!(markdown_stem("Order-Flow.markdown"), Some("Order-Flow"));
        assert_eq!(markdown_stem("README.MD"), Some("README"));
        assert_eq!(markdown_stem("v1.2-notes.md"), Some("v1.2-notes"));
        assert_eq!(markdown_stem("logo.png"), None);
        assert_eq!(markdown_stem("Makefile"), None);
        assert_eq!(markdown_stem(".md"), None);
    }

    #[test]
    fn container_names() {
        assert!(check_container("acme-wiki").is_ok());
        assert!(check_container("acme.wiki").is_ok());
        for bad in ["", ".", "..", "a/b", "a\\b", "a::b", "a\nb"] {
            assert!(check_container(bad).is_err(), "{bad:?} accepted");
        }
    }
}
