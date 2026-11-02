//! HT.9: the language seam, pinned. `docs/seams.md` names the few places a
//! new language parser touches outside its own crate; this test fails when a
//! language `detect_language` routes misses one of them, so a leap that moves
//! a seam (or a contributor who misses a line) finds out here, not at merge.
//!
//! The engine's `detect_language`, `build_group`, `TS_FAMILY` and
//! `build_solo` are `pub(crate)` and stay so (no API change for a test): the
//! test keeps its own table of the routing (`EXTS`, `PATH_RULES`, `GROUPS`)
//! and checks that table against the source text of those functions, then
//! exercises each tag through the public [`parse_one`] and [`generate_one`].
//! Checked per tag:
//!
//! 1. the table names exactly the extensions and tags `detect_language` has
//!    (a new arm with no row here fails, and so does a row for a gone arm);
//! 2. `parse_one` on a one-line sample is `Ok` with nodes (never the
//!    `unsupported language` error of `parse_one_as`'s last arm);
//! 3. the parser crate is `parsers/code/<tag>/`, a workspace member and an
//!    engine dependency;
//! 4. its build group agrees with `build_group` / `TS_FAMILY`, and the group
//!    is built by `lang_build` (`TS_GROUP`, or an arm of `build_solo`; a
//!    group that takes `build_solo`'s `_` arm on purpose is in
//!    [`SHARED_BUILD`]);
//! 5. its matrix row (`lang_build::matrix_row`: the TS family reports as
//!    `typescript`) is in `matrix_vocab.LANGUAGES`, and
//!    `bench/substrate-gap/matrix/<group>/` exists;
//! 6. none of its extensions is in `health::CODE_LIKE_EXTS` (the unparsed
//!    list a parsed extension leaves);
//! 7. a repo of one sample per tag builds with every sample's MODULE in the
//!    merged graph and no parse error.
//!
//! The exceptions are the named [`EXEMPT`] list, each with its reason.
//!
//! fired_on marker, once per run:
//!   `[seams] languages=<n> groups=<g> matrix_rows=<m> exempt=<e>`

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use glia_core::RepoId;
use glia_engine::{generate_one, parse_one};

/// Every extension `detect_language` maps, and the tag it maps it to when no
/// [`PATH_RULES`] row applies.
const EXTS: &[(&str, &str)] = &[
    ("py", "python"),
    ("go", "go"),
    ("ts", "typescript"),
    ("tsx", "typescript"),
    ("mts", "typescript"),
    ("cts", "typescript"),
    ("js", "typescript"),
    ("jsx", "typescript"),
    ("mjs", "typescript"),
    ("cjs", "typescript"),
    ("vue", "vue"),
    ("svelte", "vue"),
    ("astro", "vue"),
    ("rs", "rust"),
    ("java", "java"),
    ("kt", "kotlin"),
    ("cs", "csharp"),
    ("rb", "ruby"),
    ("php", "php"),
    ("swift", "swift"),
    ("c", "c_cpp"),
    ("cpp", "c_cpp"),
    ("cc", "c_cpp"),
    ("cxx", "c_cpp"),
    ("h", "c_cpp"),
    ("hh", "c_cpp"),
    ("hpp", "c_cpp"),
    ("hxx", "c_cpp"),
    ("inl", "c_cpp"),
    ("ipp", "c_cpp"),
    ("tpp", "c_cpp"),
    ("scala", "scala"),
    ("clj", "clojure"),
    ("cljs", "clojure"),
    ("cljc", "clojure"),
    ("dart", "dart"),
    ("ex", "elixir"),
    ("exs", "elixir"),
    ("sol", "solidity"),
    ("tf", "terraform"),
    ("hcl", "terraform"),
    ("lua", "lua"),
    ("proto", "proto"),
    ("graphql", "graphql"),
    ("graphqls", "graphql"),
    ("gql", "graphql"),
    ("avsc", "avro"),
];

/// Tags `detect_language` gives by path, not extension: (extension, path
/// fragment, tag).
const PATH_RULES: &[(&str, &str, &str)] = &[("ts", ".component.ts", "angular")];

