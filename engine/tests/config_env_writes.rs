//! FD.75 acceptance: an env subscript or property that is assigned or
//! deleted is no config read, and the generic Vault `.read(` row reads a
//! `vault/<path>` secret only in a file that names a Vault client.
//!
//! The files are the substrate-gap fixture `config-env-writes-vault-gate`'s,
//! written into a tempdir and built with `generate_one`, so the edge audit's
//! E1 / E2 rows (FD.3c) are gated in-tree before the wheel is rebuilt. Env
//! reads re-home to the function holding them (LE.4b); secrets stay on the
//! module.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use glia_code_domain::{edge_category, node_kind};
use glia_core::NodeId;
use glia_engine::generate_one;

const FILES: &[(&str, &str)] = &[
    (
        "tests/surface/test_patterns.py",
        "import os\nimport pathlib\nimport tempfile\n\n\n\
         def main():\n\
         \x20   with tempfile.TemporaryDirectory() as tmp:\n\
         \x20       gitconfig = pathlib.Path(tmp) / \"gitconfig\"\n\
         \x20       os.environ[\"GIT_CONFIG_GLOBAL\"] = str(gitconfig)\n\
         \x20       os.environ[\"GIT_CONFIG_NOSYSTEM\"] = \"1\"\n\
         \x20       del os.environ[\"GIT_DIR\"]\n\
         \x20       os.environ.pop(\"GIT_WORK_TREE\", None)\n\
         \x20       os.environ[\"PATH\"] += \":/opt/bin\"\n\
         \x20       if os.environ[\"CI\"] == \"true\":\n\
         \x20           return 1\n\
         \x20   return 0\n",
    ),
    (
        "app/settings.py",
        "import os\nimport hvac\n\n\n\
         def db_url():\n\
         \x20   return os.environ[\"DATABASE_URL\"]\n\n\n\
         def api_token():\n\
         \x20   client = hvac.Client()\n\
         \x20   client.secrets.kv.read_secret_version(path=\"api\", mount_point=\"secret\")\n\
         \x20   return client.read(\"secret/data/billing\")\n",
    ),
    (
        "web/env.js",
        "process.env.NODE_ENV = \"test\";\nexport const port = process.env.PORT;\n",
    ),
    (
        "rb/boot.rb",
        "ENV[\"RACK_ENV\"] = \"test\"\nHOST = ENV[\"HOST\"]\n",
    ),
    (
        "xtests/since_v6.rs",
        "//! Vault.sol moves to contracts/core/ between the two exports.\n\n\
         struct Scratch;\n\n\
         impl Scratch {\n\
         \x20   fn read(&self, rel: &str) -> String {\n\
         \x20       std::fs::read_to_string(rel).unwrap()\n\
         \x20   }\n\n\
         \x20   fn edit(&self) {\n\
         \x20       let orders = self.read(\"svc/orders.py\");\n\
         \x20       let _ = orders;\n\
         \x20   }\n\
         }\n\n\
         fn vault(old: &str) -> String {\n\
         \x20   format!(\"contracts::Vault{old}\")\n\
         }\n",
    ),
    (
        "app/archive.py",
        "# Deposits go to the vault contract; this reader only opens local files.\n\
         def readme(archive):\n\
         \x20   return archive.read(\"docs/readme.md\")\n",
    ),
];

fn write_fixture(dir: &Path) {
    for (rel, text) in FILES {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}

/// Every CONFIG_KEY qname, and every READS_CONFIG edge as `(from, to)` qnames.
fn config_view(repo: &Path) -> (BTreeSet<String>, BTreeSet<(String, String)>) {
    let merged = generate_one(repo.to_str().unwrap()).unwrap().merged;
    let mut qname: HashMap<NodeId, String> = HashMap::new();
    let mut keys = BTreeSet::new();
    for g in &merged.graphs {
        for (id, q) in &g.nav.qname_by_id {
            qname.insert(*id, q.clone());
            if g.nav.kind_by_id.get(id) == Some(&node_kind::CONFIG_KEY) {
                keys.insert(q.clone());
            }
        }
    }
    let edges = merged
        .graphs
        .iter()
        .flat_map(|g| g.edges.iter())
        .chain(merged.cross_edges.iter())
        .filter(|e| e.category == edge_category::READS_CONFIG)
        .filter_map(|e| Some((qname.get(&e.from)?.clone(), qname.get(&e.to)?.clone())))
        .collect();
    (keys, edges)
}

#[test]
fn env_writes_and_clientless_vault_reads_mint_no_config_key() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    write_fixture(&repo);
    let (keys, edges) = config_view(&repo);

    for forbidden in [
        "config:env:GIT_CONFIG_GLOBAL",
        "config:env:GIT_CONFIG_NOSYSTEM",
        "config:env:GIT_DIR",
        "config:env:GIT_WORK_TREE",
        "config:env:NODE_ENV",
        "config:env:RACK_ENV",
        "config:secret:vault/svc/orders.py",
        "config:secret:vault/docs/readme.md",
    ] {
        assert!(!keys.contains(forbidden), "{forbidden} in {keys:?}");
    }

    for (from, to) in [
        ("app::settings::db_url", "config:env:DATABASE_URL"),
        ("tests::surface::test_patterns::main", "config:env:PATH"),
        ("tests::surface::test_patterns::main", "config:env:CI"),
        ("web::env", "config:env:PORT"),
        ("rb::boot", "config:env:HOST"),
        ("app::settings", "config:secret:vault/secret/api"),
        ("app::settings", "config:secret:vault/secret/data/billing"),
    ] {
        assert!(
            edges.contains(&(from.to_string(), to.to_string())),
            "missing {from} -> {to} in {edges:?}"
        );
    }
}
