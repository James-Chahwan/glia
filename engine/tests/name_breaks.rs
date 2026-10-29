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

// ---- FH.7c: precision ---------------------------------------------------------

/// A one-repo Express server and fetch client: `files` at BASE, committed,
/// then `changed` written over them in the working tree.
fn express_repo(files: &[(&str, &str)], changed: &[(&str, &str)]) -> GitRepo {
    let repo = GitRepo::init();
    repo.write(
        "server/package.json",
        "{\"name\":\"server\",\"dependencies\":{\"express\":\"^4.19.0\",\"date-fns\":\"^3.6.0\"}}\n",
    );
    repo.write(
        "web/package.json",
        "{\"name\":\"web\",\"dependencies\":{\"date-fns\":\"^3.6.0\"}}\n",
    );
    for (path, text) in files {
        repo.write(path, text);
    }
    repo.commit("base");
    for (path, text) in changed {
        repo.write(path, text);
    }
    repo
}

const ORDERS_ROUTE: &str = r#"import { Router } from 'express';

export const orders = Router();

orders.get('/api/orders/:id', (req, res) => {
  const shipLabel = `L-${req.params.id}`;
  res.json({ id: req.params.id, shipLabel, carrier: 'post' });
});

function legacyAudit(id: string): void {
  console.log(id);
}
"#;

const ORDERS_CLIENT: &str = r#"export async function loadOrder(id: string) {
  const res = await fetch(`/api/orders/${id}`);
  const body = await res.json();
  legacyAudit(body.id);
  return body.shipLabel;
}

function legacyAudit(id: string): string {
  return id;
}
"#;

#[test]
fn a_bare_identifier_counts_only_with_a_rename_pair() {
    // `shipLabel` (a shorthand wire key, a bare identifier) is renamed: it
    // counts. `legacyAudit` (a bare identifier the change deletes, paired
    // with nothing) is no name, though the client spells it too.
    let repo = express_repo(
        &[
            ("server/src/routes/orders.ts", ORDERS_ROUTE),
            ("web/src/orders.ts", ORDERS_CLIENT),
        ],
        &[(
            "server/src/routes/orders.ts",
            &ORDERS_ROUTE.replace("shipLabel", "deliveryLabel").replace(
                "\nfunction legacyAudit(id: string): void {\n  console.log(id);\n}\n",
                "",
            ),
        )],
    );
    let b = names(&repo, &NameBreakArgs::default());
    let d = dump(&b);
    let changed: Vec<(&str, Option<&str>)> = b
        .names
        .iter()
        .map(|n| (n.name.as_str(), n.renamed_to.as_deref()))
        .collect();
    assert_eq!(changed, vec![("shipLabel", Some("deliveryLabel"))], "{d}");
    assert_eq!(
        sites(&b),
        vec![site("shipLabel", "web/src/orders.ts", 5)],
        "{d}"
    );
    assert!(b.unpaired_identifiers >= 1, "{d}");
}

const STAMP_ROUTE: &str = r#"import { Router } from 'express';
import { formatDistance } from 'date-fns';

export const stamps = Router();

stamps.get('/api/stamps/:id', (req, res) => {
  res.json({ id: req.params.id, age: formatDistance(new Date(), new Date()) });
});
"#;

const STAMP_CLIENT: &str = r#"import { formatDistance } from 'date-fns';

export async function loadStamp(id: string) {
  const res = await fetch(`/api/stamps/${id}`);
  const body = await res.json();
  return `${body.age} ${formatDistance(new Date(), new Date())}`;
}
"#;

