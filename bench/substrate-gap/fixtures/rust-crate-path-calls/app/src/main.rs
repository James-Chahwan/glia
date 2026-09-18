fn main() {
    acme_core::service_map();
    acme_core::api::helper();
    let e = acme_core::api::Engine::new();
    e.run();
}

fn helper() -> u32 {
    0
}
