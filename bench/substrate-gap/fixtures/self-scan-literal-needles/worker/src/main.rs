use rdkafka::consumer::{Consumer, StreamConsumer};

pub fn start(consumer: &StreamConsumer) {
    consumer.subscribe(&["payments"]).expect("subscribe");
}
