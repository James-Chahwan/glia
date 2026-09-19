use crate::repo::Cache;

impl Cache {
    pub fn lookup(&self, k: &str) -> Option<String> {
        Some(k.to_string())
    }
}
