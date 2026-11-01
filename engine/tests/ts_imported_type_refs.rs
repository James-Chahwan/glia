//! GD.27 — TypeScript type references through import bindings (FH.20's
//! follow-up). A type name in a type position (a parameter or return
//! annotation, a generic argument, an `as` type) that is a named or default
//! import binding of the file is a USES of the INTERFACE / CLASS / ENUM it
//! imports, from the scope that reads it: one edge per (scope, type) at the
//! first site's row, `graph:refs` rule `type_ref`, bound through the
//! importer's import bindings (an aliased import through a re-export barrel,
//! a default-imported class). An abstract method's signature reads its type
//! names the way a callable's does. A type parameter shadows an import, an
//! import naming a const or a function is no type, and a third-party type
//! binds nothing.

use glia_code_domain::edge_category;
use glia_code_domain::evidence::Evidence;
use glia_engine::generate_one;
use glia_graph::MergedGraph;

const TYPES: &str = r#"export type UserDto = {
  id: string;
  name: string;
};

export interface Out {
  ok: boolean;
}

export enum Role {
  Admin = "admin",
  Member = "member",
}

export function makeUser(): UserDto {
  return { id: "1", name: "a" };
}
"#;

const STATUS: &str = r#"export const Status = {
  Active: "active",
  Disabled: "disabled",
} as const;

export type Status = (typeof Status)[keyof typeof Status];
"#;

const REPO: &str = r#"export default class Repo {
  find(id: string): string {
    return id;
  }
}
"#;

const CLIENT: &str = r#"import axios from "axios";
import type { AxiosResponse } from "axios";
import { UserDto, Out, makeUser } from "./models/types";
import { Role as AccessRole } from "./models";
import { Status } from "./models/status";
import Repo from "./repo";

export async function getUser(): Promise<UserDto> {
  const res = await axios.get<UserDto>("/api/user");
  return res.data;
}

type LocalDto = { v: number };

export function getLocal(): LocalDto {
  return { v: 1 };
}

export abstract class Tool {
  abstract run(input: UserDto): Out;
  abstract local(input: LocalDto): void;
}

export function grant(role: AccessRole): boolean {
  return role === AccessRole.Admin;
}

export function lookup(repo: Repo, id: string): string {
  return repo.find(id);
}

export function wrap<UserDto>(value: UserDto): UserDto {
  return value;
}

export function isActive(s: Status): boolean {
  return s === Status.Active;
}

export function raw(res: AxiosResponse<UserDto>): UserDto {
  return res.data;
}

export function useMaker() {
  return [makeUser];
}
"#;

fn build() -> MergedGraph {
    let dir = tempfile::tempdir().expect("tempdir");
    for (f, body) in [
        (
            "package.json",
            r#"{"name":"typeref-app","dependencies":{"axios":"^1.7.0"}}"#,
        ),
        ("src/models/types.ts", TYPES),
        ("src/models/index.ts", "export * from \"./types\";\n"),
        ("src/models/status.ts", STATUS),
        ("src/repo.ts", REPO),
        ("src/client.ts", CLIENT),
    ] {
        let path = dir.path().join(f);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, body).expect("write");
    }
    generate_one(dir.path().to_str().expect("utf-8 temp path"))
        .unwrap_or_else(|e| panic!("generate_one: {e}"))
        .merged
}

/// Every USES edge whose EVIDENCE rule is `type_ref`, as (from qname, to
/// qname, emitter, 0-based line), sorted.
fn type_ref_uses(m: &MergedGraph) -> Vec<(String, String, String, u32)> {
    let qname = |id: &glia_core::NodeId| {
        m.graphs
            .iter()
            .find_map(|g| g.nav.qname_by_id.get(id).cloned())
            .unwrap_or_default()
    };
    let mut out: Vec<_> = m
        .all_edges()
        .filter(|e| e.category == edge_category::USES)
        .filter_map(|e| {
            let ev = Evidence::read(&e.cells).expect("every edge carries EVIDENCE");
            (ev.rule.as_deref() == Some("type_ref")).then(|| {
                (
                    qname(&e.from),
                    qname(&e.to),
                    ev.emitter,
                    ev.line.unwrap_or(u32::MAX),
                )
            })
        })
        .collect();
    out.sort();
    out
}

#[test]
fn imported_type_names_are_uses_of_the_types_they_import() {
    let m = build();
    let row = |from: &str, to: &str, emitter: &str, line: u32| {
        (
            format!("src::client::{from}"),
            to.to_string(),
            emitter.to_string(),
            line,
        )
    };
    // 0-based rows. The abstract `run` reads both imports; `local` and
    // `getLocal` read the in-file alias (FH.20's parser edge); `grant` binds
    // through the `export *` barrel under its alias, `lookup` the
    // default-imported class. `wrap` (a shadowing type parameter),
    // `isActive` (`Status` imports a const) and `raw`'s AxiosResponse (a
    // third-party type) add none. types.ts's own `makeUser(): UserDto` is
    // FH.20's in-file edge.
    assert_eq!(
        type_ref_uses(&m),
        vec![
            row(
                "Tool::local",
                "src::client::LocalDto",
                "parser:typescript",
                20
            ),
            row("Tool::run", "src::models::types::Out", "graph:refs", 19),
            row("Tool::run", "src::models::types::UserDto", "graph:refs", 19),
            row("getLocal", "src::client::LocalDto", "parser:typescript", 14),
            row("getUser", "src::models::types::UserDto", "graph:refs", 7),
            row("grant", "src::models::types::Role", "graph:refs", 23),
            row("lookup", "src::repo::Repo", "graph:refs", 27),
            row("raw", "src::models::types::UserDto", "graph:refs", 39),
            (
                "src::models::types::makeUser".to_string(),
                "src::models::types::UserDto".to_string(),
                "parser:typescript".to_string(),
                14,
            ),
        ]
    );
}

#[test]
fn a_function_read_by_value_keeps_its_value_ref() {
    let m = build();
    let rules: Vec<String> = m
        .all_edges()
        .filter(|e| e.category == edge_category::USES)
        .filter_map(|e| {
            let from = m
                .graphs
                .iter()
                .find_map(|g| g.nav.qname_by_id.get(&e.from).cloned())?;
            let to = m
                .graphs
                .iter()
                .find_map(|g| g.nav.qname_by_id.get(&e.to).cloned())?;
            (from == "src::client::useMaker" && to == "src::models::types::makeUser")
                .then(|| Evidence::read(&e.cells).and_then(|ev| ev.rule))
                .flatten()
        })
        .collect();
    assert_eq!(rules, ["import_binding"]);
}
