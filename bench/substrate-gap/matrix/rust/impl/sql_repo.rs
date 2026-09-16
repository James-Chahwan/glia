use crate::repo::Repo;

pub struct SqlRepo;

impl Repo for SqlRepo {
    fn get(&self, id: &str) -> String {
        id.to_string()
    }
}
