use aws_sdk_sqs::Client;

pub async fn poll(client: &Client) {
    let _ = client.receive_message().queue_url("https://sqs.us-east-1.amazonaws.com/123/orders").send().await;
}
