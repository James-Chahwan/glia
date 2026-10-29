//! FH.20 — TypeScript object type aliases are nodes. A top-level
//! `type X = { .. }`, or an intersection of object literal types and named
//! types (`type X = A & { .. }`), mints an INTERFACE `X` with a SCHEMA_FIELDS
//! cell read as an interface's (DT.1c, format ts): an intersection's named
//! members are its `extends` and its INHERITS_FROM bases. A union of
//! literals, a function type and a plain rename (`type Y = X`) mint nothing.
//!
//! A type annotation or generic argument naming an in-file alias
//! (`fetchJson<{ data: X }>(..)`, `: X`, `Promise<X>`, `x as X`) is a USES of
//! the alias from the scope that holds it (parser rule `type_ref`), one edge
//! per (scope, alias) at the first site's row. A type parameter of that name
//! shadows the alias. This is how a generated fetch SDK (oazapfts) whose DTOs
//! are aliases becomes a hop a client walk crosses: client -> SDK function
//! -> DTO.

use glia_code_domain::evidence::Evidence;
use glia_code_domain::wire::ModelSchema;
use glia_code_domain::{edge_category, node_kind};
use glia_core::NodeId;
use glia_engine::generate_one;
use glia_graph::MergedGraph;

const MODELS: &str = r#"export type UserDto = {
  id: string;
  name?: string;
  email: string | undefined;
};

export type AdminDto = UserDto & {
  role: string;
};

export type Paged = Omit<UserDto, 'id'> & { [key: string]: unknown };

export type Status = 'active' | 'disabled';
export type Handler = (id: string) => void;
export type Renamed = UserDto;
"#;

const SDK: &str = r#"declare function fetchJson<T>(url: string): Promise<T>;

export type UserResponseDto = {
  id: string;
  avatar: string;
};

export type UpdateDto = {
  name: string;
};

export function getUser(id: string): Promise<UserResponseDto> {
  return fetchJson<{
    status: 200;
    data: UserResponseDto;
  }>(`/users/${id}`);
}

export function listUsers() {
  return fetchJson<UserResponseDto[]>('/users');
}

export const updateUser = async (dto: UpdateDto): Promise<void> => {
  const body = dto as UpdateDto;
  await fetchJson<void>(`/users/${body.name}`);
};

export function shadowed<UpdateDto>(x: UpdateDto): UpdateDto {
  return x;
}

export class UserApi {
  load(): Promise<UserResponseDto> {
    return getUser('me');
  }
}
"#;

fn build() -> MergedGraph {
    let dir = tempfile::tempdir().expect("tempdir");
    for (f, body) in [
        ("package.json", r#"{"name":"alias-app","version":"1.0.0"}"#),
        ("src/models.ts", MODELS),
        ("src/sdk.ts", SDK),
    ] {
        let path = dir.path().join(f);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, body).expect("write");
    }
    generate_one(dir.path().to_str().expect("utf-8 temp path"))
        .unwrap_or_else(|e| panic!("generate_one: {e}"))
        .merged
}

/// The node of kind `kind` whose qname is `qname`.
fn node_id(m: &MergedGraph, kind: glia_core::NodeKindId, qname: &str) -> Option<NodeId> {
    m.graphs.iter().find_map(|g| {
        g.nav
            .qname_by_id
            .iter()
            .find(|(id, q)| q.as_str() == qname && g.nav.kind_by_id.get(id) == Some(&kind))
            .map(|(id, _)| *id)
    })
}

/// The SCHEMA_FIELDS schema of the INTERFACE `qname`.
fn schema(m: &MergedGraph, qname: &str) -> ModelSchema {
    let id = node_id(m, node_kind::INTERFACE, qname)
        .unwrap_or_else(|| panic!("{qname}: no INTERFACE node"));
    let node = m
        .graphs
        .iter()
        .flat_map(|g| g.nodes.iter())
        .find(|n| n.id == id)
        .expect("node");
    let mut cells = node.cells.iter().filter_map(ModelSchema::from_cell);
    let s = cells
        .next()
        .unwrap_or_else(|| panic!("{qname}: no SCHEMA_FIELDS"));
    assert!(cells.next().is_none(), "{qname}: one SCHEMA_FIELDS cell");
    s
}

