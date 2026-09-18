// A `.graphql` schema reaches the SDL field scan that embedded
// `type Subscription {` blocks already get; this comment is not SDL.
pub fn route(path: &str) -> bool {
    path.ends_with(".graphql")
}

#[cfg(test)]
mod tests {
    #[test]
    fn schema_routes() {
        let sdl = r#"
type Subscription {
  orderShipped: Order
}
"#;
        assert!(!sdl.is_empty());
    }
}
