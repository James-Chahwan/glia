use aws_sdk_sqs::Client;

pub async fn enqueue(client: &Client, body: &str) {
    let _ = client
        .send_message()
        .queue_url("https://sqs.us-east-1.amazonaws.com/123/orders")
        .message_body(body)
        .send()
        .await;
}