#[test]
fn a_name_bound_to_an_external_import_is_never_a_name() {
    // The server swaps one date-fns function for another: a rename pair of
    // two identifiers, both bound to an external package. The client's own
    // date-fns call is no drift.
    let repo = express_repo(
        &[
            ("server/src/routes/stamps.ts", STAMP_ROUTE),
            ("web/src/stamps.ts", STAMP_CLIENT),
        ],
        &[(
            "server/src/routes/stamps.ts",
            &STAMP_ROUTE.replace("formatDistance", "formatRelative"),
        )],
    );
    let b = names(&repo, &NameBreakArgs::default());
    let d = dump(&b);
    assert!(b.names.is_empty(), "{d}");
    assert!(b.rows.is_empty(), "{d}");
    assert!(b.dropped_library >= 1, "{d}");
}

const SAMPLE_ROUTE: &str = r#"import { Router } from 'express';
import { sampleOrder } from '../fixtures/sample';

export const samples = Router();

samples.get('/api/samples/:id', (req, res) => {
  res.json(sampleOrder);
});
"#;

const SAMPLE_FIXTURE: &str = r#"export const sampleOrder = {
  id: 'o-1',
  legacyCode: 'abc',
};
"#;

const SAMPLE_CLIENT: &str = r#"export async function loadSample(id: string) {
  const res = await fetch(`/api/samples/${id}`);
  const body = await res.json();
  return body.legacyCode;
}
"#;

#[test]
fn a_name_only_a_test_file_removes_is_never_a_name() {
    // The route serves a fixture module (a `fixtures/` path: test code by the
    // walk's classification); a key it renames is no client drift.
    let repo = express_repo(
        &[
            ("server/src/routes/samples.ts", SAMPLE_ROUTE),
            ("server/src/fixtures/sample.ts", SAMPLE_FIXTURE),
            ("web/src/samples.ts", SAMPLE_CLIENT),
        ],
        &[(
            "server/src/fixtures/sample.ts",
            &SAMPLE_FIXTURE.replace("legacyCode", "sampleCode"),
        )],
    );
    let b = names(&repo, &NameBreakArgs::default());
    let d = dump(&b);
    assert!(b.names.is_empty(), "{d}");
    assert!(b.rows.is_empty(), "{d}");
    assert!(b.dropped_test >= 1, "{d}");
}

const ITEMS_ROUTE: &str = r#"import { Router } from 'express';

export const items = Router();

items.get('/api/items/:id', (req, res) => {
  res.json({ id: req.params.id });
});
"#;

const ITEM_SCHEMA: &str = r#"import { z } from 'zod';

export const ItemSchema = z.object({
  id: z.string(),
  itemLabel: z.string(),
});
"#;

const ITEMS_SDK: &str = r#"/**
 * Items API
 * DO NOT MODIFY - This file has been generated using oazapfts.
 */
export type ItemDto = {
    id: string;
    itemLabel: string;
};
export type ItemList = {
    items: ItemDto[];
};
export async function getItem(id: string): Promise<ItemDto> {
    const res = await fetch(`/api/items/${id}`);
    return res.json() as Promise<ItemDto>;
}
"#;

const ITEMS_VIEW: &str = r#"import { getItem } from '../../../sdk/src/fetch-client';

export async function showItem(id: string): Promise<string> {
  const item = await getItem(id);
  return item.itemLabel;
}
"#;

#[test]
fn a_generated_sdk_between_client_and_route_is_walked_through() {
    // The server's change sits in a schema no route reaches by an edge; the
    // regenerated SDK's type is what the route's generated client returns.
    // The SDK is a transparent hop: its type's rename is the exchange's
    // name, the SDK file is no client, and the app code calling it is.
    let repo = express_repo(
        &[
            ("server/src/routes/items.ts", ITEMS_ROUTE),
            ("server/src/dtos/item.ts", ITEM_SCHEMA),
            ("sdk/src/fetch-client.ts", ITEMS_SDK),
            ("web/src/lib/item-view.ts", ITEMS_VIEW),
        ],
        &[
            (
                "server/src/dtos/item.ts",
                &ITEM_SCHEMA.replace("itemLabel", "displayLabel"),
            ),
            (
                "sdk/src/fetch-client.ts",
                &ITEMS_SDK.replace("itemLabel", "displayLabel"),
            ),
        ],
    );
    let b = names(&repo, &NameBreakArgs::default());
    let d = dump(&b);
    assert!(b.exchanges >= 1, "{d}");
    assert!(b.sdk_hops >= 1, "{d}");
    let changed: Vec<(&str, Option<&str>)> = b
        .names
        .iter()
        .map(|n| (n.name.as_str(), n.renamed_to.as_deref()))
        .collect();
    assert_eq!(changed, vec![("itemLabel", Some("displayLabel"))], "{d}");
    assert_eq!(
        sites(&b),
        vec![site("itemLabel", "web/src/lib/item-view.ts", 5)],
        "{d}"
    );
}