/// `(field name, type, required)` of a schema, in source order.
fn fields(s: &ModelSchema) -> Vec<(String, String, bool)> {
    s.fields
        .iter()
        .map(|f| (f.name.clone(), f.ty.clone(), f.required))
        .collect()
}

/// Every `cat` edge, as (from qname, to qname, emitter, rule, 0-based line),
/// sorted.
fn edges(
    m: &MergedGraph,
    cat: glia_core::EdgeCategoryId,
) -> Vec<(String, String, String, String, u32)> {
    let qname = |id: &NodeId| {
        m.graphs
            .iter()
            .find_map(|g| g.nav.qname_by_id.get(id).cloned())
            .unwrap_or_default()
    };
    let mut out: Vec<_> = m
        .all_edges()
        .filter(|e| e.category == cat)
        .map(|e| {
            let ev = Evidence::read(&e.cells).expect("every edge carries EVIDENCE");
            (
                qname(&e.from),
                qname(&e.to),
                ev.emitter,
                ev.rule.unwrap_or_default(),
                ev.line.unwrap_or(u32::MAX),
            )
        })
        .collect();
    out.sort();
    out
}

#[test]
fn object_aliases_are_interfaces_with_their_fields() {
    let m = build();
    let user = schema(&m, "src::models::UserDto");
    assert_eq!(
        fields(&user),
        vec![
            ("id".to_string(), "string".to_string(), true),
            ("name".to_string(), "string".to_string(), false),
            ("email".to_string(), "string | undefined".to_string(), false),
        ]
    );
    assert!(user.extends.is_empty() && !user.open);

    // `UserDto & { role }`: the named member is a base, as `extends` is.
    let admin = schema(&m, "src::models::AdminDto");
    assert_eq!(
        fields(&admin),
        vec![("role".to_string(), "string".to_string(), true)]
    );
    assert_eq!(admin.extends, ["UserDto"]);
    assert!(!admin.open);
    assert!(
        edges(&m, edge_category::INHERITS_FROM)
            .iter()
            .any(|(f, t, ..)| f == "src::models::AdminDto" && t == "src::models::UserDto"),
        "AdminDto INHERITS_FROM UserDto"
    );

    // A utility base and an index signature leave the model open.
    let paged = schema(&m, "src::models::Paged");
    assert!(paged.open && paged.fields.is_empty());

    for q in ["Status", "Handler", "Renamed"] {
        let qname = format!("src::models::{q}");
        assert!(
            m.graphs
                .iter()
                .all(|g| !g.nav.qname_by_id.values().any(|v| *v == qname)),
            "{qname}: a non-object alias mints nothing"
        );
    }
}

#[test]
fn a_type_naming_an_alias_is_a_uses_from_its_scope() {
    let m = build();
    let uses: Vec<_> = edges(&m, edge_category::USES)
        .into_iter()
        .filter(|(_, _, _, rule, _)| rule == "type_ref")
        .collect();
    let row = |from: &str, to: &str, line: u32| {
        (
            format!("src::sdk::{from}"),
            format!("src::sdk::{to}"),
            "parser:typescript".to_string(),
            "type_ref".to_string(),
            line,
        )
    };
    // 0-based rows. `getUser`'s first site is its return annotation, the
    // generic argument's `data: UserResponseDto` a later one of the same
    // pair; `shadowed`'s type parameter hides the alias.
    assert_eq!(
        uses,
        vec![
            row("UserApi::load", "UserResponseDto", 32),
            row("getUser", "UserResponseDto", 11),
            row("listUsers", "UserResponseDto", 19),
            row("updateUser", "UpdateDto", 22),
        ]
    );
}
