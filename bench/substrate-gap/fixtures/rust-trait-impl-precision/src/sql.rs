use crate::repo::Repo;
use std::fmt::Display;

pub struct SqlRepo;

impl Repo for SqlRepo {
    fn get(&self, id: &str) -> String {
        id.to_string()
    }
}

impl Display for SqlRepo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "sql")
    }
}
