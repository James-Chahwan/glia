// A needle table: the decorator names are data here, not decorators.
pub const NOUNS: &[&str] = &[
    "@Resolver(",
    "@ResolveField(",
    "ObjectType):",
    "@strawberry.type",
];

pub fn is_noun(s: &str) -> bool {
    NOUNS.contains(&s)
}
