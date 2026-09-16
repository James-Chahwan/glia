use reqwest::Client;
use std::collections::HashMap;

/// Outbound HTTP client. Deliberately contains NO axum/tide/salvo route
/// needles (`.route(`, `.at(`, `Router::with_path(`) so the server-route
/// scanners cannot fire — any ROUTE node here would be a phantom.
pub struct ApiClient {
    client: Client,
    cache: HashMap<String, String>,
}

impl ApiClient {
    /// `format!` path → `GET /api/users/${…}` (host stripped, `{}` → `${…}`).
    pub async fn fetch_user(&self, id: &str) -> String {
        self.client
            .get(format!("http://users-svc/api/users/{}", id))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
    }

    /// Literal relative path → `POST /api/users`, pairs with the Go chi route.
    pub async fn create_user(&self, body: String) -> String {
        self.client
            .post("/api/users")
            .body(body)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
    }

    /// NEGATIVE CONTROL: `.get(` on a HashMap with a variable key.
    pub fn cached(&self, id: &str) -> Option<&String> {
        self.cache.get(id)
    }

    /// NEGATIVE CONTROL: `.get(` on a HashMap with a *string literal* key —
    /// the case only `url_to_path` can reject.
    pub fn default_cached(&self) -> Option<&String> {
        self.cache.get("default")
    }
}
