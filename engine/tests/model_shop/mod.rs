//! The `model-breaks-shop` fixture (engine/tests/data/model-breaks-shop) as a
//! git work tree, shared by the inferred-model break tests (DT.1o's
//! model_breaks, DT.1p's contract-breaks / review wiring). It uses
//! `crate::git_fixture::GitRepo`, so a test crate that declares
//! `mod model_shop;` also declares `mod git_fixture;`.
//!
//! The fixture is examples-glia trimmed to its order exchanges and the admin
//! GraphQL documents: an Angular `OrderService` (GET / POST / PATCH typed
//! `Order`, a `NewOrder` body, an inline status body), a gin API whose
//! handlers write `models.Order` and bind `PlaceOrderRequest` /
//! `StatusChange`, and an Apollo admin console selecting the SDL `Product`.

use std::path::{Path, PathBuf};

use crate::git_fixture::GitRepo;

/// The committed fixture directory.
pub fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/model-breaks-shop")
}

/// Every file under `dir`, relative to `root`, sorted.
fn files_under(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .map(|e| e.expect("fixture dir entry").path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            files_under(root, &p, out);
        } else {
            let rel = p.strip_prefix(root).expect("under the fixture");
            out.push(rel.to_str().expect("utf-8 fixture path").to_string());
        }
    }
}

/// A fresh repo holding the fixture as one commit, `v1`.
pub fn v1() -> GitRepo {
    let repo = GitRepo::init();
    let root = fixture();
    let mut files = Vec::new();
    files_under(&root, &root, &mut files);
    files.sort();
    for rel in &files {
        let text = std::fs::read_to_string(root.join(rel))
            .unwrap_or_else(|e| panic!("read fixture {rel}: {e}"));
        repo.write(rel, &text);
    }
    repo.commit("v1");
    repo
}

/// `v1`, then `v2`: the first `json:"customerName"` of
/// api/internal/models/order.go (Order.CustomerName) becomes
/// `json:"customer_name"` (the examples-glia bug: the client never receives
/// the name).
pub fn renamed() -> GitRepo {
    let repo = v1();
    edit(
        &repo,
        "api/internal/models/order.go",
        "`json:\"customerName\"`",
        "`json:\"customer_name\"`",
    );
    repo.commit("v2");
    repo
}

/// One exact replacement of `from` by `to` in the repo-relative `rel`, in
/// the work tree (not staged). Panics when `from` is absent.
pub fn edit(repo: &GitRepo, rel: &str, from: &str, to: &str) {
    let path = repo.root().join(rel);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {rel}: {e}"));
    assert!(text.contains(from), "{rel} holds no {from:?}");
    std::fs::write(&path, text.replacen(from, to, 1))
        .unwrap_or_else(|e| panic!("write {rel}: {e}"));
}