#[test]
fn a_changed_exchange_with_no_removed_name_still_counts() {
    // A pure addition on the server side: the exchange changed, no name was
    // removed, no row.
    let repo = express_repo(
        &[
            ("server/src/routes/orders.ts", ORDERS_ROUTE),
            ("web/src/orders.ts", ORDERS_CLIENT),
        ],
        &[(
            "server/src/routes/orders.ts",
            &ORDERS_ROUTE.replace("carrier: 'post' ", "carrier: 'post', weight: 2 "),
        )],
    );
    let b = names(&repo, &NameBreakArgs::default());
    let d = dump(&b);
    assert_eq!(b.exchanges, 1, "{d}");
    assert!(b.names.is_empty() && b.rows.is_empty(), "{d}");
}

#[test]
fn a_generated_sdk_function_renamed_is_a_name_for_its_callers() {
    // The regenerated SDK renames the function the app calls, while the
    // server gains a local of the old name: the SDK is the client's
    // interface, so the name left it, and the app's call still names it.
    let sdk = ITEMS_SDK.replace(
        "export async function getItem(id: string)",
        "export async function fetchItem(id: string)",
    );
    let route = ITEMS_ROUTE.replace(
        "  res.json({ id: req.params.id });",
        "  const getItem = { id: req.params.id };\n  res.json(getItem);",
    );
    let repo = express_repo(
        &[
            ("server/src/routes/items.ts", ITEMS_ROUTE),
            ("sdk/src/fetch-client.ts", ITEMS_SDK),
            ("web/src/lib/item-view.ts", ITEMS_VIEW),
        ],
        &[
            ("server/src/routes/items.ts", &route),
            ("sdk/src/fetch-client.ts", &sdk),
        ],
    );
    let b = names(&repo, &NameBreakArgs::default());
    let d = dump(&b);
    let changed: Vec<(&str, Option<&str>)> = b
        .names
        .iter()
        .map(|n| (n.name.as_str(), n.renamed_to.as_deref()))
        .collect();
    assert_eq!(changed, vec![("getItem", Some("fetchItem"))], "{d}");
    assert_eq!(
        sites(&b),
        vec![
            site("getItem", "web/src/lib/item-view.ts", 1),
            site("getItem", "web/src/lib/item-view.ts", 4),
        ],
        "{d}"
    );
}

const ORDERS_TEST: &str = r#"import { loadOrder } from '../orders';

export async function checkOrder(): Promise<boolean> {
  const order = await loadOrder('o-1');
  const res = await fetch('/api/orders/o-2');
  const body = await res.json();
  return order === body.shipLabel;
}
"#;

#[test]
fn a_name_only_test_files_use_and_a_test_client_are_never_names() {
    // The app client reads nothing renamed; a test file beside it does,
    // and a test's own fetch of the route is no client at all.
    let client = ORDERS_CLIENT.replace("  return body.shipLabel;\n", "  return body.id;\n");
    let repo = express_repo(
        &[
            ("server/src/routes/orders.ts", ORDERS_ROUTE),
            ("web/src/orders.ts", &client),
            ("web/src/__tests__/orders.ts", ORDERS_TEST),
        ],
        &[(
            "server/src/routes/orders.ts",
            &ORDERS_ROUTE.replace("shipLabel", "deliveryLabel"),
        )],
    );
    let b = names(&repo, &NameBreakArgs::default());
    let d = dump(&b);
    assert!(b.names.is_empty(), "{d}");
    assert!(b.rows.is_empty(), "{d}");
    assert_eq!(b.dropped_test, 1, "{d}");
}

