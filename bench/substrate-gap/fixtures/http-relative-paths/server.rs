use axum::{Router, routing::get};

async fn list_widgets() {}

pub fn app() -> Router {
    Router::new().route("widgets", get(list_widgets))
}
