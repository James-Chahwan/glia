use google_cloud_pubsub::client::Client;

pub async fn listen(client: &Client) {
    let subscription = client.subscription("orders");
    let _ = subscription.pull(10, None).await;
}
