pub mod cache_impl;
pub mod repo;

use crate::repo::{self, index, Cache, Repo};
use std::sync::Arc;

pub struct Service {
    repo: Repo,
    cache: Arc<Cache>,
}

impl Service {
    pub fn get(&self, id: u32) -> String {
        self.repo.find(id)
    }

    pub fn cached(&self) -> Option<String> {
        self.cache.lookup("k")
    }

    pub fn via_param(&self, r: &Repo) -> String {
        r.find(1)
    }

    pub fn via_local(&self) -> String {
        let r = Repo::new();
        r.find(2)
    }

    pub fn via_typed(&self) -> String {
        let r: Repo = Repo::new();
        r.find(3)
    }

    pub fn unit_value(&self) -> String {
        Repo.find(5)
    }

    pub fn shadowed(&self) -> String {
        let repo = index();
        repo.find(9)
    }
}

pub fn free(r: &mut Repo) -> String {
    r.find(4)
}

pub fn sized(repo: &repo::Index) -> usize {
    repo.size()
}
