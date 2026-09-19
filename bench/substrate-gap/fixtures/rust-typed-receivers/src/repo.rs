pub struct Repo;

impl Repo {
    pub fn new() -> Self {
        Repo
    }

    pub fn find(&self, id: u32) -> String {
        id.to_string()
    }
}

pub struct Cache;

pub struct Index;

impl Index {
    pub fn find(&self, id: u32) -> String {
        format!("i{id}")
    }

    pub fn size(&self) -> usize {
        1
    }
}

pub fn size() -> usize {
    0
}

pub fn index() -> Index {
    Index
}