// ---- FH.7d: generic and over-cap names fold ---------------------------------

const TICKET_SERVER: &str = r#"package main

import (
	"net/http"

	"github.com/gin-gonic/gin"
)

type Ticket struct {
	ID    string `json:"id"`
	Owner string `json:"owner"`
	Ref   string `json:"ticketRef"`
}

func getTicket(c *gin.Context) {
	c.JSON(http.StatusOK, Ticket{ID: "t-1", Owner: "ops", Ref: "r-1"})
}

func main() {
	r := gin.Default()
	r.GET("/api/tickets/:ref", getTicket)
	_ = r.Run(":8080")
}
"#;

/// The exchange's client function: its field reads are anchored sites.
const TICKET_CLIENT: &str = r#"import axios from 'axios';

export async function loadTicket(ref: string) {
  const res = await axios.get(`/api/tickets/${ref}`);
  return { key: res.data.id, ref: res.data.ticketRef };
}
"#;

/// A file importing the client: a key site, then two free-text matches.
const TICKET_ROWS: &str = r#"import { loadTicket } from './ticket';

export async function rows(ref: string) {
  const t = await loadTicket(ref);
  const row = { id: t.key };
  return [row];
}

export function pick(items: { key: string }[]): string {
  const first = items[0].id;
  const id = first;
  return id;
}
"#;

/// A file importing the client and reading the non-generic name.
const TICKET_VIEW: &str = r#"import { loadTicket } from '../ticket';

export async function view(ref: string): Promise<string> {
  const t = await loadTicket(ref);
  return String(t.ticketRef);
}
"#;

/// The ticket repo at BASE, `views` view files under web/views/, then the
/// server's `id` -> `ticketId` and `ticketRef` -> `ticketKey` renames in
/// the working tree.
fn ticket_repo(views: usize) -> GitRepo {
    let repo = GitRepo::init();
    repo.write(
        "go.mod",
        "module example.com/tickets\n\ngo 1.22\n\nrequire github.com/gin-gonic/gin v1.9.1\n",
    );
    repo.write("main.go", TICKET_SERVER);
    repo.write(
        "package.json",
        "{\"name\":\"tickets\",\"dependencies\":{\"axios\":\"^1.6.0\"}}\n",
    );
    repo.write("web/ticket.ts", TICKET_CLIENT);
    repo.write("web/rows.ts", TICKET_ROWS);
    for i in 0..views {
        repo.write(&format!("web/views/v{i:02}.ts"), TICKET_VIEW);
    }
    repo.commit("base");
    repo.write(
        "main.go",
        &TICKET_SERVER
            .replace("`json:\"id\"`", "`json:\"ticketId\"`")
            .replace("`json:\"ticketRef\"`", "`json:\"ticketKey\"`"),
    );
    repo
}

/// `(name, file, line, folded reason)` of every row, in answer order.
fn folds(b: &NameBreaks) -> Vec<(String, String, i64, Option<&'static str>)> {
    b.rows
        .iter()
        .map(|r| (r.name.clone(), r.file.clone(), r.line, r.folded))
        .collect()
}

