use lapin::{options::BasicConsumeOptions, types::FieldTable, Channel};

pub async fn consume(channel: &Channel) {
    let _consumer = channel
        .basic_consume("orders", "worker", BasicConsumeOptions::default(), FieldTable::default())
        .await;
}
