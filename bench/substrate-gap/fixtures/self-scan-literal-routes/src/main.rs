use axum::{Router, routing::get};

async fn list() -> &'static str {
    ""
}

pub fn app() -> Router {
    Router::new().route("/items", get(list))
}

#[cfg(test)]
mod t {
    const CASES: &[(&str, &str)] = &[("Router::new().route(\"/ghost\", get(chat))", "ws:/ghost")];
}

// .route("/comment", get(list))
