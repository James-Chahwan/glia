pub async fn publish(client: &async_nats::Client) {
    let _ = client.publish("orders", "order-1".into()).await;
}
