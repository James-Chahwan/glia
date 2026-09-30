use rdkafka::producer::{FutureProducer, FutureRecord};
use std::time::Duration;

pub async fn send(producer: &FutureProducer, body: &str) {
    let _ = producer.send(FutureRecord::to("orders").payload(body).key("k"), Duration::from_secs(0)).await;
}
