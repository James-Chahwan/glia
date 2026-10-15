pub trait Repo {
    fn get(&self, id: &str) -> String;
}
