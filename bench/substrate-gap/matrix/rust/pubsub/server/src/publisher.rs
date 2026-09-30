use google_cloud_pubsub::client::Client;

pub async fn publish(client: &Client) {
    let topic = client.topic("orders");
    let publisher = topic.new_publisher(None);
    let _ = publisher.publish(Default::default()).await;
}
