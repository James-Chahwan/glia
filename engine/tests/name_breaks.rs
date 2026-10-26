//! FH.7a acceptance: style-agnostic drift (`glia_engine::contract_breaks::name_breaks`).
//! The names a change removed or renamed on the server side of a paired
//! exchange, walked across the pairing to every client file, and each client
//! site still using an old name. Each case is a real git repo: the BASE
//! commit (server and clients agree), the BROKEN working tree (the server
//! moved, the clients did not: flagged) and the FIXED one (the authors'
//! client half applied: clear).
//!
//! The two substrate-gap fixtures hold the broken state; their `drift/base/`
//! and `drift/fixed/` files (suffix `.drift`, so no walk parses them) are the
//! other two states:
//! - `drift-rename-go-ts`: a gin handler writes `models.Message`, whose json
//!   tag `messageId` becomes `messageUid`; an axios client and a view still
//!   read `messageId`.
//! - `drift-zod-express-ts`: an Express route parses a Zod object whose key
//!   `userName` becomes `displayName`; a fetch client's caller destructures
//!   `userName`.
//!
//! fired_on: `[name-breaks] base=<rev> exchanges=<e> names=<n> renames=<r>
//! clients=<c> rows=<k> fixed=<f>`, once per call.

use crate::git_fixture;

use std::fs;
use std::path::{Path, PathBuf};

use git_fixture::GitRepo;
use glia_engine::contract_breaks::{NameBreakArgs, NameBreaks, name_breaks};
use glia_engine::{generate_many, generate_one};

fn fixture_dir(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../bench/substrate-gap/fixtures")
        .join(name)
}

