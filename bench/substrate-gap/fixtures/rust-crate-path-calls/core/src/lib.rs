pub mod api;
pub use api::service_map;

pub fn root_fn() -> u32 {
    crate::api::helper() + self::api::helper()
}

pub fn root_fn2() -> u32 {
    2
}
