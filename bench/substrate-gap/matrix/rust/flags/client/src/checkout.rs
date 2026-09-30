use launchdarkly_server_sdk::{Client, ContextBuilder};

pub fn variant(client: &Client, user: &str) -> &'static str {
    let ctx = ContextBuilder::new(user).build().unwrap();
    if client.bool_variation(&ctx, "new-checkout", false) { "new" } else { "legacy" }
}