/// Every file under `dir`, `/`-separated and relative to it, sorted.
fn files_under(dir: &Path) -> Vec<String> {
    fn walk(root: &Path, at: &Path, out: &mut Vec<String>) {
        let mut entries: Vec<_> = fs::read_dir(at)
            .unwrap_or_else(|e| panic!("read {}: {e}", at.display()))
            .map(|e| e.expect("dir entry").path())
            .collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                walk(root, &p, out);
            } else {
                let rel = p.strip_prefix(root).expect("under root");
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out
}

/// The fixture's broken state under `prefix/` of `repo` (key.json and the
/// `drift/` states left out), restricted to the paths `keep` accepts.
fn write_broken(repo: &GitRepo, name: &str, prefix: &str, keep: &dyn Fn(&str) -> bool) {
    let dir = fixture_dir(name);
    for rel in files_under(&dir) {
        if rel == "key.json" || rel.starts_with("drift/") || !keep(&rel) {
            continue;
        }
        let text = fs::read_to_string(dir.join(&rel)).expect("fixture file");
        repo.write(&format!("{prefix}{rel}"), &text);
    }
}

/// Overlay the fixture's `drift/<state>/` files (the `.drift` suffix dropped).
fn write_state(repo: &GitRepo, name: &str, state: &str, prefix: &str, strip: &str) {
    let dir = fixture_dir(name).join("drift").join(state);
    for rel in files_under(&dir) {
        let text = fs::read_to_string(dir.join(&rel)).expect("drift file");
        let target = rel.strip_suffix(".drift").expect(".drift suffix");
        let target = target.strip_prefix(strip).unwrap_or(target);
        repo.write(&format!("{prefix}{target}"), &text);
    }
}

/// A monorepo of the fixture: BASE committed, the broken state in the
/// working tree.
fn monorepo(name: &str) -> GitRepo {
    let repo = GitRepo::init();
    write_broken(&repo, name, "", &|_| true);
    write_state(&repo, name, "base", "", "");
    repo.commit("base");
    write_broken(&repo, name, "", &|_| true);
    repo
}

fn names(repo: &GitRepo, args: &NameBreakArgs) -> NameBreaks {
    let after = generate_one(repo.path()).unwrap_or_else(|e| panic!("build: {e}"));
    name_breaks("HEAD", repo.path(), &after, args).unwrap_or_else(|e| panic!("name breaks: {e}"))
}

fn dump(b: &NameBreaks) -> String {
    let mut s = format!(
        "exchanges={} clients={} fixed={} blocking={}\n",
        b.exchanges, b.clients, b.fixed, b.blocking
    );
    for n in &b.names {
        s.push_str(&format!(
            "  name {} -> {:?} at {}:{}\n",
            n.name, n.renamed_to, n.file, n.line
        ));
    }
    for r in &b.rows {
        s.push_str(&format!(
            "  row {} [{}] {}:{} {} {} via {} -> {}\n",
            r.name, r.repo, r.file, r.line, r.position, r.tier, r.client, r.server
        ));
    }
    s
}

/// `(name, file, line)` of every row, in answer order.
fn sites(b: &NameBreaks) -> Vec<(String, String, i64)> {
    b.rows
        .iter()
        .map(|r| (r.name.clone(), r.file.clone(), r.line))
        .collect()
}

fn site(name: &str, file: &str, line: i64) -> (String, String, i64) {
    (name.to_string(), file.to_string(), line)
}

#[test]
fn go_json_tag_rename_flags_the_ts_clients_still_reading_it() {
    let repo = monorepo("drift-rename-go-ts");
    let b = names(&repo, &NameBreakArgs::default());
    let d = dump(&b);
    assert_eq!(b.base, "HEAD");
    assert_eq!(b.names.len(), 1, "one changed name:\n{d}");
    let n = &b.names[0];
    assert_eq!(n.name, "messageId", "{d}");
    assert_eq!(n.renamed_to.as_deref(), Some("messageUid"), "{d}");
    assert_eq!(n.file, "api/models/message.go", "{d}");
    assert_eq!(n.line, 4, "{d}");
    assert_eq!(
        sites(&b),
        vec![
            site("messageId", "web/src/models/message.ts", 2),
            site("messageId", "web/src/views/thread.ts", 5),
        ],
        "{d}"
    );
    let positions: Vec<&str> = b.rows.iter().map(|r| r.position).collect();
    assert_eq!(positions, vec!["key", "property"], "{d}");
    // HTTP_CALLS is resolver-paired (derived): never more than heuristic.
    assert!(b.rows.iter().all(|r| r.tier == "heuristic"), "{d}");
    assert!(
        b.rows
            .iter()
            .all(|r| r.server == "GET /api/messages/:id @api"
                && r.client.starts_with("endpoint:GET:/api/messages/")),
        "{d}"
    );
    assert!(b.exchanges >= 1, "{d}");
    assert_eq!(b.fixed, 0, "{d}");
    assert_eq!(b.blocking, 0, "never blocking by default:\n{d}");
    assert_eq!(b.min_tier, "fact");

    let mut heuristic = NameBreakArgs::default();
    heuristic.min_tier = "heuristic";
    let h = names(&repo, &heuristic);
    assert_eq!(
        h.blocking,
        2,
        "blocking under min tier heuristic:\n{}",
        dump(&h)
    );
    let mut derived = NameBreakArgs::default();
    derived.min_tier = "derived";
    assert_eq!(names(&repo, &derived).blocking, 0, "heuristic only");
}

#[test]
fn go_json_tag_rename_is_clear_once_the_clients_follow() {
    let repo = monorepo("drift-rename-go-ts");
    write_state(&repo, "drift-rename-go-ts", "fixed", "", "");
    let b = names(&repo, &NameBreakArgs::default());
    let d = dump(&b);
    assert_eq!(b.names.len(), 1, "the server change still names it:\n{d}");
    assert!(b.rows.is_empty(), "clear at the fixed state:\n{d}");
    assert_eq!(b.fixed, 2, "both client files stopped using it:\n{d}");
}

#[test]
fn zod_key_rename_flags_the_destructuring_client() {
    let repo = monorepo("drift-zod-express-ts");
    let b = names(&repo, &NameBreakArgs::default());
    let d = dump(&b);
    assert_eq!(b.names.len(), 1, "{d}");
    assert_eq!(b.names[0].name, "userName", "{d}");
    assert_eq!(b.names[0].renamed_to.as_deref(), Some("displayName"), "{d}");
    assert_eq!(b.names[0].file, "server/src/schemas/user.ts", "{d}");
    assert_eq!(
        sites(&b),
        vec![
            site("userName", "web/src/pages/profile.ts", 4),
            site("userName", "web/src/pages/profile.ts", 5),
        ],
        "{d}"
    );
    assert!(
        b.rows
            .iter()
            .all(|r| r.renamed_to.as_deref() == Some("displayName")),
        "{d}"
    );
}

#[test]
fn zod_key_rename_is_clear_once_the_client_follows() {
    let repo = monorepo("drift-zod-express-ts");
    write_state(&repo, "drift-zod-express-ts", "fixed", "", "");
    let b = names(&repo, &NameBreakArgs::default());
    let d = dump(&b);
    assert!(b.rows.is_empty(), "{d}");
    assert_eq!(b.fixed, 1, "{d}");
}

#[test]
fn a_client_in_another_repo_is_walked_too() {
    // The provider is the api/ tree alone (its own git repo); the client is
    // the web/ tree, built beside it as a --with repo is.
    let api = GitRepo::init();
    let only_api = |p: &str| p.starts_with("api/");
    write_broken(&api, "drift-rename-go-ts", "", &only_api);
    write_state(&api, "drift-rename-go-ts", "base", "", "");
    api.commit("base");
    write_broken(&api, "drift-rename-go-ts", "", &only_api);
    let web = tempfile::tempdir().expect("temp dir");
    let src = fixture_dir("drift-rename-go-ts").join("web");
    for rel in files_under(&src) {
        let to = web.path().join(&rel);
        fs::create_dir_all(to.parent().expect("parent")).expect("mkdir");
        fs::copy(src.join(&rel), to).expect("copy");
    }
    let web_path = web.path().to_str().expect("utf-8").to_string();
    let after = generate_many(&[api.path().to_string(), web_path]).expect("build");
    let b = name_breaks("HEAD", api.path(), &after, &NameBreakArgs::default()).expect("names");
    let d = dump(&b);
    assert_eq!(
        sites(&b),
        vec![
            site("messageId", "src/models/message.ts", 2),
            site("messageId", "src/views/thread.ts", 5),
        ],
        "{d}"
    );
    let web_label = b.rows[0].repo.clone();
    assert!(
        b.rows.iter().all(|r| r.repo == web_label),
        "one client repo:\n{d}"
    );
    assert_ne!(web_label, "", "{d}");
}

const SERVER_MAIN: &str = r#"package main

import (
	"net/http"

	"github.com/gin-gonic/gin"
)

type Ticket struct {
	Ref   string `json:"ref"`
	Kind  string `json:"type"`
	Owner string `json:"owner"`
}

func getTicket(c *gin.Context) {
	c.JSON(http.StatusOK, Ticket{Ref: "t-1", Kind: "bug", Owner: "ops"})
}

func main() {
	r := gin.Default()
	r.GET("/api/tickets/:ref", getTicket)
	_ = r.Run(":8080")
}
"#;

const CLIENT: &str = r#"import axios from 'axios';

export async function loadTicket(ref: string) {
  const res = await axios.get(`/api/tickets/${ref}`);
  const type = 'ticket';
  return { kind: res.data.type, owner: res.data.owner, label: type };
}
"#;

#[test]
fn a_generic_name_needs_a_property_access_or_a_rename() {
    // `type` is generic: dropped outright (no rename pair), only the
    // property read `res.data.type` is a site, never the local `type`.
    // `owner` is not generic: renamed to `assignee`, and its property read
    // is a site.
    let repo = GitRepo::init();
    repo.write(
        "go.mod",
        "module example.com/tickets\n\ngo 1.22\n\nrequire github.com/gin-gonic/gin v1.9.1\n",
    );
    repo.write("main.go", SERVER_MAIN);
    repo.write(
        "package.json",
        "{\"name\":\"tickets\",\"dependencies\":{\"axios\":\"^1.6.0\"}}\n",
    );
    repo.write("web/ticket.ts", CLIENT);
    repo.commit("base");
    repo.write(
        "main.go",
        &SERVER_MAIN
            .replace("\tKind  string `json:\"type\"`\n", "")
            .replace("Kind: \"bug\", ", "")
            .replace("`json:\"owner\"`", "`json:\"assignee\"`"),
    );
    let b = names(&repo, &NameBreakArgs::default());
    let d = dump(&b);
    let changed: Vec<(&str, Option<&str>)> = b
        .names
        .iter()
        .map(|n| (n.name.as_str(), n.renamed_to.as_deref()))
        .collect();
    assert!(changed.contains(&("owner", Some("assignee"))), "{d}");
    assert!(changed.iter().any(|(n, _)| *n == "type"), "{d}");
    assert_eq!(
        sites(&b),
        vec![
            site("owner", "web/ticket.ts", 6),
            site("type", "web/ticket.ts", 6),
        ],
        "{d}"
    );
}

#[test]
fn no_change_and_an_unpaired_change_name_nothing() {
    let repo = monorepo("drift-rename-go-ts");
    repo.commit("broken");
    let b = names(&repo, &NameBreakArgs::default());
    let d = dump(&b);
    assert!(b.names.is_empty() && b.rows.is_empty(), "{d}");
    assert_eq!(b.exchanges, 0, "{d}");

    // A change in a file no exchange's server side reaches names nothing.
    repo.write(
        "api/tools/seed.go",
        "package tools\n\ntype Seed struct {\n\tLegacyField string `json:\"legacyField\"`\n}\n",
    );
    repo.commit("seed");
    repo.write(
        "api/tools/seed.go",
        "package tools\n\ntype Seed struct {\n\tFreshField string `json:\"freshField\"`\n}\n",
    );
    let b = names(&repo, &NameBreakArgs::default());
    assert!(b.names.is_empty() && b.rows.is_empty(), "{}", dump(&b));
    repo.remove("api/tools/seed.go");
    let b = names(&repo, &NameBreakArgs::default());
    assert!(b.names.is_empty() && b.rows.is_empty(), "{}", dump(&b));
}

#[test]
fn a_server_model_moved_with_its_rename_is_still_read() {
    // `git mv` of the renamed model: the diff is a rename plus the one
    // changed line, and the moved file is still the handler's.
    let repo = monorepo("drift-rename-go-ts");
    repo.git_mv("api/models/message.go", "api/models/mail.go");
    let b = names(&repo, &NameBreakArgs::default());
    let d = dump(&b);
    assert_eq!(b.names.len(), 1, "{d}");
    assert_eq!(b.names[0].file, "api/models/mail.go", "{d}");
    assert_eq!(b.names[0].renamed_to.as_deref(), Some("messageUid"), "{d}");
    assert_eq!(b.rows.len(), 2, "{d}");
}

#[test]
fn an_unknown_rev_is_an_error() {
    let repo = monorepo("drift-rename-go-ts");
    let root = repo.root().to_str().expect("utf-8");
    let after = generate_one(root).expect("build");
    let err = name_breaks(
        "no-such-rev",
        repo.path(),
        &after,
        &NameBreakArgs::default(),
    )
    .expect_err("unknown rev");
    assert!(err.contains("no-such-rev"), "{err}");
}
