pub mod node_kind {
    pub const MODULE: u32 = 1;
}

pub mod endpoint {
    use super::node_kind;

    pub fn kind() -> u32 {
        node_kind::MODULE
    }
}
