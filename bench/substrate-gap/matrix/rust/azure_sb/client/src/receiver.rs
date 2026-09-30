use azservicebus::prelude::*;

pub async fn drain(conn: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut client = ServiceBusClient::new_from_connection_string(conn, ServiceBusClientOptions::default()).await?;
    let mut receiver = client.create_receiver_for_queue("orders", ServiceBusReceiverOptions::default()).await?;
    let messages = receiver.receive_messages(10).await?;
    for message in &messages {
        receiver.complete_message(message).await?;
    }
    Ok(())
}
