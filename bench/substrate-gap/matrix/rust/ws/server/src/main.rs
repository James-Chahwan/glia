use axum::{extract::ws::{WebSocket, WebSocketUpgrade}, response::Response, routing::get, Router};

async fn chat(ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(handle)
}

async fn handle(_socket: WebSocket) {}

pub fn app() -> Router {
    Router::new().route("/ws/chat", get(chat))
}
