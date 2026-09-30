use rdkafka::consumer::{Consumer, StreamConsumer};

pub fn subscribe(consumer: &StreamConsumer) {
    consumer.subscribe(&["orders"]).expect("subscribe");
}
