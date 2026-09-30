pub async fn listen(client: &async_nats::Client) {
    let _subscriber = client.subscribe("orders").await;
}
