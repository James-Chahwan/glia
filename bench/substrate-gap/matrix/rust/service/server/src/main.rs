use axum::{routing::get, Router};

pub struct UserService;

impl UserService {
    pub fn new() -> Self {
        UserService
    }
    pub fn name(&self) -> &str {
        "users"
    }
    pub async fn list(&self) -> Vec<String> {
        vec![]
    }
}

async fn list_users() -> String {
    String::new()
}

#[tokio::main]
async fn main() {
    let _app: Router = Router::new().route("/users", get(list_users));
}
