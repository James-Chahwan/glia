//! FA.21: each code MODULE that imports an external library carries one
//! IMPORT_BINDINGS cell, the rows `{"spec", "name", "alias", "line"}` of
//! those imports, so a cross-graph pass, a joint build and a layout merge can
//! all route an import by its bound names. An in-repo import (relative,
//! tsconfig-alias resolved, a `crate::` path, the repo's own package) is not
//! listed, the cell sits on the file's MODULE only, right after its IMPORTS
//! cell, and a file importing no library has none.

use std::path::Path;

use glia_code_domain::{cell_type, node_kind};
use glia_core::CellPayload;
use glia_engine::{GenerateResult, ParseCache, generate_one, generate_one_with_cache};

fn write(dir: &Path, rel: &str, body: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

/// TS (named + aliased, namespace, default, relative, tsconfig alias), Go
/// (aliased third-party package), Python (an external `from` import, a
/// relative one and the repo's own package) and Rust (an external `use` and a
/// `crate::` path).
fn write_fixture(dir: &Path) {
    write(
        dir,
        "tsconfig.json",
        r#"{"compilerOptions":{"baseUrl":".","paths":{"@core/*":["web/core/*"]}}}"#,
    );
    write(
        dir,
        "web/app.ts",
        "import { a as b } from '@org/lib';\n\
         import * as ns from '@org/lib';\n\
         import Def from 'zod';\n\
         import { local } from './local';\n\
         import { aliased } from '@core/thing';\n\
         \n\
         export function run() {\n  return [b(), ns.x, Def, local, aliased];\n}\n",
    );
    write(dir, "web/local.ts", "export const local = 1;\n");
    write(dir, "web/core/thing.ts", "export const aliased = 2;\n");
    write(
        dir,
        "svc/server.go",
        "package svc\n\nimport lib \"example.com/org/lib/pkg\"\n\nfunc Serve() {\n\tlib.Run()\n}\n",
    );
    write(dir, "tools/__init__.py", "");
    write(
        dir,
        "tools/job.py",
        "from org_lib.util import fetch_rows\nfrom .other import thing\nfrom tools.other import thing as t2\n\n\ndef run():\n    return fetch_rows(thing, t2)\n",
    );
    write(dir, "tools/other.py", "thing = 1\n");
    write(
        dir,
        "src/lib.rs",
        "mod util;\n\nuse org_lib::util::load_rows;\nuse crate::util::inner;\n\npub fn run() -> u32 {\n    load_rows() + inner()\n}\n",
    );
    write(dir, "src/util.rs", "pub fn inner() -> u32 {\n    1\n}\n");
}

/// (qname, kind is MODULE, every cell kind in order, the IMPORT_BINDINGS
/// payloads) of every node, sorted by qname.
type Row = (String, bool, Vec<u32>, Vec<String>);

fn rows(r: &GenerateResult) -> Vec<Row> {
    let mut out = Vec::new();
    for g in &r.merged.graphs {
        for n in &g.nodes {
            let qname = g.nav.qname_by_id.get(&n.id).cloned().unwrap_or_default();
            let module = g.nav.kind_by_id.get(&n.id) == Some(&node_kind::MODULE);
            let kinds = n.cells.iter().map(|c| c.kind.0).collect();
            let bindings = n
                .cells
                .iter()
                .filter(|c| c.kind == cell_type::IMPORT_BINDINGS)
                .map(|c| match &c.payload {
                    CellPayload::Json(s) | CellPayload::Text(s) => s.clone(),
                    CellPayload::Bytes(_) => String::from("<bytes>"),
                })
                .collect();
            out.push((qname, module, kinds, bindings));
        }
    }
    out.sort();
    out
}

/// The one IMPORT_BINDINGS payload of the MODULE named `qname`, after
/// checking the cell sits right after the MODULE's IMPORTS cell.
fn module_cell(rows: &[Row], qname: &str) -> Option<String> {
    let (_, _, kinds, cells) = rows
        .iter()
        .find(|(q, m, _, _)| q == qname && *m)
        .unwrap_or_else(|| panic!("no MODULE {qname}: {rows:#?}"));
    assert!(cells.len() <= 1, "{qname}: two cells {cells:?}");
    if let Some(at) = kinds
        .iter()
        .position(|k| *k == cell_type::IMPORT_BINDINGS.0)
    {
        assert!(
            at > 0 && kinds[at - 1] == cell_type::IMPORTS.0,
            "{qname}: IMPORT_BINDINGS not right after IMPORTS: {kinds:?}"
        );
    }
    cells.first().cloned()
}

#[test]
fn each_module_records_its_external_imports_bound_names() {
    let tmp = tempfile::tempdir().unwrap();
    write_fixture(tmp.path());
    let r = generate_one(tmp.path().to_str().unwrap()).unwrap();
    let rows = rows(&r);

    assert_eq!(
        module_cell(&rows, "web::app").as_deref(),
        Some(
            r#"[{"spec":"@org/lib","name":null,"alias":"ns","line":1},{"spec":"@org/lib","name":"a","alias":"b","line":0},{"spec":"zod","name":"default","alias":"Def","line":2}]"#
        ),
        "TS: named, namespace and default imports; the relative and tsconfig-alias imports are in-repo"
    );
    assert_eq!(
        module_cell(&rows, "svc::server").as_deref(),
        Some(r#"[{"spec":"example.com::org::lib::pkg","name":null,"alias":"lib","line":2}]"#),
        "Go: the aliased package import, its path as the Go parser stores it (`::`-joined)"
    );
    assert_eq!(
        module_cell(&rows, "tools::job").as_deref(),
        Some(r#"[{"spec":"org_lib.util","name":"fetch_rows","alias":null,"line":0}]"#),
        "Python: the external `from` import; relative and own-package imports are in-repo"
    );
    assert_eq!(
        module_cell(&rows, "src::lib").as_deref(),
        Some(r#"[{"spec":"org_lib::util","name":"load_rows","alias":null,"line":2}]"#),
        "Rust: the external `use`; the crate:: path is in-repo"
    );
    for local in [
        "web::local",
        "web::core::thing",
        "tools::other",
        "src::util",
    ] {
        assert_eq!(
            module_cell(&rows, local),
            None,
            "{local} imports nothing external"
        );
    }
    // The cell is on the MODULE only, never on the file's other nodes.
    for (q, module, _, cells) in &rows {
        assert!(
            *module || cells.is_empty(),
            "{q} is no MODULE but has {cells:?}"
        );
    }
}

#[test]
fn the_cell_is_capped_at_256_rows_with_a_trailing_count() {
    let tmp = tempfile::tempdir().unwrap();
    let mut body = String::new();
    for i in 0..300 {
        body.push_str(&format!("import {{ n{i:03} }} from 'pkg{i:03}';\n"));
    }
    body.push_str("export const all = 1;\n");
    write(tmp.path(), "barrel.ts", &body);
    let r = generate_one(tmp.path().to_str().unwrap()).unwrap();
    let cell = module_cell(&rows(&r), "barrel").expect("a cell");
    let json: Vec<serde_json::Value> = serde_json::from_str(&cell).unwrap();
    assert_eq!(json.len(), 257, "256 rows and the trailing count");
    assert_eq!(json[256], serde_json::json!({ "capped": 44 }));
    assert_eq!(
        json[0],
        serde_json::json!({"spec": "pkg000", "name": "n000", "alias": null, "line": 0})
    );
    assert_eq!(json[255]["spec"], "pkg255");
}

#[test]
fn a_cached_rebuild_writes_the_same_cells() {
    let tmp = tempfile::tempdir().unwrap();
    write_fixture(tmp.path());
    let clean = rows(&generate_one(tmp.path().to_str().unwrap()).unwrap());
    let mut cache = ParseCache::default();
    let first = rows(&generate_one_with_cache(tmp.path().to_str().unwrap(), &mut cache).unwrap());
    let second = rows(&generate_one_with_cache(tmp.path().to_str().unwrap(), &mut cache).unwrap());
    assert_eq!(first, clean, "a cold cache build is a clean build");
    assert_eq!(
        second, clean,
        "a warm cache replays no stale or doubled cell"
    );
}