/// Each parser tag's build group (`build_group`): the TS family builds as
/// `typescript`, Kotlin joins Java's JVM graph, every other tag is its own.
const GROUPS: &[(&str, &str)] = &[
    ("python", "python"),
    ("go", "go"),
    ("typescript", "typescript"),
    ("angular", "typescript"),
    ("vue", "typescript"),
    ("rust", "rust"),
    ("java", "java"),
    ("kotlin", "java"),
    ("csharp", "csharp"),
    ("ruby", "ruby"),
    ("php", "php"),
    ("swift", "swift"),
    ("c_cpp", "c_cpp"),
    ("scala", "scala"),
    ("clojure", "clojure"),
    ("dart", "dart"),
    ("elixir", "elixir"),
    ("solidity", "solidity"),
    ("terraform", "terraform"),
    ("lua", "lua"),
];

/// One sample per parser tag: (tag, file name, one line of source). Stems
/// are distinct so every MODULE keeps its stem-form qname in the joint build.
const SAMPLES: &[(&str, &str, &str)] = &[
    ("python", "seam_python.py", "def seam_f():\n    return 1\n"),
    ("go", "seam_go.go", "package seam\n\nfunc SeamF() {}\n"),
    (
        "typescript",
        "seam_typescript.ts",
        "export function seamF() { return 1; }\n",
    ),
    (
        "angular",
        "seam_angular.component.ts",
        "export class SeamComponent {}\n",
    ),
    (
        "vue",
        "seam_vue.vue",
        "<script>\nexport default { name: 'seam' };\n</script>\n",
    ),
    ("rust", "seam_rust.rs", "fn seam_f() {}\n"),
    ("java", "SeamJava.java", "class SeamJava { void f() {} }\n"),
    ("kotlin", "SeamKotlin.kt", "fun seamF() {}\n"),
    (
        "csharp",
        "SeamCsharp.cs",
        "class SeamCsharp { void F() {} }\n",
    ),
    ("ruby", "seam_ruby.rb", "def seam_f; end\n"),
    ("php", "seam_php.php", "<?php function seam_f() {}\n"),
    ("swift", "SeamSwift.swift", "func seamF() {}\n"),
    ("c_cpp", "seam_c.c", "int seam_f(void) { return 0; }\n"),
    (
        "scala",
        "SeamScala.scala",
        "object SeamScala { def f = 1 }\n",
    ),
    ("clojure", "seam_clojure.clj", "(defn seam-f [] 1)\n"),
    ("dart", "seam_dart.dart", "void seamF() {}\n"),
    (
        "elixir",
        "seam_elixir.ex",
        "defmodule Seam do\n  def f, do: 1\nend\n",
    ),
    (
        "solidity",
        "seam_solidity.sol",
        "contract Seam { function f() public {} }\n",
    ),
    (
        "terraform",
        "seam_terraform.tf",
        "resource \"null_resource\" \"seam\" {}\n",
    ),
    (
        "lua",
        "seam_lua.lua",
        "local function seam_f() return 1 end\n",
    ),
];

/// Which language-seam check an [`EXEMPT`] row lifts.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Lifts {
    /// A routing tag: `route.rs` sends the file to a scan, never to a
    /// language parser, so it has no parser crate, build group or matrix row
    /// (`parse_one` refuses it, which the test checks).
    Routing,
    /// The tag has its parser and build group but no
    /// `bench/substrate-gap/matrix/<group>/` cell directory.
    MatrixDir,
}

/// The named exceptions, each with its reason.
const EXEMPT: &[(&str, Lifts, &str)] = &[
    (
        "proto",
        Lifts::Routing,
        "a .proto file is scanned for messages and services in route.rs (A10.x), never parsed as a language",
    ),
    (
        "graphql",
        Lifts::Routing,
        "a GraphQL schema is routed to the SDL resolver scan in route.rs (A10.4)",
    ),
    (
        "avro",
        Lifts::Routing,
        "an .avsc schema is routed to the MESSAGE_TYPE scan in route.rs (A10.6)",
    ),
    (
        "terraform",
        Lifts::MatrixDir,
        "IaC: its structural cells are n/a in matrix_vocab.NOT_APPLICABLE and its graded cells \
         are legacy fixtures (fixtures/terraform-*), so no matrix/terraform/ cell exists yet",
    ),
];

