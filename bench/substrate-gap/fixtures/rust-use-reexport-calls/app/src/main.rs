mod extra;
use reexp_lib::{generate_one, Core};
use serde::Serialize;

fn main() {
    generate_one();
    let _ = Core::new();
    helpers();
}

fn helpers() -> u32 {
    use reexp_lib::generate_many as gm;
    gm()
}