#[test]
fn a_generic_rename_counts_key_class_sites_and_folds_the_rest() {
    // `id` is generic: its read in the exchange's client function and the
    // object-literal key in rows.ts stay sites; the property read and the
    // local's two lines in rows.ts' `pick` (free text) fold into one row
    // with a count.
    let repo = ticket_repo(0);
    let b = names(&repo, &NameBreakArgs::default());
    let d = dump(&b);
    let changed: Vec<(&str, Option<&str>)> = b
        .names
        .iter()
        .map(|n| (n.name.as_str(), n.renamed_to.as_deref()))
        .collect();
    assert!(changed.contains(&("id", Some("ticketId"))), "{d}");
    assert!(changed.contains(&("ticketRef", Some("ticketKey"))), "{d}");
    let s =
        |n: &str, f: &str, l: i64, r: Option<&'static str>| (n.to_string(), f.to_string(), l, r);
    assert_eq!(
        folds(&b),
        vec![
            s("id", "web/rows.ts", 5, None),
            s("id", "web/rows.ts", 10, Some("generic")),
            s("id", "web/ticket.ts", 5, None),
            s("ticketRef", "web/ticket.ts", 5, None),
        ],
        "{d}"
    );
    let agg = &b.rows[1];
    assert_eq!(agg.position, "folded", "{d}");
    assert_eq!(agg.client_sites, Some(3), "{d}");
    assert_eq!(agg.client_files, Some(1), "{d}");
    assert_eq!(agg.sample_files, vec!["web/rows.ts".to_string()], "{d}");
    assert_eq!(agg.renamed_to.as_deref(), Some("ticketId"), "{d}");
    assert!(
        b.rows[0].client_sites.is_none() && b.rows[0].sample_files.is_empty(),
        "{d}"
    );
    assert_eq!((b.folded_generic, b.folded_cap), (1, 0), "{d}");
    // The aggregate's serialised form carries its counts; a site row none.
    let json = serde_json::to_value(&b.rows).expect("json");
    assert_eq!(json[1]["client_sites"], 3, "{json}");
    assert_eq!(json[1]["folded"], "generic", "{json}");
    assert!(json[0].get("client_sites").is_none(), "{json}");
    assert!(json[0].get("folded").is_none(), "{json}");
}

#[test]
fn a_name_matched_in_more_files_than_the_cap_folds_but_its_anchored_site_stays() {
    // `ticketRef` is no generic name, but 26 view files read it as well as
    // the client function: 27 files is over the cap (25 files), so the views fold
    // into one row; the client function's read is the exchange's and stays.
    let repo = ticket_repo(26);
    let b = names(&repo, &NameBreakArgs::default());
    let d = dump(&b);
    let refs: Vec<&glia_engine::contract_breaks::NameBreak> =
        b.rows.iter().filter(|r| r.name == "ticketRef").collect();
    assert_eq!(refs.len(), 2, "{d}");
    assert_eq!(
        (refs[0].file.as_str(), refs[0].line, refs[0].folded),
        ("web/ticket.ts", 5, None),
        "{d}"
    );
    let agg = refs[1];
    assert_eq!(agg.folded, Some("cap"), "{d}");
    assert_eq!(agg.client_sites, Some(26), "{d}");
    assert_eq!(agg.client_files, Some(26), "{d}");
    assert_eq!(
        agg.sample_files,
        vec![
            "web/views/v00.ts".to_string(),
            "web/views/v01.ts".to_string(),
            "web/views/v02.ts".to_string()
        ],
        "{d}"
    );
    assert_eq!(
        (agg.file.as_str(), agg.line),
        ("web/views/v00.ts", 5),
        "{d}"
    );
    assert_eq!(b.folded_cap, 1, "{d}");

    // At the cap (24 views + the client: 25 files) every site is a row.
    let repo = ticket_repo(24);
    let b = names(&repo, &NameBreakArgs::default());
    let d = dump(&b);
    let refs = b.rows.iter().filter(|r| r.name == "ticketRef").count();
    assert_eq!(refs, 25, "{d}");
    assert!(b.rows.iter().all(|r| r.folded != Some("cap")), "{d}");
    assert_eq!(b.folded_cap, 0, "{d}");
}