/// Build groups that take `build_solo`'s `_` arm (`glia_graph::build_typescript`
/// with `resolve_relative_source`) on purpose, as `build_solo`'s doc says.
/// A new language that falls through to `_` without being named here fails:
/// it either gets its own `glia_graph::build_<lang>` arm or a row here.
const SHARED_BUILD: &[&str] = &["solidity", "terraform"];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("engine/ has a parent")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

/// The lines of `fn <name>`'s body in `src`: from its signature to the first
/// line that is exactly `}` (a top-level fn's closing brace).
fn fn_body<'a>(src: &'a str, signature: &str) -> Vec<&'a str> {
    let mut lines = src.lines();
    lines
        .by_ref()
        .find(|l| l.starts_with(signature))
        .unwrap_or_else(|| panic!("`{signature}` not found: the seam moved, update docs/seams.md"));
    lines.take_while(|l| *l != "}").collect()
}

/// Every `"..."` literal in `text`, in order.
fn literals(text: &str) -> Vec<&str> {
    text.split('"').skip(1).step_by(2).collect()
}

/// Every `Some("x")` tag in `text`.
fn some_tags(text: &str) -> Vec<&str> {
    text.match_indices("Some(\"")
        .filter_map(|(i, m)| {
            let rest = &text[i + m.len()..];
            rest.find('"').map(|end| &rest[..end])
        })
        .collect()
}

/// `detect_language`'s arms as (extensions, tags the arm can return).
fn detect_arms(src: &str) -> Vec<(Vec<String>, BTreeSet<String>)> {
    let mut arms: Vec<(Vec<String>, BTreeSet<String>)> = Vec::new();
    for line in fn_body(src, "pub(crate) fn detect_language(") {
        let t = line.trim_start();
        if t.starts_with("//") {
            continue;
        }
        let rhs = match t.split_once("=>") {
            Some((lhs, rhs)) if lhs.trim_start().starts_with('"') => {
                arms.push((
                    literals(lhs).into_iter().map(str::to_string).collect(),
                    BTreeSet::new(),
                ));
                rhs
            }
            // `_ => None` closes the last arm.
            Some(_) => break,
            None => t,
        };
        if let Some((_, tags)) = arms.last_mut() {
            tags.extend(some_tags(rhs).into_iter().map(str::to_string));
        }
    }
    arms
}

