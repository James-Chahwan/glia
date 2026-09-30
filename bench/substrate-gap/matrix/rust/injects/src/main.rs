use axum::{extract::State, routing::get, Router};
use std::sync::Arc;

pub trait UserRepository: Send + Sync {
    fn all(&self) -> Vec<String>;
}

pub struct PgUserRepository;

impl UserRepository for PgUserRepository {
    fn all(&self) -> Vec<String> {
        vec![]
    }
}

async fn list_users(State(repo): State<Arc<dyn UserRepository>>) -> String {
    repo.all().join(",")
}

pub fn app() -> Router {
    let repo: Arc<dyn UserRepository> = Arc::new(PgUserRepository);
    Router::new().route("/users", get(list_users)).with_state(repo)
}
