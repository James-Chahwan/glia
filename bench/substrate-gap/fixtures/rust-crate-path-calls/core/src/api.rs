pub fn service_map() -> u32 {
    helper()
}

pub fn helper() -> u32 {
    super::root_fn2()
}

pub struct Engine;

impl Engine {
    pub fn new() -> Self {
        Engine
    }
    pub fn run(&self) -> u32 {
        Self::step()
    }
    fn step() -> u32 {
        1
    }
}
