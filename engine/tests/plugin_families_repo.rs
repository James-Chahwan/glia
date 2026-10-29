//! The plugin-family discovery seam (lua-support): `for_repo` reads a
//! repo's declared `.glia/plugin_families.toml` when one exists, else infers
//! the family from the tree (a seed manifest beside Lua sources). Both paths
//! are checked here against real directories, so the build's family input is
//! exercised end to end without any global state.

use std::path::Path;

#[test]
fn a_declared_file_names_the_families() {
    let dir = tempdir();
    std::fs::create_dir_all(dir.path().join(".glia")).unwrap();
    std::fs::write(
        dir.path().join(".glia/plugin_families.toml"),
        glia_code_domain::plugin_families::declaration_template(),
    )
    .unwrap();
    let families = glia_code_domain::plugin_families::for_repo(dir.path());
    assert_eq!(
        families.families().len(),
        1,
        "the seed template declares one family"
    );
    assert_eq!(families.families()[0].name, "maki");
    assert_eq!(families.families()[0].manifest, "plugin.toml");
    assert_eq!(
        families.families()[0].lib_roots,
        vec!["lib".to_string()],
        "the template carries the seed's lib root"
    );
    assert!(families.call_for("maki.api.register_tool").is_some());
}

#[test]
fn an_undeclared_repo_infers_from_the_tree() {
    let dir = tempdir();
    // A plugin directory laid out like the seed's architecture: a manifest
    // beside Lua sources, and a lib root the dotted requires walk.
    std::fs::create_dir_all(dir.path().join("plugins/demo")).unwrap();
    std::fs::create_dir_all(dir.path().join("plugins/lib/host")).unwrap();
    std::fs::write(
        dir.path().join("plugins/demo/plugin.toml"),
        "[permissions]\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("plugins/demo/init.lua"), "return 1\n").unwrap();
    std::fs::write(dir.path().join("plugins/lib/host/view.lua"), "return {}\n").unwrap();
    let families = glia_code_domain::plugin_families::for_repo(dir.path());
    assert_eq!(
        families.families().len(),
        1,
        "the seed manifest beside Lua sources infers one family"
    );
    assert_eq!(families.families()[0].name, "maki");
    assert_eq!(
        families.families()[0].lib_roots,
        vec!["lib".to_string()],
        "the lib root is kept only where Lua sources sit under it"
    );
    // No manifest anywhere: no family is invented.
    let empty = tempdir();
    std::fs::write(empty.path().join("main.lua"), "return 1\n").unwrap();
    assert!(glia_code_domain::plugin_families::for_repo(empty.path()).is_empty());
}

/// A unique temporary directory, removed at the end of the test.
fn tempdir() -> TempDir {
    let base = std::env::temp_dir().join(format!("glia-plugin-families-{}", std::process::id()));
    let dir = base.join(format!(
        "{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    TempDir(dir)
}

struct TempDir(std::path::PathBuf);

impl TempDir {
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