#[test]
fn every_language_tag_has_its_seams() {
    let extract = read("engine/src/extract.rs");
    let lang_build = read("engine/src/build/lang_build.rs");
    let workspace = read("Cargo.toml");
    let engine_toml = read("engine/Cargo.toml");
    let vocab = read("bench/substrate-gap/matrix_vocab.py");

    let exempt = |tag: &str, lifts: Lifts| EXEMPT.iter().any(|(t, l, _)| *t == tag && *l == lifts);
    let routing = |tag: &str| exempt(tag, Lifts::Routing);

    // 1. The table against detect_language's arms.
    let arms = detect_arms(&extract);
    assert!(
        !arms.is_empty(),
        "detect_language has no arms the test can read"
    );
    let table: BTreeMap<&str, &str> = EXTS.iter().copied().collect();
    assert_eq!(
        table.len(),
        EXTS.len(),
        "an extension is listed twice in EXTS"
    );
    let mut source_exts: BTreeSet<String> = BTreeSet::new();
    let mut source_tags: BTreeSet<String> = BTreeSet::new();
    for (exts, tags) in &arms {
        assert!(
            !tags.is_empty(),
            "detect_language arm {exts:?} returns no tag"
        );
        for ext in exts {
            source_exts.insert(ext.clone());
            let tag = table.get(ext.as_str()).unwrap_or_else(|| {
                panic!(
                    "detect_language maps .{ext} but the seams table does not: add it to \
                     EXTS in engine/tests/seams.rs and check docs/seams.md's language seam"
                )
            });
            assert!(
                tags.contains(*tag),
                "EXTS maps .{ext} to {tag}, detect_language's arm returns {tags:?}"
            );
        }
        source_tags.extend(tags.iter().cloned());
    }
    let table_exts: BTreeSet<String> = EXTS.iter().map(|(e, _)| e.to_string()).collect();
    assert_eq!(
        table_exts, source_exts,
        "EXTS lists an extension detect_language no longer maps"
    );
    for (ext, fragment, tag) in PATH_RULES {
        assert!(
            extract.contains(&format!("path.contains(\"{fragment}\")")),
            "detect_language no longer routes `{fragment}` paths"
        );
        assert!(
            table.contains_key(ext),
            "PATH_RULES names .{ext}, which EXTS lacks"
        );
        assert!(
            source_tags.contains(*tag),
            "detect_language no longer returns {tag}"
        );
    }
    let table_tags: BTreeSet<String> = EXTS
        .iter()
        .map(|(_, t)| *t)
        .chain(PATH_RULES.iter().map(|(_, _, t)| *t))
        .map(str::to_string)
        .collect();
    assert_eq!(
        table_tags, source_tags,
        "the seams tables and detect_language disagree on the tags"
    );

    let parser_tags: Vec<&str> = {
        let mut v: Vec<&str> = source_tags
            .iter()
            .map(String::as_str)
            .filter(|t| !routing(t))
            .collect();
        v.sort_unstable();
        v
    };
    for (tag, _, _) in EXEMPT {
        assert!(
            source_tags.contains(*tag),
            "EXEMPT names {tag}, which detect_language no longer returns: drop the row"
        );
    }

    // 2. parse_one: every parser tag parses its sample; a routing tag is refused.
    let repo = RepoId::from_canonical("test://seams");
    let samples: BTreeMap<&str, (&str, &str)> =
        SAMPLES.iter().map(|(t, f, s)| (*t, (*f, *s))).collect();
    for tag in &parser_tags {
        let (file, source) = samples.get(tag).unwrap_or_else(|| {
            panic!("no SAMPLES row for {tag}: add a one-line sample to engine/tests/seams.rs")
        });
        let fp = parse_one(source, file, tag, repo)
            .unwrap_or_else(|e| panic!("parse_one({file}, {tag}) failed: {e}"));
        assert!(
            !fp.nodes.is_empty(),
            "parse_one({file}, {tag}) minted no node"
        );
    }
    for (tag, lifts, _) in EXEMPT {
        if *lifts == Lifts::Routing {
            let err = parse_one("", &format!("seam.{tag}"), tag, repo)
                .err()
                .unwrap_or_else(|| {
                    panic!("{tag} is exempt as routing-only but parse_one parses it")
                });
            assert!(err.contains("unsupported language"), "{tag}: {err}");
        }
    }

    // 3. The parser crate: its directory, the workspace member, the engine dependency.
    for tag in &parser_tags {
        let dir = format!("parsers/code/{tag}");
        assert!(
            root().join(&dir).join("Cargo.toml").is_file(),
            "{dir}/Cargo.toml missing"
        );
        assert!(
            workspace.contains(&format!("\"{dir}\"")),
            "{dir} is not a workspace member (root Cargo.toml)"
        );
        assert!(
            engine_toml.contains(&format!("path = \"../{dir}\"")),
            "engine/Cargo.toml does not depend on {dir} (the engine owns every parser dependency)"
        );
    }

    // 4. The build group: build_group / TS_FAMILY agree, and lang_build builds it.
    let groups: BTreeMap<&str, &str> = GROUPS.iter().copied().collect();
    let table_parsers: BTreeSet<&str> = groups.keys().copied().collect();
    let source_parsers: BTreeSet<&str> = parser_tags.iter().copied().collect();
    assert_eq!(
        table_parsers, source_parsers,
        "GROUPS and the parser tags disagree"
    );
    let ts_family_line = extract
        .lines()
        .find(|l| l.starts_with("pub(crate) const TS_FAMILY"))
        .expect("TS_FAMILY not found in engine/src/extract.rs");
    let ts_family: BTreeSet<&str> = literals(ts_family_line).into_iter().collect();
    let build_group_body = fn_body(&extract, "pub(crate) fn build_group(").join("\n");
    assert!(
        lang_build.contains("const TS_GROUP: &str = \"typescript\";"),
        "lang_build's TS_GROUP moved"
    );
    let solo_arms: BTreeSet<&str> = fn_body(&lang_build, "fn build_solo(")
        .into_iter()
        .filter_map(|l| l.trim_start().split_once("=>"))
        .filter(|(lhs, _)| lhs.trim_start().starts_with('"'))
        .flat_map(|(lhs, _)| literals(lhs))
        .collect();
    for tag in &parser_tags {
        let group = groups[tag];
        if group == "typescript" {
            assert!(
                *tag == "typescript" || ts_family.contains(tag),
                "{tag} is grouped as typescript but is not in TS_FAMILY"
            );
        } else if group != *tag {
            assert!(
                build_group_body.contains(&format!("\"{tag}\""))
                    && build_group_body.contains(&format!("\"{group}\"")),
                "GROUPS joins {tag} to {group} but build_group does not"
            );
        } else {
            assert!(
                !ts_family.contains(tag),
                "{tag} is in TS_FAMILY but GROUPS keeps it apart"
            );
            // The tag's literal in build_group is only a remap's target
            // (`"java"` for Kotlin), never a remapped source.
            let is_target = GROUPS.iter().any(|(t, g)| t != tag && g == tag);
            assert!(
                is_target || !build_group_body.contains(&format!("\"{tag}\"")),
                "build_group remaps {tag} but GROUPS keeps it as its own group"
            );
        }
        if group != "typescript" {
            let shared = SHARED_BUILD.contains(&group);
            assert!(
                solo_arms.contains(group) != shared,
                "group {group}: lang_build's build_solo {} for it, SHARED_BUILD {} it",
                if solo_arms.contains(group) {
                    "has an arm"
                } else {
                    "has no arm"
                },
                if shared { "names" } else { "does not name" },
            );
        }
    }

    // 5. The matrix row and the matrix directory.
    let languages_block = vocab
        .split_once("\nLANGUAGES = [")
        .and_then(|(_, rest)| rest.split_once(']'))
        .map(|(block, _)| block)
        .expect("LANGUAGES not found in matrix_vocab.py");
    let languages: BTreeSet<&str> = literals(languages_block).into_iter().collect();
    let mut rows: BTreeSet<&str> = BTreeSet::new();
    for tag in &parser_tags {
        let group = groups[tag];
        // lang_build::matrix_row: the TS-family tags report as `typescript`.
        let row = if group == "typescript" {
            "typescript"
        } else {
            *tag
        };
        assert!(
            languages.contains(row),
            "matrix row {row} ({tag}) is not in matrix_vocab.LANGUAGES"
        );
        rows.insert(row);
        if !exempt(tag, Lifts::MatrixDir) {
            assert!(
                root()
                    .join("bench/substrate-gap/matrix")
                    .join(group)
                    .is_dir(),
                "bench/substrate-gap/matrix/{group}/ is missing for {tag}"
            );
        }
    }

    // 6. A parsed extension is not on health's unparsed list.
    for (ext, tag) in EXTS {
        assert!(
            !glia_engine::health::CODE_LIKE_EXTS.contains(ext),
            ".{ext} ({tag}) is parsed but still in health::CODE_LIKE_EXTS"
        );
    }

    // 7. One build of every sample: each MODULE lands in the merged graph.
    let tmp = tempfile::tempdir().expect("tempdir");
    for (_, file, source) in SAMPLES {
        std::fs::write(tmp.path().join(file), source).expect("write sample");
    }
    let built = generate_one(tmp.path().to_str().expect("utf-8 tempdir")).expect("samples build");
    assert!(
        built.parse_errors.is_empty(),
        "parse errors: {:?}",
        built.parse_errors
    );
    let qnames: BTreeSet<&str> = built
        .merged
        .graphs
        .iter()
        .flat_map(|g| g.nav.qname_by_id.values())
        .map(String::as_str)
        .collect();
    for (tag, file, _) in SAMPLES {
        let stem = file.split('.').next().unwrap_or(file);
        assert!(
            qnames.iter().any(|q| q.starts_with(stem)),
            "the build of {file} ({tag}) has no node under {stem}"
        );
    }

    let distinct_groups: BTreeSet<&str> = parser_tags.iter().map(|t| groups[t]).collect();
    eprintln!(
        "[seams] languages={} groups={} matrix_rows={} exempt={}",
        parser_tags.len(),
        distinct_groups.len(),
        rows.len(),
        EXEMPT.len()
    );
}
