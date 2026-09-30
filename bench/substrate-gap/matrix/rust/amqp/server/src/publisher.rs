use lapin::{options::BasicPublishOptions, BasicProperties, Channel};

pub async fn publish(channel: &Channel, body: &[u8]) {
    let _ = channel
        .basic_publish("", "orders", BasicPublishOptions::default(), body, BasicProperties::default())
        .await;
}
