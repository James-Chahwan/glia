use axum::{routing::get, Router};

async fn list_users() -> &'static str {
    "users"
}

fn app() -> Router {
    Router::new().route("/users", get(list_users))
}
